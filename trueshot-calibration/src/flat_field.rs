use image::GrayImage;
use anyhow::Result;

/// Flat Field Calibrator for Sensor Dust Mapping
/// Generates a Gain Map to normalize brightness and remove dust spots.

pub struct FlatField;

impl FlatField {
    pub fn generate_gain_map(images: &[GrayImage]) -> Result<Vec<f32>> {
        if images.is_empty() {
            anyhow::bail!("No images provided for flat field");
        }
        
        let (w, h) = images[0].dimensions();
        let len = (w * h) as usize;
        let mut accum = vec![0.0f32; len];
        
        // Sum
        for img in images {
            for (i, p) in img.pixels().enumerate() {
                accum[i] += p[0] as f32;
            }
        }
        
        // Average
        let num_imgs = images.len() as f32;
        for v in &mut accum { *v /= num_imgs; }
        
        // Find center brightness (approximate max reliable brightness)
        // Or just use max
        let max_val = accum.iter().fold(0.0f32, |a, &b| a.max(b));
        
        // Invert to get Gain ( Gain = Max / Check )
        // Dark spot (dust) = low value -> High Gain to correct it
        let mut gain_map = vec![1.0; len];
        for (i, &val) in accum.iter().enumerate() {
            if val > 1.0 { // Avoid div by zero
                gain_map[i] = max_val / val;
            }
        }
        
        Ok(gain_map)
    }
    
    pub fn apply_correction(img: &mut GrayImage, gain_map: &[f32]) {
        for (i, p) in img.pixels_mut().enumerate() {
            let val = p[0] as f32 * gain_map[i];
            p[0] = val.min(255.0) as u8;
        }
    }
}
