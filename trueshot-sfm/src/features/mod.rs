//! Feature Detection Module
//!
//! Native Rust implementation of feature detection without OpenCV.
//! Provides ORB and SIFT-like feature detectors.

use image::{DynamicImage, GrayImage};

// ============================================================================
// Types
// ============================================================================

/// 2D keypoint with orientation and scale
#[derive(Clone, Debug)]
pub struct Keypoint {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub angle: f32,
    pub response: f32,
    pub octave: i32,
}

/// Feature descriptor (128 or 256 dimensions)
#[derive(Clone, Debug)]
pub struct Descriptor {
    pub data: Vec<u8>,
}

impl Descriptor {
    /// Hamming distance between two descriptors
    pub fn hamming_distance(&self, other: &Descriptor) -> u32 {
        self.data.iter()
            .zip(other.data.iter())
            .map(|(a, b)| (a ^ b).count_ones())
            .sum()
    }
    
    /// L2 distance between two descriptors (for SIFT)
    pub fn l2_distance(&self, other: &Descriptor) -> f32 {
        self.data.iter()
            .zip(other.data.iter())
            .map(|(a, b)| (*a as f32 - *b as f32).powi(2))
            .sum::<f32>()
            .sqrt()
    }
}

// ============================================================================
// ORB Feature Detection
// ============================================================================

/// Detect ORB features in image
pub fn detect_orb(image: &DynamicImage, max_features: usize) -> (Vec<Keypoint>, Vec<Descriptor>) {
    let gray = image.to_luma8();
    let (width, height) = gray.dimensions();
    
    // 1. Detect FAST corners at multiple scales
    let mut keypoints = Vec::new();
    
    for octave in 0..4 {
        let scale = 1.0 / (1 << octave) as f32;
        let scaled_gray = if octave == 0 {
            gray.clone()
        } else {
            image::imageops::resize(
                &gray,
                (width as f32 * scale) as u32,
                (height as f32 * scale) as u32,
                image::imageops::FilterType::Lanczos3,
            )
        };
        
        let corners = detect_fast_corners(&scaled_gray, 20);
        
        for (x, y, response) in corners {
            let angle = compute_orientation(&scaled_gray, x, y);
            keypoints.push(Keypoint {
                x: x as f32 / scale,
                y: y as f32 / scale,
                scale,
                angle,
                response,
                octave: octave,
            });
        }
    }
    
    // 2. Sort by response and take top N
    keypoints.sort_by(|a, b| b.response.partial_cmp(&a.response).unwrap());
    keypoints.truncate(max_features);
    
    // 3. Compute ORB descriptors
    let descriptors: Vec<Descriptor> = keypoints.iter()
        .map(|kp| compute_orb_descriptor(&gray, kp))
        .collect();
    
    (keypoints, descriptors)
}

/// FAST corner detection
fn detect_fast_corners(gray: &GrayImage, threshold: i32) -> Vec<(u32, u32, f32)> {
    let (width, height) = gray.dimensions();
    let mut corners = Vec::new();
    
    // FAST-9 pattern offsets
    let circle: [(i32, i32); 16] = [
        (0, -3), (1, -3), (2, -2), (3, -1),
        (3, 0), (3, 1), (2, 2), (1, 3),
        (0, 3), (-1, 3), (-2, 2), (-3, 1),
        (-3, 0), (-3, -1), (-2, -2), (-1, -3),
    ];
    
    for y in 3..height.saturating_sub(3) {
        for x in 3..width.saturating_sub(3) {
            let center = gray.get_pixel(x, y).0[0] as i32;
            
            let mut brighter = 0;
            let mut darker = 0;
            
            for &(dx, dy) in &circle {
                let nx = (x as i32 + dx) as u32;
                let ny = (y as i32 + dy) as u32;
                let pixel = gray.get_pixel(nx, ny).0[0] as i32;
                
                if pixel > center + threshold {
                    brighter += 1;
                } else if pixel < center - threshold {
                    darker += 1;
                }
            }
            
            if brighter >= 9 || darker >= 9 {
                let response = brighter.max(darker) as f32;
                corners.push((x, y, response));
            }
        }
    }
    
    // Non-maximum suppression
    non_max_suppression(&mut corners, 5);
    
    corners
}

fn non_max_suppression(corners: &mut Vec<(u32, u32, f32)>, radius: u32) {
    corners.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    
    let mut keep = vec![true; corners.len()];
    
    for i in 0..corners.len() {
        if !keep[i] { continue; }
        
        for j in (i + 1)..corners.len() {
            if !keep[j] { continue; }
            
            let dx = corners[i].0 as i32 - corners[j].0 as i32;
            let dy = corners[i].1 as i32 - corners[j].1 as i32;
            
            if dx * dx + dy * dy < (radius * radius) as i32 {
                keep[j] = false;
            }
        }
    }
    
    let mut i = 0;
    corners.retain(|_| {
        let k = keep[i];
        i += 1;
        k
    });
}

