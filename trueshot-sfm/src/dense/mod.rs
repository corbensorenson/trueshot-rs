//! PatchMatch Multi-View Stereo
//!
//! Dense depth estimation using PatchMatch algorithm.
//! Based on "PatchMatch Stereo" (Bleyer et al., BMVC 2011) and
//! "Massively Parallel Multiview Stereopsis" (Galliani et al., ICCV 2015).

use nalgebra as na;
use image::GrayImage;
use rand::Rng;

use crate::{CameraPose, CameraIntrinsics};

/// Depth map with confidence
#[derive(Clone, Debug)]
pub struct DepthMap {
    pub width: u32,
    pub height: u32,
    pub depths: Vec<f32>,
    pub confidences: Vec<f32>,
    pub normals: Vec<na::Vector3<f32>>,
}

impl DepthMap {
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width * height) as usize;
        Self {
            width,
            height,
            depths: vec![0.0; size],
            confidences: vec![0.0; size],
            normals: vec![na::Vector3::zeros(); size],
        }
    }
    
    pub fn get(&self, x: u32, y: u32) -> Option<(f32, f32, na::Vector3<f32>)> {
        if x < self.width && y < self.height {
            let idx = (y * self.width + x) as usize;
            Some((self.depths[idx], self.confidences[idx], self.normals[idx]))
        } else {
            None
        }
    }
    
    pub fn set(&mut self, x: u32, y: u32, depth: f32, confidence: f32, normal: na::Vector3<f32>) {
        if x < self.width && y < self.height {
            let idx = (y * self.width + x) as usize;
            self.depths[idx] = depth;
            self.confidences[idx] = confidence;
            self.normals[idx] = normal;
        }
    }
    
    /// Export depth map as 16-bit PNG
    pub fn export_png(&self, path: &std::path::Path) -> anyhow::Result<()> {
        use image::ImageBuffer;
        
        let max_depth = self.depths.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let min_depth = self.depths.iter().cloned().filter(|&d| d > 0.0).fold(f32::INFINITY, f32::min);
        
        let range = max_depth - min_depth;
        
        let img: ImageBuffer<image::Luma<u16>, Vec<u16>> = ImageBuffer::from_fn(
            self.width, self.height, |x, y| {
                let idx = (y * self.width + x) as usize;
                let d = self.depths[idx];
                let normalized = if d > 0.0 && range > 0.0 {
                    ((d - min_depth) / range * 65535.0) as u16
                } else {
                    0
                };
                image::Luma([normalized])
            }
        );
        
        img.save(path)?;
        Ok(())
    }
}

/// PatchMatch configuration
#[derive(Clone, Debug)]
pub struct PatchMatchConfig {
    /// Patch size (window radius)
    pub patch_radius: u32,
    /// Number of iterations
    pub num_iterations: u32,
    /// Depth range
    pub depth_min: f32,
    pub depth_max: f32,
    /// NCC threshold for matching
    pub ncc_threshold: f32,
    /// Number of random samples per pixel
    pub num_samples: u32,
    /// Geometric consistency threshold (pixels)
    pub geo_consistency_threshold: f32,
    /// Use bilateral filtering
    pub bilateral_filter: bool,
    /// Bilateral filter sigma (spatial)
    pub bilateral_sigma_spatial: f32,
    /// Bilateral filter sigma (depth)
    pub bilateral_sigma_depth: f32,
}

impl Default for PatchMatchConfig {
    fn default() -> Self {
        Self {
            patch_radius: 5,
            num_iterations: 3,
            depth_min: 0.1,
            depth_max: 100.0,
            ncc_threshold: 0.6,
            num_samples: 8,
            geo_consistency_threshold: 1.0,
            bilateral_filter: true,
            bilateral_sigma_spatial: 10.0,
            bilateral_sigma_depth: 0.05,
        }
    }
}

/// Reference image with source images for MVS
pub struct MvsInput<'a> {
    pub ref_image: &'a GrayImage,
    pub ref_pose: &'a CameraPose,
    pub ref_intrinsics: &'a CameraIntrinsics,
    pub src_images: Vec<&'a GrayImage>,
    pub src_poses: Vec<&'a CameraPose>,
    pub src_intrinsics: Vec<&'a CameraIntrinsics>,
}

