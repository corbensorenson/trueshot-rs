use image::GrayImage;
use anyhow::Result;

/// Modulation Transfer Function (MTF) Mapper
/// Uses the edges of the calibration checkerboard to estimate local sharpness.

pub struct MtfMapper;

impl MtfMapper {
    /// Compute a sharpness map (0.0 - 1.0) for the lens field of view.
    /// Returns a low-res float grid representing sharpness weights.
    pub fn compute_sharpness_map(img: &GrayImage, corners: &[(f32, f32)]) -> Result<Vec<f32>> {
        // 1. For each checkerboard corner, extract a small ROI
        // 2. Measure edge slope (gradient magnitude)
        // 3. Normalize
        
        // This is a simplified "Local Contrast" version of MTF
        
        let (w, h) = img.dimensions();
        let mut scores = Vec::new();
        
        for (cx, cy) in corners {
            let x = *cx as u32;
            let y = *cy as u32;
            
            if x < 10 || x >= w-10 || y < 10 || y >= h-10 {
                scores.push(0.0);
                continue;
            }
            
            // ROI 20x20
            let mut grad_sum = 0.0;
            for dy in 0..20 {
                for dx in 0..20 {
                    let px = x - 10 + dx;
                    let py = y - 10 + dy;
                    let val = img.get_pixel(px, py)[0] as f32;
                    let val_right = img.get_pixel(px+1, py)[0] as f32;
                    let val_down = img.get_pixel(px, py+1)[0] as f32;
                    
                    let gx = (val_right - val).abs();
                    let gy = (val_down - val).abs();
                    grad_sum += (gx*gx + gy*gy).sqrt();
                }
            }
            scores.push(grad_sum / 400.0);
        }
        
        // Normalize 0.0 to 1.0 based on max score
        let max_score = scores.iter().fold(0.0f32, |a, &b| a.max(b));
        if max_score > 0.0 {
            for s in &mut scores { *s /= max_score; }
        }
        
        Ok(scores)
    }
}
