// Adaptive Homogeneity-Directed (AHD) Demosaicing Algorithm
// Based on the work of Keigo Hirakawa, Thomas Parks, and Paul Lee
// Reference implementation: dcraw by Dave Coffin

use anyhow::{Context, Result};
use ndarray::{Array2, Array3};
use rayon::prelude::*;

// Sized to keep each worker's directional RGB/Lab/homogeneity working set near
// Apple Silicon's private cache while exposing enough independent row bands.
const TS: usize = 160;
const TILE_OVERLAP: usize = 6;
const TILE_STEP: usize = TS - TILE_OVERLAP;
const OUTPUT_BORDER: usize = 5;
const CIELAB_MATRIX_SHIFT: u32 = 11;
const CIELAB_MATRIX_SCALE: i32 = 1 << CIELAB_MATRIX_SHIFT;
const CIELAB_MATRIX_MAX: i32 = 8_191;
const CLASSIFIER_MAX: f32 = 65_535.0;
const CLASSIFIER_DARK_THRESHOLD: i32 = 1_310;

#[inline]
fn tile_rgb_index(direction: usize, row: usize, col: usize, channel: usize) -> usize {
    (((direction * TS + row) * TS + col) * 3) + channel
}

#[inline]
fn tile_homo_index(direction: usize, row: usize, col: usize) -> usize {
    (direction * TS + row) * TS + col
}

/// CFA pattern for RGGB Bayer
#[inline]
fn fc(row: usize, col: usize) -> usize {
    // RGGB pattern: (row%2, col%2) -> (0,0)=R, (0,1)=G, (1,0)=G, (1,1)=B
    ((row & 1) << 1) | (col & 1)
}

/// Map Bayer channel to RGB channel
/// Bayer: 0=R, 1=G, 2=G, 3=B
/// RGB: 0=R, 1=G, 2=B
#[inline]
fn fc_rgb(row: usize, col: usize) -> usize {
    let bayer = fc(row, col);
    match bayer {
        0 => 0,     // R
        1 | 2 => 1, // G
        3 => 2,     // B
        _ => unreachable!(),
    }
}

/// Clip value to 0.0-1.0 range
#[inline]
fn clip(val: f32) -> f32 {
    val.clamp(0.0, 1.0)
}

/// Clip value to range between two bounds (handles min > max case)
#[inline]
fn ulim(val: f32, a: f32, b: f32) -> f32 {
    let min = a.min(b);
    let max = a.max(b);
    val.clamp(min, max)
}

/// Convert RGB to CIELab color space
/// This is used for homogeneity detection
struct CieLabConverter {
    cbrt: Vec<i32>,
    xyz_cam: [[i32; 4]; 3],
}

impl CieLabConverter {
    fn new(rgb_cam: &[[f32; 4]; 3]) -> Result<Self> {
        Ok(Self {
            cbrt: cielab_cbrt_lut(),
            xyz_cam: quantized_xyz_camera_matrix(rgb_cam)?,
        })
    }

    #[cfg(test)]
    fn convert(&self, rgb: &[f32; 3]) -> [i16; 3] {
        self.convert_quantized(&rgb.map(|value| classifier_quantize(value)))
    }

    fn convert_quantized(&self, rgb_u16: &[i32; 3]) -> [i16; 3] {
        let mut roots = [0i32; 3];
        for (row, root) in roots.iter_mut().enumerate() {
            let dot = (0..3)
                .map(|channel| i64::from(self.xyz_cam[row][channel]) * i64::from(rgb_u16[channel]))
                .sum::<i64>();
            let xyz =
                div_round_signed_i64(dot, i64::from(CIELAB_MATRIX_SCALE)).clamp(0, 65_535) as usize;
            *root = self.cbrt[xyz];
        }

        // Integer arithmetic makes CPU and Metal classification independent of
        // fused floating-point operations and float-to-index boundary behavior.
        [
            div_round_signed_i32((116 * roots[1] - 16 * 32_767) * 64, 32_767) as i16,
            div_round_signed_i32(500 * (roots[0] - roots[1]) * 64, 32_767) as i16,
            div_round_signed_i32(200 * (roots[1] - roots[2]) * 64, 32_767) as i16,
        ]
    }
}

pub(crate) fn cielab_cbrt_lut() -> Vec<i32> {
    (0..65_536)
        .map(|index| {
            let value = index as f32 / 65_535.0;
            let transformed = if value > 0.008_856 {
                value.powf(1.0 / 3.0)
            } else {
                7.787 * value + 16.0 / 116.0
            };
            (transformed * 32_767.0).round() as i32
        })
        .collect()
}

