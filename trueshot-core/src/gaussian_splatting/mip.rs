//! Mip-Splatting - Alias-Free 3D Gaussian Splatting
//! 
//! CVPR 2024 Best Student Paper implementation.
//! Addresses aliasing artifacts when camera zooms in/out.
//! 
//! Reference: Yu et al., "Mip-Splatting: Alias-free 3D Gaussian Splatting"

use nalgebra as na;
use super::Camera;

/// 3D Low-pass filter for scale adaptation
#[derive(Debug, Clone)]
pub struct MipFilter3D {
    /// Minimum allowed Gaussian scale (based on sampling frequency)
    pub min_scale: f32,
}

impl MipFilter3D {
    /// Compute minimum Gaussian scale based on camera and image resolution
    /// 
    /// Key insight: Gaussian scale must be >= 2 * pixel footprint at that depth
    /// (Nyquist sampling theorem)
    pub fn compute_for_camera(
        gaussian_pos: &na::Point3<f32>,
        camera: &Camera,
    ) -> Self {
        // Get camera position from transform
        let cam_pos = na::Point3::new(
            camera.transform[(0, 3)],
            camera.transform[(1, 3)],
            camera.transform[(2, 3)],
        );

        // Distance from camera to gaussian
        let distance = (gaussian_pos - cam_pos).norm();

        // Focal length (average of fx, fy)
        let focal = (camera.intrinsics[(0, 0)] + camera.intrinsics[(1, 1)]) / 2.0;

        // Pixel footprint at gaussian depth
        let pixel_footprint = distance / focal;

        // Nyquist: min_scale = 2 * pixel_footprint
        let min_scale = 2.0 * pixel_footprint;

        Self { min_scale }
    }

    /// Filter a Gaussian's scale to prevent aliasing
    pub fn filter_scale(&self, scale: &na::Vector3<f32>) -> na::Vector3<f32> {
        na::Vector3::new(
            scale.x.max(self.min_scale.ln()), // Scales are in log space
            scale.y.max(self.min_scale.ln()),
            scale.z.max(self.min_scale.ln()),
        )
    }
}

/// 2D Mip filter for screen-space anti-aliasing
#[derive(Debug, Clone)]
pub struct MipFilter2D {
    /// Kernel variance to add based on pixel size
    pub pixel_variance: f32,
}

impl MipFilter2D {
    /// Create 2D Mip filter for given image resolution
    pub fn new(image_width: u32, image_height: u32) -> Self {
        // Standard pixel variance (0.3 pixels works well)
        let pixel_variance = 0.3;
        
        Self { pixel_variance }
    }

    /// Apply Mip filter to 2D projected covariance
    /// 
    /// Instead of simple dilation, we add to the covariance matrix
    /// This preserves the Gaussian shape while preventing aliasing
    pub fn filter_covariance(&self, cov_2d: &na::Matrix2<f32>) -> na::Matrix2<f32> {
        // Add pixel variance to diagonal
        let mip_addition = na::Matrix2::new(
            self.pixel_variance, 0.0,
            0.0, self.pixel_variance,
        );

        cov_2d + mip_addition
    }
}

/// Compute 2D covariance from 3D Gaussian projected to screen
pub fn project_covariance_to_2d(
    cov_3d: &na::Matrix3<f32>,
    gaussian_pos: &na::Point3<f32>,
    camera: &Camera,
) -> na::Matrix2<f32> {
    // Get view matrix
    let view = camera.transform.try_inverse()
        .unwrap_or(na::Matrix4::identity());

    // Transform to camera space
    let cam_pos = view.transform_point(gaussian_pos);
    
    if cam_pos.z <= 0.0 {
        return na::Matrix2::identity();
    }

    // Jacobian of perspective projection
    let fx = camera.intrinsics[(0, 0)];
    let fy = camera.intrinsics[(1, 1)];
    let z = cam_pos.z;
    let z2 = z * z;

    let j = na::Matrix2x3::new(
        fx / z, 0.0, -fx * cam_pos.x / z2,
        0.0, fy / z, -fy * cam_pos.y / z2,
    );

    // Rotate covariance to camera space
    let r = view.fixed_view::<3, 3>(0, 0);
    let cov_cam = r * cov_3d * r.transpose();

    // Project to 2D: J * Sigma * J^T
    j * cov_cam * j.transpose()
}

