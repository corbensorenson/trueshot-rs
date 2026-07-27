//! Dependency-free native focus-evidence kernels.
//!
//! Keeping this module independent of the rest of TrueShot permits direct
//! `rustc -O` Apple Silicon qualification without linking the full product.

pub(crate) fn compute_focus_metric(
    green: &[f32],
    valid: &[bool],
    metric: &mut [f32],
    width: usize,
    height: usize,
    apple_simd: bool,
) {
    if width < 3 || height < 3 {
        return;
    }
    let pixels = width
        .checked_mul(height)
        .expect("focus evidence dimensions overflow");
    assert!(
        green.len() >= pixels && valid.len() >= pixels && metric.len() >= pixels,
        "focus evidence buffers are smaller than the declared dimensions"
    );
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    if apple_simd {
        // SAFETY: NEON is mandatory on AArch64. The implementation bounds
        // every vector load to one interior row and handles the tail scalar.
        unsafe {
            compute_focus_metric_apple_neon(green, valid, metric, width, height);
        }
        return;
    }
    compute_focus_metric_scalar(green, valid, metric, width, height);
}

pub(crate) fn active_focus_kernel(apple_simd: bool) -> &'static str {
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    if apple_simd {
        return "apple-neon-f32-v1";
    }
    "portable-scalar-f32-v1"
}

#[inline]
fn focus_response(center: f32, left: f32, right: f32, up: f32, down: f32) -> f32 {
    let horizontal = (left - 2.0 * center + right).abs();
    let vertical = (up - 2.0 * center + down).abs();
    let gradient_x = (right - left).abs() * 0.5;
    let gradient_y = (down - up).abs() * 0.5;
    let response = horizontal + vertical + 0.35 * (gradient_x + gradient_y);
    response / (0.0015 + 0.02 * center.max(0.0).sqrt())
}

#[inline(never)]
pub(crate) fn compute_focus_metric_scalar(
    green: &[f32],
    valid: &[bool],
    metric: &mut [f32],
    width: usize,
    height: usize,
) {
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let index = y * width + x;
            if !valid[index] {
                continue;
            }
            metric[index] = focus_response(
                green[index],
                green[index - 1],
                green[index + 1],
                green[index - width],
                green[index + width],
            );
        }
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
#[target_feature(enable = "neon")]
pub(crate) unsafe fn compute_focus_metric_apple_neon(
    green: &[f32],
    valid: &[bool],
    metric: &mut [f32],
    width: usize,
    height: usize,
) {
    use std::arch::aarch64::{
        vabsq_f32, vaddq_f32, vdivq_f32, vdupq_n_f32, vld1q_f32, vmaxq_f32, vmulq_f32, vsqrtq_f32,
        vst1q_f32, vsubq_f32,
    };

    let two = vdupq_n_f32(2.0);
    let half = vdupq_n_f32(0.5);
    let gradient_weight = vdupq_n_f32(0.35);
    let noise_floor = vdupq_n_f32(0.0015);
    let shot_scale = vdupq_n_f32(0.02);
    let zero = vdupq_n_f32(0.0);
    for y in 1..height - 1 {
        let mut x = 1usize;
        while x + 3 < width - 1 {
            let index = y * width + x;
            // SAFETY: x spans four interior pixels; all five row pointers have
            // at least four contiguous elements from their stated offsets.
            let center = unsafe { vld1q_f32(green.as_ptr().add(index)) };
            let left = unsafe { vld1q_f32(green.as_ptr().add(index - 1)) };
            let right = unsafe { vld1q_f32(green.as_ptr().add(index + 1)) };
            let up = unsafe { vld1q_f32(green.as_ptr().add(index - width)) };
            let down = unsafe { vld1q_f32(green.as_ptr().add(index + width)) };
            let twice_center = vmulq_f32(center, two);
            let horizontal = vabsq_f32(vaddq_f32(vsubq_f32(left, twice_center), right));
            let vertical = vabsq_f32(vaddq_f32(vsubq_f32(up, twice_center), down));
            let gradient_x = vmulq_f32(vabsq_f32(vsubq_f32(right, left)), half);
            let gradient_y = vmulq_f32(vabsq_f32(vsubq_f32(down, up)), half);
            let response = vaddq_f32(
                vaddq_f32(horizontal, vertical),
                vmulq_f32(vaddq_f32(gradient_x, gradient_y), gradient_weight),
            );
            let denominator = vaddq_f32(
                noise_floor,
                vmulq_f32(vsqrtq_f32(vmaxq_f32(center, zero)), shot_scale),
            );
            let value = vdivq_f32(response, denominator);
            if valid[index] && valid[index + 1] && valid[index + 2] && valid[index + 3] {
                // SAFETY: the output has the same validated dimensions.
                unsafe { vst1q_f32(metric.as_mut_ptr().add(index), value) };
            } else {
                let mut lanes = [0.0f32; 4];
                unsafe { vst1q_f32(lanes.as_mut_ptr(), value) };
                for lane in 0..4 {
                    if valid[index + lane] {
                        metric[index + lane] = lanes[lane];
                    }
                }
            }
            x += 4;
        }
        for x in x..width - 1 {
            let index = y * width + x;
            if valid[index] {
                metric[index] = focus_response(
                    green[index],
                    green[index - 1],
                    green[index + 1],
                    green[index - width],
                    green[index + width],
                );
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn sorted_trimmed_focus_mean_at(
    metric: &[f32],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> f32 {
    let x0 = x.saturating_sub(1);
    let y0 = y.saturating_sub(1);
    let x1 = (x + 1).min(width - 1);
    let y1 = (y + 1).min(height - 1);
    let mut values = [0.0f32; 9];
    let mut count = 0usize;
    for yy in y0..=y1 {
        for xx in x0..=x1 {
            values[count] = metric[yy * width + xx];
            count += 1;
        }
    }
    values[..count].sort_unstable_by(f32::total_cmp);
    let trim = usize::from(count >= 7);
    let kept = &values[trim..count - trim];
    kept.iter().sum::<f32>() / kept.len().max(1) as f32
}

pub(crate) fn compute_trimmed_focus_mean(
    metric: &[f32],
    output: &mut [f32],
    width: usize,
    height: usize,
) {
    debug_assert_eq!(metric.len(), width.saturating_mul(height));
    debug_assert_eq!(output.len(), metric.len());
    for y in 0..height {
        let y0 = y.saturating_sub(1);
        let y1 = (y + 1).min(height - 1);
        for x in 0..width {
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(width - 1);
            let mut sum = 0.0f32;
            let mut minimum = f32::INFINITY;
            let mut maximum = f32::NEG_INFINITY;
            let mut count = 0usize;
            for yy in y0..=y1 {
                for xx in x0..=x1 {
                    let value = metric[yy * width + xx];
                    sum += value;
                    minimum = minimum.min(value);
                    maximum = maximum.max(value);
                    count += 1;
                }
            }
            if count >= 7 {
                sum -= minimum + maximum;
                count -= 2;
            }
            output[y * width + x] = sum / count.max(1) as f32;
        }
    }
}
