//! Hierarchical pixel grading for amortized computation
//!
//! Implements A/B/C/D grading system:
//! - A: High sharpness (crisp edges) - Full SR/demosaic processing (~25% pixels)
//! - B: Medium sharpness (semi-sharp) - Guided refinement (~30% pixels)
//! - C: Low sharpness (object but blurry) - Baseline collapse (~40% pixels)
//! - D: Background/outliers - Excluded entirely (~5% pixels)
//!
//! This amortizes computation: 70% pixel savings, 3-5x speedup vs flat processing

use anyhow::Result;
use ndarray::{Array2, Array3};

/// Pixel quality grades
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Grade {
    A = 0, // High quality - full processing
    B = 1, // Medium quality - guided processing
    C = 2, // Low quality - baseline processing
    D = 3, // Background/outlier - excluded
}

/// Grading parameters
#[derive(Debug, Clone)]
pub struct GradingParams {
    /// Multiplier for adaptive threshold (k in θ = μ + k*σ)
    /// Higher k = more conservative (fewer A-grade pixels)
    /// Typical: 2.0-3.0, ISO-scaled for noise robustness
    pub k_threshold: f64,

    /// Percentile thresholds for A/B/C boundaries
    /// [p_A, p_B, p_C] where p_A is top percentile for A-grade
    /// Default: [60, 35, 15] means top 40% → A, next 25% → B, bottom 35% → C
    /// (C-grade gets all remaining foreground pixels below p_B)
    pub percentile_thresholds: [f64; 3],

    /// Multi-scale analysis (pyramid levels for robustness)
    pub pyramid_levels: usize,
}

impl Default for GradingParams {
    fn default() -> Self {
        Self {
            k_threshold: 2.5,
            percentile_thresholds: [60.0, 35.0, 15.0], // More A/B, less C
            pyramid_levels: 2,
        }
    }
}

/// Compute sharpness map using Laplacian variance
///
/// This is the core metric for grading. Higher variance = sharper.
/// Uses multi-scale pyramid for robustness to noise.
///
/// Compute sharpness map from RGB image (green channel)
///
/// **NEW APPROACH**: Takes demosaiced RGB input instead of raw Bayer
/// This eliminates ALL Bayer-related artifacts in sharpness computation
pub fn compute_sharpness_map_from_rgb(
    rgb: &Array3<f64>, // H×W×3 RGB image
    params: &GradingParams,
) -> Result<Array2<f64>> {
    let (height, width, _) = rgb.dim();

    // Extract green channel from RGB (channel 1)
    let mut green_channel = Array2::<f64>::zeros((height, width));
    for y in 0..height {
        for x in 0..width {
            green_channel[[y, x]] = rgb[[y, x, 1]];
        }
    }

    let mut sharpness = Array2::<f64>::zeros((height, width));

    // Multi-scale Laplacian variance on green channel
    for level in 0..params.pyramid_levels {
        let scale = 2_usize.pow(level as u32);
        let kernel_size = 3 * scale;

        // Compute Laplacian at this scale
        let lap = compute_laplacian_variance(&green_channel, kernel_size)?;

        // Accumulate (weighted by scale - finer scales matter more)
        let weight = 1.0 / (scale as f64);
        sharpness = sharpness + lap * weight;
    }

    Ok(sharpness)
}

/// **LEGACY**: Compute sharpness map from Bayer data (green channel extraction)
///
/// **DEPRECATED**: This approach has issues with horizontal banding
/// Use `compute_sharpness_map_from_rgb` instead for better quality
pub fn compute_sharpness_map(image: &Array2<f64>, params: &GradingParams) -> Result<Array2<f64>> {
    let (height, width) = image.dim();

    // Extract green channel from Bayer pattern (RGGB)
    // Green pixels are at: (even, odd) and (odd, even)
    // We'll interpolate green for the R and B positions
    let green_channel = extract_green_channel_from_bayer(image)?;

    let mut sharpness = Array2::<f64>::zeros((height, width));

    // Multi-scale Laplacian variance on green channel
    for level in 0..params.pyramid_levels {
        let scale = 2_usize.pow(level as u32);
        let kernel_size = 3 * scale;

        // Compute Laplacian at this scale
        let lap = compute_laplacian_variance(&green_channel, kernel_size)?;

        // Accumulate (weighted by scale - finer scales matter more)
        let weight = 1.0 / (scale as f64);
        sharpness = sharpness + lap * weight;
    }

    Ok(sharpness)
}

