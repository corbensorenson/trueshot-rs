//! Universal Scene Description (USD) Export
//!
//! Exports meshes to USD format with full attribute support.
//! Outputs ASCII .usda files for maximum compatibility.

use crate::export::write_provenance_for_export;
use crate::reconstruction::Mesh;
use crate::security::provenance::ProvenanceSigner;
use anyhow::{Context, Result};
use std::fmt::Write;
use std::path::Path;

/// USD export options
#[derive(Clone, Debug)]
pub struct UsdExportOptions {
    /// Include vertex normals
    pub include_normals: bool,
    /// Include texture coordinates
    pub include_uvs: bool,
    /// Include vertex colors
    pub include_colors: bool,
    /// Up axis (Y or Z)
    pub up_axis: UpAxis,
    /// Scene scale in meters per unit
    pub meters_per_unit: f32,
}

/// Up axis for USD scene
#[derive(Clone, Debug, Default)]
pub enum UpAxis {
    #[default]
    Y,
    Z,
}

impl Default for UsdExportOptions {
    fn default() -> Self {
        Self {
            include_normals: false,
            include_uvs: false,
            include_colors: false,
            up_axis: UpAxis::Y,
            meters_per_unit: 1.0,
        }
    }
}

/// Export mesh as Universal Scene Description (ASCII .usda)
pub fn export_usd(mesh: &Mesh, path: &Path) -> Result<()> {
    export_usd_with_options(mesh, path, &UsdExportOptions::default())
}

/// Export mesh with custom options
pub fn export_usd_with_options(mesh: &Mesh, path: &Path, options: &UsdExportOptions) -> Result<()> {
    let asset_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("asset.usda");
    let out = build_usda_document(mesh, options, asset_name)?;

    std::fs::write(path, out)
        .with_context(|| format!("Failed to write USD file: {}", path.display()))?;
    write_provenance_for_export(path)?;
    Ok(())
}

pub(crate) fn build_usda_document(
    mesh: &Mesh,
    options: &UsdExportOptions,
    asset_name: &str,
) -> Result<String> {
    let mut out = String::new();
    let provenance_sidecar = format!("{}.provenance.json", asset_name);
    let provenance_key_id = ProvenanceSigner::global().key_id();

    // Header
    writeln!(&mut out, "#usda 1.0")?;
    writeln!(&mut out, "(")?;
    writeln!(&mut out, "    defaultPrim = \"TrueShotExport\"")?;
    let meters_per_unit = options.meters_per_unit.max(1e-6);
    writeln!(&mut out, "    metersPerUnit = {}", meters_per_unit)?;
    match options.up_axis {
        UpAxis::Y => writeln!(&mut out, "    upAxis = \"Y\"")?,
        UpAxis::Z => writeln!(&mut out, "    upAxis = \"Z\"")?,
    }
    writeln!(&mut out, "    customLayerData = {{")?;
    writeln!(
        &mut out,
        "        string trueshot:provenanceSidecar = \"{}\"",
        provenance_sidecar
    )?;
    writeln!(
        &mut out,
        "        string trueshot:provenanceKeyId = \"{}\"",
        provenance_key_id
    )?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, ")")?;
    writeln!(&mut out)?;

    // Root Xform
    writeln!(&mut out, "def Xform \"TrueShotExport\" {{")?;
    writeln!(&mut out, "    def Mesh \"Mesh_0\" {{")?;

    // Points (vertices)
    writeln!(&mut out, "        point3f[] points = [")?;
    for (i, v) in mesh.vertices.iter().enumerate() {
        let comma = if i < mesh.vertices.len() - 1 { "," } else { "" };
        writeln!(&mut out, "            ({}, {}, {}){}", v.x, v.y, v.z, comma)?;
    }
    writeln!(&mut out, "        ]")?;

    // Face vertex counts (all triangles = 3)
    write!(&mut out, "        int[] faceVertexCounts = [")?;
    for (i, _) in mesh.faces.iter().enumerate() {
        let comma = if i < mesh.faces.len() - 1 { ", " } else { "" };
        write!(&mut out, "3{}", comma)?;
    }
    writeln!(&mut out, "]")?;

    // Face vertex indices
    write!(&mut out, "        int[] faceVertexIndices = [")?;
    for (i, f) in mesh.faces.iter().enumerate() {
        let comma = if i < mesh.faces.len() - 1 { ", " } else { "" };
        write!(
            &mut out,
            "{}, {}, {}{}",
            f.vertices[0], f.vertices[1], f.vertices[2], comma
        )?;
    }
    writeln!(&mut out, "]")?;

    // Normals (if available and requested)
    if options.include_normals && !mesh.normals.is_empty() {
        writeln!(&mut out, "        normal3f[] normals = [")?;
        for (i, n) in mesh.normals.iter().enumerate() {
            let comma = if i < mesh.normals.len() - 1 { "," } else { "" };
            writeln!(&mut out, "            ({}, {}, {}){}", n.x, n.y, n.z, comma)?;
        }
        writeln!(&mut out, "        ] (")?;
        writeln!(&mut out, "            interpolation = \"vertex\"")?;
        writeln!(&mut out, "        )")?;
    }

    // UVs (if available and requested)
    if options.include_uvs && !mesh.uvs.is_empty() {
        writeln!(&mut out, "        texCoord2f[] primvars:st = [")?;
        for (i, uv) in mesh.uvs.iter().enumerate() {
            let comma = if i < mesh.uvs.len() - 1 { "," } else { "" };
            writeln!(&mut out, "            ({}, {}){}", uv[0], uv[1], comma)?;
        }
        writeln!(&mut out, "        ] (")?;
        writeln!(&mut out, "            interpolation = \"vertex\"")?;
        writeln!(&mut out, "        )")?;
    }

    // Vertex colors (if available and requested)
    if options.include_colors && !mesh.colors.is_empty() {
        writeln!(&mut out, "        color3f[] primvars:displayColor = [")?;
        for (i, c) in mesh.colors.iter().enumerate() {
            let comma = if i < mesh.colors.len() - 1 { "," } else { "" };
            let r = c[0] as f32 / 255.0;
            let g = c[1] as f32 / 255.0;
            let b = c[2] as f32 / 255.0;
            writeln!(&mut out, "            ({}, {}, {}){}", r, g, b, comma)?;
        }
        writeln!(&mut out, "        ] (")?;
        writeln!(&mut out, "            interpolation = \"vertex\"")?;
        writeln!(&mut out, "        )")?;
    }

    // Extent (bounding box)
    if !mesh.vertices.is_empty() {
        let mut min = mesh.vertices[0];
        let mut max = mesh.vertices[0];
        for v in &mesh.vertices {
            min.x = min.x.min(v.x);
            min.y = min.y.min(v.y);
            min.z = min.z.min(v.z);
            max.x = max.x.max(v.x);
            max.y = max.y.max(v.y);
            max.z = max.z.max(v.z);
        }
        writeln!(
            &mut out,
            "        float3[] extent = [({}, {}, {}), ({}, {}, {})]",
            min.x, min.y, min.z, max.x, max.y, max.z
        )?;
    }

    // Subdivision scheme (none = polygon mesh)
    writeln!(&mut out, "        token subdivisionScheme = \"none\"")?;

    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_usd_export_basic() {
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
        let path = dir.path().join("test.usda");

        export_usd(&mesh, &path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("#usda 1.0"));
        assert!(content.contains("point3f[] points"));
        assert!(content.contains("faceVertexIndices"));
    }
}
