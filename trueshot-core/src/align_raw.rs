//! FFT-based phase correlation alignment for raw Bayer frames
//!
//! This module implements professional-grade subpixel image alignment using:
//! - Multi-scale pyramid (coarse-to-fine for speed and robustness)
//! - FFT phase correlation with Hanning window
//! - Subpixel peak interpolation
//! - Bayer-aware processing (green channel only for alignment)

use ndarray::Array2;
use rustfft::{num_complex::Complex, FftPlanner};
use std::f64::consts::PI;

/// Align two raw Bayer frames using multi-scale FFT phase correlation
///
/// # Arguments
/// * `reference` - Reference frame (Array2 of raw Bayer data)
/// * `frame` - Frame to align (Array2 of raw Bayer data)
/// * `num_levels` - Number of pyramid levels (3 is good default)
///
/// # Returns
/// * `(dx, dy)` - Subpixel shift in pixels (frame is shifted by this amount to align with reference)
pub fn align_phasecorr_bayer(
    reference: &Array2<f64>,
    frame: &Array2<f64>,
    num_levels: usize,
) -> (f64, f64) {
    // Extract green channel for alignment (highest resolution, least noise)
    let ref_green = extract_green_channel_from_array(reference);
    let frame_green = extract_green_channel_from_array(frame);

    // Multi-scale pyramid alignment
    multiscale_phase_correlation(&ref_green, &frame_green, num_levels)
}

/// Align two raw Bayer frames WITH scale estimation (for focus breathing)
///
/// # Arguments
/// * `reference` - Reference frame (Array2 of raw Bayer data)
/// * `frame` - Frame to align (Array2 of raw Bayer data)
/// * `num_levels` - Number of pyramid levels (3 is good default)
///
/// # Returns
/// * `(dx, dy, scale)` - Subpixel shift and magnification scale
pub fn align_phasecorr_bayer_with_scale(
    reference: &Array2<f64>,
    frame: &Array2<f64>,
    num_levels: usize,
) -> (f64, f64, f64) {
    // Extract green channel for alignment (highest resolution, least noise)
    let ref_green = extract_green_channel_from_array(reference);
    let frame_green = extract_green_channel_from_array(frame);

    // Compute translation
    let (dx, dy) = multiscale_phase_correlation(&ref_green, &frame_green, num_levels);

    // Compute scale using normalized cross-correlation at different scales
    let scale = estimate_scale(&ref_green, &frame_green);

    (dx, dy, scale)
}

/// Align two grayscale images WITH scale estimation (for focus breathing)
///
/// # Arguments
/// * `reference` - Reference frame (Array2 grayscale data)
/// * `frame` - Frame to align (Array2 grayscale data)
/// * `num_levels` - Number of pyramid levels (3 is good default)
///
/// # Returns
/// * `(dx, dy, scale)` - Subpixel shift and magnification scale
pub fn align_phasecorr_gray_with_scale(
    reference: &Array2<f64>,
    frame: &Array2<f64>,
    num_levels: usize,
) -> (f64, f64, f64) {
    let (dx, dy) = multiscale_phase_correlation(reference, frame, num_levels);
    let scale = estimate_scale(reference, frame);
    (dx, dy, scale)
}

/// Extract green channel from Bayer array (RGGB pattern)
fn extract_green_channel_from_array(bayer: &Array2<f64>) -> Array2<f64> {
    let (height, width) = bayer.dim();

    // Green pixels are at (y%2==0, x%2==1) and (y%2==1, x%2==0) for RGGB
    let mut green = Array2::<f64>::zeros((height / 2, width / 2));

    for y in 0..height / 2 {
        for x in 0..width / 2 {
            // Sample green pixels from RGGB pattern
            // G1 at (0,1), G2 at (1,0)
            let g1 = bayer[[y * 2, x * 2 + 1]];
            let g2 = bayer[[y * 2 + 1, x * 2]];
            green[[y, x]] = (g1 + g2) / 2.0;
        }
    }

    green
}

