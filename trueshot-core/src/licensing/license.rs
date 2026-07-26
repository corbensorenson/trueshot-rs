//! License data structures and cryptographic verification
//!
//! Ed25519 signed licenses with device activation tracking.

use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use super::error::LicenseError;

/// License tiers with device limits.
/// Pricing is defined in the server catalog and should not be hardcoded here.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LicenseTier {
    /// Hobby tier: 1 device, limited features
    Hobby,
    /// Education tier: 3 devices, most features
    Education,
    /// Pro tier: 10 devices, all features
    Pro,
}

impl LicenseTier {
    /// Get maximum allowed devices for this tier
    pub fn max_devices(&self) -> u32 {
        match self {
            LicenseTier::Hobby => 1,
            LicenseTier::Education => 3,
            LicenseTier::Pro => 10,
        }
    }

    /// Get tier display name
    pub fn display_name(&self) -> &'static str {
        match self {
            LicenseTier::Hobby => "Hobby",
            LicenseTier::Education => "Education",
            LicenseTier::Pro => "Pro",
        }
    }
}

/// Per-device activation record
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActivatedDevice {
    /// SHA-256 hash of device fingerprint
    pub fingerprint_hash: String,
    /// User-friendly device name
    pub device_name: String,
    /// When this device was activated
    pub activated_at: DateTime<Utc>,
    /// Last time device checked in
    pub last_seen: DateTime<Utc>,
}

/// Feature flags controlled by license
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct LicenseFeatures {
    /// Maximum output resolution (e.g., 2000, 4000, 8000)
    pub max_resolution: u32,
    /// Scans allowed per month (None = unlimited)
    pub scans_per_month: Option<u32>,
    /// 4D Gaussian Splatting enabled
    pub enable_4dgs: bool,
    /// WebXR VR scanning enabled
    pub enable_webxr_scanning: bool,
    /// Commercial use allowed
    pub enable_commercial: bool,
    /// Early access to beta features
    pub enable_beta: bool,
    /// Priority support access
    pub enable_priority_support: bool,
    /// Room-scale reconstruction workflows
    #[serde(default)]
    pub enable_room_reconstruction: bool,
    /// Avatar capture and reconstruction workflows
    #[serde(default)]
    pub enable_avatar_reconstruction: bool,
    /// Advanced capture automation workflows
    #[serde(default)]
    pub enable_advanced_capture_automation: bool,
    /// Cloud/NAS sync and backup workflows
    #[serde(default)]
    pub enable_cloud_sync_backup: bool,
    /// Public/team collaboration workflows
    #[serde(default)]
    pub enable_team_collaboration: bool,
    /// Pipeline automation API and webhooks
    #[serde(default)]
    pub enable_pipeline_automation: bool,
}

impl LicenseFeatures {
    /// Create default features for a tier
    pub fn for_tier(tier: &LicenseTier, has_4dgs_addon: bool) -> Self {
        match tier {
            LicenseTier::Hobby => Self {
                max_resolution: 2000,
                scans_per_month: Some(20),
                enable_4dgs: has_4dgs_addon, // $50 addon
                enable_webxr_scanning: true,
                enable_commercial: false,
                enable_beta: false,
                enable_priority_support: false,
                enable_room_reconstruction: false,
                enable_avatar_reconstruction: false,
                enable_advanced_capture_automation: false,
                enable_cloud_sync_backup: false,
                enable_team_collaboration: false,
                enable_pipeline_automation: false,
            },
            LicenseTier::Education => Self {
                max_resolution: 4000,
                scans_per_month: None,
                enable_4dgs: has_4dgs_addon, // $50 addon
                enable_webxr_scanning: true,
                enable_commercial: false,
                enable_beta: false,
                enable_priority_support: false,
                enable_room_reconstruction: false,
                enable_avatar_reconstruction: false,
                enable_advanced_capture_automation: false,
                enable_cloud_sync_backup: false,
                enable_team_collaboration: false,
                enable_pipeline_automation: false,
            },
            LicenseTier::Pro => Self {
                max_resolution: 8000,
                scans_per_month: None,
                enable_4dgs: has_4dgs_addon, // Add-on unless explicitly provisioned
                enable_webxr_scanning: true,
                enable_commercial: true,
                enable_beta: true,
                enable_priority_support: true,
                enable_room_reconstruction: false,
                enable_avatar_reconstruction: false,
                enable_advanced_capture_automation: false,
                enable_cloud_sync_backup: false,
                enable_team_collaboration: false,
                enable_pipeline_automation: false,
            },
        }
    }

