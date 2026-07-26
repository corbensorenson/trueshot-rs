//! Joint demosaicing for multiframe fusion
//!
//! Implements direct Bayer-to-RGB accumulation to avoid artifacts from
//! creating merged Bayer mosaics with discontinuities between focus planes.
//!
//! Key insight: When merging frames from different focus planes, we must NOT
//! create a merged Bayer mosaic (which has discontinuities that confuse demosaicing).
//! Instead, we accumulate Bayer pixels directly to RGB channels during fusion.

use anyhow::Result;
use ndarray::{Array2, Array3};

/// Get Bayer color channel for a pixel position (RGGB pattern)
///
/// Returns: 0=R, 1=G, 2=B
#[inline]
pub fn get_bayer_color(y: usize, x: usize) -> usize {
    match (y % 2, x % 2) {
        (0, 0) => 0,          // R at (even, even)
        (0, 1) | (1, 0) => 1, // G at (even, odd) and (odd, even)
        (1, 1) => 2,          // B at (odd, odd)
        _ => unreachable!(),
    }
}

/// Strategy for selecting which pixels to process
pub enum PixelSelection<'a> {
    All,
    List(&'a [(usize, usize)]),
    Grades(&'a Array2<u8>, &'a [u8]), // Map and allowed values
}

/// Joint demosaicing: Properly interpolate RGB from multiple Bayer frames
///
/// This function performs HDR fusion and focus stacking simultaneously while demosaicing.
///
/// **NEW ARCHITECTURE**: For each output pixel, we interpolate a full RGB triplet from EACH
/// input frame, then accumulate the weighted RGB triplets. This avoids the fundamental issues
/// of the old approach:
/// - No merged Bayer mosaic (which creates discontinuities)
/// - No green bias from Bayer sampling density (each frame is properly demosaiced)
/// - No edge artifacts from mixing pixels from different frames
///
/// **SOFT BLENDING**: Instead of hard focus plane selection, we blend multiple adjacent
/// focus planes with Gaussian weights. This eliminates horizontal banding artifacts at
/// focus plane boundaries.
///
/// Algorithm:
/// 1. For each output pixel (y, x):
///    - Determine best focus plane and blend with adjacent planes
///    - For each plane in blend range:
///      a. For each exposure in that plane:
///         - Get full RGB triplet (interpolate missing channels)
///         - Apply HDR weight and focus plane blend weight
///      b. Accumulate weighted RGB triplets
///    - Normalize by total weight
/// 2. Apply white balance to final RGB image
///
/// # Arguments
/// * `frames` - Input Bayer frames (each is H×W×1)
/// * `image_weights` - Mertens HDR weights for each frame
/// * `best_plane_map` - Best focus plane index for each pixel (H×W)
/// * `num_exposures` - Number of exposures per focus plane
/// * `wb_multipliers` - White balance multipliers [R, G1, G2, B]
/// * `selection` - Strategy for selecting pixels to process (All, List, or Grades)
/// * `blend_radius` - Number of adjacent focus planes to blend (0 = hard selection, 1 = blend ±1 planes)
///
/// # Returns
/// RGB image (H×W×3) with joint demosaicing applied
pub fn joint_demosaic_with_focus_selection(
    frames: &[Array3<f64>],
    image_weights: &[f64],
    best_plane_map: &Array2<usize>,
    num_exposures: usize,
    wb_multipliers: &[f32; 4],
    selection: PixelSelection,
    blend_radius: usize,
) -> Result<Array3<f64>> {
    let (height, width, _) = frames[0].dim();
    let mut rgb = Array3::<f64>::zeros((height, width, 3));

    // Calculate number of focus planes
    let num_planes = if num_exposures > 0 {
        (frames.len() + num_exposures - 1) / num_exposures
    } else {
        1
    };

    // Soft blending parameters
    // blend_radius=0: hard selection (only best plane)
    // blend_radius=1: blend with ±1 adjacent focus planes (only at boundaries)
    let blend_sigma = 0.8; // Gaussian sigma for plane distance weighting

    // Process each pixel
    let mut process_pixel = |y: usize, x: usize| {
        // Get best focus plane for this pixel
        let best_plane = best_plane_map[[y, x]];

        // Check if this pixel is at a focus plane boundary
        // Only blend if neighboring pixels have different best planes
        let is_boundary = if blend_radius > 0 {
            let mut has_different_neighbor = false;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dy == 0 && dx == 0 {
                        continue;
                    }
                    let ny = (y as i32 + dy).max(0).min(height as i32 - 1) as usize;
                    let nx = (x as i32 + dx).max(0).min(width as i32 - 1) as usize;
                    if best_plane_map[[ny, nx]] != best_plane {
                        has_different_neighbor = true;
                        break;
                    }
                }
                if has_different_neighbor {
                    break;
                }
            }
            has_different_neighbor
        } else {
            false
        };

        // Accumulate weighted RGB triplets from multiple focus planes
        let mut rgb_sum = [0.0, 0.0, 0.0];
        let mut total_weight = 0.0;

        // Blend with adjacent focus planes ONLY at boundaries
        let (plane_start, plane_end) = if is_boundary {
            (
                best_plane.saturating_sub(blend_radius),
                (best_plane + blend_radius + 1).min(num_planes),
            )
        } else {
            // Not at boundary: use only best plane (hard selection)
            (best_plane, best_plane + 1)
        };

        for plane_idx in plane_start..plane_end {
            // Compute Gaussian weight based on distance from best plane
            let plane_distance = (plane_idx as i32 - best_plane as i32).abs() as f64;
            let plane_weight =
                (-plane_distance * plane_distance / (2.0 * blend_sigma * blend_sigma)).exp();

            // Process all exposures in this focus plane
            let start_frame = plane_idx * num_exposures;
            let end_frame = (start_frame + num_exposures).min(frames.len());

            // Use per-image weights (passed from caller)
            // These are global Mertens weights computed once per frame
            let mut exposure_weights = Vec::with_capacity(end_frame - start_frame);
            for frame_idx in start_frame..end_frame {
                exposure_weights.push(image_weights[frame_idx]);
            }

            // Normalize weights within this plane
            let weight_sum: f64 = exposure_weights.iter().sum();
            if weight_sum > 0.0 {
                for w in &mut exposure_weights {
                    *w /= weight_sum;
                }
            } else {
                // If all weights are zero, use uniform
                let uniform = 1.0 / (end_frame - start_frame) as f64;
                exposure_weights.fill(uniform);
            }

            // Accumulate weighted RGB from each exposure
            for (i, frame_idx) in (start_frame..end_frame).enumerate() {
                let hdr_weight = exposure_weights[i];
                let combined_weight = hdr_weight * plane_weight;

                // Get full RGB triplet for this pixel from this frame
                // This interpolates the missing color channels from neighbors
                let rgb_triplet = get_rgb_from_bayer_frame(&frames[frame_idx], y, x, height, width);

                // Accumulate weighted RGB
                rgb_sum[0] += combined_weight * rgb_triplet[0];
                rgb_sum[1] += combined_weight * rgb_triplet[1];
                rgb_sum[2] += combined_weight * rgb_triplet[2];
                total_weight += combined_weight;
            }
        }

        // Normalize by total weight
        if total_weight > 0.0 {
            rgb[[y, x, 0]] = rgb_sum[0] / total_weight;
            rgb[[y, x, 1]] = rgb_sum[1] / total_weight;
            rgb[[y, x, 2]] = rgb_sum[2] / total_weight;
        }
    };

    match selection {
        PixelSelection::All => {
            for y in 0..height {
                for x in 0..width {
                    process_pixel(y, x);
                }
            }
        }
        PixelSelection::List(pixels) => {
            for &(y, x) in pixels {
                process_pixel(y, x);
            }
        }
        PixelSelection::Grades(grades, allowed) => {
            for y in 0..height {
                for x in 0..width {
                    let g = grades[[y, x]];
                    if allowed.contains(&g) {
                        process_pixel(y, x);
                    }
                }
            }
        }
    }

    // Apply white balance to final RGB image
    let wb_r = wb_multipliers[0] as f64;
    let wb_g = wb_multipliers[1] as f64;
    let wb_b = wb_multipliers[2] as f64;

    for y in 0..height {
        for x in 0..width {
            rgb[[y, x, 0]] *= wb_r;
            rgb[[y, x, 1]] *= wb_g;
            rgb[[y, x, 2]] *= wb_b;
        }
    }

    Ok(rgb)
}

