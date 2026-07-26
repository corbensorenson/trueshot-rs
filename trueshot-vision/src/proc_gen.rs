use image::DynamicImage;
/// Procedural Material Generators (Replacing JIT Placeholder)
/// Implements concrete pixel math for specific material profiles.
/// This runs on the CPU as a fallback or pre-processor for when GPU is busy.
pub enum MaterialProfile {
    Matte,
    Shiny,
    Dark,
}

pub fn apply_profile_proc(img: &mut DynamicImage, profile: MaterialProfile) {
    // We access the raw buffer for speed (SIMD auto-vectorization friendly)
    if let Some(rgb) = img.as_mut_rgb8() {
        // Parallel iteration could be added here with rayon par_chunks_mut
        for pixel in rgb.pixels_mut() {
            match profile {
                MaterialProfile::Matte => {
                    // Brighten shadows, clamp highlights (simulating diffuse)
                    // pixel = pixel * 1.2 + 10 (clamped)
                    pixel[0] = ((pixel[0] as f32 * 1.2) + 10.0).min(255.0) as u8;
                    pixel[1] = ((pixel[1] as f32 * 1.2) + 10.0).min(255.0) as u8;
                    pixel[2] = ((pixel[2] as f32 * 1.2) + 10.0).min(255.0) as u8;
                }
                MaterialProfile::Shiny => {
                    // Increase contrast (S-curve)
                    // pixel = ((pixel - 128) * 1.5) + 128
                    // Simplified: just darken mids to emphasize speculars
                    pixel[0] = (pixel[0] as f32 * 0.9).max(0.0) as u8;
                    pixel[1] = (pixel[1] as f32 * 0.9).max(0.0) as u8;
                    pixel[2] = (pixel[2] as f32 * 0.9).max(0.0) as u8;
                }
                MaterialProfile::Dark => {
                    // Gamma correction
                    // pixel = pow(pixel, 1/2.2)
                    // Quick approx: sqrt
                    pixel[0] = ((pixel[0] as f32).sqrt() * 16.0).min(255.0) as u8; // *16 scales 0-16(sqrt255) to 0-255 approx
                    pixel[1] = ((pixel[1] as f32).sqrt() * 16.0).min(255.0) as u8;
                    pixel[2] = ((pixel[2] as f32).sqrt() * 16.0).min(255.0) as u8;
                }
            }
        }
    }
}
