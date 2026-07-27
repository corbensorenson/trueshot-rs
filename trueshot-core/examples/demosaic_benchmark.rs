use anyhow::{Context, Result};
use ndarray::Array3;
use rayon::ThreadPoolBuilder;
use std::time::Instant;
use trueshot_core::demosaic_ahd::ahd_demosaic_f32_owned;

fn main() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let width = arguments
        .next()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(1310usize);
    let height = arguments
        .next()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(1304usize);
    let threads = arguments
        .next()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or_else(num_cpus::get);
    if width == 0 || height == 0 || threads == 0 {
        anyhow::bail!("usage: demosaic_benchmark [width height threads]");
    }

    let (bayer, ground_truth) = synthetic_scene(width, height)?;
    let pool = ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .context("Unable to create isolated AHD benchmark pool")?;
    let started = Instant::now();
    let rgb = pool.install(|| {
        ahd_demosaic_f32_owned(
            bayer,
            &[
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
        )
    })?;
    let elapsed = started.elapsed();
    let megapixels = width as f64 * height as f64 / 1_000_000.0;
    let checksum = rgb
        .iter()
        .enumerate()
        .fold(0u64, |accumulator, (index, value)| {
            accumulator.wrapping_add(u64::from(value.to_bits()).wrapping_mul(index as u64 + 1))
        });
    let border = 5usize.min(width / 2).min(height / 2);
    let mut squared_error = [0.0f64; 3];
    let mut sample_count = 0usize;
    for y in border..height.saturating_sub(border) {
        for x in border..width.saturating_sub(border) {
            for channel in 0..3 {
                let error = f64::from(rgb[[y, x, channel]] - ground_truth[[y, x, channel]]);
                squared_error[channel] += error * error;
            }
            sample_count += 1;
        }
    }
    let psnr = squared_error.map(|sum| {
        let mse = sum / sample_count.max(1) as f64;
        if mse <= f64::EPSILON {
            f64::INFINITY
        } else {
            -10.0 * mse.log10()
        }
    });
    println!(
        "AHD {}x{} threads={} elapsed_ms={:.3} throughput_mpix_s={:.3} \
         psnr_rgb_db={:.3}/{:.3}/{:.3} checksum={:016x}",
        width,
        height,
        threads,
        elapsed.as_secs_f64() * 1000.0,
        megapixels / elapsed.as_secs_f64(),
        psnr[0],
        psnr[1],
        psnr[2],
        checksum
    );
    Ok(())
}

fn synthetic_scene(width: usize, height: usize) -> Result<(Array3<f32>, Array3<f32>)> {
    let pixels = width
        .checked_mul(height)
        .context("Synthetic Bayer dimensions overflow")?;
    let mut bayer = Vec::with_capacity(pixels);
    let mut ground_truth = Vec::with_capacity(
        pixels
            .checked_mul(3)
            .context("Synthetic RGB dimensions overflow")?,
    );
    for y in 0..height {
        for x in 0..width {
            let horizontal = x as f32 / width as f32;
            let vertical = y as f32 / height as f32;
            let edge = if (x / 37 + y / 53) & 1 == 0 {
                0.18
            } else {
                0.72
            };
            let base = edge + 0.08 * (horizontal * 31.0).sin() + 0.06 * vertical;
            let rgb = [
                (base * 1.07 + 0.025 * (vertical * 17.0).cos()).clamp(0.0, 1.0),
                base.clamp(0.0, 1.0),
                (base * 0.91 + 0.03 * (horizontal * 13.0).cos()).clamp(0.0, 1.0),
            ];
            ground_truth.extend_from_slice(&rgb);
            let channel = match ((y & 1) << 1) | (x & 1) {
                0 => 0,
                1 | 2 => 1,
                3 => 2,
                _ => unreachable!(),
            };
            bayer.push(rgb[channel]);
        }
    }
    Ok((
        Array3::from_shape_vec((height, width, 1), bayer)
            .context("Unable to shape synthetic Bayer benchmark")?,
        Array3::from_shape_vec((height, width, 3), ground_truth)
            .context("Unable to shape synthetic RGB benchmark")?,
    ))
}
