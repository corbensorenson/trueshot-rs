//! Hierarchical processing pipeline
//!
//! Integrates grading and cascaded collapse for both speed and quality

use ndarray::{Array2, Array3};
use anyhow::{Result, Context};
use std::time::Instant;

use crate::hierarchical_grading::{
    compute_sharpness_map, grade_pixels, GradingParams, compute_grade_stats,
};
use crate::hierarchical_collapse::{
    collapse_c_grade, collapse_b_grade, collapse_a_grade,
    merge_graded_results,
    HierarchicalParams, CollapseResult,
};
use crate::types::AlignmentInfo;
use crate::joint_demosaic::demosaic_bayer_frame;


/// Process a stack of images using hierarchical grading and collapse
///
/// # Arguments
/// * `frames` - Slice of Bayer frames, each (H × W × 1)
/// * `foreground_mask` - Binary mask (true = object, false = background)
/// * `_reference_idx` - (Deprecated) Index of reference image for sharpness analysis
/// * `exposures` - Exposure values for each image
/// * `alignments` - Optional subpixel alignment data for SR
/// * `grading_params` - Grading parameters
/// * `collapse_params` - Collapse parameters
/// * `num_focus_planes` - Number of focus planes (for per-plane sharpness)
/// * `num_exposures` - Number of exposures per focus plane
/// * `wb_multipliers` - White balance multipliers [R, G1, G2, B] for Bayer pixels
///
/// # Returns
/// * Collapsed image (Bayer or RGB depending on whether SR was used)
///
/// # Memory Optimization
/// This function now accepts frames directly instead of a pre-stacked array,
/// eliminating the need to copy all frame data into a single Array3.
/// This saves ~33% memory (no bayer_stack duplication).
pub fn hierarchical_process(
    frames: &[Array3<f64>],
    foreground_mask: &Array2<bool>,
    _reference_idx: usize,
    exposures: &[f64],
    alignments: Option<&[AlignmentInfo]>,
    grading_params: &GradingParams,
    collapse_params: &HierarchicalParams,
    num_focus_planes: usize,
    num_exposures: usize,
    wb_multipliers: &[f32; 4],
) -> Result<CollapseResult> {
    let total_start = Instant::now();

    if frames.is_empty() {
        anyhow::bail!("No frames provided");
    }

    let num_images = frames.len();
    let (height, width, channels) = frames[0].dim();

    if channels != 1 {
        anyhow::bail!("Expected single-channel Bayer frames, got {} channels", channels);
    }

    tracing::info!(
        "Hierarchical processing: {}x{} pixels, {} images ({} focus planes × {} exposures), SR={:.3}x",
        width, height, num_images, num_focus_planes, num_exposures, collapse_params.sr_factor.as_f64()
    );

    // Step 1: Compute sharpness map per focus plane (not just reference!)
    let sharpness_start = Instant::now();

    // Compute sharpness for one reference exposure per focus plane
    let ref_exposure_idx = num_exposures / 2;  // Middle exposure (0 EV)
    let mut sharpness_maps: Vec<Array2<f64>> = Vec::with_capacity(num_focus_planes);

    for plane_idx in 0..num_focus_planes {
        let frame_idx = plane_idx * num_exposures + ref_exposure_idx;
        if frame_idx >= num_images {
            tracing::warn!("Focus plane {} reference frame {} out of bounds", plane_idx, frame_idx);
            continue;
        }

        // Extract frame (it's (H, W, 1), we need (H, W))
        let frame = frames[frame_idx].slice(ndarray::s![.., .., 0]).to_owned();
        let sharpness = compute_sharpness_map(&frame, grading_params)
            .context(format!("Failed to compute sharpness map for focus plane {}", plane_idx))?;
        sharpness_maps.push(sharpness);
    }

    tracing::info!("Sharpness analysis: {:.1}ms ({} focus planes)",
        sharpness_start.elapsed().as_secs_f64() * 1000.0, sharpness_maps.len());

    // Step 1b: Find maximum sharpness across all focus planes for each pixel
    let mut max_sharpness = Array2::<f64>::zeros((height, width));
    let mut best_plane = Array2::<usize>::zeros((height, width));

    for y in 0..height {
        for x in 0..width {
            let mut max_sharp = 0.0;
            let mut best_p = 0;

            for (plane_idx, sharpness_map) in sharpness_maps.iter().enumerate() {
                let sharp = sharpness_map[[y, x]];
                if sharp > max_sharp {
                    max_sharp = sharp;
                    best_p = plane_idx;
                }
            }

            max_sharpness[[y, x]] = max_sharp;
            best_plane[[y, x]] = best_p;
        }
    }

    tracing::info!("Per-pixel best focus plane selection complete");

    // Step 2: Grade pixels based on maximum sharpness across all focus planes
    let grading_start = Instant::now();
    let grades = grade_pixels(&max_sharpness, foreground_mask, grading_params)
        .context("Failed to grade pixels")?;
    let stats = compute_grade_stats(&grades);
    tracing::info!("Pixel grading: {:.1}ms", grading_start.elapsed().as_secs_f64() * 1000.0);
    tracing::info!(
        "Grade distribution: A={:.1}%, B={:.1}%, C={:.1}%, D={:.1}%",
        stats.percent_a, stats.percent_b, stats.percent_c, stats.percent_d
    );

    // Step 3: Cascaded collapse (multi-pass A/B/C grading)
    // NO STACKING REQUIRED! Work directly with frames to avoid memory duplication

    // C-grade: Process with hard focus plane selection (no soft blending)
    let c_start = Instant::now();
    let c_result = collapse_c_grade(
        frames,
        &grades,
        exposures,
        collapse_params,
        Some(&best_plane),  // Pass best focus plane map
        num_exposures,      // Pass number of exposures per plane
        wb_multipliers,     // Pass white balance multipliers
        alignments,         // Pass scale information for focus breathing compensation
    ).context("Failed to collapse C-grade")?;
    tracing::info!("C-grade collapse: {:.1}ms ({} pixels)",
        c_start.elapsed().as_secs_f64() * 1000.0, stats.count_c);

    // B-grade: Guided by C (with per-pixel focus plane selection and soft blending)
    let b_start = Instant::now();
    let b_result = collapse_b_grade(
        frames,
        &grades,
        &c_result,
        exposures,
        collapse_params,
        Some(&best_plane),  // Pass best focus plane map
        num_exposures,      // Pass number of exposures per plane
        wb_multipliers,     // Pass white balance multipliers
        alignments,         // Pass scale information for focus breathing compensation
    ).context("Failed to collapse B-grade")?;
    tracing::info!("B-grade collapse: {:.1}ms ({} pixels)",
        b_start.elapsed().as_secs_f64() * 1000.0, stats.count_b);

    // A-grade: Full quality, guided by B/C (with optional SR and per-pixel focus plane selection)
    let a_start = Instant::now();
    let a_result = collapse_a_grade(
        frames,
        &grades,
        &b_result,
        &c_result,
        exposures,
        collapse_params,
        alignments,
        Some(&best_plane),  // Pass best focus plane map
        num_exposures,      // Pass number of exposures per plane
        wb_multipliers,     // Pass white balance multipliers
    ).context("Failed to collapse A-grade")?;
    tracing::info!("A-grade collapse: {:.1}ms ({} pixels)",
        a_start.elapsed().as_secs_f64() * 1000.0, stats.count_a);

    // Step 4: Merge results
    let merge_start = Instant::now();
    let merged = merge_graded_results(&a_result, &b_result, &c_result, &grades)
        .context("Failed to merge graded results")?;
    tracing::info!("Merge: {:.1}ms", merge_start.elapsed().as_secs_f64() * 1000.0);

    // Step 5: Demosaic the final Bayer result
    let demosaic_start = Instant::now();
    let final_result = match merged {
        CollapseResult::Bayer(bayer) => {
            tracing::info!("Demosaicing final Bayer result");
            let rgb = demosaic_bayer_frame(&bayer.insert_axis(ndarray::Axis(2)), wb_multipliers)?;
            CollapseResult::Rgb(rgb.permuted_axes([2, 0, 1]))  // Convert (H, W, 3) to (3, H, W)
        }
        CollapseResult::Rgb(rgb) => {
            // Already RGB (from SR path)
            CollapseResult::Rgb(rgb)
        }
    };
    tracing::info!("Demosaic: {:.1}ms", demosaic_start.elapsed().as_secs_f64() * 1000.0);

    tracing::info!(
        "Hierarchical processing complete: {:.1}ms total",
        total_start.elapsed().as_secs_f64() * 1000.0
    );

    Ok(final_result)
}

