use actix_web::HttpResponse;
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;
use sysinfo::Disks;

const MAX_ID_LEN: usize = 128;
const MAX_FILENAME_LEN: usize = 255;

pub fn ensure_project_id(id: &str) -> Result<(), HttpResponse> {
    if id.is_empty() || id.len() > MAX_ID_LEN {
        return Err(HttpResponse::BadRequest().body("Invalid project id length"));
    }
    if id.chars().any(|c| c.is_control()) {
        return Err(HttpResponse::BadRequest().body("Invalid project id characters"));
    }
    if id.contains('/') || id.contains('\\') || id.contains(':') {
        return Err(HttpResponse::BadRequest().body("Project id must be a simple name"));
    }
    if id == "." || id == ".." || id.contains("..") {
        return Err(HttpResponse::BadRequest().body("Project id cannot contain path traversal"));
    }
    Ok(())
}

pub fn ensure_filename(name: &str) -> Result<(), HttpResponse> {
    if name.is_empty() || name.len() > MAX_FILENAME_LEN {
        return Err(HttpResponse::BadRequest().body("Invalid filename length"));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(HttpResponse::BadRequest().body("Invalid filename characters"));
    }
    if name.contains('/') || name.contains('\\') || name.contains(':') {
        return Err(HttpResponse::BadRequest().body("Filename must be a single path segment"));
    }
    if name == "." || name == ".." || name.contains("..") {
        return Err(HttpResponse::BadRequest().body("Filename cannot contain path traversal"));
    }
    Ok(())
}

pub fn canonicalize_root(root: &Path) -> Result<PathBuf, HttpResponse> {
    if !root.exists() {
        std::fs::create_dir_all(root)
            .map_err(|e| HttpResponse::InternalServerError().body(format!("Failed to create root: {e}")))?;
    }
    root.canonicalize()
        .map_err(|e| HttpResponse::InternalServerError().body(format!("Failed to resolve root: {e}")))
}

pub fn resolve_project_dir(root: &Path, id: &str) -> Result<PathBuf, HttpResponse> {
    ensure_project_id(id)?;
    let root = canonicalize_root(root)?;
    let candidate = root.join(id);
    if candidate.exists() {
        let canon = candidate
            .canonicalize()
            .map_err(|e| HttpResponse::InternalServerError().body(format!("Failed to resolve project: {e}")))?;
        if !canon.starts_with(&root) {
            return Err(HttpResponse::BadRequest().body("Invalid project path"));
        }
        Ok(canon)
    } else {
        let normalized = normalize_path(&candidate);
        if !normalized.starts_with(&root) {
            return Err(HttpResponse::BadRequest().body("Invalid project path"));
        }
        Ok(normalized)
    }
}

pub fn resolve_project_child(root: &Path, id: &str, child: &str) -> Result<PathBuf, HttpResponse> {
    let project_dir = resolve_project_dir(root, id)?;
    let candidate = project_dir.join(child);
    if candidate.exists() {
        let canon = candidate
            .canonicalize()
            .map_err(|e| HttpResponse::InternalServerError().body(format!("Failed to resolve project child: {e}")))?;
        if !canon.starts_with(&project_dir) {
            return Err(HttpResponse::BadRequest().body("Invalid project child path"));
        }
        Ok(canon)
    } else {
        let normalized = normalize_path(&candidate);
        if !normalized.starts_with(&project_dir) {
            return Err(HttpResponse::BadRequest().body("Invalid project child path"));
        }
        Ok(normalized)
    }
}

pub fn resolve_project_file(root: &Path, id: &str, filename: &str) -> Result<PathBuf, HttpResponse> {
    ensure_filename(filename)?;
    let project_dir = resolve_project_dir(root, id)?;
    let candidate = project_dir.join(filename);
    let normalized = normalize_path(&candidate);
    if !normalized.starts_with(&project_dir) {
        return Err(HttpResponse::BadRequest().body("Invalid project file path"));
    }
    Ok(normalized)
}

pub fn resolve_project_child_file(
    root: &Path,
    id: &str,
    child: &str,
    rel_path: &str,
) -> Result<PathBuf, HttpResponse> {
    if rel_path.is_empty() {
        return Err(HttpResponse::BadRequest().body("Missing file path"));
    }
    let rel = Path::new(rel_path);
    if rel.is_absolute() {
        return Err(HttpResponse::BadRequest().body("Invalid file path"));
    }
    for component in rel.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(HttpResponse::BadRequest().body("Invalid file path"));
            }
            _ => {}
        }
    }
    let project_dir = resolve_project_dir(root, id)?;
    let base = project_dir.join(child);
    let candidate = base.join(rel);
    let normalized = normalize_path(&candidate);
    if !normalized.starts_with(&base) {
        return Err(HttpResponse::BadRequest().body("Invalid project child path"));
    }
    Ok(normalized)
}

pub fn project_size_bytes(path: &Path) -> Result<u64, HttpResponse> {
    let mut total = 0u64;
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry.map_err(|e| HttpResponse::InternalServerError().body(format!("Walk failed: {e}")))?;
        if entry.file_type().is_file() {
            let meta = entry.metadata()
                .map_err(|e| HttpResponse::InternalServerError().body(format!("Metadata failed: {e}")))?;
            total = total.saturating_add(meta.len());
        }
    }
    Ok(total)
}

pub fn available_space_bytes(path: &Path) -> Option<u64> {
    let mut disks = Disks::new_with_refreshed_list();
    disks.refresh(false);

    let mut candidate = path.to_path_buf();
    while !candidate.exists() {
        if !candidate.pop() {
            break;
        }
    }

    let mut best: Option<(usize, u64)> = None;
    for disk in disks.list() {
        let mount = disk.mount_point();
        if candidate.starts_with(mount) {
            let len = mount.as_os_str().len();
            if best.map_or(true, |(best_len, _)| len > best_len) {
                best = Some((len, disk.available_space()));
            }
        }
    }

    best.map(|(_, space)| space)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}
