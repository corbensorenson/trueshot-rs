//! License Manager
//!
//! Handles license loading, verification, activation, and feature checking.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chrono::DateTime;
use chrono::Utc;
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::{env, fs};
use tracing::{info, warn};

use super::{
    integrity::{IntegrityChecker, IntegrityStatus},
    license::LicensePayload,
    ActivatedDevice, DeviceFingerprint, Feature, License, LicenseError, LicenseFeatures,
    LicenseTier,
};

/// Embedded public key for license verification
///
/// This key is used to verify license signatures offline.
/// The corresponding private key is kept secure on the license server.
///
/// NOTE: Generate a real key pair for production using:
/// ```ignore
/// use ed25519_dalek::SigningKey;
/// let mut csprng = rand::rngs::OsRng;
/// let signing_key = SigningKey::generate(&mut csprng);
/// let verifying_key = signing_key.verifying_key();
/// ```
const PUBLIC_KEY_BYTES: [u8; 32] = [
    // Embed a production public key for offline verification.
    // In release builds, missing keys are rejected; use env overrides for staging.
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// License manager handles all licensing operations
#[derive(Debug)]
pub struct LicenseManager {
    /// Ed25519 public key for signature verification
    public_key: Option<VerifyingKey>,
    /// Currently loaded license
    current_license: Option<License>,
    /// This device's fingerprint
    device: DeviceFingerprint,
    /// Path to cached license file
    cache_path: PathBuf,
    /// Whether we're in development/trial mode
    dev_mode: bool,
}

#[derive(Debug, Clone)]
pub struct TrialInfo {
    pub active: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub days_remaining: Option<i64>,
}

#[derive(Debug, Serialize)]
struct ActivationRequest {
    license_key: String,
    device_hash: String,
    device_name: String,
}

#[derive(Debug, Deserialize)]
struct ActivationResponse {
    license_json: String,
    device_name: Option<String>,
}

impl LicenseManager {
    /// Create a new license manager
    pub fn new() -> Result<Self, LicenseError> {
        let public_key = load_public_key_override()?.or_else(|| {
            if PUBLIC_KEY_BYTES == [0u8; 32] {
                None
            } else {
                VerifyingKey::from_bytes(&PUBLIC_KEY_BYTES).ok()
            }
        });

        let dev_mode = dev_mode_enabled();
        if dev_mode {
            warn!("License dev mode enabled via TRUESHOT_LICENSE_DEV_MODE");
        }
        if public_key.is_none() && !dev_mode {
            return Err(LicenseError::MissingPublicKey);
        }

        let device = DeviceFingerprint::generate();
        let cache_path = Self::default_cache_path();

        let mut manager = Self {
            public_key,
            current_license: None,
            device,
            cache_path,
            dev_mode,
        };

        // Try to load cached license
        let _ = manager.load_cached_license();

        Ok(manager)
    }

    /// Create license manager with custom cache path
    pub fn with_cache_path(cache_path: PathBuf) -> Result<Self, LicenseError> {
        let mut manager = Self::new()?;
        manager.cache_path = cache_path;
        let _ = manager.load_cached_license();
        Ok(manager)
    }

    /// Get default cache path for license file
    fn default_cache_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("TrueShot")
            .join("license.json")
    }

    /// Load license from cache
    fn load_cached_license(&mut self) -> Result<(), LicenseError> {
        if !self.cache_path.exists() {
            return Err(LicenseError::FileNotFound(
                self.cache_path.display().to_string(),
            ));
        }

        let content = std::fs::read_to_string(&self.cache_path)?;
        let license: License = serde_json::from_str(&content)
            .map_err(|e| LicenseError::SerializationError(e.to_string()))?;

        // Verify if we have a public key
        if let Some(ref pk) = self.public_key {
            license.verify_signature(pk)?;
        }

        self.current_license = Some(license);
        Ok(())
    }

    /// Save license to cache
    fn save_license_cache(&self, license: &License) -> Result<(), LicenseError> {
        // Ensure parent directory exists
        if let Some(parent) = self.cache_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(license)
            .map_err(|e| LicenseError::SerializationError(e.to_string()))?;

        std::fs::write(&self.cache_path, content)?;
        Ok(())
    }

    /// Verify the current license
    pub fn verify(&self) -> Result<(), LicenseError> {
        let integrity_status = IntegrityChecker::new().verify();
        if !matches!(integrity_status, IntegrityStatus::Valid) {
            warn!(?integrity_status, "License integrity check failed");
            return Err(LicenseError::IntegrityFailure(format!(
                "{integrity_status:?}"
            )));
        }

        // Development mode - always valid
        if self.dev_mode {
            info!("License verification bypassed in explicit dev mode");
            return Ok(());
        }

        let license = self
            .current_license
            .as_ref()
            .ok_or(LicenseError::NoLicense)?;

        // Check signature
        if let Some(ref pk) = self.public_key {
            license.verify_signature(pk)?;
        } else {
            return Err(LicenseError::MissingPublicKey);
        }

        // Check expiration
        if license.is_expired() {
            return Err(LicenseError::Expired);
        }

        // Check device activation
        let my_hash = self.device.fingerprint_hash();
        if !license.is_device_activated(&my_hash) {
            return Err(LicenseError::DeviceNotActivated);
        }

        // Check grace period for offline operation
        if !license.is_within_grace_period(&my_hash) {
            return Err(LicenseError::GracePeriodExpired);
        }

        info!(
            tier = ?license.payload.tier,
            expires_at = ?license.payload.expires_at,
            "License verified"
        );

        Ok(())
    }

    /// Check if a feature is enabled
    pub fn is_feature_enabled(&self, feature: Feature) -> bool {
        // Development mode - all features enabled
        if self.dev_mode {
            return true;
        }

        self.current_license
            .as_ref()
            .map(|l| l.is_feature_enabled(feature))
            .unwrap_or(false)
    }

    /// Require a feature, returning error if not available
    pub fn require_feature(&self, feature: Feature) -> Result<(), LicenseError> {
        if self.is_feature_enabled(feature) {
            Ok(())
        } else {
            Err(LicenseError::FeatureNotAvailable(format!("{:?}", feature)))
        }
    }

    /// Get current license tier
    pub fn tier(&self) -> Option<&LicenseTier> {
        self.current_license.as_ref().map(|l| &l.payload.tier)
    }

    /// Get maximum allowed output resolution (pixels) if licensed
    pub fn max_resolution(&self) -> Option<u32> {
        if self.dev_mode {
            return None;
        }
        self.current_license
            .as_ref()
            .map(|l| l.payload.features.max_resolution)
    }

    /// Get monthly scan limit if enforced (None = unlimited)
    pub fn scans_per_month(&self) -> Option<u32> {
        if self.dev_mode {
            return None;
        }
        self.current_license
            .as_ref()
            .and_then(|l| l.payload.features.scans_per_month)
    }

    /// Get a stable license key hash for usage tracking
    pub fn license_key_hash(&self) -> Option<String> {
        if self.dev_mode {
            return None;
        }
        let key = self
            .current_license
            .as_ref()?
            .payload
            .license_key
            .as_bytes();
        let mut hasher = Sha256::new();
        hasher.update(key);
        let digest = hasher.finalize();
        Some(hex::encode(digest))
    }

    /// Get trial metadata if the active license is a trial.
    pub fn trial_info(&self) -> Option<TrialInfo> {
        if self.dev_mode {
            return None;
        }
        let license = self.current_license.as_ref()?;
        if !Self::is_trial_license(license) {
            return None;
        }
        let expires_at = license.payload.expires_at;
        let days_remaining = expires_at.map(|expires| {
            let delta = expires.signed_duration_since(Utc::now()).num_days();
            delta.max(0)
        });
        Some(TrialInfo {
            active: true,
            expires_at,
            days_remaining,
        })
    }

    /// Get license status summary
    pub fn status(&self) -> LicenseStatus {
        if self.dev_mode {
            return LicenseStatus::Development;
        }

        match &self.current_license {
            None => LicenseStatus::Unlicensed,
            Some(license) => {
                if license.is_expired() {
                    LicenseStatus::Expired
                } else {
                    let my_hash = self.device.fingerprint_hash();
                    if !license.is_device_activated(&my_hash) {
                        LicenseStatus::NotActivated
                    } else if !license.is_within_grace_period(&my_hash) {
                        LicenseStatus::GracePeriodExpired
                    } else {
                        LicenseStatus::Valid {
                            tier: license.payload.tier.clone(),
                            expires: license.payload.expires_at,
                        }
                    }
                }
            }
        }
    }

    /// Load license from a key (requires network for activation)
    pub fn load_license_key(
        &mut self,
        license_key: &str,
        device_name_override: Option<String>,
    ) -> Result<(), LicenseError> {
        // Validate key format (XXXX-XXXX-XXXX-XXXX)
        if !Self::is_valid_key_format(license_key) {
            return Err(LicenseError::InvalidKeyFormat);
        }

        let activation_url = env::var("TRUESHOT_LICENSE_ACTIVATION_URL")
            .map_err(|_| {
                LicenseError::ActivationFailed(
                    "License activation not configured. Set TRUESHOT_LICENSE_ACTIVATION_URL or import a license JSON payload.".to_string(),
                )
            })?;

        let device_hash = self.device.fingerprint_hash();
        let device_name = device_name_override.unwrap_or_else(|| self.device.device_name());
        let request = ActivationRequest {
            license_key: license_key.to_string(),
            device_hash,
            device_name,
        };

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|err| {
                LicenseError::ActivationFailed(format!("Activation client setup failed: {err}"))
            })?;

        let mut builder = client.post(activation_url).json(&request);
        if let Ok(token) = env::var("TRUESHOT_LICENSE_ACTIVATION_TOKEN") {
            if !token.trim().is_empty() {
                builder = builder.bearer_auth(token);
            }
        }

        let response = builder.send().map_err(|err| {
            LicenseError::ActivationFailed(format!("Activation request failed: {err}"))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(LicenseError::ActivationFailed(format!(
                "Activation failed ({status}): {body}"
            )));
        }

        let activation: ActivationResponse = response.json().map_err(|err| {
            LicenseError::ActivationFailed(format!("Invalid activation response: {err}"))
        })?;

        self.import_license_with_activation(&activation.license_json, activation.device_name)
    }

    /// Import a license file directly
    pub fn import_license(&mut self, license_json: &str) -> Result<(), LicenseError> {
        let license: License = serde_json::from_str(license_json)
            .map_err(|e| LicenseError::SerializationError(e.to_string()))?;

        // Verify signature if we have a key
        if let Some(ref pk) = self.public_key {
            license.verify_signature(pk)?;
        }

        // Check if this device is activated
        let my_hash = self.device.fingerprint_hash();
        if !license.is_device_activated(&my_hash) {
            // Check if we can add this device
            if !license.can_activate_device() {
                return Err(LicenseError::DeviceLimitReached(
                    license.payload.max_devices,
                ));
            }

            // Would need server to add device - for now, error
            return Err(LicenseError::DeviceNotActivated);
        }

        // Save to cache
        self.save_license_cache(&license)?;
        self.current_license = Some(license);

        Ok(())
    }

    /// Import a license file and activate this device if possible.
    pub fn import_license_with_activation(
        &mut self,
        license_json: &str,
        device_name_override: Option<String>,
    ) -> Result<(), LicenseError> {
        let mut license: License = serde_json::from_str(license_json)
            .map_err(|e| LicenseError::SerializationError(e.to_string()))?;

        if let Some(ref pk) = self.public_key {
            license.verify_signature(pk)?;
        }

        if license.is_expired() {
            return Err(LicenseError::Expired);
        }

        let my_hash = self.device.fingerprint_hash();
        let now = Utc::now();

        if let Some(device) = license
            .activated_devices
            .iter_mut()
            .find(|d| d.fingerprint_hash == my_hash)
        {
            device.last_seen = now;
            if let Some(name) = device_name_override {
                device.device_name = name;
            }
        } else {
            if !license.can_activate_device() {
                return Err(LicenseError::DeviceLimitReached(
                    license.payload.max_devices,
                ));
            }
            license.activated_devices.push(ActivatedDevice {
                fingerprint_hash: my_hash,
                device_name: device_name_override.unwrap_or_else(|| self.device.device_name()),
                activated_at: now,
                last_seen: now,
            });
        }

        self.save_license_cache(&license)?;
        self.current_license = Some(license);

        Ok(())
    }

    /// Activate the current device on the loaded license.
    pub fn activate_current_device(
        &mut self,
        device_name_override: Option<String>,
    ) -> Result<(), LicenseError> {
        let license = self
            .current_license
            .as_mut()
            .ok_or(LicenseError::NoLicense)?;

        if let Some(ref pk) = self.public_key {
            license.verify_signature(pk)?;
        } else {
            return Err(LicenseError::MissingPublicKey);
        }

        if license.is_expired() {
            return Err(LicenseError::Expired);
        }

        let my_hash = self.device.fingerprint_hash();
        let now = Utc::now();

        if let Some(device) = license
            .activated_devices
            .iter_mut()
            .find(|d| d.fingerprint_hash == my_hash)
        {
            device.last_seen = now;
            if let Some(name) = device_name_override {
                device.device_name = name;
            }
        } else {
            if !license.can_activate_device() {
                return Err(LicenseError::DeviceLimitReached(
                    license.payload.max_devices,
                ));
            }
            license.activated_devices.push(ActivatedDevice {
                fingerprint_hash: my_hash,
                device_name: device_name_override.unwrap_or_else(|| self.device.device_name()),
                activated_at: now,
                last_seen: now,
            });
        }

        let license_snapshot = license.clone();
        self.save_license_cache(&license_snapshot)?;
        Ok(())
    }

    /// Deactivate a device seat by fingerprint hash.
    pub fn deactivate_device(&mut self, fingerprint_hash: &str) -> Result<(), LicenseError> {
        let license = self
            .current_license
            .as_mut()
            .ok_or(LicenseError::NoLicense)?;

        let before = license.activated_devices.len();
        license
            .activated_devices
            .retain(|d| d.fingerprint_hash != fingerprint_hash);
        if before == license.activated_devices.len() {
            return Err(LicenseError::ActivationFailed(
                "Device not found".to_string(),
            ));
        }

        let license_snapshot = license.clone();
        self.save_license_cache(&license_snapshot)?;
        Ok(())
    }

    /// List activated devices for the loaded license.
    pub fn activated_devices(&self) -> Result<Vec<ActivatedDevice>, LicenseError> {
        let license = self
            .current_license
            .as_ref()
            .ok_or(LicenseError::NoLicense)?;
        Ok(license.activated_devices.clone())
    }

    /// Validate license key format
    fn is_valid_key_format(key: &str) -> bool {
        let parts: Vec<&str> = key.split('-').collect();
        if parts.len() != 4 {
            return false;
        }
        parts
            .iter()
            .all(|p| p.len() == 4 && p.chars().all(|c| c.is_ascii_alphanumeric()))
    }

    /// Get this device's fingerprint hash
    pub fn device_hash(&self) -> String {
        self.device.fingerprint_hash()
    }

    /// Get device info
    pub fn device_name(&self) -> String {
        self.device.device_name()
    }

    /// Check if running in development mode
    pub fn is_dev_mode(&self) -> bool {
        self.dev_mode
    }

    fn is_trial_license(license: &License) -> bool {
        license
            .payload
            .license_key
            .to_uppercase()
            .starts_with("TRIAL-")
            || license.signature.is_empty()
    }

    /// Enable development mode (for testing)
    #[cfg(debug_assertions)]
    pub fn enable_dev_mode(&mut self) {
        self.dev_mode = true;
    }

    /// Create a trial license for evaluation using baseline features.
    pub fn create_trial(&mut self) -> Result<License, LicenseError> {
        self.create_trial_with_features(14, &[])
    }

    /// Create a trial license for evaluation with optional feature overrides.
    pub fn create_trial_with_features(
        &mut self,
        duration_days: i64,
        features: &[Feature],
    ) -> Result<License, LicenseError> {
        if !self.dev_mode && !local_trial_issuer_enabled() {
            return Err(LicenseError::ActivationFailed(
                "Trial creation requires TRUESHOT_LICENSE_DEV_MODE=1 or TRUESHOT_LICENSE_ENABLE_LOCAL_TRIAL_ISSUER=1".to_string(),
            ));
        }
        let mut duration_days = duration_days.clamp(1, 90);
        if duration_days <= 0 {
            duration_days = 14;
        }
        let now = Utc::now();
        let mut trial_features = LicenseFeatures::for_tier(&LicenseTier::Education, false);
        for feature in features {
            trial_features.enable_feature(*feature);
        }
        let payload = LicensePayload {
            license_key: "TRIAL-MODE-0000-0000".to_string(),
            tier: LicenseTier::Education, // Trial gets Education features
            max_devices: 1,
            issued_at: now,
            expires_at: Some(now + chrono::Duration::days(duration_days)),
            features: trial_features,
            customer_email: "trial@localhost".to_string(),
            has_4dgs_addon: false,
        };

        let activated = ActivatedDevice {
            fingerprint_hash: self.device.fingerprint_hash(),
            device_name: self.device.device_name(),
            activated_at: now,
            last_seen: now,
        };

        let license = License {
            payload,
            activated_devices: vec![activated],
            signature: String::new(), // Trial has no signature
        };

        self.current_license = Some(license.clone());
        let _ = self.save_license_cache(&license);

        Ok(license)
    }
}

