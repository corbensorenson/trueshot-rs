//! Sensor-domain Poisson-Gaussian noise models for RAW inference.
//!
//! The model is intentionally expressed in native digital numbers (DN). This
//! keeps camera calibration auditable and avoids claiming electron-domain
//! accuracy until photon-transfer measurements have established conversion
//! gain for a camera, ISO, and CFA site.

use anyhow::Result;

/// Per-CFA-site sensor model in local RGGB order: R, G1, G2, B.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensorNoiseModel {
    /// Signal-independent temporal read noise, in DN standard deviation.
    pub read_noise_dn: [f32; 4],
    /// Conversion gain in electrons per DN. Shot variance in DN is
    /// `signal_dn / electrons_per_dn`.
    pub electrons_per_dn: [f32; 4],
    /// Additional black-level drift/fixed-pattern uncertainty in DN.
    pub black_drift_dn: [f32; 4],
    /// Distance below the encoded white level treated as censored saturation.
    pub saturation_margin_dn: f32,
    /// True only for a profile produced by a retained calibration artifact.
    pub calibrated: bool,
}

impl SensorNoiseModel {
    /// Conservative compatibility model for cameras without a measured
    /// photon-transfer profile. Callers must expose its uncalibrated status.
    pub fn conservative(read_noise_dn: f32) -> Self {
        Self {
            read_noise_dn: [read_noise_dn; 4],
            electrons_per_dn: [1.0; 4],
            black_drift_dn: [0.5; 4],
            saturation_margin_dn: 16.0,
            calibrated: false,
        }
    }

    pub fn validate(self) -> Result<Self> {
        for (site, ((read_noise, electrons_per_dn), black_drift)) in self
            .read_noise_dn
            .into_iter()
            .zip(self.electrons_per_dn)
            .zip(self.black_drift_dn)
            .enumerate()
        {
            if !read_noise.is_finite() || read_noise <= 0.0 {
                anyhow::bail!("CFA site {site} has invalid read noise {read_noise}");
            }
            if !electrons_per_dn.is_finite() || electrons_per_dn <= 0.0 {
                anyhow::bail!("CFA site {site} has invalid conversion gain {electrons_per_dn}");
            }
            if !black_drift.is_finite() || black_drift < 0.0 {
                anyhow::bail!("CFA site {site} has invalid black drift {black_drift}");
            }
        }
        if !self.saturation_margin_dn.is_finite() || self.saturation_margin_dn < 0.0 {
            anyhow::bail!(
                "Sensor saturation margin must be finite and nonnegative, got {}",
                self.saturation_margin_dn
            );
        }
        Ok(self)
    }

    /// Predict normalized sensor-signal variance for a black-subtracted sample.
    pub fn normalized_variance(self, site: usize, signal: f32, range_dn: f32) -> f32 {
        let site = site.min(3);
        let signal_dn = signal.max(0.0) * range_dn;
        let temporal = self.read_noise_dn[site] * self.read_noise_dn[site];
        let black = self.black_drift_dn[site] * self.black_drift_dn[site];
        let shot = signal_dn / self.electrons_per_dn[site];
        (temporal + black + shot) / (range_dn * range_dn)
    }

    pub fn saturation_signal(self, range_dn: f32) -> f32 {
        (1.0 - self.saturation_margin_dn / range_dn).clamp(0.5, 1.0 - f32::EPSILON)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IsoNoiseModel {
    pub iso: u32,
    pub model: SensorNoiseModel,
}

/// Auditable camera profile containing exact per-ISO noise calibrations.
#[derive(Debug, Clone, PartialEq)]
pub struct SensorNoiseProfile {
    pub camera_make: String,
    pub camera_model: String,
    pub bits_per_sample: u16,
    /// Stable identifier or digest of the retained calibration artifact.
    pub calibration_id: String,
    pub iso_models: Vec<IsoNoiseModel>,
}

impl SensorNoiseProfile {
    pub fn validate(&self) -> Result<()> {
        if self.camera_make.trim().is_empty()
            || self.camera_model.trim().is_empty()
            || self.calibration_id.trim().is_empty()
            || self.bits_per_sample == 0
            || self.iso_models.is_empty()
        {
            anyhow::bail!("Sensor noise profile identity and ISO models are required");
        }
        let mut isos = self
            .iso_models
            .iter()
            .map(|entry| entry.iso)
            .collect::<Vec<_>>();
        isos.sort_unstable();
        if isos.first() == Some(&0) || isos.windows(2).any(|pair| pair[0] == pair[1]) {
            anyhow::bail!("Sensor noise profile ISO values must be positive and unique");
        }
        for entry in &self.iso_models {
            entry.model.validate()?;
            if !entry.model.calibrated {
                anyhow::bail!(
                    "Profile {} contains an uncalibrated ISO {} model",
                    self.calibration_id,
                    entry.iso
                );
            }
        }
        Ok(())
    }

    pub fn matches(&self, make: &str, model: &str, bits_per_sample: u16) -> bool {
        normalized_camera_name(&self.camera_make) == normalized_camera_name(make)
            && normalized_camera_name(&self.camera_model) == normalized_camera_name(model)
            && self.bits_per_sample == bits_per_sample
    }

    pub fn model_for_iso(&self, iso: u32) -> Option<SensorNoiseModel> {
        self.iso_models
            .iter()
            .find(|entry| entry.iso == iso)
            .map(|entry| entry.model)
    }
}

fn normalized_camera_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{IsoNoiseModel, SensorNoiseModel, SensorNoiseProfile};

    #[test]
    fn normalized_variance_matches_dn_model() {
        let model = SensorNoiseModel {
            read_noise_dn: [2.0; 4],
            electrons_per_dn: [0.5; 4],
            black_drift_dn: [1.0; 4],
            saturation_margin_dn: 8.0,
            calibrated: true,
        };
        let range = 1000.0;
        let expected_dn_variance = 4.0 + 1.0 + 200.0 / 0.5;
        assert!(
            (model.normalized_variance(1, 0.2, range) - expected_dn_variance / (range * range))
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn invalid_profiles_fail_closed() {
        let mut model = SensorNoiseModel::conservative(3.0);
        model.electrons_per_dn[2] = 0.0;
        assert!(model.validate().is_err());
    }

    #[test]
    fn camera_profiles_require_exact_iso_and_identity() {
        let profile = SensorNoiseProfile {
            camera_make: "NIKON CORPORATION".to_string(),
            camera_model: "NIKON Z 9".to_string(),
            bits_per_sample: 14,
            calibration_id: "sha256:test".to_string(),
            iso_models: vec![IsoNoiseModel {
                iso: 64,
                model: SensorNoiseModel {
                    calibrated: true,
                    ..SensorNoiseModel::conservative(2.0)
                },
            }],
        };
        profile.validate().unwrap();
        assert!(profile.matches("Nikon Corporation", "Nikon Z9", 14));
        assert!(profile.model_for_iso(64).is_some());
        assert!(profile.model_for_iso(100).is_none());
    }
}
