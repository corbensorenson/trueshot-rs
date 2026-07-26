//! 4D Gaussian Splatting for Dynamic Scene Capture
//! 
//! Implements real-time rendering of dynamic scenes using 4D Gaussian primitives.
//! Based on ICLR 2024 research: "Real-time Photorealistic Dynamic Scene Representation
//! and Rendering with 4D Gaussian Splatting" by Yang et al.
//! 
//! Key features:
//! - True 4D Gaussian primitives with temporal dimension
//! - 4D Spherindrical Harmonics for time-varying appearance
//! - Temporal slicing for real-time playback
//! - Multi-camera synchronized capture support

use nalgebra as na;
use serde::{Deserialize, Serialize};

/// 4D Gaussian primitive for dynamic scene representation
/// 
/// Extends standard 3D Gaussians with a temporal dimension, allowing
/// representation of moving and deforming objects over time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Gaussian4D {
    /// Unique identifier
    pub id: usize,
    
    /// 4D center position: (x, y, z, t)
    /// - xyz: spatial position
    /// - t: temporal center (normalized 0-1 for sequence duration)
    pub center: na::Vector4<f32>,
    
    /// 4D covariance matrix (anisotropic 4D ellipsoid)
    /// Stored as upper triangular (10 unique values for symmetric 4x4)
    pub covariance: Covariance4D,
    
    /// Base color (RGB)
    pub color: [f32; 3],
    
    /// Spherical harmonics coefficients for view-dependent color
    /// Degree 2 = 9 coefficients per channel = 27 total
    pub sh_coeffs: [[f32; 9]; 3],
    
    /// Temporal spherical harmonics for time-varying appearance
    /// Captures how color changes over time
    pub temporal_sh: TemporalSH,
    
    /// Opacity (0.0 - 1.0)
    pub opacity: f32,
    
    /// Temporal extent: splat is only visible within this time range
    pub time_range: (f32, f32),
    
    /// Velocity (for motion blur and prediction)
    pub velocity: na::Vector3<f32>,
}

/// 4D Covariance representation
/// 
/// Stores the upper triangular part of a symmetric 4x4 matrix
/// for efficient memory usage.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Covariance4D {
    /// Upper triangular values: [00, 01, 02, 03, 11, 12, 13, 22, 23, 33]
    pub values: [f32; 10],
}

impl Covariance4D {
    /// Create from scale and rotation in 4D
    pub fn from_scale_rotation(
        scale: na::Vector4<f32>,
        rotation: na::UnitQuaternion<f32>,
        temporal_scale: f32,
    ) -> Self {
        // For simplicity, we treat the 4D case as 3D spatial + 1D temporal
        // Full 4D rotation would require a 4D rotation representation
        
        let s = na::Matrix3::from_diagonal(&scale.xyz());
        let r = rotation.to_rotation_matrix();
        
        // 3D covariance: R * S * S^T * R^T
        let cov3d = r.matrix() * s * s * r.matrix().transpose();
        
        // Temporal variance (independent)
        let t_var = temporal_scale * temporal_scale;
        
        Self {
            values: [
                cov3d[(0, 0)], cov3d[(0, 1)], cov3d[(0, 2)], 0.0, // row 0, plus time coupling
                cov3d[(1, 1)], cov3d[(1, 2)], 0.0,                 // row 1
                cov3d[(2, 2)], 0.0,                                 // row 2
                t_var,                                              // temporal variance
            ],
        }
    }
    
    /// Convert to full 4x4 matrix
    pub fn to_matrix(&self) -> na::Matrix4<f32> {
        let v = &self.values;
        na::Matrix4::new(
            v[0], v[1], v[2], v[3],
            v[1], v[4], v[5], v[6],
            v[2], v[5], v[7], v[8],
            v[3], v[6], v[8], v[9],
        )
    }
    
    /// Get spatial 3D covariance (slice at a given time)
    pub fn spatial_covariance(&self) -> na::Matrix3<f32> {
        let v = &self.values;
        na::Matrix3::new(
            v[0], v[1], v[2],
            v[1], v[4], v[5],
            v[2], v[5], v[7],
        )
    }
    
    /// Get temporal variance
    pub fn temporal_variance(&self) -> f32 {
        self.values[9]
    }
}

impl Default for Covariance4D {
    fn default() -> Self {
        Self {
            values: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.1],
        }
    }
}

