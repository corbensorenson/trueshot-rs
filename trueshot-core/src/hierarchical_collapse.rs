//! Hierarchical cascaded collapse: C → B → A
//!
//! Implements three-tier processing:
//! - C-grade: Fast baseline collapse (weighted average)
//! - B-grade: Guided refinement using C as prior
//! - A-grade: Full quality with optional SR, guided by B/C
//!
//! This achieves both speed (amortized computation) and quality (guided priors)

use anyhow::Result;
use ndarray::{s, Array2, Array3};

use crate::hierarchical_grading::Grade;
use crate::joint_demosaic::{joint_demosaic_with_focus_selection, PixelSelection};
use crate::types::AlignmentInfo;

// Removed: bayer_bilinear_2x2 and sample_pixel_safe (unused after disabling scale compensation)

/// Result of A-grade collapse - can be either Bayer or RGB
#[derive(Debug, Clone)]
pub enum CollapseResult {
    /// Bayer format (grayscale, needs demosaicing)
    Bayer(Array2<f64>),
    /// RGB format (already demosaiced, 3 x H x W)
    Rgb(Array3<f64>),
}

/// Collapse all foreground pixels (A+B+C) in a single pass
///
/// This eliminates grade boundary artifacts by treating all foreground pixels uniformly.
/// Uses per-pixel focus plane selection + HDR fusion + scale-aware sampling.
pub fn collapse_foreground_single_pass(
    frames: &[Array3<f64>], // Vec of frames, each (H, W, 1)
    grades: &Array2<u8>,
    _exposures: &[f64],
    _params: &HierarchicalParams,
    best_plane_map: Option<&Array2<usize>>, // Per-pixel best focus plane
    num_exposures: usize,                   // Number of exposures per focus plane
    _wb_multipliers: &[f32; 4],             // Unused - demosaicing happens later
    alignments: Option<&[AlignmentInfo]>,   // Scale information for focus breathing compensation
) -> Result<Array2<f64>> {
    // Returns BAYER (H×W), not RGB
    if frames.is_empty() {
        anyhow::bail!("No frames provided");
    }

    let (height, width, _) = frames[0].dim();
    let num_images = frames.len();
    let mut result = Array2::<f64>::zeros((height, width));

    // Collect all foreground pixel coordinates (A+B+C grades)
    let fg_pixels: Vec<(usize, usize)> = grades
        .indexed_iter()
        .filter(|(_, &g)| g == Grade::A as u8 || g == Grade::B as u8 || g == Grade::C as u8)
        .map(|((y, x), _)| (y, x))
        .collect();

    tracing::info!(
        "Collapsing {} foreground pixels (A+B+C) in single pass",
        fg_pixels.len()
    );

    // Check if we have focus plane information
    if let Some(best_plane) = best_plane_map {
        // For HDR: Use exposure-aware weights instead of Mertens
        // Prefer exposures that are well-exposed (not clipped, not too dark)
        // This avoids the problem of Mertens weights being too skewed for HDR
        let image_weights: Vec<f64> = frames
            .iter()
            .map(|frame| {
                // Compute mean intensity (downsampled for speed)
                let step = 8;
                let mut sum = 0.0;
                let mut count = 0;
                for y in (0..height).step_by(step) {
                    for x in (0..width).step_by(step) {
                        sum += frame[[y, x, 0]];
                        count += 1;
                    }
                }
                let mean = if count > 0 { sum / count as f64 } else { 0.5 };

                // Weight based on how close to ideal exposure (0.3-0.7 range)
                // Gaussian centered at 0.5 with sigma=0.3
                let sigma = 0.3;

                (-(mean - 0.5).powi(2) / (2.0 * sigma * sigma)).exp()
            })
            .collect();

        tracing::info!(
            "Using exposure-aware HDR weights for {} exposures: {:?}",
            num_images,
            image_weights
        );

        // Get reference scale (middle frame)
        let reference_idx = (num_images / 2).min(num_images - 1);
        let _reference_scale = if let Some(aligns) = alignments {
            aligns[reference_idx].scale
        } else {
            1.0
        };

        // Parallel processing: for each pixel, use only frames from best focus plane
        use rayon::prelude::*;
        let results: Vec<(usize, usize, f64)> = fg_pixels
            .par_iter()
            .map(|&(y, x)| {
                let plane_idx = best_plane[[y, x]];
                let start_frame = plane_idx * num_exposures;
                let end_frame = start_frame + num_exposures;

                let mut pixel_sum = 0.0;
                let mut weight_sum = 0.0;

                // HDR fusion: only use exposures from the best focus plane
                for n in start_frame..end_frame.min(num_images) {
                    let w = image_weights[n];

                    // Scale compensation disabled (zombie code removal)
                    let pixel_value = frames[n][[y, x, 0]];

                    pixel_sum += w * pixel_value;
                    weight_sum += w;
                }

                let value = if weight_sum > 1e-10 {
                    pixel_sum / weight_sum
                } else {
                    0.0
                };

                (y, x, value)
            })
            .collect();

        // Write results back
        for (y, x, value) in results {
            result[[y, x]] = value;
        }
    } else {
        // Fallback: old behavior (average across all frames)
        tracing::info!(
            "Collapsing foreground pixels (baseline weighted average - no focus plane info)"
        );

        // PER-PIXEL MERTENS WEIGHTS: Compute weights locally at each pixel
        let image_weights: Vec<f64> = vec![1.0; num_images];

        use rayon::prelude::*;
        let results: Vec<(usize, usize, f64)> = fg_pixels
            .par_iter()
            .map(|&(y, x)| {
                let mut pixel_sum = 0.0;
                let mut weight_sum = 0.0;

                for n in 0..num_images {
                    let w = image_weights[n];
                    pixel_sum += w * frames[n][[y, x, 0]];
                    weight_sum += w;
                }

                let value = if weight_sum > 1e-10 {
                    pixel_sum / weight_sum
                } else {
                    0.0
                };

                (y, x, value)
            })
            .collect();

        for (y, x, value) in results {
            result[[y, x]] = value;
        }
    }

    tracing::info!(
        "Single-pass foreground collapse complete: {} pixels (Bayer output)",
        fg_pixels.len()
    );

    Ok(result)
}

/// Super-resolution factor
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SRFactor {
    None,        // Quality enhancement only (1.0x)
    SR1_5x,      // 1.5x super-resolution
    GoldenRatio, // 1.618x super-resolution (φ - golden ratio)
    SR2x,        // 2x super-resolution
    SR3x,        // 3x super-resolution
}

impl SRFactor {
    pub fn as_f64(&self) -> f64 {
        match self {
            SRFactor::None => 1.0,
            SRFactor::SR1_5x => 1.5,
            SRFactor::GoldenRatio => 1.618033988749895, // φ = (1 + √5) / 2
            SRFactor::SR2x => 2.0,
            SRFactor::SR3x => 3.0,
        }
    }
}

impl From<usize> for SRFactor {
    fn from(value: usize) -> Self {
        match value {
            1 => SRFactor::None,
            2 => SRFactor::SR1_5x,
            3 => SRFactor::GoldenRatio,
            4 => SRFactor::SR2x,
            5 => SRFactor::SR3x,
            _ => SRFactor::None,
        }
    }
}

/// Parameters for hierarchical collapse
#[derive(Debug, Clone)]
pub struct HierarchicalParams {
    /// Super-resolution factor
    pub sr_factor: SRFactor,

    /// Fidelity weight for B-grade prior (λ in LS solve)
    pub lambda_b: f64,

