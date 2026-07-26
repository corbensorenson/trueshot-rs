use image::GrayImage;

/// Simple Static Background Subtraction
/// Assumes tripod + controlled lighting.

pub fn remove_static_background(img: &GrayImage, bg: &GrayImage, threshold: u8) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let mut mask = vec![0u8; (w * h) as usize];
    
    for (i, (p_img, p_bg)) in img.pixels().zip(bg.pixels()).enumerate() {
        let diff = (p_img[0] as i16 - p_bg[0] as i16).abs();
        if diff > threshold as i16 {
            mask[i] = 255; // Foreground
        } else {
            mask[i] = 0; // Background
        }
    }
    mask
}