fn load_public_key_override() -> Result<Option<VerifyingKey>, LicenseError> {
    if let Ok(path) = env::var("TRUESHOT_LICENSE_PUBLIC_KEY_PATH") {
        let bytes =
            fs::read(&path).map_err(|e| LicenseError::FileNotFound(format!("{path}: {e}")))?;
        return decode_public_key_bytes(&bytes).map(Some);
    }

    if let Ok(b64) = env::var("TRUESHOT_LICENSE_PUBLIC_KEY_B64") {
        let decoded = B64.decode(b64.as_bytes()).map_err(|e| {
            LicenseError::InvalidSignature(format!("Invalid base64 public key: {e}"))
        })?;
        return decode_public_key_bytes(&decoded).map(Some);
    }

    if let Ok(hex) = env::var("TRUESHOT_LICENSE_PUBLIC_KEY_HEX") {
        let decoded = decode_hex_key(&hex)?;
        return decode_public_key_bytes(&decoded).map(Some);
    }

    Ok(None)
}

fn dev_mode_enabled() -> bool {
    if !cfg!(any(debug_assertions, feature = "dev_license")) {
        return false;
    }
    match env::var("TRUESHOT_LICENSE_DEV_MODE") {
        Ok(value) => matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"),
        Err(_) => false,
    }
}