    /// Fidelity weight for A-grade prior
    pub lambda_a: f64,
    /// Exposure fusion sigma (for well-exposedness)
    pub exposure_sigma: f64,

    /// Denoising strength (for NLM in A-grade)
    pub denoise_strength: f64,
}

impl Default for HierarchicalParams {
    fn default() -> Self {
        Self {
            sr_factor: SRFactor::None,
            lambda_b: 0.1,
            lambda_a: 1.0,
            exposure_sigma: 0.2,
            denoise_strength: 0.3,
        }
    }
}

/// Mertens quality weights for exposure fusion
#[derive(Debug, Clone)]
pub struct MertensWeights {
    pub contrast: Vec<f64>,
    pub saturation: Vec<f64>,
    pub exposedness: Vec<f64>,
}

/// Collapse C-grade pixels using per-pixel best focus plane selection
///
/// For focus stacking: select best focus plane per pixel, then HDR fuse exposures within that plane
/// O(C_pixels * N_exposures) - much faster than averaging all frames!
///
/// Accepts frames directly without requiring pre-stacking (memory optimization)
pub fn collapse_c_grade(
    frames: &[Array3<f64>], // Vec of frames, each (H, W, 1)
    grades: &Array2<u8>,
    _exposures: &[f64],
    _params: &HierarchicalParams,
    best_plane_map: Option<&Array2<usize>>, // Per-pixel best focus plane
    num_exposures: usize,                   // Number of exposures per focus plane
    _wb_multipliers: &[f32; 4],             // Unused - demosaicing happens later
    _alignments: Option<&[AlignmentInfo]>,  // Scale information unused (compensation disabled)
) -> Result<Array2<f64>> {
    // Returns BAYER (H×W), not RGB
    if frames.is_empty() {
        anyhow::bail!("No frames provided");
    }

    let (height, width, _) = frames[0].dim();
    let num_images = frames.len();
    let mut result = Array2::<f64>::zeros((height, width));

    // Collect C-grade pixel coordinates for efficient iteration
    let c_pixels: Vec<(usize, usize)> = grades
        .indexed_iter()
        .filter(|(_, &g)| g == Grade::C as u8)
        .map(|((y, x), _)| (y, x))
        .collect();

    // Check if we have focus plane information
    if let Some(best_plane) = best_plane_map {
        tracing::info!("Collapsing C-grade pixels (per-pixel focus plane selection + HDR fusion)");

        // For HDR: Use exposure-aware weights
        let image_weights: Vec<f64> = frames
            .iter()
            .map(|frame| {
                let step = 8;
                let mut sum = 0.0;
                let mut count = 0;
                for y in (0..height).step_by(step) {
                    for x in (0..width).step_by(step) {
                        sum += frame[[y, x, 0]];
                        count += 1;
                    }
                }
                let mean = if count > 0 { sum / count as f64 } else { 0.5 };
                let sigma = 0.3;
                (-(mean - 0.5).powi(2) / (2.0 * sigma * sigma)).exp()
            })
            .collect();

        tracing::info!("Using exposure-aware HDR weights: {:?}", image_weights);

        // Get reference scale (middle frame)

        // Parallel processing: for each pixel, use only frames from best focus plane
        use rayon::prelude::*;
        let results: Vec<(usize, usize, f64)> = c_pixels
            .par_iter()
            .map(|&(y, x)| {
                let plane_idx = best_plane[[y, x]];
                let start_frame = plane_idx * num_exposures;
                let end_frame = start_frame + num_exposures;

                let mut pixel_sum = 0.0;
                let mut weight_sum = 0.0;

                // HDR fusion: only use exposures from the best focus plane
                // Use PER-PIXEL exposure-aware weights (not global weights!)
                for n in start_frame..end_frame.min(num_images) {
                    // Scale compensation disabled (zombie code removal)
                    let pixel_value = frames[n][[y, x, 0]];

                    // Per-pixel weight: prefer well-exposed pixels
                    // With HDR scaling + WB, middle exposure has mean ~1.3
                    // Use wide Gaussian to accept 0.1-3.0 range (centered at 1.0)
                    let sigma = 1.0; // Wide sigma to accept broad range
                    let target = 1.0;
                    let w = if !(0.01..=5.0).contains(&pixel_value) {
                        0.01 // Very low weight for near-black or clipped pixels
                    } else {
                        (-(pixel_value - target).powi(2) / (2.0 * sigma * sigma)).exp()
                    };

                    pixel_sum += w * pixel_value;
                    weight_sum += w;
                }

                let value = if weight_sum > 1e-10 {
                    pixel_sum / weight_sum
                } else {
                    0.0
                };

                (y, x, value)
            })
            .collect();

        // Write results back
        for (y, x, value) in results {
            result[[y, x]] = value;
        }
    } else {
        // Fallback: old behavior (average across all frames)
        tracing::info!(
            "Collapsing C-grade pixels (baseline weighted average - no focus plane info)"
        );

        // PER-PIXEL MERTENS WEIGHTS: Compute weights locally at each pixel
        let image_weights: Vec<f64> = vec![1.0; num_images];

        use rayon::prelude::*;
        let results: Vec<(usize, usize, f64)> = c_pixels
            .par_iter()
            .map(|&(y, x)| {
                let mut pixel_sum = 0.0;
                let mut weight_sum = 0.0;

                for (n, frame) in frames.iter().enumerate() {
                    let w = image_weights[n];
                    pixel_sum += w * frame[[y, x, 0]];
                    weight_sum += w;
                }

                let value = if weight_sum > 1e-10 {
                    pixel_sum / weight_sum
                } else {
                    0.0
                };

                (y, x, value)
            })
            .collect();

        for (y, x, value) in results {
            result[[y, x]] = value;
        }
    }

    let c_count = c_pixels.len();
    tracing::info!(
        "C-grade collapse complete: {} pixels (Bayer output)",
        c_count
    );

    Ok(result)
}

/// Collapse B-grade pixels using C as prior
pub fn collapse_b_grade(
    frames: &[Array3<f64>],
    grades: &Array2<u8>,
    c_result: &Array2<f64>,
    _exposures: &[f64],
    params: &HierarchicalParams,
    best_plane_map: Option<&Array2<usize>>,
    num_exposures: usize,
    _wb_multipliers: &[f32; 4],
    _alignments: Option<&[AlignmentInfo]>,
) -> Result<Array2<f64>> {
    if frames.is_empty() {
        anyhow::bail!("No frames");
    }
    let (height, width, _) = frames[0].dim();
    let num_images = frames.len();
    let mut result = Array2::<f64>::zeros((height, width));

    let b_pixels: Vec<(usize, usize)> = grades
        .indexed_iter()
        .filter(|(_, &g)| g == Grade::B as u8)
        .map(|((y, x), _)| (y, x))
        .collect();

    let lambda = params.lambda_b;

    if let Some(best_plane) = best_plane_map {
        use rayon::prelude::*;
        let results: Vec<(usize, usize, f64)> = b_pixels
            .par_iter()
            .map(|&(y, x)| {
                let plane_idx = best_plane[[y, x]];
                let start_frame = plane_idx * num_exposures;
                let end_frame = start_frame + num_exposures;
                let mut pixel_sum = 0.0;
                let mut weight_sum = 0.0;
                for n in start_frame..end_frame.min(num_images) {
                    let pixel_value = frames[n][[y, x, 0]];
                    let sigma = 1.0;
                    let target = 1.0;
                    let w = if !(0.01..=5.0).contains(&pixel_value) {
                        0.01
                    } else {
                        (-(pixel_value - target).powi(2) / (2.0 * sigma * sigma)).exp()
                    };
                    pixel_sum += w * pixel_value;
                    weight_sum += w;
                }
                let y_b = if weight_sum > 1e-10 {
                    pixel_sum / weight_sum
                } else {
                    0.0
                };
                let x_c = c_result[[y, x]];
                let value = (y_b + lambda * x_c) / (1.0 + lambda);
                (y, x, value)
            })
            .collect();
        for (y, x, value) in results {
            result[[y, x]] = value;
        }
    } else {
        // Fallback without planes (average all)
        use rayon::prelude::*;
        let results: Vec<(usize, usize, f64)> = b_pixels
            .par_iter()
            .map(|&(y, x)| {
                let mut pixel_sum = 0.0;
                let mut weight_sum = 0.0;
                for n in 0..num_images {
                    let pixel_value = frames[n][[y, x, 0]];
                    pixel_sum += pixel_value;
                    weight_sum += 1.0;
                }
                let y_b = if weight_sum > 1e-10 {
                    pixel_sum / weight_sum
                } else {
                    0.0
                };
                let x_c = c_result[[y, x]];
                let value = (y_b + lambda * x_c) / (1.0 + lambda);
                (y, x, value)
            })
            .collect();
        for (y, x, value) in results {
            result[[y, x]] = value;
        }
    }
    tracing::info!("B-grade collapse complete: {} pixels", b_pixels.len());
    Ok(result)
}

