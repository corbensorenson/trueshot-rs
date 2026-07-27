//! Preprocessing: alignment, sharpness masking, background removal, and depth estimation.
//!
//! This module prepares raw Bayer frames for the collapse step by:
//! 1. Computing foreground bounding box via Otsu thresholding
//! 2. Aligning frames via FFT phase correlation
//! 3. Computing sharpness masks via Laplacian variance
//! 4. Removing background via Chan-Vese segmentation
//! 5. Estimating depth from focus stack

use crate::types::{BayerFrame, Meta, Rect};
use anyhow::Result;
use ndarray::{Array2, Array3};
use rayon::prelude::*;

/// Preprocessed frame stack ready for collapse
pub struct PreprocessedStack {
    /// Masked Bayer frames (H×W×1×N) - single channel Bayer
    pub frames: Vec<Array3<f64>>,
    /// Frame metadata only (for WB extraction, exposure normalization)
    /// This is much smaller than keeping full frames!
    pub frame_metadata: Vec<crate::types::FrameMeta>,
    /// Global foreground mask (H×W)
    pub fg_mask: Array2<bool>,
    /// Bounding box used for cropping
    pub bbox: Rect,
    /// Alignment shifts (dx, dy, scale) for each frame (for SR with focus breathing)
    pub alignments: Vec<(f64, f64, f64)>,
}

/// Preprocess a sequence of Bayer frames
pub fn preprocess_stack(
    frames: Vec<BayerFrame>,
    meta: &Meta,
    noise_sigma: f64,
    skip_physical_alignment: bool,
) -> Result<PreprocessedStack> {
    preprocess_stack_with_options(frames, meta, noise_sigma, skip_physical_alignment, false)
}