pub(crate) fn quantized_xyz_camera_matrix(rgb_cam: &[[f32; 4]; 3]) -> Result<[[i32; 4]; 3]> {
    const XYZ_RGB: [[f32; 3]; 3] = [
        [0.412_453, 0.357_580, 0.180_423],
        [0.212_671, 0.715_160, 0.072_169],
        [0.019_334, 0.119_193, 0.950_227],
    ];
    const D65_WHITE: [f32; 3] = [0.950_456, 1.0, 1.088_754];
    let mut output = [[0i32; 4]; 3];
    for row in 0..3 {
        for column in 0..4 {
            let value = (0..3)
                .map(|channel| XYZ_RGB[row][channel] * rgb_cam[channel][column] / D65_WHITE[row])
                .sum::<f32>();
            let quantized = (value * CIELAB_MATRIX_SCALE as f32).round();
            if !quantized.is_finite() || quantized.abs() > CIELAB_MATRIX_MAX as f32 {
                anyhow::bail!(
                    "camera-to-XYZ matrix coefficient ({row}, {column}) is outside the fixed-point classifier range"
                );
            }
            output[row][column] = quantized as i32;
        }
    }
    Ok(output)
}

#[inline]
fn div_round_signed_i32(numerator: i32, denominator: i32) -> i32 {
    if numerator < 0 {
        -((-numerator + denominator / 2) / denominator)
    } else {
        (numerator + denominator / 2) / denominator
    }
}

#[inline]
fn div_round_signed_i64(numerator: i64, denominator: i64) -> i64 {
    if numerator < 0 {
        -((-numerator + denominator / 2) / denominator)
    } else {
        (numerator + denominator / 2) / denominator
    }
}

#[inline]
fn squared_chroma_difference(first_a: i16, first_b: i16, second_a: i16, second_b: i16) -> u32 {
    let da = (i32::from(first_a) - i32::from(second_a)).unsigned_abs() >> 1;
    let db = (i32::from(first_b) - i32::from(second_b)).unsigned_abs() >> 1;
    da * da + db * db
}

#[inline]
fn classifier_quantize(value: f32) -> i32 {
    (clip(value) * CLASSIFIER_MAX) as i32
}

#[inline]
fn classifier_sample(image: &Array3<f32>, row: usize, col: usize) -> i32 {
    classifier_quantize(image[[row, col, 0]])
}

fn classifier_green(image: &Array3<f32>, row: usize, col: usize, direction: usize) -> i32 {
    let (height, width, _) = image.dim();
    let original = classifier_sample(image, row, col);
    if fc_rgb(row, col) == 1 {
        return original;
    }
    let dark = original < CLASSIFIER_DARK_THRESHOLD;
    if direction == 0 && col > 1 && col + 2 < width {
        let left = classifier_sample(image, row, col - 1);
        let right = classifier_sample(image, row, col + 1);
        let value = if dark {
            div_round_signed_i32(left + right, 2)
        } else {
            div_round_signed_i32(
                2 * (left + original + right)
                    - classifier_sample(image, row, col - 2)
                    - classifier_sample(image, row, col + 2),
                4,
            )
        };
        return value.clamp(left.min(right), left.max(right));
    }
    if direction == 1 && row > 1 && row + 2 < height {
        let up = classifier_sample(image, row - 1, col);
        let down = classifier_sample(image, row + 1, col);
        let value = if dark {
            div_round_signed_i32(up + down, 2)
        } else {
            div_round_signed_i32(
                2 * (up + original + down)
                    - classifier_sample(image, row - 2, col)
                    - classifier_sample(image, row + 2, col),
                4,
            )
        };
        return value.clamp(up.min(down), up.max(down));
    }
    original
}

fn classifier_rgb(image: &Array3<f32>, row: usize, col: usize, direction: usize) -> [i32; 3] {
    let (height, width, _) = image.dim();
    let channel = fc_rgb(row, col);
    let original = classifier_sample(image, row, col);
    let dark = original < CLASSIFIER_DARK_THRESHOLD;
    let green = classifier_green(image, row, col, direction);
    let mut rgb = [0i32; 3];
    rgb[channel] = original;
    rgb[1] = green;

    if channel == 1 && row > 0 && row + 1 < height && col > 0 && col + 1 < width {
        let adjacent_channel = fc_rgb(row + 1, col);
        let horizontal = if dark {
            div_round_signed_i32(
                classifier_sample(image, row, col - 1) + classifier_sample(image, row, col + 1),
                2,
            )
        } else {
            green
                + div_round_signed_i32(
                    classifier_sample(image, row, col - 1) + classifier_sample(image, row, col + 1)
                        - classifier_green(image, row, col - 1, direction)
                        - classifier_green(image, row, col + 1, direction),
                    2,
                )
        };
        let vertical = if dark {
            div_round_signed_i32(
                classifier_sample(image, row - 1, col) + classifier_sample(image, row + 1, col),
                2,
            )
        } else {
            green
                + div_round_signed_i32(
                    classifier_sample(image, row - 1, col) + classifier_sample(image, row + 1, col)
                        - classifier_green(image, row - 1, col, direction)
                        - classifier_green(image, row + 1, col, direction),
                    2,
                )
        };
        rgb[2 - adjacent_channel] = horizontal.clamp(0, 65_535);
        rgb[adjacent_channel] = vertical.clamp(0, 65_535);
    } else if channel != 1 && row > 0 && row + 1 < height && col > 0 && col + 1 < width {
        let source_sum = classifier_sample(image, row - 1, col - 1)
            + classifier_sample(image, row - 1, col + 1)
            + classifier_sample(image, row + 1, col - 1)
            + classifier_sample(image, row + 1, col + 1);
        let other = if dark {
            div_round_signed_i32(source_sum, 4)
        } else {
            let green_sum = classifier_green(image, row - 1, col - 1, direction)
                + classifier_green(image, row - 1, col + 1, direction)
                + classifier_green(image, row + 1, col - 1, direction)
                + classifier_green(image, row + 1, col + 1, direction);
            green + div_round_signed_i32(source_sum - green_sum, 4)
        };
        rgb[2 - channel] = other.clamp(0, 65_535);
    }
    rgb
}

