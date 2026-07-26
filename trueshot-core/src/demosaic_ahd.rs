// Adaptive Homogeneity-Directed (AHD) Demosaicing Algorithm
// Based on the work of Keigo Hirakawa, Thomas Parks, and Paul Lee
// Reference implementation: dcraw by Dave Coffin

use anyhow::Result;
use ndarray::Array3;

const TS: usize = 512; // Tile size for processing

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
        0 => 0,  // R
        1 | 2 => 1,  // G
        3 => 2,  // B
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
            cbrt[i] = (cbrt_val * 64.0 * 65535.0) as i16;
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
        let mut xyz = [0.5f32; 3];

        // Convert RGB to XYZ (rgb values are in 0.0-1.0 range)
        for c in 0..3 {
            xyz[0] += self.xyz_cam[0][c] * rgb[c];
            xyz[1] += self.xyz_cam[1][c] * rgb[c];
            xyz[2] += self.xyz_cam[2][c] * rgb[c];
        }

        // Apply cube root with clipping using lookup table
        let xyz_idx = [
            (clip(xyz[0]) * 65535.0) as usize,
            (clip(xyz[1]) * 65535.0) as usize,
            (clip(xyz[2]) * 65535.0) as usize,
        ];

        let xyz_cbrt = [
            self.cbrt[xyz_idx[0]] as i32,
            self.cbrt[xyz_idx[1]] as i32,
            self.cbrt[xyz_idx[2]] as i32,
        ];

        // Convert to Lab (cbrt values are scaled by 64*65535)
        [
            ((116 * xyz_cbrt[1] / 65535 - 16 * 64) as i16),
            ((500 * (xyz_cbrt[0] - xyz_cbrt[1]) / 65535) as i16),
            ((200 * (xyz_cbrt[1] - xyz_cbrt[2]) / 65535) as i16),
        ]
    }
}

/// Border interpolation using a local smoothing filter to stabilize edge values
fn border_interpolate(image: &mut Array3<f32>, width: usize, height: usize, border: usize) {
    if width == 0 || height == 0 {
        return;
    }
    let mut smoothed = image.clone();
    let clamp_row = |r: isize| -> usize { r.max(0).min(height as isize - 1) as usize };
    let clamp_col = |c: isize| -> usize { c.max(0).min(width as isize - 1) as usize };
    for row in 0..height {
        for col in 0..width {
            if row <= border || row + border >= height || col <= border || col + border >= width {
                let mut sum = 0.0f32;
                let mut count = 0.0f32;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let rr = clamp_row(row as isize + dy);
                        let cc = clamp_col(col as isize + dx);
                        sum += image[[rr, cc, 0]];
                        count += 1.0;
                    }
                }
                smoothed[[row, col, 0]] = sum / count.max(1.0);
            }
        }
    }
    *image = smoothed;
}

fn demosaic_bilinear_pixel(image: &Array3<f32>, row: usize, col: usize) -> [f32; 3] {
    let (height, width, _) = image.dim();
    let clamp_row = |r: isize| -> usize { r.max(0).min(height as isize - 1) as usize };
    let clamp_col = |c: isize| -> usize { c.max(0).min(width as isize - 1) as usize };
    let sample = |r: isize, c: isize| -> f32 {
        image[[clamp_row(r), clamp_col(c), 0]]
    };

    let center = image[[row, col, 0]];
    let is_row_even = row % 2 == 0;
    let is_col_even = col % 2 == 0;

    match fc_rgb(row, col) {
        0 => {
            // Red pixel
            let g = (sample(row as isize - 1, col as isize)
                + sample(row as isize + 1, col as isize)
                + sample(row as isize, col as isize - 1)
                + sample(row as isize, col as isize + 1)) * 0.25;
            let b = (sample(row as isize - 1, col as isize - 1)
                + sample(row as isize - 1, col as isize + 1)
                + sample(row as isize + 1, col as isize - 1)
                + sample(row as isize + 1, col as isize + 1)) * 0.25;
            [center, g, b]
        }
        1 => {
            // Green pixel
            let (r, b) = if is_row_even && !is_col_even {
                // Green on red row
                let r = (sample(row as isize, col as isize - 1)
                    + sample(row as isize, col as isize + 1)) * 0.5;
                let b = (sample(row as isize - 1, col as isize)
                    + sample(row as isize + 1, col as isize)) * 0.5;
                (r, b)
            } else {
                // Green on blue row
                let r = (sample(row as isize - 1, col as isize)
                    + sample(row as isize + 1, col as isize)) * 0.5;
                let b = (sample(row as isize, col as isize - 1)
                    + sample(row as isize, col as isize + 1)) * 0.5;
                (r, b)
            };
            [r, center, b]
        }
        2 => {
            // Blue pixel
            let g = (sample(row as isize - 1, col as isize)
                + sample(row as isize + 1, col as isize)
                + sample(row as isize, col as isize - 1)
                + sample(row as isize, col as isize + 1)) * 0.25;
            let r = (sample(row as isize - 1, col as isize - 1)
                + sample(row as isize - 1, col as isize + 1)
                + sample(row as isize + 1, col as isize - 1)
                + sample(row as isize + 1, col as isize + 1)) * 0.25;
            [r, g, center]
        }
        _ => [center, center, center],
    }
}

