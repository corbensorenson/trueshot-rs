//! OBJ Export
//!
//! Exports meshes to Wavefront OBJ with optional normals and UVs.

use crate::reconstruction::Mesh;
use anyhow::{Context, Result};
use std::io::{BufWriter, Write};
use std::path::Path;
use crate::export::write_provenance_for_export;

/// Export mesh to OBJ format
pub fn export_obj(mesh: &Mesh, path: &Path) -> Result<()> {
    if mesh.is_empty() {
        anyhow::bail!("Cannot export empty mesh");
    }

    let file = std::fs::File::create(path)
        .with_context(|| format!("Failed to create OBJ file: {}", path.display()))?;
    let mut writer = BufWriter::new(file);

    writeln!(writer, "# TrueShot OBJ export")?;
    writeln!(writer, "# Vertices: {}", mesh.vertices.len())?;
    writeln!(writer, "# Faces: {}", mesh.faces.len())?;

    // Vertices
    for v in &mesh.vertices {
        writeln!(writer, "v {} {} {}", v.x, v.y, v.z)?;
    }

    // UVs
    if !mesh.uvs.is_empty() {
        for uv in &mesh.uvs {
            writeln!(writer, "vt {} {}", uv[0], uv[1])?;
        }
    }

    // Normals
    if !mesh.normals.is_empty() {
        for n in &mesh.normals {
            writeln!(writer, "vn {} {} {}", n.x, n.y, n.z)?;
        }
    }

    let has_uvs = !mesh.uvs.is_empty();
    let has_normals = !mesh.normals.is_empty();

    // Faces (OBJ is 1-indexed)
    for face in &mesh.faces {
        let a = face.vertices[0] + 1;
        let b = face.vertices[1] + 1;
        let c = face.vertices[2] + 1;

        if has_uvs && has_normals {
            writeln!(writer, "f {0}/{0}/{0} {1}/{1}/{1} {2}/{2}/{2}", a, b, c)?;
        } else if has_uvs {
            writeln!(writer, "f {0}/{0} {1}/{1} {2}/{2}", a, b, c)?;
        } else if has_normals {
            writeln!(writer, "f {0}//{0} {1}//{1} {2}//{2}", a, b, c)?;
        } else {
            writeln!(writer, "f {} {} {}", a, b, c)?;
        }
    }

    writer.flush()?;
    write_provenance_for_export(path)?;
    Ok(())
}