fn demosaic_bilinear_pixel(image: &Array3<f32>, row: usize, col: usize) -> [f32; 3] {
    let (height, width, _) = image.dim();
    let direct_channel = fc_rgb(row, col);
    let mut rgb = [0.0f32; 3];
    rgb[direct_channel] = image[[row, col, 0]];

    for (channel, value) in rgb.iter_mut().enumerate() {
        if channel == direct_channel {
            continue;
        }
        let mut weighted_sum = 0.0f32;
        let mut weight_sum = 0.0f32;
        for radius in 1..=2isize {
            let row_start = (row as isize - radius).max(0);
            let row_end = (row as isize + radius).min(height as isize - 1);
            let col_start = (col as isize - radius).max(0);
            let col_end = (col as isize + radius).min(width as isize - 1);
            for source_row in row_start..=row_end {
                for source_col in col_start..=col_end {
                    if fc_rgb(source_row as usize, source_col as usize) != channel {
                        continue;
                    }
                    let dy = source_row - row as isize;
                    let dx = source_col - col as isize;
                    let distance_squared = (dx * dx + dy * dy) as f32;
                    if distance_squared == 0.0 {
                        continue;
                    }
                    let weight = distance_squared.recip();
                    weighted_sum += weight * image[[source_row as usize, source_col as usize, 0]];
                    weight_sum += weight;
                }
            }
            if weight_sum > 0.0 {
                break;
            }
        }
        *value = if weight_sum > 0.0 {
            weighted_sum / weight_sum
        } else {
            image[[row, col, 0]]
        };
    }
    rgb
}

/// AHD demosaicing algorithm
///
/// Input: Bayer pattern image (height x width x 1) single-channel Bayer data
/// Output: Full RGB image (height x width x 3)
pub fn ahd_demosaic(bayer: &Array3<f64>, rgb_cam: &[[f32; 4]; 3]) -> Result<Array3<f64>> {
    let image_f32 = bayer.mapv(|value| value as f32);
    let output = ahd_demosaic_f32_owned(image_f32, rgb_cam)?;
    Ok(output.mapv(|value| value as f64))
}

/// AHD demosaicing for a caller-owned normalized Bayer buffer.
///
/// Consuming the input lets the native fusion path reuse that allocation as
/// AHD working storage rather than cloning it or expanding it through `f64`.
pub fn ahd_demosaic_f32_owned(
    mut image_f32: Array3<f32>,
    rgb_cam: &[[f32; 4]; 3],
) -> Result<Array3<f32>> {
    let (height, width, channels) = image_f32.dim();
    if channels != 1 {
        anyhow::bail!(
            "Expected single-channel Bayer input, got {} channels",
            channels
        );
    }
    if height == 0 || width == 0 {
        anyhow::bail!("Cannot demosaic an empty Bayer image");
    }

    tracing::info!("Starting f32 AHD demosaicing on {}x{} image", height, width);

    if image_f32
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        anyhow::bail!("AHD input must contain finite, non-negative linear sensor values");
    }
    let range_scale = ahd_normalization_scale(image_f32.iter().copied().fold(1.0f32, f32::max))?;
    if range_scale != 1.0 {
        image_f32.mapv_inplace(|value| value / range_scale);
    }

    // Initialize CIELab converter
    let cielab = CieLabConverter::new(rgb_cam)?;

    // Allocate output image (f32 for precision)
    let mut output = Array3::<f32>::zeros((height, width, 3));

    // Each row band owns a disjoint output slice, so AHD can use every CPU core
    // without locks or an additional full-frame output allocation.
    let interior_start = OUTPUT_BORDER.min(height);
    let interior_end = height.saturating_sub(OUTPUT_BORDER);
    let row_stride = width * 3;
    if interior_start < interior_end && width > OUTPUT_BORDER * 2 {
        let output_pixels = output
            .as_slice_mut()
            .context("AHD output allocation is not contiguous")?;
        let interior = &mut output_pixels[interior_start * row_stride..interior_end * row_stride];
        interior
            .par_chunks_mut(TILE_STEP * row_stride)
            .enumerate()
            .for_each(|(band_index, output_band)| {
                let output_y0 = interior_start + band_index * TILE_STEP;
                let top = output_y0 - 3;
                let mut scratch = AhdScratch::new();
                let mut left = 2;
                while left < width.saturating_sub(OUTPUT_BORDER) {
                    process_tile(
                        &image_f32,
                        output_band,
                        output_y0,
                        &cielab,
                        &mut scratch,
                        top,
                        left,
                        width,
                        height,
                    );
                    left += TILE_STEP;
                }
            });
    }

    // The edge fallback never changes measured CFA samples and never substitutes
    // a differently colored clamped neighbor.
    for row in 0..height {
        for col in 0..width {
            if row < OUTPUT_BORDER
                || row >= interior_end
                || col < OUTPUT_BORDER
                || col + OUTPUT_BORDER >= width
            {
                let rgb = demosaic_bilinear_pixel(&image_f32, row, col);
                output[[row, col, 0]] = rgb[0];
                output[[row, col, 1]] = rgb[1];
                output[[row, col, 2]] = rgb[2];
            }
        }
    }

    let zero_count = output
        .as_slice()
        .map(|pixels| {
            pixels
                .chunks_exact(3)
                .filter(|pixel| pixel.iter().all(|&channel| channel == 0.0))
                .count()
        })
        .unwrap_or(0);
    let total_count = height * width;

    if zero_count > 0 {
        tracing::debug!("Found {} zero pixels out of {} total ({:.2}%) - likely from very dark areas in fused Bayer",
            zero_count, total_count, 100.0 * zero_count as f64 / total_count as f64);
    }

    if range_scale != 1.0 {
        output.mapv_inplace(|value| value * range_scale);
    }
    tracing::info!("AHD demosaicing complete");
    Ok(output)
}