/// AHD demosaicing algorithm
///
/// Input: Bayer pattern image (height x width x 1) single-channel Bayer data
/// Output: Full RGB image (height x width x 3)
pub fn ahd_demosaic(
    bayer: &Array3<f64>,
    rgb_cam: &[[f32; 4]; 3],
) -> Result<Array3<f64>> {
    // CRITICAL: ndarray.dim() returns (rows, cols, channels) = (height, width, channels)
    let (height, width, channels) = bayer.dim();

    if channels != 1 {
        anyhow::bail!("Expected single-channel Bayer input, got {} channels", channels);
    }

    tracing::info!("Starting AHD demosaicing on {}x{} image", height, width);

    // Convert bayer to f32 for processing (preserves precision in dark areas)
    // Input is already single-channel, just convert to f32
    let mut image_f32 = Array3::<f32>::zeros((height, width, 1));

    // CRITICAL: Add small epsilon to prevent true zeros which cause color artifacts in demosaic
    // When Bayer values are exactly 0, interpolation can create color imbalance
    const EPSILON: f32 = 1e-6;

    for row in 0..height {
        for col in 0..width {
            // Single-channel Bayer: all pixels in channel 0
            // Color is determined by pixel position (row, col) via CFA pattern
            let val = (bayer[[row, col, 0]] as f32).max(EPSILON);
            image_f32[[row, col, 0]] = val;

            // Debug: Check a 4x4 region to see the Bayer pattern
            if (640..644).contains(&row) && (652..656).contains(&col) {
                let c = fc_rgb(row, col);
                let color_name = match c {
                    0 => "R",
                    1 => "G",
                    2 => "B",
                    _ => "?",
                };
                tracing::info!("Input Bayer at ({}, {}) [{}]: bayer={:.6}",
                    row, col, color_name, bayer[[row, col, 0]]);
            }
        }
    }

    // Initialize CIELab converter
    let cielab = CieLabConverter::new(rgb_cam);

    // Border interpolation (stabilize edges before tile processing)
    border_interpolate(&mut image_f32, width, height, 5);

    // Allocate output image (f32 for precision)
    let mut output = Array3::<f32>::zeros((height, width, 3));

    // Process image in tiles
    let mut top = 2;
    let mut tile_count = 0;
    while top < height.saturating_sub(5) {
        let mut left = 2;
        while left < width.saturating_sub(5) {
            tile_count += 1;
            process_tile(
                &image_f32,
                &mut output,
                &cielab,
                top,
                left,
                width,
                height,
            );
            left += TS - 6;
        }
        top += TS - 6;
    }
    tracing::info!("Processed {} tiles", tile_count);

    // Debug: Check multiple pixels to see the pattern
    // (640, 652) should be R, (640, 653) should be G, (641, 652) should be G, (641, 653) should be B
    tracing::info!("=== Demosaic Output Debug ===");
    for test_row in 640..642 {
        for test_col in 652..654 {
            let c = fc_rgb(test_row, test_col);
            let color_name = match c {
                0 => "R",
                1 => "G",
                2 => "B",
                _ => "?",
            };
            tracing::info!("Output at ({}, {}) [{}]: R={:.6}, G={:.6}, B={:.6}",
                test_row, test_col, color_name,
                output[[test_row, test_col, 0]],
                output[[test_row, test_col, 1]],
                output[[test_row, test_col, 2]]);
        }
    }

    // Debug: Check a larger region to see if there's a pattern
    tracing::info!("=== Checking 4x4 region at (640, 652) ===");
    for test_row in 640..644 {
        let mut row_str = String::new();
        for test_col in 652..656 {
            let c = fc_rgb(test_row, test_col);
            let color_name = match c {
                0 => "R",
                1 => "G",
                2 => "B",
                _ => "?",
            };
            let r = output[[test_row, test_col, 0]];
            let g = output[[test_row, test_col, 1]];
            let b = output[[test_row, test_col, 2]];
            row_str.push_str(&format!("[{}:R={:.3},G={:.3},B={:.3}] ", color_name, r, g, b));
        }
        tracing::info!("Row {}: {}", test_row, row_str);
    }

    // Border interpolation for output
    // CRITICAL: Must cover all pixels not processed by tiles
    // Tiles start at top=2 and process from (top+3) onwards, so rows [0,4] need border handling
    // Similarly for bottom and left/right edges
    for row in 0..height {
        for col in 0..width {
            if row <= 4 || row + 5 >= height || col <= 4 || col + 5 >= width {
                let rgb = demosaic_bilinear_pixel(&image_f32, row, col);
                output[[row, col, 0]] = rgb[0];
                output[[row, col, 1]] = rgb[1];
                output[[row, col, 2]] = rgb[2];
            }
        }
    }

    // Convert back to f64 (output is already in 0.0-1.0 range)
    let mut result = Array3::<f64>::zeros((height, width, 3));
    let mut zero_count = 0;
    let mut total_count = 0;

    for row in 0..height {
        for col in 0..width {
            for c in 0..3 {
                result[[row, col, c]] = output[[row, col, c]] as f64;
            }

            // Check if all channels are zero (unprocessed pixel)
            if output[[row, col, 0]] == 0.0 && output[[row, col, 1]] == 0.0 && output[[row, col, 2]] == 0.0 {
                zero_count += 1;
            }
            total_count += 1;
        }
    }

    if zero_count > 0 {
        tracing::debug!("Found {} zero pixels out of {} total ({:.2}%) - likely from very dark areas in fused Bayer",
            zero_count, total_count, 100.0 * zero_count as f64 / total_count as f64);
    }

    tracing::info!("AHD demosaicing complete");
    Ok(result)
}

