// Adaptive Homogeneity-Directed (AHD) Demosaicing Algorithm
// Based on the work of Keigo Hirakawa, Thomas Parks, and Paul Lee
// Reference implementation: dcraw by Dave Coffin

use anyhow::{Context, Result};
use ndarray::Array3;
use rayon::prelude::*;

// Sized to keep each worker's directional RGB/Lab/homogeneity working set near
// Apple Silicon's private cache while exposing enough independent row bands.
const TS: usize = 160;
const TILE_OVERLAP: usize = 6;
const TILE_STEP: usize = TS - TILE_OVERLAP;
const OUTPUT_BORDER: usize = 5;
const HOMOGENEITY_DECISION_MARGIN: u32 = 1;

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
    cbrt: Vec<i16>,
    xyz_cam: [[f32; 4]; 3],
}

impl CieLabConverter {
    fn new(rgb_cam: &[[f32; 4]; 3]) -> Self {
        // Precompute cube root lookup table for 0.0-1.0 range
        // Use 16-bit precision for lookup (65536 entries)
        let mut cbrt = vec![0i16; 65536];
        for i in 0..65536 {
            let r = i as f32 / 65535.0;
            let cbrt_val = if r > 0.008856 {
                r.powf(1.0 / 3.0)
            } else {
                7.787 * r + 16.0 / 116.0
            };
            cbrt[i] = (cbrt_val * 32767.0).round() as i16;
        }

        // XYZ from RGB matrix (D65 white point)
        const XYZ_RGB: [[f32; 3]; 3] = [
            [0.412453, 0.357580, 0.180423],
            [0.212671, 0.715160, 0.072169],
            [0.019334, 0.119193, 0.950227],
        ];
        const D65_WHITE: [f32; 3] = [0.950456, 1.0, 1.088754];

        // Compute xyz_cam = xyz_rgb * rgb_cam / d65_white
        let mut xyz_cam = [[0.0f32; 4]; 3];
        for i in 0..3 {
            for j in 0..4 {
                for k in 0..3 {
                    xyz_cam[i][j] += XYZ_RGB[i][k] * rgb_cam[k][j] / D65_WHITE[i];
                }
            }
        }

        Self { cbrt, xyz_cam }
    }

    fn convert(&self, rgb: &[f32; 3]) -> [i16; 3] {
        let mut xyz = [0.0f32; 3];
        for channel in 0..3 {
            xyz[0] += self.xyz_cam[0][channel] * rgb[channel];
            xyz[1] += self.xyz_cam[1][channel] * rgb[channel];
            xyz[2] += self.xyz_cam[2][channel] * rgb[channel];
        }
        let xyz_idx = xyz.map(|value| (clip(value) * 65_535.0) as usize);

        let xyz_cbrt = [
            self.cbrt[xyz_idx[0]] as f32 / 32767.0,
            self.cbrt[xyz_idx[1]] as f32 / 32767.0,
            self.cbrt[xyz_idx[2]] as f32 / 32767.0,
        ];

        // Scale Lab by 64 while retaining the full range inside i16.
        [
            ((116.0 * xyz_cbrt[1] - 16.0) * 64.0).round() as i16,
            (500.0 * (xyz_cbrt[0] - xyz_cbrt[1]) * 64.0).round() as i16,
            (200.0 * (xyz_cbrt[1] - xyz_cbrt[2]) * 64.0).round() as i16,
        ]
    }
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
    let cielab = CieLabConverter::new(rgb_cam);

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
                    ((image[[row, col - 1, 0]] + image[[row, col, 0]] + image[[row, col + 1, 0]])
                        * 2.0
                        - image[[row, col - 2, 0]]
                        - image[[row, col + 2, 0]])
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
                    ((image[[row - 1, col, 0]] + image[[row, col, 0]] + image[[row + 1, col, 0]])
                        * 2.0
                        - image[[row - 2, col, 0]]
                        - image[[row + 2, col, 0]])
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
                            rgb[tile_rgb_index(d, tr, tc, 1)]
                                + ((image[[row, col - 1, 0]] + image[[row, col + 1, 0]]
                                    - rgb[tile_rgb_index(d, tr, tc - 1, 1)]
                                    - rgb[tile_rgb_index(d, tr, tc + 1, 1)])
                                    * 0.5)
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
                            rgb[tile_rgb_index(d, tr, tc, 1)]
                                + ((image[[row - 1, col, 0]] + image[[row + 1, col, 0]]
                                    - rgb[tile_rgb_index(d, tr - 1, tc, 1)]
                                    - rgb[tile_rgb_index(d, tr + 1, tc, 1)])
                                    * 0.5)
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
                            rgb[tile_rgb_index(d, tr, tc, 1)]
                                + ((image[[row - 1, col - 1, 0]]
                                    + image[[row - 1, col + 1, 0]]
                                    + image[[row + 1, col - 1, 0]]
                                    + image[[row + 1, col + 1, 0]]
                                    - rgb[tile_rgb_index(d, tr - 1, tc - 1, 1)]
                                    - rgb[tile_rgb_index(d, tr - 1, tc + 1, 1)]
                                    - rgb[tile_rgb_index(d, tr + 1, tc - 1, 1)]
                                    - rgb[tile_rgb_index(d, tr + 1, tc + 1, 1)])
                                    * 0.25)
                        };
                        rgb[tile_rgb_index(d, tr, tc, other_color)] = clip(val);
                    }
                }

                // Copy original color
                let rgb_c = fc_rgb(row, col);
                rgb[tile_rgb_index(d, tr, tc, rgb_c)] = image[[row, col, 0]];

                // Convert to CIELab
                let rgb_pixel = [
                    rgb[tile_rgb_index(d, tr, tc, 0)],
                    rgb[tile_rgb_index(d, tr, tc, 1)],
                    rgb[tile_rgb_index(d, tr, tc, 2)],
                ];
                let lab_pixel = cielab.convert(&rgb_pixel);
                for channel in 0..3 {
                    lab[tile_rgb_index(d, tr, tc, channel)] = lab_pixel[channel];
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
                    let a_difference = lab[tile_rgb_index(d, tr, tc, 1)] as i32
                        - lab[tile_rgb_index(d, nr, nc, 1)] as i32;
                    let b_difference = lab[tile_rgb_index(d, tr, tc, 2)] as i32
                        - lab[tile_rgb_index(d, nr, nc, 2)] as i32;
                    abdiff[d][i] =
                        (a_difference * a_difference + b_difference * b_difference) as u32;
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
            if hm[0].abs_diff(hm[1]) > HOMOGENEITY_DECISION_MARGIN {
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
        let converter = CieLabConverter::new(&identity_camera_matrix());
        let lab = converter.convert(&[0.5, 0.5, 0.5]);
        assert!(lab[0] > 0);
        assert!(lab[1].abs() < 128, "neutral a* was {}", lab[1]);
        assert!(lab[2].abs() < 128, "neutral b* was {}", lab[2]);
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