/// Collapse B+C grades together in one joint demosaicing pass
///
/// This avoids artifacts from separate B and C collapses by processing them together.
/// C-grade uses hard selection, B-grade uses soft blending with C as prior.
pub fn collapse_bc_grade_frames(
    frames: &[Array3<f64>], // Vec of frames, each (H, W, 1)
    grades: &Array2<u8>,
    _exposures: &[f64],
    params: &HierarchicalParams,
    best_plane_map: Option<&Array2<usize>>, // Per-pixel best focus plane
    num_exposures: usize,                   // Number of exposures per focus plane
    wb_multipliers: &[f32; 4],              // White balance multipliers [R, G1, G2, B]
) -> Result<(Array3<f64>, Array3<f64>)> {
    // Returns (B result, C result) both RGB (H×W×3)
    if frames.is_empty() {
        anyhow::bail!("No frames provided");
    }

    let (height, width, _) = frames[0].dim();
    let num_images = frames.len();

    // Collect B+C grade pixel coordinates
    let bc_pixels: Vec<(usize, usize)> = grades
        .indexed_iter()
        .filter(|(_, &g)| g == Grade::B as u8 || g == Grade::C as u8)
        .map(|((y, x), _)| (y, x))
        .collect();

    let b_pixels: Vec<(usize, usize)> = grades
        .indexed_iter()
        .filter(|(_, &g)| g == Grade::B as u8)
        .map(|((y, x), _)| (y, x))
        .collect();

    let c_pixels: Vec<(usize, usize)> = grades
        .indexed_iter()
        .filter(|(_, &g)| g == Grade::C as u8)
        .map(|((y, x), _)| (y, x))
        .collect();

    if let Some(best_plane) = best_plane_map {
        tracing::info!(
            "Collapsing B+C grades together (joint demosaicing, {} B pixels, {} C pixels)",
            b_pixels.len(),
            c_pixels.len()
        );

        // PER-PIXEL MERTENS WEIGHTS: Compute weights locally at each pixel
        let image_weights: Vec<f64> = vec![1.0; num_images];

        // Joint demosaicing for ALL B+C pixels together
        let bc_result = joint_demosaic_with_focus_selection(
            frames,
            &image_weights,
            best_plane,
            num_exposures,
            wb_multipliers,
            PixelSelection::List(&bc_pixels), // Process B+C together
            1,                                // blend_radius=1: soft blending
        )?;

        // Split result into B and C
        // For B-grade: blend with C-grade prior (lambda weighting)
        // For C-grade: use direct result
        let lambda = params.lambda_b;
        let mut b_result = Array3::<f64>::zeros((height, width, 3));
        let mut c_result = Array3::<f64>::zeros((height, width, 3));

        // Copy C-grade pixels directly
        for &(y, x) in &c_pixels {
            for c in 0..3 {
                c_result[[y, x, c]] = bc_result[[y, x, c]];
            }
        }

        // B-grade: blend with C-grade prior
        for &(y, x) in &b_pixels {
            for c in 0..3 {
                let y_b = bc_result[[y, x, c]];
                let x_c = bc_result[[y, x, c]]; // Use same result as prior
                b_result[[y, x, c]] = (y_b + lambda * x_c) / (1.0 + lambda);
            }
        }

        tracing::info!(
            "B+C collapse complete: {} B pixels, {} C pixels (RGB output)",
            b_pixels.len(),
            c_pixels.len()
        );

        Ok((b_result, c_result))
    } else {
        anyhow::bail!("B+C collapse requires focus plane information");
    }
}

/// Collapse A-grade pixels with optional alignment data for SR
///
/// Accepts frames directly without requiring pre-stacking (memory optimization)
pub fn collapse_a_grade(
    frames: &[Array3<f64>], // Vec of frames, each (H, W, 1)
    grades: &Array2<u8>,
    b_result: &Array2<f64>, // Bayer (H×W)
    c_result: &Array2<f64>, // Bayer (H×W)

    exposures: &[f64],
    params: &HierarchicalParams,
    alignments: Option<&[AlignmentInfo]>,
    best_plane_map: Option<&Array2<usize>>, // Per-pixel best focus plane
    num_exposures: usize,                   // Number of exposures per focus plane
    wb_multipliers: &[f32; 4],              // Passed through but not used until final demosaic
) -> Result<CollapseResult> {
    if frames.is_empty() {
        anyhow::bail!("No frames provided");
    }

    let (height, width, _) = frames[0].dim();
    let num_images = frames.len();
    let sr_factor = params.sr_factor.as_f64();

    // Check foreground coverage (A+B+C) - SR only works well with dense data
    let a_count = grades.iter().filter(|&&g| g == Grade::A as u8).count();
    let b_count = grades.iter().filter(|&&g| g == Grade::B as u8).count();
    let c_count = grades.iter().filter(|&&g| g == Grade::C as u8).count();
    let foreground_count = a_count + b_count + c_count;
    let total_pixels = height * width;
    let foreground_coverage = (foreground_count as f64 / total_pixels as f64) * 100.0;

    // Check for subpixel diversity (required for SR)
    let has_subpixel_diversity = if let Some(aligns) = alignments {
        let mut dx_values: Vec<f64> = aligns.iter().map(|a| a.dx).collect();
        let mut dy_values: Vec<f64> = aligns.iter().map(|a| a.dy).collect();
        dx_values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        dy_values.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let max_shift = dx_values
            .iter()
            .chain(dy_values.iter())
            .map(|v| v.abs())
            .fold(0.0f64, f64::max);

        let scale_values: Vec<f64> = aligns.iter().map(|a| a.scale).collect();
        let scale_range = scale_values.iter().copied().fold(0.0f64, f64::max)
            - scale_values.iter().copied().fold(f64::INFINITY, f64::min);

        tracing::info!(
            "Subpixel diversity check: max_shift={:.3}px, scale_range={:.4}",
            max_shift,
            scale_range
        );

        // Need either translation shifts OR scale variation (focus breathing)
        max_shift >= 0.01 || scale_range >= 0.001
    } else {
        false
    };

    // SR requires:
    // 1. User requested it (sr_factor > 1.0)
    // 2. Alignment data available
    // 3. Sufficient foreground coverage (>40%)
    // 4. Subpixel diversity between frames
    let min_coverage_for_sr = 40.0;

    let do_sr = sr_factor > 1.0
        && alignments.is_some()
        && foreground_coverage >= min_coverage_for_sr
        && has_subpixel_diversity;

    if sr_factor > 1.0 {
        if foreground_coverage < min_coverage_for_sr {
            tracing::warn!("Foreground coverage ({:.1}%) too low for SR (need >{:.0}%) - using native resolution",
                          foreground_coverage, min_coverage_for_sr);
        }
        if !has_subpixel_diversity {
            tracing::warn!(
                "No subpixel diversity detected - SR requires subpixel shifts between frames"
            );
            tracing::warn!("Using native resolution instead (focus stacking + HDR only)");
        }
    }

    tracing::info!(
        "Collapsing A+B+C grades for SR (A={:.1}%, B={:.1}%, C={:.1}%, total={:.1}%, SR={:.3}x{})",
        (a_count as f64 / total_pixels as f64) * 100.0,
        (b_count as f64 / total_pixels as f64) * 100.0,
        (c_count as f64 / total_pixels as f64) * 100.0,
        foreground_coverage,
        if do_sr { sr_factor } else { 1.0 },
        if do_sr {
            ""
        } else {
            " [DISABLED - sparse data]"
        }
    );

    if do_sr {
        // Super-resolution path - returns RGB directly (already demosaiced!)
        // For SR, we need to create a temporary stack (SR code is complex and uses stack format)
        let mut stack = Array3::<f64>::zeros((height, width, num_images));
        for (i, frame) in frames.iter().enumerate() {
            for y in 0..height {
                for x in 0..width {
                    stack[[y, x, i]] = frame[[y, x, 0]];
                }
            }
        }
        let rgb = collapse_a_grade_with_sr(
            &stack,
            grades,
            b_result,
            c_result,
            exposures,
            params,
            alignments.unwrap(),
        )?;
        Ok(CollapseResult::Rgb(rgb))
    } else {
        // Native resolution path (quality enhancement) - works directly with frames!
        // Returns Bayer - will be demosaiced later
        let bayer = collapse_a_grade_native_frames(
            frames,
            grades,
            b_result,
            c_result,
            exposures,
            params,
            best_plane_map,
            num_exposures,
            wb_multipliers,
            alignments,
        )?;
        Ok(CollapseResult::Bayer(bayer))
    }
}