/// Multi-scale phase correlation (coarse-to-fine)
fn multiscale_phase_correlation(
    ref_img: &Array2<f64>,
    frame_img: &Array2<f64>,
    num_levels: usize,
) -> (f64, f64) {
    let mut total_dx = 0.0;
    let mut total_dy = 0.0;

    // Build pyramids
    let ref_pyramid = build_pyramid(ref_img, num_levels);
    let frame_pyramid = build_pyramid(frame_img, num_levels);

    // Start from coarsest level
    for level in (0..num_levels).rev() {
        let scale = 2.0_f64.powi(level as i32);

        // Compute phase correlation at this level
        let (dx, dy) = phase_correlation_fft(&ref_pyramid[level], &frame_pyramid[level]);

        // Accumulate shift (scaled to original resolution)
        total_dx += dx * scale;
        total_dy += dy * scale;

        tracing::trace!(
            "Level {}: shift=({:.2}, {:.2}), total=({:.2}, {:.2})",
            level,
            dx * scale,
            dy * scale,
            total_dx,
            total_dy
        );
    }

    (total_dx, total_dy)
}

/// Build Gaussian pyramid
fn build_pyramid(img: &Array2<f64>, num_levels: usize) -> Vec<Array2<f64>> {
    let mut pyramid = Vec::with_capacity(num_levels);
    pyramid.push(img.clone());

    for level in 1..num_levels {
        let prev = &pyramid[level - 1];
        let downsampled = downsample_2x(prev);
        pyramid.push(downsampled);
    }

    pyramid
}

/// Downsample image by 2x using averaging
fn downsample_2x(img: &Array2<f64>) -> Array2<f64> {
    let (h, w) = img.dim();
    let new_h = h / 2;
    let new_w = w / 2;

    let mut result = Array2::<f64>::zeros((new_h, new_w));

    for y in 0..new_h {
        for x in 0..new_w {
            let sum = img[[y * 2, x * 2]]
                + img[[y * 2, x * 2 + 1]]
                + img[[y * 2 + 1, x * 2]]
                + img[[y * 2 + 1, x * 2 + 1]];
            result[[y, x]] = sum / 4.0;
        }
    }

    result
}

/// Phase correlation using FFT
fn phase_correlation_fft(ref_img: &Array2<f64>, frame_img: &Array2<f64>) -> (f64, f64) {
    let (height, width) = ref_img.dim();

    // Apply Hanning window to reduce edge effects
    let window = hanning_window_2d(height, width);
    let ref_windowed = ref_img * &window;
    let frame_windowed = frame_img * &window;

    // Compute FFTs
    let ref_fft = fft_2d(&ref_windowed);
    let frame_fft = fft_2d(&frame_windowed);

    // Compute cross-power spectrum
    let mut cross_power = Array2::<Complex<f64>>::zeros((height, width));
    for y in 0..height {
        for x in 0..width {
            let r = ref_fft[[y, x]];
            let f = frame_fft[[y, x]];
            let cross = r * f.conj();
            let mag = cross.norm();
            if mag > 1e-10 {
                cross_power[[y, x]] = cross / mag;
            }
        }
    }

    // Inverse FFT to get correlation surface
    let correlation = ifft_2d(&cross_power);

    // Find peak with subpixel accuracy
    find_subpixel_peak(&correlation, height, width)
}

/// Create 2D Hanning window
fn hanning_window_2d(height: usize, width: usize) -> Array2<f64> {
    let mut window = Array2::<f64>::zeros((height, width));

    for y in 0..height {
        for x in 0..width {
            let wy = 0.5 * (1.0 - (2.0 * PI * y as f64 / height as f64).cos());
            let wx = 0.5 * (1.0 - (2.0 * PI * x as f64 / width as f64).cos());
            window[[y, x]] = wy * wx;
        }
    }

    window
}

/// 2D FFT (row-column decomposition)
fn fft_2d(img: &Array2<f64>) -> Array2<Complex<f64>> {
    let (height, width) = img.dim();
    let mut planner = FftPlanner::new();
    let fft_row = planner.plan_fft_forward(width);
    let fft_col = planner.plan_fft_forward(height);

    // Convert to complex
    let mut data = Array2::<Complex<f64>>::zeros((height, width));
    for y in 0..height {
        for x in 0..width {
            data[[y, x]] = Complex::new(img[[y, x]], 0.0);
        }
    }

    // FFT rows
    for y in 0..height {
        let mut row: Vec<Complex<f64>> = data.row(y).to_vec();
        fft_row.process(&mut row);
        for x in 0..width {
            data[[y, x]] = row[x];
        }
    }

    // FFT columns
    for x in 0..width {
        let mut col: Vec<Complex<f64>> = data.column(x).to_vec();
        fft_col.process(&mut col);
        for y in 0..height {
            data[[y, x]] = col[y];
        }
    }

    data
}

