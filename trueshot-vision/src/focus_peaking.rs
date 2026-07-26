use image::DynamicImage;
/// Focus Peaking (Sobel)
/// Highlights high-frequency edges in green.

pub fn compute_focus_peaking(img: &DynamicImage, threshold: u8) -> DynamicImage {
    let gray = img.to_luma8();
    let (w, h) = gray.dimensions();
    let mut output = img.to_rgba8();

    for y in 1..h-1 {
        for x in 1..w-1 {
            // Simple Sobel X/Y
            let gx = -1.0 * gray.get_pixel(x-1, y-1)[0] as f32 + 
                      1.0 * gray.get_pixel(x+1, y-1)[0] as f32 +
                     -2.0 * gray.get_pixel(x-1, y)[0] as f32 + 
                      2.0 * gray.get_pixel(x+1, y)[0] as f32 +
                     -1.0 * gray.get_pixel(x-1, y+1)[0] as f32 + 
                      1.0 * gray.get_pixel(x+1, y+1)[0] as f32;
                      
            let gy = -1.0 * gray.get_pixel(x-1, y-1)[0] as f32 + 
                     -2.0 * gray.get_pixel(x, y-1)[0] as f32 + 
                     -1.0 * gray.get_pixel(x+1, y-1)[0] as f32 +
                      1.0 * gray.get_pixel(x-1, y+1)[0] as f32 + 
                      2.0 * gray.get_pixel(x, y+1)[0] as f32 + 
                      1.0 * gray.get_pixel(x+1, y+1)[0] as f32;

            let mag = (gx*gx + gy*gy).sqrt();
            if mag > threshold as f32 {
                // Paint Green
                output.put_pixel(x, y, image::Rgba([0, 255, 0, 255]));
            }
        }
    }
    DynamicImage::ImageRgba8(output)
}
