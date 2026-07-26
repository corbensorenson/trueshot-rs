//! Color Calibration Module (Feature 4)
//!
//! Detects ColorChecker Passport charts and generates CCMs.
//! Currently uses heuristic detection based on grid variance analysis.

use anyhow::Result;
use nalgebra as na;
use ndarray::Array3;

pub struct ColorChartDetector;

#[derive(Debug, Clone)]
pub struct CalibrationResult {
    pub matrix: [[f32; 3]; 3],
    pub error: f32,
}

impl ColorChartDetector {
    /// Detect color chart in image and compute CCM
    pub fn detect_and_calibrate(rgb: &Array3<f64>) -> Result<Option<CalibrationResult>> {
        let (height, width, channels) = rgb.dim();
        if channels < 3 || height < 64 || width < 64 {
            return Ok(None);
        }

        tracing::info!("Searching for color chart...");

        let (downsampled, ds_width, ds_height, scale_x, scale_y) = downsample(rgb, 512);
        let lum = compute_luminance(&downsampled, ds_width, ds_height);
        let grad = compute_gradient(&lum, ds_width, ds_height);
        let integral = compute_integral(&grad, ds_width, ds_height);

        let Some(candidate) = find_best_chart_window(ds_width, ds_height, &integral) else {
            return Ok(None);
        };

        let rect = Rect {
            x: (candidate.x as f32 * scale_x) as usize,
            y: (candidate.y as f32 * scale_y) as usize,
            w: (candidate.w as f32 * scale_x) as usize,
            h: (candidate.h as f32 * scale_y) as usize,
        };

        let patches = sample_patches(rgb, rect)?;
        if patches.len() != 24 {
            return Ok(None);
        }

        let variance = patch_variance(&patches);
        if variance < 0.002 {
            tracing::info!("Color variance too low for chart detection");
            return Ok(None);
        }

        let (matrix, error) = fit_ccm_with_rotation(&patches)?;
        if error > 10.0 {
            tracing::info!("Chart detected but DeltaE too high: {:.2}", error);
            return Ok(None);
        }

        Ok(Some(CalibrationResult { matrix, error }))
    }
}

#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

#[derive(Clone, Copy)]
struct Window {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    score: f32,
}

fn downsample(rgb: &Array3<f64>, max_dim: usize) -> (Vec<[f32; 3]>, usize, usize, f32, f32) {
    let (height, width, _) = rgb.dim();
    let max_side = width.max(height);
    let scale = if max_side > max_dim {
        max_side as f32 / max_dim as f32
    } else {
        1.0
    };
    let step = scale.ceil() as usize;
    let ds_width = (width + step - 1) / step;
    let ds_height = (height + step - 1) / step;

    let mut out = vec![[0.0f32; 3]; ds_width * ds_height];
    for y in 0..ds_height {
        let src_y = (y * step).min(height - 1);
        for x in 0..ds_width {
            let src_x = (x * step).min(width - 1);
            let r = rgb[(src_y, src_x, 0)] as f32;
            let g = rgb[(src_y, src_x, 1)] as f32;
            let b = rgb[(src_y, src_x, 2)] as f32;
            out[y * ds_width + x] = [r, g, b];
        }
    }

    let scale_x = width as f32 / ds_width as f32;
    let scale_y = height as f32 / ds_height as f32;
    (out, ds_width, ds_height, scale_x, scale_y)
}

fn compute_luminance(rgb: &[[f32; 3]], width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let [r, g, b] = rgb[y * width + x];
            out[y * width + x] = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        }
    }
    out
}

fn compute_gradient(lum: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut grad = vec![0.0f32; width * height];
    if width < 3 || height < 3 {
        return grad;
    }
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let gx = lum[y * width + (x + 1)] - lum[y * width + (x - 1)];
            let gy = lum[(y + 1) * width + x] - lum[(y - 1) * width + x];
            grad[y * width + x] = (gx * gx + gy * gy).sqrt();
        }
    }
    grad
}

fn compute_integral(src: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut integral = vec![0.0f32; (width + 1) * (height + 1)];
    for y in 0..height {
        let mut row_sum = 0.0f32;
        for x in 0..width {
            row_sum += src[y * width + x];
            let idx = (y + 1) * (width + 1) + (x + 1);
            integral[idx] = integral[(y) * (width + 1) + (x + 1)] + row_sum;
        }
    }
    integral
}

fn rect_sum(integral: &[f32], width: usize, x: usize, y: usize, w: usize, h: usize) -> f32 {
    let stride = width + 1;
    let x2 = x + w;
    let y2 = y + h;
    let a = integral[y * stride + x];
    let b = integral[y * stride + x2];
    let c = integral[y2 * stride + x];
    let d = integral[y2 * stride + x2];
    d - b - c + a
}

