use anyhow::{Context, Result};
use chrono::Utc;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static GLOBAL_SIGNER: OnceLock<ProvenanceSigner> = OnceLock::new();
static MODEL_FINGERPRINT: OnceLock<Mutex<Option<ModelFingerprint>>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFingerprint {
    pub model_id: String,
    pub model_version: String,
    pub model_weights_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LicenseTerms {
    pub title: Option<String>,
    pub url: Option<String>,
    pub data_ownership: Option<String>,
    pub export_rights: Option<String>,
    pub updated_at: Option<String>,
    pub trial_active: Option<bool>,
    pub trial_expires_at: Option<String>,
    pub trial_days_remaining: Option<i64>,
}

pub fn set_model_fingerprint(fingerprint: ModelFingerprint) {
    if fingerprint.model_id.trim().is_empty()
        || fingerprint.model_version.trim().is_empty()
        || fingerprint.model_weights_hash.trim().is_empty()
    {
        return;
    }

    let store = MODEL_FINGERPRINT.get_or_init(|| Mutex::new(None));
    let mut guard = store.lock().unwrap();
    match &*guard {
        None => {
            *guard = Some(fingerprint);
        }
        Some(existing) => {
            if existing != &fingerprint {
                tracing::warn!(
                    "Model fingerprint already set (id {}, version {}); new fingerprint ignored",
                    existing.model_id,
                    existing.model_version
                );
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProvenanceSigner {
    signing_key: SigningKey,
}

impl Default for ProvenanceSigner {
    fn default() -> Self {
        Self::new()
    }
}

impl ProvenanceSigner {
    pub fn new() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        Self { signing_key }
    }

    pub fn load_or_generate(path: Option<&Path>) -> Result<Self> {
        if let Some(path) = path {
            if path.exists() {
                enforce_key_permissions(path)?;
                let bytes = std::fs::read(path).with_context(|| {
                    format!("Failed to read provenance key: {}", path.display())
                })?;
                if bytes.len() != 32 {
                    anyhow::bail!("Invalid provenance key length: expected 32 bytes");
                }
                let mut key_bytes = [0u8; 32];
                key_bytes.copy_from_slice(&bytes);
                return Ok(Self {
                    signing_key: SigningKey::from_bytes(&key_bytes),
                });
            }

            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create provenance key dir: {}", parent.display())
                })?;
            }
            let signing_key = SigningKey::generate(&mut OsRng);
            std::fs::write(path, signing_key.to_bytes())
                .with_context(|| format!("Failed to write provenance key: {}", path.display()))?;
            set_key_permissions(path)?;
            return Ok(Self { signing_key });
        }

        Ok(Self::new())
    }

    pub fn global() -> &'static ProvenanceSigner {
        GLOBAL_SIGNER.get_or_init(|| {
            let key_path = std::env::var("TRUESHOT_PROVENANCE_KEY_PATH")
                .ok()
                .map(PathBuf::from);
            if provenance_key_required() && key_path.is_none() {
                panic!("TRUESHOT_PROVENANCE_KEY_PATH must be set in production");
            }
            match Self::load_or_generate(key_path.as_deref()) {
                Ok(signer) => signer,
                Err(err) => {
                    if provenance_key_required() {
                        panic!("Provenance key load failed in production: {}", err);
                    }
                    tracing::warn!("Provenance key load failed, using ephemeral key: {}", err);
                    Self::new()
                }
            }
        })
    }

    pub fn public_key_hex(&self) -> String {
        hex::encode(self.signing_key.verifying_key().as_bytes())
    }

    pub fn key_id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.signing_key.verifying_key().as_bytes());
        let digest = hasher.finalize();
        hex::encode(digest)[0..12].to_string()
    }

    pub fn sign_bytes(&self, payload: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(payload);
        let digest = hasher.finalize();
        let signature = self.signing_key.sign(&digest);
        hex::encode(signature.to_bytes())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceRecord {
    pub asset_path: String,
    pub asset_hash: String,
    pub generated_at: String,
    pub software_version: String,
    pub pipeline_config_hash: Option<String>,
    pub model_id: Option<String>,
    pub model_version: Option<String>,
    pub model_weights_hash: Option<String>,
    pub build_commit: Option<String>,
    pub hardware_fingerprint: Option<String>,
    pub license_title: Option<String>,
    pub license_url: Option<String>,
    pub data_ownership: Option<String>,
    pub export_rights: Option<String>,
    pub license_updated_at: Option<String>,
    pub license_trial_active: Option<bool>,
    pub license_trial_expires_at: Option<String>,
    pub license_trial_days_remaining: Option<i64>,
    pub key_id: String,
    pub device_id: Option<String>,
    pub operator_id: Option<String>,
    pub capture_session_id: Option<String>,
    pub capture_hashes: Option<Vec<String>>,
    pub signer_public_key: String,
    pub signature: String,
    pub signature_payload: String,
    pub redactions: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProvenanceOptions {
    pub device_id: Option<String>,
    pub operator_id: Option<String>,
    pub capture_session_id: Option<String>,
    pub capture_hashes: Option<Vec<String>>,
    pub redact_device_id: bool,
    pub redact_operator_id: bool,
    pub redact_session_id: bool,
    pub redact_capture_hashes: bool,
    pub license_terms: LicenseTerms,
}

impl ProvenanceOptions {
    pub fn from_env() -> Self {
        let device_id = std::env::var("TRUESHOT_DEVICE_ID")
            .ok()
            .or_else(|| hostname::get().ok()?.into_string().ok());
        let operator_id = std::env::var("TRUESHOT_OPERATOR_ID").ok();
        let capture_session_id = std::env::var("TRUESHOT_SESSION_ID").ok();
        Self {
            device_id,
            operator_id,
            capture_session_id,
            capture_hashes: None,
            redact_device_id: env_flag("TRUESHOT_REDACT_DEVICE_ID"),
            redact_operator_id: env_flag("TRUESHOT_REDACT_OPERATOR_ID"),
            redact_session_id: env_flag("TRUESHOT_REDACT_SESSION_ID"),
            redact_capture_hashes: env_flag("TRUESHOT_REDACT_CAPTURE_HASHES"),
            license_terms: LicenseTerms {
                title: std::env::var("TRUESHOT_LICENSE_TITLE").ok(),
                url: std::env::var("TRUESHOT_LICENSE_URL").ok(),
                data_ownership: std::env::var("TRUESHOT_DATA_OWNERSHIP").ok(),
                export_rights: std::env::var("TRUESHOT_EXPORT_RIGHTS").ok(),
                updated_at: std::env::var("TRUESHOT_LICENSE_UPDATED_AT").ok(),
                trial_active: std::env::var("TRUESHOT_LICENSE_TRIAL")
                    .ok()
                    .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES")),
                trial_expires_at: std::env::var("TRUESHOT_LICENSE_TRIAL_EXPIRES_AT").ok(),
                trial_days_remaining: std::env::var("TRUESHOT_LICENSE_TRIAL_DAYS_REMAINING")
                    .ok()
                    .and_then(|value| value.parse::<i64>().ok()),
            },
        }
    }
}

pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("Failed to open asset for hashing: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn verify_signature(public_key_hex: &str, payload: &str, signature_hex: &str) -> Result<bool> {
    let key_bytes = hex::decode(public_key_hex).with_context(|| "Invalid public key hex")?;
    if key_bytes.len() != 32 {
        anyhow::bail!("Invalid public key length: expected 32 bytes");
    }
    let mut key_arr = [0u8; 32];
    key_arr.copy_from_slice(&key_bytes);
    let verifying_key = VerifyingKey::from_bytes(&key_arr)?;

    let sig_bytes = hex::decode(signature_hex).with_context(|| "Invalid signature hex")?;
    if sig_bytes.len() != 64 {
        anyhow::bail!("Invalid signature length: expected 64 bytes");
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let signature = Signature::from_bytes(&sig_arr);

    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    let digest = hasher.finalize();

    Ok(verifying_key.verify(&digest, &signature).is_ok())
}

pub fn write_provenance_sidecar(asset_path: &Path, options: ProvenanceOptions) -> Result<PathBuf> {
    let signer = ProvenanceSigner::global();
    let asset_hash = hash_file(asset_path)?;
    let generated_at = Utc::now().to_rfc3339();
    let software_version = env!("CARGO_PKG_VERSION").to_string();
    let pipeline_config_hash = pipeline_config_hash();
    let (model_id, model_version, model_weights_hash) = model_fingerprint();
    let build_commit = build_commit();
    let hardware_fingerprint = hardware_fingerprint();
    let key_id = signer.key_id();
    let license_terms = resolve_license_terms(asset_path, &options.license_terms);

    let mut redactions = Vec::new();
    let mut device_id = options.device_id;
    if options.redact_device_id {
        device_id = None;
        redactions.push("device_id".to_string());
    }
    let mut operator_id = options.operator_id;
    if options.redact_operator_id {
        operator_id = None;
        redactions.push("operator_id".to_string());
    }
    let mut capture_session_id = options.capture_session_id;
    if options.redact_session_id {
        capture_session_id = None;
        redactions.push("capture_session_id".to_string());
    }
    let mut capture_hashes = options.capture_hashes;
    if options.redact_capture_hashes {
        capture_hashes = None;
        redactions.push("capture_hashes".to_string());
    }

    let asset_path_str = asset_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();

    let signature_payload = build_signature_payload(
        &asset_path_str,
        &asset_hash,
        &generated_at,
        &software_version,
        pipeline_config_hash.as_deref(),
        model_id.as_deref(),
        model_version.as_deref(),
        model_weights_hash.as_deref(),
        build_commit.as_deref(),
        hardware_fingerprint.as_deref(),
        license_terms.title.as_deref(),
        license_terms.url.as_deref(),
        license_terms.data_ownership.as_deref(),
        license_terms.export_rights.as_deref(),
        license_terms.updated_at.as_deref(),
        license_terms.trial_active,
        license_terms.trial_expires_at.as_deref(),
        license_terms.trial_days_remaining,
        &key_id,
        device_id.as_deref(),
        operator_id.as_deref(),
        capture_session_id.as_deref(),
        capture_hashes.as_deref(),
    );

    let signature = signer.sign_bytes(signature_payload.as_bytes());

    let record = ProvenanceRecord {
        asset_path: asset_path_str,
        asset_hash,
        generated_at,
        software_version,
        pipeline_config_hash,
        model_id,
        model_version,
        model_weights_hash,
        build_commit,
        hardware_fingerprint,
        license_title: license_terms.title,
        license_url: license_terms.url,
        data_ownership: license_terms.data_ownership,
        export_rights: license_terms.export_rights,
        license_updated_at: license_terms.updated_at,
        license_trial_active: license_terms.trial_active,
        license_trial_expires_at: license_terms.trial_expires_at,
        license_trial_days_remaining: license_terms.trial_days_remaining,
        key_id,
        device_id,
        operator_id,
        capture_session_id,
        capture_hashes,
        signer_public_key: signer.public_key_hex(),
        signature,
        signature_payload,
        redactions,
    };

    let provenance_path = provenance_sidecar_path(asset_path);
    if let Some(parent) = provenance_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create provenance directory: {}",
                parent.display()
            )
        })?;
    }
    let json = serde_json::to_string_pretty(&record)?;
    std::fs::write(&provenance_path, json)
        .with_context(|| format!("Failed to write provenance: {}", provenance_path.display()))?;

    append_export_audit(asset_path, &record)?;
    Ok(provenance_path)
}

