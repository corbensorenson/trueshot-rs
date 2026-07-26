use nalgebra::Point3;
use std::fs::File;
use std::io::{Seek, Write};

/// Point Cloud Octree (.pco) Format
/// Header: Magic (4) | Version (4) | Bounds (24) | Root Offset (8)
/// Node: ChildMask (1) | PointCount (4) | PointsOffset (8) | ChildrenOffsets (8*8)
pub struct PointOctreeWriter {
    file: File,
}

impl PointOctreeWriter {
    pub fn new(path: &str) -> std::io::Result<Self> {
        let mut file = File::create(path)?;
        file.write_all(b"PCO1")?;
        Ok(Self { file })
    }

    pub fn write_node(
        &mut self,
        points: &[Point3<f32>],
        _children: Vec<u64>,
    ) -> std::io::Result<u64> {
        let offset = self.file.stream_position()?;

        // Write Node Header
        let mask = 0u8; // Calc from children
        self.file.write_all(&[mask])?;
        self.file.write_all(&(points.len() as u32).to_le_bytes())?;

        // Write Points
        for p in points {
            self.file.write_all(&p.x.to_le_bytes())?;
            self.file.write_all(&p.y.to_le_bytes())?;
            self.file.write_all(&p.z.to_le_bytes())?;
        }

        Ok(offset)
    }
}