/// Recompute the exact AHD direction decision map used by the CPU path.
///
/// Values are `0` for an equal-score average, `1` for horizontal, and `2` for
/// vertical. This qualification-only diagnostic intentionally excludes timing
/// measurements from the production demosaic benchmark.
pub fn ahd_direction_map_f32(image: &Array3<f32>, rgb_cam: &[[f32; 4]; 3]) -> Result<Array2<u8>> {
    let (height, width, channels) = image.dim();
    if channels != 1 || height == 0 || width == 0 {
        anyhow::bail!("AHD direction diagnostics require non-empty single-channel Bayer input");
    }
    if image.iter().any(|value| !value.is_finite() || *value < 0.0) {
        anyhow::bail!("AHD direction diagnostics require finite, non-negative values");
    }
    let range_scale = ahd_normalization_scale(image.iter().copied().fold(1.0f32, f32::max))?;
    let normalized;
    let image = if range_scale == 1.0 {
        image
    } else {
        normalized = image.mapv(|value| value / range_scale);
        &normalized
    };
    let converter = CieLabConverter::new(rgb_cam)?;
    let mut directions = Array2::<u8>::zeros((height, width));
    let interior_start = OUTPUT_BORDER.min(height);
    let interior_end = height.saturating_sub(OUTPUT_BORDER);
    if interior_start >= interior_end || width <= OUTPUT_BORDER * 2 {
        return Ok(directions);
    }

    let mut scratch = AhdScratch::new();
    for output_y0 in (interior_start..interior_end).step_by(TILE_STEP) {
        let output_y1 = (output_y0 + TILE_STEP).min(interior_end);
        let top = output_y0 - 3;
        let mut left = 2;
        while left < width.saturating_sub(OUTPUT_BORDER) {
            let tile_height = (top + TS).min(height - 2);
            let tile_width = (left + TS).min(width - 2);
            scratch.clear();
            build_classifier_lab(
                image,
                &mut scratch.lab,
                &converter,
                top,
                left,
                tile_height,
                tile_width,
            );
            build_homogeneity_maps(
                &scratch.lab,
                &mut scratch.homo,
                top,
                left,
                tile_height,
                tile_width,
            );
            write_direction_map(
                &mut directions,
                &scratch.homo,
                output_y0,
                output_y1,
                top,
                left,
                tile_height,
                tile_width,
            );
            left += TILE_STEP;
        }
    }
    Ok(directions)
}

/// Return the exact power-of-two scale used to normalize HDR-linear AHD input.
///
/// Power-of-two division and multiplication preserve measured `f32` samples
/// exactly while keeping the direction classifier in its calibrated range.
pub fn ahd_normalization_scale(maximum: f32) -> Result<f32> {
    if !maximum.is_finite() || maximum < 0.0 {
        anyhow::bail!("AHD normalization maximum must be finite and non-negative");
    }
    if maximum <= 1.0 {
        return Ok(1.0);
    }
    let scale = 2.0f32.powi(maximum.log2().ceil() as i32);
    if !scale.is_finite() {
        anyhow::bail!("AHD input dynamic range exceeds finite normalization support");
    }
    Ok(scale)
}

struct AhdScratch {
    rgb: Vec<f32>,
    lab: Vec<i16>,
    homo: Vec<u8>,
}