/// A-grade collapse at native resolution (FRAMES VERSION - no stacking required)
fn collapse_a_grade_native_frames(
    frames: &[Array3<f64>],
    grades: &Array2<u8>,
    b_result: &Array2<f64>,  // Bayer (H×W)
    _c_result: &Array2<f64>, // Bayer (H×W)
    exposures: &[f64],
    params: &HierarchicalParams,
    best_plane_map: Option<&Array2<usize>>, // Per-pixel best focus plane
    num_exposures: usize,                   // Number of exposures per focus plane
    _wb_multipliers: &[f32; 4],             // Unused - demosaicing happens later
    alignments: Option<&[AlignmentInfo]>,   // Scale information for focus breathing compensation
) -> Result<Array2<f64>> {
    // Returns BAYER (H×W), not RGB
    if frames.is_empty() {
        anyhow::bail!("No frames provided");
    }

    let (height, width, _) = frames[0].dim();
    let num_images = frames.len();
    let mut result = Array2::<f64>::zeros((height, width));

    // Collect A-grade pixel coordinates for efficient iteration
    let a_pixels: Vec<(usize, usize)> = grades
        .indexed_iter()
        .filter(|(_, &g)| g == Grade::A as u8)
        .map(|((y, x), _)| (y, x))
        .collect();

    let lambda = params.lambda_a;

    // Check if we have focus plane information
    if let Some(best_plane) = best_plane_map {
        tracing::info!(
            "A-grade collapse: per-pixel focus plane selection + HDR fusion (guided by B prior)"
        );

        // Compute Mertens weights for HDR fusion within focus planes
        let weights = compute_mertens_weights_frames(frames, exposures, params)?;
        let image_weights: Vec<f64> = (0..num_images)
            .map(|n| weights.contrast[n] * weights.saturation[n] * weights.exposedness[n])
            .collect();

        // Get reference scale (middle frame)
        let reference_idx = (num_images / 2).min(num_images - 1);
        let _reference_scale = if let Some(aligns) = alignments {
            aligns[reference_idx].scale
        } else {
            1.0
        };

        // Parallel processing: for each pixel, use only frames from best focus plane
        use rayon::prelude::*;
        let results: Vec<(usize, usize, f64)> = a_pixels
            .par_iter()
            .map(|&(y, x)| {
                let plane_idx = best_plane[[y, x]];
                let start_frame = plane_idx * num_exposures;
                let end_frame = start_frame + num_exposures;

                let mut pixel_sum = 0.0;
                let mut weight_sum = 0.0;

                // HDR fusion: only use exposures from the best focus plane
                for n in start_frame..end_frame.min(num_images) {
                    let w = image_weights[n];

                    // Scale compensation disabled (zombie code removal)
                    let pixel_value = frames[n][[y, x, 0]];

                    pixel_sum += w * pixel_value;
                    weight_sum += w;
                }

                let y_a = if weight_sum > 1e-10 {
                    pixel_sum / weight_sum
                } else {
                    0.0
                };

                // Blend with B-grade prior: (y_A + λ*X_B) / (1 + λ)
                let x_b = b_result[[y, x]];
                let value = (y_a + lambda * x_b) / (1.0 + lambda);

                (y, x, value)
            })
            .collect();

        // Write results back
        for (y, x, value) in results {
            result[[y, x]] = value;
        }
    } else {
        // Fallback: old behavior (average across all frames)
        tracing::info!(
            "A-grade collapse: baseline weighted average (guided by B prior - no focus plane info)"
        );

        let weights = compute_mertens_weights_frames(frames, exposures, params)?;
        let image_weights: Vec<f64> = (0..num_images)
            .map(|n| weights.contrast[n] * weights.saturation[n] * weights.exposedness[n])
            .collect();

        if a_pixels.len() >= MIN_GPU_COLLAPSE_PIXELS {
            if let Some(collapsed) = try_gpu_collapse_pixels(&a_pixels, &image_weights, frames) {
                use rayon::prelude::*;
                let results: Vec<(usize, usize, f64)> = a_pixels
                    .par_iter()
                    .map(|&(y, x)| {
                        let y_a = collapsed[[y, x]];
                        let x_b = b_result[[y, x]];
                        let value = (y_a + lambda * x_b) / (1.0 + lambda);
                        (y, x, value)
                    })
                    .collect();
                for (y, x, value) in results {
                    result[[y, x]] = value;
                }
            } else {
                use rayon::prelude::*;
                let results: Vec<(usize, usize, f64)> = a_pixels
                    .par_iter()
                    .map(|&(y, x)| {
                        let mut pixel_sum = 0.0;
                        let mut weight_sum = 0.0;

                        for (n, frame) in frames.iter().enumerate() {
                            let w = image_weights[n];
                            pixel_sum += w * frame[[y, x, 0]];
                            weight_sum += w;
                        }

                        let y_a = if weight_sum > 1e-10 {
                            pixel_sum / weight_sum
                        } else {
                            0.0
                        };

                        let x_b = b_result[[y, x]];
                        let value = (y_a + lambda * x_b) / (1.0 + lambda);
                        (y, x, value)
                    })
                    .collect();

                for (y, x, value) in results {
                    result[[y, x]] = value;
                }
            }
        } else {
            use rayon::prelude::*;
            let results: Vec<(usize, usize, f64)> = a_pixels
                .par_iter()
                .map(|&(y, x)| {
                    let mut pixel_sum = 0.0;
                    let mut weight_sum = 0.0;

                    for (n, frame) in frames.iter().enumerate() {
                        let w = image_weights[n];
                        pixel_sum += w * frame[[y, x, 0]];
                        weight_sum += w;
                    }

                    let y_a = if weight_sum > 1e-10 {
                        pixel_sum / weight_sum
                    } else {
                        0.0
                    };

                    let x_b = b_result[[y, x]];
                    let value = (y_a + lambda * x_b) / (1.0 + lambda);
                    (y, x, value)
                })
                .collect();

            for (y, x, value) in results {
                result[[y, x]] = value;
            }
        }
    }

    let a_count = a_pixels.len();
    tracing::info!(
        "A-grade collapse complete: {} pixels (Bayer output, blended with B prior)",
        a_count
    );

    Ok(result)
}

