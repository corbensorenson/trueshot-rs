use crate::config::PrivacyConfig;
use crate::state::AppState;
use actix_web::web;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use rand::RngCore;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

const LEGACY_MAGIC: &[u8; 4] = b"TSE1";
const LEGACY_VERSION: u8 = 1;
const LEGACY_MAX_CHUNK_SIZE: usize = 1024 * 1024;
const MASTER_KEY_ENV: &str = "TRUESHOT_MASTER_KEY";
const MASTER_KEYRING_ENTRY: &str = "at_rest_master_key";

#[derive(Clone, Copy)]
pub struct MasterKey {
    key: [u8; 32],
}

impl MasterKey {
    fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() != 32 {
            anyhow::bail!("Invalid master key length: expected 32 bytes");
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        Ok(Self { key })
    }

    fn as_bytes(&self) -> &[u8; 32] {
        &self.key
    }
}

#[derive(Clone)]
pub struct ProjectKeyStore {
    root: PathBuf,
    master_key: MasterKey,
}

impl ProjectKeyStore {
    pub fn new(projects_dir: &Path, master_key: MasterKey) -> Self {
        let root = projects_dir.join("_security").join("keys");
        Self { root, master_key }
    }

    pub fn load_or_create(&self, project_id: &str) -> Result<[u8; 32]> {
        let wrapped_path = self.wrapped_key_path(project_id);
        if wrapped_path.exists() {
            let bytes = std::fs::read(&wrapped_path).with_context(|| {
                format!("Failed to read wrapped key: {}", wrapped_path.display())
            })?;
            let record: WrappedKeyRecord = serde_json::from_slice(&bytes)
                .with_context(|| format!("Invalid wrapped key JSON: {}", wrapped_path.display()))?;
            return unwrap_project_key(&self.master_key, &record);
        }

        let legacy_path = self.legacy_key_path(project_id);
        if legacy_path.exists() {
            let bytes = std::fs::read(&legacy_path)
                .with_context(|| format!("Failed to read legacy key: {}", legacy_path.display()))?;
            if bytes.len() != 32 {
                anyhow::bail!("Invalid encryption key length: expected 32 bytes");
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            self.persist_wrapped_key(project_id, &key)?;
            let _ = std::fs::remove_file(&legacy_path);
            return Ok(key);
        }

        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        self.persist_wrapped_key(project_id, &key)?;
        Ok(key)
    }

    pub fn legacy_key_path(&self, project_id: &str) -> PathBuf {
        let safe = project_id.replace(['/', '\\'], "_");
        self.root.join(format!("{safe}.key"))
    }

    pub fn wrapped_key_path(&self, project_id: &str) -> PathBuf {
        let safe = project_id.replace(['/', '\\'], "_");
        self.root.join(format!("{safe}.json"))
    }

    fn persist_wrapped_key(&self, project_id: &str, key: &[u8; 32]) -> Result<()> {
        let record = wrap_project_key(&self.master_key, key)?;
        let path = self.wrapped_key_path(project_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create key directory: {}", parent.display()))?;
        }
        let payload = serde_json::to_vec_pretty(&record)?;
        std::fs::write(&path, payload)
            .with_context(|| format!("Failed to write wrapped key: {}", path.display()))?;
        set_key_permissions(&path)?;
        Ok(())
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct EncryptionReport {
    pub encrypted_files: usize,
    pub decrypted_files: usize,
    pub skipped_files: usize,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

#[derive(Clone, Debug)]
pub struct EncryptionPolicy {
    pub scopes: Vec<String>,
    pub min_age_seconds: u64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct WrappedKeyRecord {
    version: u8,
    nonce: String,
    wrapped_key: String,
    created_at: String,
}

fn wrap_project_key(master: &MasterKey, project_key: &[u8; 32]) -> Result<WrappedKeyRecord> {
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(master.as_bytes()));
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), project_key.as_ref())
        .with_context(|| "Failed to wrap project key")?;
    Ok(WrappedKeyRecord {
        version: 1,
        nonce: B64.encode(nonce),
        wrapped_key: B64.encode(ciphertext),
        created_at: chrono::Utc::now().to_rfc3339(),
    })
}

fn unwrap_project_key(master: &MasterKey, record: &WrappedKeyRecord) -> Result<[u8; 32]> {
    if record.version != 1 {
        anyhow::bail!("Unsupported wrapped key version: {}", record.version);
    }
    let nonce = B64
        .decode(record.nonce.as_bytes())
        .with_context(|| "Invalid wrapped key nonce")?;
    if nonce.len() != 12 {
        anyhow::bail!("Invalid wrapped key nonce length");
    }
    let wrapped = B64
        .decode(record.wrapped_key.as_bytes())
        .with_context(|| "Invalid wrapped key payload")?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(master.as_bytes()));
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce), wrapped.as_ref())
        .with_context(|| "Failed to unwrap project key")?;
    if plaintext.len() != 32 {
        anyhow::bail!("Invalid project key length after unwrap");
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&plaintext);
    Ok(key)
}

