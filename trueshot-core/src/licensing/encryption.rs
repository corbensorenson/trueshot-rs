//! License Encryption Module
//!
//! Provides RSA-based license verification for commercial distribution.
//! Licenses are signed by the vendor's private key and verified at runtime.

use anyhow::{anyhow, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use rsa::{pkcs8::DecodePublicKey, Pkcs1v15Sign, RsaPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

/// License types
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum LicenseType {
    /// Free tier with limited features
    Hobby,
    /// Professional tier ($X/month)
    Professional,
    /// Enterprise tier with all features
    Enterprise,
    /// Development/testing license
    Development,
}

/// License data structure (encoded in license file)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LicenseData {
    /// License ID (UUID)
    pub id: String,
    /// License holder email
    pub email: String,
    /// License type
    pub license_type: LicenseType,
    /// Issue timestamp (Unix seconds)
    pub issued_at: u64,
    /// Expiry timestamp (Unix seconds, 0 = perpetual)
    pub expires_at: u64,
    /// Maximum scans per month (0 = unlimited)
    pub max_scans_per_month: u32,
    /// Machine fingerprint (optional binding)
    pub machine_id: Option<String>,
    /// Features enabled
    pub features: Vec<String>,
}

/// Complete license with signature
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedLicense {
    /// License data (JSON)
    pub data: LicenseData,
    /// Base64-encoded RSA signature of SHA256(data)
    pub signature: String,
}

/// Embedded public key for license verification
/// In production, this would be the vendor's RSA public key
const EMBEDDED_PUBLIC_KEY: &str = r#"
-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA0Z3VS5JJcds3xfn/ygWs
TRVf7Y9xMS5GdDXJ07aRYNP3kOQZl0wHkPJkAk3E0F8xQrL8kZWA8hXlMJODQvNS
7bN6xJQGhZp92rOKfX8CYaXn3EWjkqnQxyz/xLxWvPGqrtN/4xwK3eZ0HvAzXByl
QhFTPzZ1lGSvXxPfYE8vD6zk0ELHvYPhZhN3p2x+pO8LuPl8KQbZ3CfH8j5XFGJD
placeholder_for_actual_public_key_bytes_here_in_production
zQIDAQAB
-----END PUBLIC KEY-----
"#;

fn placeholder_key_allowed() -> bool {
    matches!(
        std::env::var("TRUESHOT_ALLOW_PLACEHOLDER_LICENSE_KEY").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

/// License verifier
pub struct LicenseVerifier {
    /// Cached license (after successful verification)
    cached_license: Option<LicenseData>,
    /// Last verification timestamp
    last_verified: u64,
}

impl LicenseVerifier {
    pub fn new() -> Self {
        Self {
            cached_license: None,
            last_verified: 0,
        }
    }

    /// Verify a signed license
    pub fn verify(&mut self, signed_license: &SignedLicense) -> Result<&LicenseData> {
        let pem = load_public_key_pem()?;
        if is_placeholder_key(&pem) && !cfg!(test) && !placeholder_key_allowed() {
            return Err(anyhow!(
                "Placeholder RSA public key is not allowed in production builds"
            ));
        }

        // 1. Serialize license data for signature verification
        let data_json = serde_json::to_string(&signed_license.data)?;
        let data_hash = self.compute_hash(data_json.as_bytes());

        // 2. Verify RSA signature
        if !self.verify_signature(&data_hash, &signed_license.signature) {
            tracing::warn!("License signature verification failed");
            return Err(anyhow!("Invalid license signature"));
        }

        // 3. Check expiry
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if signed_license.data.expires_at > 0 && now > signed_license.data.expires_at {
            tracing::warn!("License expired");
            return Err(anyhow!("License expired"));
        }

        // 4. Check machine binding (if specified)
        if let Some(ref bound_machine) = signed_license.data.machine_id {
            let current_machine = self.get_machine_fingerprint();
            if bound_machine != &current_machine {
                tracing::warn!("License bound to different machine");
                return Err(anyhow!("License not valid for this machine"));
            }
        }

        // 5. Cache verified license
        self.cached_license = Some(signed_license.data.clone());
        self.last_verified = now;
        tracing::info!(
            license_type = ?signed_license.data.license_type,
            expires_at = signed_license.data.expires_at,
            "License verified"
        );

        Ok(self.cached_license.as_ref().unwrap())
    }

    /// Load and verify license from file
    pub fn load_license(&mut self, path: &std::path::Path) -> Result<&LicenseData> {
        let license_json = std::fs::read_to_string(path)?;
        let signed_license: SignedLicense = serde_json::from_str(&license_json)?;
        self.verify(&signed_license)
    }

    /// Get cached license (returns None if not verified)
    pub fn get_license(&self) -> Option<&LicenseData> {
        self.cached_license.as_ref()
    }

    /// Check if a feature is enabled
    pub fn has_feature(&self, feature: &str) -> bool {
        self.cached_license
            .as_ref()
            .map(|l| l.features.contains(&feature.to_string()))
            .unwrap_or(false)
    }

    /// Check if license is Enterprise tier
    pub fn is_enterprise(&self) -> bool {
        self.cached_license
            .as_ref()
            .map(|l| l.license_type == LicenseType::Enterprise)
            .unwrap_or(false)
    }

    /// Compute SHA256 hash
    fn compute_hash(&self, data: &[u8]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.finalize().into()
    }

    /// Verify RSA signature (simplified - uses embedded public key)
    fn verify_signature(&self, hash: &[u8; 32], signature_b64: &str) -> bool {
        // Decode base64 signature
        let signature_bytes = match STANDARD.decode(signature_b64) {
            Ok(bytes) => bytes,
            Err(_) => return false,
        };

        let public_key_pem = match load_public_key_pem() {
            Ok(key) => key,
            Err(err) => {
                tracing::warn!("Failed to load RSA public key: {}", err);
                return false;
            }
        };

        if is_placeholder_key(&public_key_pem) {
            tracing::warn!("Placeholder RSA public key detected");
            return false;
        }

        let public_key = match RsaPublicKey::from_public_key_pem(&public_key_pem) {
            Ok(key) => key,
            Err(_) => return false,
        };

        let scheme = Pkcs1v15Sign::new::<Sha256>();
        public_key.verify(scheme, hash, &signature_bytes).is_ok()
    }

    /// Generate machine fingerprint for license binding
    fn get_machine_fingerprint(&self) -> String {
        let mut components = Vec::new();

        // CPU info
        #[cfg(target_os = "linux")]
        {
            if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
                for line in cpuinfo.lines() {
                    if line.starts_with("model name") {
                        components.push(line.to_string());
                        break;
                    }
                }
            }
        }

        // MAC address (first interface)
        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = std::process::Command::new("ifconfig").arg("en0").output() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if line.contains("ether") {
                        components.push(line.trim().to_string());
                        break;
                    }
                }
            }
        }

        // Hostname
        if let Ok(hostname) = std::env::var("HOSTNAME") {
            components.push(hostname);
        }

        // Hash all components
        let combined = components.join("|");
        let hash = self.compute_hash(combined.as_bytes());
        hex::encode(&hash[..16]) // Use first 16 bytes
    }
}

