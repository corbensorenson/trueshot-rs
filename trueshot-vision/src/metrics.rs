use image::{DynamicImage, GenericImageView};

/// Compute 256-bin histogram for RGB channels
pub fn compute_histogram(img: &DynamicImage) -> ([u32; 256], [u32; 256], [u32; 256]) {
    let mut r_hist = [0u32; 256];
    let mut g_hist = [0u32; 256];
    let mut b_hist = [0u32; 256];

    for pixel in img.pixels() {
        let rgba = pixel.2;
        r_hist[rgba[0] as usize] += 1;
        g_hist[rgba[1] as usize] += 1;
        b_hist[rgba[2] as usize] += 1;
    }

    (r_hist, g_hist, b_hist)
}

/// Compute Laplacian Variance (Blur Detection)
/// Higher score = sharper image. < 100 usually means blurry.
pub fn compute_sharpness(img: &DynamicImage) -> f32 {
    let gray = img.to_luma8();
    let (width, height) = gray.dimensions();
    if width < 3 || height < 3 { return 0.0; }

    let mut laplacian_sum = 0.0;
    let mut laplacian_sq_sum = 0.0;
    let count = ((width - 2) * (height - 2)) as f32;

    // 3x3 Laplacian Kernel
    //  0  1  0
    //  1 -4  1
    //  0  1  0
    
    for y in 1..height-1 {
        for x in 1..width-1 {
            let center = gray.get_pixel(x, y)[0] as i32;
            let top = gray.get_pixel(x, y-1)[0] as i32;
            let bottom = gray.get_pixel(x, y+1)[0] as i32;
            let left = gray.get_pixel(x-1, y)[0] as i32;
            let right = gray.get_pixel(x+1, y)[0] as i32;

            let val = top + bottom + left + right - (4 * center);
            let val_f = val as f32;
            
            laplacian_sum += val_f;
            laplacian_sq_sum += val_f * val_f;
        }
    }

    let mean = laplacian_sum / count;
    
    
    (laplacian_sq_sum / count) - (mean * mean)
}