/// Preprocess a sequence of Bayer frames with options
pub fn preprocess_stack_with_options(
    frames: Vec<BayerFrame>,
    meta: &Meta,
    _noise_sigma: f64,
    skip_physical_alignment: bool,
    skip_sharpness_masks: bool,
) -> Result<PreprocessedStack> {
    use std::time::Instant;

    tracing::info!(
        "Preprocessing {} frames (skip_physical_alignment={}, skip_sharpness_masks={})",
        frames.len(),
        skip_physical_alignment,
        skip_sharpness_masks
    );

    // 1. Skip bbox computation - frames are already cropped from selective loading!
    let ref_idx = compute_ref_index(meta);
    let (h, w, _) = frames[0].data.dim();
    let bbox = Rect {
        x: 0.0,
        y: 0.0,
        width: w as f64,
        height: h as f64,
    };
    tracing::debug!("Using full frame bbox (already cropped): {}x{}", w, h);

    // 2. Extract metadata FIRST before consuming frames
    let t0 = Instant::now();
    let frame_metadata: Vec<crate::types::FrameMeta> =
        frames.iter().map(|f| f.meta.clone()).collect();
    tracing::info!(
        "⏱️  Metadata extraction: {:.1}ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );

    // 3. Extract frame data by consuming frames (no clone!)
    let t1 = Instant::now();
    let mut frame_data: Vec<Array3<f64>> = frames.into_iter().map(|f| f.data).collect();
    tracing::info!(
        "⏱️  Frame data extraction (zero-copy): {:.1}ms",
        t1.elapsed().as_secs_f64() * 1000.0
    );

    // 3.5. Apply white balance to Bayer data BEFORE HDR fusion
    // This is critical! If we apply WB after fusion, the per-pixel HDR weights
    // will be computed on raw Bayer values, causing R/G/B to be weighted differently
    let t1b = Instant::now();
    let cam_mul = frame_metadata[0].cam_mul;
    tracing::info!(
        "Applying white balance to input frames: R={:.3}, G={:.3}, B={:.3}, G2={:.3}",
        cam_mul[0],
        cam_mul[1],
        cam_mul[2],
        cam_mul[3]
    );

    // Normalize by green channel
    let green_mul = cam_mul[1].max(cam_mul[3]);
    let wb_r = (cam_mul[0] / green_mul) as f64;
    let wb_g = ((cam_mul[1] + cam_mul[3]) / (2.0 * green_mul)) as f64;
    let wb_b = (cam_mul[2] / green_mul) as f64;

    tracing::info!(
        "Normalized WB multipliers: R={:.3}, G={:.3}, B={:.3}",
        wb_r,
        wb_g,
        wb_b
    );

    // Apply WB to each frame
    for frame in &mut frame_data {
        let (h, w, _) = (frame.shape()[0], frame.shape()[1], frame.shape()[2]);
        for y in 0..h {
            for x in 0..w {
                let row_even = y % 2 == 0;
                let col_even = x % 2 == 0;

                let multiplier = match (row_even, col_even) {
                    (true, true) => wb_r,                  // R
                    (true, false) | (false, true) => wb_g, // G
                    (false, false) => wb_b,                // B
                };

                frame[[y, x, 0]] *= multiplier;
            }
        }
    }
    tracing::info!(
        "⏱️  White balance: {:.1}ms",
        t1b.elapsed().as_secs_f64() * 1000.0
    );

    // Debug: Check frame values after WB
    if !frame_data.is_empty() {
        let frame = &frame_data[0];
        let (h, w, _) = (frame.shape()[0], frame.shape()[1], frame.shape()[2]);
        let mut sum = 0.0;
        let mut count = 0;
        for y in (0..h).step_by(8) {
            for x in (0..w).step_by(8) {
                sum += frame[[y, x, 0]];
                count += 1;
            }
        }
        let mean = if count > 0 { sum / count as f64 } else { 0.0 };
        tracing::info!("Frame 0 after WB: mean={:.3}", mean);
    }

    // 4. Compute alignment shifts (and optionally apply them)
    // ENABLED: Compute scale for focus breathing compensation (Attempt 89)
    let skip_all_alignment = false;

    let t2 = Instant::now();
    let (aligned, alignments) = if skip_all_alignment {
        tracing::info!("SKIPPING alignment (tripod-mounted, testing performance)");
        // Return frames as-is with zero shifts
        let zero_shifts = vec![(0.0, 0.0, 1.0); frame_data.len()];
        (frame_data, zero_shifts)
    } else if skip_physical_alignment {
        // SR mode: Apply focus-plane shifts, return per-exposure subpixel shifts
        tracing::info!("SR-aware alignment: applying focus-plane shifts, computing per-exposure subpixel shifts");
        let (aligned, per_exposure_shifts) = align_frames_for_sr(&frame_data, meta, ref_idx)?;
        tracing::info!("SR-aware alignment complete");
        (aligned, per_exposure_shifts)
    } else {
        // Traditional mode: Compute shifts and warp frames (per-focus-plane)
        tracing::info!(
            "Aligning {} frames to reference frame {} (optimized per-focus-plane)",
            frame_data.len(),
            ref_idx
        );
        let (aligned, alignments) = align_frames_optimized(&frame_data, meta, ref_idx)?;
        tracing::info!("Frame alignment complete");
        (aligned, alignments)
    };
    tracing::info!(
        "⏱️  Alignment: {:.1}ms",
        t2.elapsed().as_secs_f64() * 1000.0
    );

    // 5. Compute background mask from REFERENCE frame only (like original pixelcollapse)
    let t3 = Instant::now();
    let fg_mask = compute_background_mask_from_reference(&aligned[ref_idx])?;
    tracing::info!(
        "⏱️  Background mask: {:.1}ms",
        t3.elapsed().as_secs_f64() * 1000.0
    );

    // Debug: Check mask
    let mask_true_count = fg_mask.iter().filter(|&&b| b).count();
    tracing::info!(
        "Foreground mask: {} / {} pixels ({:.1}%)",
        mask_true_count,
        fg_mask.len(),
        100.0 * mask_true_count as f64 / fg_mask.len() as f64
    );

    // 6. DON'T apply mask to frames - keep all data for fusion
    // Mask will be used for output only
    let masked_frames = aligned;

    Ok(PreprocessedStack {
        frames: masked_frames,
        frame_metadata,
        fg_mask,
        bbox,
        alignments,
    })
}

/// Compute reference frame index
fn compute_ref_index(meta: &Meta) -> usize {
    let exp_idx = meta
        .exposures
        .iter()
        .position(|&e| (e - meta.ref_exp).abs() < 0.01)
        .unwrap_or(meta.exposures.len() / 2);

    meta.ref_focus as usize * meta.exposures.len() + exp_idx
}

/// Compute foreground bounding box via Otsu thresholding
/// Align frames to reference using FFT phase correlation (OPTIMIZED)
///
/// Key optimization: Only compute alignment once per focus plane.
/// Assumes all exposures within a focus plane are aligned (same focus position).
/// This reduces alignment computations from N to focus_steps.
///
/// For example, with 7 focus planes × 3 exposures = 21 frames:
/// - Old: 20 alignment computations
/// - New: 6 alignment computations (one per non-reference focus plane)
fn align_frames_optimized(
    frames: &[Array3<f64>],
    meta: &Meta,
    ref_idx: usize,
) -> Result<(Vec<Array3<f64>>, Vec<(f64, f64, f64)>)> {
    let num_focus_steps = meta.focus_steps as usize;
    let num_exposures = meta.exposures.len();
    let total_frames = frames.len();

    tracing::info!(
        "Optimized alignment: {} focus planes × {} exposures = {} frames",
        num_focus_steps,
        num_exposures,
        total_frames
    );

    if total_frames != num_focus_steps * num_exposures {
        tracing::warn!(
            "Frame count mismatch: expected {}×{}={}, got {}",
            num_focus_steps,
            num_exposures,
            num_focus_steps * num_exposures,
            total_frames
        );
    }

    let reference = &frames[ref_idx];
    let ref_focus_step = ref_idx / num_exposures;

    tracing::info!(
        "Reference frame {} is in focus plane {}",
        ref_idx,
        ref_focus_step
    );

    // Step 1: Compute one shift+scale per focus plane (using first exposure of each plane)
    // Pattern: F0E0, F0E1, F0E2, F1E0, F1E1, F1E2, ..., F6E0, F6E1, F6E2
    let mut focus_plane_shifts: Vec<(f64, f64, f64)> = Vec::with_capacity(num_focus_steps);

    for focus_step in 0..num_focus_steps {
        if focus_step == ref_focus_step {
            // Reference focus plane - no shift, no scale change
            focus_plane_shifts.push((0.0, 0.0, 1.0));
            tracing::debug!(
                "Focus plane {}: reference (no shift, scale=1.0)",
                focus_step
            );
        } else {
            // Compute shift AND scale using first exposure (E0) of this focus plane
            let frame_idx = focus_step * num_exposures; // First exposure of this focus plane
            if frame_idx < total_frames {
                let (dx, dy, scale) = compute_phase_correlation(reference, &frames[frame_idx]);
                focus_plane_shifts.push((dx, dy, scale));
                tracing::info!(
                    "Focus plane {}: shift=({:.3}, {:.3}), scale={:.4} (frame {})",
                    focus_step,
                    dx,
                    dy,
                    scale,
                    frame_idx
                );
            } else {
                focus_plane_shifts.push((0.0, 0.0, 1.0));
                tracing::warn!(
                    "Focus plane {}: frame index {} out of bounds",
                    focus_step,
                    frame_idx
                );
            }
        }
    }

    // Step 2: Build per-frame alignment list
    let mut alignments: Vec<(f64, f64, f64)> = Vec::with_capacity(total_frames);
    for i in 0..total_frames {
        let focus_step = i / num_exposures;
        alignments.push(focus_plane_shifts[focus_step]);
    }

    // Step 3: DON'T apply scale correction here - let the pipeline handle it
    // The pipeline will apply focus breathing compensation during collapse
    // This avoids double-compensation and allows per-pixel focus plane selection to work correctly
    tracing::info!(
        "Computed scale for {} frames (will be applied in pipeline)",
        total_frames
    );

    // Return frames as-is (no rescaling in preprocessing)
    let aligned = frames.to_vec();

    Ok((aligned, alignments))
}

/// Align frames with per-exposure subpixel shifts for super-resolution
///
/// This function computes TWO types of shifts:
/// 1. **Focus-plane shifts** (large, hundreds of pixels): For aligning different focus planes
/// 2. **Per-exposure shifts** (tiny, < 1 pixel): Camera shake/vibration within each focus plane
///
/// The focus-plane shifts are applied to physically align the frames.
/// The per-exposure shifts are returned for SR accumulation (subpixel precision).
///
/// For example, with 7 focus planes × 3 exposures = 21 frames:
/// - Compute 6 focus-plane shifts (one per non-reference plane)
/// - Compute 21 per-exposure shifts (relative to first exposure in each plane)
/// - Apply focus-plane shifts to frames
/// - Return per-exposure shifts for SR
fn align_frames_for_sr(
    frames: &[Array3<f64>],
    meta: &Meta,
    ref_idx: usize,
) -> Result<(Vec<Array3<f64>>, Vec<(f64, f64, f64)>)> {
    let num_focus_steps = meta.focus_steps as usize;
    let num_exposures = meta.exposures.len();
    let total_frames = frames.len();

    tracing::info!(
        "SR-aware alignment: {} focus planes × {} exposures = {} frames",
        num_focus_steps,
        num_exposures,
        total_frames
    );

    if total_frames != num_focus_steps * num_exposures {
        tracing::warn!(
            "Frame count mismatch: expected {}×{}={}, got {}",
            num_focus_steps,
            num_exposures,
            num_focus_steps * num_exposures,
            total_frames
        );
    }

    let reference = &frames[ref_idx];
    let ref_focus_step = ref_idx / num_exposures;
    let ref_exposure_idx = ref_idx % num_exposures;

    tracing::info!(
        "Reference frame {} is in focus plane {}, exposure {}",
        ref_idx,
        ref_focus_step,
        ref_exposure_idx
    );

    // Step 1: Compute focus-plane shifts (large shifts for alignment) + scale
    let mut focus_plane_shifts: Vec<(f64, f64, f64)> = Vec::with_capacity(num_focus_steps);

    for focus_step in 0..num_focus_steps {
        if focus_step == ref_focus_step {
            focus_plane_shifts.push((0.0, 0.0, 1.0));
            tracing::debug!(
                "Focus plane {}: reference (no shift, scale=1.0)",
                focus_step
            );
        } else {
            let frame_idx = focus_step * num_exposures;
            if frame_idx < total_frames {
                let (dx, dy, scale) = compute_phase_correlation(reference, &frames[frame_idx]);
                focus_plane_shifts.push((dx, dy, scale));
                tracing::info!(
                    "Focus plane {}: shift=({:.3}, {:.3}), scale={:.4} (frame {})",
                    focus_step,
                    dx,
                    dy,
                    scale,
                    frame_idx
                );
            } else {
                focus_plane_shifts.push((0.0, 0.0, 1.0));
                tracing::warn!(
                    "Focus plane {}: frame index {} out of bounds",
                    focus_step,
                    frame_idx
                );
            }
        }
    }

    // Step 2: Apply focus-plane shifts to physically align frames
    let aligned: Vec<Array3<f64>> = frames
        .par_iter()
        .enumerate()
        .map(|(i, frame)| {
            let focus_step = i / num_exposures;
            let shift = focus_plane_shifts[focus_step];

            if shift.0.abs() < 1e-6 && shift.1.abs() < 1e-6 {
                frame.clone()
            } else {
                shift_frame(frame, shift.0, shift.1)
            }
        })
        .collect();

    // Step 3: Compute per-exposure subpixel shifts WITHIN each focus plane
    // These are the tiny shifts from camera shake/vibration + scale from focus breathing
    let mut per_exposure_shifts: Vec<(f64, f64, f64)> = Vec::with_capacity(total_frames);

    for focus_step in 0..num_focus_steps {
        // Reference exposure within this focus plane (usually E0 or middle exposure)
        let plane_ref_idx = focus_step * num_exposures;

        if plane_ref_idx >= total_frames {
            // Fill with zeros if out of bounds
            for _ in 0..num_exposures {
                per_exposure_shifts.push((0.0, 0.0, 1.0));
            }
            continue;
        }

        let plane_reference = &aligned[plane_ref_idx];

        for exposure_idx in 0..num_exposures {
            let frame_idx = focus_step * num_exposures + exposure_idx;

            if frame_idx >= total_frames {
                per_exposure_shifts.push((0.0, 0.0, 1.0));
                continue;
            }

            if exposure_idx == 0 {
                // First exposure is the reference within this plane
                per_exposure_shifts.push((0.0, 0.0, 1.0));
            } else {
                // Compute subpixel shift + scale relative to first exposure in this plane
                let (dx, dy, scale) =
                    compute_phase_correlation(plane_reference, &aligned[frame_idx]);
                per_exposure_shifts.push((dx, dy, scale));
                tracing::debug!(
                    "Frame {} (F{}E{}): shift=({:.3}, {:.3}), scale={:.4}",
                    frame_idx,
                    focus_step,
                    exposure_idx,
                    dx,
                    dy,
                    scale
                );
            }
        }
    }

    // Log statistics on per-exposure shifts
    let mut dx_values: Vec<f64> = per_exposure_shifts
        .iter()
        .map(|(dx, _, _)| dx.abs())
        .collect();
    let mut dy_values: Vec<f64> = per_exposure_shifts
        .iter()
        .map(|(_, dy, _)| dy.abs())
        .collect();
    dx_values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    dy_values.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let dx_median = dx_values[dx_values.len() / 2];
    let dy_median = dy_values[dy_values.len() / 2];
    let dx_max = dx_values[dx_values.len() - 1];
    let dy_max = dy_values[dy_values.len() - 1];

    tracing::info!(
        "Per-exposure subpixel shifts: dx median={:.3}, max={:.3}; dy median={:.3}, max={:.3}",
        dx_median,
        dx_max,
        dy_median,
        dy_max
    );

    Ok((aligned, per_exposure_shifts))
}

/// Align frames via FFT phase correlation on green channel (OLD - NOT USED)
#[allow(dead_code)]
fn align_frames(frames: &[Array3<f64>], ref_idx: usize) -> Result<Vec<Array3<f64>>> {
    tracing::debug!("Aligning {} frames to reference {}", frames.len(), ref_idx);

    let reference = &frames[ref_idx];

    let aligned: Vec<Array3<f64>> = frames
        .par_iter()
        .enumerate()
        .map(|(i, frame)| {
            if i == ref_idx {
                tracing::debug!("Frame {}: reference (no shift)", i);
                frame.clone()
            } else {
                // Compute shift via phase correlation
                let shift = compute_phase_correlation(reference, frame);

                tracing::info!(
                    "Frame {}: shift = ({:.3}, {:.3}) pixels",
                    i,
                    shift.0,
                    shift.1
                );

                // Apply shift
                shift_frame(frame, shift.0, shift.1)
            }
        })
        .collect();

    Ok(aligned)
}

/// Compute phase correlation WITH scale between two frames (for focus breathing SR)
fn compute_phase_correlation(ref_frame: &Array3<f64>, frame: &Array3<f64>) -> (f64, f64, f64) {
    // Use the professional FFT-based phase correlation from align_raw.rs
    // Extract single-channel Bayer data (channel 0)
    let (height, width, _) = ref_frame.dim();

    // Extract 2D arrays from the 3D Bayer frames
    let mut ref_2d = Array2::<f64>::zeros((height, width));
    let mut frame_2d = Array2::<f64>::zeros((height, width));

    for y in 0..height {
        for x in 0..width {
            ref_2d[[y, x]] = ref_frame[[y, x, 0]];
            frame_2d[[y, x]] = frame[[y, x, 0]];
        }
    }

    // Use 3-level pyramid for speed + scale estimation
    let (dx, dy, scale) = crate::align_raw::align_phasecorr_bayer_with_scale(&ref_2d, &frame_2d, 3);

    tracing::debug!(
        "Phase correlation: shift=({:.2}, {:.2}), scale={:.4}",
        dx,
        dy,
        scale
    );

    (dx, dy, scale)
}

/// Rescale a Bayer frame to correct for focus breathing (magnification changes)
///
/// This function rescales the Bayer grid by sampling at the correct positions
/// to maintain the RGGB pattern. For tripod-mounted shots, we only need scale
/// correction (no x/y shifts).
///
/// Key insight: We resample the Bayer grid at positions that preserve the pattern:
/// - R pixels stay at (even, even) positions
/// - G pixels stay at (even, odd) and (odd, even) positions
/// - B pixels stay at (odd, odd) positions
///
/// # Arguments
/// * `frame` - Input Bayer frame (H × W × 1)
/// * `scale` - Scale factor (>1.0 = zoom in, <1.0 = zoom out)
///
/// # Returns
/// Rescaled Bayer frame with preserved RGGB pattern
#[expect(
    dead_code,
    reason = "legacy Bayer-space alignment baseline retained for regression comparisons"
)]
fn rescale_bayer_frame(frame: &Array3<f64>, scale: f64) -> Result<Array3<f64>> {
    let (height, width, _) = frame.dim();
    let mut rescaled = Array3::<f64>::zeros((height, width, 1));

    // Center of image (scale around this point)
    let cy = height as f64 / 2.0;
    let cx = width as f64 / 2.0;

    for y in 0..height {
        for x in 0..width {
            // Map output position to input position (inverse scale)
            // (x_out - cx) = scale * (x_in - cx)
            // x_in = (x_out - cx) / scale + cx
            let x_src = ((x as f64) - cx) / scale + cx;
            let y_src = ((y as f64) - cy) / scale + cy;

            // Determine Bayer color at OUTPUT position (y, x)
            let color = get_bayer_color_local(y, x);

            // Sample from INPUT frame at positions with the SAME color
            // This preserves the Bayer pattern
            let value = sample_bayer_channel(frame, y_src, x_src, color, height, width);
            rescaled[[y, x, 0]] = value;
        }
    }

    Ok(rescaled)
}