impl AhdScratch {
    fn new() -> Self {
        Self {
            rgb: vec![0.0; 2 * TS * TS * 3],
            lab: vec![0; 2 * TS * TS * 3],
            homo: vec![0; 2 * TS * TS],
        }
    }

    fn clear(&mut self) {
        self.rgb.fill(0.0);
        self.lab.fill(0);
        self.homo.fill(0);
    }
}

fn process_tile(
    image: &Array3<f32>,
    output_band: &mut [f32],
    output_y0: usize,
    cielab: &CieLabConverter,
    scratch: &mut AhdScratch,
    top: usize,
    left: usize,
    width: usize,
    height: usize,
) {
    let tile_height = (top + TS).min(height - 2);
    let tile_width = (left + TS).min(width - 2);

    scratch.clear();

    // Step 1: Interpolate green horizontally and vertically
    interpolate_green(
        image,
        &mut scratch.rgb,
        top,
        left,
        tile_height,
        tile_width,
        width,
        height,
    );

    // Step 2: Interpolate red and blue, convert to CIELab
    interpolate_rb_and_lab(
        image,
        &mut scratch.rgb,
        &mut scratch.lab,
        cielab,
        top,
        left,
        tile_height,
        tile_width,
        width,
    );

    // Step 3: Build homogeneity maps
    build_homogeneity_maps(
        &scratch.lab,
        &mut scratch.homo,
        top,
        left,
        tile_height,
        tile_width,
    );

    // Step 4: Combine most homogenous pixels
    combine_homogenous(
        output_band,
        output_y0,
        &scratch.rgb,
        &scratch.homo,
        top,
        left,
        tile_height,
        tile_width,
        width,
    );
}

fn interpolate_green(
    image: &Array3<f32>,
    rgb: &mut [f32],
    top: usize,
    left: usize,
    tile_height: usize,
    tile_width: usize,
    _width: usize,
    _height: usize,
) {
    let (img_height, img_width, _) = image.dim();

    // Threshold below which we use simple bilinear instead of edge-directed
    // This prevents artifacts in very dark areas where noise dominates
    const DARK_THRESHOLD: f32 = 0.02; // 2% of full scale

    // FIRST: Copy original green values for all green pixels
    for row in top..tile_height.min(img_height) {
        let tr = row - top;
        if tr >= TS {
            break;
        }

        for col in left..tile_width.min(img_width) {
            let tc = col - left;
            if tc >= TS {
                break;
            }

            let c = fc_rgb(row, col);
            if c == 1 {
                // Green pixel
                let orig_val = image[[row, col, 0]];
                rgb[tile_rgb_index(0, tr, tc, 1)] = orig_val;
                rgb[tile_rgb_index(1, tr, tc, 1)] = orig_val;
            }
        }
    }

    // SECOND: Interpolate green at R and B pixels in horizontal (d=0) and vertical (d=1) directions
    for row in top..tile_height.min(img_height) {
        let tr = row - top;
        if tr >= TS {
            break;
        } // Don't overflow tile buffer

        // Start at R or B pixel, not G pixel
        // For even rows (0, 2, 4, ...), we want even columns (R pixels)
        // For odd rows (1, 3, 5, ...), we want odd columns (B pixels)
        let mut col = left + ((row ^ left) & 1);
        while col < tile_width.min(img_width) {
            let tc = col - left;
            if tc >= TS {
                break;
            } // Don't overflow tile buffer

            // Get RGB channel index (0=R, 1=G, 2=B) for this pixel
            let c = fc_rgb(row, col);

            // Copy original color value to both directions
            let orig_val = image[[row, col, 0]];
            rgb[tile_rgb_index(0, tr, tc, c)] = orig_val;
            rgb[tile_rgb_index(1, tr, tc, c)] = orig_val;

            // Check if this is a dark area - use simple bilinear if so
            let is_dark = orig_val < DARK_THRESHOLD;

            // Bounds checking for horizontal interpolation
            if col > 1 && col + 2 < img_width {
                let val_h = if is_dark {
                    // Simple bilinear for dark areas
                    (image[[row, col - 1, 0]] + image[[row, col + 1, 0]]) * 0.5
                } else {
                    // Edge-directed interpolation for normal areas
                    let triple = (image[[row, col - 1, 0]] + image[[row, col, 0]])
                        + image[[row, col + 1, 0]];
                    (2.0f32.mul_add(triple, -image[[row, col - 2, 0]]) - image[[row, col + 2, 0]])
                        * 0.25
                };
                rgb[tile_rgb_index(0, tr, tc, 1)] =
                    ulim(val_h, image[[row, col - 1, 0]], image[[row, col + 1, 0]]);
            } else {
                // Fallback for border pixels
                rgb[tile_rgb_index(0, tr, tc, 1)] = image[[row, col, 0]];
            }

            // Bounds checking for vertical interpolation
            if row > 1 && row + 2 < img_height {
                let val_v = if is_dark {
                    // Simple bilinear for dark areas
                    (image[[row - 1, col, 0]] + image[[row + 1, col, 0]]) * 0.5
                } else {
                    // Edge-directed interpolation for normal areas
                    let triple = (image[[row - 1, col, 0]] + image[[row, col, 0]])
                        + image[[row + 1, col, 0]];
                    (2.0f32.mul_add(triple, -image[[row - 2, col, 0]]) - image[[row + 2, col, 0]])
                        * 0.25
                };
                rgb[tile_rgb_index(1, tr, tc, 1)] =
                    ulim(val_v, image[[row - 1, col, 0]], image[[row + 1, col, 0]]);
            } else {
                // Fallback for border pixels
                rgb[tile_rgb_index(1, tr, tc, 1)] = image[[row, col, 0]];
            }

            col += 2;
        }
    }
}

