use image::RgbImage;

pub fn psnr_rgb_u8(a: &RgbImage, b: &RgbImage) -> Option<f64> {
    if a.width() != b.width() || a.height() != b.height() {
        return None;
    }
    let mut mse = 0.0f64;
    let mut count = 0u64;
    for (pa, pb) in a.pixels().zip(b.pixels()) {
        for c in 0..3 {
            let diff = pa[c] as f64 - pb[c] as f64;
            mse += diff * diff;
            count += 1;
        }
    }
    if count == 0 {
        return None;
    }
    mse /= count as f64;
    if mse == 0.0 {
        return Some(f64::INFINITY);
    }
    let max_i = 255.0;
    let psnr = 10.0 * (max_i * max_i / mse).log10();
    Some(psnr)
}

pub fn ssim_luma_u8(a: &RgbImage, b: &RgbImage) -> Option<f64> {
    if a.width() != b.width() || a.height() != b.height() {
        return None;
    }
    let mut mean_x = 0.0f64;
    let mut mean_y = 0.0f64;
    let mut count = 0u64;

    for (pa, pb) in a.pixels().zip(b.pixels()) {
        let luma_a = luma_from_rgb(pa[0], pa[1], pa[2]);
        let luma_b = luma_from_rgb(pb[0], pb[1], pb[2]);
        mean_x += luma_a;
        mean_y += luma_b;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    mean_x /= count as f64;
    mean_y /= count as f64;

    let mut var_x = 0.0f64;
    let mut var_y = 0.0f64;
    let mut cov_xy = 0.0f64;

    for (pa, pb) in a.pixels().zip(b.pixels()) {
        let luma_a = luma_from_rgb(pa[0], pa[1], pa[2]);
        let luma_b = luma_from_rgb(pb[0], pb[1], pb[2]);
        let dx = luma_a - mean_x;
        let dy = luma_b - mean_y;
        var_x += dx * dx;
        var_y += dy * dy;
        cov_xy += dx * dy;
    }
    var_x /= count as f64;
    var_y /= count as f64;
    cov_xy /= count as f64;

    let c1 = (0.01f64 * 255.0).powi(2);
    let c2 = (0.03f64 * 255.0).powi(2);

    let numerator = (2.0 * mean_x * mean_y + c1) * (2.0 * cov_xy + c2);
    let denominator = (mean_x * mean_x + mean_y * mean_y + c1) * (var_x + var_y + c2);
    if denominator == 0.0 {
        return None;
    }
    Some(numerator / denominator)
}

#[inline]
fn luma_from_rgb(r: u8, g: u8, b: u8) -> f64 {
    0.2126 * r as f64 + 0.7152 * g as f64 + 0.0722 * b as f64
}