fn local_trial_issuer_enabled() -> bool {
    match env::var("TRUESHOT_LICENSE_ENABLE_LOCAL_TRIAL_ISSUER") {
        Ok(value) => matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"),
        Err(_) => false,
    }
}

fn decode_public_key_bytes(bytes: &[u8]) -> Result<VerifyingKey, LicenseError> {
    let key_bytes = if bytes.len() == 32 {
        let mut out = [0u8; 32];
        out.copy_from_slice(bytes);
        out
    } else {
        let text = std::str::from_utf8(bytes)
            .map_err(|e| LicenseError::InvalidSignature(e.to_string()))?
            .trim();
        if text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit()) {
            let decoded = decode_hex_key(text)?;
            let mut out = [0u8; 32];
            out.copy_from_slice(&decoded);
            out
        } else {
            let decoded = B64
                .decode(text.as_bytes())
                .map_err(|e| LicenseError::InvalidSignature(format!("Invalid base64 key: {e}")))?;
            if decoded.len() != 32 {
                return Err(LicenseError::InvalidSignature(
                    "Base64 key must decode to 32 bytes".to_string(),
                ));
            }
            let mut out = [0u8; 32];
            out.copy_from_slice(&decoded);
            out
        }
    };
    VerifyingKey::from_bytes(&key_bytes).map_err(|e| LicenseError::InvalidSignature(e.to_string()))
}

