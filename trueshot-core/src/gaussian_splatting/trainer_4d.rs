//! 4D Gaussian Splatting Trainer
//!
//! Trains 4D Gaussian representations from synchronized multi-camera footage.
//! Optimizes spatial, temporal, and appearance parameters jointly.

use nalgebra as na;
use rayon::prelude::*;
use std::collections::HashMap;

use super::gaussian_4d::{
    Covariance4D, Dynamic4DScene, Gaussian4D, Scene4DMetadata, SlicedGaussian3D, SyncedCamera,
    TemporalSH,
};

/// Configuration for 4DGS training
#[derive(Clone, Debug)]
pub struct Training4DConfig {
    /// Total training iterations
    pub iterations: u32,
    /// Learning rate for positions
    pub lr_position: f32,
    /// Learning rate for covariance
    pub lr_covariance: f32,
    /// Learning rate for color/SH
    pub lr_color: f32,
    /// Learning rate for temporal parameters
    pub lr_temporal: f32,
    /// Learning rate for opacity
    pub lr_opacity: f32,
    /// When to start densification
    pub densify_from_iter: u32,
    /// When to stop densification
    pub densify_until_iter: u32,
    /// Densification interval
    pub densification_interval: u32,
    /// Gradient threshold for densification
    pub densify_grad_threshold: f32,
    /// Opacity threshold for pruning
    pub prune_opacity_threshold: f32,
    /// Weight for temporal smoothness loss
    pub temporal_smoothness_weight: f32,
    /// Weight for velocity consistency loss
    pub velocity_consistency_weight: f32,
    /// Number of random frames to sample per iteration
    pub frames_per_batch: usize,
    /// Lambda for L1 loss (vs SSIM)
    pub lambda_l1: f32,
}

impl Default for Training4DConfig {
    fn default() -> Self {
        Self {
            iterations: 30000,
            lr_position: 0.00016,
            lr_covariance: 0.001,
            lr_color: 0.0025,
            lr_temporal: 0.001,
            lr_opacity: 0.05,
            densify_from_iter: 500,
            densify_until_iter: 15000,
            densification_interval: 100,
            densify_grad_threshold: 0.0002,
            prune_opacity_threshold: 0.005,
            temporal_smoothness_weight: 0.1,
            velocity_consistency_weight: 0.05,
            frames_per_batch: 4,
            lambda_l1: 0.8,
        }
    }
}

/// Input data for 4DGS training
pub struct MultiCameraFootage {
    /// Frames indexed by (camera_id, frame_idx)
    frames: HashMap<(usize, usize), image::RgbImage>,
    /// Camera information
    cameras: Vec<SyncedCamera>,
    /// Total number of frames
    num_frames: usize,
    /// Duration in seconds
    duration: f32,
    /// Capture FPS
    fps: f32,
}

impl MultiCameraFootage {
    /// Create new footage container
    pub fn new(cameras: Vec<SyncedCamera>, num_frames: usize, fps: f32) -> Self {
        Self {
            frames: HashMap::new(),
            cameras,
            num_frames,
            duration: num_frames as f32 / fps,
            fps,
        }
    }

    /// Add a frame
    pub fn add_frame(&mut self, camera_id: usize, frame_idx: usize, image: image::RgbImage) {
        self.frames.insert((camera_id, frame_idx), image);
    }

    /// Get a frame
    pub fn get_frame(&self, camera_id: usize, frame_idx: usize) -> Option<&image::RgbImage> {
        self.frames.get(&(camera_id, frame_idx))
    }

    /// Get normalized time for frame index
    pub fn frame_to_time(&self, frame_idx: usize) -> f32 {
        if self.num_frames <= 1 {
            0.0
        } else {
            frame_idx as f32 / (self.num_frames - 1) as f32
        }
    }
}

/// 4DGS Trainer
pub struct Trainer4D {
    /// Current scene being trained
    scene: Dynamic4DScene,
    /// Training configuration
    config: Training4DConfig,
    /// Current iteration
    iteration: u32,
    /// Adam optimizer state for each parameter
    optimizer_state: OptimizerState4D,
    /// Loss history
    loss_history: Vec<f32>,
    /// Best loss achieved
    best_loss: f32,
}

/// Adam optimizer state for 4DGS parameters
struct OptimizerState4D {
    // First and second moments for positions (xyz + t)
    m_position: Vec<na::Vector4<f32>>,
    v_position: Vec<na::Vector4<f32>>,
    // For covariance
    m_covariance: Vec<[f32; 10]>,
    v_covariance: Vec<[f32; 10]>,
    // For color
    m_color: Vec<[f32; 3]>,
    v_color: Vec<[f32; 3]>,
    // For SH (degree 2, 9 coeffs per channel)
    m_sh: Vec<[[f32; 9]; 3]>,
    v_sh: Vec<[[f32; 9]; 3]>,
    // For opacity
    m_opacity: Vec<f32>,
    v_opacity: Vec<f32>,
    // For temporal SH coefficients (27 coeffs * 3 poly terms)
    m_temporal_sh: Vec<[[f32; 3]; 27]>,
    v_temporal_sh: Vec<[[f32; 3]; 27]>,
    // For velocity
    m_velocity: Vec<na::Vector3<f32>>,
    v_velocity: Vec<na::Vector3<f32>>,
    // Adam hyperparameters
    beta1: f32,
    beta2: f32,
    epsilon: f32,
    t: u32,
}

