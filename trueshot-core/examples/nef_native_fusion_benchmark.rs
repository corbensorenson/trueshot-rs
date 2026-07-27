use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;
use trueshot_core::demosaic_ahd::ahd_demosaic_f32_owned;
use trueshot_core::export::{
    generate_output_path, save_depth_tiff, save_png, save_tiff16_from_f32,
};
use trueshot_core::native_fusion::{fuse_native_group, NativeFusionConfig};
use trueshot_core::postprocess::postprocess_f32;
use trueshot_core::smart_loader::{NativeGroupArena, SmartLoader};
use trueshot_core::timing::HierarchicalTimer;
use trueshot_core::types::ProcessingOptions;

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let input = arguments.next().map(PathBuf::from).context(
        "usage: nef_native_fusion_benchmark <nef_dir> [output_dir] \
             [--workers N] [--no-depth-refusion] [--deghost-strength 0..2]",
    )?;
    let mut output = None;
    let mut workers = None;
    let mut depth_refusion = true;
    let mut deghost_strength = 1.0;
    while let Some(argument) = arguments.next() {
        if argument == "--workers" {
            workers = Some(
                arguments
                    .next()
                    .context("--workers requires a positive integer")?
                    .to_string_lossy()
                    .parse::<usize>()
                    .context("--workers must be a positive integer")?,
            );
            if workers == Some(0) {
                anyhow::bail!("--workers must be positive");
            }
        } else if argument == "--no-depth-refusion" {
            depth_refusion = false;
        } else if argument == "--deghost-strength" {
            deghost_strength = arguments
                .next()
                .context("--deghost-strength requires a value from 0 to 2")?
                .to_string_lossy()
                .parse::<f32>()
                .context("--deghost-strength must be numeric")?;
            if !(0.0..=2.0).contains(&deghost_strength) {
                anyhow::bail!("--deghost-strength must be between 0 and 2");
            }
        } else if argument.to_string_lossy().starts_with('-') {
            anyhow::bail!("Unknown option {}", argument.to_string_lossy());
        } else if output.is_none() {
            output = Some(PathBuf::from(argument));
        } else {
            anyhow::bail!("Only one output directory may be supplied");
        }
    }

    run(
        &input,
        output.as_deref(),
        workers,
        depth_refusion,
        deghost_strength,
    )
}