/// Sample a specific Bayer channel at fractional coordinates
///
/// Uses bilinear interpolation between the 4 nearest pixels of the same color
/// This is much faster than weighted averaging over a large window
fn sample_bayer_channel(
    frame: &Array3<f64>,
    y_src: f64,
    x_src: f64,
    target_color: usize,
    height: usize,
    width: usize,
) -> f64 {
    // For Bayer pattern, pixels of the same color are spaced 2 pixels apart
    // Find the 4 nearest pixels of the target color and do bilinear interpolation

    // Map to the grid of pixels with target color
    // For R (0,0): grid is (0,0), (0,2), (2,0), (2,2), ...
    // For G (0,1) or (1,0): two grids
    // For B (1,1): grid is (1,1), (1,3), (3,1), (3,3), ...

    let (y_offset, x_offset) = match target_color {
        0 => (0, 0), // R at (even, even)
        1 => {
            // G at (even, odd) or (odd, even) - pick closest
            if (y_src as usize) % 2 == 0 {
                (0, 1) // (even, odd)
            } else {
                (1, 0) // (odd, even)
            }
        }
        2 => (1, 1), // B at (odd, odd)
        _ => unreachable!(),
    };

    // Map source position to the color grid (spaced by 2)
    let y_grid = (y_src - y_offset as f64) / 2.0;
    let x_grid = (x_src - x_offset as f64) / 2.0;

    // Find 4 corners in the color grid
    let y0_grid = y_grid.floor().max(0.0) as usize;
    let x0_grid = x_grid.floor().max(0.0) as usize;

    // Convert back to image coordinates
    let y0 = y0_grid * 2 + y_offset;
    let x0 = x0_grid * 2 + x_offset;
    let y1 = (y0 + 2).min(height - 1);
    let x1 = (x0 + 2).min(width - 1);

    // Bounds check
    if y0 >= height || x0 >= width {
        return 0.0;
    }

    // Bilinear interpolation weights
    let fy = (y_grid - y0_grid as f64).clamp(0.0, 1.0);
    let fx = (x_grid - x0_grid as f64).clamp(0.0, 1.0);

    // Sample 4 corners
    let v00 = frame[[y0, x0, 0]];
    let v01 = frame[[y0, x1, 0]];
    let v10 = frame[[y1, x0, 0]];
    let v11 = frame[[y1, x1, 0]];

    // Bilinear interpolation
    (1.0 - fx) * (1.0 - fy) * v00 + fx * (1.0 - fy) * v01 + (1.0 - fx) * fy * v10 + fx * fy * v11
}

