//! 3D Gaussian Primitive and Cloud
//! 
//! Core data structures for 3D Gaussian Splatting.

use nalgebra as na;
use anyhow::Result;
use std::path::PathBuf;
use std::io::Write;
use zstd;

pub const SH_DEGREE: usize = 4;
pub const SH_COEFFS_PER_CHANNEL: usize = (SH_DEGREE + 1) * (SH_DEGREE + 1);
pub const SH_COEFFS_TOTAL: usize = SH_COEFFS_PER_CHANNEL * 3;

/// A single 3D Gaussian primitive
#[derive(Debug, Clone)]
pub struct Gaussian3D {
    /// Position in world space
    pub position: na::Point3<f32>,
    /// Rotation quaternion (w, x, y, z)
    pub rotation: na::Vector4<f32>,
    /// Log-space scale (3D anisotropic)
    pub scale: na::Vector3<f32>,
    /// Opacity (before sigmoid)
    pub opacity: f32,
    /// Spherical harmonics coefficients (25 coeffs * 3 channels = 75)
    pub sh_coeffs: Vec<f32>,
}

impl Gaussian3D {
    /// Create a new Gaussian from position and color
    pub fn from_point(position: na::Point3<f32>, color: [u8; 3]) -> Self {
        // Initial rotation: identity
        let rotation = na::Vector4::new(1.0, 0.0, 0.0, 0.0);
        
        // Initial scale: small sphere
        let scale = na::Vector3::new(-4.0, -4.0, -4.0); // exp(-4) ≈ 0.018
        
        // Initial opacity: fully opaque (before sigmoid)
        let opacity = 0.1;
        
        // Initialize SH coefficients with DC term (base color)
        let mut sh_coeffs = vec![0.0f32; SH_COEFFS_TOTAL];
        // SH DC term = color * C0 where C0 = 0.28209479
        let c0 = 0.28209479;
        sh_coeffs[0] = (color[0] as f32 / 255.0 - 0.5) / c0;  // R
        sh_coeffs[SH_COEFFS_PER_CHANNEL] = (color[1] as f32 / 255.0 - 0.5) / c0; // G
        sh_coeffs[SH_COEFFS_PER_CHANNEL * 2] = (color[2] as f32 / 255.0 - 0.5) / c0; // B

        Self {
            position,
            rotation,
            scale,
            opacity,
            sh_coeffs,
        }
    }

    /// Get world-space covariance matrix
    pub fn covariance(&self) -> na::Matrix3<f32> {
        // Rotation matrix from quaternion
        let r = self.rotation_matrix();
        
        // Scale matrix (exp of log-scale)
        let s = na::Matrix3::from_diagonal(&na::Vector3::new(
            self.scale.x.exp(),
            self.scale.y.exp(),
            self.scale.z.exp(),
        ));

        // Covariance = R * S * S^T * R^T
        let rs = r * s;
        rs * rs.transpose()
    }

    /// Get rotation matrix from quaternion
    pub fn rotation_matrix(&self) -> na::Matrix3<f32> {
        let w = self.rotation.w;
        let x = self.rotation.x;
        let y = self.rotation.y;
        let z = self.rotation.z;

        na::Matrix3::new(
            1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y),
            2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x),
            2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y),
        )
    }

    /// Get RGB color from spherical harmonics (for a given viewing direction)
    pub fn color(&self, view_dir: na::Vector3<f32>) -> [f32; 3] {
        let basis = eval_sh_basis(view_dir);
        let (_, color, _) = eval_sh_color(&self.sh_coeffs, &basis);
        color
    }

    /// Get opacity (after sigmoid activation)
    pub fn activated_opacity(&self) -> f32 {
        1.0 / (1.0 + (-self.opacity).exp())
    }
}

/// Cloud of 3D Gaussians
pub struct GaussianCloud {
    gaussians: Vec<Gaussian3D>,
    /// Accumulated gradients for densification
    position_gradients: Vec<na::Vector3<f32>>,
    gradient_count: Vec<u32>,
}

