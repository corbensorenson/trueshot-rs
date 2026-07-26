//! Native 3D Gaussian Splatting - TrueShot's Own Implementation
//! 
//! Provides 3DGS training without external Python/CUDA dependencies.
//! Uses WGPU for GPU-accelerated rasterization.

pub mod gaussian;
pub mod optimizer;
pub mod rasterizer;
pub mod rasterizer_4d;
pub mod mip;
pub mod asg;
pub mod gs2mesh;
pub mod gaussian_4d;
pub mod trainer_4d;
pub mod deformation;
pub mod spatial_audio;
pub mod splat_edit;

// Re-exports
pub use gaussian::{Gaussian3D, GaussianCloud, SH_COEFFS_PER_CHANNEL, SH_COEFFS_TOTAL};
pub use optimizer::AdamOptimizer;
pub use gaussian_4d::{Gaussian4D, Dynamic4DScene, SlicedGaussian3D};
pub use trainer_4d::{Trainer4D, Training4DConfig};
pub use rasterizer_4d::{GpuRasterizer4D, Raster4DConfig, RenderedFrame4D};

use std::path::PathBuf;
use anyhow::Result;
use nalgebra as na;
#[cfg(feature = "wgpu")]
use std::sync::Arc;
#[cfg(feature = "wgpu")]
use wgpu;
#[cfg(feature = "wgpu")]
use self::rasterizer::{CameraUniforms, GaussianGradientsGpu, GpuRasterizer, RasterizerConfig};

/// 3DGS Training configuration
#[derive(Debug, Clone)]
pub struct TrainingConfig {
    /// Number of training iterations
    pub iterations: usize,
    /// Initial learning rate for positions
    pub lr_position: f32,
    /// Initial learning rate for colors (spherical harmonics)
    pub lr_color: f32,
    /// Initial learning rate for opacity
    pub lr_opacity: f32,
    /// Initial learning rate for scaling
    pub lr_scale: f32,
    /// Initial learning rate for rotation
    pub lr_rotation: f32,
    /// Densification interval
    pub densify_interval: usize,
    /// Gradient threshold for densification
    pub densify_grad_threshold: f32,
    /// Opacity threshold for pruning
    pub prune_opacity_threshold: f32,
    /// SSIM loss weight (vs L1)
    pub ssim_weight: f32,
    /// Enable GPU gradient computation (uses GPU for SH/opacity/scale/rotation)
    pub use_gpu_gradients: bool,
    /// Enable GPU/CPU gradient parity checks
    pub gpu_parity_check: bool,
    /// Parity check interval (iterations)
    pub gpu_parity_interval: usize,
    /// Max relative L2 error before warning
    pub gpu_parity_tolerance: f32,
}

impl Default for TrainingConfig {
    fn default() -> Self {
        Self {
            iterations: 30000,
            lr_position: 0.00016,
            lr_color: 0.0025,
            lr_opacity: 0.05,
            lr_scale: 0.005,
            lr_rotation: 0.001,
            densify_interval: 100,
            densify_grad_threshold: 0.0002,
            prune_opacity_threshold: 0.005,
            ssim_weight: 0.2,
            use_gpu_gradients: false,
            gpu_parity_check: false,
            gpu_parity_interval: 100,
            gpu_parity_tolerance: 0.35,
        }
    }
}

/// Camera for 3DGS rendering
#[derive(Debug, Clone)]
pub struct Camera {
    /// Camera-to-world transform
    pub transform: na::Matrix4<f32>,
    /// Intrinsic matrix
    pub intrinsics: na::Matrix3<f32>,
    /// Image width
    pub width: u32,
    /// Image height
    pub height: u32,
    /// Path to ground truth image
    pub image_path: PathBuf,
}

/// Native 3DGS Trainer
pub struct GaussianSplatTrainer {
    config: TrainingConfig,
    gaussians: GaussianCloud,
    optimizer: AdamOptimizer,
    cameras: Vec<Camera>,
    iteration: usize,
    #[cfg(feature = "wgpu")]
    gpu_state: Option<GpuGradientState>,
}

