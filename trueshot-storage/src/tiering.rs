use std::path::Path;

/// Storage Tiering
/// Moves older projects to "Cold Storage"
pub fn tier_project(project_path: &Path, archive_path: &Path) -> std::io::Result<()> {
    if !archive_path.exists() {
        std::fs::create_dir_all(archive_path)?;
    }

    let raw_src = project_path.join("raw");
    let raw_dst = archive_path.join(project_path.file_name().unwrap()).join("raw");

    if raw_src.exists() {
        if !raw_dst.parent().unwrap().exists() {
            std::fs::create_dir_all(raw_dst.parent().unwrap())?;
        }
        std::fs::rename(raw_src, raw_dst)?;
    }
    
    Ok(())
}