impl OptimizerState4D {
    fn new(num_gaussians: usize) -> Self {
        Self {
            m_position: vec![na::Vector4::zeros(); num_gaussians],
            v_position: vec![na::Vector4::zeros(); num_gaussians],
            m_covariance: vec![[0.0; 10]; num_gaussians],
            v_covariance: vec![[0.0; 10]; num_gaussians],
            m_color: vec![[0.0; 3]; num_gaussians],
            v_color: vec![[0.0; 3]; num_gaussians],
            m_sh: vec![[[0.0; 9]; 3]; num_gaussians],
            v_sh: vec![[[0.0; 9]; 3]; num_gaussians],
            m_opacity: vec![0.0; num_gaussians],
            v_opacity: vec![0.0; num_gaussians],
            m_temporal_sh: vec![[[0.0; 3]; 27]; num_gaussians],
            v_temporal_sh: vec![[[0.0; 3]; 27]; num_gaussians],
            m_velocity: vec![na::Vector3::zeros(); num_gaussians],
            v_velocity: vec![na::Vector3::zeros(); num_gaussians],
            beta1: 0.9,
            beta2: 0.999,
            epsilon: 1e-8,
            t: 0,
        }
    }

    fn step(&mut self) {
        self.t += 1;
    }

    fn resize(&mut self, new_size: usize) {
        self.m_position.resize(new_size, na::Vector4::zeros());
        self.v_position.resize(new_size, na::Vector4::zeros());
        self.m_covariance.resize(new_size, [0.0; 10]);
        self.v_covariance.resize(new_size, [0.0; 10]);
        self.m_color.resize(new_size, [0.0; 3]);
        self.v_color.resize(new_size, [0.0; 3]);
        self.m_sh.resize(new_size, [[0.0; 9]; 3]);
        self.v_sh.resize(new_size, [[0.0; 9]; 3]);
        self.m_opacity.resize(new_size, 0.0);
        self.v_opacity.resize(new_size, 0.0);
        self.m_temporal_sh.resize(new_size, [[0.0; 3]; 27]);
        self.v_temporal_sh.resize(new_size, [[0.0; 3]; 27]);
        self.m_velocity.resize(new_size, na::Vector3::zeros());
        self.v_velocity.resize(new_size, na::Vector3::zeros());
    }
}

impl Trainer4D {
    /// Create a new trainer from initial point cloud
    pub fn new(
        initial_points: &[(na::Point3<f32>, [f32; 3])],
        duration_seconds: f32,
        capture_fps: f32,
        config: Training4DConfig,
    ) -> Self {
        let gaussians: Vec<Gaussian4D> = initial_points
            .iter()
            .enumerate()
            .map(|(id, (pos, color))| Gaussian4D {
                id,
                center: na::Vector4::new(pos.x, pos.y, pos.z, 0.5),
                covariance: Covariance4D::default(),
                color: *color,
                sh_coeffs: [[0.0; 9]; 3],
                temporal_sh: TemporalSH::default(),
                opacity: 0.5,
                time_range: (0.0, 1.0),
                velocity: na::Vector3::zeros(),
            })
            .collect();

        let num_gaussians = gaussians.len();

        let scene = Dynamic4DScene {
            gaussians,
            duration_seconds,
            capture_fps,
            num_cameras: 0,
            cameras: Vec::new(),
            metadata: Scene4DMetadata::default(),
        };

        Self {
            scene,
            config,
            iteration: 0,
            optimizer_state: OptimizerState4D::new(num_gaussians),
            loss_history: Vec::new(),
            best_loss: f32::INFINITY,
        }
    }

    /// Create from synchronized multi-camera footage
    pub fn from_footage(
        footage: &MultiCameraFootage,
        initial_points: Vec<(na::Point3<f32>, [f32; 3])>,
        config: Training4DConfig,
    ) -> Self {
        let mut trainer = Self::new(&initial_points, footage.duration, footage.fps, config);

        trainer.scene.cameras = footage.cameras.clone();
        trainer.scene.num_cameras = footage.cameras.len();

        trainer
    }

    /// Run a single training iteration
    pub fn train_step(&mut self, footage: &MultiCameraFootage) -> f32 {
        self.iteration += 1;
        self.optimizer_state.step();

        // Sample random frames across time
        let frame_samples = self.sample_training_frames(footage);

        // Compute loss and gradients for each frame
        let mut total_loss = 0.0;
        let mut gradients = Gradients4D::new(self.scene.num_gaussians());

        for (camera_id, frame_idx, time) in frame_samples {
            if let Some(gt_image) = footage.get_frame(camera_id, frame_idx) {
                if let Some(camera) = footage.cameras.get(camera_id) {
                    // Slice scene at this time
                    let sliced = self.scene.slice_at_time(time * self.scene.duration_seconds);

                    // Render
                    let rendered = self.render_frame(&sliced, camera);

                    // Compute loss
                    let (loss, grads) =
                        self.compute_loss_and_gradients(&rendered, gt_image, &sliced, time, camera);

                    total_loss += loss;
                    gradients.accumulate(&grads);
                }
            }
        }

        total_loss /= self.config.frames_per_batch as f32;

        // Add temporal smoothness loss
        let temporal_loss = self.compute_temporal_smoothness_loss();
        total_loss += self.config.temporal_smoothness_weight * temporal_loss;

        // Apply gradients
        self.apply_gradients(&gradients);

        // Densification and pruning
        if self.should_densify() {
            self.densify(&gradients);
        }

        if self.iteration % 1000 == 0 {
            self.prune();
        }

        // Track loss
        self.loss_history.push(total_loss);
        if total_loss < self.best_loss {
            self.best_loss = total_loss;
        }

        total_loss
    }

