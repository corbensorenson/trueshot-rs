//! 3D Reconstruction Types
//!
//! Core data types for 3D reconstruction including meshes, point clouds,
//! and quality settings.

use nalgebra as na;
use serde::{Deserialize, Serialize};

/// A colored point with position, color, and confidence score.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColoredPoint {
    /// 3D position in world coordinates
    pub position: na::Point3<f32>,
    /// RGB color (0-255)
    pub color: [u8; 3],
    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,
}

/// A triangular mesh with vertices, normals, colors, UVs, and faces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mesh {
    /// Vertex positions in 3D space
    pub vertices: Vec<na::Point3<f32>>,
    /// Vertex colors (RGB, 0-255)
    pub colors: Vec<[u8; 3]>,
    /// Vertex normals (unit vectors)
    pub normals: Vec<na::Vector3<f32>>,
    /// Texture coordinates (UV, 0.0-1.0)
    pub uvs: Vec<[f32; 2]>,
    /// Triangle faces (indices into vertices array)
    pub faces: Vec<Face>,
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Face {
    pub vertices: [usize; 3],
}

#[derive(Debug, Clone, Default)]
pub struct ReconstructionStats {
    pub voxel_count: usize,
    pub point_count: usize,
    pub vertex_count: usize,
    pub face_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityLevel {
    Low,
    Medium,
    High,
    Ultra,
}

impl QualityLevel {
    pub fn voxel_size(&self) -> f32 {
        match self {
            Self::Low => 0.02,
            Self::Medium => 0.01,
            Self::High => 0.005,
            Self::Ultra => 0.002,
        }
    }
}

/// Reconstruction method selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReconstructionMethod {
    /// Classic photogrammetry (SfM + MVS + Mesh)
    Photogrammetry,
    /// Pure 3D Gaussian Splatting
    GaussianSplatting,
    /// Hybrid: Best of both worlds (DEFAULT)
    /// - Real-time processing during scanning
    /// - High-quality refinement with DSLR images
    /// - Outputs both 3DGS and mesh
    Hybrid,
}

impl Default for ReconstructionMethod {
    fn default() -> Self {
        Self::Hybrid // Hybrid is the default!
    }
}

pub mod hull;
pub mod pipeline;
pub mod livescan;
pub mod unified;
pub mod job;
pub mod hybrid;
pub mod multicam_sfm;

// Re-export hybrid pipeline for easy access
pub use hybrid::{HybridPipeline, HybridConfig, HybridQuality, PipelinePhase};

// Re-export multi-camera SfM
pub use multicam_sfm::{MultiCamSfm, MultiCamConfig, CameraId, LivescanFrame, HighResImage};