/// Demosaic an entire Bayer frame to RGB
///
/// This is used for computing sharpness on demosaiced RGB instead of raw Bayer,
/// which eliminates horizontal banding artifacts from Bayer pattern.
pub fn demosaic_bayer_frame(
    frame: &Array3<f64>,       // H×W×1 Bayer frame
    wb_multipliers: &[f32; 4], // White balance multipliers [R, G1, G2, B]
) -> Result<Array3<f64>> {
    // H×W×3 RGB frame
    let (height, width, _) = frame.dim();
    let mut rgb = Array3::<f64>::zeros((height, width, 3));

    // Demosaic each pixel
    for y in 0..height {
        for x in 0..width {
            let rgb_triplet = get_rgb_from_bayer_frame(frame, y, x, height, width);
            rgb[[y, x, 0]] = rgb_triplet[0];
            rgb[[y, x, 1]] = rgb_triplet[1];
            rgb[[y, x, 2]] = rgb_triplet[2];
        }
    }

    // Apply white balance
    let wb_r = wb_multipliers[0] as f64;
    let wb_g = wb_multipliers[1] as f64;
    let wb_b = wb_multipliers[2] as f64;

    for y in 0..height {
        for x in 0..width {
            rgb[[y, x, 0]] *= wb_r;
            rgb[[y, x, 1]] *= wb_g;
            rgb[[y, x, 2]] *= wb_b;
        }
    }

    Ok(rgb)
}