pub fn project_marker_path(projects_dir: &Path, project_id: &str) -> PathBuf {
    let safe = project_id.replace(['/', '\\'], "_");
    projects_dir
        .join("_security")
        .join("encrypted")
        .join(format!("{safe}.json"))
}

pub fn mark_project_encrypted(
    projects_dir: &Path,
    project_id: &str,
    scopes: &[String],
) -> Result<()> {
    let marker = project_marker_path(projects_dir, project_id);
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create encryption marker dir: {}",
                parent.display()
            )
        })?;
    }
    let payload = serde_json::json!({
        "project_id": project_id,
        "scopes": scopes,
        "marked_at": chrono::Utc::now().to_rfc3339(),
    });
    std::fs::write(&marker, serde_json::to_vec_pretty(&payload)?)
        .with_context(|| format!("Failed to write encryption marker: {}", marker.display()))?;
    Ok(())
}

pub fn clear_project_encrypted(projects_dir: &Path, project_id: &str) -> Result<()> {
    let marker = project_marker_path(projects_dir, project_id);
    if marker.exists() {
        std::fs::remove_file(&marker)
            .with_context(|| format!("Failed to remove encryption marker: {}", marker.display()))?;
    }
    Ok(())
}

pub fn load_project_scopes(projects_dir: &Path, project_id: &str) -> Option<Vec<String>> {
    let marker = project_marker_path(projects_dir, project_id);
    let bytes = std::fs::read(&marker).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let scopes = value.get("scopes")?.as_array()?;
    let scopes = scopes
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect::<Vec<_>>();
    if scopes.is_empty() {
        None
    } else {
        Some(scopes)
    }
}

pub fn policy_for_project(
    projects_dir: &Path,
    project_id: &str,
    config: &PrivacyConfig,
) -> Option<EncryptionPolicy> {
    let default_scopes = vec![
        "raw".to_string(),
        "processed".to_string(),
        "output".to_string(),
    ];
    let scopes = load_project_scopes(projects_dir, project_id)
        .or_else(|| config.encrypt_scopes.clone())
        .unwrap_or(default_scopes);
    let enabled = config.encrypt_at_rest.unwrap_or(false)
        || project_marker_path(projects_dir, project_id).exists();
    if !enabled {
        return None;
    }
    Some(EncryptionPolicy {
        scopes,
        min_age_seconds: config.encrypt_min_age_seconds.unwrap_or(60),
    })
}

pub fn require_master_key(config: &PrivacyConfig, projects_dir: &Path) -> Result<MasterKey> {
    match load_master_key(config, projects_dir)? {
        Some(key) => Ok(key),
        None => anyhow::bail!("Encryption master key is required but not configured"),
    }
}

