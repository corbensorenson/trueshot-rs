//! Unified Voxel Grid
//!
//! Generic voxel grid used across all reconstruction modes:
//! - PhotogrammetryL Coverage heatmaps
//! - 3DGS → Mesh: TSDF volume
//! - LiveHybrid: Density accumulation
//! - Scene Reconstruction: Confidence fields

use nalgebra as na;

/// Trait for data stored in voxels
pub trait VoxelData: Clone + Default + Send + Sync {
    /// Accumulate another value into this voxel
    fn accumulate(&mut self, other: &Self);

    /// Interpolate between two values
    fn lerp(&self, other: &Self, t: f32) -> Self;
}

/// Generic 3D voxel grid
#[derive(Clone)]
pub struct VoxelGrid<T: VoxelData> {
    /// Voxel data
    pub data: Vec<T>,
    /// Grid dimensions (x, y, z)
    pub dims: [usize; 3],
    /// World-space origin (min corner)
    pub origin: na::Point3<f32>,
    /// Size of each voxel in world units
    pub voxel_size: f32,
}

impl<T: VoxelData> VoxelGrid<T> {
    /// Create a new voxel grid
    pub fn new(bounds_min: na::Point3<f32>, bounds_max: na::Point3<f32>, voxel_size: f32) -> Self {
        let size = bounds_max - bounds_min;
        let dims = [
            ((size.x / voxel_size).ceil() as usize).max(1),
            ((size.y / voxel_size).ceil() as usize).max(1),
            ((size.z / voxel_size).ceil() as usize).max(1),
        ];

        let total = dims[0] * dims[1] * dims[2];
        let data = vec![T::default(); total];

        Self {
            data,
            dims,
            origin: bounds_min,
            voxel_size,
        }
    }

    /// Create with explicit dimensions
    pub fn with_dims(origin: na::Point3<f32>, dims: [usize; 3], voxel_size: f32) -> Self {
        let total = dims[0] * dims[1] * dims[2];
        Self {
            data: vec![T::default(); total],
            dims,
            origin,
            voxel_size,
        }
    }

    /// Total number of voxels
    #[inline]
    pub fn len(&self) -> usize {
        self.dims[0] * self.dims[1] * self.dims[2]
    }

    /// Check if empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Convert world position to voxel index
    #[inline]
    pub fn world_to_voxel(&self, pos: &na::Point3<f32>) -> Option<[usize; 3]> {
        let local = pos - self.origin;
        let vx = (local.x / self.voxel_size).floor() as isize;
        let vy = (local.y / self.voxel_size).floor() as isize;
        let vz = (local.z / self.voxel_size).floor() as isize;

        if vx >= 0
            && vx < self.dims[0] as isize
            && vy >= 0
            && vy < self.dims[1] as isize
            && vz >= 0
            && vz < self.dims[2] as isize
        {
            Some([vx as usize, vy as usize, vz as usize])
        } else {
            None
        }
    }

    /// Convert voxel index to world position (voxel center)
    #[inline]
    pub fn voxel_to_world(&self, voxel: [usize; 3]) -> na::Point3<f32> {
        self.origin
            + na::Vector3::new(
                (voxel[0] as f32 + 0.5) * self.voxel_size,
                (voxel[1] as f32 + 0.5) * self.voxel_size,
                (voxel[2] as f32 + 0.5) * self.voxel_size,
            )
    }

    /// Get linear index from 3D index
    #[inline]
    pub fn linear_index(&self, voxel: [usize; 3]) -> usize {
        voxel[2] * self.dims[1] * self.dims[0] + voxel[1] * self.dims[0] + voxel[0]
    }

    /// Get 3D index from linear index
    #[inline]
    pub fn index_3d(&self, linear: usize) -> [usize; 3] {
        let z = linear / (self.dims[0] * self.dims[1]);
        let rem = linear % (self.dims[0] * self.dims[1]);
        let y = rem / self.dims[0];
        let x = rem % self.dims[0];
        [x, y, z]
    }

    /// Get voxel value
    #[inline]
    pub fn get(&self, voxel: [usize; 3]) -> &T {
        &self.data[self.linear_index(voxel)]
    }

    /// Get mutable voxel value
    #[inline]
    pub fn get_mut(&mut self, voxel: [usize; 3]) -> &mut T {
        let idx = self.linear_index(voxel);
        &mut self.data[idx]
    }

    /// Set voxel value
    #[inline]
    pub fn set(&mut self, voxel: [usize; 3], value: T) {
        let idx = self.linear_index(voxel);
        self.data[idx] = value;
    }