/// 2D inverse FFT
fn ifft_2d(fft: &Array2<Complex<f64>>) -> Array2<f64> {
    let (height, width) = fft.dim();
    let mut planner = FftPlanner::new();
    let ifft_row = planner.plan_fft_inverse(width);
    let ifft_col = planner.plan_fft_inverse(height);

    let mut data = fft.clone();

    // IFFT rows
    for y in 0..height {
        let mut row: Vec<Complex<f64>> = data.row(y).to_vec();
        ifft_row.process(&mut row);
        for x in 0..width {
            data[[y, x]] = row[x];
        }
    }

    // IFFT columns
    for x in 0..width {
        let mut col: Vec<Complex<f64>> = data.column(x).to_vec();
        ifft_col.process(&mut col);
        for y in 0..height {
            data[[y, x]] = col[y];
        }
    }

    // Extract magnitude and normalize
    let mut result = Array2::<f64>::zeros((height, width));
    let norm = (width * height) as f64;
    for y in 0..height {
        for x in 0..width {
            result[[y, x]] = data[[y, x]].norm() / norm;
        }
    }

    result
}

/// Find subpixel peak in correlation surface using parabolic interpolation
fn find_subpixel_peak(correlation: &Array2<f64>, height: usize, width: usize) -> (f64, f64) {
    // Find integer peak
    let mut max_val = 0.0;
    let mut max_y = 0;
    let mut max_x = 0;

    for y in 0..height {
        for x in 0..width {
            let val = correlation[[y, x]];
            if val > max_val {
                max_val = val;
                max_y = y;
                max_x = x;
            }
        }
    }

    // Subpixel refinement using parabolic interpolation
    let (dx, dy) = if max_x > 0 && max_x < width - 1 && max_y > 0 && max_y < height - 1 {
        let c = correlation[[max_y, max_x]];

        // X direction
        let left = correlation[[max_y, max_x - 1]];
        let right = correlation[[max_y, max_x + 1]];
        let dx_sub = if (2.0 * c - left - right).abs() > 1e-10 {
            0.5 * (right - left) / (2.0 * c - left - right)
        } else {
            0.0
        };

        // Y direction
        let top = correlation[[max_y - 1, max_x]];
        let bottom = correlation[[max_y + 1, max_x]];
        let dy_sub = if (2.0 * c - top - bottom).abs() > 1e-10 {
            0.5 * (bottom - top) / (2.0 * c - top - bottom)
        } else {
            0.0
        };

        (dx_sub, dy_sub)
    } else {
        (0.0, 0.0)
    };

    // Convert to shift (handle wraparound)
    let mut shift_x = max_x as f64 + dx;
    let mut shift_y = max_y as f64 + dy;

    if shift_x > width as f64 / 2.0 {
        shift_x -= width as f64;
    }
    if shift_y > height as f64 / 2.0 {
        shift_y -= height as f64;
    }

    (shift_x, shift_y)
}

/// Estimate magnification scale between two images (for focus breathing)
///
/// Uses normalized cross-correlation at different scales with FINE granularity
/// Focus breathing typically causes 0.1-0.5% magnification change
fn estimate_scale(reference: &Array2<f64>, frame: &Array2<f64>) -> f64 {
    let (height, width) = reference.dim();

    // Use center region (focus breathing affects center most)
    let crop_h = height / 2;
    let crop_w = width / 2;
    let start_y = height / 4;
    let start_x = width / 4;

    // COARSE search first: 0.98 to 1.02 in 0.01 steps
    let coarse_scales: Vec<f64> = (98..=102).map(|i| i as f64 / 100.0).collect();
    let mut best_scale = 1.0;
    let mut best_score = -1.0;

    // Coarse search
    for &scale in &coarse_scales {
        let score = compute_scale_score(
            reference, frame, scale, start_x, start_y, crop_w, crop_h, width, height,
        );
        if score > best_score {
            best_score = score;
            best_scale = scale;
        }
    }

    // FINE search around best coarse scale: ±0.01 in 0.001 steps (0.1% granularity)
    let fine_start = (best_scale - 0.01).max(0.95);
    let fine_end = (best_scale + 0.01).min(1.05);
    let fine_step = 0.001;

    let mut fine_scale = fine_start;
    while fine_scale <= fine_end {
        let score = compute_scale_score(
            reference, frame, fine_scale, start_x, start_y, crop_w, crop_h, width, height,
        );
        if score > best_score {
            best_score = score;
            best_scale = fine_scale;
        }
        fine_scale += fine_step;
    }

    best_scale
}