/// Get full RGB triplet for a pixel from a single Bayer frame
///
/// For the pixel at (y, x):
/// - One channel is "direct" (the Bayer value at this location)
/// - Two channels are "interpolated" (from neighboring pixels)
///
/// SIMPLIFIED: Uses simple bilinear interpolation (no edge detection)
/// to test if edge-directed demosaicing is causing speckle artifacts.
fn get_rgb_from_bayer_frame(
    frame: &Array3<f64>,
    y: usize,
    x: usize,
    height: usize,
    width: usize,
) -> [f64; 3] {
    let color = get_bayer_color(y, x);
    let direct_value = frame[[y, x, 0]];

    let mut rgb = [0.0, 0.0, 0.0];
    rgb[color] = direct_value; // Direct channel

    // Interpolate the two missing channels using edge-directed interpolation
    match color {
        0 => {
            // R pixel: have R, need to interpolate G and B
            rgb[1] = interpolate_green_at_red_edge_directed(frame, y, x, height, width);
            rgb[2] = interpolate_blue_at_red_edge_directed(frame, y, x, height, width);
        }
        1 => {
            // G pixel: have G, need to interpolate R and B
            rgb[0] = interpolate_red_at_green_edge_directed(frame, y, x, height, width);
            rgb[2] = interpolate_blue_at_green_edge_directed(frame, y, x, height, width);
        }
        2 => {
            // B pixel: have B, need to interpolate R and G
            rgb[0] = interpolate_red_at_blue_edge_directed(frame, y, x, height, width);
            rgb[1] = interpolate_green_at_blue_edge_directed(frame, y, x, height, width);
        }
        _ => unreachable!(),
    }

    rgb
}

