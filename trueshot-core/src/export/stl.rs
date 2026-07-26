//! STL Export
//!
//! Exports triangle meshes to binary STL.

use crate::export::write_provenance_for_export;
use crate::reconstruction::Mesh;
use anyhow::{Context, Result};
use std::io::{BufWriter, Write};
use std::path::Path;

/// Export mesh to binary STL format
pub fn export_stl(mesh: &Mesh, path: &Path) -> Result<()> {
    if mesh.is_empty() {
        anyhow::bail!("Cannot export empty mesh");
    }

    let file = std::fs::File::create(path)
        .with_context(|| format!("Failed to create STL file: {}", path.display()))?;
    let mut writer = BufWriter::new(file);

    // 80-byte header
    let mut header = [0u8; 80];
    let tag = b"TrueShot STL export";
    header[..tag.len()].copy_from_slice(tag);
    writer.write_all(&header)?;

    // Number of triangles (u32)
    let tri_count = mesh.faces.len() as u32;
    writer.write_all(&tri_count.to_le_bytes())?;

    for face in &mesh.faces {
        let v0 = mesh.vertices[face.vertices[0]];
        let v1 = mesh.vertices[face.vertices[1]];
        let v2 = mesh.vertices[face.vertices[2]];

        let normal = compute_face_normal(v0, v1, v2);
        writer.write_all(&normal[0].to_le_bytes())?;
        writer.write_all(&normal[1].to_le_bytes())?;
        writer.write_all(&normal[2].to_le_bytes())?;

        writer.write_all(&v0.x.to_le_bytes())?;
        writer.write_all(&v0.y.to_le_bytes())?;
        writer.write_all(&v0.z.to_le_bytes())?;

        writer.write_all(&v1.x.to_le_bytes())?;
        writer.write_all(&v1.y.to_le_bytes())?;
        writer.write_all(&v1.z.to_le_bytes())?;

        writer.write_all(&v2.x.to_le_bytes())?;
        writer.write_all(&v2.y.to_le_bytes())?;
        writer.write_all(&v2.z.to_le_bytes())?;

        // Attribute byte count (unused)
        writer.write_all(&0u16.to_le_bytes())?;
    }

    writer.flush()?;
    write_provenance_for_export(path)?;
    Ok(())
}

fn compute_face_normal(
    v0: nalgebra::Point3<f32>,
    v1: nalgebra::Point3<f32>,
    v2: nalgebra::Point3<f32>,
) -> [f32; 3] {
    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let n = e1.cross(&e2);
    let len = n.norm();
    if len > 1e-10 {
        let nn = n / len;
        [nn.x, nn.y, nn.z]
    } else {
        [0.0, 0.0, 0.0]
    }
}
