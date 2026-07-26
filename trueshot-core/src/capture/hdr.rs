//! HDR Capture and Merge Module
//! 
//! High Dynamic Range image capture with automatic exposure bracketing
//! and multiple merge algorithms.
//!
//! Supports:
//! - Mertens Fusion (exposure fusion, no tone mapping needed)
//! - Debevec (full HDR with tone mapping)
//! - Robertson (iterative refinement)

use crate::Result;
use image::{DynamicImage, ImageBuffer, Rgb};
use rayon::prelude::*;
use std::path::Path;

/// HDR merge algorithm selection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HdrAlgorithm {
    /// Mertens Fusion - Exposure fusion, no HDR data, ready for display
    MertensFusion,
    /// Debevec - True HDR with camera response recovery
    Debevec,
    /// Robertson - Iterative calibration
    Robertson,
}

/// HDR capture configuration
#[derive(Debug, Clone)]
pub struct HdrConfig {
    /// Number of bracket shots (3, 5, 7, or 9)
    pub bracket_count: u8,
    /// EV spacing between shots (1, 2, or 3)
    pub ev_spacing: u8,
    /// Algorithm for merging
    pub algorithm: HdrAlgorithm,
    /// Tone mapping settings (for Debevec/Robertson)
    pub tone_map_gamma: f32,
    /// Alignment enabled
    pub align_images: bool,
}

impl Default for HdrConfig {
    fn default() -> Self {
        Self {
            bracket_count: 5,
            ev_spacing: 2,
            algorithm: HdrAlgorithm::MertensFusion,
            tone_map_gamma: 2.2,
            align_images: true,
        }
    }
}

/// Calculate EV values for bracket sequence
pub fn calculate_bracket_evs(config: &HdrConfig) -> Vec<f32> {
    let center = config.bracket_count as f32 / 2.0;
    (0..config.bracket_count)
        .map(|i| (i as f32 - center.floor()) * config.ev_spacing as f32)
        .collect()
}

/// HDR merger for combining bracketed exposures
pub struct HdrMerger {
    config: HdrConfig,
}

impl HdrMerger {
    pub fn new(config: HdrConfig) -> Self {
        Self { config }
    }
    
    /// Merge multiple exposures into single HDR image
    pub fn merge(&self, images: &[DynamicImage], evs: &[f32]) -> Result<DynamicImage> {
        if images.is_empty() {
            return Err(crate::Error::Processing("No images to merge".into()));
        }
        
        if images.len() != evs.len() {
            return Err(crate::Error::Processing("Image count must match EV count".into()));
        }
        
        match self.config.algorithm {
            HdrAlgorithm::MertensFusion => self.mertens_fusion(images),
            HdrAlgorithm::Debevec => self.debevec_merge(images, evs),
            HdrAlgorithm::Robertson => self.robertson_merge(images, evs),
        }
    }
    