pub fn load_master_key(config: &PrivacyConfig, projects_dir: &Path) -> Result<Option<MasterKey>> {
    let required = encryption_required(config, projects_dir);
    if let Ok(raw) = std::env::var(MASTER_KEY_ENV) {
        let bytes = B64
            .decode(raw.as_bytes())
            .with_context(|| "Invalid base64 master key in TRUESHOT_MASTER_KEY")?;
        return Ok(Some(MasterKey::from_bytes(bytes)?));
    }

    if let Some(path) = config.encryption_master_key_path.as_ref() {
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read master key: {}", path.display()))?;
        if bytes.len() == 32 {
            return Ok(Some(MasterKey::from_bytes(bytes)?));
        }
        let trimmed = String::from_utf8_lossy(&bytes);
        let decoded = B64
            .decode(trimmed.trim().as_bytes())
            .with_context(|| format!("Invalid base64 master key in {}", path.display()))?;
        return Ok(Some(MasterKey::from_bytes(decoded)?));
    }

    if is_production() {
        if required {
            anyhow::bail!("Master key required in production. Set TRUESHOT_MASTER_KEY or privacy.encryption_master_key_path");
        }
        return Ok(None);
    }

    let entry = keyring::Entry::new("trueshot", MASTER_KEYRING_ENTRY)
        .map_err(|e| anyhow::anyhow!("Keyring init failed: {e}"))?;
    match entry.get_password() {
        Ok(encoded) => {
            let bytes = B64
                .decode(encoded.as_bytes())
                .with_context(|| "Invalid master key in keyring")?;
            Ok(Some(MasterKey::from_bytes(bytes)?))
        }
        Err(err) => {
            if !matches!(err, keyring::Error::NoEntry) {
                if required {
                    anyhow::bail!("Keyring error: {err}");
                }
                tracing::warn!("Keyring error while loading master key: {}", err);
                return Ok(None);
            }
            if !required {
                return Ok(None);
            }
            let mut key = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut key);
            let encoded = B64.encode(key);
            entry
                .set_password(&encoded)
                .map_err(|e| anyhow::anyhow!("Failed to store master key in keyring: {e}"))?;
            Ok(Some(MasterKey { key }))
        }
    }
}

pub fn spawn_encryption_task(state: web::Data<AppState>) {
    let config = state.config.privacy.clone();
    let projects_dir = state.config.paths.projects_dir.clone();
    let should_run =
        config.encrypt_at_rest.unwrap_or(false) || encrypted_marker_exists(&projects_dir);
    if !should_run {
        return;
    }

    tokio::spawn(async move {
        let interval = config.encrypt_sweep_interval_seconds.unwrap_or(300);
        loop {
            if let Err(err) = sweep_encryption(&state).await {
                tracing::warn!("encryption sweep failed: {}", err);
            }
            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        }
    });
}

pub fn encrypt_project_scopes(
    project_path: &Path,
    project_id: &str,
    scopes: &[String],
    key_store: &ProjectKeyStore,
) -> Result<EncryptionReport> {
    encrypt_project_scopes_with_age(project_path, project_id, scopes, key_store, 0)
}

pub fn encrypt_project_scopes_with_age(
    project_path: &Path,
    project_id: &str,
    scopes: &[String],
    key_store: &ProjectKeyStore,
    min_age_seconds: u64,
) -> Result<EncryptionReport> {
    let key = key_store.load_or_create(project_id)?;
    let mut report = EncryptionReport::default();
    for scope in scopes {
        let scope_path = project_path.join(scope);
        if !scope_path.exists() {
            continue;
        }
        encrypt_tree(&scope_path, &key, &mut report, min_age_seconds)?;
    }
    Ok(report)
}

pub fn decrypt_project_scopes(
    project_path: &Path,
    project_id: &str,
    scopes: &[String],
    key_store: &ProjectKeyStore,
) -> Result<EncryptionReport> {
    let key = key_store.load_or_create(project_id)?;
    let mut report = EncryptionReport::default();
    for scope in scopes {
        let scope_path = project_path.join(scope);
        if !scope_path.exists() {
            continue;
        }
        decrypt_tree(&scope_path, &key, &mut report)?;
    }
    Ok(report)
}