fn find_best_chart_window(width: usize, height: usize, integral: &[f32]) -> Option<Window> {
    let min_w = (width as f32 * 0.2) as usize;
    let max_w = (width as f32 * 0.8) as usize;
    if min_w < 12 || max_w < min_w {
        return None;
    }
    let mut best: Option<Window> = None;
    let steps = 6;
    for i in 0..steps {
        let w = min_w + (max_w - min_w) * i / (steps - 1);
        let h = ((w as f32) / 1.5) as usize;
        if h < 8 || h >= height {
            continue;
        }
        let step_x = (w / 8).max(4);
        let step_y = (h / 8).max(4);
        for y in (0..=(height - h)).step_by(step_y) {
            for x in (0..=(width - w)).step_by(step_x) {
                let sum = rect_sum(integral, width, x, y, w, h);
                let score = sum / (w * h) as f32;
                match best {
                    Some(b) if score <= b.score => {}
                    _ => {
                        best = Some(Window { x, y, w, h, score });
                    }
                }
            }
        }
    }
    best
}

fn sample_patches(rgb: &Array3<f64>, rect: Rect) -> Result<Vec<[f32; 3]>> {
    let (height, width, _) = rgb.dim();
    let x0 = rect.x.min(width - 1);
    let y0 = rect.y.min(height - 1);
    let w = rect.w.min(width - x0);
    let h = rect.h.min(height - y0);

    if w < 24 || h < 16 {
        return Ok(Vec::new());
    }

    let rows = 4;
    let cols = 6;
    let patch_w = w as f32 / cols as f32;
    let patch_h = h as f32 / rows as f32;
    let mut patches = Vec::with_capacity(rows * cols);

    for row in 0..rows {
        for col in 0..cols {
            let px0 = x0 as f32 + col as f32 * patch_w;
            let py0 = y0 as f32 + row as f32 * patch_h;
            let px1 = px0 + patch_w;
            let py1 = py0 + patch_h;

            let margin_x = patch_w * 0.2;
            let margin_y = patch_h * 0.2;
            let sx0 = (px0 + margin_x).round() as usize;
            let sy0 = (py0 + margin_y).round() as usize;
            let sx1 = (px1 - margin_x).round() as usize;
            let sy1 = (py1 - margin_y).round() as usize;

            let sx1 = sx1.min(width);
            let sy1 = sy1.min(height);
            if sx1 <= sx0 || sy1 <= sy0 {
                patches.push([0.0, 0.0, 0.0]);
                continue;
            }

            let mut sum = [0.0f64; 3];
            let mut count = 0u64;
            for y in sy0..sy1 {
                for x in sx0..sx1 {
                    sum[0] += rgb[(y, x, 0)];
                    sum[1] += rgb[(y, x, 1)];
                    sum[2] += rgb[(y, x, 2)];
                    count += 1;
                }
            }
            let denom = count.max(1) as f64;
            patches.push([
                (sum[0] / denom) as f32,
                (sum[1] / denom) as f32,
                (sum[2] / denom) as f32,
            ]);
        }
    }

    Ok(patches)
}

fn patch_variance(patches: &[[f32; 3]]) -> f32 {
    if patches.is_empty() {
        return 0.0;
    }
    let mut mean = [0.0f32; 3];
    for p in patches {
        mean[0] += p[0];
        mean[1] += p[1];
        mean[2] += p[2];
    }
    let n = patches.len() as f32;
    mean[0] /= n;
    mean[1] /= n;
    mean[2] /= n;

    let mut var = 0.0f32;
    for p in patches {
        var += (p[0] - mean[0]).powi(2) + (p[1] - mean[1]).powi(2) + (p[2] - mean[2]).powi(2);
    }
    var / n
}

fn fit_ccm_with_rotation(patches: &[[f32; 3]]) -> Result<([[f32; 3]; 3], f32)> {
    let reference = reference_colors();
    let rotations = [0, 180];
    let mut best: Option<([[f32; 3]; 3], f32)> = None;

    for rotation in rotations {
        let rotated = rotate_patches(patches, 4, 6, rotation);
        let (matrix, error) = fit_ccm(&rotated, &reference)?;
        match best {
            Some((_, best_err)) if error >= best_err => {}
            _ => best = Some((matrix, error)),
        }
    }

    best.ok_or_else(|| anyhow::anyhow!("No valid rotation found"))
}

fn rotate_patches(patches: &[[f32; 3]], rows: usize, cols: usize, rotation: u16) -> Vec<[f32; 3]> {
    let mut out = vec![[0.0f32; 3]; patches.len()];
    for r in 0..rows {
        for c in 0..cols {
            let src_idx = r * cols + c;
            let (nr, nc) = match rotation {
                0 => (r, c),
                180 => (rows - 1 - r, cols - 1 - c),
                _ => (r, c),
            };
            let dst_idx = nr * cols + nc;
            if dst_idx < out.len() && src_idx < patches.len() {
                out[dst_idx] = patches[src_idx];
            }
        }
    }
    out
}