/// Compare hierarchical vs standard processing
/// Returns (hierarchical_result, standard_result, timing_comparison)
pub fn benchmark_hierarchical(
    frames: &[Array3<f64>],
    foreground_mask: &Array2<bool>,
    reference_idx: usize,
    exposures: &[f64],
) -> Result<(Array2<f64>, Array2<f64>, BenchmarkResults)> {
    // Hierarchical processing (native resolution for fair comparison)
    let hier_start = Instant::now();
    let grading_params = GradingParams::default();
    let collapse_params = HierarchicalParams::default();

    // For benchmark, assume simple pattern: all frames are exposures (1 focus plane)
    let num_focus_planes = 1;
    let num_exposures = frames.len();

    // Use neutral white balance for benchmark (all 1.0)
    let wb_multipliers = [1.0f32, 1.0f32, 1.0f32, 1.0f32];

    let hier_collapse_result = hierarchical_process(
        frames,
        foreground_mask,
        reference_idx,
        exposures,
        None,  // No alignments for benchmark
        &grading_params,
        &collapse_params,
        num_focus_planes,
        num_exposures,
        &wb_multipliers,  // Neutral WB for benchmark
    )?;
    let hier_time = hier_start.elapsed();

    // Extract Bayer from result (benchmark uses native resolution, so it's always Bayer)
    let hier_result = match hier_collapse_result {
        CollapseResult::Bayer(bayer) => bayer,
        CollapseResult::Rgb(_) => {
            anyhow::bail!("Unexpected RGB result in benchmark (should be Bayer at native resolution)");
        }
    };

    // Standard processing (simple weighted average)
    let std_start = Instant::now();
    let std_result = standard_collapse_frames(frames)?;
    let std_time = std_start.elapsed();

    let results = BenchmarkResults {
        hierarchical_ms: hier_time.as_secs_f64() * 1000.0,
        standard_ms: std_time.as_secs_f64() * 1000.0,
        speedup: std_time.as_secs_f64() / hier_time.as_secs_f64(),
    };

    tracing::info!(
        "Benchmark: Hierarchical={:.1}ms, Standard={:.1}ms, Speedup={:.2}x",
        results.hierarchical_ms, results.standard_ms, results.speedup
    );

    Ok((hier_result, std_result, results))
}