/// Get Bayer color at position (y, x)
/// Returns: 0=R, 1=G, 2=B
fn get_bayer_color_local(y: usize, x: usize) -> usize {
    match (y % 2, x % 2) {
        (0, 0) => 0, // R
        (0, 1) => 1, // G
        (1, 0) => 1, // G
        (1, 1) => 2, // B
        _ => unreachable!(),
    }
}

/// Shift AND scale frame using simple bilinear interpolation (for focus breathing correction)
///
/// IMPORTANT: This operates on RAW BAYER data, NOT RGB!
/// We use simple bilinear interpolation without trying to preserve the Bayer pattern.
/// The Bayer pattern will be "broken" after transformation, but that's OK because
/// we'll do joint demosaicing later which handles this correctly.
#[expect(
    dead_code,
    reason = "legacy Bayer-space alignment baseline retained for regression comparisons"
)]
fn shift_and_scale_frame(frame: &Array3<f64>, dx: f64, dy: f64, scale: f64) -> Array3<f64> {
    let (height, width, channels) = frame.dim();

    // If no transformation needed, return original
    if dx.abs() < 0.01 && dy.abs() < 0.01 && (scale - 1.0).abs() < 0.001 {
        return frame.clone();
    }

    let mut transformed = Array3::<f64>::zeros((height, width, channels));

    // Center of image (for scaling)
    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;

    // Simple bilinear interpolation (don't try to preserve Bayer pattern)
    for y in 0..height {
        for x in 0..width {
            // Apply inverse transformation: scale around center, then shift
            // dest = scale * (src - center) + center + shift
            // => src = (dest - center - shift) / scale + center
            let dest_x = x as f64;
            let dest_y = y as f64;

            // Inverse transform to find source coordinates
            let src_x = (dest_x - center_x - dx) / scale + center_x;
            let src_y = (dest_y - center_y - dy) / scale + center_y;

            // Bounds check
            if src_x < 0.0
                || src_y < 0.0
                || src_x >= (width - 1) as f64
                || src_y >= (height - 1) as f64
            {
                // Out of bounds - set to 0
                transformed[[y, x, 0]] = 0.0;
                continue;
            }

            // Bilinear interpolation
            let x0 = src_x.floor() as usize;
            let y0 = src_y.floor() as usize;
            let x1 = (x0 + 1).min(width - 1);
            let y1 = (y0 + 1).min(height - 1);

            let fx = src_x - x0 as f64;
            let fy = src_y - y0 as f64;

            let v00 = frame[[y0, x0, 0]];
            let v01 = frame[[y0, x1, 0]];
            let v10 = frame[[y1, x0, 0]];
            let v11 = frame[[y1, x1, 0]];

            let v0 = v00 * (1.0 - fx) + v01 * fx;
            let v1 = v10 * (1.0 - fx) + v11 * fx;
            let value = v0 * (1.0 - fy) + v1 * fy;

            transformed[[y, x, 0]] = value.max(0.0);
        }
    }

    transformed
}

