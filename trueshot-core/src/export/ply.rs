//! PLY (Polygon File Format) Export
//!
//! Exports meshes and point clouds to PLY format with full attribute support.
//! Supports both ASCII and binary formats.

use crate::export::write_provenance_for_export;
use crate::reconstruction::Mesh;
use anyhow::{Context, Result};
use std::io::{BufWriter, Write};
use std::path::Path;

/// PLY export options
#[derive(Clone, Debug)]
pub struct PlyExportOptions {
    /// Use binary format (more compact, faster to load)
    pub binary: bool,
    /// Include vertex normals
    pub include_normals: bool,
    /// Include vertex colors
    pub include_colors: bool,
    /// Include texture coordinates
    pub include_uvs: bool,
    /// Comment to include in header
    pub comment: Option<String>,
}

impl Default for PlyExportOptions {
    fn default() -> Self {
        Self {
            binary: false, // ASCII for compatibility
            include_normals: true,
            include_colors: true,
            include_uvs: true,
            comment: Some("Exported by TrueShot".to_string()),
        }
    }
}

/// Export mesh to PLY format
pub fn export_ply(mesh: &Mesh, path: &Path, options: &PlyExportOptions) -> Result<()> {
    let file = std::fs::File::create(path)
        .with_context(|| format!("Failed to create PLY file: {}", path.display()))?;
    export_ply_to_writer(mesh, file, options)?;
    write_provenance_for_export(path)?;
    Ok(())
}

pub fn export_ply_to_writer<W: Write>(
    mesh: &Mesh,
    writer: W,
    options: &PlyExportOptions,
) -> Result<()> {
    let mut writer = BufWriter::new(writer);
    // Write header
    writeln!(writer, "ply")?;

    if options.binary {
        writeln!(writer, "format binary_little_endian 1.0")?;
    } else {
        writeln!(writer, "format ascii 1.0")?;
    }

    if let Some(ref comment) = options.comment {
        writeln!(writer, "comment {}", comment)?;
    }

    // Vertex element
    writeln!(writer, "element vertex {}", mesh.vertices.len())?;
    writeln!(writer, "property float x")?;
    writeln!(writer, "property float y")?;
    writeln!(writer, "property float z")?;

    if options.include_normals {
        writeln!(writer, "property float nx")?;
        writeln!(writer, "property float ny")?;
        writeln!(writer, "property float nz")?;
    }

    if options.include_colors {
        writeln!(writer, "property uchar red")?;
        writeln!(writer, "property uchar green")?;
        writeln!(writer, "property uchar blue")?;
    }

    if options.include_uvs {
        writeln!(writer, "property float s")?;
        writeln!(writer, "property float t")?;
    }

    // Face element
    writeln!(writer, "element face {}", mesh.faces.len())?;
    writeln!(writer, "property list uchar int vertex_indices")?;

    writeln!(writer, "end_header")?;

    // Write vertex data
    if options.binary {
        write_vertices_binary(&mut writer, mesh, options)?;
        write_faces_binary(&mut writer, mesh)?;
    } else {
        write_vertices_ascii(&mut writer, mesh, options)?;
        write_faces_ascii(&mut writer, mesh)?;
    }

    writer.flush()?;
    Ok(())
}

fn write_vertices_ascii<W: Write>(
    writer: &mut W,
    mesh: &Mesh,
    options: &PlyExportOptions,
) -> Result<()> {
    for (i, vertex) in mesh.vertices.iter().enumerate() {
        // Position
        write!(writer, "{} {} {}", vertex.x, vertex.y, vertex.z)?;

        // Normals
        if options.include_normals {
            let normal = mesh
                .normals
                .get(i)
                .copied()
                .unwrap_or(nalgebra::Vector3::z());
            write!(writer, " {} {} {}", normal.x, normal.y, normal.z)?;
        }

        // Colors
        if options.include_colors {
            let color = mesh.colors.get(i).copied().unwrap_or([255, 255, 255]);
            write!(writer, " {} {} {}", color[0], color[1], color[2])?;
        }

        // UVs
        if options.include_uvs {
            let uv = mesh.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
            write!(writer, " {} {}", uv[0], uv[1])?;
        }

        writeln!(writer)?;
    }
    Ok(())
}

fn write_faces_ascii<W: Write>(writer: &mut W, mesh: &Mesh) -> Result<()> {
    for face in &mesh.faces {
        writeln!(
            writer,
            "3 {} {} {}",
            face.vertices[0], face.vertices[1], face.vertices[2]
        )?;
    }
    Ok(())
}