fn decode_hex_key(hex: &str) -> Result<Vec<u8>, LicenseError> {
    let mut out = Vec::with_capacity(hex.len() / 2);
    let mut chars = hex.chars().filter(|c| !c.is_whitespace());
    while let Some(hi) = chars.next() {
        let lo = chars
            .next()
            .ok_or_else(|| LicenseError::InvalidSignature("Odd-length hex key".to_string()))?;
        let byte = (hex_val(hi)? << 4) | hex_val(lo)?;
        out.push(byte);
    }
    if out.len() != 32 {
        return Err(LicenseError::InvalidSignature(
            "Hex public key must decode to 32 bytes".to_string(),
        ));
    }
    Ok(out)
}

fn hex_val(c: char) -> Result<u8, LicenseError> {
    match c {
        '0'..='9' => Ok(c as u8 - b'0'),
        'a'..='f' => Ok(c as u8 - b'a' + 10),
        'A'..='F' => Ok(c as u8 - b'A' + 10),
        _ => Err(LicenseError::InvalidSignature(
            "Invalid hex in public key".to_string(),
        )),
    }
}

impl Default for LicenseManager {
    fn default() -> Self {
        Self::new().expect("Failed to initialize license manager")
    }
}

/// License status summary
#[derive(Clone, Debug)]
pub enum LicenseStatus {
    /// Development mode (all features enabled)
    Development,
    /// No license found
    Unlicensed,
    /// License not activated on this device
    NotActivated,
    /// License has expired
    Expired,
    /// Offline grace period has expired
    GracePeriodExpired,
    /// Valid license
    Valid {
        tier: LicenseTier,
        expires: Option<chrono::DateTime<Utc>>,
    },
}