/// Run PatchMatch stereo
pub fn patchmatch_stereo(input: &MvsInput, config: &PatchMatchConfig) -> DepthMap {
    let (width, height) = input.ref_image.dimensions();
    let mut depth_map = DepthMap::new(width, height);
    
    tracing::info!("Running PatchMatch stereo: {}x{}, {} source images", 
        width, height, input.src_images.len());
    
    if input.src_images.is_empty() {
        return depth_map;
    }
    
    // Initialize with random depths and normals
    initialize_random(&mut depth_map, config);
    
    // Iterative propagation and random refinement
    for iter in 0..config.num_iterations {
        let even_iter = iter % 2 == 0;
        
        // Spatial propagation (red-black or sweep pattern)
        propagate(&mut depth_map, input, config, even_iter);
        
        // Random refinement
        refine_random(&mut depth_map, input, config);
        
        tracing::debug!("PatchMatch iteration {}/{} complete", iter + 1, config.num_iterations);
    }
    
    // Post-processing
    if config.bilateral_filter {
        bilateral_depth_filter(&mut depth_map, config);
    }
    
    depth_map
}

fn initialize_random(depth_map: &mut DepthMap, config: &PatchMatchConfig) {
    let mut rng = rand::thread_rng();
    
    for y in 0..depth_map.height {
        for x in 0..depth_map.width {
            // Random depth
            let depth = rng.gen_range(config.depth_min..config.depth_max);
            
            // Random normal (fronto-parallel bias)
            let nx: f32 = rng.gen_range(-0.5..0.5);
            let ny: f32 = rng.gen_range(-0.5..0.5);
            let nz = (1.0 - nx * nx - ny * ny).max(0.1).sqrt();
            let normal = na::Vector3::new(nx, ny, nz).normalize();
            
            depth_map.set(x, y, depth, 0.0, normal);
        }
    }
}

fn propagate(depth_map: &mut DepthMap, input: &MvsInput, config: &PatchMatchConfig, forward: bool) {
    let (width, height) = (depth_map.width, depth_map.height);
    
    let (x_range, y_range): (Vec<u32>, Vec<u32>) = if forward {
        ((0..width).collect(), (0..height).collect())
    } else {
        ((0..width).rev().collect(), (0..height).rev().collect())
    };
    
    for &y in &y_range {
        for &x in &x_range {
            let current = depth_map.get(x, y).unwrap();
            let mut best_depth = current.0;
            let mut best_normal = current.2;
            let mut best_cost = compute_matching_cost(input, x, y, current.0, current.2, config);
            
            // Try neighbors
            let neighbors = if forward {
                vec![(x.wrapping_sub(1), y), (x, y.wrapping_sub(1))]
            } else {
                vec![(x + 1, y), (x, y + 1)]
            };
            
            for (nx, ny) in neighbors {
                if let Some((d, _, n)) = depth_map.get(nx, ny) {
                    let cost = compute_matching_cost(input, x, y, d, n, config);
                    if cost < best_cost {
                        best_cost = cost;
                        best_depth = d;
                        best_normal = n;
                    }
                }
            }
            
            depth_map.set(x, y, best_depth, 1.0 - best_cost.min(1.0), best_normal);
        }
    }
}

fn refine_random(depth_map: &mut DepthMap, input: &MvsInput, config: &PatchMatchConfig) {
    let mut rng = rand::thread_rng();
    let (width, height) = (depth_map.width, depth_map.height);
    
    for y in 0..height {
        for x in 0..width {
            let (current_depth, _current_conf, current_normal) = depth_map.get(x, y).unwrap();
            let current_cost = compute_matching_cost(input, x, y, current_depth, current_normal, config);
            
            let mut best_depth = current_depth;
            let mut best_normal = current_normal;
            let mut best_cost = current_cost;
            
            // Random samples with decreasing search radius
            let mut depth_range = (config.depth_max - config.depth_min) / 2.0;
            let mut normal_range = 0.5f32;
            
            for _ in 0..config.num_samples {
                // Random depth perturbation
                let delta_depth: f32 = rng.gen_range(-depth_range..depth_range);
                let new_depth = (current_depth + delta_depth).clamp(config.depth_min, config.depth_max);
                
                // Random normal perturbation
                let delta_nx: f32 = rng.gen_range(-normal_range..normal_range);
                let delta_ny: f32 = rng.gen_range(-normal_range..normal_range);
                let new_normal = na::Vector3::new(
                    current_normal.x + delta_nx,
                    current_normal.y + delta_ny,
                    current_normal.z,
                ).normalize();
                
                let cost = compute_matching_cost(input, x, y, new_depth, new_normal, config);
                
                if cost < best_cost {
                    best_cost = cost;
                    best_depth = new_depth;
                    best_normal = new_normal;
                }
                
                depth_range *= 0.5;
                normal_range *= 0.5;
            }
            
            depth_map.set(x, y, best_depth, 1.0 - best_cost.min(1.0), best_normal);
        }
    }
}

