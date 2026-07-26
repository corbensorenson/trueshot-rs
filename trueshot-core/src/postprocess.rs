//! Post-processing: white balance, tone mapping, and sharpening.

use anyhow::Result;
use ndarray::{Array2, Array3};
use rayon::prelude::*;

/// Post-process native `f32` linear RGB without expanding through `f64`.
///
/// Exposure is estimated from a log-luminance histogram so a single hot pixel
/// cannot darken an entire capture. Tone mapping is luminance-preserving,
/// sharpening is noise-gated, and all display conversion stays bounded to one
/// `f32` RGB working buffer plus the final `u8` image.
pub fn postprocess_f32(linear_rgb: &Array3<f32>) -> Result<Array3<u8>> {
    let (height, width, channels) = linear_rgb.dim();
    if channels != 3 {
        anyhow::bail!(
            "Expected three-channel linear RGB, got {} channels",
            channels
        );
    }
    if height == 0 || width == 0 {
        anyhow::bail!("Cannot post-process an empty image");
    }

    let exposure = robust_display_exposure(linear_rgb);
    tracing::info!("Native display exposure: {:.4}x", exposure);
    let mut display = vec![0.0f32; height * width * 3];
    display
        .par_chunks_mut(width * 3)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..width {
                let source = [
                    linear_rgb[[y, x, 0]].max(0.0) * exposure,
                    linear_rgb[[y, x, 1]].max(0.0) * exposure,
                    linear_rgb[[y, x, 2]].max(0.0) * exposure,
                ];
                let luminance = linear_luminance(source);
                let mapped_luminance = filmic_luminance(luminance);
                let scale = if luminance > 1e-8 {
                    mapped_luminance / luminance
                } else {
                    0.0
                };
                let mut mapped = [source[0] * scale, source[1] * scale, source[2] * scale];

                // Chroma below the estimated sensor floor is unreliable. The
                // smooth, capped blend avoids the blanket edge desaturation
                // performed by the legacy path.
                if mapped_luminance < 0.035 {
                    let damping = ((0.035 - mapped_luminance) / 0.035).clamp(0.0, 1.0) * 0.45;
                    for channel in &mut mapped {
                        *channel = *channel * (1.0 - damping) + mapped_luminance * damping;
                    }
                }

                let offset = x * 3;
                row[offset] = srgb_encode(mapped[0].clamp(0.0, 1.0));
                row[offset + 1] = srgb_encode(mapped[1].clamp(0.0, 1.0));
                row[offset + 2] = srgb_encode(mapped[2].clamp(0.0, 1.0));
            }
        });

    let mut output = vec![0u8; height * width * 3];
    output
        .par_chunks_mut(width * 3)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..width {
                let index = (y * width + x) * 3;
                let center = [display[index], display[index + 1], display[index + 2]];
                let center_luma = display_luminance(center);
                let mut neighbor_luma = 0.0;
                let mut neighbor_count = 0.0;
                if x > 0 {
                    neighbor_luma += display_luminance_at(&display, width, x - 1, y);
                    neighbor_count += 1.0;
                }
                if x + 1 < width {
                    neighbor_luma += display_luminance_at(&display, width, x + 1, y);
                    neighbor_count += 1.0;
                }
                if y > 0 {
                    neighbor_luma += display_luminance_at(&display, width, x, y - 1);
                    neighbor_count += 1.0;
                }
                if y + 1 < height {
                    neighbor_luma += display_luminance_at(&display, width, x, y + 1);
                    neighbor_count += 1.0;
                }
                let blurred = if neighbor_count > 0.0 {
                    neighbor_luma / neighbor_count
                } else {
                    center_luma
                };
                let detail = center_luma - blurred;
                let noise_gate = smoothstep_f32(0.002, 0.012, detail.abs());
                let sharpened_detail = (detail * 0.38 * noise_gate).clamp(-0.035, 0.035);
                for channel in 0..3 {
                    row[x * 3 + channel] = ((center[channel] + sharpened_detail).clamp(0.0, 1.0)
                        * 255.0)
                        .round() as u8;
                }
            }
        });

    Array3::from_shape_vec((height, width, 3), output)
        .map_err(|error| anyhow::anyhow!("Unable to shape display output: {error}"))
}