/// Multi-scale Gaussian representation for level-of-detail
#[derive(Debug, Clone)]
pub struct MultiScaleGaussian {
    /// Original Gaussian parameters
    pub position: na::Point3<f32>,
    pub rotation: na::Vector4<f32>,
    pub base_scale: na::Vector3<f32>,
    pub opacity: f32,
    pub sh_coeffs: Vec<f32>,
    
    /// Pre-computed scales for different LOD levels
    pub lod_scales: Vec<na::Vector3<f32>>,
}

impl MultiScaleGaussian {
    /// Create multi-scale representation with N LOD levels
    pub fn new(
        position: na::Point3<f32>,
        rotation: na::Vector4<f32>,
        base_scale: na::Vector3<f32>,
        opacity: f32,
        sh_coeffs: Vec<f32>,
        num_lods: usize,
    ) -> Self {
        // Pre-compute scales for each LOD level
        // Each level doubles the scale
        let lod_scales: Vec<na::Vector3<f32>> = (0..num_lods)
            .map(|level| {
                let factor = (level as f32).exp2().ln();
                na::Vector3::new(
                    base_scale.x + factor,
                    base_scale.y + factor,
                    base_scale.z + factor,
                )
            })
            .collect();

        Self {
            position,
            rotation,
            base_scale,
            opacity,
            sh_coeffs,
            lod_scales,
        }
    }

    /// Get appropriate scale for given camera distance
    pub fn scale_for_camera(&self, camera: &Camera) -> na::Vector3<f32> {
        let mip = MipFilter3D::compute_for_camera(&self.position, camera);
        
        // Find smallest LOD that satisfies Nyquist
        for scale in &self.lod_scales {
            if scale.x.exp() >= mip.min_scale {
                return *scale;
            }
        }

        // Return largest LOD if none found
        self.lod_scales.last().cloned().unwrap_or(self.base_scale)
    }
}

/// Screen-space EWA (Elliptical Weighted Average) filter
/// Used during rasterization for proper anti-aliasing
pub fn ewa_filter(
    cov_2d: &na::Matrix2<f32>,
    pixel_offset: (f32, f32),
) -> f32 {
    // Invert covariance for Gaussian evaluation
    let det = cov_2d[(0, 0)] * cov_2d[(1, 1)] - cov_2d[(0, 1)] * cov_2d[(1, 0)];
    
    if det.abs() < 1e-10 {
        return 0.0;
    }

    let inv_det = 1.0 / det;
    let cov_inv = na::Matrix2::new(
        cov_2d[(1, 1)] * inv_det, -cov_2d[(0, 1)] * inv_det,
        -cov_2d[(1, 0)] * inv_det, cov_2d[(0, 0)] * inv_det,
    );

    // Mahalanobis distance
    let d = na::Vector2::new(pixel_offset.0, pixel_offset.1);
    let mahal = d.transpose() * cov_inv * d;

    // Gaussian weight
    (-0.5 * mahal[(0, 0)]).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_camera() -> Camera {
        Camera {
            transform: na::Matrix4::identity(),
            intrinsics: na::Matrix3::new(
                800.0, 0.0, 400.0,
                0.0, 800.0, 300.0,
                0.0, 0.0, 1.0,
            ),
            width: 800,
            height: 600,
            image_path: std::path::PathBuf::new(),
        }
    }

    #[test]
    fn test_mip_filter_3d() {
        let camera = test_camera();
        
        // Gaussian at 1m distance
        let pos = na::Point3::new(0.0, 0.0, 1.0);
        let filter = MipFilter3D::compute_for_camera(&pos, &camera);
        
        // Should require minimum scale > 0
        assert!(filter.min_scale > 0.0);
    }

    #[test]
    fn test_mip_filter_distance_scaling() {
        let camera = test_camera();
        
        // Close gaussian
        let pos_close = na::Point3::new(0.0, 0.0, 0.5);
        let filter_close = MipFilter3D::compute_for_camera(&pos_close, &camera);
        
        // Far gaussian
        let pos_far = na::Point3::new(0.0, 0.0, 5.0);
        let filter_far = MipFilter3D::compute_for_camera(&pos_far, &camera);
        
        // Far gaussian should require larger minimum scale
        assert!(filter_far.min_scale > filter_close.min_scale);
    }

    #[test]
    fn test_ewa_filter() {
        let cov = na::Matrix2::identity();
        
        // At center
        let weight_center = ewa_filter(&cov, (0.0, 0.0));
        assert!((weight_center - 1.0).abs() < 0.01);
        
        // Away from center
        let weight_offset = ewa_filter(&cov, (2.0, 2.0));
        assert!(weight_offset < weight_center);
    }
}