fn provenance_sidecar_path(asset_path: &Path) -> PathBuf {
    let filename = asset_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("asset");
    let provenance_name = format!("{}.provenance.json", filename);
    asset_path.with_file_name(provenance_name)
}

fn build_signature_payload(
    asset_path: &str,
    asset_hash: &str,
    generated_at: &str,
    software_version: &str,
    pipeline_config_hash: Option<&str>,
    model_id: Option<&str>,
    model_version: Option<&str>,
    model_weights_hash: Option<&str>,
    build_commit: Option<&str>,
    hardware_fingerprint: Option<&str>,
    license_title: Option<&str>,
    license_url: Option<&str>,
    data_ownership: Option<&str>,
    export_rights: Option<&str>,
    license_updated_at: Option<&str>,
    license_trial_active: Option<bool>,
    license_trial_expires_at: Option<&str>,
    license_trial_days_remaining: Option<i64>,
    key_id: &str,
    device_id: Option<&str>,
    operator_id: Option<&str>,
    capture_session_id: Option<&str>,
    capture_hashes: Option<&[String]>,
) -> String {
    let capture_hashes = capture_hashes
        .map(|h| h.join(","))
        .unwrap_or_else(|| "none".to_string());
    format!(
        "asset_path={}\nasset_hash={}\ngenerated_at={}\nsoftware_version={}\npipeline_config_hash={}\nmodel_id={}\nmodel_version={}\nmodel_weights_hash={}\nbuild_commit={}\nhardware_fingerprint={}\nlicense_title={}\nlicense_url={}\ndata_ownership={}\nexport_rights={}\nlicense_updated_at={}\nlicense_trial_active={}\nlicense_trial_expires_at={}\nlicense_trial_days_remaining={}\nkey_id={}\ndevice_id={}\noperator_id={}\nsession_id={}\ncapture_hashes={}\n",
        asset_path,
        asset_hash,
        generated_at,
        software_version,
        pipeline_config_hash.unwrap_or("none"),
        model_id.unwrap_or("none"),
        model_version.unwrap_or("none"),
        model_weights_hash.unwrap_or("none"),
        build_commit.unwrap_or("none"),
        hardware_fingerprint.unwrap_or("none"),
        license_title.unwrap_or("none"),
        license_url.unwrap_or("none"),
        data_ownership.unwrap_or("none"),
        export_rights.unwrap_or("none"),
        license_updated_at.unwrap_or("none"),
        license_trial_active.map(|v| v.to_string()).unwrap_or_else(|| "none".to_string()),
        license_trial_expires_at.unwrap_or("none"),
        license_trial_days_remaining.map(|v| v.to_string()).unwrap_or_else(|| "none".to_string()),
        key_id,
        device_id.unwrap_or("none"),
        operator_id.unwrap_or("none"),
        capture_session_id.unwrap_or("none"),
        capture_hashes
    )
}