async fn sweep_encryption(state: &AppState) -> Result<()> {
    let projects_dir = &state.config.paths.projects_dir;
    let entries = std::fs::read_dir(projects_dir)
        .with_context(|| format!("Failed to read projects dir: {}", projects_dir.display()))?;

    let master_key = require_master_key(&state.config.privacy, projects_dir)?;
    let key_store = ProjectKeyStore::new(projects_dir, master_key);
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('_') {
            continue;
        }
        let Some(policy) = policy_for_project(projects_dir, &name, &state.config.privacy) else {
            continue;
        };
        let result = tokio::task::spawn_blocking({
            let path = path.clone();
            let name = name.clone();
            let scopes = policy.scopes.clone();
            let key_store = key_store.clone();
            let min_age = policy.min_age_seconds;
            move || encrypt_project_scopes_with_age(&path, &name, &scopes, &key_store, min_age)
        })
        .await?;
        if let Err(err) = result {
            tracing::warn!("encryption sweep failed for {}: {}", name, err);
        }
    }

    Ok(())
}

fn encrypted_marker_exists(projects_dir: &Path) -> bool {
    let marker_dir = projects_dir.join("_security").join("encrypted");
    if let Ok(entries) = std::fs::read_dir(marker_dir) {
        return entries
            .flatten()
            .any(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("json"));
    }
    false
}

pub fn encryption_required(config: &PrivacyConfig, projects_dir: &Path) -> bool {
    config.encrypt_at_rest.unwrap_or(false) || encrypted_marker_exists(projects_dir)
}

fn is_production() -> bool {
    std::env::var("TRUESHOT_ENV")
        .map(|env| env == "production")
        .unwrap_or(false)
}

pub fn encrypt_file_in_place(
    path: &Path,
    key: &[u8; 32],
    min_age_seconds: u64,
) -> Result<Option<PathBuf>> {
    if path.extension().and_then(|s| s.to_str()) == Some("enc") {
        return Ok(None);
    }
    if !file_is_stable(path, min_age_seconds) {
        return Ok(None);
    }
    let enc_path = PathBuf::from(format!("{}.enc", path.display()));
    if enc_path.exists() {
        if !encrypted_matches_plaintext(path, &enc_path, key)? {
            anyhow::bail!(
                "Existing encrypted destination does not match surviving plaintext {}",
                path.display()
            );
        }
        std::fs::remove_file(path)
            .with_context(|| format!("Remove committed plaintext {}", path.display()))?;
        return Ok(Some(enc_path));
    }
    let _ = encrypt_file(path, &enc_path, key)?;
    let _ = std::fs::remove_file(path);
    Ok(Some(enc_path))
}

pub fn decrypt_file_in_place(path: &Path, key: &[u8; 32]) -> Result<PathBuf> {
    if path.extension().and_then(|s| s.to_str()) != Some("enc") {
        return Ok(path.to_path_buf());
    }
    let original_path = match path.file_name().and_then(|s| s.to_str()) {
        Some(name) if name.ends_with(".enc") => path.with_file_name(name.trim_end_matches(".enc")),
        _ => anyhow::bail!("Invalid encrypted file name: {}", path.display()),
    };
    if original_path.exists() {
        return Ok(original_path);
    }
    let _ = decrypt_file(path, &original_path, key)?;
    Ok(original_path)
}

