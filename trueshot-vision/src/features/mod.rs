//! Native Feature Detection - TrueShot's Own Implementation
//!
//! Provides feature detection without OpenCV dependency.
//! Implements FAST corner detection and BRIEF-like binary descriptors.

pub mod brief;
pub mod fast;
pub mod keypoint;

pub use brief::BriefDescriptor;
pub use fast::FastDetector;
pub use keypoint::Keypoint;

use image::{GrayImage, ImageBuffer, Luma};

/// Unified feature with keypoint and descriptor
#[derive(Debug, Clone)]
pub struct NativeFeature {
    pub keypoint: Keypoint,
    pub descriptor: Vec<u8>, // 32 bytes = 256 bits for BRIEF
}

/// Native feature extractor - no OpenCV required
pub struct NativeFeatureExtractor {
    fast_detector: FastDetector,
    brief_extractor: BriefDescriptor,
    max_features: usize,
}

impl NativeFeatureExtractor {
    pub fn new(max_features: usize) -> Self {
        Self {
            fast_detector: FastDetector::new(20, true), // threshold=20, nonmax suppression
            brief_extractor: BriefDescriptor::new(),
            max_features,
        }
    }

    /// Detect features in a grayscale image
    pub fn detect(&self, image: &GrayImage) -> Vec<NativeFeature> {
        // 1. Detect FAST corners
        let mut keypoints = self.fast_detector.detect(image);

        // 2. Sort by response and take top N
        keypoints.sort_by(|a, b| b.response.partial_cmp(&a.response).unwrap());
        keypoints.truncate(self.max_features);

        // 3. Compute BRIEF descriptors
        let mut features = Vec::with_capacity(keypoints.len());
        for kp in keypoints {
            if let Some(descriptor) = self.brief_extractor.compute(image, &kp) {
                features.push(NativeFeature {
                    keypoint: kp,
                    descriptor,
                });
            }
        }

        features
    }

    /// Detect features with multi-scale pyramid
    pub fn detect_multiscale(&self, image: &GrayImage, levels: usize) -> Vec<NativeFeature> {
        let mut all_features = Vec::new();
        let mut current = image.clone();
        let mut scale = 1.0f32;

        for level in 0..levels {
            let mut features = self.detect(&current);

            // Adjust coordinates for scale
            for f in &mut features {
                f.keypoint.x *= scale;
                f.keypoint.y *= scale;
                f.keypoint.octave = level as i32;
            }

            all_features.extend(features);

            // Downsample for next level
            if level < levels - 1 {
                current = downsample_image(&current);
                scale *= 2.0;
            }
        }

        // Sort by response and limit
        all_features.sort_by(|a, b| {
            b.keypoint
                .response
                .partial_cmp(&a.keypoint.response)
                .unwrap()
        });
        all_features.truncate(self.max_features);

        all_features
    }
}

/// Simple 2x downsampling
fn downsample_image(image: &GrayImage) -> GrayImage {
    let (w, h) = image.dimensions();
    let new_w = w / 2;
    let new_h = h / 2;

    ImageBuffer::from_fn(new_w, new_h, |x, y| {
        let x2 = x * 2;
        let y2 = y * 2;

        // Simple 2x2 average
        let sum = image.get_pixel(x2, y2)[0] as u32
            + image.get_pixel((x2 + 1).min(w - 1), y2)[0] as u32
            + image.get_pixel(x2, (y2 + 1).min(h - 1))[0] as u32
            + image.get_pixel((x2 + 1).min(w - 1), (y2 + 1).min(h - 1))[0] as u32;

        Luma([(sum / 4) as u8])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feature_extractor() {
        // Create a simple test image with a corner
        let mut image = GrayImage::new(100, 100);

        // Draw a corner pattern
        for y in 0..50 {
            for x in 0..50 {
                image.put_pixel(x, y, Luma([255]));
            }
        }

        let extractor = NativeFeatureExtractor::new(100);
        let features = extractor.detect(&image);

        // Should detect at least one corner
        assert!(!features.is_empty(), "Should detect corners");
    }
}