fn resolve_license_terms(asset_path: &Path, fallback: &LicenseTerms) -> LicenseTerms {
    let mut merged = fallback.clone();
    if let Some(project_terms) = load_license_terms_from_project(asset_path) {
        if merged.title.is_none() {
            merged.title = project_terms.title;
        }
        if merged.url.is_none() {
            merged.url = project_terms.url;
        }
        if merged.data_ownership.is_none() {
            merged.data_ownership = project_terms.data_ownership;
        }
        if merged.export_rights.is_none() {
            merged.export_rights = project_terms.export_rights;
        }
        if merged.updated_at.is_none() {
            merged.updated_at = project_terms.updated_at;
        }
    }
    merged
}

fn load_license_terms_from_project(asset_path: &Path) -> Option<LicenseTerms> {
    let mut current = asset_path.parent();
    for _ in 0..8 {
        let dir = current?;
        let candidate = dir.join("project.json");
        if candidate.exists() {
            let payload = std::fs::read_to_string(candidate).ok()?;
            let value: serde_json::Value = serde_json::from_str(&payload).ok()?;
            let license_value = value.get("license")?;
            return serde_json::from_value::<LicenseTerms>(license_value.clone()).ok();
        }
        current = dir.parent();
    }
    None
}

fn pipeline_config_hash() -> Option<String> {
    if let Ok(value) = std::env::var("TRUESHOT_PIPELINE_CONFIG_HASH") {
        if !value.trim().is_empty() {
            return Some(value);
        }
    }
    let path = std::env::var("TRUESHOT_PIPELINE_CONFIG_PATH").ok()?;
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(hex::encode(hasher.finalize()))
}