    /// Sample frames for training batch
    fn sample_training_frames(&self, footage: &MultiCameraFootage) -> Vec<(usize, usize, f32)> {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        (0..self.config.frames_per_batch)
            .map(|_| {
                let camera_id = rng.gen_range(0..footage.cameras.len());
                let frame_idx = rng.gen_range(0..footage.num_frames);
                let time = footage.frame_to_time(frame_idx);
                (camera_id, frame_idx, time)
            })
            .collect()
    }

    /// Render a frame from sliced 3D Gaussians
    fn render_frame(
        &self,
        gaussians: &[SlicedGaussian3D],
        camera: &SyncedCamera,
    ) -> image::RgbImage {
        let width = camera.intrinsics.width;
        let height = camera.intrinsics.height;

        // CPU rendering with projected covariance splats
        let mut image = image::RgbImage::new(width, height);

        // Sort by depth
        let cam_pos = na::Point3::from(na::Vector3::from(camera.extrinsics.translation));
        let mut sorted_gaussians: Vec<_> = gaussians
            .iter()
            .map(|g| {
                let dist = (g.position - cam_pos).norm_squared();
                (g, dist)
            })
            .collect();
        sorted_gaussians.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        // Splat each Gaussian
        for (gaussian, _) in sorted_gaussians {
            self.splat_gaussian(&mut image, gaussian, camera);
        }

        image
    }