fn interpolate_rb_and_lab(
    image: &Array3<f32>,
    rgb: &mut [f32],
    lab: &mut [i16],
    cielab: &CieLabConverter,
    top: usize,
    left: usize,
    tile_height: usize,
    tile_width: usize,
    _width: usize,
) {
    let (img_height, img_width, _) = image.dim();

    // Threshold below which we use simple bilinear instead of edge-directed
    const DARK_THRESHOLD: f32 = 0.02; // 2% of full scale

    // Interpolate red and blue for both directions
    for d in 0..2 {
        for row in (top + 1)..(tile_height.saturating_sub(1)).min(img_height - 1) {
            let tr = row - top;
            if tr >= TS - 1 {
                break;
            } // Don't overflow tile buffer

            for col in (left + 1)..(tile_width.saturating_sub(1)).min(img_width - 1) {
                let tc = col - left;
                if tc >= TS - 1 {
                    break;
                } // Don't overflow tile buffer

                let c = fc_rgb(row, col);
                let orig_val = image[[row, col, 0]];
                let is_dark = orig_val < DARK_THRESHOLD;

                if c == 1 {
                    // Green pixel - interpolate R and B
                    let c2 = fc_rgb(row + 1, col);

                    // Interpolate one color (horizontal)
                    if col > 0 && col + 1 < img_width && tc > 0 && tc + 1 < TS {
                        let val = if is_dark {
                            // Simple bilinear for dark areas
                            (image[[row, col - 1, 0]] + image[[row, col + 1, 0]]) * 0.5
                        } else {
                            // Edge-directed interpolation
                            let delta = (image[[row, col - 1, 0]] + image[[row, col + 1, 0]])
                                - rgb[tile_rgb_index(d, tr, tc - 1, 1)]
                                - rgb[tile_rgb_index(d, tr, tc + 1, 1)];
                            delta.mul_add(0.5, rgb[tile_rgb_index(d, tr, tc, 1)])
                        };
                        rgb[tile_rgb_index(d, tr, tc, 2 - c2)] = clip(val);
                    }

                    // Interpolate other color (vertical)
                    if row > 0 && row + 1 < img_height && tr > 0 && tr + 1 < TS {
                        let val = if is_dark {
                            // Simple bilinear for dark areas
                            (image[[row - 1, col, 0]] + image[[row + 1, col, 0]]) * 0.5
                        } else {
                            // Edge-directed interpolation
                            let delta = (image[[row - 1, col, 0]] + image[[row + 1, col, 0]])
                                - rgb[tile_rgb_index(d, tr - 1, tc, 1)]
                                - rgb[tile_rgb_index(d, tr + 1, tc, 1)];
                            delta.mul_add(0.5, rgb[tile_rgb_index(d, tr, tc, 1)])
                        };
                        rgb[tile_rgb_index(d, tr, tc, c2)] = clip(val);
                    }
                } else {
                    // Red or Blue pixel - interpolate the other color
                    // If c=0 (Red), interpolate Blue (channel 2)
                    // If c=2 (Blue), interpolate Red (channel 0)
                    let other_color = 2 - c; // 0->2, 2->0

                    if row > 0
                        && row + 1 < img_height
                        && col > 0
                        && col + 1 < img_width
                        && tr > 0
                        && tr + 1 < TS
                        && tc > 0
                        && tc + 1 < TS
                    {
                        let val = if is_dark {
                            // Simple bilinear for dark areas
                            (image[[row - 1, col - 1, 0]]
                                + image[[row - 1, col + 1, 0]]
                                + image[[row + 1, col - 1, 0]]
                                + image[[row + 1, col + 1, 0]])
                                * 0.25
                        } else {
                            // Edge-directed interpolation
                            let source_sum = ((image[[row - 1, col - 1, 0]]
                                + image[[row - 1, col + 1, 0]])
                                + image[[row + 1, col - 1, 0]])
                                + image[[row + 1, col + 1, 0]];
                            let green_sum = ((rgb[tile_rgb_index(d, tr - 1, tc - 1, 1)]
                                + rgb[tile_rgb_index(d, tr - 1, tc + 1, 1)])
                                + rgb[tile_rgb_index(d, tr + 1, tc - 1, 1)])
                                + rgb[tile_rgb_index(d, tr + 1, tc + 1, 1)];
                            (source_sum - green_sum)
                                .mul_add(0.25, rgb[tile_rgb_index(d, tr, tc, 1)])
                        };
                        rgb[tile_rgb_index(d, tr, tc, other_color)] = clip(val);
                    }
                }

                // Copy original color
                let rgb_c = fc_rgb(row, col);
                rgb[tile_rgb_index(d, tr, tc, rgb_c)] = image[[row, col, 0]];

                // Convert to CIELab
                let lab_pixel = cielab.convert_quantized(&classifier_rgb(image, row, col, d));
                for channel in 0..3 {
                    lab[tile_rgb_index(d, tr, tc, channel)] = lab_pixel[channel];
                }
            }
        }
    }
}