fn load_public_key_pem() -> Result<String> {
    if let Ok(path) = std::env::var("TRUESHOT_LICENSE_RSA_PUBLIC_KEY_PATH") {
        return std::fs::read_to_string(&path).map_err(|e| anyhow!("Failed to read RSA key: {e}"));
    }

    if let Ok(pem) = std::env::var("TRUESHOT_LICENSE_RSA_PUBLIC_KEY_PEM") {
        return Ok(pem);
    }

    Ok(EMBEDDED_PUBLIC_KEY.to_string())
}

fn is_placeholder_key(pem: &str) -> bool {
    pem.contains("placeholder")
}

impl Default for LicenseVerifier {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate a development license (for testing)
#[cfg(any(test, feature = "dev_license"))]
pub fn generate_dev_license() -> SignedLicense {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    SignedLicense {
        data: LicenseData {
            id: uuid::Uuid::new_v4().to_string(),
            email: "dev@trueshot.local".to_string(),
            license_type: LicenseType::Development,
            issued_at: now,
            expires_at: now + 365 * 24 * 3600, // 1 year
            max_scans_per_month: 0,            // Unlimited
            machine_id: None,
            features: vec![
                "3dgs".to_string(),
                "4dgs".to_string(),
                "photogrammetry".to_string(),
                "ai_assist".to_string(),
                "export_usdz".to_string(),
                "export_gltf".to_string(),
            ],
        },
        signature: STANDARD.encode("dev_signature_placeholder"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dev_license_generation() {
        let license = generate_dev_license();
        assert_eq!(license.data.license_type, LicenseType::Development);
        assert!(license.data.features.contains(&"3dgs".to_string()));
    }

    #[test]
    fn test_dev_license_verification() {
        let mut verifier = LicenseVerifier::new();
        let license = generate_dev_license();

        assert!(verifier.verify(&license).is_err());
    }
}