    /// Splat a single Gaussian onto the image
    fn splat_gaussian(
        &self,
        image: &mut image::RgbImage,
        gaussian: &SlicedGaussian3D,
        camera: &SyncedCamera,
    ) {
        let intr = &camera.intrinsics;

        // Project to 2D
        let rot = na::Matrix3::from_row_slice(&camera.extrinsics.rotation);
        let trans = na::Vector3::from(camera.extrinsics.translation);

        let p_cam = rot * gaussian.position.coords + trans;

        if p_cam.z <= 0.01 {
            return; // Behind camera
        }

        let fx = intr.fx.max(1e-6);
        let fy = intr.fy.max(1e-6);
        let cx = intr.cx;
        let cy = intr.cy;
        let inv_z = 1.0 / p_cam.z;
        let x = (p_cam.x * inv_z * fx + cx) as i32;
        let y = (p_cam.y * inv_z * fy + cy) as i32;

        // Project 3D covariance to 2D
        let cov = gaussian.covariance;
        let c00 = cov[(0, 0)];
        let c01 = cov[(0, 1)];
        let c02 = cov[(0, 2)];
        let c11 = cov[(1, 1)];
        let c12 = cov[(1, 2)];
        let c22 = cov[(2, 2)];

        let j00 = fx * inv_z;
        let j01 = 0.0f32;
        let j02 = -fx * p_cam.x * inv_z * inv_z;
        let j10 = 0.0f32;
        let j11 = fy * inv_z;
        let j12 = -fy * p_cam.y * inv_z * inv_z;

        let m00 = j00 * c00 + j01 * c01 + j02 * c02;
        let m01 = j00 * c01 + j01 * c11 + j02 * c12;
        let m02 = j00 * c02 + j01 * c12 + j02 * c22;

        let m10 = j10 * c00 + j11 * c01 + j12 * c02;
        let m11 = j10 * c01 + j11 * c11 + j12 * c12;
        let m12 = j10 * c02 + j11 * c12 + j12 * c22;

        let mut cov00 = m00 * j00 + m01 * j01 + m02 * j02;
        let cov01 = m00 * j10 + m01 * j11 + m02 * j12;
        let mut cov11 = m10 * j10 + m11 * j11 + m12 * j12;

        let min_variance = 0.5f32;
        cov00 += min_variance;
        cov11 += min_variance;

        let det = cov00 * cov11 - cov01 * cov01;
        if det <= 1e-6 {
            return;
        }
        let inv_det = 1.0 / det;
        let inv00 = cov11 * inv_det;
        let inv01 = -cov01 * inv_det;
        let inv11 = cov00 * inv_det;

        let sigma_x = cov00.max(0.0).sqrt();
        let sigma_y = cov11.max(0.0).sqrt();
        let radius = (3.0 * sigma_x.max(sigma_y)).ceil().clamp(1.0, 64.0) as i32;

        let view_dir = (-p_cam).normalize();
        let basis = eval_sh_basis_d2(view_dir);
        let use_sh = has_sh_coeffs_d2(&gaussian.sh_coeffs);
        let (color, clamp_mask) = if use_sh {
            let (_raw, color, mask) = eval_sh_color_d2(&gaussian.sh_coeffs, &basis);
            (color, mask)
        } else {
            (gaussian.color, [1.0f32; 3])
        };

        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let px = x + dx;
                let py = y + dy;

                if px >= 0 && px < image.width() as i32 && py >= 0 && py < image.height() as i32 {
                    let dx_f = dx as f32;
                    let dy_f = dy as f32;
                    let exponent = -0.5
                        * (inv00 * dx_f * dx_f + 2.0 * inv01 * dx_f * dy_f + inv11 * dy_f * dy_f);
                    let weight = exponent.exp();
                    let alpha = (gaussian.opacity * weight).clamp(0.0, 0.99);
                    if alpha < 0.01 {
                        continue;
                    }

                    let pixel = image.get_pixel_mut(px as u32, py as u32);
                    for c in 0..3 {
                        let old = pixel[c] as f32 / 255.0;
                        let target = color[c].clamp(0.0, 1.0) * clamp_mask[c];
                        let new = old * (1.0 - alpha) + target * alpha;
                        pixel[c] = (new.clamp(0.0, 1.0) * 255.0) as u8;
                    }
                }
            }
        }
    }

    /// Compute loss and gradients
    fn compute_loss_and_gradients(
        &self,
        rendered: &image::RgbImage,
        ground_truth: &image::RgbImage,
        sliced: &[SlicedGaussian3D],
        time: f32,
        camera: &SyncedCamera,
    ) -> (f32, Gradients4D) {
        let width = rendered.width() as usize;
        let height = rendered.height() as usize;
        let num_pixels = (width * height) as f32;

        let mut l1_loss = 0.0;

        for y in 0..height {
            for x in 0..width {
                let r_pixel = rendered.get_pixel(x as u32, y as u32);
                let gt_pixel = ground_truth.get_pixel(x as u32, y as u32);
                let mut err = 0.0f32;
                for c in 0..3 {
                    let diff = (r_pixel[c] as f32 - gt_pixel[c] as f32) / 255.0;
                    err += diff.abs();
                }
                err /= 3.0;
                l1_loss += err;
            }
        }
        if num_pixels > 0.0 {
            l1_loss /= num_pixels;
        }

        let mut gradients = Gradients4D::new(self.scene.num_gaussians());

        let rot = na::Matrix3::from_row_slice(&camera.extrinsics.rotation);
        let trans = na::Vector3::from(camera.extrinsics.translation);
        let fx = camera.intrinsics.fx.max(1e-6);
        let fy = camera.intrinsics.fy.max(1e-6);
        let cx = camera.intrinsics.cx;
        let cy = camera.intrinsics.cy;

        let t = time;
        let t2 = t * t;
        let temporal_basis = [1.0f32, t, t2];

        for g in sliced {
            if g.id >= gradients.position.len() {
                continue;
            }
            let p_cam = rot * g.position.coords + trans;
            if p_cam.z <= 0.01 {
                continue;
            }
            let u = p_cam.x / p_cam.z * fx + cx;
            let v = p_cam.y / p_cam.z * fy + cy;

            let cov2d = project_covariance_4d(&g.covariance, p_cam.x, p_cam.y, p_cam.z, fx, fy);
            let (inv_cov, mut radius) = match invert_cov2d_4d(&cov2d) {
                Some(result) => result,
                None => continue,
            };
            radius = radius.min(64.0);
            let radius_i = radius.max(1.0) as i32;

            let min_x = (u as i32 - radius_i).max(0);
            let max_x = (u as i32 + radius_i).min(width as i32 - 1);
            let min_y = (v as i32 - radius_i).max(0);
            let max_y = (v as i32 + radius_i).min(height as i32 - 1);
            if min_x > max_x || min_y > max_y {
                continue;
            }

            let view_dir = (-p_cam).normalize();
            let basis = eval_sh_basis_d2(view_dir);
            let use_sh = has_sh_coeffs_d2(&g.sh_coeffs);
            let (_raw_color, color, clamp_mask) = if use_sh {
                eval_sh_color_d2(&g.sh_coeffs, &basis)
            } else {
                ([g.color[0], g.color[1], g.color[2]], g.color, [1.0f32; 3])
            };

            let mut grad_cam = na::Vector3::zeros();
            let mut grad_opacity = 0.0f32;
            let mut grad_time = 0.0f32;
            let mut grad_tvar = 0.0f32;
            let mut grad_inv_cov = [0.0f32; 3];

            let dz = p_cam.z;
            let inv_z = 1.0 / dz;
            let inv_z2 = inv_z * inv_z;

            for py in min_y..=max_y {
                for px in min_x..=max_x {
                    let dx = px as f32 + 0.5 - u;
                    let dy = py as f32 + 0.5 - v;

                    let quad =
                        inv_cov[0] * dx * dx + 2.0 * inv_cov[1] * dx * dy + inv_cov[2] * dy * dy;
                    let power = -0.5 * quad;
                    if power > 0.0 {
                        continue;
                    }
                    let weight = power.exp();
                    let alpha = g.opacity * weight;
                    if alpha < 0.01 {
                        continue;
                    }

                    let rendered_px = rendered.get_pixel(px as u32, py as u32);
                    let gt_px = ground_truth.get_pixel(px as u32, py as u32);

                    let rendered_rgb = [
                        rendered_px[0] as f32 / 255.0,
                        rendered_px[1] as f32 / 255.0,
                        rendered_px[2] as f32 / 255.0,
                    ];
                    let gt_rgb = [
                        gt_px[0] as f32 / 255.0,
                        gt_px[1] as f32 / 255.0,
                        gt_px[2] as f32 / 255.0,
                    ];

                    let error = [
                        rendered_rgb[0] - gt_rgb[0],
                        rendered_rgb[1] - gt_rgb[1],
                        rendered_rgb[2] - gt_rgb[2],
                    ];
                    let d_rendered_d_alpha = [
                        color[0] - rendered_rgb[0],
                        color[1] - rendered_rgb[1],
                        color[2] - rendered_rgb[2],
                    ];
                    let d_loss_d_alpha = error[0] * d_rendered_d_alpha[0]
                        + error[1] * d_rendered_d_alpha[1]
                        + error[2] * d_rendered_d_alpha[2];

                    let d_loss_d_opacity = d_loss_d_alpha * weight;
                    grad_opacity += d_loss_d_opacity;

                    let t_var = g.temporal_var.max(1e-6);
                    let temporal_weight = g.temporal_weight;
                    let d_loss_d_temporal_weight = d_loss_d_opacity * g.base_opacity;
                    let d_weight_d_center = temporal_weight * (g.temporal_dt / t_var);
                    let d_weight_d_tvar =
                        temporal_weight * 0.5 * g.temporal_dt * g.temporal_dt / (t_var * t_var);
                    grad_time += d_loss_d_temporal_weight * d_weight_d_center;
                    grad_tvar += d_loss_d_temporal_weight * d_weight_d_tvar;

                    let d_loss_d_weight = d_loss_d_alpha * g.opacity;
                    let d_loss_d_quad = -0.5 * d_loss_d_weight * weight;
                    grad_inv_cov[0] += d_loss_d_quad * dx * dx;
                    grad_inv_cov[1] += d_loss_d_quad * 2.0 * dx * dy;
                    grad_inv_cov[2] += d_loss_d_quad * dy * dy;

                    let inv_qx = inv_cov[0] * dx + inv_cov[1] * dy;
                    let inv_qy = inv_cov[1] * dx + inv_cov[2] * dy;
                    let d_loss_du = g.opacity * weight * d_loss_d_alpha * inv_qx;
                    let d_loss_dv = g.opacity * weight * d_loss_d_alpha * inv_qy;

                    let du_dx = fx * inv_z;
                    let dv_dy = fy * inv_z;
                    let du_dz = -fx * p_cam.x * inv_z2;
                    let dv_dz = -fy * p_cam.y * inv_z2;

                    grad_cam.x += d_loss_du * du_dx;
                    grad_cam.y += d_loss_dv * dv_dy;
                    grad_cam.z += d_loss_du * du_dz + d_loss_dv * dv_dz;

                    let d_loss_d_color = [error[0] * alpha, error[1] * alpha, error[2] * alpha];

                    if use_sh {
                        for channel in 0..3 {
                            if clamp_mask[channel] == 0.0 {
                                continue;
                            }
                            for i in 0..9 {
                                gradients.sh[g.id][channel][i] +=
                                    d_loss_d_color[channel] * clamp_mask[channel] * basis[i];
                                let coeff_idx = channel * 9 + i;
                                for k in 0..3 {
                                    gradients.temporal_sh[g.id][coeff_idx][k] += d_loss_d_color
                                        [channel]
                                        * clamp_mask[channel]
                                        * basis[i]
                                        * temporal_basis[k];
                                }
                            }
                        }
                    } else {
                        for channel in 0..3 {
                            gradients.color[g.id][channel] += d_loss_d_color[channel];
                        }
                    }
                }
            }

            let world_grad = rot.transpose() * grad_cam;
            gradients.position[g.id].x += world_grad.x;
            gradients.position[g.id].y += world_grad.y;
            gradients.position[g.id].z += world_grad.z;
            gradients.position[g.id].w += grad_time;

            gradients.opacity[g.id] += grad_opacity;
            gradients.covariance[g.id][9] += grad_tvar;

            if grad_inv_cov[0].abs() > 0.0
                || grad_inv_cov[1].abs() > 0.0
                || grad_inv_cov[2].abs() > 0.0
            {
                let inv_cov_mat = na::Matrix2::new(inv_cov[0], inv_cov[1], inv_cov[1], inv_cov[2]);
                let grad_inv_mat = na::Matrix2::new(
                    grad_inv_cov[0],
                    grad_inv_cov[1],
                    grad_inv_cov[1],
                    grad_inv_cov[2],
                );
                let grad_cov2d = -inv_cov_mat * grad_inv_mat * inv_cov_mat;
                let j = na::Matrix2x3::new(
                    fx * inv_z,
                    0.0,
                    -fx * p_cam.x * inv_z2,
                    0.0,
                    fy * inv_z,
                    -fy * p_cam.y * inv_z2,
                );
                let grad_cov3d = j.transpose() * grad_cov2d * j;

                gradients.covariance[g.id][0] += grad_cov3d[(0, 0)];
                gradients.covariance[g.id][1] += grad_cov3d[(0, 1)];
                gradients.covariance[g.id][2] += grad_cov3d[(0, 2)];
                gradients.covariance[g.id][4] += grad_cov3d[(1, 1)];
                gradients.covariance[g.id][5] += grad_cov3d[(1, 2)];
                gradients.covariance[g.id][7] += grad_cov3d[(2, 2)];
            }
        }

        (l1_loss, gradients)
    }

    /// Compute temporal smoothness loss
    fn compute_temporal_smoothness_loss(&self) -> f32 {
        // Encourage smooth motion trajectories
        self.scene
            .gaussians
            .par_iter()
            .map(|g| {
                // Penalize large velocities
                let velocity_mag = g.velocity.norm_squared();
                // Penalize large temporal variance
                let t_var = g.covariance.temporal_variance();
                velocity_mag * 0.1 + (t_var - 0.1).abs() * 0.1
            })
            .sum::<f32>()
            / self.scene.num_gaussians() as f32
    }

    /// Apply gradients using Adam optimizer
    fn apply_gradients(&mut self, gradients: &Gradients4D) {
        let t = self.optimizer_state.t as i32;
        let beta1 = self.optimizer_state.beta1;
        let beta2 = self.optimizer_state.beta2;
        let eps = self.optimizer_state.epsilon;
        let bias_correction1 = 1.0 - beta1.powi(t);
        let bias_correction2 = 1.0 - beta2.powi(t);

        let n = self.scene.num_gaussians().min(gradients.position.len());
        for i in 0..n {
            // Position (xyz + t)
            let g = gradients.position[i];
            self.optimizer_state.m_position[i] =
                self.optimizer_state.m_position[i] * beta1 + g * (1.0 - beta1);
            self.optimizer_state.v_position[i] =
                self.optimizer_state.v_position[i] * beta2 + g.component_mul(&g) * (1.0 - beta2);
            let m_hat = self.optimizer_state.m_position[i] / bias_correction1;
            let v_hat = self.optimizer_state.v_position[i] / bias_correction2;
            let update = m_hat.component_div(&(v_hat.map(|x| x.sqrt()) + na::Vector4::repeat(eps)));

            let center = &mut self.scene.gaussians[i].center;
            center.x -= self.config.lr_position * update.x;
            center.y -= self.config.lr_position * update.y;
            center.z -= self.config.lr_position * update.z;
            center.w = (center.w - self.config.lr_temporal * update.w).clamp(0.0, 1.0);

            // Covariance
            let g_cov = gradients.covariance[i];
            for c in 0..10 {
                self.optimizer_state.m_covariance[i][c] =
                    self.optimizer_state.m_covariance[i][c] * beta1 + g_cov[c] * (1.0 - beta1);
                self.optimizer_state.v_covariance[i][c] = self.optimizer_state.v_covariance[i][c]
                    * beta2
                    + g_cov[c] * g_cov[c] * (1.0 - beta2);
                let m_hat = self.optimizer_state.m_covariance[i][c] / bias_correction1;
                let v_hat = self.optimizer_state.v_covariance[i][c] / bias_correction2;
                let update = m_hat / (v_hat.sqrt() + eps);
                self.scene.gaussians[i].covariance.values[c] -= self.config.lr_covariance * update;
            }
            // Keep diagonal variances positive
            for &idx in &[0usize, 4, 7, 9] {
                if self.scene.gaussians[i].covariance.values[idx] < 1e-6 {
                    self.scene.gaussians[i].covariance.values[idx] = 1e-6;
                }
            }

            // Color
            let g_color = gradients.color[i];
            for c in 0..3 {
                self.optimizer_state.m_color[i][c] =
                    self.optimizer_state.m_color[i][c] * beta1 + g_color[c] * (1.0 - beta1);
                self.optimizer_state.v_color[i][c] = self.optimizer_state.v_color[i][c] * beta2
                    + g_color[c] * g_color[c] * (1.0 - beta2);
                let m_hat = self.optimizer_state.m_color[i][c] / bias_correction1;
                let v_hat = self.optimizer_state.v_color[i][c] / bias_correction2;
                let update = m_hat / (v_hat.sqrt() + eps);
                self.scene.gaussians[i].color[c] = (self.scene.gaussians[i].color[c]
                    - self.config.lr_color * update)
                    .clamp(0.0, 1.0);
            }

            // SH coefficients (degree 2)
            let g_sh = &gradients.sh[i];
            for channel in 0..3 {
                for k in 0..9 {
                    let gk = g_sh[channel][k];
                    self.optimizer_state.m_sh[i][channel][k] =
                        self.optimizer_state.m_sh[i][channel][k] * beta1 + gk * (1.0 - beta1);
                    self.optimizer_state.v_sh[i][channel][k] =
                        self.optimizer_state.v_sh[i][channel][k] * beta2 + gk * gk * (1.0 - beta2);
                    let m_hat = self.optimizer_state.m_sh[i][channel][k] / bias_correction1;
                    let v_hat = self.optimizer_state.v_sh[i][channel][k] / bias_correction2;
                    let update = m_hat / (v_hat.sqrt() + eps);
                    self.scene.gaussians[i].sh_coeffs[channel][k] -= self.config.lr_color * update;
                }
            }

            // Opacity
            let g_opacity = gradients.opacity[i];
            self.optimizer_state.m_opacity[i] =
                self.optimizer_state.m_opacity[i] * beta1 + g_opacity * (1.0 - beta1);
            self.optimizer_state.v_opacity[i] =
                self.optimizer_state.v_opacity[i] * beta2 + g_opacity * g_opacity * (1.0 - beta2);
            let m_hat = self.optimizer_state.m_opacity[i] / bias_correction1;
            let v_hat = self.optimizer_state.v_opacity[i] / bias_correction2;
            let update = m_hat / (v_hat.sqrt() + eps);
            self.scene.gaussians[i].opacity =
                (self.scene.gaussians[i].opacity - self.config.lr_opacity * update).clamp(0.0, 1.0);

            // Temporal SH coefficients (degree 2)
            let g_temporal_sh = &gradients.temporal_sh[i];
            for coeff in 0..27 {
                for k in 0..3 {
                    let gk = g_temporal_sh[coeff][k];
                    self.optimizer_state.m_temporal_sh[i][coeff][k] =
                        self.optimizer_state.m_temporal_sh[i][coeff][k] * beta1
                            + gk * (1.0 - beta1);
                    self.optimizer_state.v_temporal_sh[i][coeff][k] =
                        self.optimizer_state.v_temporal_sh[i][coeff][k] * beta2
                            + gk * gk * (1.0 - beta2);
                    let m_hat = self.optimizer_state.m_temporal_sh[i][coeff][k] / bias_correction1;
                    let v_hat = self.optimizer_state.v_temporal_sh[i][coeff][k] / bias_correction2;
                    let update = m_hat / (v_hat.sqrt() + eps);
                    self.scene.gaussians[i].temporal_sh.coeffs[coeff][k] -=
                        self.config.lr_temporal * update;
                }
            }

            // Velocity
            let g_vel = gradients.velocity[i];
            self.optimizer_state.m_velocity[i] =
                self.optimizer_state.m_velocity[i] * beta1 + g_vel * (1.0 - beta1);
            self.optimizer_state.v_velocity[i] = self.optimizer_state.v_velocity[i] * beta2
                + g_vel.component_mul(&g_vel) * (1.0 - beta2);
            let m_hat = self.optimizer_state.m_velocity[i] / bias_correction1;
            let v_hat = self.optimizer_state.v_velocity[i] / bias_correction2;
            let update = m_hat.component_div(&(v_hat.map(|x| x.sqrt()) + na::Vector3::repeat(eps)));
            self.scene.gaussians[i].velocity -= self.config.lr_temporal * update;
        }
    }

    /// Check if we should densify
    fn should_densify(&self) -> bool {
        self.iteration >= self.config.densify_from_iter
            && self.iteration <= self.config.densify_until_iter
            && self.iteration % self.config.densification_interval == 0
    }

    /// Densify Gaussians based on gradients
    fn densify(&mut self, gradients: &Gradients4D) {
        let mut to_split = Vec::new();
        let mut to_clone = Vec::new();

        for (i, g) in self.scene.gaussians.iter().enumerate() {
            let grad = gradients
                .position
                .get(i)
                .map(|v| na::Vector3::new(v.x, v.y, v.z))
                .unwrap_or_else(na::Vector3::zeros);
            let grad_magnitude = grad.norm();
            if grad_magnitude > self.config.densify_grad_threshold {
                // Large Gaussians get split, small ones get cloned
                let scale = g.covariance.spatial_covariance().trace() / 3.0;
                if scale > 0.01 {
                    to_split.push(i);
                } else {
                    to_clone.push(i);
                }
            }
        }

        // Split large Gaussians
        for &idx in to_split.iter().rev() {
            let original = self.scene.gaussians[idx].clone();

            // Create two smaller Gaussians
            let offset = na::Vector3::new(0.01, 0.0, 0.0); // Simplified

            let mut g1 = original.clone();
            g1.center.x += offset.x;
            g1.id = self.scene.gaussians.len();

            let mut g2 = original;
            g2.center.x -= offset.x;

            self.scene.gaussians[idx] = g2;
            self.scene.gaussians.push(g1);
        }

        // Clone small Gaussians
        for &idx in to_clone.iter() {
            let mut clone = self.scene.gaussians[idx].clone();
            clone.id = self.scene.gaussians.len();
            self.scene.gaussians.push(clone);
        }

        // Resize optimizer state
        self.optimizer_state.resize(self.scene.num_gaussians());
    }

    /// Prune low-opacity Gaussians
    fn prune(&mut self) {
        let threshold = self.config.prune_opacity_threshold;
        let before = self.scene.num_gaussians();

        self.scene.gaussians.retain(|g| g.opacity > threshold);

        let after = self.scene.num_gaussians();
        if after != before {
            self.optimizer_state.resize(after);
        }
    }

    /// Get the trained scene
    pub fn get_scene(&self) -> &Dynamic4DScene {
        &self.scene
    }

    /// Get training progress
    pub fn progress(&self) -> f32 {
        self.iteration as f32 / self.config.iterations as f32
    }

    /// Get current loss
    pub fn current_loss(&self) -> f32 {
        self.loss_history.last().copied().unwrap_or(f32::INFINITY)
    }

    /// Get best loss
    pub fn best_loss(&self) -> f32 {
        self.best_loss
    }

    /// Check if training is complete
    pub fn is_complete(&self) -> bool {
        self.iteration >= self.config.iterations
    }

    /// Finalize and return the trained scene
    pub fn finalize(mut self) -> Dynamic4DScene {
        self.scene.metadata.training_iterations = self.iteration;
        self.scene.metadata.created_at = chrono::Utc::now().to_rfc3339();
        self.scene.metadata.software_version = env!("CARGO_PKG_VERSION").to_string();
        self.scene
    }
}