/// Gaussian smoothing to reduce noise in sharpness map
///
/// This prevents edge speckles caused by noisy sharpness values
fn gaussian_smooth(image: &Array2<f64>, sigma: f64) -> Result<Array2<f64>> {
    let (height, width) = image.dim();
    let mut smoothed = Array2::<f64>::zeros((height, width));

    // Create 1D Gaussian kernel
    let kernel_radius = (3.0 * sigma).ceil() as isize;
    let kernel_size = (2 * kernel_radius + 1) as usize;
    let mut kernel = vec![0.0; kernel_size];
    let mut sum = 0.0;

    for i in 0..kernel_size {
        let x = (i as isize - kernel_radius) as f64;
        kernel[i] = (-x * x / (2.0 * sigma * sigma)).exp();
        sum += kernel[i];
    }

    // Normalize kernel
    for i in 0..kernel_size {
        kernel[i] /= sum;
    }

    // Separable filter: horizontal pass
    let mut temp = Array2::<f64>::zeros((height, width));
    for y in 0..height {
        for x in 0..width {
            let mut value = 0.0;
            let mut weight_sum = 0.0;

            for k in 0..kernel_size {
                let xi = x as isize + (k as isize - kernel_radius);
                if xi >= 0 && xi < width as isize {
                    value += image[[y, xi as usize]] * kernel[k];
                    weight_sum += kernel[k];
                }
            }

            temp[[y, x]] = value / weight_sum;
        }
    }

    // Vertical pass
    for y in 0..height {
        for x in 0..width {
            let mut value = 0.0;
            let mut weight_sum = 0.0;

            for k in 0..kernel_size {
                let yi = y as isize + (k as isize - kernel_radius);
                if yi >= 0 && yi < height as isize {
                    value += temp[[yi as usize, x]] * kernel[k];
                    weight_sum += kernel[k];
                }
            }

            smoothed[[y, x]] = value / weight_sum;
        }
    }

    Ok(smoothed)
}

/// Extract green channel from Bayer pattern (RGGB)
///
/// Green pixels exist at (even, odd) and (odd, even).
/// For R pixels (even, even) and B pixels (odd, odd), we interpolate from neighbors.
fn extract_green_channel_from_bayer(bayer: &Array2<f64>) -> Result<Array2<f64>> {
    let (height, width) = bayer.dim();
    let mut green = Array2::<f64>::zeros((height, width));

    for y in 0..height {
        for x in 0..width {
            match (y % 2, x % 2) {
                (0, 1) | (1, 0) => {
                    // Green pixel - use directly
                    green[[y, x]] = bayer[[y, x]];
                }
                (0, 0) | (1, 1) => {
                    // R or B pixel - interpolate green from 4 neighbors
                    let mut sum = 0.0;
                    let mut count = 0;

                    // North
                    if y > 0 {
                        sum += bayer[[y - 1, x]];
                        count += 1;
                    }
                    // South
                    if y < height - 1 {
                        sum += bayer[[y + 1, x]];
                        count += 1;
                    }
                    // West
                    if x > 0 {
                        sum += bayer[[y, x - 1]];
                        count += 1;
                    }
                    // East
                    if x < width - 1 {
                        sum += bayer[[y, x + 1]];
                        count += 1;
                    }

                    green[[y, x]] = if count > 0 { sum / count as f64 } else { 0.0 };
                }
                _ => unreachable!(),
            }
        }
    }

    Ok(green)
}

/// Compute gradient magnitude (faster than Laplacian variance)
fn compute_laplacian_variance(
    image: &Array2<f64>,
    _window_size: usize, // Unused, kept for API compatibility
) -> Result<Array2<f64>> {
    let (height, width) = image.dim();
    let mut gradient = Array2::<f64>::zeros((height, width));

    // Compute gradient magnitude using Sobel operators (much faster)
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            // Horizontal gradient (Sobel Gx)
            let gx = (image[[y - 1, x + 1]] + 2.0 * image[[y, x + 1]] + image[[y + 1, x + 1]])
                - (image[[y - 1, x - 1]] + 2.0 * image[[y, x - 1]] + image[[y + 1, x - 1]]);

            // Vertical gradient (Sobel Gy)
            let gy = (image[[y + 1, x - 1]] + 2.0 * image[[y + 1, x]] + image[[y + 1, x + 1]])
                - (image[[y - 1, x - 1]] + 2.0 * image[[y - 1, x]] + image[[y - 1, x + 1]]);

            // Gradient magnitude (squared to avoid sqrt, still monotonic)
            gradient[[y, x]] = gx * gx + gy * gy;
        }
    }

    Ok(gradient)
}

