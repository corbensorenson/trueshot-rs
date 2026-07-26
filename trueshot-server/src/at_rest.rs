use anyhow::{Context, Result};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use rand::RngCore;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use crate::config::PrivacyConfig;
use crate::state::AppState;
use actix_web::web;
use std::time::{Duration, SystemTime};

const MAGIC: &[u8; 4] = b"TSE1";
const VERSION: u8 = 1;
const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;
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
            let bytes = std::fs::read(&wrapped_path)
                .with_context(|| format!("Failed to read wrapped key: {}", wrapped_path.display()))?;
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
        let safe = project_id.replace('/', "_").replace('\\', "_");
        self.root.join(format!("{safe}.key"))
    }

    pub fn wrapped_key_path(&self, project_id: &str) -> PathBuf {
        let safe = project_id.replace('/', "_").replace('\\', "_");
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
    let safe = project_id.replace('/', "_").replace('\\', "_");
    projects_dir
        .join("_security")
        .join("encrypted")
        .join(format!("{safe}.json"))
}

pub fn mark_project_encrypted(projects_dir: &Path, project_id: &str, scopes: &[String]) -> Result<()> {
    let marker = project_marker_path(projects_dir, project_id);
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create encryption marker dir: {}", parent.display()))?;
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
    let default_scopes = vec!["raw".to_string(), "processed".to_string(), "output".to_string()];
    let scopes = load_project_scopes(projects_dir, project_id)
        .or_else(|| config.encrypt_scopes.clone())
        .unwrap_or(default_scopes);
    let enabled = config.encrypt_at_rest.unwrap_or(false) || project_marker_path(projects_dir, project_id).exists();
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
    if let Some(raw) = std::env::var(MASTER_KEY_ENV).ok() {
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
            let encoded = B64.encode(&key);
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
    let should_run = config.encrypt_at_rest.unwrap_or(false) || encrypted_marker_exists(&projects_dir);
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
        }).await?;
        if let Err(err) = result {
            tracing::warn!("encryption sweep failed for {}: {}", name, err);
        }
    }

    Ok(())
}

fn encrypted_marker_exists(projects_dir: &Path) -> bool {
    let marker_dir = projects_dir.join("_security").join("encrypted");
    if let Ok(entries) = std::fs::read_dir(marker_dir) {
        return entries.flatten().any(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("json"));
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
        return Ok(Some(enc_path));
    }
    let _ = encrypt_file(path, &enc_path, key)?;
    let _ = std::fs::remove_file(path);
    Ok(Some(enc_path))
}

pub fn decrypt_file_in_place(
    path: &Path,
    key: &[u8; 32],
) -> Result<PathBuf> {
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
    let mut file = File::open(input)
        .with_context(|| format!("Failed to open file: {}", input.display()))?;

    let mut nonce_prefix = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut nonce_prefix);

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut out = File::create(output)
        .with_context(|| format!("Failed to create file: {}", output.display()))?;

    out.write_all(MAGIC)?;
    out.write_all(&[VERSION])?;
    out.write_all(&(DEFAULT_CHUNK_SIZE as u32).to_le_bytes())?;
    out.write_all(&nonce_prefix)?;

    let mut chunk_index = 0u32;
    let mut bytes_in = 0u64;
    let mut bytes_out = 0u64;
    let mut buf = vec![0u8; DEFAULT_CHUNK_SIZE];
    loop {
        let count = file
            .read(&mut buf)
            .with_context(|| format!("Failed to read file: {}", input.display()))?;
        if count == 0 {
            break;
        }
        bytes_in = bytes_in.saturating_add(count as u64);
        let chunk = &buf[..count];
        let nonce = build_nonce(&nonce_prefix, chunk_index);
        let ciphertext = cipher.encrypt(Nonce::from_slice(&nonce), chunk)
            .with_context(|| format!("Encryption failed for {}", input.display()))?;
        out.write_all(&(ciphertext.len() as u32).to_le_bytes())?;
        out.write_all(&ciphertext)?;
        bytes_out = bytes_out.saturating_add(ciphertext.len() as u64);
        chunk_index = chunk_index.saturating_add(1);
    }

    Ok((bytes_in, bytes_out))
}

fn decrypt_file(input: &Path, output: &Path, key: &[u8; 32]) -> Result<(u64, u64)> {
    let mut file = File::open(input)
        .with_context(|| format!("Failed to open encrypted file: {}", input.display()))?;
    let mut header = [0u8; 4];
    file.read_exact(&mut header)?;
    if &header != MAGIC {
        anyhow::bail!("Invalid encryption header");
    }
    let mut version = [0u8; 1];
    file.read_exact(&mut version)?;
    if version[0] != VERSION {
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
        let plaintext = cipher.decrypt(Nonce::from_slice(&nonce), cipher_buf.as_ref())
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