/// Gradients for 4DGS parameters
struct Gradients4D {
    position: Vec<na::Vector4<f32>>,
    covariance: Vec<[f32; 10]>,
    color: Vec<[f32; 3]>,
    sh: Vec<[[f32; 9]; 3]>,
    opacity: Vec<f32>,
    temporal_sh: Vec<[[f32; 3]; 27]>,
    velocity: Vec<na::Vector3<f32>>,
}

impl Gradients4D {
    fn new(num_gaussians: usize) -> Self {
        Self {
            position: vec![na::Vector4::zeros(); num_gaussians],
            covariance: vec![[0.0; 10]; num_gaussians],
            color: vec![[0.0; 3]; num_gaussians],
            sh: vec![[[0.0; 9]; 3]; num_gaussians],
            opacity: vec![0.0; num_gaussians],
            temporal_sh: vec![[[0.0; 3]; 27]; num_gaussians],
            velocity: vec![na::Vector3::zeros(); num_gaussians],
        }
    }

    fn accumulate(&mut self, other: &Gradients4D) {
        for (a, b) in self.position.iter_mut().zip(other.position.iter()) {
            *a += b;
        }
        for (a, b) in self.covariance.iter_mut().zip(other.covariance.iter()) {
            for i in 0..10 {
                a[i] += b[i];
            }
        }
        for (a, b) in self.color.iter_mut().zip(other.color.iter()) {
            for i in 0..3 {
                a[i] += b[i];
            }
        }
        for (a, b) in self.sh.iter_mut().zip(other.sh.iter()) {
            for channel in 0..3 {
                for i in 0..9 {
                    a[channel][i] += b[channel][i];
                }
            }
        }
        for (a, b) in self.opacity.iter_mut().zip(other.opacity.iter()) {
            *a += b;
        }
        for (a, b) in self.temporal_sh.iter_mut().zip(other.temporal_sh.iter()) {
            for coeff in 0..27 {
                for k in 0..3 {
                    a[coeff][k] += b[coeff][k];
                }
            }
        }
        for (a, b) in self.velocity.iter_mut().zip(other.velocity.iter()) {
            *a += b;
        }
    }
}

