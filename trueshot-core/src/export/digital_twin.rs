use crate::export::write_provenance_for_export;
use std::fs::File;
use std::io::Write;

/// Digital Twin Export (GLB with embedded metadata)
/// Writes a prebuilt GLB payload and attaches provenance metadata.
pub fn export_digital_twin(path: &str, mesh_data: &[u8]) -> anyhow::Result<()> {
    if mesh_data.is_empty() {
        anyhow::bail!("Digital twin export requires non-empty GLB payload");
    }

    let mut file = File::create(path)?;
    file.write_all(mesh_data)?;
    write_provenance_for_export(std::path::Path::new(path))?;
    Ok(())
}
