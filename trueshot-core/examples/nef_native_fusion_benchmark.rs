use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;
use trueshot_core::demosaic_ahd::ahd_demosaic_f32_owned;
use trueshot_core::export::{
    generate_output_path, save_depth_tiff, save_png, save_tiff16_from_f32,
};
use trueshot_core::native_fusion::{
    fuse_native_group, NativeFusionConfig, FUSION_FLAG_CENSORED, FUSION_FLAG_CENSOR_CONFLICT,
    FUSION_FLAG_OUTLIER_REJECTED, FUSION_FLAG_SOURCE_FALLBACK, FUSION_FLAG_UNCALIBRATED_NOISE,
};
use trueshot_core::postprocess::postprocess_f32;
use trueshot_core::smart_loader::{NativeGroupArena, SmartLoader};
use trueshot_core::timing::HierarchicalTimer;
use trueshot_core::types::ProcessingOptions;

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let input = arguments.next().map(PathBuf::from).context(
        "usage: nef_native_fusion_benchmark <nef_dir> [output_dir] \
             [--workers N] [--no-depth-refusion] [--no-frequency-deghost] \
             [--frequency-ablation] [--deghost-strength 0..2]",
    )?;
    let mut output = None;
    let mut workers = None;
    let mut depth_refusion = true;
    let mut frequency_deghost = true;
    let mut frequency_ablation = false;
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
        } else if argument == "--no-frequency-deghost" {
            frequency_deghost = false;
        } else if argument == "--frequency-ablation" {
            frequency_ablation = true;
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
    if frequency_ablation && !frequency_deghost {
        anyhow::bail!("--frequency-ablation conflicts with --no-frequency-deghost");
    }

    run(
        &input,
        output.as_deref(),
        workers,
        depth_refusion,
        frequency_deghost,
        frequency_ablation,
        deghost_strength,
    )
}

fn run(
    input: &Path,
    output: Option<&Path>,
    workers: Option<usize>,
    depth_refusion: bool,
    frequency_deghost: bool,
    frequency_ablation: bool,
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

        let fusion_config = NativeFusionConfig {
            depth_consistent_refusion: depth_refusion,
            frequency_separated_deghosting: frequency_deghost,
            deghost_strength,
            ..NativeFusionConfig::default()
        };
        let fusion_started = Instant::now();
        let fused = fuse_native_group(&group, &sequence.meta, &fusion_config)?;
        let fusion_elapsed = fusion_started.elapsed();
        let frequency_comparison = if frequency_ablation {
            let baseline_started = Instant::now();
            let baseline = fuse_native_group(
                &group,
                &sequence.meta,
                &NativeFusionConfig {
                    frequency_separated_deghosting: false,
                    ..fusion_config.clone()
                },
            )?;
            let baseline_elapsed = baseline_started.elapsed();
            let mut changed = 0usize;
            let mut absolute_sum = 0.0f64;
            let mut square_sum = 0.0f64;
            let mut maximum = 0.0f32;
            let mut peak = 1.0f32;
            for (protected, ordinary) in fused.bayer.iter().zip(&baseline.bayer) {
                let difference = (*protected - *ordinary).abs();
                changed += usize::from(protected.to_bits() != ordinary.to_bits());
                absolute_sum += f64::from(difference);
                square_sum += f64::from(difference * difference);
                maximum = maximum.max(difference);
                peak = peak.max(protected.abs()).max(ordinary.abs());
            }
            let samples = fused.bayer.len() as f64;
            let mae = absolute_sum / samples;
            let rmse = (square_sum / samples).sqrt();
            let psnr = if rmse > 0.0 {
                20.0 * (f64::from(peak) / rmse).log10()
            } else {
                f64::INFINITY
            };
            Some((baseline_elapsed, changed, mae, rmse, maximum, psnr))
        } else {
            None
        };
        drop(group);

        let fused_bytes = fused.size_bytes();
        let transforms = fused.transforms.clone();
        let frame_alignments = fused.frame_alignments.clone();
        let depth_refusion_pixels = fused.depth_refusion_pixels;
        let frequency_separated_pixels = fused.frequency_separated_pixels;
        let detail_single_source_pixels = fused.detail_single_source_pixels;
        let detail_reference_pixels = fused.detail_reference_pixels;
        let noise_model_calibrated = fused.noise_model_calibrated;
        let count_flag = |flag| {
            fused
                .fusion_flags
                .iter()
                .filter(|value| **value & flag != 0)
                .count()
        };
        let censored_pixels = count_flag(FUSION_FLAG_CENSORED);
        let censor_conflict_pixels = count_flag(FUSION_FLAG_CENSOR_CONFLICT);
        let rejected_pixels = count_flag(FUSION_FLAG_OUTLIER_REJECTED);
        let fallback_pixels = count_flag(FUSION_FLAG_SOURCE_FALLBACK);
        let uncalibrated_pixels = count_flag(FUSION_FLAG_UNCALIBRATED_NOISE);
        let mut depth_values: Vec<f32> = fused.depth.iter().copied().collect();
        let mut confidence_values: Vec<f32> = fused.confidence.iter().copied().collect();
        let mut uncertainty_values: Vec<f32> = fused
            .radiance_uncertainty
            .iter()
            .copied()
            .filter(|value| value.is_finite())
            .collect();
        depth_values.sort_unstable_by(f32::total_cmp);
        confidence_values.sort_unstable_by(f32::total_cmp);
        uncertainty_values.sort_unstable_by(f32::total_cmp);
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
        if uncertainty_values.is_empty() {
            println!("  radiance uncertainty: no finite posterior intervals");
        } else {
            println!(
                "  uncertainty p05={:.6} p50={:.6} p95={:.6} | calibrated={}",
                percentile(&uncertainty_values, 0.05),
                percentile(&uncertainty_values, 0.50),
                percentile(&uncertainty_values, 0.95),
                noise_model_calibrated,
            );
        }
        println!(
            "  evidence pixels: censored={censored_pixels}, conflicts={censor_conflict_pixels}, \
             rejected={rejected_pixels}, fallback={fallback_pixels}, \
             uncalibrated={uncalibrated_pixels}"
        );
        println!(
            "  frequency evidence: separated={frequency_separated_pixels}, \
             single-detail-source={detail_single_source_pixels}, \
             reference-detail={detail_reference_pixels}"
        );
        if let Some((elapsed, changed, mae, rmse, maximum, psnr)) = frequency_comparison {
            println!(
                "  paired frequency ablation: ordinary-fuse={:.2}s changed={changed}/{} \
                 ({:.3}%) MAE={mae:.8} RMSE={rmse:.8} max={maximum:.8} PSNR={psnr:.3}dB",
                elapsed.as_secs_f64(),
                linear_rgb.dim().0 * linear_rgb.dim().1,
                changed as f64 * 100.0 / (linear_rgb.dim().0 * linear_rgb.dim().1) as f64,
            );
        }
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