fn has_sh_coeffs_d2(coeffs: &[[f32; 9]; 3]) -> bool {
    coeffs
        .iter()
        .any(|channel| channel.iter().any(|v| v.abs() > 1e-6))
}

fn eval_sh_basis_d2(view_dir: na::Vector3<f32>) -> [f32; 9] {
    let mut dir = view_dir;
    let norm = dir.norm();
    if norm > 1e-6 {
        dir /= norm;
    } else {
        dir = na::Vector3::new(0.0, 0.0, 1.0);
    }
    let x = dir.x;
    let y = dir.y;
    let z = dir.z;
    let xx = x * x;
    let yy = y * y;
    let zz = z * z;
    let xy = x * y;
    let yz = y * z;
    let xz = x * z;

    let c0 = 0.2820947918f32;
    let c1 = 0.4886025119f32;
    let c2_0 = 1.0925484306f32;
    let c2_1 = 0.3153915653f32;
    let c2_2 = 0.5462742153f32;

    [
        c0,
        -c1 * y,
        c1 * z,
        -c1 * x,
        c2_0 * xy,
        -c2_0 * yz,
        c2_1 * (3.0 * zz - 1.0),
        -c2_0 * xz,
        c2_2 * (xx - yy),
    ]
}

fn eval_sh_color_d2(coeffs: &[[f32; 9]; 3], basis: &[f32; 9]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let mut raw = [0.0f32; 3];
    let mut color = [0.0f32; 3];
    let mut mask = [1.0f32; 3];

    for channel in 0..3 {
        let mut accum = 0.0f32;
        for i in 0..9 {
            accum += coeffs[channel][i] * basis[i];
        }
        let value = accum + 0.5;
        raw[channel] = value;
        if value <= 0.0 {
            color[channel] = 0.0;
            mask[channel] = 0.0;
        } else if value >= 1.0 {
            color[channel] = 1.0;
            mask[channel] = 0.0;
        } else {
            color[channel] = value;
        }
    }

    (raw, color, mask)
}

