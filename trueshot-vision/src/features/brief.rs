//! BRIEF (Binary Robust Independent Elementary Features) Descriptor
//! 
//! Native Rust implementation - no OpenCV required.
//! Creates 256-bit binary descriptors for fast matching.

use image::GrayImage;
use super::keypoint::Keypoint;

/// BRIEF descriptor extractor
pub struct BriefDescriptor {
    /// Pre-computed sampling pattern (256 pairs of points)
    pattern: Vec<(i8, i8, i8, i8)>,
    /// Patch size (half-width)
    patch_size: i32,
}

impl BriefDescriptor {
    /// Create a new BRIEF extractor with default pattern
    pub fn new() -> Self {
        Self {
            pattern: Self::generate_pattern(),
            patch_size: 16, // 31x31 patch
        }
    }

    /// Generate the sampling pattern (pseudo-random but deterministic)
    /// Uses a simple LCG (Linear Congruential Generator) for reproducibility
    fn generate_pattern() -> Vec<(i8, i8, i8, i8)> {
        let mut pattern = Vec::with_capacity(256);
        let mut seed: u32 = 42; // Fixed seed for reproducibility
        
        for _ in 0..256 {
            // LCG: next = (a * current + c) mod m
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let x1 = ((seed >> 16) % 31) as i8 - 15;
            
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let y1 = ((seed >> 16) % 31) as i8 - 15;
            
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let x2 = ((seed >> 16) % 31) as i8 - 15;
            
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let y2 = ((seed >> 16) % 31) as i8 - 15;
            
            pattern.push((x1, y1, x2, y2));
        }
        
        pattern
    }

    /// Compute BRIEF descriptor for a keypoint
    /// Returns None if keypoint is too close to image border
    pub fn compute(&self, image: &GrayImage, keypoint: &Keypoint) -> Option<Vec<u8>> {
        let (width, height) = image.dimensions();
        let x = keypoint.x as i32;
        let y = keypoint.y as i32;
        
        // Check bounds
        if x < self.patch_size || y < self.patch_size 
            || x >= (width as i32 - self.patch_size) 
            || y >= (height as i32 - self.patch_size) {
            return None;
        }

        // Compute 256-bit descriptor (32 bytes)
        let mut descriptor = vec![0u8; 32];
        
        for (i, &(x1, y1, x2, y2)) in self.pattern.iter().enumerate() {
            // Get intensity at first point
            let px1 = (x + x1 as i32) as u32;
            let py1 = (y + y1 as i32) as u32;
            let i1 = image.get_pixel(px1, py1)[0];
            
            // Get intensity at second point
            let px2 = (x + x2 as i32) as u32;
            let py2 = (y + y2 as i32) as u32;
            let i2 = image.get_pixel(px2, py2)[0];
            
            // Binary test: set bit if i1 < i2
            if i1 < i2 {
                let byte_idx = i / 8;
                let bit_idx = i % 8;
                descriptor[byte_idx] |= 1 << bit_idx;
            }
        }
        
        Some(descriptor)
    }

    /// Compute BRIEF descriptor with rotation compensation (rBRIEF)
    /// Uses the keypoint angle for rotation invariance
    pub fn compute_rotated(&self, image: &GrayImage, keypoint: &Keypoint) -> Option<Vec<u8>> {
        let (width, height) = image.dimensions();
        let x = keypoint.x;
        let y = keypoint.y;
        
        // Check bounds
        let margin = (self.patch_size as f32 * 1.5) as i32;
        if (x as i32) < margin || (y as i32) < margin 
            || (x as i32) >= (width as i32 - margin) 
            || (y as i32) >= (height as i32 - margin) {
            return None;
        }

        let cos_a = keypoint.angle.cos();
        let sin_a = keypoint.angle.sin();

        // Compute 256-bit descriptor with rotation
        let mut descriptor = vec![0u8; 32];
        
        for (i, &(x1, y1, x2, y2)) in self.pattern.iter().enumerate() {
            // Rotate first point
            let rx1 = cos_a * (x1 as f32) - sin_a * (y1 as f32);
            let ry1 = sin_a * (x1 as f32) + cos_a * (y1 as f32);
            let px1 = (x + rx1).round() as u32;
            let py1 = (y + ry1).round() as u32;
            
            // Rotate second point
            let rx2 = cos_a * (x2 as f32) - sin_a * (y2 as f32);
            let ry2 = sin_a * (x2 as f32) + cos_a * (y2 as f32);
            let px2 = (x + rx2).round() as u32;
            let py2 = (y + ry2).round() as u32;
            
            // Get intensities
            let i1 = image.get_pixel(px1.min(width - 1), py1.min(height - 1))[0];
            let i2 = image.get_pixel(px2.min(width - 1), py2.min(height - 1))[0];
            
            // Binary test
            if i1 < i2 {
                let byte_idx = i / 8;
                let bit_idx = i % 8;
                descriptor[byte_idx] |= 1 << bit_idx;
            }
        }
        
        Some(descriptor)
    }
}

impl Default for BriefDescriptor {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute Hamming distance between two binary descriptors
pub fn hamming_distance(d1: &[u8], d2: &[u8]) -> u32 {
    assert_eq!(d1.len(), d2.len(), "Descriptors must have same length");
    
    d1.iter()
        .zip(d2.iter())
        .map(|(&a, &b)| (a ^ b).count_ones())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hamming_distance() {
        let d1 = vec![0b11110000u8, 0b00001111];
        let d2 = vec![0b11110000u8, 0b00001111];
        assert_eq!(hamming_distance(&d1, &d2), 0);

        let d3 = vec![0b11111111u8, 0b11111111];
        assert_eq!(hamming_distance(&d1, &d3), 8);
    }

    #[test]
    fn test_pattern_generation() {
        let brief = BriefDescriptor::new();
        assert_eq!(brief.pattern.len(), 256);
        
        // All values should be in [-15, 15]
        for &(x1, y1, x2, y2) in &brief.pattern {
            assert!(x1 >= -15 && x1 <= 15);
            assert!(y1 >= -15 && y1 <= 15);
            assert!(x2 >= -15 && x2 <= 15);
            assert!(y2 >= -15 && y2 <= 15);
        }
    }
}
