// Copyright 2025 Augment Technologies
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use nalgebra as na;
use crate::reconstruction::{ColoredPoint, Mesh};
use std::collections::HashMap;

// Note: This module uses a HashMap-based sparse voxel grid optimized for
// coverage queries. The unified mesh::VoxelGrid is a dense 3D grid better
// suited for marching cubes. We keep this implementation but document the
// relationship to the unified library.

/// Coverage density for a region
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CoverageDensity {
    None,      // No coverage (0%)
    VeryLow,   // Very low coverage (1-20%)
    Low,       // Low coverage (20-40%)
    Medium,    // Medium coverage (40-60%)
    Good,      // Good coverage (60-80%)
    Excellent, // Excellent coverage (80-100%)
}

impl CoverageDensity {
    /// Get color for this density level (RGB)
    pub fn to_color(&self) -> [u8; 3] {
        match self {
            Self::None => [128, 128, 128],      // Gray - no data
            Self::VeryLow => [255, 0, 0],       // Red - critical
            Self::Low => [255, 128, 0],         // Orange - needs work
            Self::Medium => [255, 255, 0],      // Yellow - okay
            Self::Good => [128, 255, 0],        // Yellow-green - good
            Self::Excellent => [0, 255, 0],     // Green - excellent
        }
    }
    
    /// Get color as normalized float (0-1)
    pub fn to_color_f32(&self) -> [f32; 3] {
        let rgb = self.to_color();
        [rgb[0] as f32 / 255.0, rgb[1] as f32 / 255.0, rgb[2] as f32 / 255.0]
    }
    
    /// Get density from point count in voxel
    pub fn from_point_count(count: usize, max_count: usize) -> Self {
        if max_count == 0 || count == 0 {
            return Self::None;
        }
        
        let ratio = count as f32 / max_count as f32;
        
        if ratio >= 0.8 {
            Self::Excellent
        } else if ratio >= 0.6 {
            Self::Good
        } else if ratio >= 0.4 {
            Self::Medium
        } else if ratio >= 0.2 {
            Self::Low
        } else if ratio > 0.0 {
            Self::VeryLow
        } else {
            Self::None
        }
    }
}

/// Voxel grid for coverage analysis
pub struct CoverageVoxelGrid {
    voxels: HashMap<(i32, i32, i32), usize>, // Voxel -> point count
    voxel_size: f32,
    max_count: usize,
}

impl CoverageVoxelGrid {
    pub fn new(voxel_size: f32) -> Self {
        Self {
            voxels: HashMap::new(),
            voxel_size,
            max_count: 0,
        }
    }
    
    /// Add points to the grid
    pub fn add_points(&mut self, points: &[ColoredPoint]) {
        for point in points {
            let voxel = self.world_to_voxel(&point.position);
            let count = self.voxels.entry(voxel).or_insert(0);
            *count += 1;
            self.max_count = self.max_count.max(*count);
        }
    }
    
    /// Get density at a point
    pub fn get_density(&self, point: &na::Point3<f32>) -> CoverageDensity {
        let voxel = self.world_to_voxel(point);
        let count = self.voxels.get(&voxel).copied().unwrap_or(0);
        CoverageDensity::from_point_count(count, self.max_count)
    }
    
    /// Get color for a point based on coverage
    pub fn get_color(&self, point: &na::Point3<f32>) -> [u8; 3] {
        self.get_density(point).to_color()
    }
    
    /// Get color as float for a point
    pub fn get_color_f32(&self, point: &na::Point3<f32>) -> [f32; 3] {
        self.get_density(point).to_color_f32()
    }
    
    /// Convert world coordinates to voxel
    fn world_to_voxel(&self, point: &na::Point3<f32>) -> (i32, i32, i32) {
        (
            (point.x / self.voxel_size).floor() as i32,
            (point.y / self.voxel_size).floor() as i32,
            (point.z / self.voxel_size).floor() as i32,
        )
    }
    
    /// Get coverage statistics
    pub fn get_stats(&self) -> CoverageStats {
        if self.voxels.is_empty() {
            return CoverageStats::default();
        }
        
        let total_voxels = self.voxels.len();
        let mut density_counts = [0usize; 6]; // Count for each density level
        
        for &count in self.voxels.values() {
            let density = CoverageDensity::from_point_count(count, self.max_count);
            let idx = match density {
                CoverageDensity::None => 0,
                CoverageDensity::VeryLow => 1,
                CoverageDensity::Low => 2,
                CoverageDensity::Medium => 3,
                CoverageDensity::Good => 4,
                CoverageDensity::Excellent => 5,
            };
            density_counts[idx] += 1;
        }
        
        CoverageStats {
            total_voxels,
            none_count: density_counts[0],
            very_low_count: density_counts[1],
            low_count: density_counts[2],
            medium_count: density_counts[3],
            good_count: density_counts[4],
            excellent_count: density_counts[5],
            max_density: self.max_count,
        }
    }
}