/// Shift frame by subpixel amount using Bayer-aware bilinear interpolation
fn shift_frame(frame: &Array3<f64>, dx: f64, dy: f64) -> Array3<f64> {
    let (height, width, channels) = frame.dim();

    // If shift is negligible, return original
    if dx.abs() < 0.01 && dy.abs() < 0.01 {
        return frame.clone();
    }

    let mut shifted = Array3::<f64>::zeros((height, width, channels));

    // CRITICAL: Bayer-aware bilinear interpolation
    // We must sample from pixels of the SAME COLOR to preserve the Bayer pattern
    // RGGB pattern: R at (even,even), G at (even,odd) and (odd,even), B at (odd,odd)

    for y in 0..height {
        for x in 0..width {
            // Source coordinates (with shift)
            let src_x = x as f64 - dx;
            let src_y = y as f64 - dy;

            // Bounds check with margin for interpolation
            if src_x < 1.0
                || src_y < 1.0
                || src_x >= (width - 2) as f64
                || src_y >= (height - 2) as f64
            {
                // Out of bounds - set to 0
                shifted[[y, x, 0]] = 0.0;
                continue;
            }

            // Determine color of destination pixel
            let dest_color = match (y % 2, x % 2) {
                (0, 0) => 0,          // R
                (0, 1) | (1, 0) => 1, // G
                (1, 1) => 2,          // B
                _ => unreachable!(),
            };

            // For Bayer pattern, we need to sample from a 2x2 grid of pixels of the SAME color
            // This means we need to find the 4 nearest pixels of the same color

            // Round to nearest pixel of same color
            let base_x = if dest_color == 0 {
                // R: even columns
                ((src_x / 2.0).floor() * 2.0) as usize
            } else if dest_color == 2 {
                // B: odd columns
                ((src_x / 2.0).floor() * 2.0 + 1.0) as usize
            } else {
                // G: depends on row
                if y % 2 == 0 {
                    // Even row: G at odd columns
                    ((src_x / 2.0).floor() * 2.0 + 1.0) as usize
                } else {
                    // Odd row: G at even columns
                    ((src_x / 2.0).floor() * 2.0) as usize
                }
            };

            let base_y = if dest_color == 0 {
                // R: even rows
                ((src_y / 2.0).floor() * 2.0) as usize
            } else if dest_color == 2 {
                // B: odd rows
                ((src_y / 2.0).floor() * 2.0 + 1.0) as usize
            } else {
                // G: depends on column
                if x % 2 == 1 {
                    // Odd column: G at even rows
                    ((src_y / 2.0).floor() * 2.0) as usize
                } else {
                    // Even column: G at odd rows
                    ((src_y / 2.0).floor() * 2.0 + 1.0) as usize
                }
            };

            // Compute fractional offset within the 2x2 grid of same-color pixels
            let fx = (src_x - base_x as f64) / 2.0;
            let fy = (src_y - base_y as f64) / 2.0;

            // Bilinear interpolation from 4 pixels of same color
            // These are spaced 2 pixels apart in the Bayer pattern
            let x0 = base_x;
            let x1 = base_x + 2;
            let y0 = base_y;
            let y1 = base_y + 2;

            if x1 < width && y1 < height {
                let v00 = frame[[y0, x0, 0]];
                let v10 = frame[[y0, x1, 0]];
                let v01 = frame[[y1, x0, 0]];
                let v11 = frame[[y1, x1, 0]];

                // Bilinear interpolation
                let v0 = v00 * (1.0 - fx) + v10 * fx;
                let v1 = v01 * (1.0 - fx) + v11 * fx;
                shifted[[y, x, 0]] = v0 * (1.0 - fy) + v1 * fy;
            }
        }
    }

    shifted
}

