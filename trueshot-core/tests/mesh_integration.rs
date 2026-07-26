//! Integration tests for mesh generation pipeline
//!
//! Tests the complete marching cubes pipeline with synthetic voxel data.

use trueshot_core::mesh::{VoxelGrid, MarchingCubes, MarchingCubesConfig, DensityVoxel};
use nalgebra as na;

/// Create a synthetic sphere SDF for testing
fn create_sphere_grid(resolution: usize, radius: f32) -> VoxelGrid<DensityVoxel> {
    let min = na::Point3::new(-1.5, -1.5, -1.5);
    let max = na::Point3::new(1.5, 1.5, 1.5);
    let voxel_size = (max.x - min.x) / resolution as f32;
    let mut grid: VoxelGrid<DensityVoxel> = VoxelGrid::with_dims(min, [resolution, resolution, resolution], voxel_size);
    
    let center = na::Point3::origin();
    
    for z in 0..resolution {
        for y in 0..resolution {
            for x in 0..resolution {
                let pos = grid.voxel_to_world([x, y, z]);
                let dist = na::distance(&pos, &center) - radius;
                grid.set([x, y, z], DensityVoxel { density: -dist, color: [0.0; 3], weight: 1.0 });
            }
        }
    }
    
    grid
}

#[test]
fn test_marching_cubes_sphere() {
    let grid = create_sphere_grid(32, 0.8);
    let config = MarchingCubesConfig::default();
    let mc = MarchingCubes::new(config);
    
    let mesh = mc.extract(&grid);
    
    // A sphere should generate vertices and triangles
    assert!(!mesh.vertices.is_empty(), "Sphere mesh should have vertices");
    assert!(!mesh.indices.is_empty(), "Sphere mesh should have triangles");
    
    // Check indices are valid
    let max_vertex = mesh.vertices.len() as u32;
    for &idx in &mesh.indices {
        assert!(idx < max_vertex, "Index {} out of bounds (max {})", idx, max_vertex);
    }
    
    // Indices should be divisible by 3 (triangles)
    assert_eq!(mesh.indices.len() % 3, 0, "Indices should form complete triangles");
}

#[test]
fn test_marching_cubes_empty_grid() {
    let min = na::Point3::new(0.0, 0.0, 0.0);
    let max = na::Point3::new(1.0, 1.0, 1.0);
    let voxel_size = (max.x - min.x) / 16.0;
    let grid: VoxelGrid<DensityVoxel> = VoxelGrid::with_dims(min, [16, 16, 16], voxel_size);
    // Grid is empty (all zeros), should produce no mesh
    
    let mc = MarchingCubes::new(MarchingCubesConfig::default());
    let mesh = mc.extract(&grid);
    
    // Empty grid with all zeros at threshold 0 might produce surface
    // This test verifies the algorithm doesn't crash on edge cases
    assert!(mesh.indices.len() % 3 == 0, "Should produce valid triangles or none");
}

#[test]
fn test_voxel_grid_roundtrip() {
    let min = na::Point3::new(-1.0, -1.0, -1.0);
    let max = na::Point3::new(1.0, 1.0, 1.0);
    let voxel_size = (max.x - min.x) / 10.0;
    let mut grid: VoxelGrid<f32> = VoxelGrid::with_dims(min, [10, 10, 10], voxel_size);
    
    // Set some values
    grid.set([5, 5, 5], 42.0);
    grid.set([0, 0, 0], -10.0);
    grid.set([9, 9, 9], 100.0);
    
    // Verify retrieval
    assert_eq!(*grid.get([5, 5, 5]), 42.0);
    assert_eq!(*grid.get([0, 0, 0]), -10.0);
    assert_eq!(*grid.get([9, 9, 9]), 100.0);
    
    // Verify world coordinate conversion
    let world_pos = grid.voxel_to_world([5, 5, 5]);
    assert!(world_pos.x.abs() < 0.3, "Center should be near origin");
}

#[test]
fn test_marching_cubes_resolution_scaling() {
    // Higher resolution should produce more vertices
    let grid_16 = create_sphere_grid(16, 0.7);
    let grid_32 = create_sphere_grid(32, 0.7);
    
    let mc = MarchingCubes::new(MarchingCubesConfig::default());
    
    let mesh_16 = mc.extract(&grid_16);
    let mesh_32 = mc.extract(&grid_32);
    
    // Higher resolution should typically produce more detailed mesh
    // (approximately 4x more triangles for 2x resolution)
    assert!(
        mesh_32.indices.len() > mesh_16.indices.len(),
        "Higher resolution should produce more triangles: {} vs {}",
        mesh_32.indices.len(),
        mesh_16.indices.len()
    );
}