/// Compute orientation using intensity centroid
fn compute_orientation(gray: &GrayImage, x: u32, y: u32) -> f32 {
    let (width, height) = gray.dimensions();
    let radius = 15i32;
    
    let mut m01 = 0f32;
    let mut m10 = 0f32;
    
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            
            if nx >= 0 && ny >= 0 && (nx as u32) < width && (ny as u32) < height {
                let intensity = gray.get_pixel(nx as u32, ny as u32).0[0] as f32;
                m01 += dy as f32 * intensity;
                m10 += dx as f32 * intensity;
            }
        }
    }
    
    m01.atan2(m10)
}

/// Compute ORB descriptor (BRIEF with rotation)
fn compute_orb_descriptor(gray: &GrayImage, kp: &Keypoint) -> Descriptor {
    let (width, height) = gray.dimensions();
    
    // Pre-computed BRIEF pattern (simplified)
    let pattern: [(i8, i8, i8, i8); 256] = generate_brief_pattern();
    
    let cos_a = kp.angle.cos();
    let sin_a = kp.angle.sin();
    
    let mut desc = vec![0u8; 32]; // 256 bits
    
    for (i, &(x1, y1, x2, y2)) in pattern.iter().enumerate() {
        // Rotate pattern by keypoint angle
        let rx1 = (x1 as f32 * cos_a - y1 as f32 * sin_a) as i32;
        let ry1 = (x1 as f32 * sin_a + y1 as f32 * cos_a) as i32;
        let rx2 = (x2 as f32 * cos_a - y2 as f32 * sin_a) as i32;
        let ry2 = (x2 as f32 * sin_a + y2 as f32 * cos_a) as i32;
        
        let px1 = (kp.x as i32 + rx1).clamp(0, width as i32 - 1) as u32;
        let py1 = (kp.y as i32 + ry1).clamp(0, height as i32 - 1) as u32;
        let px2 = (kp.x as i32 + rx2).clamp(0, width as i32 - 1) as u32;
        let py2 = (kp.y as i32 + ry2).clamp(0, height as i32 - 1) as u32;
        
        let v1 = gray.get_pixel(px1, py1).0[0];
        let v2 = gray.get_pixel(px2, py2).0[0];
        
        if v1 < v2 {
            desc[i / 8] |= 1 << (i % 8);
        }
    }
    
    Descriptor { data: desc }
}

fn generate_brief_pattern() -> [(i8, i8, i8, i8); 256] {
    // Deterministic BRIEF pattern (ORB uses learned patterns)
    let mut pattern = [(0i8, 0i8, 0i8, 0i8); 256];
    let mut seed = 42u64;
    
    for p in &mut pattern {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        p.0 = ((seed >> 16) % 31) as i8 - 15;
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        p.1 = ((seed >> 16) % 31) as i8 - 15;
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        p.2 = ((seed >> 16) % 31) as i8 - 15;
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        p.3 = ((seed >> 16) % 31) as i8 - 15;
    }
    
    pattern
}

// ============================================================================
// SIFT-like Feature Detection
// ============================================================================

/// Detect SIFT-like features (simplified DoG approach)
pub fn detect_sift(image: &DynamicImage, max_features: usize) -> (Vec<Keypoint>, Vec<Descriptor>) {
    let gray = image.to_luma8();
    let (width, height) = gray.dimensions();
    
    // Build Gaussian pyramid
    let mut pyramid = Vec::new();
    let mut current = gray.clone();
    
    for octave in 0..4 {
        let mut octave_images = Vec::new();
        
        for s in 0..5 {
            let sigma = 1.6 * (2.0f32).powf(s as f32 / 3.0);
            let blurred = gaussian_blur(&current, sigma);
            octave_images.push(blurred);
        }
        
        pyramid.push(octave_images);
        
        // Downsample for next octave
        if octave < 3 {
            current = image::imageops::resize(
                &current,
                width / (2 << octave),
                height / (2 << octave),
                image::imageops::FilterType::Lanczos3,
            );
        }
    }
    
    // Detect extrema in DoG space
    let mut keypoints = Vec::new();
    
    for (octave, octave_images) in pyramid.iter().enumerate() {
        let scale = (1 << octave) as f32;
        
        // Compute DoG images
        let dogs: Vec<GrayImage> = octave_images.windows(2)
            .map(|pair| subtract_images(&pair[1], &pair[0]))
            .collect();
        
        // Find extrema
        for s in 1..(dogs.len() - 1) {
            let (w, h) = dogs[s].dimensions();
            
            for y in 1..h.saturating_sub(1) {
                for x in 1..w.saturating_sub(1) {
                    let val = dogs[s].get_pixel(x, y).0[0] as i32;
                    
                    if is_extremum(&dogs, s, x, y) {
                        let angle = compute_orientation(&octave_images[s], x, y);
                        
                        keypoints.push(Keypoint {
                            x: x as f32 * scale,
                            y: y as f32 * scale,
                            scale: 1.6 * (2.0f32).powf(octave as f32 + s as f32 / 3.0),
                            angle,
                            response: val.abs() as f32,
                            octave: octave as i32,
                        });
                    }
                }
            }
        }
    }
    
    // Sort and limit
    keypoints.sort_by(|a, b| b.response.partial_cmp(&a.response).unwrap());
    keypoints.truncate(max_features);
    
    // Compute SIFT descriptors
    let descriptors: Vec<Descriptor> = keypoints.iter()
        .map(|kp| compute_sift_descriptor(&gray, kp))
        .collect();
    
    (keypoints, descriptors)
}

