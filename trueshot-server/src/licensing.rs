use std::collections::{BTreeMap, HashSet};

use actix_web::{web, HttpResponse};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::MutexGuard;
use trueshot_core::licensing::{Feature, LicenseManager, LicenseStatus, LicenseTier};

use crate::state::AppState;

#[derive(Debug)]
pub struct LicenseGate {
    manager: Option<LicenseManager>,
    init_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LicenseSnapshot {
    pub status: String,
    pub license_valid: bool,
    pub tier: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub device_hash: Option<String>,
    pub trial_active: bool,
    pub trial_expires_at: Option<DateTime<Utc>>,
    pub trial_days_remaining: Option<i64>,
    pub init_error: Option<String>,
    pub verification_error: Option<String>,
    pub features: BTreeMap<String, bool>,
    pub bundles: BTreeMap<String, bool>,
}

#[derive(Debug, Clone)]
pub struct ScanLimit {
    pub key: String,
    pub max: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct BundleDefinition {
    pub key: String,
    pub name: String,
    pub description: String,
    pub features: Vec<String>,
    pub price_usd: u32,
    pub billing: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TierDefinition {
    pub key: String,
    pub name: String,
    pub max_devices: u32,
    pub price_usd: u32,
    pub billing: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LicenseDeviceRecord {
    pub fingerprint_hash: String,
    pub device_name: String,
    pub activated_at: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

impl LicenseGate {
    pub fn initialize() -> Self {
        match LicenseManager::new() {
            Ok(manager) => Self {
                manager: Some(manager),
                init_error: None,
            },
            Err(err) => Self {
                manager: None,
                init_error: Some(err.to_string()),
            },
        }
    }

    fn ensure_manager(&mut self) -> Result<&mut LicenseManager, String> {
        if self.manager.is_none() {
            match LicenseManager::new() {
                Ok(manager) => {
                    self.manager = Some(manager);
                    self.init_error = None;
                }
                Err(err) => {
                    self.init_error = Some(err.to_string());
                }
            }
        }
        self.manager.as_mut().ok_or_else(|| {
            self.init_error
                .clone()
                .unwrap_or_else(|| "License subsystem unavailable".to_string())
        })
    }

    pub fn status_snapshot(&mut self) -> LicenseSnapshot {
        let mut status = "unavailable".to_string();
        let mut tier = None;
        let mut expires_at = None;
        let mut device_hash = None;
        let mut trial_active = false;
        let mut trial_expires_at = None;
        let mut trial_days_remaining = None;
        let mut verification_error = None;
        let mut features = BTreeMap::new();
        let mut bundles = BTreeMap::new();
        let mut license_valid = false;

        if let Ok(manager) = self.ensure_manager() {
            let verification = manager.verify();
            if let Err(err) = &verification {
                verification_error = Some(err.to_string());
            }
            license_valid = verification.is_ok();

            match manager.status() {
                LicenseStatus::Development => {
                    status = "development".to_string();
                    license_valid = true;
                }
                LicenseStatus::Unlicensed => status = "unlicensed".to_string(),
                LicenseStatus::NotActivated => status = "not_activated".to_string(),
                LicenseStatus::Expired => status = "expired".to_string(),
                LicenseStatus::GracePeriodExpired => status = "grace_period_expired".to_string(),
                LicenseStatus::Valid { tier: t, expires } => {
                    status = "valid".to_string();
                    tier = Some(tier_display_name(&t));
                    expires_at = expires;
                }
            }

            if tier.is_none() {
                tier = manager.tier().map(tier_display_name);
            }
            device_hash = Some(manager.device_hash());
            if let Some(trial) = manager.trial_info() {
                trial_active = trial.active;
                trial_expires_at = trial.expires_at;
                trial_days_remaining = trial.days_remaining;
            }

            for (name, feature) in tracked_features() {
                let enabled = if license_valid {
                    manager.is_feature_enabled(feature)
                } else {
                    false
                };
                features.insert(name.to_string(), enabled);
            }

            for bundle in bundle_catalog() {
                let enabled = if license_valid {
                    bundle_feature_set(bundle.key.as_str())
                        .iter()
                        .all(|feature| manager.is_feature_enabled(*feature))
                } else {
                    false
                };
                bundles.insert(bundle.key, enabled);
            }
        }

        LicenseSnapshot {
            status,
            license_valid,
            tier,
            expires_at,
            device_hash,
            trial_active,
            trial_expires_at,
            trial_days_remaining,
            init_error: self.init_error.clone(),
            verification_error,
            features,
            bundles,
        }
    }

    pub fn require_feature(&mut self, feature: Feature) -> Result<(), String> {
        let manager = self.ensure_manager()?;
        manager
            .verify()
            .map_err(|err| format!("License verification failed: {err}"))?;
        manager
            .require_feature(feature)
            .map_err(|err| err.to_string())
    }

    pub fn import_license_json(&mut self, license_json: &str) -> Result<(), String> {
        let manager = self.ensure_manager()?;
        manager
            .import_license_with_activation(license_json, None)
            .map_err(|err| err.to_string())
    }

    pub fn activate_with_key(
        &mut self,
        license_key: &str,
        device_name: Option<String>,
    ) -> Result<LicenseSnapshot, String> {
        let manager = self.ensure_manager()?;
        manager
            .load_license_key(license_key, device_name)
            .map_err(|err| err.to_string())?;
        Ok(self.status_snapshot())
    }

    pub fn create_trial(
        &mut self,
        duration_days: i64,
        bundles: &[String],
    ) -> Result<LicenseSnapshot, String> {
        let manager = self.ensure_manager()?;
        let selected_bundles = if bundles.is_empty() {
            default_trial_bundles()
        } else {
            bundles.to_vec()
        };
        let mut features = Vec::new();
        let mut seen = HashSet::new();
        for bundle in selected_bundles {
            for feature in bundle_feature_set(bundle.as_str()) {
                if seen.insert(feature) {
                    features.push(feature);
                }
            }
        }
        manager
            .create_trial_with_features(duration_days, &features)
            .map_err(|err| err.to_string())?;
        Ok(self.status_snapshot())
    }

    pub fn list_activated_devices(&mut self) -> Result<Vec<LicenseDeviceRecord>, String> {
        let manager = self.ensure_manager()?;
        let devices = manager.activated_devices().map_err(|err| err.to_string())?;
        Ok(devices
            .into_iter()
            .map(|device| LicenseDeviceRecord {
                fingerprint_hash: device.fingerprint_hash,
                device_name: device.device_name,
                activated_at: device.activated_at,
                last_seen: device.last_seen,
            })
            .collect())
    }

    pub fn activate_current_device(
        &mut self,
        device_name: Option<String>,
    ) -> Result<LicenseSnapshot, String> {
        let manager = self.ensure_manager()?;
        manager
            .activate_current_device(device_name)
            .map_err(|err| err.to_string())?;
        Ok(self.status_snapshot())
    }

    pub fn deactivate_device(&mut self, fingerprint_hash: &str) -> Result<LicenseSnapshot, String> {
        let manager = self.ensure_manager()?;
        manager
            .deactivate_device(fingerprint_hash)
            .map_err(|err| err.to_string())?;
        Ok(self.status_snapshot())
    }

    pub fn scan_limit(&mut self) -> Option<ScanLimit> {
        let manager = self.ensure_manager().ok()?;
        if manager.verify().is_err() {
            return None;
        }
        let max = manager.scans_per_month()?;
        let key = manager
            .license_key_hash()
            .unwrap_or_else(|| manager.device_hash());
        Some(ScanLimit { key, max })
    }

    pub fn max_resolution(&mut self) -> Option<u32> {
        let manager = self.ensure_manager().ok()?;
        if manager.verify().is_err() {
            return None;
        }
        manager.max_resolution()
    }
}

pub fn lock_license_gate(
    state: &web::Data<AppState>,
) -> Result<MutexGuard<'_, LicenseGate>, HttpResponse> {
    crate::sync_lock::lock(&state.license_gate, "license.gate")
        .map_err(|_| HttpResponse::ServiceUnavailable().finish())
}

pub async fn enforce_scan_limit(state: &web::Data<AppState>) -> Result<(), HttpResponse> {
    let limit = {
        let mut gate = lock_license_gate(state)?;
        gate.scan_limit()
    };
    let Some(limit) = limit else {
        return Ok(());
    };

    let month_key = Utc::now().format("%Y-%m").to_string();
    let setting_key = format!("scan_usage:{}:{}", limit.key, month_key);
    let current = state
        .auth
        .get_setting(&setting_key)
        .await
        .unwrap_or(None)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);

    if current >= limit.max {
        return Err(HttpResponse::PaymentRequired().json(serde_json::json!({
            "error": "scan_limit_exceeded",
            "capability": "monthly_scans",
            "message": "Monthly scan limit exceeded for this license.",
            "limit": limit.max,
            "current": current,
            "month": month_key,
        })));
    }

    let next = current + 1;
    let _ = state
        .auth
        .set_setting(&setting_key, &next.to_string())
        .await;

    Ok(())
}

pub fn require_license_feature(
    state: &web::Data<AppState>,
    feature: Feature,
    capability: &'static str,
) -> Result<(), HttpResponse> {
    let mut gate = lock_license_gate(state)?;
    match gate.require_feature(feature) {
        Ok(()) => Ok(()),
        Err(err) => Err(HttpResponse::PaymentRequired().json(serde_json::json!({
            "error": "feature_not_entitled",
            "capability": capability,
            "message": err,
            "hint": "Upgrade your TrueShot license or enable an add-on/trial for this feature."
        }))),
    }
}

pub fn sync_trial_env(snapshot: &LicenseSnapshot) {
    if snapshot.trial_active {
        std::env::set_var("TRUESHOT_LICENSE_TRIAL", "1");
        if let Some(expires) = snapshot.trial_expires_at {
            std::env::set_var("TRUESHOT_LICENSE_TRIAL_EXPIRES_AT", expires.to_rfc3339());
        } else {
            std::env::remove_var("TRUESHOT_LICENSE_TRIAL_EXPIRES_AT");
        }
        if let Some(days) = snapshot.trial_days_remaining {
            std::env::set_var("TRUESHOT_LICENSE_TRIAL_DAYS_REMAINING", days.to_string());
        } else {
            std::env::remove_var("TRUESHOT_LICENSE_TRIAL_DAYS_REMAINING");
        }
    } else {
        std::env::set_var("TRUESHOT_LICENSE_TRIAL", "0");
        std::env::remove_var("TRUESHOT_LICENSE_TRIAL_EXPIRES_AT");
        std::env::remove_var("TRUESHOT_LICENSE_TRIAL_DAYS_REMAINING");
    }
}

pub fn bundle_catalog() -> Vec<BundleDefinition> {
    vec![
        BundleDefinition {
            key: "advanced_capture".to_string(),
            name: "Advanced Capture Automation".to_string(),
            description: "HDR bracketing, focus stacking, and intervalometer orchestration"
                .to_string(),
            features: vec!["advanced_capture_automation".to_string()],
            price_usd: 49,
            billing: "lifetime".to_string(),
        },
        BundleDefinition {
            key: "room_reconstruction".to_string(),
            name: "Room Reconstruction".to_string(),
            description: "Room-scale scan planning and reconstruction workflows".to_string(),
            features: vec!["room_reconstruction".to_string()],
            price_usd: 79,
            billing: "lifetime".to_string(),
        },
        BundleDefinition {
            key: "avatar_studio".to_string(),
            name: "Avatar Studio".to_string(),
            description: "Avatar capture and reconstruction pipelines".to_string(),
            features: vec!["avatar_reconstruction".to_string()],
            price_usd: 99,
            billing: "lifetime".to_string(),
        },
        BundleDefinition {
            key: "cloud_sync_backup".to_string(),
            name: "Cloud Sync + Backup".to_string(),
            description: "Provider connectors, sync validation, backup and restore".to_string(),
            features: vec!["cloud_sync_backup".to_string()],
            price_usd: 79,
            billing: "lifetime".to_string(),
        },
        BundleDefinition {
            key: "team_collaboration".to_string(),
            name: "Team Collaboration".to_string(),
            description: "Public gallery, analytics, and review collaboration controls".to_string(),
            features: vec!["team_collaboration".to_string()],
            price_usd: 99,
            billing: "lifetime".to_string(),
        },
        BundleDefinition {
            key: "pipeline_automation".to_string(),
            name: "Pipeline Automation".to_string(),
            description: "Automation API, webhooks, and CI/CD pipeline hooks".to_string(),
            features: vec!["pipeline_automation".to_string()],
            price_usd: 99,
            billing: "lifetime".to_string(),
        },
        BundleDefinition {
            key: "dynamic_4dgs".to_string(),
            name: "Dynamic 4D Gaussian Splatting".to_string(),
            description: "Time-varying 4DGS capture and rendering workflows".to_string(),
            features: vec!["4dgs".to_string()],
            price_usd: 149,
            billing: "lifetime".to_string(),
        },
    ]
}

pub fn tier_catalog() -> Vec<TierDefinition> {
    vec![
        TierDefinition {
            key: "hobby".to_string(),
            name: "Core Solo".to_string(),
            max_devices: LicenseTier::Hobby.max_devices(),
            price_usd: 99,
            billing: "lifetime".to_string(),
        },
        TierDefinition {
            key: "education".to_string(),
            name: "Core Team".to_string(),
            max_devices: LicenseTier::Education.max_devices(),
            price_usd: 249,
            billing: "lifetime".to_string(),
        },
        TierDefinition {
            key: "pro".to_string(),
            name: "Core Studio".to_string(),
            max_devices: LicenseTier::Pro.max_devices(),
            price_usd: 599,
            billing: "lifetime".to_string(),
        },
    ]
}

pub fn tier_definition_for(tier: &LicenseTier) -> Option<TierDefinition> {
    match tier {
        LicenseTier::Hobby => Some(TierDefinition {
            key: "hobby".to_string(),
            name: "Core Solo".to_string(),
            max_devices: LicenseTier::Hobby.max_devices(),
            price_usd: 99,
            billing: "lifetime".to_string(),
        }),
        LicenseTier::Education => Some(TierDefinition {
            key: "education".to_string(),
            name: "Core Team".to_string(),
            max_devices: LicenseTier::Education.max_devices(),
            price_usd: 249,
            billing: "lifetime".to_string(),
        }),
        LicenseTier::Pro => Some(TierDefinition {
            key: "pro".to_string(),
            name: "Core Studio".to_string(),
            max_devices: LicenseTier::Pro.max_devices(),
            price_usd: 599,
            billing: "lifetime".to_string(),
        }),
    }
}

pub fn tier_display_name(tier: &LicenseTier) -> String {
    tier_definition_for(tier)
        .map(|definition| definition.name)
        .unwrap_or_else(|| tier.display_name().to_string())
}

fn tracked_features() -> Vec<(&'static str, Feature)> {
    vec![
        ("basic_scanning", Feature::BasicScanning),
        ("gaussian_splatting", Feature::GaussianSplatting),
        ("resolution_4k", Feature::Resolution4K),
        ("resolution_8k", Feature::Resolution8K),
        ("webxr_scanning", Feature::WebXRScanning),
        ("commercial_use", Feature::CommercialUse),
        ("unlimited_scans", Feature::UnlimitedScans),
        ("4dgs", Feature::FourDGS),
        ("room_reconstruction", Feature::RoomReconstruction),
        ("avatar_reconstruction", Feature::AvatarReconstruction),
        (
            "advanced_capture_automation",
            Feature::AdvancedCaptureAutomation,
        ),
        ("cloud_sync_backup", Feature::CloudSyncBackup),
        ("team_collaboration", Feature::TeamCollaboration),
        ("pipeline_automation", Feature::PipelineAutomation),
        ("priority_support", Feature::PrioritySupport),
        ("beta_features", Feature::BetaFeatures),
    ]
}

fn default_trial_bundles() -> Vec<String> {
    vec![
        "advanced_capture".to_string(),
        "room_reconstruction".to_string(),
        "avatar_studio".to_string(),
        "cloud_sync_backup".to_string(),
        "team_collaboration".to_string(),
        "pipeline_automation".to_string(),
        "dynamic_4dgs".to_string(),
    ]
}

fn bundle_feature_set(bundle: &str) -> Vec<Feature> {
    match bundle {
        "advanced_capture" => vec![Feature::AdvancedCaptureAutomation],
        "room_reconstruction" => vec![Feature::RoomReconstruction],
        "avatar_studio" => vec![Feature::AvatarReconstruction],
        "cloud_sync_backup" => vec![Feature::CloudSyncBackup],
        "team_collaboration" => vec![Feature::TeamCollaboration],
        "pipeline_automation" => vec![Feature::PipelineAutomation],
        "dynamic_4dgs" => vec![Feature::FourDGS],
        _ => Vec::new(),
    }
}