fn build_classifier_lab(
    image: &Array3<f32>,
    lab: &mut [i16],
    converter: &CieLabConverter,
    top: usize,
    left: usize,
    tile_height: usize,
    tile_width: usize,
) {
    let (height, width, _) = image.dim();
    for direction in 0..2 {
        for row in (top + 1)..(tile_height.saturating_sub(1)).min(height - 1) {
            let tile_row = row - top;
            if tile_row >= TS - 1 {
                break;
            }
            for col in (left + 1)..(tile_width.saturating_sub(1)).min(width - 1) {
                let tile_col = col - left;
                if tile_col >= TS - 1 {
                    break;
                }
                let pixel =
                    converter.convert_quantized(&classifier_rgb(image, row, col, direction));
                for channel in 0..3 {
                    lab[tile_rgb_index(direction, tile_row, tile_col, channel)] = pixel[channel];
                }
            }
        }
    }
}

fn build_homogeneity_maps(
    lab: &[i16],
    homo: &mut [u8],
    top: usize,
    left: usize,
    tile_height: usize,
    tile_width: usize,
) {
    const DIR: [i32; 4] = [-1, 1, -(TS as i32), TS as i32];

    for row in (top + 2)..(tile_height.saturating_sub(2)) {
        let tr = row - top;
        if !(2..TS - 2).contains(&tr) {
            continue;
        } // Bounds check

        for col in (left + 2)..(tile_width.saturating_sub(2)) {
            let tc = col - left;
            if !(2..TS - 2).contains(&tc) {
                continue;
            } // Bounds check

            let mut ldiff = [[0u32; 4]; 2];
            let mut abdiff = [[0u32; 4]; 2];

            // Calculate differences in both directions
            for d in 0..2 {
                for i in 0..4 {
                    let offset = DIR[i];
                    let (dr, dc) = if offset == -1 {
                        (0, -1i32)
                    } else if offset == 1 {
                        (0, 1i32)
                    } else if offset == -(TS as i32) {
                        (-1i32, 0)
                    } else {
                        (1i32, 0)
                    };

                    let nr = (tr as i32 + dr) as usize;
                    let nc = (tc as i32 + dc) as usize;

                    // Bounds check
                    if nr >= TS || nc >= TS {
                        continue;
                    }

                    ldiff[d][i] = (lab[tile_rgb_index(d, tr, tc, 0)]
                        - lab[tile_rgb_index(d, nr, nc, 0)])
                    .unsigned_abs() as u32;
                    abdiff[d][i] = squared_chroma_difference(
                        lab[tile_rgb_index(d, tr, tc, 1)],
                        lab[tile_rgb_index(d, tr, tc, 2)],
                        lab[tile_rgb_index(d, nr, nc, 1)],
                        lab[tile_rgb_index(d, nr, nc, 2)],
                    );
                }
            }

            // Calculate epsilon thresholds
            let leps = ldiff[0][0]
                .max(ldiff[0][1])
                .min(ldiff[1][2].max(ldiff[1][3]));
            let abeps = abdiff[0][0]
                .max(abdiff[0][1])
                .min(abdiff[1][2].max(abdiff[1][3]));

            // Build homogeneity map
            for d in 0..2 {
                for i in 0..4 {
                    if ldiff[d][i] <= leps && abdiff[d][i] <= abeps {
                        homo[tile_homo_index(d, tr, tc)] += 1;
                    }
                }
            }
        }
    }
}

