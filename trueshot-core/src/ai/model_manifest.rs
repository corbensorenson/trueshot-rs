use anyhow::{Context, Result};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelManifest {
    pub model_id: String,
    pub model_version: String,
    pub weights_sha256: String,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub signed_at: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct VerifiedModelInfo {
    pub model_id: String,
    pub model_version: String,
    pub weights_sha256: String,
    pub manifest_path: PathBuf,
}

pub fn verify_model_manifest(model_path: &Path) -> Result<Option<VerifiedModelInfo>> {
    let manifest_path = manifest_path_for_model(model_path)?;
    if !manifest_path.exists() {
        if require_manifest() {
            anyhow::bail!(
                "Missing model manifest at {} (required in production)",
                manifest_path.display()
            );
        }
        return Ok(None);
    }

    let manifest_bytes = fs::read(&manifest_path)
        .with_context(|| format!("Failed to read model manifest: {}", manifest_path.display()))?;
    let manifest: ModelManifest = serde_json::from_slice(&manifest_bytes)
        .with_context(|| format!("Invalid model manifest JSON: {}", manifest_path.display()))?;

    let weights_hash = compute_sha256(model_path)?;
    if weights_hash != manifest.weights_sha256 {
        anyhow::bail!(
            "Model hash mismatch for {} (manifest {}, actual {})",
            model_path.display(),
            manifest.weights_sha256,
            weights_hash
        );
    }

    let signature_required = require_signature();
    match manifest.signature.as_deref() {
        Some(signature_hex) => {
            let key = load_public_key()?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Model manifest signature present but no public key configured (set TRUESHOT_MODEL_SIGNING_PUBLIC_KEY)"
                )
            })?;
            verify_signature(
                &manifest.model_id,
                &manifest.model_version,
                &manifest.weights_sha256,
                signature_hex,
                &key,
            )?;
        }
        None => {
            if signature_required {
                anyhow::bail!(
                    "Model manifest signature is required but missing at {}",
                    manifest_path.display()
                );
            }
            tracing::warn!(
                "Model manifest for {} is unsigned; set TRUESHOT_MODEL_SIGNING_PUBLIC_KEY to enforce signatures",
                model_path.display()
            );
        }
    }

    Ok(Some(VerifiedModelInfo {
        model_id: manifest.model_id,
        model_version: manifest.model_version,
        weights_sha256: manifest.weights_sha256,
        manifest_path,
    }))
}

fn manifest_path_for_model(model_path: &Path) -> Result<PathBuf> {
    if let Ok(path) = std::env::var("TRUESHOT_MODEL_MANIFEST_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    let mut manifest_path = model_path.to_path_buf();
    manifest_path.set_extension("manifest.json");
    Ok(manifest_path)
}

fn compute_sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("Failed to open model file: {}", path.display()))?;
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

fn verify_signature(
    model_id: &str,
    model_version: &str,
    weights_sha256: &str,
    signature_hex: &str,
    public_key: &[u8; 32],
) -> Result<()> {
    let sig_bytes = hex::decode(signature_hex)
        .with_context(|| "Invalid model signature hex")?;
    if sig_bytes.len() != 64 {
        anyhow::bail!("Invalid model signature length: expected 64 bytes");
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let signature = Signature::from_bytes(&sig_arr);

    let payload = signature_payload(model_id, model_version, weights_sha256);
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    let digest = hasher.finalize();

    let verifying_key = VerifyingKey::from_bytes(public_key)?;
    verifying_key
        .verify(&digest, &signature)
        .map_err(|err| anyhow::anyhow!("Model signature verification failed: {err}"))
}

fn signature_payload(model_id: &str, model_version: &str, weights_sha256: &str) -> String {
    format!(
        "model_id={}\nmodel_version={}\nweights_sha256={}\n",
        model_id, model_version, weights_sha256
    )
}

fn load_public_key() -> Result<Option<[u8; 32]>> {
    if let Ok(value) = std::env::var("TRUESHOT_MODEL_SIGNING_PUBLIC_KEY") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Ok(Some(parse_key_bytes(trimmed.as_bytes())?));
        }
    }
    if let Ok(path) = std::env::var("TRUESHOT_MODEL_SIGNING_KEY_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            let bytes = fs::read(trimmed)
                .with_context(|| format!("Failed to read model signing key: {}", trimmed))?;
            return Ok(Some(parse_key_bytes(&bytes)?));
        }
    }
    Ok(None)
}

fn parse_key_bytes(bytes: &[u8]) -> Result<[u8; 32]> {
    let trimmed = String::from_utf8_lossy(bytes);
    let candidate = trimmed.trim();
    if candidate.len() == 64 && candidate.chars().all(|c| c.is_ascii_hexdigit()) {
        let decoded = hex::decode(candidate)?;
        let mut key = [0u8; 32];
        key.copy_from_slice(&decoded);
        return Ok(key);
    }
    if bytes.len() == 32 {
        let mut key = [0u8; 32];
        key.copy_from_slice(bytes);
        return Ok(key);
    }
    anyhow::bail!("Invalid public key format for model signing key")
}

fn require_manifest() -> bool {
    env_flag("TRUESHOT_MODEL_REQUIRE_MANIFEST") || is_production()
}

fn require_signature() -> bool {
    env_flag("TRUESHOT_MODEL_REQUIRE_SIGNATURE")
        || (is_production() && !env_flag("TRUESHOT_MODEL_ALLOW_UNSIGNED"))
}

fn is_production() -> bool {
    std::env::var("TRUESHOT_ENV")
        .map(|env| env == "production")
        .unwrap_or(false)
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}