impl GaussianCloud {
    /// Create from initial point cloud
    pub fn from_points(points: &[(na::Point3<f32>, [u8; 3])]) -> Self {
        let gaussians: Vec<Gaussian3D> = points.iter()
            .map(|(pos, color)| Gaussian3D::from_point(*pos, *color))
            .collect();

        let n = gaussians.len();
        Self {
            gaussians,
            position_gradients: vec![na::Vector3::zeros(); n],
            gradient_count: vec![0; n],
        }
    }

    /// Number of Gaussians
    pub fn num_gaussians(&self) -> usize {
        self.gaussians.len()
    }

    #[cfg(feature = "wgpu")]
    pub fn to_gpu_gaussians(&self) -> Vec<super::rasterizer::Gaussian3DGpu> {
        self.gaussians
            .iter()
            .map(|g| {
                let mut sh = [0.0f32; SH_COEFFS_TOTAL];
                let count = sh.len().min(g.sh_coeffs.len());
                sh[..count].copy_from_slice(&g.sh_coeffs[..count]);
                super::rasterizer::Gaussian3DGpu {
                    position: [g.position.x, g.position.y, g.position.z, 1.0],
                    rotation: [g.rotation.x, g.rotation.y, g.rotation.z, g.rotation.w],
                    scale: [g.scale.x, g.scale.y, g.scale.z, 0.0],
                    opacity: [g.opacity, 0.0, 0.0, 0.0],
                    sh_coeffs: sh,
                }
            })
            .collect()
    }

    /// Get position of Gaussian i
    pub fn position(&self, i: usize) -> na::Point3<f32> {
        self.gaussians[i].position
    }

    /// Get scale of Gaussian i
    pub fn scale(&self, i: usize) -> na::Vector3<f32> {
        self.gaussians[i].scale
    }

    /// Get opacity of Gaussian i (before activation)
    pub fn opacity(&self, i: usize) -> f32 {
        self.gaussians[i].activated_opacity()
    }

    /// Update position of Gaussian i
    pub fn update_position(&mut self, i: usize, delta: na::Vector3<f32>) {
        self.gaussians[i].position += delta;
    }

    /// Update rotation of Gaussian i
    pub fn update_rotation(&mut self, i: usize, delta: na::Vector4<f32>) {
        self.gaussians[i].rotation += delta;
        // Renormalize quaternion
        let norm = self.gaussians[i].rotation.norm();
        if norm > 1e-10 {
            self.gaussians[i].rotation /= norm;
        }
    }

    /// Update scale of Gaussian i
    pub fn update_scale(&mut self, i: usize, delta: na::Vector3<f32>) {
        self.gaussians[i].scale += delta;
    }

    /// Update opacity of Gaussian i
    pub fn update_opacity(&mut self, i: usize, delta: f32) {
        self.gaussians[i].opacity += delta;
    }

    /// Update spherical harmonics coefficients for Gaussian i
    pub fn update_sh_coeffs(&mut self, i: usize, delta: &[f32]) {
        if i >= self.gaussians.len() {
            return;
        }
        let coeffs = &mut self.gaussians[i].sh_coeffs;
        let count = coeffs.len().min(delta.len());
        for idx in 0..count {
            coeffs[idx] += delta[idx];
        }
    }

    /// Clone Gaussians at given indices
    pub fn clone_gaussians(&mut self, indices: &[usize]) {
        for &i in indices {
            let mut new_gaussian = self.gaussians[i].clone();
            // Offset position slightly
            new_gaussian.position.x += 0.001;
            new_gaussian.position.y += 0.001;
            self.gaussians.push(new_gaussian);
            self.position_gradients.push(na::Vector3::zeros());
            self.gradient_count.push(0);
        }
    }