fn model_fingerprint() -> (Option<String>, Option<String>, Option<String>) {
    let model_id = std::env::var("TRUESHOT_MODEL_ID")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let model_version = std::env::var("TRUESHOT_MODEL_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let model_weights_hash = std::env::var("TRUESHOT_MODEL_WEIGHTS_HASH")
        .ok()
        .filter(|value| !value.trim().is_empty());

    if model_id.is_some() || model_version.is_some() || model_weights_hash.is_some() {
        return (model_id, model_version, model_weights_hash);
    }

    let store = MODEL_FINGERPRINT.get_or_init(|| Mutex::new(None));
    let guard = store.lock().unwrap();
    if let Some(fingerprint) = guard.as_ref() {
        return (
            Some(fingerprint.model_id.clone()),
            Some(fingerprint.model_version.clone()),
            Some(fingerprint.model_weights_hash.clone()),
        );
    }

    (None, None, None)
}

fn build_commit() -> Option<String> {
    std::env::var("TRUESHOT_GIT_COMMIT")
        .or_else(|_| std::env::var("GIT_COMMIT"))
        .ok()
}

fn hardware_fingerprint() -> Option<String> {
    if let Ok(value) = std::env::var("TRUESHOT_HARDWARE_FINGERPRINT") {
        if !value.trim().is_empty() {
            return Some(value);
        }
    }
    let host = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown-host".to_string());
    let arch = std::env::consts::ARCH.to_string();
    let os = std::env::consts::OS.to_string();
    let cores = std::thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(0);
    let payload = format!("host={}\nos={}\narch={}\ncores={}\n", host, os, arch, cores);
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    Some(hex::encode(hasher.finalize()))
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn provenance_key_required() -> bool {
    let env = std::env::var("TRUESHOT_ENV").unwrap_or_else(|_| "development".to_string());
    env == "production" || env_flag("TRUESHOT_PROVENANCE_REQUIRE_KEY")
}

fn set_key_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)
            .with_context(|| format!("Failed to set key permissions: {}", path.display()))?;
    }
    Ok(())
}