impl LicenseStatus {
    /// Check if status allows using the software
    pub fn is_usable(&self) -> bool {
        matches!(
            self,
            LicenseStatus::Development | LicenseStatus::Valid { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_license_manager_creation() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("TRUESHOT_LICENSE_DEV_MODE");
        let err = LicenseManager::new().unwrap_err();
        assert!(matches!(err, LicenseError::MissingPublicKey));
    }

    #[test]
    fn test_dev_mode_features() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::set_var("TRUESHOT_LICENSE_DEV_MODE", "1");
        let manager = LicenseManager::new().unwrap();
        assert!(manager.is_dev_mode());
        assert!(manager.is_feature_enabled(Feature::FourDGS));
        assert!(manager.is_feature_enabled(Feature::Resolution8K));
        assert!(manager.is_feature_enabled(Feature::CommercialUse));
        env::remove_var("TRUESHOT_LICENSE_DEV_MODE");
    }

    #[test]
    fn test_key_format_validation() {
        assert!(LicenseManager::is_valid_key_format("ABCD-1234-EFGH-5678"));
        assert!(!LicenseManager::is_valid_key_format("INVALID"));
        assert!(!LicenseManager::is_valid_key_format("ABC-1234-EFGH-5678"));
        assert!(!LicenseManager::is_valid_key_format("ABCD-1234-EFGH-567"));
    }

    #[test]
    fn test_trial_creation() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::set_var("TRUESHOT_LICENSE_DEV_MODE", "1");
        let mut manager = LicenseManager::new().unwrap();
        let trial = manager.create_trial().unwrap();

        assert_eq!(trial.payload.tier, LicenseTier::Education);
        assert!(trial.payload.expires_at.is_some());
        assert_eq!(trial.activated_devices.len(), 1);
        env::remove_var("TRUESHOT_LICENSE_DEV_MODE");
    }

    #[test]
    fn test_license_status() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::set_var("TRUESHOT_LICENSE_DEV_MODE", "1");
        let manager = LicenseManager::new().unwrap();
        let status = manager.status();
        assert!(matches!(status, LicenseStatus::Development));
        assert!(status.is_usable());
        env::remove_var("TRUESHOT_LICENSE_DEV_MODE");
    }
}