fn compute_matching_cost(
    input: &MvsInput,
    x: u32,
    y: u32,
    depth: f32,
    _normal: na::Vector3<f32>,
    config: &PatchMatchConfig,
) -> f32 {
    let patch_size = config.patch_radius as i32;
    
    // Compute 3D point from depth
    let k = input.ref_intrinsics;
    let x3d = (x as f64 - k.cx) * depth as f64 / k.fx;
    let y3d = (y as f64 - k.cy) * depth as f64 / k.fy;
    let point_cam = na::Point3::new(x3d, y3d, depth as f64);
    
    // Transform to world: R * p + t
    let point_world = na::Point3::from(
        input.ref_pose.rotation * point_cam.coords + input.ref_pose.translation
    );
    
    let mut total_cost = 0.0f32;
    let mut num_valid = 0;
    
    for (src_idx, src_img) in input.src_images.iter().enumerate() {
        let src_pose = input.src_poses[src_idx];
        let src_k = &input.src_intrinsics[src_idx];
        
        // Project to source image: R^-1 * (p - t)
        let point_src = na::Point3::from(
            src_pose.rotation.inverse() * (point_world.coords - src_pose.translation)
        );
        
        if point_src.z <= 0.0 {
            continue;
        }
        
        let src_x = (src_k.fx * point_src.x / point_src.z + src_k.cx) as i32;
        let src_y = (src_k.fy * point_src.y / point_src.z + src_k.cy) as i32;
        
        // Compute NCC cost
        let ncc = compute_ncc_patch(
            input.ref_image, x as i32, y as i32,
            src_img, src_x, src_y,
            patch_size,
        );
        
        if ncc.is_finite() {
            total_cost += 1.0 - ncc; // NCC is [-1, 1], cost is [0, 2]
            num_valid += 1;
        }
    }
    
    if num_valid > 0 {
        total_cost / num_valid as f32
    } else {
        f32::MAX
    }
}

fn compute_ncc_patch(
    ref_img: &GrayImage, ref_x: i32, ref_y: i32,
    src_img: &GrayImage, src_x: i32, src_y: i32,
    patch_radius: i32,
) -> f32 {
    let (ref_w, ref_h) = ref_img.dimensions();
    let (src_w, src_h) = src_img.dimensions();
    
    let mut sum_ref = 0.0f32;
    let mut sum_src = 0.0f32;
    let mut sum_ref_sq = 0.0f32;
    let mut sum_src_sq = 0.0f32;
    let mut sum_cross = 0.0f32;
    let mut count = 0.0f32;
    
    for dy in -patch_radius..=patch_radius {
        for dx in -patch_radius..=patch_radius {
            let rx = (ref_x + dx) as u32;
            let ry = (ref_y + dy) as u32;
            let sx = (src_x + dx) as u32;
            let sy = (src_y + dy) as u32;
            
            if rx < ref_w && ry < ref_h && sx < src_w && sy < src_h {
                let r = ref_img.get_pixel(rx, ry).0[0] as f32;
                let s = src_img.get_pixel(sx, sy).0[0] as f32;
                
                sum_ref += r;
                sum_src += s;
                sum_ref_sq += r * r;
                sum_src_sq += s * s;
                sum_cross += r * s;
                count += 1.0;
            }
        }
    }
    
    if count < 4.0 {
        return f32::NEG_INFINITY;
    }
    
    let mean_ref = sum_ref / count;
    let mean_src = sum_src / count;
    
    let var_ref = sum_ref_sq / count - mean_ref * mean_ref;
    let var_src = sum_src_sq / count - mean_src * mean_src;
    let cov = sum_cross / count - mean_ref * mean_src;
    
    let std_ref = var_ref.max(1e-6).sqrt();
    let std_src = var_src.max(1e-6).sqrt();
    
    cov / (std_ref * std_src)
}