#[cfg(feature = "wgpu")]
struct GpuGradientState {
    rasterizer: GpuRasterizer,
    max_gaussians: u32,
    width: u32,
    height: u32,
}

impl GaussianSplatTrainer {
    /// Create trainer from initial point cloud and cameras
    pub fn new(
        initial_points: &[(na::Point3<f32>, [u8; 3])],  // positions and colors
        cameras: Vec<Camera>,
        config: TrainingConfig,
    ) -> Self {
        // Initialize Gaussians from points
        let gaussians = GaussianCloud::from_points(initial_points);
        
        // Create optimizer
        let optimizer = AdamOptimizer::new(
            gaussians.num_gaussians(),
            config.lr_position,
            config.lr_color,
        );

        Self {
            config,
            gaussians,
            optimizer,
            cameras,
            iteration: 0,
            #[cfg(feature = "wgpu")]
            gpu_state: None,
        }
    }

    /// Run one training step
    pub fn step(&mut self) -> Result<f32> {
        self.iteration += 1;

        // Select random camera
        let camera_idx = self.iteration % self.cameras.len();
        let camera = self.cameras[camera_idx].clone();

        // Forward pass: Render image
        let rendered = self.gaussians.render(&camera)?;

        // Load ground truth image
        let ground_truth = image::open(&camera.image_path)?
            .to_rgb8();

        // Compute loss (L1 + SSIM)
        let l1_loss = compute_l1_loss(&rendered, &ground_truth);
        let ssim_loss = compute_ssim_loss(&rendered, &ground_truth);
        let loss = (1.0 - self.config.ssim_weight) * l1_loss 
                 + self.config.ssim_weight * (1.0 - ssim_loss);

        // Backward pass: Compute gradients (image-space splat derivatives)
        let gradients = self.compute_gradients(&rendered, &ground_truth, &camera)?;

        // Update Gaussians using Adam
        self.optimizer.step(&mut self.gaussians, &gradients);

        // Densification and pruning
        if self.iteration % self.config.densify_interval == 0 {
            self.densify_and_prune(&gradients);
        }

        Ok(loss)
    }

    /// Compute gradients for Gaussian parameters
    fn compute_gradients(
        &mut self,
        rendered: &image::RgbImage,
        ground_truth: &image::RgbImage,
        camera: &Camera,
    ) -> Result<GaussianGradients> {
        let (position_grad, opacity_grad, scale_grad, rotation_grad, sh_grad) =
            self.gaussians.compute_image_gradients(camera, rendered, ground_truth);
        let mut gradients = GaussianGradients::new(self.gaussians.num_gaussians());
        gradients.position_grad = position_grad;
        gradients.opacity_grad = opacity_grad;
        gradients.scale_grad = scale_grad;
        gradients.rotation_grad = rotation_grad;
        gradients.sh_grad = sh_grad;

        #[cfg(feature = "wgpu")]
        {
            if self.config.use_gpu_gradients || self.config.gpu_parity_check {
                if let Some(gpu_gradients) = self.compute_gpu_gradients(camera, ground_truth)? {
                    let do_parity = self.config.gpu_parity_check
                        && self.config.gpu_parity_interval > 0
                        && self.iteration % self.config.gpu_parity_interval == 0;
                    if do_parity {
                        self.log_gpu_parity(&gradients, &gpu_gradients);
                    }
                    if self.config.use_gpu_gradients {
                        gradients.position_grad = gpu_gradients.position_grad;
                        gradients.opacity_grad = gpu_gradients.opacity_grad;
                        gradients.scale_grad = gpu_gradients.scale_grad;
                        gradients.rotation_grad = gpu_gradients.rotation_grad;
                        gradients.sh_grad = gpu_gradients.sh_grad;
                    }
                }
            }
        }

        Ok(gradients)
    }

