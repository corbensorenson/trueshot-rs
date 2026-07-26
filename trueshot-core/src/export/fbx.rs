//! FBX Export (ASCII)
//!
//! Provides a minimal FBX 7.4 ASCII mesh export for compatibility with DCC tools.

use crate::export::write_provenance_for_export;
use crate::reconstruction::Mesh;
use anyhow::{Context, Result};
use std::fmt::Write;
use std::path::Path;

/// Export mesh to FBX ASCII format
pub fn export_fbx(mesh: &Mesh, path: &Path) -> Result<()> {
    if mesh.is_empty() {
        anyhow::bail!("Cannot export empty mesh");
    }

    let mut out = String::new();

    writeln!(&mut out, "; FBX 7.4.0 project file")?;
    writeln!(&mut out, "FBXHeaderExtension:  {{")?;
    writeln!(&mut out, "    FBXHeaderVersion: 1003")?;
    writeln!(&mut out, "    FBXVersion: 7400")?;
    writeln!(&mut out, "    Creator: \"TrueShot\"")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out, "GlobalSettings:  {{")?;
    writeln!(&mut out, "    Version: 1000")?;
    writeln!(&mut out, "    Properties70:  {{")?;
    writeln!(&mut out, "        P: \"UpAxis\", \"int\", \"Integer\", \"\", 1")?;
    writeln!(&mut out, "        P: \"UpAxisSign\", \"int\", \"Integer\", \"\", 1")?;
    writeln!(&mut out, "        P: \"UnitScaleFactor\", \"double\", \"Number\", \"\", 1")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;

    writeln!(&mut out, "Definitions:  {{")?;
    writeln!(&mut out, "    Version: 100")?;
    writeln!(&mut out, "    Count: 2")?;
    writeln!(&mut out, "    ObjectType: \"Model\" {{")?;
    writeln!(&mut out, "        Count: 2")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "    ObjectType: \"Geometry\" {{")?;
    writeln!(&mut out, "        Count: 1")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;

    writeln!(&mut out, "Objects:  {{")?;
    writeln!(&mut out, "    Model: 0, \"Model::Scene\", \"Null\" {{")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "    Model: 2, \"Model::Mesh\", \"Mesh\" {{")?;
    writeln!(&mut out, "        Version: 232")?;
    writeln!(&mut out, "        Properties70:  {{")?;
    writeln!(&mut out, "            P: \"Lcl Translation\", \"Lcl Translation\", \"\", \"A\", 0,0,0")?;
    writeln!(&mut out, "            P: \"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\", 0,0,0")?;
    writeln!(&mut out, "            P: \"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\", 1,1,1")?;
    writeln!(&mut out, "        }}")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "    Geometry: 1, \"Geometry::Mesh\", \"Mesh\" {{")?;
    writeln!(&mut out, "        GeometryVersion: 124")?;

    // Vertices
    writeln!(
        &mut out,
        "        Vertices: *{} {{",
        mesh.vertices.len() * 3
    )?;
    write!(&mut out, "            a: ")?;
    for (i, v) in mesh.vertices.iter().enumerate() {
        let comma = if i < mesh.vertices.len() - 1 { "," } else { "" };
        write!(&mut out, "{},{},{}{}", v.x, v.y, v.z, comma)?;
    }
    writeln!(&mut out)?;
    writeln!(&mut out, "        }}")?;

    // Polygon indices (triangles)
    writeln!(
        &mut out,
        "        PolygonVertexIndex: *{} {{",
        mesh.faces.len() * 3
    )?;
    write!(&mut out, "            a: ")?;
    for (i, f) in mesh.faces.iter().enumerate() {
        let a = f.vertices[0] as i32;
        let b = f.vertices[1] as i32;
        let c = f.vertices[2] as i32;
        let comma = if i < mesh.faces.len() - 1 { "," } else { "" };
        write!(&mut out, "{},{},{}{}", a, b, -(c + 1), comma)?;
    }
    writeln!(&mut out)?;
    writeln!(&mut out, "        }}")?;

    // Normals
    if !mesh.normals.is_empty() {
        writeln!(&mut out, "        LayerElementNormal: 0 {{")?;
        writeln!(&mut out, "            Version: 101")?;
        writeln!(&mut out, "            Name: \"\"")?;
        writeln!(&mut out, "            MappingInformationType: \"ByVertice\"")?;
        writeln!(&mut out, "            ReferenceInformationType: \"Direct\"")?;
        writeln!(
            &mut out,
            "            Normals: *{} {{",
            mesh.normals.len() * 3
        )?;
        write!(&mut out, "                a: ")?;
        for (i, n) in mesh.normals.iter().enumerate() {
            let comma = if i < mesh.normals.len() - 1 { "," } else { "" };
            write!(&mut out, "{},{},{}{}", n.x, n.y, n.z, comma)?;
        }
        writeln!(&mut out)?;
        writeln!(&mut out, "            }}")?;
        writeln!(&mut out, "        }}")?;
    }

    // UVs
    if !mesh.uvs.is_empty() {
        writeln!(&mut out, "        LayerElementUV: 0 {{")?;
        writeln!(&mut out, "            Version: 101")?;
        writeln!(&mut out, "            Name: \"UVChannel_1\"")?;
        writeln!(&mut out, "            MappingInformationType: \"ByVertice\"")?;
        writeln!(&mut out, "            ReferenceInformationType: \"Direct\"")?;
        writeln!(
            &mut out,
            "            UV: *{} {{",
            mesh.uvs.len() * 2
        )?;
        write!(&mut out, "                a: ")?;
        for (i, uv) in mesh.uvs.iter().enumerate() {
            let comma = if i < mesh.uvs.len() - 1 { "," } else { "" };
            write!(&mut out, "{},{}{}", uv[0], uv[1], comma)?;
        }
        writeln!(&mut out)?;
        writeln!(&mut out, "            }}")?;
        writeln!(&mut out, "        }}")?;
    }

    // Vertex colors
    if !mesh.colors.is_empty() {
        writeln!(&mut out, "        LayerElementColor: 0 {{")?;
        writeln!(&mut out, "            Version: 101")?;
        writeln!(&mut out, "            Name: \"\"")?;
        writeln!(&mut out, "            MappingInformationType: \"ByVertice\"")?;
        writeln!(&mut out, "            ReferenceInformationType: \"Direct\"")?;
        writeln!(
            &mut out,
            "            Colors: *{} {{",
            mesh.colors.len() * 4
        )?;
        write!(&mut out, "                a: ")?;
        for (i, c) in mesh.colors.iter().enumerate() {
            let comma = if i < mesh.colors.len() - 1 { "," } else { "" };
            let r = c[0] as f32 / 255.0;
            let g = c[1] as f32 / 255.0;
            let b = c[2] as f32 / 255.0;
            write!(&mut out, "{},{},{},1{}", r, g, b, comma)?;
        }
        writeln!(&mut out)?;
        writeln!(&mut out, "            }}")?;
        writeln!(&mut out, "        }}")?;
    }

    // Layer setup
    writeln!(&mut out, "        Layer: 0 {{")?;
    writeln!(&mut out, "            Version: 100")?;
    if !mesh.normals.is_empty() {
        writeln!(&mut out, "            LayerElement:  {{")?;
        writeln!(&mut out, "                Type: \"LayerElementNormal\"")?;
        writeln!(&mut out, "                TypedIndex: 0")?;
        writeln!(&mut out, "            }}")?;
    }
    if !mesh.uvs.is_empty() {
        writeln!(&mut out, "            LayerElement:  {{")?;
        writeln!(&mut out, "                Type: \"LayerElementUV\"")?;
        writeln!(&mut out, "                TypedIndex: 0")?;
        writeln!(&mut out, "            }}")?;
    }
    if !mesh.colors.is_empty() {
        writeln!(&mut out, "            LayerElement:  {{")?;
        writeln!(&mut out, "                Type: \"LayerElementColor\"")?;
        writeln!(&mut out, "                TypedIndex: 0")?;
        writeln!(&mut out, "            }}")?;
    }
    writeln!(&mut out, "        }}")?;

    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;

    writeln!(&mut out, "Connections:  {{")?;
    writeln!(&mut out, "    C: \"OO\", 1, 2")?;
    writeln!(&mut out, "    C: \"OO\", 2, 0")?;
    writeln!(&mut out, "}}")?;

    std::fs::write(path, out)
        .with_context(|| format!("Failed to write FBX file: {}", path.display()))?;
    write_provenance_for_export(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fbx_export_basic() {
        let mesh = Mesh {
            vertices: vec![
                nalgebra::Point3::new(0.0, 0.0, 0.0),
                nalgebra::Point3::new(1.0, 0.0, 0.0),
                nalgebra::Point3::new(0.0, 1.0, 0.0),
            ],
            faces: vec![crate::reconstruction::Face { vertices: [0, 1, 2] }],
            normals: vec![],
            colors: vec![],
            uvs: vec![],
        };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.fbx");
        export_fbx(&mesh, &path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("FBXVersion: 7400"));
        assert!(content.contains("Geometry::Mesh"));
    }
}