fn enforce_key_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path)?;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            let msg = format!("Provenance key has insecure permissions: {:o}", mode);
            if provenance_key_required() {
                anyhow::bail!(msg);
            } else {
                tracing::warn!("{}", msg);
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportAuditRecord {
    pub timestamp: String,
    pub asset_path: String,
    pub asset_hash: String,
    pub provenance_path: String,
    pub prev_hash: String,
    pub hash: String,
}

pub fn append_export_audit(asset_path: &Path, record: &ProvenanceRecord) -> Result<PathBuf> {
    let audit_path = asset_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("export_audit.log");

    let prev_hash = read_last_audit_hash(&audit_path).unwrap_or_else(|| "genesis".to_string());
    let provenance_path = provenance_sidecar_path(asset_path);
    let mut audit_record = ExportAuditRecord {
        timestamp: record.generated_at.clone(),
        asset_path: record.asset_path.clone(),
        asset_hash: record.asset_hash.clone(),
        provenance_path: provenance_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string(),
        prev_hash,
        hash: String::new(),
    };

    let payload = serde_json::to_string(&audit_record)?;
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    let digest = hasher.finalize();
    audit_record.hash = hex::encode(digest);

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&audit_path)
        .with_context(|| format!("Failed to open export audit log: {}", audit_path.display()))?;
    writeln!(file, "{}", serde_json::to_string(&audit_record)?)?;
    Ok(audit_path)
}

fn read_last_audit_hash(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut last_line = None;
    for line in reader.lines().flatten() {
        if !line.trim().is_empty() {
            last_line = Some(line);
        }
    }
    let last_line = last_line?;
    let record: ExportAuditRecord = serde_json::from_str(&last_line).ok()?;
    Some(record.hash)
}