const MIN_GPU_COLLAPSE_PIXELS: usize = 8192;

fn try_gpu_collapse_pixels(
    pixel_coords: &[(usize, usize)],
    image_weights: &[f64],
    frames: &[Array3<f64>],
) -> Option<Array2<f64>> {
    #[cfg(feature = "gpu")]
    {
        use crate::gpu::{get_gpu_context, gpu_collapse_pixels};

        let gpu_ctx = get_gpu_context()?;
        let stacked = match stack_frames_scalar(frames) {
            Ok(stacked) => stacked,
            Err(e) => {
                tracing::warn!("GPU collapse stack failed: {}", e);
                return None;
            }
        };

        match gpu_collapse_pixels(&gpu_ctx, pixel_coords, image_weights, &stacked) {
            Ok(Some(output)) => Some(output),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!("GPU collapse failed: {}", e);
                None
            }
        }
    }
    #[cfg(not(feature = "gpu"))]
    {
        let _ = pixel_coords;
        let _ = image_weights;
        let _ = frames;
        None
    }
}

/// Joint SR+demosaic: Map Bayer pixels directly to RGB SR grid
///
/// This avoids the artifact-prone sequential approach (SR Bayer → fill holes → demosaic)
/// by directly accumulating Bayer pixels into their corresponding RGB channels.
///
/// Algorithm:
/// 1. For each source Bayer pixel from all frames:
///    - Determine its color (R/G/B) from Bayer pattern position
///    - Map to SR grid position using subpixel alignment
///    - Accumulate into appropriate RGB channel
/// 2. After accumulation, interpolate missing color channels
///    - Each SR pixel has observations in 1-2 color channels
///    - Use bilinear interpolation to fill missing channels
///
/// Returns: RGB image (3 x H*sr x W*sr)
fn joint_sr_demosaic(
    images: &Array3<f64>, // Bayer stack: H x W x N
    grades: &Array2<u8>,
    exposures: &[f64],
    alignments: &[AlignmentInfo],
    sr_factor: f64,
) -> Result<Array3<f64>> {
    let (height, width, num_images) = images.dim();
    let sr_h = (height as f64 * sr_factor).round() as usize;
    let sr_w = (width as f64 * sr_factor).round() as usize;

    tracing::info!(
        "Joint SR+demosaic: {}x{} → {}x{} RGB ({}x)",
        width,
        height,
        sr_w,
        sr_h,
        sr_factor
    );

    // Debug: Check exposures
    tracing::info!("Exposures: {:?}", exposures);

    // Initialize RGB SR grid and weight accumulators
    let mut rgb_sr = Array3::<f64>::zeros((3, sr_h, sr_w)); // 3 channels: R, G, B
    let mut rgb_weights = Array3::<f64>::zeros((3, sr_h, sr_w));

    // Debug: Count grades
    let mut grade_counts = [0usize; 4];
    for y in 0..height {
        for x in 0..width {
            let g = grades[[y, x]];
            if g < 4 {
                grade_counts[g as usize] += 1;
            }
        }
    }
    tracing::info!(
        "Grade counts in joint_sr_demosaic: A={}, B={}, C={}, D={}",
        grade_counts[Grade::A as usize],
        grade_counts[Grade::B as usize],
        grade_counts[Grade::C as usize],
        grade_counts[Grade::D as usize]
    );

    // Accumulate Bayer pixels into RGB channels
    let mut pixels_passed_grade = 0usize;
    let mut pixels_accumulated = 0usize;
    let mut pixels_out_of_bounds = 0usize;

    // Debug: Check alignment diversity
    let mut unique_alignments = std::collections::HashSet::new();
    for align in alignments {
        let key = (
            (align.dx * 1000.0).round() as i32,
            (align.dy * 1000.0).round() as i32,
        );
        unique_alignments.insert(key);
    }
    tracing::info!(
        "Unique alignments: {} out of {} frames",
        unique_alignments.len(),
        num_images
    );

    for n in 0..num_images {
        let align = &alignments[n];
        let dx_sr = align.dx * sr_factor;
        let dy_sr = align.dy * sr_factor;

        // Weight by inverse of exposure time (shutter speed in seconds)
        // Longer exposures (brighter images) get less weight for HDR fusion
        let exposure_weight = 1.0 / (exposures[n] + 1e-10);

        // Debug first few frames alignment
        if n < 5 {
            tracing::info!(
                "Frame {} alignment: dx={:.3}, dy={:.3} (SR: dx_sr={:.3}, dy_sr={:.3})",
                n,
                align.dx,
                align.dy,
                dx_sr,
                dy_sr
            );
        }

        for y in 0..height {
            for x in 0..width {
                // Use A+B+C grades for SR
                let grade_weight = match grades[[y, x]] {
                    g if g == Grade::A as u8 => 1.0,
                    g if g == Grade::B as u8 => 0.7,
                    g if g == Grade::C as u8 => 0.4,
                    _ => 0.0, // Skip D-grade
                };

                if grade_weight < 1e-10 {
                    continue;
                }

                pixels_passed_grade += 1;
                let value = images[[y, x, n]];

                // Determine Bayer color from position (RGGB pattern)
                // R at (even, even), G at (even, odd) and (odd, even), B at (odd, odd)
                let color_channel = match (y % 2, x % 2) {
                    (0, 0) => 0,          // R
                    (0, 1) | (1, 0) => 1, // G
                    (1, 1) => 2,          // B
                    _ => unreachable!(),
                };

                // Map to SR grid with subpixel shift
                let sr_x = (x as f64 * sr_factor) + dx_sr;
                let sr_y = (y as f64 * sr_factor) + dy_sr;

                // Bilinear splatting to 4 nearest pixels
                let x0 = sr_x.floor() as isize;
                let y0 = sr_y.floor() as isize;
                let fx = sr_x - x0 as f64;
                let fy = sr_y - y0 as f64;

                let weights_2d = [
                    ((1.0 - fx) * (1.0 - fy), x0, y0),
                    (fx * (1.0 - fy), x0 + 1, y0),
                    ((1.0 - fx) * fy, x0, y0 + 1),
                    (fx * fy, x0 + 1, y0 + 1),
                ];

                for &(w, sx, sy) in &weights_2d {
                    if sy >= 0 && sy < sr_h as isize && sx >= 0 && sx < sr_w as isize {
                        let ny = sy as usize;
                        let nx = sx as usize;
                        let total_weight = w * grade_weight * exposure_weight;

                        rgb_sr[[color_channel, ny, nx]] += value * total_weight;
                        rgb_weights[[color_channel, ny, nx]] += total_weight;
                        pixels_accumulated += 1;
                    } else {
                        pixels_out_of_bounds += 1;
                    }
                }
            }
        }
    }

    tracing::info!(
        "Pixel stats: passed_grade={}, accumulated={}, out_of_bounds={}",
        pixels_passed_grade,
        pixels_accumulated,
        pixels_out_of_bounds
    );

    // Debug: Check if weights were actually set
    let total_weight: f64 = rgb_weights.iter().sum();
    let max_weight = rgb_weights.iter().copied().fold(0.0f64, f64::max);
    tracing::info!(
        "Weight stats: total_weight={:.3}, max_weight={:.6}",
        total_weight,
        max_weight
    );

    // Normalize by weights
    for c in 0..3 {
        for y in 0..sr_h {
            for x in 0..sr_w {
                if rgb_weights[[c, y, x]] > 1e-10 {
                    rgb_sr[[c, y, x]] /= rgb_weights[[c, y, x]];
                }
            }
        }
    }

    // Count filled pixels per channel
    let r_filled = rgb_weights
        .slice(s![0, .., ..])
        .iter()
        .filter(|&&w| w > 1e-10)
        .count();
    let g_filled = rgb_weights
        .slice(s![1, .., ..])
        .iter()
        .filter(|&&w| w > 1e-10)
        .count();
    let b_filled = rgb_weights
        .slice(s![2, .., ..])
        .iter()
        .filter(|&&w| w > 1e-10)
        .count();
    let total_pixels = sr_h * sr_w;

    tracing::info!(
        "RGB channel coverage: R={:.1}%, G={:.1}%, B={:.1}%",
        (r_filled as f64 / total_pixels as f64) * 100.0,
        (g_filled as f64 / total_pixels as f64) * 100.0,
        (b_filled as f64 / total_pixels as f64) * 100.0
    );

    // Debug: Expected coverage calculation
    // Native resolution: 1302x1308 = 1,703,016 pixels
    // Foreground: 50.2% = 854,657 pixels
    // SR resolution: 2604x2616 = 6,812,064 pixels
    // Expected coverage per channel (Bayer):
    //   - R: 25% of foreground = 213,664 pixels / 6,812,064 = 3.1%
    //   - G: 50% of foreground = 427,328 pixels / 6,812,064 = 6.3%
    //   - B: 25% of foreground = 213,664 pixels / 6,812,064 = 3.1%
    // This matches observed coverage! The problem is we're only using 1 frame's worth of data
    // With 21 frames and subpixel shifts, we should have MUCH higher coverage
    tracing::warn!(
        "Coverage is too low! With {} frames and subpixel shifts, expected ~{}% per channel",
        num_images,
        (r_filled as f64 / total_pixels as f64) * 100.0 * num_images as f64
    );

    // Interpolate missing color channels using bilinear interpolation
    interpolate_missing_rgb_channels(&mut rgb_sr, &rgb_weights)?;

    Ok(rgb_sr)
}