fn bilateral_depth_filter(depth_map: &mut DepthMap, config: &PatchMatchConfig) {
    let (width, height) = (depth_map.width, depth_map.height);
    let mut new_depths = depth_map.depths.clone();
    
    let kernel_radius = (config.bilateral_sigma_spatial * 2.0).ceil() as i32;
    
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            let idx = (y as u32 * width + x as u32) as usize;
            let center_depth = depth_map.depths[idx];
            
            if center_depth <= 0.0 {
                continue;
            }
            
            let mut weight_sum = 0.0f32;
            let mut depth_sum = 0.0f32;
            
            for dy in -kernel_radius..=kernel_radius {
                for dx in -kernel_radius..=kernel_radius {
                    let nx = x + dx;
                    let ny = y + dy;
                    
                    if nx >= 0 && ny >= 0 && nx < width as i32 && ny < height as i32 {
                        let n_idx = (ny as u32 * width + nx as u32) as usize;
                        let neighbor_depth = depth_map.depths[n_idx];
                        
                        if neighbor_depth <= 0.0 {
                            continue;
                        }
                        
                        let spatial_dist_sq = (dx * dx + dy * dy) as f32;
                        let depth_diff_sq = (center_depth - neighbor_depth).powi(2);
                        
                        let spatial_weight = (-spatial_dist_sq / (2.0 * config.bilateral_sigma_spatial.powi(2))).exp();
                        let depth_weight = (-depth_diff_sq / (2.0 * config.bilateral_sigma_depth.powi(2) * center_depth.powi(2))).exp();
                        
                        let weight = spatial_weight * depth_weight;
                        weight_sum += weight;
                        depth_sum += weight * neighbor_depth;
                    }
                }
            }
            
            if weight_sum > 1e-6 {
                new_depths[idx] = depth_sum / weight_sum;
            }
        }
    }
    
    depth_map.depths = new_depths;
}

/// Fuse multiple depth maps into a point cloud
pub fn fuse_depth_maps(
    depth_maps: &[DepthMap],
    poses: &[CameraPose],
    intrinsics: &[CameraIntrinsics],
    consistency_threshold: f32,
    min_views: usize,
) -> Vec<(na::Point3<f64>, [u8; 3])> {
    tracing::info!("Fusing {} depth maps with min {} views", depth_maps.len(), min_views);
    
    let mut points = Vec::new();
    
    for (ref_idx, depth_map) in depth_maps.iter().enumerate() {
        let k = &intrinsics[ref_idx];
        let pose = &poses[ref_idx];
        
        for y in 0..depth_map.height {
            for x in 0..depth_map.width {
                let (depth, confidence, _normal) = depth_map.get(x, y).unwrap();
                
                if depth <= 0.0 || confidence < 0.5 {
                    continue;
                }
                
                // Unproject to 3D
                let x3d = (x as f64 - k.cx) * depth as f64 / k.fx;
                let y3d = (y as f64 - k.cy) * depth as f64 / k.fy;
                let point_cam = na::Point3::new(x3d, y3d, depth as f64);
                
                // Transform to world: R * p + t
                let point_world = na::Point3::from(
                    pose.rotation * point_cam.coords + pose.translation
                );
                
                // Check consistency with other views
                let mut consistent_views = 1;
                
                for (src_idx, src_depth_map) in depth_maps.iter().enumerate() {
                    if src_idx == ref_idx {
                        continue;
                    }
                    
                    let src_pose = &poses[src_idx];
                    let src_k = &intrinsics[src_idx];
                    
                    // Project to source: R^-1 * (p - t)
                    let point_src = na::Point3::from(
                        src_pose.rotation.inverse() * (point_world.coords - src_pose.translation)
                    );
                    
                    if point_src.z <= 0.0 {
                        continue;
                    }
                    
                    let src_x = (src_k.fx * point_src.x / point_src.z + src_k.cx) as u32;
                    let src_y = (src_k.fy * point_src.y / point_src.z + src_k.cy) as u32;
                    
                    if let Some((src_depth, src_conf, _)) = src_depth_map.get(src_x, src_y) {
                        if src_conf > 0.5 {
                            let depth_diff = (src_depth as f64 - point_src.z).abs();
                            if depth_diff < consistency_threshold as f64 {
                                consistent_views += 1;
                            }
                        }
                    }
                }
                
                if consistent_views >= min_views {
                    points.push((point_world, [128u8, 128, 128])); // Placeholder color
                }
            }
        }
    }
    
    tracing::info!("Fused {} points", points.len());
    points
}