/// Coverage statistics
#[derive(Debug, Clone, Default)]
pub struct CoverageStats {
    pub total_voxels: usize,
    pub none_count: usize,
    pub very_low_count: usize,
    pub low_count: usize,
    pub medium_count: usize,
    pub good_count: usize,
    pub excellent_count: usize,
    pub max_density: usize,
}

impl CoverageStats {
    /// Get percentage of voxels with good or excellent coverage
    pub fn good_coverage_percent(&self) -> f32 {
        if self.total_voxels == 0 {
            return 0.0;
        }
        ((self.good_count + self.excellent_count) as f32 / self.total_voxels as f32) * 100.0
    }
    
    /// Get percentage of voxels with poor coverage (none, very low, low)
    pub fn poor_coverage_percent(&self) -> f32 {
        if self.total_voxels == 0 {
            return 0.0;
        }
        ((self.none_count + self.very_low_count + self.low_count) as f32 / self.total_voxels as f32) * 100.0
    }
}

/// Apply heatmap colors to point cloud
pub fn apply_heatmap_to_points(points: &[ColoredPoint], voxel_size: f32) -> Vec<ColoredPoint> {
    let mut grid = CoverageVoxelGrid::new(voxel_size);
    grid.add_points(points);
    
    let mut result = Vec::with_capacity(points.len());
    result.extend(points.iter().map(|point| {
        let heatmap_color = grid.get_color(&point.position);
        ColoredPoint {
            position: point.position,
            color: heatmap_color,
            confidence: point.confidence,
        }
    }));
    result
}

/// Apply heatmap colors to mesh vertices
pub fn apply_heatmap_to_mesh(mesh: &Mesh, points: &[ColoredPoint], voxel_size: f32) -> Mesh {
    let mut grid = CoverageVoxelGrid::new(voxel_size);
    grid.add_points(points);
    
    let heatmap_colors: Vec<[u8; 3]> = mesh.vertices
        .iter()
        .map(|vertex| grid.get_color(vertex))
        .collect();
    
    Mesh {
        vertices: mesh.vertices.clone(),
        colors: heatmap_colors,
        normals: mesh.normals.clone(),
        uvs: mesh.uvs.clone(),
        faces: mesh.faces.clone(),
    }
}

/// Identify gaps in coverage (areas with no or very low coverage)
pub fn identify_coverage_gaps(points: &[ColoredPoint], voxel_size: f32) -> Vec<na::Point3<f32>> {
    let mut grid = CoverageVoxelGrid::new(voxel_size);
    grid.add_points(points);
    
    // Find bounding box
    if points.is_empty() {
        return Vec::new();
    }
    
    let mut min = points[0].position.coords;
    let mut max = points[0].position.coords;
    
    for point in points {
        min = min.inf(&point.position.coords);
        max = max.sup(&point.position.coords);
    }
    
    // Expand slightly
    min -= na::Vector3::new(voxel_size, voxel_size, voxel_size);
    max += na::Vector3::new(voxel_size, voxel_size, voxel_size);
    
    // Find gaps (voxels with no or very low coverage)
    let mut gaps = Vec::new();
    
    let x_steps = ((max.x - min.x) / voxel_size).ceil() as i32;
    let y_steps = ((max.y - min.y) / voxel_size).ceil() as i32;
    let z_steps = ((max.z - min.z) / voxel_size).ceil() as i32;
    
    for x in 0..x_steps {
        for y in 0..y_steps {
            for z in 0..z_steps {
                let world_pos = na::Point3::new(
                    min.x + x as f32 * voxel_size,
                    min.y + y as f32 * voxel_size,
                    min.z + z as f32 * voxel_size,
                );
                
                let density = grid.get_density(&world_pos);
                if matches!(density, CoverageDensity::None | CoverageDensity::VeryLow) {
                    // Check if this voxel is near existing points (on surface)
                    let near_surface = points.iter().any(|p| {
                        na::distance(&p.position, &world_pos) < voxel_size * 2.0
                    });
                    
                    if near_surface {
                        gaps.push(world_pos);
                    }
                }
            }
        }
    }
    
    gaps
}

/// Generate heatmap legend text
pub fn get_heatmap_legend() -> Vec<(String, [u8; 3])> {
    vec![
        ("Excellent (80-100%)".to_string(), CoverageDensity::Excellent.to_color()),
        ("Good (60-80%)".to_string(), CoverageDensity::Good.to_color()),
        ("Medium (40-60%)".to_string(), CoverageDensity::Medium.to_color()),
        ("Low (20-40%)".to_string(), CoverageDensity::Low.to_color()),
        ("Very Low (1-20%)".to_string(), CoverageDensity::VeryLow.to_color()),
        ("None (0%)".to_string(), CoverageDensity::None.to_color()),
    ]
}

#[cfg(test)]
#[path = "./heatmap_tests.rs"]
mod tests;