    /// Split Gaussians at given indices
    pub fn split_gaussians(&mut self, indices: &[usize]) {
        for &i in indices {
            let original = &self.gaussians[i];
            
            // Create two smaller Gaussians
            let scale_reduction = 0.8f32.ln(); // Reduce scale by 20%
            
            let mut g1 = original.clone();
            let mut g2 = original.clone();
            
            // Offset in random directions based on scale
            let offset = 0.01;
            g1.position.x += offset;
            g2.position.x -= offset;
            
            // Reduce scale
            g1.scale += na::Vector3::new(scale_reduction, scale_reduction, scale_reduction);
            g2.scale += na::Vector3::new(scale_reduction, scale_reduction, scale_reduction);
            
            // Replace original with g1
            self.gaussians[i] = g1;
            
            // Add g2
            self.gaussians.push(g2);
            self.position_gradients.push(na::Vector3::zeros());
            self.gradient_count.push(0);
        }
    }

    /// Remove Gaussians at given indices
    pub fn remove_gaussians(&mut self, indices: &[usize]) {
        // Sort in reverse order to remove from end first
        let mut sorted_indices: Vec<usize> = indices.to_vec();
        sorted_indices.sort_by(|a, b| b.cmp(a));
        sorted_indices.dedup();

        for i in sorted_indices {
            if i < self.gaussians.len() {
                self.gaussians.swap_remove(i);
                self.position_gradients.swap_remove(i);
                self.gradient_count.swap_remove(i);
            }
        }
    }