    /// Accumulate value at voxel
    pub fn accumulate(&mut self, voxel: [usize; 3], value: &T) {
        let idx = self.linear_index(voxel);
        self.data[idx].accumulate(value);
    }

    /// Accumulate value at world position
    pub fn accumulate_at(&mut self, pos: &na::Point3<f32>, value: &T) {
        if let Some(voxel) = self.world_to_voxel(pos) {
            self.accumulate(voxel, value);
        }
    }

    /// Sample with trilinear interpolation
    pub fn sample(&self, pos: &na::Point3<f32>) -> T {
        let local = pos - self.origin;
        let fx = local.x / self.voxel_size - 0.5;
        let fy = local.y / self.voxel_size - 0.5;
        let fz = local.z / self.voxel_size - 0.5;

        let x0 = fx.floor() as isize;
        let y0 = fy.floor() as isize;
        let z0 = fz.floor() as isize;

        let tx = fx - fx.floor();
        let ty = fy - fy.floor();
        let tz = fz - fz.floor();

        // Safe sample with boundary clamping
        let sample_safe = |x: isize, y: isize, z: isize| -> &T {
            let cx = x.clamp(0, self.dims[0] as isize - 1) as usize;
            let cy = y.clamp(0, self.dims[1] as isize - 1) as usize;
            let cz = z.clamp(0, self.dims[2] as isize - 1) as usize;
            self.get([cx, cy, cz])
        };

        // 8 corners
        let c000 = sample_safe(x0, y0, z0);
        let c100 = sample_safe(x0 + 1, y0, z0);
        let c010 = sample_safe(x0, y0 + 1, z0);
        let c110 = sample_safe(x0 + 1, y0 + 1, z0);
        let c001 = sample_safe(x0, y0, z0 + 1);
        let c101 = sample_safe(x0 + 1, y0, z0 + 1);
        let c011 = sample_safe(x0, y0 + 1, z0 + 1);
        let c111 = sample_safe(x0 + 1, y0 + 1, z0 + 1);

        // Trilinear interpolation
        let c00 = c000.lerp(c100, tx);
        let c10 = c010.lerp(c110, tx);
        let c01 = c001.lerp(c101, tx);
        let c11 = c011.lerp(c111, tx);

        let c0 = c00.lerp(&c10, ty);
        let c1 = c01.lerp(&c11, ty);

        c0.lerp(&c1, tz)
    }

    /// Get bounds
    pub fn bounds(&self) -> (na::Point3<f32>, na::Point3<f32>) {
        let max = self.origin
            + na::Vector3::new(
                self.dims[0] as f32 * self.voxel_size,
                self.dims[1] as f32 * self.voxel_size,
                self.dims[2] as f32 * self.voxel_size,
            );
        (self.origin, max)
    }
}

// ============================================================================
// Common Voxel Data Types
// ============================================================================

/// Simple density voxel (f32)
impl VoxelData for f32 {
    fn accumulate(&mut self, other: &Self) {
        *self += other;
    }

    fn lerp(&self, other: &Self, t: f32) -> Self {
        self + (other - self) * t
    }
}

/// TSDF voxel for signed distance fields
#[derive(Clone, Default)]
pub struct TsdfVoxel {
    /// Signed distance value
    pub tsdf: f32,
    /// Integration weight
    pub weight: f32,
    /// Color (RGB)
    pub color: [f32; 3],
}

impl VoxelData for TsdfVoxel {
    fn accumulate(&mut self, other: &Self) {
        if other.weight > 0.0 {
            let w_sum = self.weight + other.weight;
            self.tsdf = (self.tsdf * self.weight + other.tsdf * other.weight) / w_sum;
            for i in 0..3 {
                self.color[i] =
                    (self.color[i] * self.weight + other.color[i] * other.weight) / w_sum;
            }
            self.weight = w_sum;
        }
    }

    fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            tsdf: self.tsdf + (other.tsdf - self.tsdf) * t,
            weight: self.weight + (other.weight - self.weight) * t,
            color: [
                self.color[0] + (other.color[0] - self.color[0]) * t,
                self.color[1] + (other.color[1] - self.color[1]) * t,
                self.color[2] + (other.color[2] - self.color[2]) * t,
            ],
        }
    }
}

/// Coverage voxel for heatmaps
#[derive(Clone, Default)]
pub struct CoverageVoxel {
    /// Number of observations
    pub count: u32,
    /// Accumulated weight
    pub weight: f32,
}

impl VoxelData for CoverageVoxel {
    fn accumulate(&mut self, other: &Self) {
        self.count += other.count;
        self.weight += other.weight;
    }

    fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            count: ((self.count as f32 + (other.count as f32 - self.count as f32) * t) as u32),
            weight: self.weight + (other.weight - self.weight) * t,
        }
    }
}

/// Confidence voxel for uncertainty mapping
#[derive(Clone, Default)]
pub struct ConfidenceVoxel {
    /// View count
    pub view_count: u32,
    /// Accumulated confidence
    pub confidence: f32,
    /// Time of last update
    pub last_update: f32,
}

impl VoxelData for ConfidenceVoxel {
    fn accumulate(&mut self, other: &Self) {
        self.view_count += other.view_count;
        self.confidence = (self.confidence + other.confidence).min(1.0);
        self.last_update = self.last_update.max(other.last_update);
    }

    fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            view_count: ((self.view_count as f32
                + (other.view_count as f32 - self.view_count as f32) * t)
                as u32),
            confidence: self.confidence + (other.confidence - self.confidence) * t,
            last_update: self.last_update + (other.last_update - self.last_update) * t,
        }
    }
}

/// Density voxel with color
#[derive(Clone, Default)]
pub struct DensityVoxel {
    /// Accumulated density/opacity
    pub density: f32,
    /// Weighted color
    pub color: [f32; 3],
    /// Weight for averaging
    pub weight: f32,
}

impl VoxelData for DensityVoxel {
    fn accumulate(&mut self, other: &Self) {
        self.density += other.density;
        self.weight += other.weight;
        for i in 0..3 {
            self.color[i] += other.color[i];
        }
    }

    fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            density: self.density + (other.density - self.density) * t,
            weight: self.weight + (other.weight - self.weight) * t,
            color: [
                self.color[0] + (other.color[0] - self.color[0]) * t,
                self.color[1] + (other.color[1] - self.color[1]) * t,
                self.color[2] + (other.color[2] - self.color[2]) * t,
            ],
        }
    }
}

impl DensityVoxel {
    /// Get normalized color
    pub fn normalized_color(&self) -> [f32; 3] {
        if self.weight > 0.0 {
            [
                self.color[0] / self.weight,
                self.color[1] / self.weight,
                self.color[2] / self.weight,
            ]
        } else {
            [0.0; 3]
        }
    }
}

// ============================================================================
// Type Aliases
// ============================================================================

/// Density grid (simple f32)
pub type DensityGrid = VoxelGrid<f32>;

/// TSDF volume grid
pub type TsdfGrid = VoxelGrid<TsdfVoxel>;

/// Coverage heatmap grid
pub type CoverageGrid = VoxelGrid<CoverageVoxel>;

/// Confidence field grid
pub type ConfidenceGrid = VoxelGrid<ConfidenceVoxel>;

/// Colored density grid
pub type ColorDensityGrid = VoxelGrid<DensityVoxel>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_voxel_grid_creation() {
        let grid: DensityGrid =
            VoxelGrid::new(na::Point3::origin(), na::Point3::new(1.0, 1.0, 1.0), 0.1);

        assert_eq!(grid.dims, [10, 10, 10]);
        assert_eq!(grid.len(), 1000);
    }

    #[test]
    fn test_world_to_voxel() {
        let grid: DensityGrid =
            VoxelGrid::new(na::Point3::origin(), na::Point3::new(1.0, 1.0, 1.0), 0.1);

        let voxel = grid.world_to_voxel(&na::Point3::new(0.55, 0.55, 0.55));
        assert_eq!(voxel, Some([5, 5, 5]));
    }

    #[test]
    fn test_accumulate() {
        let mut grid: DensityGrid =
            VoxelGrid::new(na::Point3::origin(), na::Point3::new(1.0, 1.0, 1.0), 0.1);

        grid.accumulate_at(&na::Point3::new(0.5, 0.5, 0.5), &1.0);
        grid.accumulate_at(&na::Point3::new(0.5, 0.5, 0.5), &0.5);

        let voxel = grid
            .world_to_voxel(&na::Point3::new(0.5, 0.5, 0.5))
            .unwrap();
        assert_eq!(*grid.get(voxel), 1.5);
    }

    #[test]
    fn test_tsdf_accumulate() {
        let mut v1 = TsdfVoxel {
            tsdf: 0.5,
            weight: 1.0,
            color: [1.0, 0.0, 0.0],
        };
        let v2 = TsdfVoxel {
            tsdf: -0.5,
            weight: 1.0,
            color: [0.0, 1.0, 0.0],
        };

        v1.accumulate(&v2);

        assert!((v1.tsdf - 0.0).abs() < 0.001);
        assert_eq!(v1.weight, 2.0);
        assert!((v1.color[0] - 0.5).abs() < 0.001);
    }
}
