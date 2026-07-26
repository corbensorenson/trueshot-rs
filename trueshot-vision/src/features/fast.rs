//! FAST (Features from Accelerated Segment Test) Corner Detection
//!
//! Native Rust implementation - no OpenCV required.
//! Reference: Rosten & Drummond, "Machine Learning for High-Speed Corner Detection"

use super::keypoint::Keypoint;
use image::GrayImage;

/// FAST corner detector
pub struct FastDetector {
    /// Intensity threshold for corner detection
    threshold: u8,
    /// Whether to apply non-maximum suppression
    nonmax_suppression: bool,
    /// Minimum number of contiguous pixels for a corner (typically 9 or 12)
    n_contiguous: usize,
}

impl FastDetector {
    /// Create a new FAST detector
    ///
    /// # Arguments
    /// * `threshold` - Intensity difference threshold (typically 10-50)
    /// * `nonmax_suppression` - Apply non-maximum suppression to reduce duplicates
    pub fn new(threshold: u8, nonmax_suppression: bool) -> Self {
        Self {
            threshold,
            nonmax_suppression,
            n_contiguous: 9, // FAST-9 (most common)
        }
    }

    /// Detect FAST corners in an image (PARALLELIZED with Rayon)
    pub fn detect(&self, image: &GrayImage) -> Vec<Keypoint> {
        use rayon::prelude::*;

        let (width, height) = image.dimensions();
        if width < 7 || height < 7 {
            return Vec::new();
        }
        let threshold = self.threshold;
        let n_contiguous = self.n_contiguous;

        // Parallel row-by-row processing for cache efficiency
        let candidates: Vec<Keypoint> = (3..(height - 3) as usize)
            .into_par_iter()
            .flat_map(|y| {
                let mut row_keypoints = Vec::new();
                for x in 3..(width - 3) {
                    if let Some(response) =
                        is_corner_fast(image, x, y as u32, threshold, n_contiguous)
                    {
                        row_keypoints.push(Keypoint::new(x as f32, y as f32, response));
                    }
                }
                row_keypoints
            })
            .collect();

        if self.nonmax_suppression {
            self.apply_nonmax_suppression(&candidates, width, height)
        } else {
            candidates
        }
    }

    /// Check if a pixel is a FAST corner (instance method)
    /// Returns Some(response) if corner, None otherwise
    #[allow(dead_code)]
    fn is_corner(&self, image: &GrayImage, x: u32, y: u32) -> Option<f32> {
        is_corner_fast(image, x, y, self.threshold, self.n_contiguous)
    }
}

/// Standalone FAST corner check (for parallel processing)
/// Returns Some(response) if corner, None otherwise
fn is_corner_fast(
    image: &GrayImage,
    x: u32,
    y: u32,
    threshold: u8,
    n_contiguous: usize,
) -> Option<f32> {
    let center = image.get_pixel(x, y)[0] as i16;
    let t = threshold as i16;

    // 16-pixel Bresenham circle offsets (radius 3)
    // Ordered for optimal early rejection
    let circle: [(i32, i32); 16] = [
        (0, -3),  // 0 - top
        (1, -3),  // 1
        (2, -2),  // 2
        (3, -1),  // 3
        (3, 0),   // 4 - right
        (3, 1),   // 5
        (2, 2),   // 6
        (1, 3),   // 7
        (0, 3),   // 8 - bottom
        (-1, 3),  // 9
        (-2, 2),  // 10
        (-3, 1),  // 11
        (-3, 0),  // 12 - left
        (-3, -1), // 13
        (-2, -2), // 14
        (-1, -3), // 15
    ];

    // Any N-pixel circular arc must cross at least floor(N / 4) cardinal
    // samples. FAST-9 therefore needs two, not the FAST-12 shortcut of three.
    let p0 = image.get_pixel(
        (x as i32 + circle[0].0) as u32,
        (y as i32 + circle[0].1) as u32,
    )[0] as i16;
    let p4 = image.get_pixel(
        (x as i32 + circle[4].0) as u32,
        (y as i32 + circle[4].1) as u32,
    )[0] as i16;
    let p8 = image.get_pixel(
        (x as i32 + circle[8].0) as u32,
        (y as i32 + circle[8].1) as u32,
    )[0] as i16;
    let p12 = image.get_pixel(
        (x as i32 + circle[12].0) as u32,
        (y as i32 + circle[12].1) as u32,
    )[0] as i16;

    let high = center + t;
    let low = center - t;

    // Count cardinal points that are brighter/darker
    let brighter_count =
        (p0 > high) as u8 + (p4 > high) as u8 + (p8 > high) as u8 + (p12 > high) as u8;
    let darker_count = (p0 < low) as u8 + (p4 < low) as u8 + (p8 < low) as u8 + (p12 < low) as u8;

    let required_cardinals = (n_contiguous / 4).max(1) as u8;
    if brighter_count < required_cardinals && darker_count < required_cardinals {
        return None;
    }

    // Full segment test: check all 16 pixels
    let mut intensities = [0i16; 16];
    for (i, &(dx, dy)) in circle.iter().enumerate() {
        intensities[i] = image.get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32)[0] as i16;
    }

    // Check for N contiguous brighter pixels
    let brighter = count_contiguous(&intensities, |p| p > high);
    if brighter >= n_contiguous {
        // Corner response = sum of absolute differences
        let response: f32 = intensities.iter().map(|&p| (p - center).abs() as f32).sum();
        return Some(response);
    }

    // Check for N contiguous darker pixels
    let darker = count_contiguous(&intensities, |p| p < low);
    if darker >= n_contiguous {
        let response: f32 = intensities.iter().map(|&p| (p - center).abs() as f32).sum();
        return Some(response);
    }

    None
}

