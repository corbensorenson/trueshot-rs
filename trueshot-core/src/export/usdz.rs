//! USDZ (zip bundle) Export
//!
//! Packages an ASCII USDA mesh into a USDZ container for Apple Quick Look.

use crate::export::usd::{build_usda_document, UsdExportOptions};
use crate::export::write_provenance_for_export;
use crate::reconstruction::Mesh;
use anyhow::{Context, Result};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use zip::write::FileOptions;
use zip::CompressionMethod;
use zip::ZipWriter;

/// Export mesh as USDZ (zip bundle)
pub fn export_usdz(mesh: &Mesh, path: &Path) -> Result<()> {
    export_usdz_with_options(mesh, path, &UsdExportOptions::default())
}

/// Export mesh as USDZ with options
pub fn export_usdz_with_options(
    mesh: &Mesh,
    path: &Path,
    options: &UsdExportOptions,
) -> Result<()> {
    let asset_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("asset.usdz");
    let usda = build_usda_document(mesh, options, asset_name)?;

    let file =
        File::create(path).with_context(|| format!("Failed to create USDZ: {}", path.display()))?;
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(CompressionMethod::Stored);

    zip.start_file("model.usda", options)?;
    zip.write_all(usda.as_bytes())?;
    zip.finish()?;

    write_provenance_for_export(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use zip::ZipArchive;

    #[test]
    fn test_usdz_export_contains_usda() {
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

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.usdz");
        export_usdz(&mesh, &path).unwrap();

        let file = File::open(&path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut entry = archive.by_name("model.usda").unwrap();
        let mut contents = String::new();
        entry.read_to_string(&mut contents).unwrap();
        assert!(contents.contains("#usda 1.0"));
    }
}
