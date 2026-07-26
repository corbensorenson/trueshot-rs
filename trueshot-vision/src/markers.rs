use image::{DynamicImage, GenericImageView};
/// Vision Markers & Color Calibration
/// Implements detection of "Neutral Gray" patch for white balance.
/// Assumes a standard 18% gray card is the most "uniform" region in the center.

pub fn detect_gray_patch(img: &DynamicImage) -> Option<(f32, f32, f32)> {
    // 1. Convert to Lab (omitted for brevity, using RGB heuristics)
    // 2. Window search for low-variance patch
    
    let (w, h) = (img.width(), img.height());
    let step = 50;
    
    let mut best_score = f32::MAX;
    let mut best_color = None;

    if w < step || h < step { return None; }

    for y in (0..h-step).step_by(step as usize) {
        for x in (0..w-step).step_by(step as usize) {
            // Compute Mean & Variance in this 50x50 block
            let mut sum_r = 0.0f32;
            let mut sum_g = 0.0f32;
            let mut sum_b = 0.0f32;
            let mut count = 0.0f32;
            
            for by in 0..step {
                for bx in 0..step {
                    let p = img.get_pixel(x+bx, y+by);
                    sum_r += p[0] as f32;
                    sum_g += p[1] as f32;
                    sum_b += p[2] as f32;
                    count += 1.0;
                }
            }
            
            let mean_r = sum_r / count;
            let mean_g = sum_g / count;
            let mean_b = sum_b / count;
             
            // Calculate Variance (Standard Deviation)
            // A gray card is flat color -> Low Variance
            let mut sq_diff = 0.0;
             for by in 0..step {
                for bx in 0..step {
                    let p = img.get_pixel(x+bx, y+by);
                    let dr = p[0] as f32 - mean_r;
                    let dg = p[1] as f32 - mean_g;
                    let db = p[2] as f32 - mean_b;
                    sq_diff += dr*dr + dg*dg + db*db;
                }
            }
            let variance = sq_diff / count;
            
            // Heuristic: Gray is not Saturated (R~=G~=B)
            let saturation = (mean_r - mean_g).abs() + (mean_g - mean_b).abs() + (mean_b - mean_r).abs();
            
            let score = variance + (saturation * 10.0); // Penalize saturation heavily
            
            if score < best_score {
                best_score = score;
                best_color = Some((mean_r, mean_g, mean_b));
            }
        }
    }
    
    best_color
}
