use crate::reconstruction::Mesh;
use nalgebra as na;
use image::{ImageBuffer, Rgb, RgbaImage};

/// UV coordinate (texture coordinate)
#[derive(Debug, Clone, Copy)]
pub struct UV {
    pub u: f32,
    pub v: f32,
}

impl UV {
    pub fn new(u: f32, v: f32) -> Self {
        Self { u, v }
    }
}

/// Textured mesh with UV coordinates
#[derive(Debug, Clone)]
pub struct TexturedMesh {
    pub mesh: Mesh,
    pub uv_coords: Vec<UV>,           // UV coordinate per vertex
    pub texture_atlas: Option<RgbaImage>,
}

impl TexturedMesh {
    pub fn new(mesh: Mesh) -> Self {
        let uv_coords = vec![UV::new(0.0, 0.0); mesh.vertices.len()];
        Self {
            mesh,
            uv_coords,
            texture_atlas: None,
        }
    }
}

/// Texture atlas builder
pub struct TextureAtlasBuilder {
    atlas_size: u32,
    images: Vec<ImageBuffer<Rgb<u8>, Vec<u8>>>,
    camera_poses: Vec<CameraPose>,
}

#[derive(Debug, Clone)]
pub struct CameraPose {
    pub position: na::Point3<f32>,
    pub rotation: na::Matrix3<f32>,
    pub intrinsics: CameraIntrinsics,
}

#[derive(Debug, Clone)]
pub struct CameraIntrinsics {
    pub fx: f32,
    pub fy: f32,
    pub cx: f32,
    pub cy: f32,
    pub width: u32,
    pub height: u32,
}

impl TextureAtlasBuilder {
    pub fn new(atlas_size: u32) -> Self {
        Self {
            atlas_size,
            images: Vec::new(),
            camera_poses: Vec::new(),
        }
    }
    
    /// Add a camera view with image
    pub fn add_view(&mut self, image: ImageBuffer<Rgb<u8>, Vec<u8>>, pose: CameraPose) {
        self.images.push(image);
        self.camera_poses.push(pose);
    }
    
    /// Generate UV coordinates using simple planar projection
    pub fn generate_uv_planar(&self, mesh: &Mesh) -> Vec<UV> {
        if mesh.vertices.is_empty() {
            return Vec::new();
        }
        
        // Find bounding box
        let mut min = mesh.vertices[0].coords;
        let mut max = mesh.vertices[0].coords;
        
        for vertex in &mesh.vertices {
            min = min.inf(&vertex.coords);
            max = max.sup(&vertex.coords);
        }
        
        let size = max - min;
        let max_dim = size.x.max(size.y).max(size.z);
        
        if max_dim == 0.0 {
            return vec![UV::new(0.5, 0.5); mesh.vertices.len()];
        }
        
        // Project onto XY plane and normalize to [0, 1]
        mesh.vertices
            .iter()
            .map(|vertex| {
                let u = (vertex.x - min.x) / max_dim;
                let v = (vertex.y - min.y) / max_dim;
                UV::new(u.clamp(0.0, 1.0), v.clamp(0.0, 1.0))
            })
            .collect()
    }
}