    /// Render to image (CPU fallback with anisotropic splats)
    pub fn render(&self, camera: &super::Camera) -> Result<image::RgbImage> {
        let width = camera.width;
        let height = camera.height;
        let mut image = image::RgbImage::new(width, height);

        // Sort Gaussians by depth (front to back for alpha blending would be back to front,
        // but we use a simplified depth-test approach)
        let view_matrix = camera.transform.try_inverse()
            .unwrap_or(na::Matrix4::identity());

        let mut sorted_indices: Vec<(usize, f32)> = self.gaussians.iter()
            .enumerate()
            .map(|(i, g)| {
                let cam_pos = view_matrix * na::Vector4::new(g.position.x, g.position.y, g.position.z, 1.0);
                (i, cam_pos.z)
            })
            .filter(|(_, z)| *z > 0.0) // Only in front of camera
            .collect();

        sorted_indices.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Render each Gaussian as a splat
        for (i, depth) in sorted_indices {
            let g = &self.gaussians[i];
            let opacity = g.activated_opacity();
            
            if opacity < 0.01 {
                continue;
            }

            // Project center
            let cam_pos = view_matrix * na::Vector4::new(g.position.x, g.position.y, g.position.z, 1.0);
            if cam_pos.z <= 0.0 {
                continue;
            }

            let fx = camera.intrinsics[(0, 0)];
            let fy = camera.intrinsics[(1, 1)];
            let cx = camera.intrinsics[(0, 2)];
            let cy = camera.intrinsics[(1, 2)];
            let u = fx * (cam_pos.x / cam_pos.z) + cx;
            let v = fy * (cam_pos.y / cam_pos.z) + cy;

            // Get color
            let view_dir = na::Vector3::new(-cam_pos.x, -cam_pos.y, -cam_pos.z).normalize();
            let color = g.color(view_dir);

            // Project covariance to 2D for anisotropic splat
            let cov3d = g.covariance();
            let cov2d = project_covariance(&cov3d, cam_pos.x, cam_pos.y, cam_pos.z, fx, fy);
            let (inv_cov, radius) = match invert_cov2d(&cov2d) {
                Some(result) => result,
                None => continue,
            };
            let radius = radius.max(1.0) as i32;

            // Draw splat
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let px = u as i32 + dx;
                    let py = v as i32 + dy;

                    if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                        continue;
                    }

                    let px = px as u32;
                    let py = py as u32;

                    // Gaussian falloff
                    let qx = dx as f32;
                    let qy = dy as f32;
                    let power = -0.5 * (
                        inv_cov[0] * qx * qx +
                        2.0 * inv_cov[1] * qx * qy +
                        inv_cov[2] * qy * qy
                    );
                    if power > 0.0 {
                        continue;
                    }
                    let weight = power.exp();

                    let alpha = opacity * weight;
                    if alpha < 0.01 {
                        continue;
                    }

                    // Alpha blending (back-to-front)
                    let current = image.get_pixel(px, py);
                    let new_r = (current[0] as f32 * (1.0 - alpha) + color[0] * 255.0 * alpha).round() as u8;
                    let new_g = (current[1] as f32 * (1.0 - alpha) + color[1] * 255.0 * alpha).round() as u8;
                    let new_b = (current[2] as f32 * (1.0 - alpha) + color[2] * 255.0 * alpha).round() as u8;

                    image.put_pixel(px, py, image::Rgb([new_r, new_g, new_b]));
                    let _ = depth;
                }
            }
        }

        Ok(image)
    }

    /// Compute image-space gradients for positions and opacities.
    pub fn compute_image_gradients(
        &self,
        camera: &super::Camera,
        rendered: &image::RgbImage,
        ground_truth: &image::RgbImage,
    ) -> (
        Vec<na::Vector3<f32>>,
        Vec<f32>,
        Vec<na::Vector3<f32>>,
        Vec<na::Vector4<f32>>,
        Vec<Vec<f32>>,
    ) {
        let width = rendered.width().min(ground_truth.width()).min(camera.width) as i32;
        let height = rendered.height().min(ground_truth.height()).min(camera.height) as i32;
        let mut position_grad = vec![na::Vector3::zeros(); self.gaussians.len()];
        let mut opacity_grad = vec![0.0f32; self.gaussians.len()];
        let mut scale_grad = vec![na::Vector3::zeros(); self.gaussians.len()];
        let mut rotation_grad = vec![na::Vector4::zeros(); self.gaussians.len()];
        let mut sh_grad = vec![vec![0.0f32; SH_COEFFS_TOTAL]; self.gaussians.len()];

        if width <= 0 || height <= 0 {
            return (position_grad, opacity_grad, scale_grad, rotation_grad, sh_grad);
        }

        let view_matrix = camera.transform.try_inverse().unwrap_or(na::Matrix4::identity());
        let rot = view_matrix.fixed_slice::<3, 3>(0, 0).into_owned();
        let rot_t = rot.transpose();
        let fx = camera.intrinsics[(0, 0)];
        let fy = camera.intrinsics[(1, 1)];
        let cx = camera.intrinsics[(0, 2)];
        let cy = camera.intrinsics[(1, 2)];
        for (idx, g) in self.gaussians.iter().enumerate() {
            let opacity = g.activated_opacity();
            if opacity < 0.01 {
                continue;
            }

            let cam_pos = view_matrix * na::Vector4::new(g.position.x, g.position.y, g.position.z, 1.0);
            if cam_pos.z <= 0.0 {
                continue;
            }

            let u = fx * (cam_pos.x / cam_pos.z) + cx;
            let v = fy * (cam_pos.y / cam_pos.z) + cy;

            let cov2d = project_covariance(&g.covariance(), cam_pos.x, cam_pos.y, cam_pos.z, fx, fy);
            let (inv_cov, mut radius) = match invert_cov2d(&cov2d) {
                Some(result) => result,
                None => continue,
            };
            radius = radius.min(64.0);
            let radius_i = radius.max(1.0) as i32;

            let min_x = (u as i32 - radius_i).max(0);
            let max_x = (u as i32 + radius_i).min(width - 1);
            let min_y = (v as i32 - radius_i).max(0);
            let max_y = (v as i32 + radius_i).min(height - 1);
            if min_x > max_x || min_y > max_y {
                continue;
            }

            let view_dir = na::Vector3::new(-cam_pos.x, -cam_pos.y, -cam_pos.z).normalize();
            let basis = eval_sh_basis(view_dir);
            let (_raw_color, color, clamp_mask) = eval_sh_color(&g.sh_coeffs, &basis);

            let mut grad_cam = na::Vector3::zeros();
            let mut grad_opacity = 0.0f32;
            let mut grad_scale = na::Vector3::zeros();
            let mut grad_sh = [0.0f32; SH_COEFFS_TOTAL];
            let mut grad_inv_cov = [0.0f32; 3];
            let dz = cam_pos.z;
            let inv_z = 1.0 / dz;
            let inv_z2 = inv_z * inv_z;

            for py in min_y..=max_y {
                for px in min_x..=max_x {
                    let dx = px as f32 + 0.5 - u;
                    let dy = py as f32 + 0.5 - v;

                    let quad = inv_cov[0] * dx * dx + 2.0 * inv_cov[1] * dx * dy + inv_cov[2] * dy * dy;
                    let power = -0.5 * quad;
                    if power > 0.0 {
                        continue;
                    }
                    let weight = power.exp();
                    let alpha = opacity * weight;
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

                    let inv_qx = inv_cov[0] * dx + inv_cov[1] * dy;
                    let inv_qy = inv_cov[1] * dx + inv_cov[2] * dy;
                    let d_loss_du = opacity * weight * d_loss_d_alpha * inv_qx;
                    let d_loss_dv = opacity * weight * d_loss_d_alpha * inv_qy;

                    let du_dx = fx * inv_z;
                    let dv_dy = fy * inv_z;
                    let du_dz = -fx * cam_pos.x * inv_z2;
                    let dv_dz = -fy * cam_pos.y * inv_z2;

                    grad_cam.x += d_loss_du * du_dx;
                    grad_cam.y += d_loss_dv * dv_dy;
                    grad_cam.z += d_loss_du * du_dz + d_loss_dv * dv_dz;
                    grad_opacity += d_loss_d_alpha * weight;

                    let radial = (dx * dx + dy * dy).max(1.0);
                    let scale_update = d_loss_d_alpha * alpha * radial * 1e-4;
                    grad_scale += na::Vector3::new(scale_update, scale_update, scale_update * 0.5);

                    let d_loss_d_weight = d_loss_d_alpha * opacity;
                    let d_loss_d_quad = -0.5 * d_loss_d_weight * weight;
                    grad_inv_cov[0] += d_loss_d_quad * dx * dx;
                    grad_inv_cov[1] += d_loss_d_quad * 2.0 * dx * dy;
                    grad_inv_cov[2] += d_loss_d_quad * dy * dy;

                    let d_loss_d_color = [
                        error[0] * alpha,
                        error[1] * alpha,
                        error[2] * alpha,
                    ];
                    for channel in 0..3 {
                        if clamp_mask[channel] == 0.0 {
                            continue;
                        }
                        let base = channel * SH_COEFFS_PER_CHANNEL;
                        let d = d_loss_d_color[channel] * clamp_mask[channel];
                        for i in 0..SH_COEFFS_PER_CHANNEL {
                            grad_sh[base + i] += d * basis[i];
                        }
                    }
                }
            }

            position_grad[idx] = rot_t * grad_cam;
            opacity_grad[idx] = grad_opacity;
            scale_grad[idx] = grad_scale;
            if grad_inv_cov[0].abs() > 0.0 || grad_inv_cov[1].abs() > 0.0 || grad_inv_cov[2].abs() > 0.0 {
                let inv_cov_mat = na::Matrix2::new(inv_cov[0], inv_cov[1], inv_cov[1], inv_cov[2]);
                let grad_inv_mat = na::Matrix2::new(
                    grad_inv_cov[0],
                    grad_inv_cov[1],
                    grad_inv_cov[1],
                    grad_inv_cov[2],
                );
                let grad_cov2d = -inv_cov_mat * grad_inv_mat * inv_cov_mat;
                let j = na::Matrix2x3::new(
                    fx * inv_z, 0.0, -fx * cam_pos.x * inv_z2,
                    0.0, fy * inv_z, -fy * cam_pos.y * inv_z2,
                );
                let grad_cov3d = j.transpose() * grad_cov2d * j;
                let scale = na::Vector3::new(g.scale.x.exp(), g.scale.y.exp(), g.scale.z.exp());
                let l = na::Matrix3::from_diagonal(&na::Vector3::new(
                    scale.x * scale.x,
                    scale.y * scale.y,
                    scale.z * scale.z,
                ));
                let r = g.rotation_matrix();
                let grad_sym = grad_cov3d + grad_cov3d.transpose();
                let grad_r = grad_sym * r * l;
                rotation_grad[idx] = rotation_grad_from_matrix(&grad_r, g.rotation);
            }
            for coeff in 0..SH_COEFFS_TOTAL {
                sh_grad[idx][coeff] = grad_sh[coeff];
            }
        }

        (position_grad, opacity_grad, scale_grad, rotation_grad, sh_grad)
    }

    /// Export to PLY format
    pub fn export_ply(&self, path: &PathBuf) -> Result<()> {
        let mut file = std::fs::File::create(path)?;

        // PLY header
        writeln!(file, "ply")?;
        writeln!(file, "format binary_little_endian 1.0")?;
        writeln!(file, "element vertex {}", self.gaussians.len())?;
        writeln!(file, "property float x")?;
        writeln!(file, "property float y")?;
        writeln!(file, "property float z")?;
        writeln!(file, "property float nx")?;
        writeln!(file, "property float ny")?;
        writeln!(file, "property float nz")?;
        
        // Spherical harmonics
        for i in 0..SH_COEFFS_TOTAL {
            writeln!(file, "property float f_dc_{}", i)?;
        }
        
        writeln!(file, "property float opacity")?;
        writeln!(file, "property float scale_0")?;
        writeln!(file, "property float scale_1")?;
        writeln!(file, "property float scale_2")?;
        writeln!(file, "property float rot_0")?;
        writeln!(file, "property float rot_1")?;
        writeln!(file, "property float rot_2")?;
        writeln!(file, "property float rot_3")?;
        writeln!(file, "end_header")?;

        // Write binary data
        for g in &self.gaussians {
            // Position
            file.write_all(&g.position.x.to_le_bytes())?;
            file.write_all(&g.position.y.to_le_bytes())?;
            file.write_all(&g.position.z.to_le_bytes())?;
            
            // Normal (dummy)
            file.write_all(&0.0f32.to_le_bytes())?;
            file.write_all(&0.0f32.to_le_bytes())?;
            file.write_all(&1.0f32.to_le_bytes())?;

            // SH coefficients
            for sh in &g.sh_coeffs {
                file.write_all(&sh.to_le_bytes())?;
            }

            // Opacity
            file.write_all(&g.opacity.to_le_bytes())?;

            // Scale
            file.write_all(&g.scale.x.to_le_bytes())?;
            file.write_all(&g.scale.y.to_le_bytes())?;
            file.write_all(&g.scale.z.to_le_bytes())?;

            // Rotation (quaternion)
            file.write_all(&g.rotation.w.to_le_bytes())?;
            file.write_all(&g.rotation.x.to_le_bytes())?;
            file.write_all(&g.rotation.y.to_le_bytes())?;
            file.write_all(&g.rotation.z.to_le_bytes())?;
        }

        Ok(())
    }

    /// Export to .splat format compatible with drei/gaussian-splats
    pub fn export_splat(&self, path: &PathBuf) -> Result<()> {
        let mut file = std::fs::File::create(path)?;
        let bytes = self.build_splat_bytes();
        file.write_all(&bytes)?;

        Ok(())
    }

    /// Export to .spz (zstd-compressed .splat payload)
    pub fn export_spz(&self, path: &PathBuf) -> Result<()> {
        let mut file = std::fs::File::create(path)?;
        let payload = self.build_splat_bytes();
        let compressed = zstd::encode_all(payload.as_slice(), 3)?;

        // Header: "SPZ1" + u32 LE uncompressed length
        file.write_all(b"SPZ1")?;
        let len = payload.len() as u32;
        file.write_all(&len.to_le_bytes())?;
        file.write_all(&compressed)?;

        Ok(())
    }
}