/// Temporal Spherical Harmonics
/// 
/// Models how the appearance (via SH) changes over time.
/// Uses a low-order polynomial or Fourier basis for efficiency.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemporalSH {
    /// Polynomial coefficients for temporal variation of each SH coefficient
    /// [dc, linear, quadratic] for each of 27 SH coefficients
    pub coeffs: [[f32; 3]; 27],
}

impl TemporalSH {
    /// Create with no temporal variation
    pub fn constant() -> Self {
        Self {
            coeffs: [[0.0; 3]; 27],
        }
    }
    
    /// Evaluate SH modulation at time t
    pub fn evaluate(&self, t: f32) -> [f32; 27] {
        let mut result = [0.0f32; 27];
        for (i, c) in self.coeffs.iter().enumerate() {
            result[i] = c[0] + c[1] * t + c[2] * t * t;
        }
        result
    }
}

impl Default for TemporalSH {
    fn default() -> Self {
        Self::constant()
    }
}

impl Gaussian4D {
    /// Create a new static 4D Gaussian (no temporal variation)
    pub fn new_static(position: na::Point3<f32>, color: [f32; 3], time: f32) -> Self {
        Self {
            id: 0,
            center: na::Vector4::new(position.x, position.y, position.z, time),
            covariance: Covariance4D::default(),
            color,
            sh_coeffs: [[0.0; 9]; 3],
            temporal_sh: TemporalSH::default(),
            opacity: 1.0,
            time_range: (0.0, 1.0),
            velocity: na::Vector3::zeros(),
        }
    }
    
    /// Get 3D position at a specific time
    pub fn position_at_time(&self, t: f32) -> na::Point3<f32> {
        let dt = t - self.center.w;
        na::Point3::new(
            self.center.x + self.velocity.x * dt,
            self.center.y + self.velocity.y * dt,
            self.center.z + self.velocity.z * dt,
        )
    }
    
    /// Check if this Gaussian is visible at time t
    pub fn is_visible_at(&self, t: f32) -> bool {
        t >= self.time_range.0 && t <= self.time_range.1
    }
    
    /// Get temporal weight (Gaussian falloff in time)
    pub fn temporal_weight(&self, t: f32) -> f32 {
        let dt = t - self.center.w;
        let t_var = self.covariance.temporal_variance();
        (-0.5 * dt * dt / t_var).exp()
    }
    
    /// Slice to 3D Gaussian at time t
    pub fn slice_at_time(&self, t: f32) -> Option<SlicedGaussian3D> {
        if !self.is_visible_at(t) {
            return None;
        }
        
        let weight = self.temporal_weight(t);
        if weight < 0.01 {
            return None;  // Too far from temporal center
        }

        let position = self.position_at_time(t);
        let cov3d = self.covariance.spatial_covariance();
        let dt = t - self.center.w;
        let temporal_var = self.covariance.temporal_variance();
        
        // Modulate SH coefficients and fallback color by temporal SH
        let temporal_mod = self.temporal_sh.evaluate(t);
        let mut sh_coeffs = self.sh_coeffs;
        for channel in 0..3 {
            for i in 0..9 {
                let idx = channel * 9 + i;
                sh_coeffs[channel][i] += temporal_mod[idx];
            }
        }
        let mut color = self.color;
        for i in 0..3 {
            color[i] = (color[i] + temporal_mod[i * 9]).clamp(0.0, 1.0);
        }
        
        Some(SlicedGaussian3D {
            id: self.id,
            position,
            covariance: cov3d,
            color,
            sh_coeffs,
            opacity: self.opacity * weight,
            base_opacity: self.opacity,
            temporal_weight: weight,
            temporal_dt: dt,
            temporal_var,
        })
    }
}

/// 3D Gaussian sliced from 4D at a specific time
/// 
/// This is what gets rendered - a standard 3DGS primitive
#[derive(Clone, Debug)]
pub struct SlicedGaussian3D {
    pub id: usize,
    pub position: na::Point3<f32>,
    pub covariance: na::Matrix3<f32>,
    pub color: [f32; 3],
    pub sh_coeffs: [[f32; 9]; 3],
    pub opacity: f32,
    pub base_opacity: f32,
    pub temporal_weight: f32,
    pub temporal_dt: f32,
    pub temporal_var: f32,
}