// ============================================================================
// Edge-Directed Demosaicing with Gradient Analysis
// Inspired by AMAZE (Aliasing Minimization and Zipper Elimination)
// Uses gradient analysis to choose interpolation direction adaptively
// ============================================================================

/// Helper to safely get pixel value with bounds checking
#[inline]
fn get_pixel(frame: &Array3<f64>, y: isize, x: isize, height: usize, width: usize) -> f64 {
    if y < 0 || x < 0 || y >= height as isize || x >= width as isize {
        return 0.0;
    }
    frame[[y as usize, x as usize, 0]]
}

/// Compute gradient in a direction (for edge detection)
#[inline]
fn compute_gradient(v1: f64, v2: f64, v3: f64) -> f64 {
    // Second derivative (curvature) as gradient measure
    ((v1 - v2).abs() + (v3 - v2).abs()).max(1e-10)
}

/// Interpolate green channel at red pixel position (edge-directed)
/// At R pixel (even, even): need to interpolate G
/// Uses gradient analysis to choose between horizontal and vertical interpolation
fn interpolate_green_at_red_edge_directed(
    frame: &Array3<f64>,
    y: usize,
    x: usize,
    height: usize,
    width: usize,
) -> f64 {
    let yi = y as isize;
    let xi = x as isize;

    let r_center = get_pixel(frame, yi, xi, height, width);

    // Get neighboring green pixels
    let g_n = get_pixel(frame, yi - 1, xi, height, width); // North
    let g_s = get_pixel(frame, yi + 1, xi, height, width); // South
    let g_w = get_pixel(frame, yi, xi - 1, height, width); // West
    let g_e = get_pixel(frame, yi, xi + 1, height, width); // East

    // Get neighboring red pixels for gradient computation
    let r_n = get_pixel(frame, yi - 2, xi, height, width);
    let r_s = get_pixel(frame, yi + 2, xi, height, width);
    let r_w = get_pixel(frame, yi, xi - 2, height, width);
    let r_e = get_pixel(frame, yi, xi + 2, height, width);

    // Compute gradients in vertical and horizontal directions
    let grad_v = compute_gradient(r_n, r_center, r_s) + (g_n - g_s).abs();
    let grad_h = compute_gradient(r_w, r_center, r_e) + (g_w - g_e).abs();

    // Edge-directed interpolation: use direction with smaller gradient
    let threshold = 1.2; // Threshold for choosing direction

    if grad_v < grad_h / threshold {
        // Vertical edge: interpolate vertically
        ((g_n + g_s) / 2.0 + (2.0 * r_center - r_n - r_s) / 4.0).max(0.0)
    } else if grad_h < grad_v / threshold {
        // Horizontal edge: interpolate horizontally
        ((g_w + g_e) / 2.0 + (2.0 * r_center - r_w - r_e) / 4.0).max(0.0)
    } else {
        // No clear edge: use weighted average of both directions
        let weight_v = 1.0 / (grad_v + 1e-10);
        let weight_h = 1.0 / (grad_h + 1e-10);
        let val_v = (g_n + g_s) / 2.0 + (2.0 * r_center - r_n - r_s) / 4.0;
        let val_h = (g_w + g_e) / 2.0 + (2.0 * r_center - r_w - r_e) / 4.0;
        ((weight_v * val_v + weight_h * val_h) / (weight_v + weight_h)).max(0.0)
    }
}