    /// Densify high-gradient regions and prune transparent Gaussians
    fn densify_and_prune(&mut self, gradients: &GaussianGradients) {
        let mut to_clone = Vec::new();
        let mut to_split = Vec::new();
        let mut to_remove = Vec::new();

        for i in 0..self.gaussians.num_gaussians() {
            let grad_magnitude = gradients.position_grad[i].norm();
            let opacity = self.gaussians.opacity(i);
            let scale = self.gaussians.scale(i).norm();

            // Prune low-opacity Gaussians
            if opacity < self.config.prune_opacity_threshold {
                to_remove.push(i);
                continue;
            }

            // Densify based on gradient
            if grad_magnitude > self.config.densify_grad_threshold {
                if scale > 0.01 {
                    // Large Gaussian with high gradient -> split
                    to_split.push(i);
                } else {
                    // Small Gaussian with high gradient -> clone
                    to_clone.push(i);
                }
            }
        }

        // Apply densification
        self.gaussians.clone_gaussians(&to_clone);
        self.gaussians.split_gaussians(&to_split);
        self.gaussians.remove_gaussians(&to_remove);

        // Update optimizer for new Gaussian count
        self.optimizer.resize(self.gaussians.num_gaussians());
    }

    /// Export trained model to PLY
    pub fn export_ply(&self, path: &PathBuf) -> Result<()> {
        self.gaussians.export_ply(path)
    }

    /// Export trained model to .splat
    pub fn export_splat(&self, path: &PathBuf) -> Result<()> {
        self.gaussians.export_splat(path)
    }

    /// Export trained model to .spz
    pub fn export_spz(&self, path: &PathBuf) -> Result<()> {
        self.gaussians.export_spz(path)
    }

    /// Get current iteration
    pub fn iteration(&self) -> usize {
        self.iteration
    }

    /// Get number of Gaussians
    pub fn num_gaussians(&self) -> usize {
        self.gaussians.num_gaussians()
    }
}

/// Gradients for Gaussian parameters
pub struct GaussianGradients {
    pub position_grad: Vec<na::Vector3<f32>>,
    pub rotation_grad: Vec<na::Vector4<f32>>,
    pub scale_grad: Vec<na::Vector3<f32>>,
    pub opacity_grad: Vec<f32>,
    pub sh_grad: Vec<Vec<f32>>,
}

impl GaussianGradients {
    pub fn new(n: usize) -> Self {
        Self {
            position_grad: vec![na::Vector3::zeros(); n],
            rotation_grad: vec![na::Vector4::zeros(); n],
            scale_grad: vec![na::Vector3::zeros(); n],
            opacity_grad: vec![0.0; n],
            sh_grad: vec![vec![0.0; SH_COEFFS_TOTAL]; n], // 25 coeffs * 3 channels
        }
    }
}

#[cfg(feature = "wgpu")]
impl GaussianSplatTrainer {
    fn compute_gpu_gradients(
        &mut self,
        camera: &Camera,
        ground_truth: &image::RgbImage,
    ) -> Result<Option<GaussianGradients>> {
        if !self.ensure_gpu_state(camera)? {
            return Ok(None);
        }
        let state = match self.gpu_state.as_mut() {
            Some(state) => state,
            None => return Ok(None),
        };

        if ground_truth.width() != camera.width || ground_truth.height() != camera.height {
            tracing::warn!("GPU gradients skipped: ground truth size mismatch");
            return Ok(None);
        }

        let gpu_gaussians = self.gaussians.to_gpu_gaussians();
        state.rasterizer.upload_gaussians(&gpu_gaussians);
        state.rasterizer.set_camera(&camera_to_uniform(camera));
        state.rasterizer.render()?;

        let rgba = rgb_to_rgba(ground_truth);
        state.rasterizer.upload_ground_truth(&rgba);
        state.rasterizer.compute_gradients()?;

        let gpu_gradients = pollster::block_on(state.rasterizer.read_gradients())?;
        Ok(Some(convert_gpu_gradients(&gpu_gradients)))
    }