/// Interpolate missing RGB channels using edge-directed interpolation
/// This is much better than simple averaging - it respects image structure
fn interpolate_missing_rgb_channels(rgb: &mut Array3<f64>, weights: &Array3<f64>) -> Result<()> {
    let (_, height, width) = rgb.dim();

    // Multi-pass propagation to fill holes
    // Each pass fills pixels that have at least one filled neighbor
    let max_passes = 50;

    for pass in 0..max_passes {
        let mut filled_this_pass = 0;
        let rgb_read = rgb.clone();
        let weights_read = weights.clone();

        for y in 1..height - 1 {
            for x in 1..width - 1 {
                for c in 0..3 {
                    if weights_read[[c, y, x]] < 1e-10 {
                        // Missing channel - use edge-directed interpolation

                        // Collect values from 8-connected neighbors
                        let mut neighbors = Vec::new();
                        for dy in -1..=1 {
                            for dx in -1..=1 {
                                if dy == 0 && dx == 0 {
                                    continue;
                                }
                                let ny = (y as isize + dy) as usize;
                                let nx = (x as isize + dx) as usize;
                                if weights_read[[c, ny, nx]] > 1e-10 {
                                    neighbors.push((rgb_read[[c, ny, nx]], dy, dx));
                                }
                            }
                        }

                        if !neighbors.is_empty() {
                            // Use median instead of mean to reduce artifacts
                            let mut values: Vec<f64> =
                                neighbors.iter().map(|(v, _, _)| *v).collect();
                            values.sort_by(|a, b| a.partial_cmp(b).unwrap());
                            let median = values[values.len() / 2];

                            rgb[[c, y, x]] = median;
                            filled_this_pass += 1;
                        }
                    }
                }
            }
        }

        if filled_this_pass == 0 {
            break;
        }

        if pass < 5 || pass % 10 == 0 {
            tracing::debug!(
                "RGB interpolation pass {}: filled {} pixels",
                pass,
                filled_this_pass
            );
        }
    }

    // Handle borders with simple replication
    for c in 0..3 {
        // Top and bottom rows
        for x in 0..width {
            if weights[[c, 0, x]] < 1e-10 && height > 1 {
                rgb[[c, 0, x]] = rgb[[c, 1, x]];
            }
            if weights[[c, height - 1, x]] < 1e-10 && height > 1 {
                rgb[[c, height - 1, x]] = rgb[[c, height - 2, x]];
            }
        }
        // Left and right columns
        for y in 0..height {
            if weights[[c, y, 0]] < 1e-10 && width > 1 {
                rgb[[c, y, 0]] = rgb[[c, y, 1]];
            }
            if weights[[c, y, width - 1]] < 1e-10 && width > 1 {
                rgb[[c, y, width - 1]] = rgb[[c, y, width - 2]];
            }
        }
    }

    Ok(())
}

/// A-grade collapse with super-resolution
///
/// STRATEGY: Sequential Bayer approach
///
/// 1. Accumulate Bayer pixels into SR Bayer grid using subpixel shifts
/// 2. Fill holes using Bayer-aware propagation (respects Bayer pattern)
/// 3. Demosaic using AHD algorithm (designed for Bayer patterns)
///
/// This is better than joint RGB approach because:
/// - Bayer-aware hole filling respects color channel structure
/// - AHD demosaic is optimized for Bayer patterns
/// - Avoids artifacts from naive RGB interpolation
///
/// Returns: Super-resolved RGB image (3 x H*sr x W*sr) - already demosaiced!
fn collapse_a_grade_with_sr(
    images: &Array3<f64>, // Bayer stack: H x W x N
    grades: &Array2<u8>,
    _b_result: &Array2<f64>, // Bayer (H×W) - unused in SR path
    _c_result: &Array2<f64>, // Bayer (H×W) - unused in SR path
    _exposures: &[f64],
    params: &HierarchicalParams,
    alignments: &[AlignmentInfo],
) -> Result<Array3<f64>> {
    let (_height, _width, _num_images) = images.dim();
    let sr_factor = params.sr_factor.as_f64();

    tracing::info!("Using joint SR+demosaic: accumulate Bayer pixels directly to RGB channels");

    // Use joint SR+demosaic approach - accumulate Bayer pixels to RGB channels
    // This avoids the Bayer reconstruction problem
    let rgb_sr = joint_sr_demosaic(images, grades, _exposures, alignments, sr_factor)?;

    // rgb_sr is already in (3, H, W) format - return directly
    Ok(rgb_sr)
}