/// Interpolate blue channel at red pixel position (edge-directed diagonal)
/// At R pixel (even, even): need to interpolate B (diagonal neighbors)
fn interpolate_blue_at_red_edge_directed(
    frame: &Array3<f64>,
    y: usize,
    x: usize,
    height: usize,
    width: usize,
) -> f64 {
    let yi = y as isize;
    let xi = x as isize;

    let r_center = get_pixel(frame, yi, xi, height, width);

    // Get diagonal blue neighbors
    let b_nw = get_pixel(frame, yi - 1, xi - 1, height, width);
    let b_ne = get_pixel(frame, yi - 1, xi + 1, height, width);
    let b_sw = get_pixel(frame, yi + 1, xi - 1, height, width);
    let b_se = get_pixel(frame, yi + 1, xi + 1, height, width);

    // Get diagonal red neighbors for gradient computation
    let r_nw = get_pixel(frame, yi - 2, xi - 2, height, width);
    let r_ne = get_pixel(frame, yi - 2, xi + 2, height, width);
    let r_sw = get_pixel(frame, yi + 2, xi - 2, height, width);
    let r_se = get_pixel(frame, yi + 2, xi + 2, height, width);

    // Compute gradients along diagonals
    let grad_nw_se = compute_gradient(r_nw, r_center, r_se) + (b_nw - b_se).abs();
    let grad_ne_sw = compute_gradient(r_ne, r_center, r_sw) + (b_ne - b_sw).abs();

    // Edge-directed interpolation along diagonals
    let threshold = 1.2;

    if grad_nw_se < grad_ne_sw / threshold {
        // NW-SE edge: interpolate along this diagonal
        ((b_nw + b_se) / 2.0 + (2.0 * r_center - r_nw - r_se) / 4.0).max(0.0)
    } else if grad_ne_sw < grad_nw_se / threshold {
        // NE-SW edge: interpolate along this diagonal
        ((b_ne + b_sw) / 2.0 + (2.0 * r_center - r_ne - r_sw) / 4.0).max(0.0)
    } else {
        // No clear edge: use weighted average
        let weight_nw_se = 1.0 / (grad_nw_se + 1e-10);
        let weight_ne_sw = 1.0 / (grad_ne_sw + 1e-10);
        let val_nw_se = (b_nw + b_se) / 2.0 + (2.0 * r_center - r_nw - r_se) / 4.0;
        let val_ne_sw = (b_ne + b_sw) / 2.0 + (2.0 * r_center - r_ne - r_sw) / 4.0;
        ((weight_nw_se * val_nw_se + weight_ne_sw * val_ne_sw) / (weight_nw_se + weight_ne_sw))
            .max(0.0)
    }
}

/// Interpolate red channel at green pixel position (edge-directed)
/// At G pixel: R is either horizontal or vertical depending on Bayer position
fn interpolate_red_at_green_edge_directed(
    frame: &Array3<f64>,
    y: usize,
    x: usize,
    height: usize,
    width: usize,
) -> f64 {
    let yi = y as isize;
    let xi = x as isize;

    let g_center = get_pixel(frame, yi, xi, height, width);

    if y % 2 == 0 {
        // G at (even, odd): R neighbors are horizontal (West/East)
        let r_w = get_pixel(frame, yi, xi - 1, height, width);
        let r_e = get_pixel(frame, yi, xi + 1, height, width);
        ((r_w + r_e) / 2.0
            + (g_center
                - (get_pixel(frame, yi, xi - 2, height, width)
                    + get_pixel(frame, yi, xi + 2, height, width))
                    / 2.0)
                / 2.0)
            .max(0.0)
    } else {
        // G at (odd, even): R neighbors are vertical (North/South)
        let r_n = get_pixel(frame, yi - 1, xi, height, width);
        let r_s = get_pixel(frame, yi + 1, xi, height, width);
        ((r_n + r_s) / 2.0
            + (g_center
                - (get_pixel(frame, yi - 2, xi, height, width)
                    + get_pixel(frame, yi + 2, xi, height, width))
                    / 2.0)
                / 2.0)
            .max(0.0)
    }
}