fn fit_ccm(observed: &[[f32; 3]], reference: &[[f32; 3]]) -> Result<([[f32; 3]; 3], f32)> {
    let n = observed.len().min(reference.len());
    if n < 6 {
        return Err(anyhow::anyhow!("Not enough patches"));
    }

    let mut a_data = Vec::with_capacity(n * 3);
    let mut b_data = Vec::with_capacity(n * 3);
    for i in 0..n {
        let obs = to_linear(observed[i]);
        let refc = to_linear(reference[i]);
        a_data.extend_from_slice(&[obs[0] as f64, obs[1] as f64, obs[2] as f64]);
        b_data.extend_from_slice(&[refc[0] as f64, refc[1] as f64, refc[2] as f64]);
    }

    let a = na::DMatrix::<f64>::from_row_slice(n, 3, &a_data);
    let b = na::DMatrix::<f64>::from_row_slice(n, 3, &b_data);
    let ata = &a.transpose() * &a;
    let atb = &a.transpose() * &b;
    let Some(inv) = ata.try_inverse() else {
        return Err(anyhow::anyhow!("Matrix inversion failed"));
    };
    let m = inv * atb;

    let matrix = [
        [m[(0, 0)] as f32, m[(0, 1)] as f32, m[(0, 2)] as f32],
        [m[(1, 0)] as f32, m[(1, 1)] as f32, m[(1, 2)] as f32],
        [m[(2, 0)] as f32, m[(2, 1)] as f32, m[(2, 2)] as f32],
    ];

    let error = delta_e_error(observed, reference, &matrix);
    Ok((matrix, error))
}

fn to_linear(rgb: [f32; 3]) -> [f32; 3] {
    [
        srgb_to_linear(rgb[0]),
        srgb_to_linear(rgb[1]),
        srgb_to_linear(rgb[2]),
    ]
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_xyz(rgb: [f32; 3]) -> [f32; 3] {
    let r = rgb[0];
    let g = rgb[1];
    let b = rgb[2];
    [
        r * 0.4124 + g * 0.3576 + b * 0.1805,
        r * 0.2126 + g * 0.7152 + b * 0.0722,
        r * 0.0193 + g * 0.1192 + b * 0.9505,
    ]
}

fn xyz_to_lab(xyz: [f32; 3]) -> [f32; 3] {
    let x = xyz[0] / 0.95047;
    let y = xyz[1] / 1.0;
    let z = xyz[2] / 1.08883;

    let fx = lab_f(x);
    let fy = lab_f(y);
    let fz = lab_f(z);

    let l = 116.0 * fy - 16.0;
    let a = 500.0 * (fx - fy);
    let b = 200.0 * (fy - fz);
    [l, a, b]
}

fn lab_f(t: f32) -> f32 {
    if t > 0.008856 {
        t.powf(1.0 / 3.0)
    } else {
        (7.787 * t) + (16.0 / 116.0)
    }
}

fn delta_e(lab1: [f32; 3], lab2: [f32; 3]) -> f32 {
    let dl = lab1[0] - lab2[0];
    let da = lab1[1] - lab2[1];
    let db = lab1[2] - lab2[2];
    (dl * dl + da * da + db * db).sqrt()
}

fn delta_e_error(observed: &[[f32; 3]], reference: &[[f32; 3]], matrix: &[[f32; 3]; 3]) -> f32 {
    let n = observed.len().min(reference.len());
    if n == 0 {
        return 0.0;
    }
    let mut total = 0.0f32;
    for i in 0..n {
        let obs = to_linear(observed[i]);
        let corrected = apply_matrix(obs, matrix);
        let ref_lin = to_linear(reference[i]);
        let lab_obs = xyz_to_lab(linear_to_xyz(corrected));
        let lab_ref = xyz_to_lab(linear_to_xyz(ref_lin));
        total += delta_e(lab_obs, lab_ref);
    }
    total / n as f32
}

fn apply_matrix(rgb: [f32; 3], matrix: &[[f32; 3]; 3]) -> [f32; 3] {
    let r = rgb[0];
    let g = rgb[1];
    let b = rgb[2];
    [
        (matrix[0][0] * r + matrix[0][1] * g + matrix[0][2] * b).clamp(0.0, 1.0),
        (matrix[1][0] * r + matrix[1][1] * g + matrix[1][2] * b).clamp(0.0, 1.0),
        (matrix[2][0] * r + matrix[2][1] * g + matrix[2][2] * b).clamp(0.0, 1.0),
    ]
}

fn reference_colors() -> Vec<[f32; 3]> {
    vec![
        [0.400, 0.350, 0.260],
        [0.650, 0.480, 0.350],
        [0.290, 0.420, 0.690],
        [0.370, 0.580, 0.290],
        [0.450, 0.270, 0.270],
        [0.770, 0.650, 0.210],
        [0.180, 0.240, 0.600],
        [0.310, 0.180, 0.510],
        [0.680, 0.230, 0.230],
        [0.200, 0.560, 0.570],
        [0.810, 0.720, 0.230],
        [0.460, 0.160, 0.440],
        [0.750, 0.330, 0.210],
        [0.120, 0.120, 0.120],
        [0.230, 0.230, 0.230],
        [0.350, 0.350, 0.350],
        [0.480, 0.480, 0.480],
        [0.620, 0.620, 0.620],
        [0.750, 0.750, 0.750],
        [0.880, 0.880, 0.880],
        [0.060, 0.060, 0.060],
        [0.180, 0.070, 0.060],
        [0.090, 0.090, 0.240],
        [0.040, 0.160, 0.070],
    ]
}