fn process_tile(
    image: &Array3<f32>,
    output: &mut Array3<f32>,
    cielab: &CieLabConverter,
    top: usize,
    left: usize,
    width: usize,
    height: usize,
) {
    let tile_height = (top + TS).min(height - 2);
    let tile_width = (left + TS).min(width - 2);

    // Allocate tile buffers (f32 for precision)
    let mut rgb = vec![[[[0.0f32; 3]; TS]; TS]; 2]; // 2 directions (H and V)
    let mut lab = vec![[[[0i16; 3]; TS]; TS]; 2];
    let mut homo = vec![[[0u8; TS]; TS]; 2];

    // Step 1: Interpolate green horizontally and vertically
    interpolate_green(image, &mut rgb, top, left, tile_height, tile_width, width, height);

    // Step 2: Interpolate red and blue, convert to CIELab
    interpolate_rb_and_lab(image, &mut rgb, &mut lab, cielab, top, left, tile_height, tile_width, width);

    // Step 3: Build homogeneity maps
    build_homogeneity_maps(&lab, &mut homo, top, left, tile_height, tile_width);

    // Step 4: Combine most homogenous pixels
    combine_homogenous(output, &rgb, &homo, top, left, tile_height, tile_width, width);
}

fn interpolate_green(
    image: &Array3<f32>,
    rgb: &mut Vec<[[[f32; 3]; TS]; TS]>,
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
    const DARK_THRESHOLD: f32 = 0.02;  // 2% of full scale

    // FIRST: Copy original green values for all green pixels
    for row in top..tile_height.min(img_height) {
        let tr = row - top;
        if tr >= TS { break; }

        for col in left..tile_width.min(img_width) {
            let tc = col - left;
            if tc >= TS { break; }

            let c = fc_rgb(row, col);
            if c == 1 {  // Green pixel
                let orig_val = image[[row, col, 0]];
                rgb[0][tr][tc][1] = orig_val;
                rgb[1][tr][tc][1] = orig_val;
            }
        }
    }

    // SECOND: Interpolate green at R and B pixels in horizontal (d=0) and vertical (d=1) directions
    for row in top..tile_height.min(img_height) {
        let tr = row - top;
        if tr >= TS { break; }  // Don't overflow tile buffer

        // Start at R or B pixel, not G pixel
        // For even rows (0, 2, 4, ...), we want even columns (R pixels)
        // For odd rows (1, 3, 5, ...), we want odd columns (B pixels)
        let mut col = left + ((row ^ left) & 1);
        while col < tile_width.min(img_width) {
            let tc = col - left;
            if tc >= TS { break; }  // Don't overflow tile buffer

            // Get RGB channel index (0=R, 1=G, 2=B) for this pixel
            let c = fc_rgb(row, col);

            // Copy original color value to both directions
            let orig_val = image[[row, col, 0]];
            rgb[0][tr][tc][c] = orig_val;
            rgb[1][tr][tc][c] = orig_val;

            // Check if this is a dark area - use simple bilinear if so
            let is_dark = orig_val < DARK_THRESHOLD;

            // Bounds checking for horizontal interpolation
            if col > 1 && col + 2 < img_width {
                let val_h = if is_dark {
                    // Simple bilinear for dark areas
                    (image[[row, col - 1, 0]] + image[[row, col + 1, 0]]) * 0.5
                } else {
                    // Edge-directed interpolation for normal areas
                    ((image[[row, col - 1, 0]]
                        + image[[row, col, 0]]
                        + image[[row, col + 1, 0]]) * 2.0
                        - image[[row, col - 2, 0]]
                        - image[[row, col + 2, 0]])
                        * 0.25
                };
                rgb[0][tr][tc][1] = ulim(
                    val_h,
                    image[[row, col - 1, 0]],
                    image[[row, col + 1, 0]],
                );
            } else {
                // Fallback for border pixels
                rgb[0][tr][tc][1] = image[[row, col, 0]];
            }

            // Bounds checking for vertical interpolation
            if row > 1 && row + 2 < img_height {
                let val_v = if is_dark {
                    // Simple bilinear for dark areas
                    (image[[row - 1, col, 0]] + image[[row + 1, col, 0]]) * 0.5
                } else {
                    // Edge-directed interpolation for normal areas
                    ((image[[row - 1, col, 0]]
                        + image[[row, col, 0]]
                        + image[[row + 1, col, 0]]) * 2.0
                        - image[[row - 2, col, 0]]
                        - image[[row + 2, col, 0]])
                        * 0.25
                };
                rgb[1][tr][tc][1] = ulim(
                    val_v,
                    image[[row - 1, col, 0]],
                    image[[row + 1, col, 0]],
                );
            } else {
                // Fallback for border pixels
                rgb[1][tr][tc][1] = image[[row, col, 0]];
            }

            col += 2;
        }
    }
}