/// 4D Gaussian cloud representing a dynamic scene
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dynamic4DScene {
    /// All 4D Gaussians in the scene
    pub gaussians: Vec<Gaussian4D>,
    
    /// Total duration of the capture in seconds
    pub duration_seconds: f32,
    
    /// Frame rate of the original capture
    pub capture_fps: f32,
    
    /// Number of synchronized cameras used
    pub num_cameras: usize,
    
    /// Camera intrinsics and extrinsics for each camera
    pub cameras: Vec<SyncedCamera>,
    
    /// Scene metadata
    pub metadata: Scene4DMetadata,
}

/// Camera used in synchronized multi-view capture
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncedCamera {
    pub id: usize,
    pub name: String,
    pub intrinsics: CameraIntrinsics,
    pub extrinsics: CameraExtrinsics,
    pub frame_offset_ms: i32,  // Sync offset from master
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CameraIntrinsics {
    pub fx: f32,
    pub fy: f32,
    pub cx: f32,
    pub cy: f32,
    pub width: u32,
    pub height: u32,
    pub k1: f32,
    pub k2: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CameraExtrinsics {
    pub rotation: [f32; 9],      // Row-major 3x3 rotation
    pub translation: [f32; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Scene4DMetadata {
    pub created_at: String,
    pub software_version: String,
    pub training_iterations: u32,
    pub total_frames: u32,
    pub description: String,
}

impl Dynamic4DScene {
    /// Create a new empty 4D scene
    pub fn new(duration_seconds: f32, capture_fps: f32) -> Self {
        Self {
            gaussians: Vec::new(),
            duration_seconds,
            capture_fps,
            num_cameras: 0,
            cameras: Vec::new(),
            metadata: Scene4DMetadata::default(),
        }
    }
    
    /// Get number of Gaussians
    pub fn num_gaussians(&self) -> usize {
        self.gaussians.len()
    }
    
    /// Slice scene at time t to get 3D Gaussians for rendering
    pub fn slice_at_time(&self, time_seconds: f32) -> Vec<SlicedGaussian3D> {
        let t_normalized = (time_seconds / self.duration_seconds).clamp(0.0, 1.0);
        
        self.gaussians
            .iter()
            .enumerate()
            .filter_map(|(idx, g)| {
                g.slice_at_time(t_normalized).map(|mut sliced| {
                    sliced.id = idx;
                    sliced
                })
            })
            .collect()
    }
    
    /// Get total number of frames in the sequence
    pub fn total_frames(&self) -> usize {
        (self.duration_seconds * self.capture_fps).ceil() as usize
    }
    
    /// Convert frame index to normalized time
    pub fn frame_to_time(&self, frame: usize) -> f32 {
        let total = self.total_frames() as f32;
        if total <= 1.0 {
            0.0
        } else {
            (frame as f32 / (total - 1.0)).clamp(0.0, 1.0)
        }
    }
    
    /// Initialize from a static 3DGS scene (first frame)
    pub fn from_static_gaussians(
        gaussians_3d: &[super::gaussian::Gaussian3D],
        duration_seconds: f32,
        capture_fps: f32,
    ) -> Self {
        let default_view_dir = na::Vector3::new(0.0, 0.0, 1.0);
        let gaussians = gaussians_3d
            .iter()
            .enumerate()
            .map(|(id, g)| {
                // Get color from the color method
                let color_rgb = g.color(default_view_dir);
                // Convert rotation Vector4 to UnitQuaternion
                let rotation = na::UnitQuaternion::from_quaternion(
                    na::Quaternion::new(g.rotation.w, g.rotation.x, g.rotation.y, g.rotation.z)
                );
                // Convert Vec<f32> (25 elements per channel) to [[f32; 9]; 3]
                // Gaussian3D uses 25 coeffs per channel at indices 0-24, 25-49, 50-74
                // Gaussian4D uses 9 coeffs per channel (degree 2 SH)
                let mut sh_array = [[0.0f32; 9]; 3];
                for ch in 0..3 {
                    for i in 0..9.min(g.sh_coeffs.len().saturating_sub(ch * super::gaussian::SH_COEFFS_PER_CHANNEL)) {
                        let idx = ch * super::gaussian::SH_COEFFS_PER_CHANNEL + i;
                        if idx < g.sh_coeffs.len() {
                            sh_array[ch][i] = g.sh_coeffs[idx];
                        }
                    }
                }
                Gaussian4D {
                    id,
                    center: na::Vector4::new(
                        g.position.x,
                        g.position.y,
                        g.position.z,
                        0.5,  // Center in time
                    ),
                    covariance: Covariance4D::from_scale_rotation(
                        na::Vector4::new(g.scale.x, g.scale.y, g.scale.z, 0.5),
                        rotation,
                        0.5,
                    ),
                    color: color_rgb,
                    sh_coeffs: sh_array,
                    temporal_sh: TemporalSH::default(),
                    opacity: g.opacity,
                    time_range: (0.0, 1.0),
                    velocity: na::Vector3::zeros(),
                }
            })
            .collect();
        
        Self {
            gaussians,
            duration_seconds,
            capture_fps,
            num_cameras: 0,
            cameras: Vec::new(),
            metadata: Scene4DMetadata::default(),
        }
    }
    
    /// Export to PLY format (first frame snapshot)
    pub fn to_ply_at_time(&self, time: f32) -> Result<Vec<u8>, std::io::Error> {
        let sliced = self.slice_at_time(time);
        
        let mut output = Vec::new();
        use std::io::Write;
        
        // PLY header
        writeln!(output, "ply")?;
        writeln!(output, "format ascii 1.0")?;
        writeln!(output, "element vertex {}", sliced.len())?;
        writeln!(output, "property float x")?;
        writeln!(output, "property float y")?;
        writeln!(output, "property float z")?;
        writeln!(output, "property uchar red")?;
        writeln!(output, "property uchar green")?;
        writeln!(output, "property uchar blue")?;
        writeln!(output, "property float opacity")?;
        writeln!(output, "end_header")?;
        
        for g in sliced {
            writeln!(
                output,
                "{} {} {} {} {} {} {}",
                g.position.x,
                g.position.y,
                g.position.z,
                (g.color[0] * 255.0) as u8,
                (g.color[1] * 255.0) as u8,
                (g.color[2] * 255.0) as u8,
                g.opacity
            )?;
        }
        
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_gaussian4d_creation() {
        let g = Gaussian4D::new_static(
            na::Point3::new(1.0, 2.0, 3.0),
            [0.5, 0.6, 0.7],
            0.5,
        );
        assert_eq!(g.center.x, 1.0);
        assert_eq!(g.center.y, 2.0);
        assert_eq!(g.center.z, 3.0);
        assert_eq!(g.center.w, 0.5);
    }
    
    #[test]
    fn test_position_at_time() {
        let mut g = Gaussian4D::new_static(
            na::Point3::new(0.0, 0.0, 0.0),
            [1.0, 1.0, 1.0],
            0.0,
        );
        g.velocity = na::Vector3::new(1.0, 0.0, 0.0);
        
        let pos = g.position_at_time(0.5);
        assert!((pos.x - 0.5).abs() < 0.001);
    }
    
    #[test]
    fn test_time_visibility() {
        let mut g = Gaussian4D::new_static(
            na::Point3::origin(),
            [1.0, 1.0, 1.0],
            0.5,
        );
        g.time_range = (0.2, 0.8);
        
        assert!(!g.is_visible_at(0.1));
        assert!(g.is_visible_at(0.5));
        assert!(!g.is_visible_at(0.9));
    }
    
    #[test]
    fn test_slice_at_time() {
        let g = Gaussian4D::new_static(
            na::Point3::new(1.0, 2.0, 3.0),
            [0.5, 0.6, 0.7],
            0.5,
        );
        
        let sliced = g.slice_at_time(0.5).unwrap();
        assert!((sliced.position.x - 1.0).abs() < 0.001);
        assert!(sliced.opacity > 0.9);  // Should be near max at temporal center
    }
    
    #[test]
    fn test_dynamic_scene() {
        let scene = Dynamic4DScene::new(5.0, 30.0);
        assert_eq!(scene.total_frames(), 150);
        assert_eq!(scene.num_gaussians(), 0);
    }
    
    #[test]
    fn test_covariance4d() {
        let cov = Covariance4D::from_scale_rotation(
            na::Vector4::new(1.0, 1.0, 1.0, 0.5),
            na::UnitQuaternion::identity(),
            0.5,
        );
        
        let mat = cov.to_matrix();
        assert!((mat[(0, 0)] - 1.0).abs() < 0.001);
        assert!((mat[(3, 3)] - 0.25).abs() < 0.001);  // temporal scale^2
    }
}