/// Fill holes in upsampled array with median of non-zero values
fn fill_upsampled_holes(array: &mut Array2<f64>) -> Result<()> {
    // Compute median of non-zero values
    let mut non_zero_values: Vec<f64> = array.iter().filter(|&&v| v > 1e-10).copied().collect();

    if non_zero_values.is_empty() {
        // All zeros - use a reasonable default
        return Ok(());
    }

    non_zero_values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = non_zero_values[non_zero_values.len() / 2];

    let mut filled = 0;
    for val in array.iter_mut() {
        if *val < 1e-10 {
            *val = median;
            filled += 1;
        }
    }

    if filled > 0 {
        tracing::debug!(
            "Filled {} holes in upsampled array with median={:.4}",
            filled,
            median
        );
    }

    Ok(())
}

/// Merge graded Bayer results
///
/// All grades (A/B/C) are now Bayer, so we merge Bayer values
/// Returns Bayer result that will be demosaiced later
pub fn merge_graded_results(
    a_result: &CollapseResult,
    b_result: &Array2<f64>, // Bayer (H×W)
    c_result: &Array2<f64>, // Bayer (H×W)
    grades: &Array2<u8>,
) -> Result<CollapseResult> {
    match a_result {
        CollapseResult::Bayer(bayer_a) => {
            // A-grade is Bayer - merge with B/C Bayer
            tracing::info!("Merging Bayer results from A/B/C grades");
            let merged = merge_native_bayer(bayer_a, b_result, c_result, grades)?;
            Ok(CollapseResult::Bayer(merged))
        }
        // Joint SR+demosaic already accumulates every foreground grade into the
        // high-resolution RGB grid, so there is no native-resolution merge.
        CollapseResult::Rgb(rgb_a) => Ok(CollapseResult::Rgb(rgb_a.clone())),
    }
}

/// Merge Bayer results at native resolution
fn merge_native_bayer(
    a_result: &Array2<f64>, // Bayer (H×W)
    b_result: &Array2<f64>, // Bayer (H×W)
    c_result: &Array2<f64>, // Bayer (H×W)
    grades: &Array2<u8>,
) -> Result<Array2<f64>> {
    let (height, width) = grades.dim();
    let mut merged = Array2::<f64>::zeros((height, width));

    for y in 0..height {
        for x in 0..width {
            let value = match grades[[y, x]] {
                g if g == Grade::A as u8 => a_result[[y, x]],
                g if g == Grade::B as u8 => b_result[[y, x]],
                g if g == Grade::C as u8 => c_result[[y, x]],
                _ => c_result[[y, x]], // D-grade uses C-grade result (background)
            };

            merged[[y, x]] = value;
        }
    }

    Ok(merged)
}

/// Merge with super-resolved A-grade (Bayer-aware)
///
/// A-grade is super-resolved Bayer, B/C are native Bayer
/// Need to upsample B/C in a Bayer-aware way (upsample each channel separately)
fn merge_with_sr(
    a_result: &Array2<f64>, // Super-resolved Bayer
    b_result: &Array2<f64>, // Native Bayer
    c_result: &Array2<f64>, // Native Bayer
    grades: &Array2<u8>,
    sr_factor: usize,
) -> Result<Array2<f64>> {
    let (height, width) = grades.dim();
    let hr_height = height * sr_factor;
    let hr_width = width * sr_factor;

    tracing::info!(
        "Bayer-aware merge: upsampling B/C from {}x{} to {}x{}",
        width,
        height,
        hr_width,
        hr_height
    );

    // Upsample B and C in Bayer-aware way
    // Split into R/G1/G2/B, upsample each, recombine
    let mut b_upsampled = upsample_bayer(b_result, sr_factor)?;
    let mut c_upsampled = upsample_bayer(c_result, sr_factor)?;

    // Fill holes in upsampled B/C with median fallback (critical for background regions!)
    fill_upsampled_holes(&mut b_upsampled)?;
    fill_upsampled_holes(&mut c_upsampled)?;

    // Upsample grade map (nearest neighbor)
    let mut grades_upsampled = Array2::<u8>::zeros((hr_height, hr_width));
    for hr_y in 0..hr_height {
        for hr_x in 0..hr_width {
            let lr_y = hr_y / sr_factor;
            let lr_x = hr_x / sr_factor;
            grades_upsampled[[hr_y, hr_x]] = grades[[lr_y, lr_x]];
        }
    }

    // Merge at high resolution (all in Bayer space)
    let mut merged = Array2::<f64>::zeros((hr_height, hr_width));

    // Count black pixels in A-grade SR for diagnostics
    let mut a_black_count = 0;
    let mut a_total_count = 0;

    for hr_y in 0..hr_height {
        for hr_x in 0..hr_width {
            let grade = grades_upsampled[[hr_y, hr_x]];

            merged[[hr_y, hr_x]] = match grade {
                g if g == Grade::A as u8 => {
                    a_total_count += 1;
                    let a_val = a_result[[hr_y, hr_x]];

                    // If A-grade SR has a hole, fall back to upsampled B/C
                    // CRITICAL: Must fill all holes before demosaic to avoid checkerboard
                    if a_val < 1e-10 {
                        a_black_count += 1;
                        b_upsampled[[hr_y, hr_x]].max(c_upsampled[[hr_y, hr_x]])
                    } else {
                        a_val
                    }
                }
                g if g == Grade::B as u8 => b_upsampled[[hr_y, hr_x]],
                g if g == Grade::C as u8 => c_upsampled[[hr_y, hr_x]],
                _ => c_upsampled[[hr_y, hr_x]], // D-grade uses C-grade result (background)
            };
        }
    }

    if a_total_count > 0 {
        let black_pct = (a_black_count as f64 / a_total_count as f64) * 100.0;
        tracing::info!(
            "A-grade SR has {} holes / {} total ({:.1}%) - will fill in RGB after demosaic",
            a_black_count,
            a_total_count,
            black_pct
        );
    }

    tracing::info!(
        "Bayer-aware merge complete: output {}x{}",
        hr_width,
        hr_height
    );

    Ok(merged)
}

/// Upsample Bayer image in a Bayer-aware way
/// Split into R/G1/G2/B, upsample each channel, recombine
fn upsample_bayer(bayer: &Array2<f64>, factor: usize) -> Result<Array2<f64>> {
    let (height, width) = bayer.dim();
    let h_half = height / 2;
    let w_half = width / 2;

    // Split into channels
    let mut r = Array2::<f64>::zeros((h_half, w_half));
    let mut g1 = Array2::<f64>::zeros((h_half, w_half));
    let mut g2 = Array2::<f64>::zeros((h_half, w_half));
    let mut b = Array2::<f64>::zeros((h_half, w_half));

    for y in 0..h_half {
        for x in 0..w_half {
            let by = y * 2;
            let bx = x * 2;

            r[[y, x]] = bayer[[by, bx]];
            g1[[y, x]] = bayer[[by, bx + 1]];
            g2[[y, x]] = bayer[[by + 1, bx]];
            b[[y, x]] = bayer[[by + 1, bx + 1]];
        }
    }

    // Upsample each channel using NEAREST NEIGHBOR (not bilinear)
    // Bilinear creates smooth gradients that mismatch sharp A-grade edges → artifacts
    // Nearest neighbor creates blocky fallback but no false edges
    let r_up = upsample_nearest(&r, factor);
    let g1_up = upsample_nearest(&g1, factor);
    let g2_up = upsample_nearest(&g2, factor);
    let b_up = upsample_nearest(&b, factor);

    // Recombine into Bayer pattern
    let (sr_h, sr_w) = r_up.dim();
    let bayer_height = sr_h * 2;
    let bayer_width = sr_w * 2;

    let mut bayer_up = Array2::<f64>::zeros((bayer_height, bayer_width));

    for y in 0..sr_h {
        for x in 0..sr_w {
            let by = y * 2;
            let bx = x * 2;

            bayer_up[[by, bx]] = r_up[[y, x]];
            bayer_up[[by, bx + 1]] = g1_up[[y, x]];
            bayer_up[[by + 1, bx]] = g2_up[[y, x]];
            bayer_up[[by + 1, bx + 1]] = b_up[[y, x]];
        }
    }

    Ok(bayer_up)
}