/// Compute background mask from reference frame only (like original pixelcollapse)
/// Uses simple intensity thresholding + connected components + morphology
fn compute_background_mask_from_reference(frame: &Array3<f64>) -> Result<Array2<bool>> {
    tracing::debug!("Computing background mask from reference frame");

    let (height, width, _) = frame.dim();

    // Extract luminance from Bayer pattern
    // Single-channel Bayer: all pixels in channel 0
    let mut gray = Array2::<f64>::zeros((height, width));
    for y in 0..height {
        for x in 0..width {
            // Single-channel Bayer: all values in channel 0
            gray[[y, x]] = frame[[y, x, 0]];
        }
    }

    // Find max brightness for normalization
    let max_luma = gray.iter().copied().fold(0.0, f64::max);

    // Use 2% threshold (more lenient than 5%)
    let threshold = max_luma * 0.02;
    tracing::info!(
        "Background threshold: {:.6} (2% of max {:.6})",
        threshold,
        max_luma
    );

    // Create binary mask
    let mut mask = Array2::<bool>::from_elem((height, width), false);
    let mut above_threshold = 0;
    for y in 0..height {
        for x in 0..width {
            if gray[[y, x]] >= threshold {
                mask[[y, x]] = true;
                above_threshold += 1;
            }
        }
    }

    let threshold_pct = (above_threshold as f64 / (width * height) as f64) * 100.0;
    tracing::info!(
        "Pixels above threshold: {} ({:.1}%)",
        above_threshold,
        threshold_pct
    );

    // Find connected components
    let components = find_connected_components_2d(&mask);
    tracing::info!("Found {} connected components", components.len());

    if components.is_empty() {
        // No components found - just use all pixels above threshold
        tracing::warn!("No connected components found, using all pixels above threshold");
        return Ok(mask);
    }

    // Find largest component (the object)
    let largest = components.iter().max_by_key(|c| c.len()).unwrap(); // Safe because we checked components.is_empty()

    let object_pct = (largest.len() as f64 / (width * height) as f64) * 100.0;
    tracing::info!(
        "Largest component: {} pixels ({:.1}%)",
        largest.len(),
        object_pct
    );

    // Create new mask with only largest component
    mask.fill(false);
    for &(y, x) in largest {
        mask[[y, x]] = true;
    }

    // Fill holes (e.g., nasal cavity) - these are part of the bone, not background
    let filled_mask = fill_holes(&mask);
    let filled_pct =
        (filled_mask.iter().filter(|&&b| b).count() as f64 / (width * height) as f64) * 100.0;
    tracing::info!("After hole filling: {:.1}% object pixels", filled_pct);

    // Remove small isolated regions near borders BEFORE morphological operations
    // This prevents them from being connected to the main object
    let cleaned_mask = remove_border_artifacts(&filled_mask, 50, 1000);

    let cleaned_pct =
        (cleaned_mask.iter().filter(|&&b| b).count() as f64 / (width * height) as f64) * 100.0;
    tracing::info!(
        "After removing border artifacts: {:.1}% object pixels",
        cleaned_pct
    );

    // Morphological smoothing to clean up jagged edges
    // Increased radius from 1 to 2 for smoother edges
    let smoothed = morphology_close(&cleaned_mask, 2); // Close = dilate then erode

    let final_pct =
        (smoothed.iter().filter(|&&b| b).count() as f64 / (width * height) as f64) * 100.0;
    tracing::info!(
        "Final mask: {:.1}% object pixels after morphology",
        final_pct
    );

    Ok(smoothed)
}