impl GaussianCloud {
    fn build_splat_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.gaussians.len() * 32);
        for g in &self.gaussians {
            bytes.extend_from_slice(&g.position.x.to_le_bytes());
            bytes.extend_from_slice(&g.position.y.to_le_bytes());
            bytes.extend_from_slice(&g.position.z.to_le_bytes());

            let scale = na::Vector3::new(g.scale.x.exp(), g.scale.y.exp(), g.scale.z.exp());
            bytes.extend_from_slice(&scale.x.to_le_bytes());
            bytes.extend_from_slice(&scale.y.to_le_bytes());
            bytes.extend_from_slice(&scale.z.to_le_bytes());

            let (r, g_c, b) = sh_dc_to_srgb(&g.sh_coeffs);
            let a = float_to_u8(g.activated_opacity());
            bytes.extend_from_slice(&[r, g_c, b, a]);

            let quat = encode_quat_bytes(g.rotation);
            bytes.extend_from_slice(&quat);
        }
        bytes
    }
}

fn sh_dc_to_srgb(sh_coeffs: &[f32]) -> (u8, u8, u8) {
    let c0 = 0.28209479;
    let r = sh_coeffs.get(0).copied().unwrap_or(0.0) * c0 + 0.5;
    let g = sh_coeffs
        .get(SH_COEFFS_PER_CHANNEL)
        .copied()
        .unwrap_or(0.0)
        * c0
        + 0.5;
    let b = sh_coeffs
        .get(SH_COEFFS_PER_CHANNEL * 2)
        .copied()
        .unwrap_or(0.0)
        * c0
        + 0.5;
    (float_to_u8(r), float_to_u8(g), float_to_u8(b))
}