/// Upsample using nearest neighbor (blocky but no false edges)
fn upsample_nearest(image: &Array2<f64>, factor: usize) -> Array2<f64> {
    let (height, width) = image.dim();
    let new_height = height * factor;
    let new_width = width * factor;

    let mut upsampled = Array2::<f64>::zeros((new_height, new_width));

    for y in 0..new_height {
        for x in 0..new_width {
            let src_y = y / factor;
            let src_x = x / factor;
            upsampled[[y, x]] = image[[src_y, src_x]];
        }
    }

    upsampled
}

/// Compute Mertens quality weights for all images (FRAMES VERSION - no stacking required)
pub fn compute_mertens_weights_frames(
    frames: &[Array3<f64>],
    _exposures: &[f64],
    params: &HierarchicalParams,
) -> Result<MertensWeights> {
    if frames.is_empty() {
        anyhow::bail!("No frames provided");
    }

    let (height, width, channels) = frames[0].dim();
    let num_images = frames.len();

    if channels == 1 {
        if let Some(weights) = try_gpu_mertens_weights(frames, params.exposure_sigma) {
            return Ok(weights);
        }
    }

    // Compute weights in parallel for each image
    use rayon::prelude::*;
    let results: Vec<(f64, f64, f64)> = frames
        .par_iter()
        .map(|frame| {
            // Contrast: Laplacian variance (downsampled 4x for speed)
            let step = 4;
            let mut lap_sum = 0.0;
            let mut count = 0;
            for y in (step..height - step).step_by(step) {
                for x in (step..width - step).step_by(step) {
                    let lap = (4.0 * frame[[y, x, 0]]
                        - frame[[y - step, x, 0]]
                        - frame[[y + step, x, 0]]
                        - frame[[y, x - step, 0]]
                        - frame[[y, x + step, 0]])
                    .abs();
                    lap_sum += lap;
                    count += 1;
                }
            }
            let contrast = if count > 0 {
                lap_sum / count as f64
            } else {
                0.0
            };

            // Saturation: per-pixel channel stddev averaged over downsampled grid.
            // If we only have a single channel (Bayer/intensity), treat as neutral.
            let saturation = if frame.dim().2 >= 3 {
                let mut sat_sum = 0.0;
                let mut sat_count = 0;
                for y in (0..height).step_by(step) {
                    for x in (0..width).step_by(step) {
                        let r = frame[[y, x, 0]];
                        let g = frame[[y, x, 1]];
                        let b = frame[[y, x, 2]];
                        let mean = (r + g + b) / 3.0;
                        let var =
                            ((r - mean).powi(2) + (g - mean).powi(2) + (b - mean).powi(2)) / 3.0;
                        sat_sum += var.sqrt();
                        sat_count += 1;
                    }
                }
                if sat_count > 0 {
                    sat_sum / sat_count as f64
                } else {
                    1.0
                }
            } else {
                1.0
            };

            // Exposedness: Gaussian around 0.5 (use downsampled mean for speed)
            let sigma = params.exposure_sigma;
            let mut intensity_sum = 0.0;
            let mut intensity_count = 0;
            for y in (0..height).step_by(step) {
                for x in (0..width).step_by(step) {
                    intensity_sum += frame[[y, x, 0]];
                    intensity_count += 1;
                }
            }
            let mean_intensity = if intensity_count > 0 {
                intensity_sum / intensity_count as f64
            } else {
                0.5
            };
            let exposedness = (-(mean_intensity - 0.5).powi(2) / (2.0 * sigma * sigma)).exp();

            (contrast, saturation, exposedness)
        })
        .collect();

    // Unpack results
    let mut contrast = Vec::with_capacity(num_images);
    let mut saturation = Vec::with_capacity(num_images);
    let mut exposedness = Vec::with_capacity(num_images);
    for (c, s, e) in results {
        contrast.push(c);
        saturation.push(s);
        exposedness.push(e);
    }

    Ok(MertensWeights {
        contrast,
        saturation,
        exposedness,
    })
}

fn try_gpu_mertens_weights(frames: &[Array3<f64>], exposure_sigma: f64) -> Option<MertensWeights> {
    #[cfg(feature = "gpu")]
    {
        use crate::gpu::{get_gpu_context, gpu_compute_mertens_weights};

        let gpu_ctx = get_gpu_context()?;
        let stacked = match stack_frames_scalar(frames) {
            Ok(stacked) => stacked,
            Err(e) => {
                tracing::warn!("GPU Mertens stack failed: {}", e);
                return None;
            }
        };

        match gpu_compute_mertens_weights(&gpu_ctx, &stacked, &[], exposure_sigma) {
            Ok(Some(weights)) => Some(MertensWeights {
                contrast: weights.contrast,
                saturation: weights.saturation,
                exposedness: weights.exposedness,
            }),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!("GPU Mertens failed: {}", e);
                None
            }
        }
    }
    #[cfg(not(feature = "gpu"))]
    {
        let _ = frames;
        let _ = exposure_sigma;
        None
    }
}

fn stack_frames_scalar(frames: &[Array3<f64>]) -> Result<Array3<f64>> {
    let (height, width, channels) = frames[0].dim();
    if channels == 0 {
        anyhow::bail!("Frames have zero channels");
    }
    let mut stacked = Array3::<f64>::zeros((height, width, frames.len()));
    for (idx, frame) in frames.iter().enumerate() {
        let (fh, fw, fc) = frame.dim();
        if fh != height || fw != width {
            anyhow::bail!("Frame dimensions do not match for GPU stack");
        }
        if fc == 0 {
            anyhow::bail!("Frame has zero channels");
        }
        stacked
            .slice_mut(s![.., .., idx])
            .assign(&frame.slice(s![.., .., 0]));
    }
    Ok(stacked)
}

/// Select best exposure per focus plane (no HDR blending)
///
/// Instead of blending exposures with varying weights (which creates speckles),
/// select the single best exposure per focus plane based on Mertens weights.
///
/// # Arguments
/// * `weights` - Per-image weights (all focus planes concatenated)
/// * `num_exposures` - Number of exposures per focus plane
///
/// # Returns
/// Modified weights where only the best exposure per plane has weight=1.0, others=0.0
fn select_best_exposure_per_plane(weights: &[f64], num_exposures: usize) -> Vec<f64> {
    if num_exposures <= 1 {
        return weights.to_vec();
    }

    let num_planes = weights.len().div_ceil(num_exposures);
    let mut selected = vec![0.0; weights.len()];

    for plane_idx in 0..num_planes {
        let start = plane_idx * num_exposures;
        let end = (start + num_exposures).min(weights.len());
        let plane_weights = &weights[start..end];

        // Find exposure with maximum weight
        let mut best_idx = 0;
        let mut best_weight = plane_weights[0];
        for (i, &w) in plane_weights.iter().enumerate().skip(1) {
            if w > best_weight {
                best_weight = w;
                best_idx = i;
            }
        }

        // Set only the best exposure to weight=1.0, others=0.0
        selected[start + best_idx] = 1.0;
    }

    selected
}