/// Count maximum contiguous pixels satisfying a predicate (standalone for parallel)
/// Treats the array as circular
fn count_contiguous<F>(intensities: &[i16; 16], pred: F) -> usize
where
    F: Fn(i16) -> bool,
{
    // Create a doubled array to handle wraparound
    let mut max_count = 0;
    let mut current_count = 0;

    // First pass: count contiguous in circular manner
    for i in 0..32 {
        if pred(intensities[i % 16]) {
            current_count += 1;
            max_count = max_count.max(current_count);
        } else {
            current_count = 0;
        }
    }

    max_count.min(16) // Can't be more than 16
}

impl FastDetector {
    /// Apply non-maximum suppression in linear time with a compact response map.
    fn apply_nonmax_suppression(
        &self,
        keypoints: &[Keypoint],
        width: u32,
        height: u32,
    ) -> Vec<Keypoint> {
        if keypoints.is_empty() {
            return Vec::new();
        }

        let width = width as usize;
        let height = height as usize;
        let mut responses = vec![f32::NEG_INFINITY; width * height];
        for keypoint in keypoints {
            let x = keypoint.x as usize;
            let y = keypoint.y as usize;
            responses[y * width + x] = responses[y * width + x].max(keypoint.response);
        }

        keypoints
            .iter()
            .filter(|keypoint| {
                let x = keypoint.x as usize;
                let y = keypoint.y as usize;
                let min_x = x.saturating_sub(2);
                let max_x = (x + 2).min(width - 1);
                let min_y = y.saturating_sub(2);
                let max_y = (y + 2).min(height - 1);

                for other_y in min_y..=max_y {
                    for other_x in min_x..=max_x {
                        let dx = other_x.abs_diff(x);
                        let dy = other_y.abs_diff(y);
                        if dx * dx + dy * dy < 9
                            && responses[other_y * width + other_x] > keypoint.response
                        {
                            return false;
                        }
                    }
                }
                true
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    #[test]
    fn test_fast_detection() {
        // Create a simple image with a corner
        let mut image = GrayImage::new(50, 50);

        // Create a dark background
        for pixel in image.pixels_mut() {
            *pixel = Luma([30]);
        }

        // Create a bright square in corner - should detect corner at (25, 25)
        for y in 0..25 {
            for x in 0..25 {
                image.put_pixel(x, y, Luma([200]));
            }
        }

        let detector = FastDetector::new(30, true);
        let keypoints = detector.detect(&image);

        // Should detect corners
        assert!(!keypoints.is_empty(), "Should detect at least one corner");
    }

    #[test]
    fn small_images_return_no_features() {
        let detector = FastDetector::new(20, true);
        assert!(detector.detect(&GrayImage::new(6, 6)).is_empty());
    }
}