fn robust_display_exposure(rgb: &Array3<f32>) -> f32 {
    const BINS: usize = 2048;
    const LOG_MIN: f32 = -16.0;
    const LOG_MAX: f32 = 6.0;
    let mut histogram = [0u64; BINS];
    let (height, width, _) = rgb.dim();
    let sample_step = ((height * width) / 2_000_000).max(1);
    let mut samples = 0u64;
    for index in (0..height * width).step_by(sample_step) {
        let y = index / width;
        let x = index % width;
        let luminance = linear_luminance([
            rgb[[y, x, 0]].max(0.0),
            rgb[[y, x, 1]].max(0.0),
            rgb[[y, x, 2]].max(0.0),
        ]);
        if luminance <= 1e-7 {
            continue;
        }
        let normalized = ((luminance.log2() - LOG_MIN) / (LOG_MAX - LOG_MIN)).clamp(0.0, 1.0);
        let bin = (normalized * (BINS - 1) as f32).round() as usize;
        histogram[bin] += 1;
        samples += 1;
    }
    if samples == 0 {
        return 1.0;
    }
    let percentile = |fraction: f32| {
        let target = (samples as f32 * fraction).ceil() as u64;
        let mut cumulative = 0u64;
        for (index, count) in histogram.iter().enumerate() {
            cumulative += count;
            if cumulative >= target {
                let normalized = index as f32 / (BINS - 1) as f32;
                return 2.0f32.powf(LOG_MIN + normalized * (LOG_MAX - LOG_MIN));
            }
        }
        1.0
    };
    let median = percentile(0.50).max(1e-5);
    let highlight = percentile(0.995).max(median);
    let middle_gray_exposure = 0.18 / median;
    let highlight_guard = 1.5 / highlight;
    middle_gray_exposure.min(highlight_guard).clamp(0.05, 32.0)
}

#[inline]
fn linear_luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

#[inline]
fn display_luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

#[inline]
fn display_luminance_at(rgb: &[f32], width: usize, x: usize, y: usize) -> f32 {
    let index = (y * width + x) * 3;
    display_luminance([rgb[index], rgb[index + 1], rgb[index + 2]])
}