fn interpolate_rb_and_lab(
    image: &Array3<f32>,
    rgb: &mut Vec<[[[f32; 3]; TS]; TS]>,
    lab: &mut Vec<[[[i16; 3]; TS]; TS]>,
    cielab: &CieLabConverter,
    top: usize,
    left: usize,
    tile_height: usize,
    tile_width: usize,
    _width: usize,
) {
    let (img_height, img_width, _) = image.dim();

    // Threshold below which we use simple bilinear instead of edge-directed
    const DARK_THRESHOLD: f32 = 0.02;  // 2% of full scale

    // Interpolate red and blue for both directions
    for d in 0..2 {
        for row in (top + 1)..(tile_height.saturating_sub(1)).min(img_height - 1) {
            let tr = row - top;
            if tr >= TS - 1 { break; }  // Don't overflow tile buffer

            for col in (left + 1)..(tile_width.saturating_sub(1)).min(img_width - 1) {
                let tc = col - left;
                if tc >= TS - 1 { break; }  // Don't overflow tile buffer

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
                            rgb[d][tr][tc][1]
                                + ((image[[row, col - 1, 0]]
                                    + image[[row, col + 1, 0]]
                                    - rgb[d][tr][tc - 1][1]
                                    - rgb[d][tr][tc + 1][1])
                                    * 0.5)
                        };
                        rgb[d][tr][tc][2 - c2] = clip(val);
                    }

                    // Interpolate other color (vertical)
                    if row > 0 && row + 1 < img_height && tr > 0 && tr + 1 < TS {
                        let val = if is_dark {
                            // Simple bilinear for dark areas
                            (image[[row - 1, col, 0]] + image[[row + 1, col, 0]]) * 0.5
                        } else {
                            // Edge-directed interpolation
                            rgb[d][tr][tc][1]
                                + ((image[[row - 1, col, 0]]
                                    + image[[row + 1, col, 0]]
                                    - rgb[d][tr - 1][tc][1]
                                    - rgb[d][tr + 1][tc][1])
                                    * 0.5)
                        };
                        rgb[d][tr][tc][c2] = clip(val);
                    }
                } else {
                    // Red or Blue pixel - interpolate the other color
                    // If c=0 (Red), interpolate Blue (channel 2)
                    // If c=2 (Blue), interpolate Red (channel 0)
                    let other_color = 2 - c;  // 0->2, 2->0

                    if row > 0 && row + 1 < img_height && col > 0 && col + 1 < img_width
                        && tr > 0 && tr + 1 < TS && tc > 0 && tc + 1 < TS {
                        let val = if is_dark {
                            // Simple bilinear for dark areas
                            (image[[row - 1, col - 1, 0]]
                                + image[[row - 1, col + 1, 0]]
                                + image[[row + 1, col - 1, 0]]
                                + image[[row + 1, col + 1, 0]])
                                * 0.25
                        } else {
                            // Edge-directed interpolation
                            rgb[d][tr][tc][1]
                                + ((image[[row - 1, col - 1, 0]]
                                    + image[[row - 1, col + 1, 0]]
                                    + image[[row + 1, col - 1, 0]]
                                    + image[[row + 1, col + 1, 0]]
                                    - rgb[d][tr - 1][tc - 1][1]
                                    - rgb[d][tr - 1][tc + 1][1]
                                    - rgb[d][tr + 1][tc - 1][1]
                                    - rgb[d][tr + 1][tc + 1][1])
                                    * 0.25)
                        };
                        rgb[d][tr][tc][other_color] = clip(val);
                    }
                }

                // Copy original color
                let rgb_c = fc_rgb(row, col);
                rgb[d][tr][tc][rgb_c] = image[[row, col, 0]];

                // Convert to CIELab
                let rgb_pixel = [
                    rgb[d][tr][tc][0],
                    rgb[d][tr][tc][1],
                    rgb[d][tr][tc][2],
                ];
                let lab_pixel = cielab.convert(&rgb_pixel);
                lab[d][tr][tc] = lab_pixel;
            }
        }
    }
}