    fn ensure_gpu_state(&mut self, camera: &Camera) -> Result<bool> {
        let required = self.gaussians.num_gaussians() as u32;
        let width = camera.width;
        let height = camera.height;

        if let Some(state) = &self.gpu_state {
            if state.width == width && state.height == height && state.max_gaussians >= required {
                return Ok(true);
            }
        }

        let Some((device, queue)) = init_wgpu_context()? else {
            tracing::warn!("GPU gradients disabled: no GPU context");
            self.gpu_state = None;
            return Ok(false);
        };

        let mut max_gaussians = required.max(1024);
        max_gaussians = max_gaussians.next_power_of_two();

        let rasterizer = pollster::block_on(GpuRasterizer::new(
            device,
            queue,
            RasterizerConfig {
                width,
                height,
                ..Default::default()
            },
            max_gaussians,
        ))?;

        self.gpu_state = Some(GpuGradientState {
            rasterizer,
            max_gaussians,
            width,
            height,
        });
        Ok(true)
    }

    fn log_gpu_parity(&self, cpu: &GaussianGradients, gpu: &GaussianGradients) {
        let position_rel = relative_l2_vec3(&cpu.position_grad, &gpu.position_grad);
        let sh_rel = relative_l2_sh(&cpu.sh_grad, &gpu.sh_grad);
        let opacity_rel = relative_l2_scalar(&cpu.opacity_grad, &gpu.opacity_grad);
        let scale_rel = relative_l2_vec3(&cpu.scale_grad, &gpu.scale_grad);
        let rotation_rel = relative_l2_vec4(&cpu.rotation_grad, &gpu.rotation_grad);

        if position_rel > self.config.gpu_parity_tolerance
            || sh_rel > self.config.gpu_parity_tolerance
            || opacity_rel > self.config.gpu_parity_tolerance
            || scale_rel > self.config.gpu_parity_tolerance
            || rotation_rel > self.config.gpu_parity_tolerance
        {
            tracing::warn!(
                "GPU/CPU gradient parity exceeded: pos={:.3}, sh={:.3}, opacity={:.3}, scale={:.3}, rotation={:.3}",
                position_rel,
                sh_rel,
                opacity_rel,
                scale_rel,
                rotation_rel
            );
        }
    }
}

#[cfg(feature = "wgpu")]
fn camera_to_uniform(camera: &Camera) -> CameraUniforms {
    let view = camera.transform.try_inverse().unwrap_or(na::Matrix4::identity());
    let width = camera.width.max(1) as f32;
    let height = camera.height.max(1) as f32;
    let fx = camera.intrinsics[(0, 0)];
    let fy = camera.intrinsics[(1, 1)];
    let cx = camera.intrinsics[(0, 2)];
    let cy = camera.intrinsics[(1, 2)];
    let near = 0.1f32;
    let far = 1000.0f32;
    let proj = na::Matrix4::new(
        2.0 * fx / width, 0.0, 1.0 - 2.0 * cx / width, 0.0,
        0.0, 2.0 * fy / height, 2.0 * cy / height - 1.0, 0.0,
        0.0, 0.0, (far + near) / (near - far), (2.0 * far * near) / (near - far),
        0.0, 0.0, -1.0, 0.0,
    );
    let view_projection = proj * view;
    let camera_position = camera.transform * na::Vector4::new(0.0, 0.0, 0.0, 1.0);

    CameraUniforms {
        view: view.into(),
        projection: proj.into(),
        view_projection: view_projection.into(),
        camera_position: [camera_position.x, camera_position.y, camera_position.z, 1.0],
        viewport: [width, height, near, far],
    }
}

#[cfg(feature = "wgpu")]
fn init_wgpu_context() -> Result<Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)>> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }));
    let Some(adapter) = adapter else {
        return Ok(None);
    };

    let limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("trueshot_3dgs_gpu"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
        },
        None,
    ))?;

    Ok(Some((Arc::new(device), Arc::new(queue))))
}

#[cfg(feature = "wgpu")]
fn rgb_to_rgba(image: &image::RgbImage) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((image.width() * image.height() * 4) as usize);
    for pixel in image.pixels() {
        rgba.push(pixel[0]);
        rgba.push(pixel[1]);
        rgba.push(pixel[2]);
        rgba.push(255u8);
    }
    rgba
}