/// Compute normalized cross-correlation score for a given scale
fn compute_scale_score(
    reference: &Array2<f64>,
    frame: &Array2<f64>,
    scale: f64,
    start_x: usize,
    start_y: usize,
    crop_w: usize,
    crop_h: usize,
    width: usize,
    height: usize,
) -> f64 {
    let center_x = width.saturating_sub(1) as f64 * 0.5;
    let center_y = height.saturating_sub(1) as f64 * 0.5;
    let mut count = 0usize;
    let mut reference_mean = 0.0f64;
    let mut frame_mean = 0.0f64;
    let mut covariance = 0.0f64;
    let mut reference_variance = 0.0f64;
    let mut frame_variance = 0.0f64;

    for y in start_y..(start_y + crop_h) {
        for x in start_x..(start_x + crop_w) {
            let frame_x = (x as f64 - center_x) * scale + center_x;
            let frame_y = (y as f64 - center_y) * scale + center_y;
            if let Some(frame_value) = bilinear_sample(frame, frame_x, frame_y) {
                let reference_value = reference[[y, x]];
                count += 1;
                let count_f64 = count as f64;
                let reference_delta = reference_value - reference_mean;
                reference_mean += reference_delta / count_f64;
                let frame_delta = frame_value - frame_mean;
                frame_mean += frame_delta / count_f64;
                covariance += reference_delta * (frame_value - frame_mean);
                reference_variance += reference_delta * (reference_value - reference_mean);
                frame_variance += frame_delta * (frame_value - frame_mean);
            }
        }
    }

    let denominator = (reference_variance * frame_variance).sqrt();
    if count < 16 || denominator <= 1e-12 {
        return -1.0;
    }
    covariance / denominator
}

fn bilinear_sample(image: &Array2<f64>, x: f64, y: f64) -> Option<f64> {
    let (height, width) = image.dim();
    if width == 0
        || height == 0
        || x < 0.0
        || y < 0.0
        || x > width.saturating_sub(1) as f64
        || y > height.saturating_sub(1) as f64
    {
        return None;
    }
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = x - x0 as f64;
    let ty = y - y0 as f64;
    let top = image[[y0, x0]] + (image[[y0, x1]] - image[[y0, x0]]) * tx;
    let bottom = image[[y1, x0]] + (image[[y1, x1]] - image[[y1, x0]]) * tx;
    Some(top + (bottom - top) * ty)
}

#[cfg(test)]
mod tests {
    use super::{compute_scale_score, estimate_scale};
    use ndarray::Array2;

    fn textured_image(size: usize) -> Array2<f64> {
        Array2::from_shape_fn((size, size), |(y, x)| {
            let dx = x as f64 - size as f64 * 0.5;
            let dy = y as f64 - size as f64 * 0.5;
            (dx * 0.17).sin() + (dy * 0.11).cos() + (dx * dx + dy * dy).sqrt() * 0.003
        })
    }

    #[test]
    fn scale_score_handles_coordinates_left_of_center_without_overflow() {
        let image = textured_image(64);
        let exact = compute_scale_score(&image, &image, 1.0, 16, 16, 32, 32, 64, 64);
        let wrong = compute_scale_score(&image, &image, 0.97, 16, 16, 32, 32, 64, 64);
        assert!(exact > 0.999_999);
        assert!(exact > wrong);
        assert!((estimate_scale(&image, &image) - 1.0).abs() < 1e-9);
    }
}