fn float_to_u8(value: f32) -> u8 {
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

fn encode_quat_bytes(rotation: na::Vector4<f32>) -> [u8; 4] {
    let mut quat = rotation;
    let norm = (quat.w * quat.w + quat.x * quat.x + quat.y * quat.y + quat.z * quat.z).sqrt();
    if norm > 1e-6 {
        quat /= norm;
    } else {
        quat = na::Vector4::new(1.0, 0.0, 0.0, 0.0);
    }

    // Encode inverse quaternion so loader inversion yields original.
    let inv = na::Vector4::new(quat.w, -quat.x, -quat.y, -quat.z);
    let bw = (128.0 - inv.w * 128.0).round();
    let bx = (128.0 - inv.x * 128.0).round();
    let by = (128.0 + inv.y * 128.0).round();
    let bz = (128.0 + inv.z * 128.0).round();

    [
        bw.clamp(0.0, 255.0) as u8,
        bx.clamp(0.0, 255.0) as u8,
        by.clamp(0.0, 255.0) as u8,
        bz.clamp(0.0, 255.0) as u8,
    ]
}

fn project_covariance(
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

fn invert_cov2d(cov: &[f32; 3]) -> Option<([f32; 3], f32)> {
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

fn eval_sh_basis(view_dir: na::Vector3<f32>) -> [f32; SH_COEFFS_PER_CHANNEL] {
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
    let zz2 = zz * zz;
    let xx2 = xx * xx;
    let yy2 = yy * yy;

    let c0 = 0.2820947918f32;
    let c1 = 0.4886025119f32;
    let c2_0 = 1.0925484306f32;
    let c2_1 = 0.3153915653f32;
    let c2_2 = 0.5462742153f32;
    let c3_0 = 0.5900435899f32;
    let c3_1 = 2.8906114426f32;
    let c3_2 = 0.4570457995f32;
    let c3_3 = 0.3731763326f32;
    let c3_4 = 1.4453057213f32;
    let c4_0 = 2.5033429418f32;
    let c4_1 = 1.7701307698f32;
    let c4_2 = 0.9461746958f32;
    let c4_3 = 0.6690465436f32;
    let c4_4 = 0.1057855469f32;
    let c4_6 = 0.4730873479f32;
    let c4_8 = 0.6258357354f32;

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
        -c3_0 * y * (3.0 * xx - yy),
        c3_1 * xy * z,
        -c3_2 * y * (5.0 * zz - 1.0),
        c3_3 * z * (5.0 * zz - 3.0),
        -c3_2 * x * (5.0 * zz - 1.0),
        c3_4 * z * (xx - yy),
        -c3_0 * x * (xx - 3.0 * yy),
        c4_0 * xy * (xx - yy),
        -c4_1 * y * z * (3.0 * xx - yy),
        c4_2 * xy * (7.0 * zz - 1.0),
        -c4_3 * y * z * (7.0 * zz - 3.0),
        c4_4 * (35.0 * zz2 - 30.0 * zz + 3.0),
        -c4_3 * x * z * (7.0 * zz - 3.0),
        c4_6 * (xx - yy) * (7.0 * zz - 1.0),
        -c4_1 * x * z * (xx - 3.0 * yy),
        c4_8 * (xx2 - 6.0 * xx * yy + yy2),
    ]
}

fn eval_sh_color(sh_coeffs: &[f32], basis: &[f32; SH_COEFFS_PER_CHANNEL]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let mut raw = [0.0f32; 3];
    let mut color = [0.0f32; 3];
    let mut mask = [1.0f32; 3];

    for channel in 0..3 {
        let base = channel * SH_COEFFS_PER_CHANNEL;
        let mut accum = 0.0f32;
        for i in 0..SH_COEFFS_PER_CHANNEL {
            if base + i < sh_coeffs.len() {
                accum += sh_coeffs[base + i] * basis[i];
            }
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

fn rotation_grad_from_matrix(grad_r: &na::Matrix3<f32>, q: na::Vector4<f32>) -> na::Vector4<f32> {
    let w = q.w;
    let x = q.x;
    let y = q.y;
    let z = q.z;
    let gr = grad_r;

    let grad_w = gr[(0, 1)] * (-2.0 * z)
        + gr[(0, 2)] * (2.0 * y)
        + gr[(1, 0)] * (2.0 * z)
        + gr[(1, 2)] * (-2.0 * x)
        + gr[(2, 0)] * (-2.0 * y)
        + gr[(2, 1)] * (2.0 * x);

    let grad_x = gr[(0, 1)] * (2.0 * y)
        + gr[(0, 2)] * (2.0 * z)
        + gr[(1, 0)] * (2.0 * y)
        + gr[(1, 1)] * (-4.0 * x)
        + gr[(1, 2)] * (-2.0 * w)
        + gr[(2, 0)] * (2.0 * z)
        + gr[(2, 1)] * (2.0 * w)
        + gr[(2, 2)] * (-4.0 * x);

    let grad_y = gr[(0, 0)] * (-4.0 * y)
        + gr[(0, 1)] * (2.0 * x)
        + gr[(0, 2)] * (2.0 * w)
        + gr[(1, 0)] * (2.0 * x)
        + gr[(1, 2)] * (2.0 * z)
        + gr[(2, 0)] * (-2.0 * w)
        + gr[(2, 1)] * (2.0 * z)
        + gr[(2, 2)] * (-4.0 * y);

    let grad_z = gr[(0, 0)] * (-4.0 * z)
        + gr[(0, 1)] * (-2.0 * w)
        + gr[(0, 2)] * (2.0 * x)
        + gr[(1, 0)] * (2.0 * w)
        + gr[(1, 1)] * (-4.0 * z)
        + gr[(1, 2)] * (2.0 * y)
        + gr[(2, 0)] * (2.0 * x)
        + gr[(2, 1)] * (2.0 * y);

    na::Vector4::new(grad_w, grad_x, grad_y, grad_z)
}