#[derive(Debug, Clone)]
pub struct BenchmarkResults {
    pub hierarchical_ms: f64,
    pub standard_ms: f64,
    pub speedup: f64,
}

/// Standard collapse (baseline for comparison) - accepts frames
fn standard_collapse_frames(frames: &[Array3<f64>]) -> Result<Array2<f64>> {
    let (height, width, _) = frames[0].dim();
    let num_images = frames.len();

    let mut result = Array2::<f64>::zeros((height, width));

    // Simple average
    for frame in frames.iter() {
        for y in 0..height {
            for x in 0..width {
                result[[y, x]] += frame[[y, x, 0]];
            }
        }
    }

    for y in 0..height {
        for x in 0..width {
            result[[y, x]] /= num_images as f64;
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array2;

    fn create_test_stack(width: usize, height: usize, frames: usize) -> Array3<f64> {
        let mut stack = Array3::<f64>::zeros((height, width, frames));
        for n in 0..frames {
            let phase = n as f64 / (frames.max(1) as f64);
            for y in 0..height {
                for x in 0..width {
                    let val = (x as f64 / width as f64 * 0.5)
                        + (y as f64 / height as f64 * 0.5)
                        + phase * 0.1;
                    stack[[y, x, n]] = val.clamp(0.0, 1.0);
                }
            }
        }
        stack
    }

    #[test]
    fn test_create_test_stack() {
        let stack = create_test_stack(100, 100, 3);
        assert_eq!(stack.dim(), (100, 100, 3));

        // Check values are in valid range
        for n in 0..3 {
            for y in 0..100 {
                for x in 0..100 {
                    let val = stack[[y, x, n]];
                    assert!(val >= 0.0 && val <= 1.0, "Value {} out of range", val);
                }
            }
        }
    }

    #[test]
    fn test_hierarchical_process_basic() {
        // Create synthetic stack and convert to frames
        let stack = create_test_stack(100, 100, 5);

        // Convert stack to frames (each frame is H × W × 1)
        let mut frames = Vec::new();
        for n in 0..5 {
            let mut frame = Array3::<f64>::zeros((100, 100, 1));
            for y in 0..100 {
                for x in 0..100 {
                    frame[[y, x, 0]] = stack[[y, x, n]];
                }
            }
            frames.push(frame);
        }

        let mask = Array2::<bool>::from_elem((100, 100), true);
        let exposures = vec![1.0, 2.0, 3.0, 4.0, 5.0];

        let grading_params = GradingParams::default();
        let collapse_params = HierarchicalParams::default();

        // Test: 5 frames = 1 focus plane × 5 exposures
        let result = hierarchical_process(
            &frames,
            &mask,
            2,  // middle image as reference
            &exposures,
            None,  // No alignments (native resolution)
            &grading_params,
            &collapse_params,
            1,  // num_focus_planes
            5,  // num_exposures
            &[1.0, 1.0, 1.0, 1.0],
        ).unwrap();

        // Result is CollapseResult, extract Bayer data
        match result {
            CollapseResult::Bayer(bayer) => {
                assert_eq!(bayer.dim(), (100, 100));
                // Result should be in valid range
                for y in 0..100 {
                    for x in 0..100 {
                        let val = bayer[[y, x]];
                        assert!(val >= 0.0 && val <= 1.0, "Value {} out of range at ({}, {})", val, y, x);
                    }
                }
            }
            CollapseResult::Rgb(_) => panic!("Expected Bayer result, got RGB"),
        }
    }

    #[test]
    fn test_hierarchical_process_with_sr() {
        use crate::hierarchical_collapse::SRFactor;

        // Create synthetic stack and convert to frames
        let stack = create_test_stack(50, 50, 5);

        // Convert stack to frames
        let mut frames = Vec::new();
        for n in 0..5 {
            let mut frame = Array3::<f64>::zeros((50, 50, 1));
            for y in 0..50 {
                for x in 0..50 {
                    frame[[y, x, 0]] = stack[[y, x, n]];
                }
            }
            frames.push(frame);
        }

        let mask = Array2::<bool>::from_elem((50, 50), true);
        let exposures = vec![1.0, 2.0, 3.0, 4.0, 5.0];

        // Create synthetic alignments (small subpixel shifts)
        let alignments: Vec<AlignmentInfo> = (0..5).map(|i| {
            AlignmentInfo {
                dx: (i as f64 - 2.0) * 0.1,  // -0.2, -0.1, 0.0, 0.1, 0.2
                dy: (i as f64 - 2.0) * 0.15,
                scale: 1.0,
            }
        }).collect();

        let grading_params = GradingParams::default();
        let mut collapse_params = HierarchicalParams::default();
        collapse_params.sr_factor = SRFactor::SR2x;

        // Test: 5 frames = 1 focus plane × 5 exposures
        let result = hierarchical_process(
            &frames,
            &mask,
            2,
            &exposures,
            Some(&alignments),
            &grading_params,
            &collapse_params,
            1,  // num_focus_planes
            5,  // num_exposures
            &[1.0, 1.0, 1.0, 1.0],
        ).unwrap();

        // Result should be RGB (SR path)
        match result {
            CollapseResult::Rgb(rgb) => {
                let (channels, h, w) = rgb.dim();
                assert_eq!(channels, 3);
                assert_eq!(h, 100);  // 2x upsampled
                assert_eq!(w, 100);
            }
            CollapseResult::Bayer(_) => panic!("Expected RGB result for SR, got Bayer"),
        }
    }

    #[test]
    fn test_benchmark() {
        let stack = create_test_stack(50, 50, 3);

        // Convert stack to frames
        let mut frames = Vec::new();
        for n in 0..3 {
            let mut frame = Array3::<f64>::zeros((50, 50, 1));
            for y in 0..50 {
                for x in 0..50 {
                    frame[[y, x, 0]] = stack[[y, x, n]];
                }
            }
            frames.push(frame);
        }

        let mask = Array2::<bool>::from_elem((50, 50), true);
        let exposures = vec![1.0, 2.0, 3.0];

        let (hier_result, std_result, bench) = benchmark_hierarchical(
            &frames,
            &mask,
            1,
            &exposures,
        ).unwrap();

        assert_eq!(hier_result.dim(), (50, 50));
        assert_eq!(std_result.dim(), (50, 50));

        // Both should produce valid results
        assert!(bench.hierarchical_ms > 0.0);
        assert!(bench.standard_ms > 0.0);

        println!("Benchmark: Hierarchical={:.1}ms, Standard={:.1}ms, Speedup={:.2}x",
            bench.hierarchical_ms, bench.standard_ms, bench.speedup);
    }

    #[test]
    fn test_sr_upsample() {
        let input = Array2::<f64>::from_shape_fn((10, 10), |(y, x)| {
            (y as f64 / 10.0 + x as f64 / 10.0) / 2.0
        });

        let upsampled = upsample_bilinear(&input, 2).unwrap();
        assert_eq!(upsampled.dim(), (20, 20));

        // Check values are in valid range
        for y in 0..20 {
            for x in 0..20 {
                let val = upsampled[[y, x]];
                assert!(val >= 0.0 && val <= 1.0);
            }
        }
    }

    fn upsample_bilinear(input: &Array2<f64>, factor: usize) -> Result<Array2<f64>> {
        let (h, w) = input.dim();
        if factor == 0 {
            return Err(anyhow::anyhow!("factor must be > 0"));
        }
        let out_h = h * factor;
        let out_w = w * factor;
        let mut output = Array2::<f64>::zeros((out_h, out_w));

        for y in 0..out_h {
            let src_y = (y as f64 + 0.5) / factor as f64 - 0.5;
            let y0 = src_y.floor().max(0.0) as isize;
            let y1 = (y0 + 1).min(h as isize - 1);
            let wy = src_y - y0 as f64;
            for x in 0..out_w {
                let src_x = (x as f64 + 0.5) / factor as f64 - 0.5;
                let x0 = src_x.floor().max(0.0) as isize;
                let x1 = (x0 + 1).min(w as isize - 1);
                let wx = src_x - x0 as f64;

                let v00 = input[[y0 as usize, x0 as usize]];
                let v01 = input[[y0 as usize, x1 as usize]];
                let v10 = input[[y1 as usize, x0 as usize]];
                let v11 = input[[y1 as usize, x1 as usize]];

                let v0 = v00 * (1.0 - wx) + v01 * wx;
                let v1 = v10 * (1.0 - wx) + v11 * wx;
                output[[y, x]] = v0 * (1.0 - wy) + v1 * wy;
            }
        }

        Ok(output)
    }
}

/// Apply median filter to focus plane map to smooth transitions
///
/// This reduces speckle artifacts from hard focus plane boundaries.
fn median_filter_focus_plane(plane_map: &Array2<usize>, kernel_size: usize) -> Array2<usize> {
    let (height, width) = plane_map.dim();
    let mut filtered = plane_map.clone();
    let radius = kernel_size / 2;

    for y in 0..height {
        for x in 0..width {
            // Collect neighboring values
            let mut neighbors = Vec::new();
            for dy in -(radius as isize)..=(radius as isize) {
                for dx in -(radius as isize)..=(radius as isize) {
                    let ny = (y as isize + dy).max(0).min(height as isize - 1) as usize;
                    let nx = (x as isize + dx).max(0).min(width as isize - 1) as usize;
                    neighbors.push(plane_map[[ny, nx]]);
                }
            }

            // Compute median
            neighbors.sort_unstable();
            filtered[[y, x]] = neighbors[neighbors.len() / 2];
        }
    }

    filtered
}