fn project_covariance_4d(
    cov3d: &na::Matrix3<f32>,
    x: f32,
    y: f32,
    z: f32,
    fx: f32,
    fy: f32,
) -> [f32; 3] {
    let inv_z = 1.0 / z;
    let j00 = fx * inv_z;
    let j01 = 0.0f32;
    let j02 = -fx * x * inv_z * inv_z;
    let j10 = 0.0f32;
    let j11 = fy * inv_z;
    let j12 = -fy * y * inv_z * inv_z;

    let c00 = cov3d[(0, 0)];
    let c01 = cov3d[(0, 1)];
    let c02 = cov3d[(0, 2)];
    let c11 = cov3d[(1, 1)];
    let c12 = cov3d[(1, 2)];
    let c22 = cov3d[(2, 2)];

    let m00 = j00 * c00 + j01 * c01 + j02 * c02;
    let m01 = j00 * c01 + j01 * c11 + j02 * c12;
    let m02 = j00 * c02 + j01 * c12 + j02 * c22;

    let m10 = j10 * c00 + j11 * c01 + j12 * c02;
    let m11 = j10 * c01 + j11 * c11 + j12 * c12;
    let m12 = j10 * c02 + j11 * c12 + j12 * c22;

    let mut cov00 = m00 * j00 + m01 * j01 + m02 * j02;
    let cov01 = m00 * j10 + m01 * j11 + m02 * j12;
    let mut cov11 = m10 * j10 + m11 * j11 + m12 * j12;

    let min_variance = 0.5f32;
    cov00 += min_variance;
    cov11 += min_variance;

    [cov00, cov01, cov11]
}