fn write_vertices_binary<W: Write>(
    writer: &mut W,
    mesh: &Mesh,
    options: &PlyExportOptions,
) -> Result<()> {
    for (i, vertex) in mesh.vertices.iter().enumerate() {
        // Position
        writer.write_all(&vertex.x.to_le_bytes())?;
        writer.write_all(&vertex.y.to_le_bytes())?;
        writer.write_all(&vertex.z.to_le_bytes())?;

        // Normals
        if options.include_normals {
            let normal = mesh
                .normals
                .get(i)
                .copied()
                .unwrap_or(nalgebra::Vector3::z());
            writer.write_all(&normal.x.to_le_bytes())?;
            writer.write_all(&normal.y.to_le_bytes())?;
            writer.write_all(&normal.z.to_le_bytes())?;
        }

        // Colors
        if options.include_colors {
            let color = mesh.colors.get(i).copied().unwrap_or([255, 255, 255]);
            writer.write_all(&color)?;
        }

        // UVs
        if options.include_uvs {
            let uv = mesh.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
            writer.write_all(&uv[0].to_le_bytes())?;
            writer.write_all(&uv[1].to_le_bytes())?;
        }
    }
    Ok(())
}

fn write_faces_binary<W: Write>(writer: &mut W, mesh: &Mesh) -> Result<()> {
    for face in &mesh.faces {
        writer.write_all(&[3u8])?; // Triangle
        writer.write_all(&(face.vertices[0] as i32).to_le_bytes())?;
        writer.write_all(&(face.vertices[1] as i32).to_le_bytes())?;
        writer.write_all(&(face.vertices[2] as i32).to_le_bytes())?;
    }
    Ok(())
}

/// Export point cloud to PLY format
pub fn export_point_cloud_ply(
    points: &[nalgebra::Point3<f32>],
    colors: Option<&[[u8; 3]]>,
    normals: Option<&[nalgebra::Vector3<f32>]>,
    path: &Path,
) -> Result<()> {
    let file = std::fs::File::create(path)
        .with_context(|| format!("Failed to create PLY file: {}", path.display()))?;
    let mut writer = BufWriter::new(file);

    // Header
    writeln!(writer, "ply")?;
    writeln!(writer, "format ascii 1.0")?;
    writeln!(writer, "comment Exported by TrueShot")?;
    writeln!(writer, "element vertex {}", points.len())?;
    writeln!(writer, "property float x")?;
    writeln!(writer, "property float y")?;
    writeln!(writer, "property float z")?;

    if normals.is_some() {
        writeln!(writer, "property float nx")?;
        writeln!(writer, "property float ny")?;
        writeln!(writer, "property float nz")?;
    }

    if colors.is_some() {
        writeln!(writer, "property uchar red")?;
        writeln!(writer, "property uchar green")?;
        writeln!(writer, "property uchar blue")?;
    }

    writeln!(writer, "end_header")?;

    // Data
    for (i, point) in points.iter().enumerate() {
        write!(writer, "{} {} {}", point.x, point.y, point.z)?;

        if let Some(normals) = normals {
            let n = normals.get(i).copied().unwrap_or(nalgebra::Vector3::z());
            write!(writer, " {} {} {}", n.x, n.y, n.z)?;
        }

        if let Some(colors) = colors {
            let c = colors.get(i).copied().unwrap_or([255, 255, 255]);
            write!(writer, " {} {} {}", c[0], c[1], c[2])?;
        }

        writeln!(writer)?;
    }

    writer.flush()?;
    write_provenance_for_export(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_ply_export_basic() {
        let mesh = Mesh {
            vertices: vec![
                nalgebra::Point3::new(0.0, 0.0, 0.0),
                nalgebra::Point3::new(1.0, 0.0, 0.0),
                nalgebra::Point3::new(0.0, 1.0, 0.0),
            ],
            faces: vec![crate::reconstruction::Face {
                vertices: [0, 1, 2],
            }],
            normals: vec![],
            colors: vec![],
            uvs: vec![],
        };

        let dir = tempdir().unwrap();
        let path = dir.path().join("test.ply");

        let options = PlyExportOptions {
            include_normals: false,
            include_colors: false,
            include_uvs: false,
            ..Default::default()
        };

        export_ply(&mesh, &path, &options).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("ply"));
        assert!(content.contains("element vertex 3"));
        assert!(content.contains("element face 1"));
    }
}
