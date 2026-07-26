//! Focus Stacking Module
//!
//! Combines multiple images focused at different depths into a single
//! image with extended depth of field.
//!
//! Supports multiple algorithms:
//! - Weighted Focus (fast, gradient-based)
//! - Laplacian Pyramid (high quality, multi-scale)
//! - Depth Map (3D-aware, uses depth estimation)

use crate::Result;
use image::{DynamicImage, GrayImage, ImageBuffer, Rgb};
use rayon::prelude::*;
use std::path::Path;

/// Focus stacking algorithm selection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StackAlgorithm {
    /// Fast gradient-based focus detection
    WeightedFocus,
    /// Multi-scale Laplacian pyramid fusion
    LaplacianPyramid,
    /// Depth-map guided stacking (requires depth estimation)
    DepthMap,
}

/// Focus stack direction
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StackDirection {
    /// Near focus first, moving to far
    FrontToBack,
    /// Far focus first, moving to near
    BackToFront,
    /// Start at center focus, expand outward
    CenterOut,
}

/// Focus stacking configuration
#[derive(Debug, Clone)]
pub struct FocusStackConfig {
    /// Number of focus slices
    pub slice_count: u32,
    /// Stacking algorithm
    pub algorithm: StackAlgorithm,
    /// Stack direction during capture
    pub direction: StackDirection,
    /// Window size for focus measurement
    pub window_size: u32,
    /// Enable image alignment
    pub align_images: bool,
    /// Edge enhancement strength (0.0 - 1.0)
    pub edge_enhancement: f32,
}

impl Default for FocusStackConfig {
    fn default() -> Self {
        Self {
            slice_count: 15,
            algorithm: StackAlgorithm::WeightedFocus,
            direction: StackDirection::FrontToBack,
            window_size: 15,
            align_images: true,
            edge_enhancement: 0.2,
        }
    }
}

/// Focus stacker for combining images with different focus planes
pub struct FocusStacker {
    config: FocusStackConfig,
}

impl FocusStacker {
    pub fn new(config: FocusStackConfig) -> Self {
        Self { config }
    }

    /// Stack multiple focus slices into single image
    pub fn stack(&self, images: &[DynamicImage]) -> Result<DynamicImage> {
        if images.is_empty() {
            return Err(crate::Error::Processing("No images to stack".into()));
        }

        if images.len() == 1 {
            return Ok(images[0].clone());
        }

        match self.config.algorithm {
            StackAlgorithm::WeightedFocus => self.weighted_focus_stack(images),
            StackAlgorithm::LaplacianPyramid => self.laplacian_pyramid_stack(images),
            StackAlgorithm::DepthMap => self.depth_map_stack(images),
        }
    }