fn gaussian_blur(img: &GrayImage, sigma: f32) -> GrayImage {
    // Simple box blur approximation (3 passes)
    let radius = (sigma * 3.0).ceil() as u32;
    
    let mut result = img.clone();
    for _ in 0..3 {
        result = box_blur(&result, radius);
    }
    result
}

fn box_blur(img: &GrayImage, radius: u32) -> GrayImage {
    let (width, height) = img.dimensions();
    let mut result = GrayImage::new(width, height);
    
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0u32;
            let mut count = 0u32;
            
            for dy in -(radius as i32)..=(radius as i32) {
                for dx in -(radius as i32)..=(radius as i32) {
                    let nx = (x as i32 + dx).clamp(0, width as i32 - 1) as u32;
                    let ny = (y as i32 + dy).clamp(0, height as i32 - 1) as u32;
                    
                    sum += img.get_pixel(nx, ny).0[0] as u32;
                    count += 1;
                }
            }
            
            result.put_pixel(x, y, image::Luma([(sum / count) as u8]));
        }
    }
    
    result
}

fn subtract_images(a: &GrayImage, b: &GrayImage) -> GrayImage {
    let (width, height) = a.dimensions();
    let mut result = GrayImage::new(width, height);
    
    for y in 0..height {
        for x in 0..width {
            let va = a.get_pixel(x, y).0[0] as i32;
            let vb = b.get_pixel(x, y).0[0] as i32;
            result.put_pixel(x, y, image::Luma([((va - vb).abs().min(255)) as u8]));
        }
    }
    
    result
}

fn is_extremum(dogs: &[GrayImage], s: usize, x: u32, y: u32) -> bool {
    let val = dogs[s].get_pixel(x, y).0[0] as i32;
    
    if val.abs() < 5 { return false; } // Threshold
    
    let mut is_max = true;
    let mut is_min = true;
    
    for ds in -1i32..=1 {
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if ds == 0 && dy == 0 && dx == 0 { continue; }
                
                let ns = (s as i32 + ds) as usize;
                let nx = (x as i32 + dx) as u32;
                let ny = (y as i32 + dy) as u32;
                
                let neighbor = dogs[ns].get_pixel(nx, ny).0[0] as i32;
                
                if neighbor >= val { is_max = false; }
                if neighbor <= val { is_min = false; }
            }
        }
    }
    
    is_max || is_min
}

fn compute_sift_descriptor(gray: &GrayImage, kp: &Keypoint) -> Descriptor {
    // 4x4 grid, 8 orientation bins each = 128 dimensions
    let (width, height) = gray.dimensions();
    let mut desc = vec![0f32; 128];
    
    let cos_a = kp.angle.cos();
    let sin_a = kp.angle.sin();
    for dy in -8..8 {
        for dx in -8..8 {
            let rx = dx as f32 * cos_a - dy as f32 * sin_a;
            let ry = dx as f32 * sin_a + dy as f32 * cos_a;
            
            let px = (kp.x + rx * kp.scale).round() as i32;
            let py = (kp.y + ry * kp.scale).round() as i32;
            
            if px < 1 || py < 1 || px >= width as i32 - 1 || py >= height as i32 - 1 {
                continue;
            }
            
            // Compute gradient
            let gx = gray.get_pixel((px + 1) as u32, py as u32).0[0] as f32
                   - gray.get_pixel((px - 1) as u32, py as u32).0[0] as f32;
            let gy = gray.get_pixel(px as u32, (py + 1) as u32).0[0] as f32
                   - gray.get_pixel(px as u32, (py - 1) as u32).0[0] as f32;
            
            let mag = (gx * gx + gy * gy).sqrt();
            let angle = gy.atan2(gx) - kp.angle;
            
            // Bin into 4x4 grid
            let bx = ((dx + 8) / 4).min(3) as usize;
            let by = ((dy + 8) / 4).min(3) as usize;
            let bo = ((angle + std::f32::consts::PI) / (std::f32::consts::PI / 4.0)) as usize % 8;
            
            let idx = (by * 4 + bx) * 8 + bo;
            desc[idx] += mag;
        }
    }
    
    // Normalize
    let norm: f32 = desc.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    for v in &mut desc {
        *v = (*v / norm * 512.0).min(255.0);
    }
    
    Descriptor {
        data: desc.iter().map(|x| *x as u8).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_orb_detection() {
        // Create test image
        let img = DynamicImage::new_rgb8(640, 480);
        let (keypoints, descriptors) = detect_orb(&img, 100);
        
        // Empty image should have few/no features
        assert!(keypoints.len() <= 100);
        assert_eq!(keypoints.len(), descriptors.len());
    }
}