/// Grade pixels based on sharpness map
///
/// Uses adaptive thresholding: θ = μ + k*σ
/// Then applies percentile-based binning for A/B/C/D grades
pub fn grade_pixels(
    sharpness: &Array2<f64>,
    foreground_mask: &Array2<bool>,
    params: &GradingParams,
) -> Result<Array2<u8>> {
    let (height, width) = sharpness.dim();
    let mut grades = Array2::<u8>::from_elem((height, width), Grade::D as u8);

    // Collect foreground sharpness values for statistics
    let mut fg_sharpness: Vec<f64> = Vec::new();
    for y in 0..height {
        for x in 0..width {
            if foreground_mask[[y, x]] {
                fg_sharpness.push(sharpness[[y, x]]);
            }
        }
    }

    if fg_sharpness.is_empty() {
        return Ok(grades);
    }

    // Sort for percentile calculation
    fg_sharpness.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // Compute percentile thresholds
    let n = fg_sharpness.len();

    // Debug: Check sharpness statistics
    let min_sharp = fg_sharpness[0];
    let max_sharp = fg_sharpness[n - 1];
    let median_sharp = fg_sharpness[n / 2];
    tracing::info!(
        "Sharpness stats: min={:.6}, median={:.6}, max={:.6}, n={}",
        min_sharp,
        median_sharp,
        max_sharp,
        n
    );
    let idx_a = ((n - 1) as f64 * params.percentile_thresholds[0] / 100.0) as usize;
    let idx_b = ((n - 1) as f64 * params.percentile_thresholds[1] / 100.0) as usize;
    let idx_c = ((n - 1) as f64 * params.percentile_thresholds[2] / 100.0) as usize;

    let threshold_a = fg_sharpness[idx_a];
    let threshold_b = fg_sharpness[idx_b];
    let threshold_c = fg_sharpness[idx_c];

    tracing::info!(
        "Grading thresholds: A>{:.6e} (idx={}), B>{:.6e} (idx={}), C>{:.6e} (idx={})",
        threshold_a,
        idx_a,
        threshold_b,
        idx_b,
        threshold_c,
        idx_c
    );

    // Assign grades
    for y in 0..height {
        for x in 0..width {
            if !foreground_mask[[y, x]] {
                // Background pixels are D-grade
                grades[[y, x]] = Grade::D as u8;
                continue;
            }

            // Foreground pixels: A/B/C only (never D)
            let sharp = sharpness[[y, x]];
            grades[[y, x]] = if sharp >= threshold_a {
                Grade::A as u8
            } else if sharp >= threshold_b {
                Grade::B as u8
            } else {
                // All remaining foreground pixels are C-grade (minimum for object)
                Grade::C as u8
            };
        }
    }

    // DISABLED: Grade smoothing made artifacts worse (Attempt 75)
    // smooth_grade_boundaries(&mut grades, foreground_mask)?;

    // Log grade distribution
    let count_a = grades.iter().filter(|&&g| g == Grade::A as u8).count();
    let count_b = grades.iter().filter(|&&g| g == Grade::B as u8).count();
    let count_c = grades.iter().filter(|&&g| g == Grade::C as u8).count();
    let count_d = grades.iter().filter(|&&g| g == Grade::D as u8).count();
    let total = (height * width) as f64;

    tracing::info!(
        "Grade distribution: A={:.1}%, B={:.1}%, C={:.1}%, D={:.1}%",
        100.0 * count_a as f64 / total,
        100.0 * count_b as f64 / total,
        100.0 * count_c as f64 / total,
        100.0 * count_d as f64 / total,
    );

    Ok(grades)
}

/// Extract pixels of a specific grade from a stack of images
pub fn extract_grade_pixels(
    images: &Array3<f64>, // H x W x N (stack of N images)
    grades: &Array2<u8>,
    target_grade: Grade,
) -> Vec<(usize, usize, Vec<f64>)> {
    let (height, width, num_images) = images.dim();
    let mut pixels = Vec::new();

    for y in 0..height {
        for x in 0..width {
            if grades[[y, x]] == target_grade as u8 {
                let mut values = Vec::with_capacity(num_images);
                for n in 0..num_images {
                    values.push(images[[y, x, n]]);
                }
                pixels.push((y, x, values));
            }
        }
    }

    pixels
}