/// Decrypt a file into memory without materializing plaintext beside the
/// encrypted artifact. Intended for bounded API reads of reports and previews.
pub fn decrypt_file_to_bytes(
    path: &Path,
    key: &[u8; 32],
    max_plaintext_bytes: usize,
) -> Result<Vec<u8>> {
    let mut file = File::open(path)
        .with_context(|| format!("Failed to open encrypted file: {}", path.display()))?;
    let mut header = [0u8; 4];
    file.read_exact(&mut header)?;
    if header == trueshot_storage::encrypted::MAGIC {
        let mut reader = trueshot_storage::encrypted::SeekableEncryptedFile::open(path, key)?;
        let plaintext_len = usize::try_from(reader.plaintext_len())
            .context("Encrypted plaintext length exceeds this platform")?;
        if plaintext_len > max_plaintext_bytes {
            anyhow::bail!(
                "Decrypted file exceeds {} byte read limit",
                max_plaintext_bytes
            );
        }
        let mut output = Vec::with_capacity(plaintext_len);
        reader.read_to_end(&mut output)?;
        return Ok(output);
    }
    if &header != LEGACY_MAGIC {
        anyhow::bail!("Invalid encryption header");
    }
    let mut version = [0u8; 1];
    file.read_exact(&mut version)?;
    if version[0] != LEGACY_VERSION {
        anyhow::bail!("Unsupported encryption version");
    }
    let mut chunk_bytes = [0u8; 4];
    file.read_exact(&mut chunk_bytes)?;
    let chunk_size = u32::from_le_bytes(chunk_bytes) as usize;
    if chunk_size == 0 || chunk_size > LEGACY_MAX_CHUNK_SIZE {
        anyhow::bail!("Invalid encrypted chunk size");
    }
    let mut nonce_prefix = [0u8; 8];
    file.read_exact(&mut nonce_prefix)?;

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let capacity = std::fs::metadata(path)
        .ok()
        .and_then(|meta| usize::try_from(meta.len()).ok())
        .unwrap_or(0)
        .min(max_plaintext_bytes);
    let mut output = Vec::with_capacity(capacity);
    let mut chunk_index = 0u32;
    loop {
        let mut len_buf = [0u8; 4];
        let first = file.read(&mut len_buf[..1])?;
        if first == 0 {
            break;
        }
        file.read_exact(&mut len_buf[1..])?;
        let len = u32::from_le_bytes(len_buf) as usize;
        if len < 16 || len > chunk_size.saturating_add(16) {
            anyhow::bail!("Invalid encrypted chunk length");
        }
        let mut cipher_buf = vec![0u8; len];
        file.read_exact(&mut cipher_buf)?;
        let nonce = build_nonce(&nonce_prefix, chunk_index);
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&nonce), cipher_buf.as_ref())
            .with_context(|| format!("Decryption failed for {}", path.display()))?;
        if output.len().saturating_add(plaintext.len()) > max_plaintext_bytes {
            anyhow::bail!(
                "Decrypted file exceeds {} byte read limit",
                max_plaintext_bytes
            );
        }
        output.extend_from_slice(&plaintext);
        chunk_index = chunk_index
            .checked_add(1)
            .context("Encrypted file contains too many chunks")?;
    }
    Ok(output)
}