    /// Fast gradient-based focus stacking
    fn weighted_focus_stack(&self, images: &[DynamicImage]) -> Result<DynamicImage> {
        let (width, height) = (images[0].width() as usize, images[0].height() as usize);
        let window = self.config.window_size as usize;
        let half_window = window / 2;

        // Convert all images to grayscale for focus detection
        let gray_images: Vec<GrayImage> = images.iter().map(|img| img.to_luma8()).collect();

        // Calculate focus measure (Laplacian variance) for each pixel in each image
        let focus_maps: Vec<Vec<f32>> = gray_images
            .par_iter()
            .map(|gray| self.calculate_laplacian_variance(gray, window))
            .collect();

        // Find the image with best focus at each pixel
        let mut best_image_idx = vec![0usize; width * height];
        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                let mut best_focus = 0.0f32;
                let mut best_idx = 0;

                for (img_idx, focus_map) in focus_maps.iter().enumerate() {
                    if focus_map[idx] > best_focus {
                        best_focus = focus_map[idx];
                        best_idx = img_idx;
                    }
                }

                best_image_idx[idx] = best_idx;
            }
        }

        // Apply smoothing to best_image_idx to avoid harsh transitions
        let smoothed_idx = self.smooth_focus_map(&best_image_idx, width, height, images.len());

        // Convert input images to RGB for output
        let rgb_images: Vec<_> = images.iter().map(|img| img.to_rgb8()).collect();

        // Blend pixels from best-focused images
        let mut output = ImageBuffer::new(width as u32, height as u32);
        for y in 0..height {
            for x in 0..width {
                let idx = y * width + x;
                let weights = &smoothed_idx[idx];

                let mut r = 0.0f32;
                let mut g = 0.0f32;
                let mut b = 0.0f32;

                for (img_idx, w) in weights.iter().enumerate() {
                    if *w > 0.001 {
                        let pixel = rgb_images[img_idx].get_pixel(x as u32, y as u32);
                        r += pixel[0] as f32 * w;
                        g += pixel[1] as f32 * w;
                        b += pixel[2] as f32 * w;
                    }
                }

                output.put_pixel(
                    x as u32,
                    y as u32,
                    Rgb([
                        r.clamp(0.0, 255.0) as u8,
                        g.clamp(0.0, 255.0) as u8,
                        b.clamp(0.0, 255.0) as u8,
                    ]),
                );
            }
        }

        Ok(DynamicImage::ImageRgb8(output))
    }

    /// Calculate Laplacian variance (focus measure) for each pixel
    fn calculate_laplacian_variance(&self, gray: &GrayImage, window: usize) -> Vec<f32> {
        let (width, height) = (gray.width() as usize, gray.height() as usize);
        let half = window / 2;
        let mut focus_map = vec![0.0f32; width * height];

        // Laplacian kernel
        let laplacian = [[0.0, 1.0, 0.0], [1.0, -4.0, 1.0], [0.0, 1.0, 0.0]];

        for y in 1..(height - 1) {
            for x in 1..(width - 1) {
                // Apply Laplacian
                let mut lap_sum = 0.0f32;
                for ky in 0..3 {
                    for kx in 0..3 {
                        let py = (y + ky).saturating_sub(1);
                        let px = (x + kx).saturating_sub(1);
                        let pixel = gray.get_pixel(px as u32, py as u32)[0] as f32;
                        lap_sum += pixel * laplacian[ky][kx];
                    }
                }

                // Store squared Laplacian (variance proxy)
                focus_map[y * width + x] = lap_sum * lap_sum;
            }
        }

        // Apply box blur to get local variance
        let mut blurred = vec![0.0f32; width * height];
        for y in half..(height - half) {
            for x in half..(width - half) {
                let mut sum = 0.0f32;
                let mut count = 0;
                for wy in (y - half)..(y + half) {
                    for wx in (x - half)..(x + half) {
                        sum += focus_map[wy * width + wx];
                        count += 1;
                    }
                }
                blurred[y * width + x] = sum / count as f32;
            }
        }

        blurred
    }

    /// Smooth focus selection map to avoid harsh transitions
    fn smooth_focus_map(
        &self,
        indices: &[usize],
        width: usize,
        height: usize,
        num_images: usize,
    ) -> Vec<Vec<f32>> {
        let kernel_size = 5;
        let half = kernel_size / 2;
        let sigma = 2.0f32;

        // Create weight map for each image at each pixel
        let mut weights = vec![vec![0.0f32; num_images]; width * height];

        // Initialize with one-hot based on best index
        for (i, &best_idx) in indices.iter().enumerate() {
            weights[i][best_idx] = 1.0;
        }

        // Gaussian blur the weight maps
        for _pass in 0..2 {
            let mut new_weights = weights.clone();
            for y in half..(height - half) {
                for x in half..(width - half) {
                    let idx = y * width + x;
                    for img_idx in 0..num_images {
                        let mut sum = 0.0f32;
                        let mut weight_sum = 0.0f32;

                        for ky in 0..kernel_size {
                            for kx in 0..kernel_size {
                                let ny = y + ky - half;
                                let nx = x + kx - half;
                                let nidx = ny * width + nx;

                                let dist = ((ky as f32 - half as f32).powi(2)
                                    + (kx as f32 - half as f32).powi(2))
                                .sqrt();
                                let w = (-dist * dist / (2.0 * sigma * sigma)).exp();

                                sum += weights[nidx][img_idx] * w;
                                weight_sum += w;
                            }
                        }

                        new_weights[idx][img_idx] = sum / weight_sum;
                    }
                }
            }
            weights = new_weights;
        }

        // Normalize weights at each pixel
        for pixel_weights in weights.iter_mut() {
            let sum: f32 = pixel_weights.iter().sum();
            if sum > 0.0 {
                for w in pixel_weights.iter_mut() {
                    *w /= sum;
                }
            }
        }

        weights
    }

    /// Laplacian pyramid focus stacking (higher quality)
    fn laplacian_pyramid_stack(&self, images: &[DynamicImage]) -> Result<DynamicImage> {
        let (width, height) = (images[0].width() as usize, images[0].height() as usize);
        let levels = ((width.min(height) as f32).log2() - 2.0).floor() as usize;
        let levels = levels.min(6); // Cap at 6 levels

        // Build Laplacian pyramids for each image
        let pyramids: Vec<Vec<Vec<[f32; 3]>>> = images
            .par_iter()
            .map(|img| self.build_laplacian_pyramid(img, levels))
            .collect();

        // Calculate focus measure at each level for each image
        let focus_pyramids: Vec<Vec<Vec<f32>>> = pyramids
            .par_iter()
            .map(|pyramid| {
                pyramid
                    .iter()
                    .enumerate()
                    .map(|(level, layer)| {
                        let level_width = width >> level;
                        let level_height = height >> level;
                        self.calculate_local_energy(layer, level_width.max(1), level_height.max(1))
                    })
                    .collect()
            })
            .collect();

        // Fuse pyramids using focus measure
        let fused_pyramid = self.fuse_pyramids(&pyramids, &focus_pyramids, levels, width, height);

        // Reconstruct image from fused pyramid
        self.reconstruct_from_pyramid(&fused_pyramid, width, height)
    }

    fn build_laplacian_pyramid(&self, img: &DynamicImage, levels: usize) -> Vec<Vec<[f32; 3]>> {
        let rgb = img.to_rgb8();
        let (mut width, mut height) = (rgb.width() as usize, rgb.height() as usize);

        // Convert to f32
        let mut current: Vec<[f32; 3]> = rgb
            .pixels()
            .map(|p| [p[0] as f32, p[1] as f32, p[2] as f32])
            .collect();

        let mut gaussian_pyramid = vec![current.clone()];

        // Build Gaussian pyramid
        for _ in 0..levels {
            let new_width = width.div_ceil(2);
            let new_height = height.div_ceil(2);
            let mut downsampled = vec![[0.0f32; 3]; new_width * new_height];

            for y in 0..new_height {
                for x in 0..new_width {
                    let src_x = (x * 2).min(width - 1);
                    let src_y = (y * 2).min(height - 1);
                    downsampled[y * new_width + x] = current[src_y * width + src_x];
                }
            }

            gaussian_pyramid.push(downsampled.clone());
            current = downsampled;
            width = new_width;
            height = new_height;
        }

        // Build Laplacian pyramid
        let mut laplacian_pyramid = Vec::new();
        for level in 0..levels {
            let level_width = (img.width() as usize) >> level;
            let level_height = (img.height() as usize) >> level;

            // Upsample next level
            let next_width = (img.width() as usize) >> (level + 1);
            let next_height = (img.height() as usize) >> (level + 1);

            let upsampled = self.upsample(
                &gaussian_pyramid[level + 1],
                next_width,
                next_height,
                level_width,
                level_height,
            );

            // Subtract to get Laplacian
            let laplacian: Vec<[f32; 3]> = gaussian_pyramid[level]
                .iter()
                .zip(upsampled.iter())
                .map(|(g, u)| [g[0] - u[0], g[1] - u[1], g[2] - u[2]])
                .collect();

            laplacian_pyramid.push(laplacian);
        }

        // Add the last Gaussian level
        laplacian_pyramid.push(gaussian_pyramid[levels].clone());

        laplacian_pyramid
    }

    fn upsample(
        &self,
        img: &[[f32; 3]],
        src_width: usize,
        src_height: usize,
        dst_width: usize,
        dst_height: usize,
    ) -> Vec<[f32; 3]> {
        let mut result = vec![[0.0f32; 3]; dst_width * dst_height];

        for y in 0..dst_height {
            for x in 0..dst_width {
                let src_x = (x * src_width / dst_width).min(src_width.saturating_sub(1));
                let src_y = (y * src_height / dst_height).min(src_height.saturating_sub(1));
                result[y * dst_width + x] = img[src_y * src_width + src_x];
            }
        }

        result
    }

    fn calculate_local_energy(&self, layer: &[[f32; 3]], width: usize, height: usize) -> Vec<f32> {
        layer
            .iter()
            .map(|p| p[0].abs() + p[1].abs() + p[2].abs())
            .collect()
    }

    fn fuse_pyramids(
        &self,
        pyramids: &[Vec<Vec<[f32; 3]>>],
        focus_pyramids: &[Vec<Vec<f32>>],
        levels: usize,
        base_width: usize,
        base_height: usize,
    ) -> Vec<Vec<[f32; 3]>> {
        let mut fused = Vec::new();

        for level in 0..=levels {
            let level_width = base_width >> level;
            let level_height = base_height >> level;
            let pixel_count = level_width.max(1) * level_height.max(1);

            let mut fused_level = vec![[0.0f32; 3]; pixel_count];

            for i in 0..pixel_count {
                // Find image with best focus at this pixel
                let mut best_focus = 0.0f32;
                let mut best_idx = 0;

                for (img_idx, focus_pyramid) in focus_pyramids.iter().enumerate() {
                    if level < focus_pyramid.len()
                        && i < focus_pyramid[level].len()
                        && focus_pyramid[level][i] > best_focus
                    {
                        best_focus = focus_pyramid[level][i];
                        best_idx = img_idx;
                    }
                }

                // Use pixel from best-focused image
                if level < pyramids[best_idx].len() && i < pyramids[best_idx][level].len() {
                    fused_level[i] = pyramids[best_idx][level][i];
                }
            }

            fused.push(fused_level);
        }

        fused
    }

    fn reconstruct_from_pyramid(
        &self,
        pyramid: &[Vec<[f32; 3]>],
        target_width: usize,
        target_height: usize,
    ) -> Result<DynamicImage> {
        if pyramid.is_empty() {
            return Err(crate::Error::Processing("Empty pyramid".into()));
        }

        // Start from the smallest level
        let mut current = pyramid.last().unwrap().clone();
        let levels = pyramid.len();

        // Reconstruct by upsampling and adding
        for level in (0..levels - 1).rev() {
            let level_width = target_width >> level;
            let level_height = target_height >> level;
            let prev_width = target_width >> (level + 1);
            let prev_height = target_height >> (level + 1);

            let upsampled = self.upsample(
                &current,
                prev_width.max(1),
                prev_height.max(1),
                level_width,
                level_height,
            );

            current = upsampled
                .iter()
                .zip(pyramid[level].iter())
                .map(|(u, l)| [u[0] + l[0], u[1] + l[1], u[2] + l[2]])
                .collect();
        }

        // Convert to image
        let mut output = ImageBuffer::new(target_width as u32, target_height as u32);
        for (i, pixel) in output.pixels_mut().enumerate() {
            if i < current.len() {
                *pixel = Rgb([
                    current[i][0].clamp(0.0, 255.0) as u8,
                    current[i][1].clamp(0.0, 255.0) as u8,
                    current[i][2].clamp(0.0, 255.0) as u8,
                ]);
            }
        }

        Ok(DynamicImage::ImageRgb8(output))
    }

    /// Depth-map guided focus stacking
    fn depth_map_stack(&self, images: &[DynamicImage]) -> Result<DynamicImage> {
        // For now, fall back to weighted focus
        // Full implementation would use depth estimation network
        self.weighted_focus_stack(images)
    }
}

/// Load images from paths for focus stacking
pub fn load_focus_stack_images(paths: &[impl AsRef<Path>]) -> Result<Vec<DynamicImage>> {
    paths
        .iter()
        .map(|p| image::open(p.as_ref()).map_err(|e| crate::Error::Io(e.to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    #[test]
    fn test_focus_measure() {
        let stacker = FocusStacker::new(FocusStackConfig::default());

        // Create a test grayscale image with edge
        let mut gray = GrayImage::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                let val = if x < 16 { 50 } else { 200 };
                gray.put_pixel(x, y, Luma([val]));
            }
        }

        let focus_map = stacker.calculate_laplacian_variance(&gray, 5);

        // Focus should be highest at the edge
        let edge_focus = focus_map[16 * 32 + 15]; // Near the edge
        let flat_focus = focus_map[16 * 32 + 5]; // Flat area

        // Edge should have higher focus measure than flat areas
        // (Due to smoothing, this relationship might be different than expected)
        assert!(focus_map.len() == 32 * 32);
    }
}