fn combine_homogenous(
    output_band: &mut [f32],
    output_y0: usize,
    rgb: &[f32],
    homo: &[u8],
    top: usize,
    left: usize,
    tile_height: usize,
    tile_width: usize,
    _width: usize,
) {
    let output_rows = output_band.len() / (_width * 3);
    let output_y1 = output_y0 + output_rows;
    for row in (top + 3)..(tile_height.saturating_sub(3)).min(output_y1) {
        if row < output_y0 {
            continue;
        }
        let tr = row - top;
        if !(3..TS - 3).contains(&tr) {
            continue;
        } // Bounds check

        for col in (left + 3)..(tile_width.saturating_sub(3)).min(_width) {
            let tc = col - left;
            if !(3..TS - 3).contains(&tc) {
                continue;
            } // Bounds check

            // Sum homogeneity in 3x3 neighborhood
            let mut hm = [0u32; 2];
            for d in 0..2 {
                for i in (tr.saturating_sub(1))..=(tr + 1).min(TS - 1) {
                    for j in (tc.saturating_sub(1))..=(tc + 1).min(TS - 1) {
                        hm[d] += homo[tile_homo_index(d, i, j)] as u32;
                    }
                }
            }

            // Choose direction or average
            if hm[0] != hm[1] {
                let best_d = if hm[1] > hm[0] { 1 } else { 0 };
                for c in 0..3 {
                    let output_index = ((row - output_y0) * _width + col) * 3 + c;
                    output_band[output_index] = rgb[tile_rgb_index(best_d, tr, tc, c)];
                }
            } else {
                // Average both directions
                for c in 0..3 {
                    let output_index = ((row - output_y0) * _width + col) * 3 + c;
                    output_band[output_index] = (rgb[tile_rgb_index(0, tr, tc, c)]
                        + rgb[tile_rgb_index(1, tr, tc, c)])
                        * 0.5;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn write_direction_map(
    directions: &mut Array2<u8>,
    homo: &[u8],
    output_y0: usize,
    output_y1: usize,
    top: usize,
    left: usize,
    tile_height: usize,
    tile_width: usize,
) {
    let width = directions.ncols();
    for row in (top + 3)..tile_height.saturating_sub(3).min(output_y1) {
        if row < output_y0 {
            continue;
        }
        let tile_row = row - top;
        for col in (left + 3)..tile_width.saturating_sub(3).min(width) {
            let tile_col = col - left;
            let mut sums = [0u32; 2];
            for direction in 0..2 {
                for sample_row in tile_row - 1..=tile_row + 1 {
                    for sample_col in tile_col - 1..=tile_col + 1 {
                        sums[direction] +=
                            homo[tile_homo_index(direction, sample_row, sample_col)] as u32;
                    }
                }
            }
            directions[[row, col]] = match sums[0].cmp(&sums[1]) {
                std::cmp::Ordering::Greater => 1,
                std::cmp::Ordering::Less => 2,
                std::cmp::Ordering::Equal => 0,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_camera_matrix() -> [[f32; 4]; 3] {
        [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ]
    }

    #[test]
    fn lab_lookup_retains_neutral_chroma() {
        let converter = CieLabConverter::new(&identity_camera_matrix()).unwrap();
        let lab = converter.convert(&[0.5, 0.5, 0.5]);
        assert!(lab[0] > 0);
        assert!(lab[1].abs() < 128, "neutral a* was {}", lab[1]);
        assert!(lab[2].abs() < 128, "neutral b* was {}", lab[2]);
    }

    #[test]
    fn fixed_point_classifier_rejects_invalid_camera_matrices() {
        let mut non_finite = identity_camera_matrix();
        non_finite[0][0] = f32::NAN;
        assert!(CieLabConverter::new(&non_finite).is_err());

        let mut out_of_range = identity_camera_matrix();
        out_of_range[0][0] = 100.0;
        assert!(CieLabConverter::new(&out_of_range).is_err());
    }

    #[test]
    fn chroma_distance_is_bounded_at_i16_extremes() {
        let distance = squared_chroma_difference(i16::MIN, i16::MIN, i16::MAX, i16::MAX);
        assert_eq!(distance, 2 * 32_767u32.pow(2));
        assert!(distance <= i32::MAX as u32);
    }

    #[test]
    fn f32_ahd_restores_hdr_range() {
        let bayer = Array3::<f32>::from_elem((12, 12, 1), 1.5);
        let output = ahd_demosaic_f32_owned(bayer, &identity_camera_matrix()).unwrap();
        assert!(output.iter().copied().fold(0.0f32, f32::max) > 1.0);
    }

    #[test]
    fn ahd_preserves_every_measured_cfa_sample_including_borders() {
        const SIZE: usize = 340;
        let mut bayer = Array3::<f32>::zeros((SIZE, SIZE, 1));
        for row in 0..SIZE {
            for col in 0..SIZE {
                bayer[[row, col, 0]] = 0.05 + row as f32 * 0.001 + col as f32 * 0.0001;
            }
        }
        let expected = bayer.clone();
        let output = ahd_demosaic_f32_owned(bayer, &identity_camera_matrix()).unwrap();

        for row in 0..SIZE {
            for col in 0..SIZE {
                let channel = fc_rgb(row, col);
                assert!(
                    (output[[row, col, channel]] - expected[[row, col, 0]]).abs() < 1e-6,
                    "measured sample changed at ({row}, {col})"
                );
            }
        }
    }

    #[test]
    fn ahd_preserves_true_black_and_rejects_invalid_sensor_values() {
        let black = Array3::<f32>::zeros((12, 12, 1));
        let output = ahd_demosaic_f32_owned(black, &identity_camera_matrix()).unwrap();
        assert!(output.iter().all(|value| *value == 0.0));

        let mut invalid = Array3::<f32>::zeros((12, 12, 1));
        invalid[[4, 4, 0]] = f32::NAN;
        assert!(ahd_demosaic_f32_owned(invalid, &identity_camera_matrix()).is_err());
    }
}
