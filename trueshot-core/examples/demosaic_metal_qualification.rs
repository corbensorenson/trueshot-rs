use anyhow::{Context, Result};
use ndarray::Array3;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use trueshot_core::demosaic_ahd::{ahd_demosaic_f32_owned, ahd_normalization_scale};
use trueshot_core::gpu::{GpuAhdEngine, GpuContext, METAL_AHD_RELEASE_QUALIFICATION_TOLERANCE};

const RGB_CAM: [[f32; 4]; 3] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

fn main() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let width = parse_or(&mut arguments, 1310)?;
    let height = parse_or(&mut arguments, 1304)?;
    let measured_runs = parse_or(&mut arguments, 5)?;
    let input_multiplier = parse_f32_or(&mut arguments, 1.0)?;
    if width < 32 || height < 32 || measured_runs < 3 {
        anyhow::bail!(
            "usage: demosaic_metal_qualification [width>=32 height>=32 measured_runs>=3 input_multiplier>0]"
        );
    }
    if !input_multiplier.is_finite() || input_multiplier <= 0.0 {
        anyhow::bail!("input multiplier must be finite and positive");
    }

    let context = GpuContext::new_blocking()?.context("No qualified GPU context is available")?;
    let engine = GpuAhdEngine::new(Arc::new(context))?
        .context("The active adapter is not a qualified Apple Metal adapter")?;
    let mut bayer = synthetic_bayer(width, height)?;
    bayer.mapv_inplace(|value| value * input_multiplier);
    let normalization_scale =
        ahd_normalization_scale(bayer.iter().copied().fold(1.0f32, f32::max))?;
    let absolute_tolerance = METAL_AHD_RELEASE_QUALIFICATION_TOLERANCE * normalization_scale;
    let scratch_bytes = engine
        .scratch_bytes(width, height)?
        .context("Workload is below the production Metal threshold")?;

    // Warm both implementations before alternating measurements.
    let _ = ahd_demosaic_f32_owned(bayer.clone(), &RGB_CAM)?;
    let _ = engine
        .demosaic(&bayer, &RGB_CAM)?
        .context("Metal workload was rejected during warmup")?;

    let mut cpu_times = Vec::with_capacity(measured_runs);
    let mut gpu_times = Vec::with_capacity(measured_runs);
    let mut cpu_output = None;
    let mut gpu_output = None;
    for iteration in 0..measured_runs {
        if iteration & 1 == 0 {
            let (elapsed, output) = measure(|| ahd_demosaic_f32_owned(bayer.clone(), &RGB_CAM))?;
            cpu_times.push(elapsed);
            cpu_output = Some(output);
            let (elapsed, output) = measure(|| {
                engine
                    .demosaic(&bayer, &RGB_CAM)?
                    .context("Metal workload was rejected")
            })?;
            gpu_times.push(elapsed);
            gpu_output = Some(output);
        } else {
            let (elapsed, output) = measure(|| {
                engine
                    .demosaic(&bayer, &RGB_CAM)?
                    .context("Metal workload was rejected")
            })?;
            gpu_times.push(elapsed);
            gpu_output = Some(output);
            let (elapsed, output) = measure(|| ahd_demosaic_f32_owned(bayer.clone(), &RGB_CAM))?;
            cpu_times.push(elapsed);
            cpu_output = Some(output);
        }
    }

    let cpu = cpu_output.context("CPU qualification produced no output")?;
    let gpu = gpu_output.context("Metal qualification produced no output")?;
    let mut maximum_error = 0.0f32;
    let mut squared_error = 0.0f64;
    let mut values_over_tolerance = 0usize;
    let mut mismatches = Vec::new();
    for (index, (left, right)) in cpu.iter().zip(gpu.image.iter()).enumerate() {
        let error = (*left - *right).abs();
        let normalized_error = error / normalization_scale;
        maximum_error = maximum_error.max(error);
        squared_error += f64::from(normalized_error) * f64::from(normalized_error);
        values_over_tolerance += usize::from(error > absolute_tolerance);
        if error > absolute_tolerance && mismatches.len() < 16 {
            let pixel = index / 3;
            mismatches.push(json!({
                "x": pixel % width,
                "y": pixel / width,
                "channel": index % 3,
                "cpu": left,
                "metal": right,
                "absolute_error": error,
            }));
        }
    }
    let mse = squared_error / cpu.len() as f64;
    let parity_psnr_db = if mse == 0.0 {
        f64::INFINITY
    } else {
        -10.0 * mse.log10()
    };
    let cpu_p50 = percentile_ms(&cpu_times, 0.50);
    let cpu_p95 = percentile_ms(&cpu_times, 0.95);
    let gpu_p50 = percentile_ms(&gpu_times, 0.50);
    let gpu_p95 = percentile_ms(&gpu_times, 0.95);
    let artifact = json!({
        "schema": "trueshot.demosaic-metal-qualification.v1",
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "adapter": gpu.adapter,
        "width": width,
        "height": height,
        "measured_runs": measured_runs,
        "input_multiplier": input_multiplier,
        "normalization_scale": normalization_scale,
        "bands": gpu.bands,
        "scratch_bytes": scratch_bytes,
        "cpu": {
            "p50_ms": cpu_p50,
            "p95_ms": cpu_p95,
            "checksum": checksum(&cpu),
        },
        "metal": {
            "p50_ms": gpu_p50,
            "p95_ms": gpu_p95,
            "checksum": checksum(&gpu.image),
        },
        "speedup": {
            "p50": cpu_p50 / gpu_p50,
            "p95": cpu_p95 / gpu_p95,
        },
        "parity": {
            "maximum_absolute_error": maximum_error,
            "maximum_normalized_error": maximum_error / normalization_scale,
            "psnr_db": parity_psnr_db,
            "normalized_tolerance": METAL_AHD_RELEASE_QUALIFICATION_TOLERANCE,
            "values_over_tolerance": values_over_tolerance,
            "measured_cfa_exact": measured_cfa_exact(&bayer, &gpu.image),
            "first_mismatches": mismatches,
        }
    });
    println!("{}", serde_json::to_string_pretty(&artifact)?);

    if maximum_error > absolute_tolerance
        || values_over_tolerance != 0
        || !measured_cfa_exact(&bayer, &gpu.image)
    {
        anyhow::bail!("Metal AHD failed the retained parity contract");
    }
    Ok(())
}