    /// Mertens exposure fusion (no HDR, direct LDR output)
    /// Based on "Exposure Fusion" by Mertens, Kautz, and Van Reeth
    fn mertens_fusion(&self, images: &[DynamicImage]) -> Result<DynamicImage> {
        let (width, height) = (images[0].width() as usize, images[0].height() as usize);
        
        // Convert images to f32 buffers
        let rgba_images: Vec<Vec<[f32; 3]>> = images
            .iter()
            .map(|img| {
                let rgb = img.to_rgb8();
                rgb.pixels()
                    .map(|p| [
                        p[0] as f32 / 255.0,
                        p[1] as f32 / 255.0,
                        p[2] as f32 / 255.0,
                    ])
                    .collect()
            })
            .collect();
        
        // Calculate weight maps based on:
        // - Contrast (Laplacian)
        // - Saturation
        // - Well-exposedness (Gaussian centered at 0.5)
        let weight_maps: Vec<Vec<f32>> = rgba_images
            .par_iter()
            .map(|img| self.calculate_mertens_weights(img, width, height))
            .collect();
        
        // Normalize weights across all images per pixel
        let mut normalized_weights = vec![vec![0.0f32; width * height]; images.len()];
        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                let sum: f32 = weight_maps.iter().map(|w| w[idx]).sum();
                let sum = sum.max(1e-6); // Avoid division by zero
                
                for (i, weights) in normalized_weights.iter_mut().enumerate() {
                    weights[idx] = weight_maps[i][idx] / sum;
                }
            }
        }
        
        // Blend images using normalized weights
        let mut result = vec![[0.0f32; 3]; width * height];
        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                for (i, img) in rgba_images.iter().enumerate() {
                    let w = normalized_weights[i][idx];
                    result[idx][0] += img[idx][0] * w;
                    result[idx][1] += img[idx][1] * w;
                    result[idx][2] += img[idx][2] * w;
                }
            }
        }
        
        // Convert to image
        let mut output = ImageBuffer::new(width as u32, height as u32);
        for (i, pixel) in output.pixels_mut().enumerate() {
            *pixel = Rgb([
                (result[i][0].clamp(0.0, 1.0) * 255.0) as u8,
                (result[i][1].clamp(0.0, 1.0) * 255.0) as u8,
                (result[i][2].clamp(0.0, 1.0) * 255.0) as u8,
            ]);
        }
        
        Ok(DynamicImage::ImageRgb8(output))
    }
    
    /// Calculate Mertens weights for a single image
    fn calculate_mertens_weights(&self, img: &[[f32; 3]], width: usize, height: usize) -> Vec<f32> {
        let mut weights = vec![1.0f32; width * height];
        
        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                let pixel = img[idx];
                
                // Luminance
                let lum = 0.299 * pixel[0] + 0.587 * pixel[1] + 0.114 * pixel[2];
                
                // Well-exposedness: Gaussian centered at 0.5
                let sigma = 0.2;
                let exposedness = (-(pixel[0] - 0.5).powi(2) / (2.0 * sigma * sigma)).exp()
                    * (-(pixel[1] - 0.5).powi(2) / (2.0 * sigma * sigma)).exp()
                    * (-(pixel[2] - 0.5).powi(2) / (2.0 * sigma * sigma)).exp();
                
                // Saturation
                let mean = (pixel[0] + pixel[1] + pixel[2]) / 3.0;
                let saturation = ((pixel[0] - mean).powi(2) 
                    + (pixel[1] - mean).powi(2) 
                    + (pixel[2] - mean).powi(2)).sqrt() / 3.0_f32.sqrt();
                
                // Contrast (simplified Laplacian)
                let contrast = if x > 0 && x < width - 1 && y > 0 && y < height - 1 {
                    let neighbors = [
                        img[(y - 1) * width + x],
                        img[(y + 1) * width + x],
                        img[y * width + x - 1],
                        img[y * width + x + 1],
                    ];
                    let avg_neighbor: f32 = neighbors.iter()
                        .map(|n| 0.299 * n[0] + 0.587 * n[1] + 0.114 * n[2])
                        .sum::<f32>() / 4.0;
                    (lum - avg_neighbor).abs()
                } else {
                    0.0
                };
                
                // Combine weights (raised to power for emphasis)
                weights[idx] = (contrast + 0.001).powf(1.0) 
                    * (saturation + 0.001).powf(1.0) 
                    * (exposedness + 0.001).powf(1.0);
            }
        }
        
        weights
    }
    
    /// Debevec HDR merge with camera response recovery
    fn debevec_merge(&self, images: &[DynamicImage], evs: &[f32]) -> Result<DynamicImage> {
        let (width, height) = (images[0].width() as usize, images[0].height() as usize);
        
        // Recover camera response function (simplified linear assumption)
        // In production, solve for g(Z) using SVD
        
        // Compute HDR radiance map
        let mut hdr = vec![[0.0f32; 3]; width * height];
        let mut weight_sum = vec![[0.0f32; 3]; width * height];
        
        for (img_idx, (img, ev)) in images.iter().zip(evs).enumerate() {
            let rgb = img.to_rgb8();
            let exposure_time = 2.0_f32.powf(-*ev); // Relative exposure
            
            for (i, pixel) in rgb.pixels().enumerate() {
                for c in 0..3 {
                    let z = pixel[c];
                    // Hat-shaped weighting function
                    let w = if z <= 127 { z as f32 / 127.0 } else { (255 - z) as f32 / 128.0 };
                    let w = w.max(0.01);
                    
                    // Assume linear response: g(Z) = Z
                    let radiance = (z as f32 / 255.0) / exposure_time;
                    
                    hdr[i][c] += w * radiance;
                    weight_sum[i][c] += w;
                }
            }
        }
        
        // Normalize by weights
        for i in 0..(width * height) {
            for c in 0..3 {
                if weight_sum[i][c] > 0.0 {
                    hdr[i][c] /= weight_sum[i][c];
                }
            }
        }
        
        // Tone mapping (Reinhard global operator)
        let max_lum = hdr.iter()
            .map(|p| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2])
            .fold(0.0f32, |a, b| a.max(b));
        
        let mut output = ImageBuffer::new(width as u32, height as u32);
        for (i, pixel) in output.pixels_mut().enumerate() {
            let mut rgb = hdr[i];
            
            // Reinhard tone mapping per channel
            for c in 0..3 {
                rgb[c] = rgb[c] / (1.0 + rgb[c]);
                rgb[c] = rgb[c].powf(1.0 / self.config.tone_map_gamma);
            }
            
            *pixel = Rgb([
                (rgb[0].clamp(0.0, 1.0) * 255.0) as u8,
                (rgb[1].clamp(0.0, 1.0) * 255.0) as u8,
                (rgb[2].clamp(0.0, 1.0) * 255.0) as u8,
            ]);
        }
        
        Ok(DynamicImage::ImageRgb8(output))
    }
    
    /// Robertson iterative HDR merge
    fn robertson_merge(&self, images: &[DynamicImage], evs: &[f32]) -> Result<DynamicImage> {
        // Robertson is an iterative refinement of Debevec
        // For initial implementation, use Debevec as base
        self.debevec_merge(images, evs)
    }
}

/// Load images from paths for HDR merge
pub fn load_bracket_images(paths: &[impl AsRef<Path>]) -> Result<Vec<DynamicImage>> {
    paths
        .iter()
        .map(|p| image::open(p.as_ref()).map_err(|e| crate::Error::Io(e.to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_calculate_bracket_evs() {
        let config = HdrConfig {
            bracket_count: 5,
            ev_spacing: 2,
            ..Default::default()
        };
        
        let evs = calculate_bracket_evs(&config);
        assert_eq!(evs.len(), 5);
        // Should be centered: -4, -2, 0, +2, +4
        assert!((evs[2] - 0.0).abs() < 0.01); // Center should be ~0
    }
    
    #[test]
    fn test_mertens_weights() {
        let merger = HdrMerger::new(HdrConfig::default());
        let img = vec![[0.5, 0.5, 0.5]; 9]; // 3x3 mid-gray image
        let weights = merger.calculate_mertens_weights(&img, 3, 3);
        assert!(weights.iter().all(|w| *w > 0.0));
    }
}