#[inline]
fn filmic_luminance(value: f32) -> f32 {
    let numerator = value * (2.51 * value + 0.03);
    let denominator = value * (2.43 * value + 0.59) + 0.14;
    if denominator > 0.0 {
        (numerator / denominator).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[inline]
fn srgb_encode(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

#[inline]
fn smoothstep_f32(edge0: f32, edge1: f32, value: f32) -> f32 {
    let normalized = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    normalized * normalized * (3.0 - 2.0 * normalized)
}

/// Post-process linear RGB to display-ready u8
pub fn postprocess(linear_rgb: &Array3<f64>) -> Result<Array3<u8>> {
    tracing::debug!("Post-processing RGB image");

    // Try GPU first if available
    #[cfg(feature = "gpu")]
    {
        use crate::gpu::{get_gpu_context, gpu_postprocess};

        if let Some(gpu_ctx) = get_gpu_context() {
            match gpu_postprocess(&gpu_ctx, linear_rgb) {
                Ok(Some(result)) => {
                    tracing::info!("Postprocessing completed on GPU");
                    return Ok(result);
                }
                Ok(None) => {
                    tracing::debug!("GPU returned None, falling back to CPU");
                }
                Err(e) => {
                    tracing::warn!("GPU postprocessing failed: {}, falling back to CPU", e);
                }
            }
        }
    }

    // CPU fallback
    tracing::debug!("Postprocessing on CPU");

    // 1. Skip white balance - use camera WB from demosaic
    // (Gray world assumption doesn't work well for bone specimens)

    // 2. Tone mapping
    let toned = tone_map(linear_rgb)?;

    // 3. Sharpening
    let sharpened = sharpen(&toned)?;

    // 4. Convert to u8
    let output = to_u8(&sharpened)?;

    Ok(output)
}

/// Tone mapping via Reinhard global operator with exposure compensation
fn tone_map(rgb: &Array3<f64>) -> Result<Array3<f64>> {
    let (height, width, channels) = rgb.dim();

    // BISECT DEBUG: Check input to tone mapping
    let mut r_sum = 0.0;
    let mut g_sum = 0.0;
    let mut b_sum = 0.0;
    for y in 0..height {
        for x in 0..width {
            r_sum += rgb[[y, x, 0]];
            g_sum += rgb[[y, x, 1]];
            b_sum += rgb[[y, x, 2]];
        }
    }
    let pixel_count = (height * width) as f64;
    tracing::info!("=== BISECT STAGE 4: Input to tone_map ===");
    tracing::info!(
        "R avg={:.6}, G avg={:.6}, B avg={:.6}",
        r_sum / pixel_count,
        g_sum / pixel_count,
        b_sum / pixel_count
    );
    tracing::info!(
        "Ratio: R/G={:.3}, B/G={:.3}, R/B={:.3}",
        (r_sum / pixel_count) / (g_sum / pixel_count),
        (b_sum / pixel_count) / (g_sum / pixel_count),
        (r_sum / pixel_count) / (b_sum / pixel_count)
    );

    // Find max value for auto-exposure
    let max_val = rgb.iter().copied().fold(0.0f64, f64::max);
    tracing::info!("Tone mapping: input max = {:.6}", max_val);

    // Apply exposure compensation to bring max to ~0.6 (reduced from 0.9 to avoid overexposure)
    let exposure_compensation = if max_val > 0.01 { 0.6 / max_val } else { 1.0 };
    tracing::info!(
        "Applying exposure compensation: {:.2}x",
        exposure_compensation
    );

    let mut toned = Array3::<f64>::zeros((height, width, channels));

    for y in 0..height {
        for x in 0..width {
            for c in 0..channels {
                let value = rgb[[y, x, c]] * exposure_compensation;
                // Simple clamp instead of Reinhard for now
                toned[[y, x, c]] = value.min(1.0);
            }
        }
    }

    // BISECT DEBUG: Check after exposure compensation (before gamma)
    let mut r_sum = 0.0;
    let mut g_sum = 0.0;
    let mut b_sum = 0.0;
    for y in 0..height {
        for x in 0..width {
            r_sum += toned[[y, x, 0]];
            g_sum += toned[[y, x, 1]];
            b_sum += toned[[y, x, 2]];
        }
    }
    tracing::info!("=== BISECT STAGE 5: After exposure comp (before gamma) ===");
    tracing::info!(
        "R avg={:.6}, G avg={:.6}, B avg={:.6}",
        r_sum / pixel_count,
        g_sum / pixel_count,
        b_sum / pixel_count
    );
    tracing::info!(
        "Ratio: R/G={:.3}, B/G={:.3}, R/B={:.3}",
        (r_sum / pixel_count) / (g_sum / pixel_count),
        (b_sum / pixel_count) / (g_sum / pixel_count),
        (r_sum / pixel_count) / (b_sum / pixel_count)
    );

    // Apply chroma noise reduction in dark areas (before gamma)
    // In very dark areas, noise dominates and creates color artifacts (purple/blue tint)
    // Reduce chroma (color) while preserving luma (brightness)
    for y in 0..height {
        for x in 0..width {
            let r = toned[[y, x, 0]];
            let g = toned[[y, x, 1]];
            let b = toned[[y, x, 2]];

            // Compute luminance (Y in YCbCr)
            let luma = 0.299 * r + 0.587 * g + 0.114 * b;

            // In dark areas (luma < 0.15), blend towards grayscale
            if luma < 0.15 {
                let strength = (0.15 - luma) / 0.15; // 0 at luma=0.15, 1 at luma=0
                let strength = strength.min(1.0).max(0.0);

                // Blend towards grayscale (preserves luma, removes chroma)
                toned[[y, x, 0]] = r * (1.0 - strength) + luma * strength;
                toned[[y, x, 1]] = g * (1.0 - strength) + luma * strength;
                toned[[y, x, 2]] = b * (1.0 - strength) + luma * strength;
            }
        }
    }

    // Apply sRGB gamma curve (like original pixelcollapse)
    // This is critical for proper display - raw data is linear but displays expect gamma ~2.2
    for y in 0..height {
        for x in 0..width {
            for c in 0..channels {
                let v = toned[[y, x, c]];
                toned[[y, x, c]] = if v <= 0.0031308 {
                    12.92 * v // Linear portion for dark values
                } else {
                    1.055 * v.powf(1.0 / 2.4) - 0.055 // Power curve for bright values
                };
            }
        }
    }

    // CRITICAL: Apply edge desaturation AFTER gamma to fix color fringing
    // The gamma curve amplifies small color differences, so we must desaturate in gamma space
    fix_edge_color_fringing_post_gamma(&mut toned)?;

    // BISECT DEBUG: Check after gamma
    let mut r_sum = 0.0;
    let mut g_sum = 0.0;
    let mut b_sum = 0.0;
    for y in 0..height {
        for x in 0..width {
            r_sum += toned[[y, x, 0]];
            g_sum += toned[[y, x, 1]];
            b_sum += toned[[y, x, 2]];
        }
    }
    tracing::info!("=== BISECT STAGE 6: After gamma ===");
    tracing::info!(
        "R avg={:.6}, G avg={:.6}, B avg={:.6}",
        r_sum / pixel_count,
        g_sum / pixel_count,
        b_sum / pixel_count
    );
    tracing::info!(
        "Ratio: R/G={:.3}, B/G={:.3}, R/B={:.3}",
        (r_sum / pixel_count) / (g_sum / pixel_count),
        (b_sum / pixel_count) / (g_sum / pixel_count),
        (r_sum / pixel_count) / (b_sum / pixel_count)
    );

    Ok(toned)
}

/// Sharpening via unsharp mask
fn sharpen(rgb: &Array3<f64>) -> Result<Array3<f64>> {
    let (height, width, channels) = rgb.dim();

    // Fixed sharpening amount
    let amount = 0.5;
    let mut sharpened = rgb.clone();

    for y in 1..height - 1 {
        for x in 1..width - 1 {
            for c in 0..channels {
                // Simple unsharp mask: original + amount * (original - blurred)
                let center = rgb[[y, x, c]];
                let blurred = (rgb[[y - 1, x, c]]
                    + rgb[[y + 1, x, c]]
                    + rgb[[y, x - 1, c]]
                    + rgb[[y, x + 1, c]])
                    / 4.0;

                let detail = center - blurred;
                sharpened[[y, x, c]] = (center + amount * detail).max(0.0).min(1.0);
            }
        }
    }

    Ok(sharpened)
}

/// Convert f64 [0,1] to u8 [0,255]
fn to_u8(rgb: &Array3<f64>) -> Result<Array3<u8>> {
    let (height, width, channels) = rgb.dim();
    let mut output = Array3::<u8>::zeros((height, width, channels));

    for y in 0..height {
        for x in 0..width {
            for c in 0..channels {
                let value = (rgb[[y, x, c]] * 255.0).round().max(0.0).min(255.0) as u8;
                output[[y, x, c]] = value;
            }
        }
    }

    // Debug: Check final u8 channel means
    let mean_r = output
        .slice(ndarray::s![.., .., 0])
        .iter()
        .map(|&v| v as f64)
        .sum::<f64>()
        / (height * width) as f64;
    let mean_g = output
        .slice(ndarray::s![.., .., 1])
        .iter()
        .map(|&v| v as f64)
        .sum::<f64>()
        / (height * width) as f64;
    let mean_b = output
        .slice(ndarray::s![.., .., 2])
        .iter()
        .map(|&v| v as f64)
        .sum::<f64>()
        / (height * width) as f64;
    tracing::info!(
        "Final u8 output: mean R={:.2}, G={:.2}, B={:.2}",
        mean_r,
        mean_g,
        mean_b
    );

    Ok(output)
}

/// Fix edge color fringing after gamma correction
///
/// Demosaic algorithms create color artifacts at high-contrast edges.
/// These artifacts are amplified by the gamma curve, so we must apply
/// desaturation AFTER gamma, not before.
fn fix_edge_color_fringing_post_gamma(rgb: &mut Array3<f64>) -> Result<()> {
    let (height, width, _) = rgb.dim();

    tracing::info!("Fixing edge color fringing (post-gamma)...");

    // First pass: compute edge map (gradient magnitude) in gamma space
    let mut edge_map = Array2::<f64>::zeros((height, width));
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            // Compute luminance gradient using Sobel operator
            let _luma_center =
                0.299 * rgb[[y, x, 0]] + 0.587 * rgb[[y, x, 1]] + 0.114 * rgb[[y, x, 2]];
            let luma_left = 0.299 * rgb[[y, x - 1, 0]]
                + 0.587 * rgb[[y, x - 1, 1]]
                + 0.114 * rgb[[y, x - 1, 2]];
            let luma_right = 0.299 * rgb[[y, x + 1, 0]]
                + 0.587 * rgb[[y, x + 1, 1]]
                + 0.114 * rgb[[y, x + 1, 2]];
            let luma_up = 0.299 * rgb[[y - 1, x, 0]]
                + 0.587 * rgb[[y - 1, x, 1]]
                + 0.114 * rgb[[y - 1, x, 2]];
            let luma_down = 0.299 * rgb[[y + 1, x, 0]]
                + 0.587 * rgb[[y + 1, x, 1]]
                + 0.114 * rgb[[y + 1, x, 2]];

            let grad_x = (luma_right - luma_left).abs();
            let grad_y = (luma_down - luma_up).abs();
            let gradient = (grad_x * grad_x + grad_y * grad_y).sqrt();

            edge_map[[y, x]] = gradient;
        }
    }

    // Debug: Sample edge gradients
    let mut edge_samples = Vec::new();
    for y in (100..height.min(1500)).step_by(100) {
        for x in (200..width.min(600)).step_by(50) {
            if y > 0 && y < height - 1 && x > 0 && x < width - 1 {
                edge_samples.push(edge_map[[y, x]]);
            }
        }
    }
    if !edge_samples.is_empty() {
        edge_samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = edge_samples[edge_samples.len() / 2];
        let p90 = edge_samples[(edge_samples.len() as f64 * 0.9) as usize];
        let max = edge_samples.iter().copied().fold(0.0, f64::max);
        tracing::info!(
            "Edge gradient stats (post-gamma): median={:.4}, p90={:.4}, max={:.4}",
            median,
            p90,
            max
        );
    }

    // Second pass: desaturate edges
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let edge_strength = edge_map[[y, x]];

            // Apply desaturation to edges with gradient > 0.003 (extremely low threshold)
            // Based on stats: median=0.0004, p90=0.0053, max=0.1979
            if edge_strength > 0.003 {
                let r = rgb[[y, x, 0]];
                let g = rgb[[y, x, 1]];
                let b = rgb[[y, x, 2]];

                let luma = 0.299 * r + 0.587 * g + 0.114 * b;

                // EXTREMELY aggressive desaturation: 100% for gradients > 0.03
                // This catches all significant edges (p90 is 0.0053, max is 0.1979)
                let desat_strength = if edge_strength > 0.03 {
                    1.0 // Complete desaturation for strong edges
                } else {
                    // Linear ramp from 0.003 to 0.03
                    ((edge_strength - 0.003) / (0.03 - 0.003)).min(1.0)
                };

                // Blend towards grayscale
                rgb[[y, x, 0]] = r * (1.0 - desat_strength) + luma * desat_strength;
                rgb[[y, x, 1]] = g * (1.0 - desat_strength) + luma * desat_strength;
                rgb[[y, x, 2]] = b * (1.0 - desat_strength) + luma * desat_strength;
            }
        }
    }

    tracing::info!("Edge color fringing correction complete (post-gamma)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_u8() {
        let rgb = Array3::from_shape_fn((2, 2, 3), |(_, _, _)| 0.5);
        let u8_rgb = to_u8(&rgb).unwrap();

        assert_eq!(u8_rgb[[0, 0, 0]], 128);
        assert_eq!(u8_rgb[[1, 1, 2]], 128);
    }

    #[test]
    fn test_tone_map() {
        let rgb = Array3::from_shape_fn((2, 2, 3), |(_, _, _)| 0.5);
        let toned = tone_map(&rgb).unwrap();

        assert!(toned
            .iter()
            .all(|value| value.is_finite() && (0.0..=1.0).contains(value)));
        assert_eq!(toned[[0, 0, 0]], toned[[0, 0, 1]]);
    }

    #[test]
    fn f32_exposure_is_robust_to_one_hot_pixel() {
        let mut rgb = Array3::<f32>::from_elem((32, 32, 3), 0.18);
        rgb[[0, 0, 0]] = 100.0;
        rgb[[0, 0, 1]] = 100.0;
        rgb[[0, 0, 2]] = 100.0;
        let output = postprocess_f32(&rgb).unwrap();
        assert!(output[[16, 16, 1]] > 80);
    }
}
