use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ai::model_manifest::VerifiedModelInfo;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedModelRecord {
    pub model_id: String,
    pub model_version: String,
    pub weights_sha256: String,
    pub model_path: PathBuf,
    pub manifest_path: PathBuf,
    pub activated_at: String,
}

#[derive(Debug, Clone)]
pub struct CachedModelPaths {
    pub primary: CachedModelRecord,
    pub rollback: Option<CachedModelRecord>,
}

pub fn ensure_cached_model(
    model_path: &Path,
    manifest: &VerifiedModelInfo,
) -> Result<Option<CachedModelPaths>> {
    if cache_disabled() {
        return Ok(None);
    }

    enforce_pins(manifest)?;

    let cache_dir = cache_dir()?;
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("Failed to create model cache dir: {}", cache_dir.display()))?;

    let base = format!(
        "{}_{}_{}",
        sanitize(&manifest.model_id),
        sanitize(&manifest.model_version),
        manifest.weights_sha256
    );
    let cached_model = cache_dir.join(format!("{base}.onnx"));
    let cached_manifest = cache_dir.join(format!("{base}.manifest.json"));

    if !cached_model.exists() {
        copy_atomic(model_path, &cached_model)?;
    }
    if !cached_manifest.exists() {
        copy_atomic(&manifest.manifest_path, &cached_manifest)?;
    }

    let record = CachedModelRecord {
        model_id: manifest.model_id.clone(),
        model_version: manifest.model_version.clone(),
        weights_sha256: manifest.weights_sha256.clone(),
        model_path: cached_model,
        manifest_path: cached_manifest,
        activated_at: Utc::now().to_rfc3339(),
    };

    let rollback = update_active_record(&cache_dir, &record)?;

    Ok(Some(CachedModelPaths {
        primary: record,
        rollback,
    }))
}

fn cache_dir() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("TRUESHOT_MODEL_CACHE_DIR") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    Ok(PathBuf::from(".trueshot").join("model_cache"))
}

fn update_active_record(
    cache_dir: &Path,
    record: &CachedModelRecord,
) -> Result<Option<CachedModelRecord>> {
    let active_path = cache_dir.join("active.json");
    let rollback_path = cache_dir.join("rollback.json");

    let previous = read_record(&active_path).unwrap_or(None);
    if let Some(prev) = previous.clone() {
        write_record(&rollback_path, &prev)?;
    }
    write_record(&active_path, record)?;

    let rollback = read_record(&rollback_path).unwrap_or(None);
    Ok(rollback)
}

fn read_record(path: &Path) -> Result<Option<CachedModelRecord>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let record = serde_json::from_slice(&bytes)
        .with_context(|| format!("Invalid cached model record {}", path.display()))?;
    Ok(Some(record))
}

fn write_record(path: &Path, record: &CachedModelRecord) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create cache dir {}", parent.display()))?;
    }
    let payload = serde_json::to_vec_pretty(record)?;
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, payload)
        .with_context(|| format!("Failed to write {}", tmp_path.display()))?;
    fs::rename(&tmp_path, path).with_context(|| format!("Failed to move {}", path.display()))?;
    Ok(())
}

fn copy_atomic(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create cache dir {}", parent.display()))?;
    }
    let tmp_path = dst.with_extension("partial");
    fs::copy(src, &tmp_path)
        .with_context(|| format!("Failed to copy {} to {}", src.display(), tmp_path.display()))?;
    fs::rename(&tmp_path, dst)
        .with_context(|| format!("Failed to move {} to {}", tmp_path.display(), dst.display()))?;
    Ok(())
}

fn enforce_pins(manifest: &VerifiedModelInfo) -> Result<()> {
    if let Ok(pin) = std::env::var("TRUESHOT_MODEL_PIN") {
        let trimmed = pin.trim();
        if !trimmed.is_empty() {
            let (id, version) = parse_pin(trimmed)?;
            if id != manifest.model_id || version != manifest.model_version {
                anyhow::bail!(
                    "Model pin mismatch: expected {}@{}, got {}@{}",
                    id,
                    version,
                    manifest.model_id,
                    manifest.model_version
                );
            }
        }
    }
    if let Ok(hash) = std::env::var("TRUESHOT_MODEL_EXPECTED_HASH") {
        let trimmed = hash.trim();
        if !trimmed.is_empty() && trimmed != manifest.weights_sha256 {
            anyhow::bail!(
                "Model hash pin mismatch: expected {}, got {}",
                trimmed,
                manifest.weights_sha256
            );
        }
    }
    Ok(())
}

fn parse_pin(pin: &str) -> Result<(String, String)> {
    if let Some((id, version)) = pin.split_once('@').or_else(|| pin.split_once(':')) {
        let id = id.trim();
        let version = version.trim();
        if id.is_empty() || version.is_empty() {
            anyhow::bail!("Invalid TRUESHOT_MODEL_PIN format");
        }
        return Ok((id.to_string(), version.to_string()));
    }
    anyhow::bail!("Invalid TRUESHOT_MODEL_PIN format; expected id@version")
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn cache_disabled() -> bool {
    env_flag("TRUESHOT_MODEL_CACHE_DISABLE")
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}