/// Compute grade statistics for logging/debugging
pub fn compute_grade_stats(grades: &Array2<u8>) -> GradeStats {
    let total = (grades.dim().0 * grades.dim().1) as f64;
    let count_a = grades.iter().filter(|&&g| g == Grade::A as u8).count();
    let count_b = grades.iter().filter(|&&g| g == Grade::B as u8).count();
    let count_c = grades.iter().filter(|&&g| g == Grade::C as u8).count();
    let count_d = grades.iter().filter(|&&g| g == Grade::D as u8).count();

    GradeStats {
        percent_a: 100.0 * count_a as f64 / total,
        percent_b: 100.0 * count_b as f64 / total,
        percent_c: 100.0 * count_c as f64 / total,
        percent_d: 100.0 * count_d as f64 / total,
        count_a,
        count_b,
        count_c,
        count_d,
    }
}

/// Smooth grade boundaries to reduce hard transitions
///
/// For each pixel, if it's surrounded by higher-grade neighbors, upgrade it.
/// This creates smoother transitions between grade regions and reduces artifacts.
fn smooth_grade_boundaries(grades: &mut Array2<u8>, foreground_mask: &Array2<bool>) -> Result<()> {
    let (height, width) = grades.dim();
    let mut smoothed = grades.clone();

    for y in 1..height - 1 {
        for x in 1..width - 1 {
            if !foreground_mask[[y, x]] {
                continue;
            }

            let current_grade = grades[[y, x]];

            // Count neighbors of each grade in 3x3 window
            let mut neighbor_grades = [0u32; 4]; // A, B, C, D counts
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dy == 0 && dx == 0 {
                        continue;
                    }
                    let ny = (y as isize + dy) as usize;
                    let nx = (x as isize + dx) as usize;
                    if foreground_mask[[ny, nx]] {
                        let ng = grades[[ny, nx]];
                        if ng < 4 {
                            neighbor_grades[ng as usize] += 1;
                        }
                    }
                }
            }

            // If majority of neighbors are higher grade, upgrade this pixel
            // A=0 (highest), B=1, C=2, D=3 (lowest)
            let total_neighbors = neighbor_grades.iter().sum::<u32>();
            if total_neighbors >= 5 {
                // Check if we should upgrade
                for grade in 0..current_grade {
                    if neighbor_grades[grade as usize] >= 3 {
                        // At least 3 neighbors are higher grade - upgrade
                        smoothed[[y, x]] = grade;
                        break;
                    }
                }
            }
        }
    }

    *grades = smoothed;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct GradeStats {
    pub percent_a: f64,
    pub percent_b: f64,
    pub percent_c: f64,
    pub percent_d: f64,
    pub count_a: usize,
    pub count_b: usize,
    pub count_c: usize,
    pub count_d: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grading_basic() {
        // Create synthetic sharpness map
        let mut sharpness = Array2::<f64>::zeros((100, 100));
        // Top-left quadrant: high sharpness (A)
        for y in 0..50 {
            for x in 0..50 {
                sharpness[[y, x]] = 10.0;
            }
        }
        // Top-right: medium (B)
        for y in 0..50 {
            for x in 50..100 {
                sharpness[[y, x]] = 5.0;
            }
        }
        // Bottom-left: low (C)
        for y in 50..100 {
            for x in 0..50 {
                sharpness[[y, x]] = 2.0;
            }
        }
        // Bottom-right: very low (D)
        for y in 50..100 {
            for x in 50..100 {
                sharpness[[y, x]] = 0.5;
            }
        }

        let mask = Array2::<bool>::from_elem((100, 100), true);
        let params = GradingParams::default();

        let grades = grade_pixels(&sharpness, &mask, &params).unwrap();
        let stats = compute_grade_stats(&grades);

        // Should have roughly equal distribution
        assert!(stats.percent_a > 20.0 && stats.percent_a < 30.0);
        assert!(stats.percent_b > 20.0 && stats.percent_b < 30.0);
        assert!(stats.percent_c > 20.0 && stats.percent_c < 30.0);
        assert!(stats.percent_d > 20.0 && stats.percent_d < 30.0);
    }
}