/// Interpolate blue channel at green pixel position (edge-directed)
/// At G pixel: B is either horizontal or vertical depending on Bayer position
fn interpolate_blue_at_green_edge_directed(
    frame: &Array3<f64>,
    y: usize,
    x: usize,
    height: usize,
    width: usize,
) -> f64 {
    let yi = y as isize;
    let xi = x as isize;

    let g_center = get_pixel(frame, yi, xi, height, width);

    if y % 2 == 0 {
        // G at (even, odd): B neighbors are vertical (North/South)
        let b_n = get_pixel(frame, yi - 1, xi, height, width);
        let b_s = get_pixel(frame, yi + 1, xi, height, width);
        ((b_n + b_s) / 2.0
            + (g_center
                - (get_pixel(frame, yi - 2, xi, height, width)
                    + get_pixel(frame, yi + 2, xi, height, width))
                    / 2.0)
                / 2.0)
            .max(0.0)
    } else {
        // G at (odd, even): B neighbors are horizontal (West/East)
        let b_w = get_pixel(frame, yi, xi - 1, height, width);
        let b_e = get_pixel(frame, yi, xi + 1, height, width);
        ((b_w + b_e) / 2.0
            + (g_center
                - (get_pixel(frame, yi, xi - 2, height, width)
                    + get_pixel(frame, yi, xi + 2, height, width))
                    / 2.0)
                / 2.0)
            .max(0.0)
    }
}

/// Interpolate red channel at blue pixel position (edge-directed diagonal)
/// At B pixel (odd, odd): need to interpolate R (diagonal neighbors)
fn interpolate_red_at_blue_edge_directed(
    frame: &Array3<f64>,
    y: usize,
    x: usize,
    height: usize,
    width: usize,
) -> f64 {
    let yi = y as isize;
    let xi = x as isize;

    let b_center = get_pixel(frame, yi, xi, height, width);

    // Get diagonal red neighbors
    let r_nw = get_pixel(frame, yi - 1, xi - 1, height, width);
    let r_ne = get_pixel(frame, yi - 1, xi + 1, height, width);
    let r_sw = get_pixel(frame, yi + 1, xi - 1, height, width);
    let r_se = get_pixel(frame, yi + 1, xi + 1, height, width);

    // Get diagonal blue neighbors for gradient computation
    let b_nw = get_pixel(frame, yi - 2, xi - 2, height, width);
    let b_ne = get_pixel(frame, yi - 2, xi + 2, height, width);
    let b_sw = get_pixel(frame, yi + 2, xi - 2, height, width);
    let b_se = get_pixel(frame, yi + 2, xi + 2, height, width);

    // Compute gradients along diagonals
    let grad_nw_se = compute_gradient(b_nw, b_center, b_se) + (r_nw - r_se).abs();
    let grad_ne_sw = compute_gradient(b_ne, b_center, b_sw) + (r_ne - r_sw).abs();

    // Edge-directed interpolation along diagonals
    let threshold = 1.2;

    if grad_nw_se < grad_ne_sw / threshold {
        // NW-SE edge: interpolate along this diagonal
        ((r_nw + r_se) / 2.0 + (2.0 * b_center - b_nw - b_se) / 4.0).max(0.0)
    } else if grad_ne_sw < grad_nw_se / threshold {
        // NE-SW edge: interpolate along this diagonal
        ((r_ne + r_sw) / 2.0 + (2.0 * b_center - b_ne - b_sw) / 4.0).max(0.0)
    } else {
        // No clear edge: use weighted average
        let weight_nw_se = 1.0 / (grad_nw_se + 1e-10);
        let weight_ne_sw = 1.0 / (grad_ne_sw + 1e-10);
        let val_nw_se = (r_nw + r_se) / 2.0 + (2.0 * b_center - b_nw - b_se) / 4.0;
        let val_ne_sw = (r_ne + r_sw) / 2.0 + (2.0 * b_center - b_ne - b_sw) / 4.0;
        ((weight_nw_se * val_nw_se + weight_ne_sw * val_ne_sw) / (weight_nw_se + weight_ne_sw))
            .max(0.0)
    }
}