/// Find connected components in binary mask
/// Returns list of components, each component is a list of (y, x) coordinates
fn find_connected_components_2d(mask: &Array2<bool>) -> Vec<Vec<(usize, usize)>> {
    let (height, width) = mask.dim();
    let mut visited = Array2::<bool>::from_elem((height, width), false);
    let mut components = Vec::new();

    for y in 0..height {
        for x in 0..width {
            if mask[[y, x]] && !visited[[y, x]] {
                // Start new component with flood fill
                let mut component = Vec::new();
                let mut stack = vec![(y, x)];

                while let Some((cy, cx)) = stack.pop() {
                    if visited[[cy, cx]] {
                        continue;
                    }
                    visited[[cy, cx]] = true;
                    component.push((cy, cx));

                    // Check 4-connected neighbors
                    if cy > 0 && mask[[cy - 1, cx]] && !visited[[cy - 1, cx]] {
                        stack.push((cy - 1, cx));
                    }
                    if cy < height - 1 && mask[[cy + 1, cx]] && !visited[[cy + 1, cx]] {
                        stack.push((cy + 1, cx));
                    }
                    if cx > 0 && mask[[cy, cx - 1]] && !visited[[cy, cx - 1]] {
                        stack.push((cy, cx - 1));
                    }
                    if cx < width - 1 && mask[[cy, cx + 1]] && !visited[[cy, cx + 1]] {
                        stack.push((cy, cx + 1));
                    }
                }

                if component.len() > 100 {
                    // Filter out tiny noise components
                    components.push(component);
                }
            }
        }
    }

    components
}