/// Atomically publish encrypted bytes without ever writing a plaintext sibling.
/// A same-directory hard link provides create-if-absent publication, so a
/// concurrent writer cannot replace an immutable target.
pub fn write_encrypted_bytes_atomic(path: &Path, key: &[u8; 32], bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("Encrypted target has no parent directory")?;
    let parent_metadata = std::fs::symlink_metadata(parent)
        .with_context(|| format!("Inspect encrypted target directory {}", parent.display()))?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        anyhow::bail!("Encrypted target parent is not a real directory");
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("Encrypted target filename is not UTF-8")?;
    let temporary = parent.join(format!(
        ".{file_name}.{}.part",
        uuid::Uuid::new_v4().as_simple()
    ));
    let result = (|| -> Result<()> {
        trueshot_storage::encrypted::encrypt_bytes(
            &temporary,
            key,
            bytes,
            trueshot_storage::encrypted::DEFAULT_CHUNK_SIZE,
        )?;
        std::fs::hard_link(&temporary, path)
            .with_context(|| format!("Publish encrypted artifact {}", path.display()))?;
        std::fs::remove_file(&temporary)
            .with_context(|| format!("Remove encrypted temporary {}", temporary.display()))?;
        sync_parent_directory(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<()> {
    File::open(path.parent().unwrap_or_else(|| Path::new(".")))?
        .sync_all()
        .context("Sync encrypted artifact directory")
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<()> {
    Ok(())
}

fn encrypt_tree(
    root: &Path,
    key: &[u8; 32],
    report: &mut EncryptionReport,
    min_age_seconds: u64,
) -> Result<()> {
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("enc") {
            report.skipped_files += 1;
            continue;
        }
        let enc_path = PathBuf::from(format!("{}.enc", path.display()));
        if enc_path.exists() {
            if !encrypted_matches_plaintext(path, &enc_path, key)? {
                anyhow::bail!(
                    "Existing encrypted destination does not match surviving plaintext {}",
                    path.display()
                );
            }
            std::fs::remove_file(path)?;
            report.skipped_files += 1;
            continue;
        }
        if !file_is_stable(path, min_age_seconds) {
            report.skipped_files += 1;
            continue;
        }
        let (bytes_in, bytes_out) = encrypt_file(path, &enc_path, key)?;
        report.encrypted_files += 1;
        report.bytes_in = report.bytes_in.saturating_add(bytes_in);
        report.bytes_out = report.bytes_out.saturating_add(bytes_out);
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

fn decrypt_tree(root: &Path, key: &[u8; 32], report: &mut EncryptionReport) -> Result<()> {
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("enc") {
            continue;
        }
        let original_path = match path.file_name().and_then(|s| s.to_str()) {
            Some(name) if name.ends_with(".enc") => {
                let trimmed = name.trim_end_matches(".enc");
                path.with_file_name(trimmed)
            }
            _ => {
                report.skipped_files += 1;
                continue;
            }
        };
        if original_path.exists() {
            report.skipped_files += 1;
            continue;
        }
        let (bytes_in, bytes_out) = decrypt_file(path, &original_path, key)?;
        report.decrypted_files += 1;
        report.bytes_in = report.bytes_in.saturating_add(bytes_in);
        report.bytes_out = report.bytes_out.saturating_add(bytes_out);
    }
    Ok(())
}

fn encrypt_file(input: &Path, output: &Path, key: &[u8; 32]) -> Result<(u64, u64)> {
    let parent = output
        .parent()
        .context("Encrypted output has no parent directory")?;
    let name = output
        .file_name()
        .and_then(|value| value.to_str())
        .context("Encrypted output filename is not UTF-8")?;
    let temporary = parent.join(format!(".{name}.{}.part", uuid::Uuid::new_v4().as_simple()));
    let result = (|| -> Result<(u64, u64)> {
        let stats = trueshot_storage::encrypted::encrypt_file(
            input,
            &temporary,
            key,
            trueshot_storage::encrypted::DEFAULT_CHUNK_SIZE,
        )?;
        std::fs::hard_link(&temporary, output)
            .with_context(|| format!("Publish encrypted asset {}", output.display()))?;
        std::fs::remove_file(&temporary)?;
        sync_parent_directory(output)?;
        Ok((stats.plaintext_bytes, stats.encrypted_bytes))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn encrypted_matches_plaintext(plaintext: &Path, encrypted: &Path, key: &[u8; 32]) -> Result<bool> {
    let mut clear = File::open(plaintext)?;
    let mut protected = trueshot_storage::encrypted::SeekableEncryptedFile::open(encrypted, key)
        .with_context(|| {
            format!(
                "Existing encrypted destination {} is incomplete or unauthenticated",
                encrypted.display()
            )
        })?;
    if protected.plaintext_len() != clear.metadata()?.len() {
        return Ok(false);
    }
    let mut clear_chunk = vec![0u8; LEGACY_MAX_CHUNK_SIZE];
    let mut protected_chunk = vec![0u8; LEGACY_MAX_CHUNK_SIZE];
    loop {
        let count = clear.read(&mut clear_chunk)?;
        if count == 0 {
            return Ok(true);
        }
        protected.read_exact(&mut protected_chunk[..count])?;
        if clear_chunk[..count] != protected_chunk[..count] {
            return Ok(false);
        }
    }
}

fn decrypt_file(input: &Path, output: &Path, key: &[u8; 32]) -> Result<(u64, u64)> {
    let mut file = File::open(input)
        .with_context(|| format!("Failed to open encrypted file: {}", input.display()))?;
    let mut header = [0u8; 4];
    file.read_exact(&mut header)?;
    if header == trueshot_storage::encrypted::MAGIC {
        let stats = trueshot_storage::encrypted::decrypt_file(input, output, key)?;
        return Ok((stats.encrypted_bytes, stats.plaintext_bytes));
    }
    if &header != LEGACY_MAGIC {
        anyhow::bail!("Invalid encryption header");
    }
    let mut version = [0u8; 1];
    file.read_exact(&mut version)?;
    if version[0] != LEGACY_VERSION {
        anyhow::bail!("Unsupported encryption version");
    }
    let mut chunk_bytes = [0u8; 4];
    file.read_exact(&mut chunk_bytes)?;
    let _chunk_size = u32::from_le_bytes(chunk_bytes) as usize;
    let mut nonce_prefix = [0u8; 8];
    file.read_exact(&mut nonce_prefix)?;

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut out = File::create(output)
        .with_context(|| format!("Failed to create output file: {}", output.display()))?;

    let mut chunk_index = 0u32;
    let mut bytes_in = 0u64;
    let mut bytes_out = 0u64;
    loop {
        let mut len_buf = [0u8; 4];
        match file.read_exact(&mut len_buf) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(err.into()),
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 {
            break;
        }
        let mut cipher_buf = vec![0u8; len];
        file.read_exact(&mut cipher_buf)?;
        bytes_in = bytes_in.saturating_add(len as u64);
        let nonce = build_nonce(&nonce_prefix, chunk_index);
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&nonce), cipher_buf.as_ref())
            .with_context(|| format!("Decryption failed for {}", input.display()))?;
        out.write_all(&plaintext)?;
        bytes_out = bytes_out.saturating_add(plaintext.len() as u64);
        chunk_index = chunk_index.saturating_add(1);
    }

    Ok((bytes_in, bytes_out))
}

fn build_nonce(prefix: &[u8; 8], counter: u32) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..8].copy_from_slice(prefix);
    nonce[8..].copy_from_slice(&counter.to_be_bytes());
    nonce
}

fn file_is_stable(path: &Path, min_age_seconds: u64) -> bool {
    if min_age_seconds == 0 {
        return true;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return false;
    };
    age >= Duration::from_secs(min_age_seconds)
}

#[cfg(unix)]
fn set_key_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_key_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_legacy_tse1(path: &Path, key: &[u8; 32], payload: &[u8]) {
        let nonce_prefix = [0x19u8; 8];
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
        let mut output = File::create(path).unwrap();
        output.write_all(LEGACY_MAGIC).unwrap();
        output.write_all(&[LEGACY_VERSION]).unwrap();
        output
            .write_all(&(LEGACY_MAX_CHUNK_SIZE as u32).to_le_bytes())
            .unwrap();
        output.write_all(&nonce_prefix).unwrap();
        for (index, chunk) in payload.chunks(LEGACY_MAX_CHUNK_SIZE).enumerate() {
            let ciphertext = cipher
                .encrypt(
                    Nonce::from_slice(&build_nonce(&nonce_prefix, index as u32)),
                    chunk,
                )
                .unwrap();
            output
                .write_all(&(ciphertext.len() as u32).to_le_bytes())
                .unwrap();
            output.write_all(&ciphertext).unwrap();
        }
    }

    #[test]
    fn bounded_decryption_does_not_materialize_plaintext() {
        let directory = tempfile::tempdir().unwrap();
        let plaintext_path = directory.path().join("report.json");
        let encrypted_path = directory.path().join("report.json.enc");
        let payload = br#"{"schema":"trueshot.fusion.provenance.v2"}"#;
        let key = [0x5au8; 32];
        std::fs::write(&plaintext_path, payload).unwrap();
        encrypt_file(&plaintext_path, &encrypted_path, &key).unwrap();
        std::fs::remove_file(&plaintext_path).unwrap();

        let decoded = decrypt_file_to_bytes(&encrypted_path, &key, payload.len()).unwrap();

        assert_eq!(decoded, payload);
        assert!(!plaintext_path.exists());
        assert!(encrypted_path.exists());
    }

    #[test]
    fn bounded_decryption_rejects_oversized_plaintext() {
        let directory = tempfile::tempdir().unwrap();
        let plaintext_path = directory.path().join("map.png");
        let encrypted_path = directory.path().join("map.png.enc");
        let key = [0x33u8; 32];
        std::fs::write(&plaintext_path, [7u8; 32]).unwrap();
        encrypt_file(&plaintext_path, &encrypted_path, &key).unwrap();

        let error = decrypt_file_to_bytes(&encrypted_path, &key, 31).unwrap_err();

        assert!(error.to_string().contains("exceeds 31 byte read limit"));
    }

    #[test]
    fn encrypted_atomic_write_never_materializes_plaintext() {
        let directory = tempfile::tempdir().unwrap();
        let encrypted_path = directory.path().join("revision.json.enc");
        let plaintext_path = directory.path().join("revision.json");
        let payload = br#"{"schema":"trueshot.fusion.edits.v1"}"#;
        let key = [0x71u8; 32];

        write_encrypted_bytes_atomic(&encrypted_path, &key, payload).unwrap();

        assert!(!plaintext_path.exists());
        assert_eq!(
            decrypt_file_to_bytes(&encrypted_path, &key, payload.len()).unwrap(),
            payload
        );
        assert!(write_encrypted_bytes_atomic(&encrypted_path, &key, payload).is_err());
    }

    #[test]
    fn concurrent_encrypted_publication_never_replaces_winner() {
        let directory = tempfile::tempdir().unwrap();
        let encrypted_path = directory.path().join("revision.json.enc");
        let key = [0x42u8; 32];
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut workers = Vec::new();
        for payload in [b"first".as_slice(), b"second".as_slice()] {
            let path = encrypted_path.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                write_encrypted_bytes_atomic(&path, &key, payload)
            }));
        }
        barrier.wait();
        let successes = workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap().is_ok()))
            .sum::<usize>();
        let decoded = decrypt_file_to_bytes(&encrypted_path, &key, 16).unwrap();

        assert_eq!(successes, 1);
        assert!(decoded == b"first" || decoded == b"second");
    }

    #[test]
    fn legacy_tse1_remains_bounded_read_compatible() {
        let directory = tempfile::tempdir().unwrap();
        let encrypted = directory.path().join("legacy.json.enc");
        let payload = br#"{"legacy":true}"#;
        let key = [0x28u8; 32];
        write_legacy_tse1(&encrypted, &key, payload);

        assert_eq!(
            decrypt_file_to_bytes(&encrypted, &key, payload.len()).unwrap(),
            payload
        );
        assert!(
            trueshot_storage::encrypted::SeekableEncryptedFile::open(&encrypted, &key).is_err(),
            "legacy files must not be misrepresented as authenticated random-access TSE2"
        );
    }

    #[test]
    fn interrupted_in_place_commit_is_authenticated_before_plaintext_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let plaintext = directory.path().join("capture.NEF");
        let encrypted = directory.path().join("capture.NEF.enc");
        let payload = vec![0x44u8; 70_000];
        let key = [0x62u8; 32];
        std::fs::write(&plaintext, &payload).unwrap();
        trueshot_storage::encrypted::encrypt_file(&plaintext, &encrypted, &key, 64 * 1024).unwrap();

        encrypt_file_in_place(&plaintext, &key, 0).unwrap();
        assert!(!plaintext.exists());

        let mut conflicting = payload.clone();
        conflicting[0] ^= 0xff;
        std::fs::write(&plaintext, &conflicting).unwrap();
        assert!(encrypt_file_in_place(&plaintext, &key, 0).is_err());
        assert!(
            plaintext.exists(),
            "same-length but different ciphertext must not authorize plaintext removal"
        );

        std::fs::write(&plaintext, &payload).unwrap();
        let mut damaged = std::fs::read(&encrypted).unwrap();
        damaged.pop();
        std::fs::write(&encrypted, damaged).unwrap();
        assert!(encrypt_file_in_place(&plaintext, &key, 0).is_err());
        assert!(
            plaintext.exists(),
            "unauthenticated destination must never authorize plaintext removal"
        );
    }
}