fn build_homogeneity_maps(
    lab: &Vec<[[[i16; 3]; TS]; TS]>,
    homo: &mut Vec<[[u8; TS]; TS]>,
    top: usize,
    left: usize,
    tile_height: usize,
    tile_width: usize,
) {
    const DIR: [i32; 4] = [-1, 1, -(TS as i32), TS as i32];

    for row in (top + 2)..(tile_height.saturating_sub(2)) {
        let tr = row - top;
        if !(2..TS - 2).contains(&tr) { continue; }  // Bounds check

        for col in (left + 2)..(tile_width.saturating_sub(2)) {
            let tc = col - left;
            if !(2..TS - 2).contains(&tc) { continue; }  // Bounds check

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

                    ldiff[d][i] = (lab[d][tr][tc][0] - lab[d][nr][nc][0]).unsigned_abs() as u32;
                    abdiff[d][i] = ((lab[d][tr][tc][1] - lab[d][nr][nc][1]).pow(2)
                        + (lab[d][tr][tc][2] - lab[d][nr][nc][2]).pow(2)) as u32;
                }
            }

            // Calculate epsilon thresholds
            let leps = ldiff[0][0].max(ldiff[0][1]).min(ldiff[1][2].max(ldiff[1][3]));
            let abeps = abdiff[0][0].max(abdiff[0][1]).min(abdiff[1][2].max(abdiff[1][3]));

            // Build homogeneity map
            for d in 0..2 {
                for i in 0..4 {
                    if ldiff[d][i] <= leps && abdiff[d][i] <= abeps {
                        homo[d][tr][tc] += 1;
                    }
                }
            }
        }
    }
}

fn combine_homogenous(
    output: &mut Array3<f32>,
    rgb: &Vec<[[[f32; 3]; TS]; TS]>,
    homo: &Vec<[[u8; TS]; TS]>,
    top: usize,
    left: usize,
    tile_height: usize,
    tile_width: usize,
    _width: usize,
) {
    let (img_height, img_width, _) = output.dim();

    for row in (top + 3)..(tile_height.saturating_sub(3)).min(img_height) {
        let tr = row - top;
        if !(3..TS - 3).contains(&tr) { continue; }  // Bounds check

        for col in (left + 3)..(tile_width.saturating_sub(3)).min(img_width) {
            let tc = col - left;
            if !(3..TS - 3).contains(&tc) { continue; }  // Bounds check

            // Sum homogeneity in 3x3 neighborhood
            let mut hm = [0u32; 2];
            for d in 0..2 {
                for i in (tr.saturating_sub(1))..=(tr + 1).min(TS - 1) {
                    for j in (tc.saturating_sub(1))..=(tc + 1).min(TS - 1) {
                        hm[d] += homo[d][i][j] as u32;
                    }
                }
            }

            // Choose direction or average
            if hm[0] != hm[1] {
                let best_d = if hm[1] > hm[0] { 1 } else { 0 };
                for c in 0..3 {
                    output[[row, col, c]] = rgb[best_d][tr][tc][c];
                }
            } else {
                // Average both directions
                for c in 0..3 {
                    output[[row, col, c]] = (rgb[0][tr][tc][c] + rgb[1][tr][tc][c]) * 0.5;
                }
            }
        }
    }
}