/// Morphological dilation
fn morphology_dilate(mask: &Array2<bool>, radius: usize) -> Array2<bool> {
    let (height, width) = mask.dim();
    let mut result = mask.clone();

    for y in radius..height - radius {
        for x in radius..width - radius {
            let mut any_true = false;
            for dy in 0..=2 * radius {
                for dx in 0..=2 * radius {
                    if mask[[y - radius + dy, x - radius + dx]] {
                        any_true = true;
                        break;
                    }
                }
                if any_true {
                    break;
                }
            }
            result[[y, x]] = any_true;
        }
    }

    result
}

/// Morphological erosion
fn morphology_erode(mask: &Array2<bool>, radius: usize) -> Array2<bool> {
    let (height, width) = mask.dim();
    let mut result = mask.clone();

    for y in radius..height - radius {
        for x in radius..width - radius {
            let mut all_true = true;
            for dy in 0..=2 * radius {
                for dx in 0..=2 * radius {
                    if !mask[[y - radius + dy, x - radius + dx]] {
                        all_true = false;
                        break;
                    }
                }
                if !all_true {
                    break;
                }
            }
            result[[y, x]] = all_true;
        }
    }

    result
}

/// Morphological closing (dilate then erode) - smooths edges and fills small gaps
fn morphology_close(mask: &Array2<bool>, radius: usize) -> Array2<bool> {
    let dilated = morphology_dilate(mask, radius);
    morphology_erode(&dilated, radius)
}

/// Remove small isolated regions near image borders
/// These are likely background artifacts that shouldn't be part of the object
/// Strategy: Erode to disconnect thin border connections, remove small components, then restore
fn remove_border_artifacts(
    mask: &Array2<bool>,
    _border_width: usize,
    _min_size: usize,
) -> Array2<bool> {
    let (height, width) = mask.dim();

    // Step 1: Very light erosion to disconnect thin border connections (reduced from 2 to 1)
    let eroded = morphology_erode(mask, 1);

    // Step 2: Find connected components in eroded mask
    let components = find_connected_components_2d(&eroded);

    // Step 3: Identify the largest component (main object)
    let largest_component = components
        .iter()
        .max_by_key(|c| c.len())
        .cloned()
        .unwrap_or_default();

    // Step 4: Create mask with only the largest component
    let mut main_object_mask = Array2::<bool>::from_elem((height, width), false);
    for &(y, x) in &largest_component {
        main_object_mask[[y, x]] = true;
    }

    // Step 5: Dilate back to restore original size (reduced from 2 to 1)
    let restored = morphology_dilate(&main_object_mask, 1);

    // Step 6: Intersect with original mask to avoid growing beyond original boundaries
    let mut result = Array2::<bool>::from_elem((height, width), false);
    for y in 0..height {
        for x in 0..width {
            result[[y, x]] = restored[[y, x]] && mask[[y, x]];
        }
    }

    result
}

/// Fill holes in binary mask using flood fill from borders
/// Any false region not connected to the border is considered a hole and filled
fn fill_holes(mask: &Array2<bool>) -> Array2<bool> {
    let (height, width) = mask.dim();
    let mut result = mask.clone();

    // Mark all background pixels connected to the border
    let mut visited = Array2::<bool>::from_elem((height, width), false);
    let mut stack = Vec::new();

    // Start flood fill from all border pixels that are false (background)
    for y in 0..height {
        for x in 0..width {
            if (y == 0 || y == height - 1 || x == 0 || x == width - 1) && !mask[[y, x]] {
                stack.push((y, x));
                visited[[y, x]] = true;
            }
        }
    }

    // Flood fill to mark all background connected to border
    while let Some((y, x)) = stack.pop() {
        // Check 4-connected neighbors
        for (dy, dx) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let ny = y as isize + dy;
            let nx = x as isize + dx;

            if ny >= 0 && ny < height as isize && nx >= 0 && nx < width as isize {
                let ny = ny as usize;
                let nx = nx as usize;

                if !mask[[ny, nx]] && !visited[[ny, nx]] {
                    visited[[ny, nx]] = true;
                    stack.push((ny, nx));
                }
            }
        }
    }

    // Any unvisited false pixel is a hole - fill it
    for y in 0..height {
        for x in 0..width {
            if !mask[[y, x]] && !visited[[y, x]] {
                result[[y, x]] = true; // Fill the hole
            }
        }
    }

    result
}
