//! Visual Hull (Voxel Carving) Module
//!
//! Implements visual hull estimation using multi-view silhouettes.
//! Enhanced with unified mesh library for complete pipeline:
//! - Voxel grid generation
//! - Silhouette-based carving
//! - Mesh extraction via marching cubes

use crate::mesh::{ExtractedMesh, MarchingCubes, MarchingCubesConfig, VoxelGrid};
use nalgebra::{Matrix4, Point3, Vector3};

/// Configuration for visual hull reconstruction
#[derive(Clone, Debug)]
pub struct VisualHullConfig {
    /// Voxel size in world units
    pub voxel_size: f32,
    /// Minimum silhouette visibility for a voxel to be valid
    pub min_visibility_ratio: f32,
    /// Surface threshold for mesh extraction
    pub surface_threshold: f32,
    /// Whether to compute normals in output mesh
    pub compute_normals: bool,
}

impl Default for VisualHullConfig {
    fn default() -> Self {
        Self {
            voxel_size: 0.01,
            min_visibility_ratio: 0.9,
            surface_threshold: 0.5,
            compute_normals: true,
        }
    }
}

/// Generate a regular voxel grid covering the bounding box
pub fn generate_voxel_grid(
    bounds_min: Point3<f32>,
    bounds_max: Point3<f32>,
    voxel_size: f32,
) -> Vec<Vector3<f32>> {
    let mut voxels = Vec::new();

    let mut x = bounds_min.x;
    while x < bounds_max.x {
        let mut y = bounds_min.y;
        while y < bounds_max.y {
            let mut z = bounds_min.z;
            while z < bounds_max.z {
                voxels.push(Vector3::new(x, y, z));
                z += voxel_size;
            }
            y += voxel_size;
        }
        x += voxel_size;
    }

    voxels
}

/// Simple Visual Hull (Voxel Carving)
/// Carves a voxel grid based on a set of binary silhouettes and camera matrices.
/// Returns a list of center-points of remaining voxels.
pub fn carve_visual_hull(
    voxels: Vec<Vector3<f32>>,
    masks: &[&[u8]],
    mvps: &[Matrix4<f32>],
    img_dims: (u32, u32),
) -> Vec<Vector3<f32>> {
    let mut visible_voxels = Vec::new();

    for voxel in voxels {
        let p = voxel.to_homogeneous();
        let mut seen_count = 0;

        // A voxel must be inside the silhouette of ALL cameras (intersection of cones)
        // In practice, we allow some error (e.g. 90% match) to handle noise.
        let mut is_inside = true;

        for (i, mvp) in mvps.iter().enumerate() {
            let proj = mvp * p;
            // Perspective Divide
            if proj.w <= 0.0 {
                continue;
            } // Behind camera

            let u = ((proj.x / proj.w) * 0.5 + 0.5) * img_dims.0 as f32;
            let v = ((1.0 - (proj.y / proj.w)) * 0.5) * img_dims.1 as f32; // Flip Y for image coords

            if u >= 0.0 && u < img_dims.0 as f32 && v >= 0.0 && v < img_dims.1 as f32 {
                let idx = (v as u32 * img_dims.0 + u as u32) as usize;
                if masks[i][idx] < 128 {
                    is_inside = false;
                    break;
                }
                seen_count += 1;
            } else {
                // Out of frame?
                // If the object is fully in frame for all cameras, out of frame means "not object".
                // But in closeups, it might just be cropped.
                // Conservative approach: Only carve if we validly see "Empty Space".
            }
        }

        if is_inside && seen_count > 0 {
            visible_voxels.push(voxel);
        }
    }

    visible_voxels
}

/// Convert carved voxels to a density grid for mesh extraction
pub fn voxels_to_density_grid(voxels: &[Vector3<f32>], voxel_size: f32) -> VoxelGrid<f32> {
    if voxels.is_empty() {
        return VoxelGrid::with_dims(Point3::origin(), [1, 1, 1], voxel_size);
    }

    // Find bounds
    let mut min = Point3::new(f32::MAX, f32::MAX, f32::MAX);
    let mut max = Point3::new(f32::MIN, f32::MIN, f32::MIN);

    for v in voxels {
        min.x = min.x.min(v.x);
        min.y = min.y.min(v.y);
        min.z = min.z.min(v.z);
        max.x = max.x.max(v.x);
        max.y = max.y.max(v.y);
        max.z = max.z.max(v.z);
    }

    // Add padding
    min -= Vector3::new(voxel_size, voxel_size, voxel_size);
    max += Vector3::new(voxel_size, voxel_size, voxel_size);

    let mut grid: VoxelGrid<f32> = VoxelGrid::new(min, max, voxel_size);

    // Mark occupied voxels
    for v in voxels {
        let pos = Point3::new(v.x, v.y, v.z);
        if let Some(voxel) = grid.world_to_voxel(&pos) {
            grid.set(voxel, 1.0);
        }
    }

    grid
}

/// Extract mesh from visual hull using marching cubes
pub fn extract_hull_mesh(voxels: &[Vector3<f32>], config: &VisualHullConfig) -> ExtractedMesh {
    let grid = voxels_to_density_grid(voxels, config.voxel_size);

    let mc = MarchingCubes::new(MarchingCubesConfig {
        threshold: config.surface_threshold,
        compute_normals: config.compute_normals,
        compute_uvs: true,
        uv_scale: 1.0,
    });

    mc.extract(&grid)
}

/// Complete visual hull pipeline: generate grid, carve, and extract mesh
pub fn visual_hull_to_mesh(
    bounds_min: Point3<f32>,
    bounds_max: Point3<f32>,
    masks: &[&[u8]],
    mvps: &[Matrix4<f32>],
    img_dims: (u32, u32),
    config: &VisualHullConfig,
) -> ExtractedMesh {
    // Generate initial voxel grid
    let voxels = generate_voxel_grid(bounds_min, bounds_max, config.voxel_size);

    // Carve using silhouettes
    let carved = carve_visual_hull(voxels, masks, mvps, img_dims);

    // Extract mesh
    extract_hull_mesh(&carved, config)
}