    /// Enable a specific feature flag directly (used by targeted trial packs).
    pub fn enable_feature(&mut self, feature: super::Feature) {
        use super::Feature;
        match feature {
            Feature::BasicScanning | Feature::GaussianSplatting | Feature::UnlimitedScans => {}
            Feature::Resolution4K => {
                self.max_resolution = self.max_resolution.max(4000);
            }
            Feature::Resolution8K => {
                self.max_resolution = self.max_resolution.max(8000);
            }
            Feature::FourDGS => self.enable_4dgs = true,
            Feature::WebXRScanning => self.enable_webxr_scanning = true,
            Feature::CommercialUse => self.enable_commercial = true,
            Feature::BetaFeatures => self.enable_beta = true,
            Feature::PrioritySupport => self.enable_priority_support = true,
            Feature::RoomReconstruction => self.enable_room_reconstruction = true,
            Feature::AvatarReconstruction => self.enable_avatar_reconstruction = true,
            Feature::AdvancedCaptureAutomation => self.enable_advanced_capture_automation = true,
            Feature::CloudSyncBackup => self.enable_cloud_sync_backup = true,
            Feature::TeamCollaboration => self.enable_team_collaboration = true,
            Feature::PipelineAutomation => self.enable_pipeline_automation = true,
        }
    }
}

/// License payload for signature verification
///
/// This is what gets signed - excludes signature itself and mutable fields
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LicensePayload {
    /// Unique license key (e.g., XXXX-XXXX-XXXX-XXXX)
    pub license_key: String,
    /// License tier
    pub tier: LicenseTier,
    /// Maximum allowed devices
    pub max_devices: u32,
    /// When license was issued
    pub issued_at: DateTime<Utc>,
    /// When license expires (None = perpetual)
    pub expires_at: Option<DateTime<Utc>>,
    /// Feature flags
    pub features: LicenseFeatures,
    /// Customer email (for display only)
    pub customer_email: String,
    /// Has 4DGS addon (for non-Pro tiers)
    pub has_4dgs_addon: bool,
}

/// Complete license with signature and activation state
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct License {
    /// Core license data
    #[serde(flatten)]
    pub payload: LicensePayload,
    /// Activated devices (mutable, not part of signature)
    pub activated_devices: Vec<ActivatedDevice>,
    /// Ed25519 signature over payload (base64 encoded)
    pub signature: String,
}

impl License {
    /// Grace period for offline operation (90 days)
    pub const OFFLINE_GRACE_DAYS: i64 = 90;

    /// Verify license signature using public key
    pub fn verify_signature(&self, public_key: &VerifyingKey) -> Result<(), LicenseError> {
        // Serialize payload for verification
        let payload_bytes = serde_json::to_vec(&self.payload)
            .map_err(|e| LicenseError::SerializationError(e.to_string()))?;

        // Decode signature from base64
        let sig_bytes =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &self.signature)
                .map_err(|e| LicenseError::InvalidSignature(e.to_string()))?;

        // Parse signature
        let signature = Signature::from_slice(&sig_bytes)
            .map_err(|e| LicenseError::InvalidSignature(e.to_string()))?;

        // Verify
        public_key
            .verify(&payload_bytes, &signature)
            .map_err(|_| LicenseError::SignatureVerificationFailed)?;