fn run(
    input: &Path,
    output: Option<&Path>,
    workers: Option<usize>,
    depth_refusion: bool,
    deghost_strength: f32,
) -> Result<()> {
    if let Some(output) = output {
        std::fs::create_dir_all(output)?;
    }
    let options = ProcessingOptions {
        max_parallel_sequences: workers,
        full_decode: false,
        verbose_timing: false,
        ..Default::default()
    };
    let loader = SmartLoader::new(options);
    let scan_started = Instant::now();
    let sequences = loader.scan_and_group(input)?;
    println!(
        "scan: {} groups in {:.2} ms",
        sequences.len(),
        scan_started.elapsed().as_secs_f64() * 1000.0
    );

    let mut arena = NativeGroupArena::default();
    for (sequence_index, sequence) in sequences.iter().enumerate() {
        let mut timer = HierarchicalTimer::new(&sequence.meta.bone_id);
        let decode_started = Instant::now();
        let group = loader.load_sequence_native_into(sequence, &mut arena, &mut timer)?;
        let decode_elapsed = decode_started.elapsed();
        let input_bytes = group.size_bytes();

        let fusion_started = Instant::now();
        let fused = fuse_native_group(
            &group,
            &sequence.meta,
            &NativeFusionConfig {
                depth_consistent_refusion: depth_refusion,
                deghost_strength,
                ..NativeFusionConfig::default()
            },
        )?;
        let fusion_elapsed = fusion_started.elapsed();
        drop(group);

        let fused_bytes = fused.size_bytes();
        let transforms = fused.transforms.clone();
        let frame_alignments = fused.frame_alignments.clone();
        let depth_refusion_pixels = fused.depth_refusion_pixels;
        let mut depth_values: Vec<f32> = fused.depth.iter().copied().collect();
        let mut confidence_values: Vec<f32> = fused.confidence.iter().copied().collect();
        depth_values.sort_unstable_by(f32::total_cmp);
        confidence_values.sort_unstable_by(f32::total_cmp);
        let percentile = |values: &[f32], fraction: f32| {
            values[((values.len() - 1) as f32 * fraction).round() as usize]
        };
        let demosaic_started = Instant::now();
        let linear_rgb = ahd_demosaic_f32_owned(
            fused.bayer,
            &[
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
        )?;
        let display_rgb = postprocess_f32(&linear_rgb)?;
        let demosaic_elapsed = demosaic_started.elapsed();

        let export_started = Instant::now();
        if let Some(output) = output {
            let output_path = generate_output_path(
                output,
                &sequence.meta.bone_id,
                &sequence.meta.vantage,
                sequence.meta.rot_deg,
            );
            save_tiff16_from_f32(&linear_rgb, &fused.foreground_mask, &output_path)?;
            save_png(
                &display_rgb,
                &fused.foreground_mask,
                &output_path.with_extension("png"),
            )?;
            save_depth_tiff(
                &fused.depth,
                &output_path.with_file_name(format!(
                    "{}_{}_{}deg_depth.tiff",
                    sequence.meta.bone_id, sequence.meta.vantage, sequence.meta.rot_deg as u32
                )),
            )?;
        }
        let export_elapsed = export_started.elapsed();
        let accepted = transforms
            .iter()
            .filter(|transform| transform.accepted)
            .count();
        let bracket_frames = frame_alignments
            .iter()
            .filter(|alignment| !alignment.reference_frame)
            .count();
        let accepted_brackets = frame_alignments
            .iter()
            .filter(|alignment| !alignment.reference_frame && alignment.global_accepted)
            .count();
        let local_aligned_cells: u32 = frame_alignments
            .iter()
            .map(|alignment| alignment.local_aligned_cells)
            .sum();
        let disoccluded_cells: u32 = frame_alignments
            .iter()
            .map(|alignment| alignment.disoccluded_cells)
            .sum();
        println!(
            "group {}/{}: {} frames, {}x{}, native {:.2} MiB, fused {:.2} MiB",
            sequence_index + 1,
            sequences.len(),
            sequence.len(),
            linear_rgb.dim().1,
            linear_rgb.dim().0,
            input_bytes as f64 / (1024.0 * 1024.0),
            fused_bytes as f64 / (1024.0 * 1024.0),
        );
        println!(
            "  decode {:.2}s | fuse {:.2}s | demosaic/display {:.2}s | export {:.2}s | transforms {}/{}",
            decode_elapsed.as_secs_f64(),
            fusion_elapsed.as_secs_f64(),
            demosaic_elapsed.as_secs_f64(),
            export_elapsed.as_secs_f64(),
            accepted,
            transforms.len(),
        );
        println!(
            "  depth p05={:.3} p50={:.3} p95={:.3} | confidence p05={:.3} p50={:.3} p95={:.3}",
            percentile(&depth_values, 0.05),
            percentile(&depth_values, 0.50),
            percentile(&depth_values, 0.95),
            percentile(&confidence_values, 0.05),
            percentile(&confidence_values, 0.50),
            percentile(&confidence_values, 0.95),
        );
        println!(
            "  depth-consistent refusion: {} / {} pixels ({:.2}%)",
            depth_refusion_pixels,
            linear_rgb.dim().0 * linear_rgb.dim().1,
            depth_refusion_pixels as f64 * 100.0 / (linear_rgb.dim().0 * linear_rgb.dim().1) as f64,
        );
        println!(
            "  bracket alignment: {accepted_brackets}/{bracket_frames} global accepted, \
             {local_aligned_cells} local cells, {disoccluded_cells} disoccluded cells"
        );
        for (index, transform) in transforms.iter().enumerate() {
            println!(
                "  F{index}: dx={:.3} dy={:.3} scale={:.5} ncc={:.3} accepted={}",
                transform.shift_x,
                transform.shift_y,
                transform.source_scale,
                transform.quality,
                transform.accepted,
            );
        }
    }
    Ok(())
}