/// Interpolate green channel at blue pixel position (edge-directed)
/// At B pixel (odd, odd): need to interpolate G (same structure as G at R)
fn interpolate_green_at_blue_edge_directed(
    frame: &Array3<f64>,
    y: usize,
    x: usize,
    height: usize,
    width: usize,
) -> f64 {
    let yi = y as isize;
    let xi = x as isize;

    let b_center = get_pixel(frame, yi, xi, height, width);

    // Get neighboring green pixels
    let g_n = get_pixel(frame, yi - 1, xi, height, width);
    let g_s = get_pixel(frame, yi + 1, xi, height, width);
    let g_w = get_pixel(frame, yi, xi - 1, height, width);
    let g_e = get_pixel(frame, yi, xi + 1, height, width);

    // Get neighboring blue pixels for gradient computation
    let b_n = get_pixel(frame, yi - 2, xi, height, width);
    let b_s = get_pixel(frame, yi + 2, xi, height, width);
    let b_w = get_pixel(frame, yi, xi - 2, height, width);
    let b_e = get_pixel(frame, yi, xi + 2, height, width);

    // Compute gradients in vertical and horizontal directions
    let grad_v = compute_gradient(b_n, b_center, b_s) + (g_n - g_s).abs();
    let grad_h = compute_gradient(b_w, b_center, b_e) + (g_w - g_e).abs();

    // Edge-directed interpolation
    let threshold = 1.2;

    if grad_v < grad_h / threshold {
        // Vertical edge: interpolate vertically
        ((g_n + g_s) / 2.0 + (2.0 * b_center - b_n - b_s) / 4.0).max(0.0)
    } else if grad_h < grad_v / threshold {
        // Horizontal edge: interpolate horizontally
        ((g_w + g_e) / 2.0 + (2.0 * b_center - b_w - b_e) / 4.0).max(0.0)
    } else {
        // No clear edge: use weighted average
        let weight_v = 1.0 / (grad_v + 1e-10);
        let weight_h = 1.0 / (grad_h + 1e-10);
        let val_v = (g_n + g_s) / 2.0 + (2.0 * b_center - b_n - b_s) / 4.0;
        let val_h = (g_w + g_e) / 2.0 + (2.0 * b_center - b_w - b_e) / 4.0;
        ((weight_v * val_v + weight_h * val_h) / (weight_v + weight_h)).max(0.0)
    }
}

// ============================================================================
// Simple Bilinear Demosaicing (No Edge Detection)
// Testing if edge-directed interpolation causes speckle artifacts
// ============================================================================

/// Interpolate green at red pixel (simple bilinear)
fn interpolate_green_at_red_bilinear(
    frame: &Array3<f64>,
    y: usize,
    x: usize,
    height: usize,
    width: usize,
) -> f64 {
    let yi = y as isize;
    let xi = x as isize;

    // Average of 4 neighboring green pixels (N, S, W, E)
    let g_n = get_pixel(frame, yi - 1, xi, height, width);
    let g_s = get_pixel(frame, yi + 1, xi, height, width);
    let g_w = get_pixel(frame, yi, xi - 1, height, width);
    let g_e = get_pixel(frame, yi, xi + 1, height, width);

    (g_n + g_s + g_w + g_e) / 4.0
}

/// Interpolate blue at red pixel (simple bilinear)
fn interpolate_blue_at_red_bilinear(
    frame: &Array3<f64>,
    y: usize,
    x: usize,
    height: usize,
    width: usize,
) -> f64 {
    let yi = y as isize;
    let xi = x as isize;

    // Average of 4 diagonal blue pixels (NW, NE, SW, SE)
    let b_nw = get_pixel(frame, yi - 1, xi - 1, height, width);
    let b_ne = get_pixel(frame, yi - 1, xi + 1, height, width);
    let b_sw = get_pixel(frame, yi + 1, xi - 1, height, width);
    let b_se = get_pixel(frame, yi + 1, xi + 1, height, width);

    (b_nw + b_ne + b_sw + b_se) / 4.0
}