#[cfg(feature = "wgpu")]
fn convert_gpu_gradients(gpu: &[GaussianGradientsGpu]) -> GaussianGradients {
    let mut gradients = GaussianGradients::new(gpu.len());
    for (i, g) in gpu.iter().enumerate() {
        gradients.position_grad[i] = na::Vector3::new(g.position_grad[0], g.position_grad[1], g.position_grad[2]);
        gradients.opacity_grad[i] = g.opacity_grad;
        gradients.scale_grad[i] = na::Vector3::new(g.scale_grad[0], g.scale_grad[1], g.scale_grad[2]);
        gradients.rotation_grad[i] = na::Vector4::new(
            g.rotation_grad[3],
            g.rotation_grad[0],
            g.rotation_grad[1],
            g.rotation_grad[2],
        );
        for j in 0..SH_COEFFS_TOTAL {
            gradients.sh_grad[i][j] = g.sh_grad[j];
        }
    }
    gradients
}

#[cfg(feature = "wgpu")]
fn relative_l2_sh(cpu: &[Vec<f32>], gpu: &[Vec<f32>]) -> f32 {
    let mut num = 0.0f32;
    let mut den = 0.0f32;
    let n = cpu.len().min(gpu.len());
    for i in 0..n {
        let c = &cpu[i];
        let g = &gpu[i];
        let m = c.len().min(g.len());
        for j in 0..m {
            let diff = c[j] - g[j];
            num += diff * diff;
            den += c[j] * c[j];
        }
    }
    (num.sqrt()) / (den.sqrt() + 1e-6)
}

#[cfg(feature = "wgpu")]
fn relative_l2_scalar(cpu: &[f32], gpu: &[f32]) -> f32 {
    let mut num = 0.0f32;
    let mut den = 0.0f32;
    let n = cpu.len().min(gpu.len());
    for i in 0..n {
        let diff = cpu[i] - gpu[i];
        num += diff * diff;
        den += cpu[i] * cpu[i];
    }
    (num.sqrt()) / (den.sqrt() + 1e-6)
}

#[cfg(feature = "wgpu")]
fn relative_l2_vec3(cpu: &[na::Vector3<f32>], gpu: &[na::Vector3<f32>]) -> f32 {
    let mut num = 0.0f32;
    let mut den = 0.0f32;
    let n = cpu.len().min(gpu.len());
    for i in 0..n {
        let diff = cpu[i] - gpu[i];
        num += diff.dot(&diff);
        den += cpu[i].dot(&cpu[i]);
    }
    (num.sqrt()) / (den.sqrt() + 1e-6)
}

#[cfg(feature = "wgpu")]
fn relative_l2_vec4(cpu: &[na::Vector4<f32>], gpu: &[na::Vector4<f32>]) -> f32 {
    let mut num = 0.0f32;
    let mut den = 0.0f32;
    let n = cpu.len().min(gpu.len());
    for i in 0..n {
        let diff = cpu[i] - gpu[i];
        num += diff.dot(&diff);
        den += cpu[i].dot(&cpu[i]);
    }
    (num.sqrt()) / (den.sqrt() + 1e-6)
}

/// Compute L1 loss between images
fn compute_l1_loss(rendered: &image::RgbImage, ground_truth: &image::RgbImage) -> f32 {
    let mut sum = 0.0f32;
    let n = (rendered.width() * rendered.height() * 3) as f32;

    for (r, g) in rendered.pixels().zip(ground_truth.pixels()) {
        for c in 0..3 {
            sum += (r[c] as f32 - g[c] as f32).abs() / 255.0;
        }
    }

    sum / n
}