fn invert_cov2d_4d(cov: &[f32; 3]) -> Option<([f32; 3], f32)> {
    let a = cov[0];
    let b = cov[1];
    let c = cov[2];
    let det = a * c - b * b;
    if det <= 1e-6 {
        return None;
    }
    let inv_det = 1.0 / det;
    let inv = [c * inv_det, -b * inv_det, a * inv_det];

    let trace = a + c;
    let temp = (trace * trace * 0.25 - det).max(0.0).sqrt();
    let lambda_max = trace * 0.5 + temp;
    let radius = (lambda_max.max(1e-4)).sqrt() * 3.0;
    Some((inv, radius))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trainer_creation() {
        let points = vec![
            (na::Point3::new(0.0, 0.0, 0.0), [1.0, 0.0, 0.0]),
            (na::Point3::new(1.0, 0.0, 0.0), [0.0, 1.0, 0.0]),
        ];

        let trainer = Trainer4D::new(&points, 5.0, 30.0, Training4DConfig::default());
        assert_eq!(trainer.scene.num_gaussians(), 2);
        assert_eq!(trainer.iteration, 0);
    }

    #[test]
    fn test_config_defaults() {
        let config = Training4DConfig::default();
        assert_eq!(config.iterations, 30000);
        assert!(config.lr_position > 0.0);
        assert!(config.temporal_smoothness_weight > 0.0);
    }
}