/// Interpolate red at green pixel (simple bilinear)
fn interpolate_red_at_green_bilinear(
    frame: &Array3<f64>,
    y: usize,
    x: usize,
    height: usize,
    width: usize,
) -> f64 {
    let yi = y as isize;
    let xi = x as isize;

    // At Gr (even row, odd col): R is at W and E
    // At Gb (odd row, even col): R is at N and S
    if y % 2 == 0 {
        // Gr: average horizontal neighbors
        let r_w = get_pixel(frame, yi, xi - 1, height, width);
        let r_e = get_pixel(frame, yi, xi + 1, height, width);
        (r_w + r_e) / 2.0
    } else {
        // Gb: average vertical neighbors
        let r_n = get_pixel(frame, yi - 1, xi, height, width);
        let r_s = get_pixel(frame, yi + 1, xi, height, width);
        (r_n + r_s) / 2.0
    }
}

/// Interpolate blue at green pixel (simple bilinear)
fn interpolate_blue_at_green_bilinear(
    frame: &Array3<f64>,
    y: usize,
    x: usize,
    height: usize,
    width: usize,
) -> f64 {
    let yi = y as isize;
    let xi = x as isize;

    // At Gr (even row, odd col): B is at N and S
    // At Gb (odd row, even col): B is at W and E
    if y % 2 == 0 {
        // Gr: average vertical neighbors
        let b_n = get_pixel(frame, yi - 1, xi, height, width);
        let b_s = get_pixel(frame, yi + 1, xi, height, width);
        (b_n + b_s) / 2.0
    } else {
        // Gb: average horizontal neighbors
        let b_w = get_pixel(frame, yi, xi - 1, height, width);
        let b_e = get_pixel(frame, yi, xi + 1, height, width);
        (b_w + b_e) / 2.0
    }
}

/// Interpolate red at blue pixel (simple bilinear)
fn interpolate_red_at_blue_bilinear(
    frame: &Array3<f64>,
    y: usize,
    x: usize,
    height: usize,
    width: usize,
) -> f64 {
    let yi = y as isize;
    let xi = x as isize;

    // Average of 4 diagonal red pixels (NW, NE, SW, SE)
    let r_nw = get_pixel(frame, yi - 1, xi - 1, height, width);
    let r_ne = get_pixel(frame, yi - 1, xi + 1, height, width);
    let r_sw = get_pixel(frame, yi + 1, xi - 1, height, width);
    let r_se = get_pixel(frame, yi + 1, xi + 1, height, width);

    (r_nw + r_ne + r_sw + r_se) / 4.0
}

/// Compute simple exposure weight for HDR fusion on Bayer data
///
/// SIMPLE AVERAGE - just use uniform weights for all valid exposures
///
/// # Arguments
/// * `frame` - Bayer frame (H×W×1)
/// * `y`, `x` - Pixel coordinates
/// * `height`, `width` - Frame dimensions (unused, for API compatibility)
///
/// # Returns
/// Always returns 1.0 (uniform weighting)
fn compute_pixel_mertens_weight(
    _frame: &Array3<f64>,
    _y: usize,
    _x: usize,
    _height: usize,
    _width: usize,
) -> f64 {
    // Just use uniform weights - simple average
    1.0
}

/// Interpolate green at blue pixel (simple bilinear)
fn interpolate_green_at_blue_bilinear(
    frame: &Array3<f64>,
    y: usize,
    x: usize,
    height: usize,
    width: usize,
) -> f64 {
    let yi = y as isize;
    let xi = x as isize;

    // Average of 4 neighboring green pixels (N, S, W, E)
    let g_n = get_pixel(frame, yi - 1, xi, height, width);
    let g_s = get_pixel(frame, yi + 1, xi, height, width);
    let g_w = get_pixel(frame, yi, xi - 1, height, width);
    let g_e = get_pixel(frame, yi, xi + 1, height, width);

    (g_n + g_s + g_w + g_e) / 4.0
}