/// Compute SSIM loss (simplified)
fn compute_ssim_loss(rendered: &image::RgbImage, ground_truth: &image::RgbImage) -> f32 {
    // Simplified SSIM - just structural similarity
    let mut sum_r = 0.0f64;
    let mut sum_g = 0.0f64;
    let mut sum_rg = 0.0f64;
    let mut sum_r2 = 0.0f64;
    let mut sum_g2 = 0.0f64;

    let n = (rendered.width() * rendered.height() * 3) as f64;

    for (r, g) in rendered.pixels().zip(ground_truth.pixels()) {
        for c in 0..3 {
            let rv = r[c] as f64 / 255.0;
            let gv = g[c] as f64 / 255.0;
            sum_r += rv;
            sum_g += gv;
            sum_rg += rv * gv;
            sum_r2 += rv * rv;
            sum_g2 += gv * gv;
        }
    }

    let mean_r = sum_r / n;
    let mean_g = sum_g / n;
    let var_r = sum_r2 / n - mean_r * mean_r;
    let var_g = sum_g2 / n - mean_g * mean_g;
    let cov_rg = sum_rg / n - mean_r * mean_g;

    let c1 = 0.01 * 0.01;
    let c2 = 0.03 * 0.03;

    let ssim = ((2.0 * mean_r * mean_g + c1) * (2.0 * cov_rg + c2))
             / ((mean_r.powi(2) + mean_g.powi(2) + c1) * (var_r + var_g + c2));

    ssim as f32
}

#[cfg(all(test, feature = "wgpu"))]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn gpu_cpu_gradient_parity_smoke() {
        let (device, queue) = match init_wgpu_context() {
            Ok(Some(ctx)) => ctx,
            _ => return,
        };

        let width = 64u32;
        let height = 64u32;
        let mut rasterizer = pollster::block_on(GpuRasterizer::new(
            device,
            queue,
            RasterizerConfig { width, height, ..Default::default() },
            16,
        )).unwrap();

        let camera = Camera {
            transform: na::Matrix4::identity(),
            intrinsics: na::Matrix3::new(80.0, 0.0, width as f32 * 0.5,
                                         0.0, 80.0, height as f32 * 0.5,
                                         0.0, 0.0, 1.0),
            width,
            height,
            image_path: PathBuf::new(),
        };

        let points = vec![(na::Point3::new(0.0, 0.0, 1.2), [200, 120, 90])];
        let cloud = GaussianCloud::from_points(&points);
        let rendered = cloud.render(&camera).unwrap();
        let ground_truth = rendered.clone();

        let (cpu_pos, cpu_opacity, cpu_scale, cpu_rot, cpu_sh) =
            cloud.compute_image_gradients(&camera, &rendered, &ground_truth);
        let mut cpu_gradients = GaussianGradients::new(1);
        cpu_gradients.position_grad = cpu_pos;
        cpu_gradients.opacity_grad = cpu_opacity;
        cpu_gradients.scale_grad = cpu_scale;
        cpu_gradients.rotation_grad = cpu_rot;
        cpu_gradients.sh_grad = cpu_sh;

        let gpu_gaussians = cloud.to_gpu_gaussians();
        rasterizer.upload_gaussians(&gpu_gaussians);
        rasterizer.set_camera(&camera_to_uniform(&camera));
        rasterizer.render().unwrap();
        rasterizer.upload_ground_truth(&rgb_to_rgba(&ground_truth));
        rasterizer.compute_gradients().unwrap();

        let gpu_raw = pollster::block_on(rasterizer.read_gradients()).unwrap();
        let gpu_gradients = convert_gpu_gradients(&gpu_raw);

        let pos_rel = relative_l2_vec3(&cpu_gradients.position_grad, &gpu_gradients.position_grad);
        let sh_rel = relative_l2_sh(&cpu_gradients.sh_grad, &gpu_gradients.sh_grad);
        let op_rel = relative_l2_scalar(&cpu_gradients.opacity_grad, &gpu_gradients.opacity_grad);
        let sc_rel = relative_l2_vec3(&cpu_gradients.scale_grad, &gpu_gradients.scale_grad);
        let rot_rel = relative_l2_vec4(&cpu_gradients.rotation_grad, &gpu_gradients.rotation_grad);

        let max_rel = pos_rel.max(sh_rel).max(op_rel).max(sc_rel).max(rot_rel);
        assert!(max_rel.is_finite());
    }
}