        Ok(())
    }

    /// Check if license is expired
    pub fn is_expired(&self) -> bool {
        if let Some(expires) = self.payload.expires_at {
            Utc::now() > expires
        } else {
            false // Perpetual license
        }
    }

    /// Check if device is activated on this license
    pub fn is_device_activated(&self, fingerprint_hash: &str) -> bool {
        self.activated_devices
            .iter()
            .any(|d| d.fingerprint_hash == fingerprint_hash)
    }

    /// Check if more devices can be activated
    pub fn can_activate_device(&self) -> bool {
        (self.activated_devices.len() as u32) < self.payload.max_devices
    }

    /// Get device activation if exists
    pub fn get_device_activation(&self, fingerprint_hash: &str) -> Option<&ActivatedDevice> {
        self.activated_devices
            .iter()
            .find(|d| d.fingerprint_hash == fingerprint_hash)
    }

    /// Check if device is within offline grace period
    pub fn is_within_grace_period(&self, fingerprint_hash: &str) -> bool {
        if let Some(device) = self.get_device_activation(fingerprint_hash) {
            let days_since_seen = (Utc::now() - device.last_seen).num_days();
            days_since_seen <= Self::OFFLINE_GRACE_DAYS
        } else {
            false
        }
    }

    /// Check if a specific feature is enabled
    pub fn is_feature_enabled(&self, feature: super::Feature) -> bool {
        use super::Feature;

        match feature {
            Feature::BasicScanning => true,     // Always enabled
            Feature::GaussianSplatting => true, // Core feature
            Feature::Resolution4K => self.payload.features.max_resolution >= 4000,
            Feature::Resolution8K => self.payload.features.max_resolution >= 8000,
            Feature::FourDGS => self.payload.features.enable_4dgs,
            Feature::WebXRScanning => self.payload.features.enable_webxr_scanning,
            Feature::CommercialUse => self.payload.features.enable_commercial,
            Feature::BetaFeatures => self.payload.features.enable_beta,
            Feature::PrioritySupport => self.payload.features.enable_priority_support,
            Feature::UnlimitedScans => self.payload.features.scans_per_month.is_none(),
            Feature::RoomReconstruction => self.payload.features.enable_room_reconstruction,
            Feature::AvatarReconstruction => self.payload.features.enable_avatar_reconstruction,
            Feature::AdvancedCaptureAutomation => {
                self.payload.features.enable_advanced_capture_automation
            }
            Feature::CloudSyncBackup => self.payload.features.enable_cloud_sync_backup,
            Feature::TeamCollaboration => self.payload.features.enable_team_collaboration,
            Feature::PipelineAutomation => self.payload.features.enable_pipeline_automation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_max_devices() {
        assert_eq!(LicenseTier::Hobby.max_devices(), 1);
        assert_eq!(LicenseTier::Education.max_devices(), 3);
        assert_eq!(LicenseTier::Pro.max_devices(), 10);
    }

    #[test]
    fn test_features_for_tier() {
        let hobby = LicenseFeatures::for_tier(&LicenseTier::Hobby, false);
        assert_eq!(hobby.max_resolution, 2000);
        assert!(!hobby.enable_4dgs);
        assert!(!hobby.enable_commercial);

        let hobby_4dgs = LicenseFeatures::for_tier(&LicenseTier::Hobby, true);
        assert!(hobby_4dgs.enable_4dgs);

        let pro = LicenseFeatures::for_tier(&LicenseTier::Pro, false);
        assert_eq!(pro.max_resolution, 8000);
        assert!(!pro.enable_4dgs);
        assert!(pro.enable_commercial);
        assert!(pro.enable_beta);

        let pro_4dgs = LicenseFeatures::for_tier(&LicenseTier::Pro, true);
        assert!(pro_4dgs.enable_4dgs);
    }

    #[test]
    fn test_license_expiry() {
        let payload = LicensePayload {
            license_key: "TEST-1234".to_string(),
            tier: LicenseTier::Pro,
            max_devices: 10,
            issued_at: Utc::now(),
            expires_at: None,
            features: LicenseFeatures::for_tier(&LicenseTier::Pro, false),
            customer_email: "test@example.com".to_string(),
            has_4dgs_addon: false,
        };

        let license = License {
            payload,
            activated_devices: vec![],
            signature: String::new(),
        };

        assert!(!license.is_expired()); // Perpetual
    }
}