fn parse_or(arguments: &mut impl Iterator<Item = String>, default: usize) -> Result<usize> {
    arguments
        .next()
        .map(|value| value.parse().context("Invalid integer argument"))
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_f32_or(arguments: &mut impl Iterator<Item = String>, default: f32) -> Result<f32> {
    arguments
        .next()
        .map(|value| value.parse().context("Invalid floating-point argument"))
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn synthetic_bayer(width: usize, height: usize) -> Result<Array3<f32>> {
    let pixels = width
        .checked_mul(height)
        .context("Synthetic Bayer dimensions overflow")?;
    let mut values = Vec::with_capacity(pixels);
    for y in 0..height {
        for x in 0..width {
            let edge = if (x / 37 + y / 53) & 1 == 0 {
                0.18
            } else {
                0.72
            };
            let channel_scale = match cfa_channel(y, x) {
                0 => 1.07,
                1 => 1.0,
                2 => 0.91,
                _ => unreachable!(),
            };
            values.push(
                (edge * channel_scale
                    + 0.08 * (x as f32 * 0.031).sin()
                    + 0.06 * (y as f32 * 0.017).cos())
                .clamp(0.0, 1.0),
            );
        }
    }
    Array3::from_shape_vec((height, width, 1), values)
        .context("Unable to shape synthetic Bayer qualification input")
}

fn measure<T>(operation: impl FnOnce() -> Result<T>) -> Result<(Duration, T)> {
    let started = Instant::now();
    let output = operation()?;
    Ok((started.elapsed(), output))
}

fn percentile_ms(samples: &[Duration], percentile: f64) -> f64 {
    let mut samples = samples.to_vec();
    samples.sort_unstable();
    let index = ((samples.len() - 1) as f64 * percentile).ceil() as usize;
    samples[index].as_secs_f64() * 1000.0
}

fn checksum(image: &Array3<f32>) -> String {
    format!(
        "{:016x}",
        image
            .iter()
            .enumerate()
            .fold(0u64, |accumulator, (index, value)| {
                accumulator.wrapping_add(u64::from(value.to_bits()).wrapping_mul(index as u64 + 1))
            })
    )
}

fn measured_cfa_exact(bayer: &Array3<f32>, rgb: &Array3<f32>) -> bool {
    let (height, width, _) = bayer.dim();
    (0..height).all(|y| (0..width).all(|x| rgb[[y, x, cfa_channel(y, x)]] == bayer[[y, x, 0]]))
}

fn cfa_channel(row: usize, column: usize) -> usize {
    match ((row & 1) << 1) | (column & 1) {
        0 => 0,
        1 | 2 => 1,
        3 => 2,
        _ => unreachable!(),
    }
}
