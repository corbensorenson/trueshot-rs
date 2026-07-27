//! Deterministic information-gain planning for HDR and focus acquisition.
//!
//! The planner operates on compact posterior summaries, never image content,
//! and selects only camera-declared candidates with exact sensor calibration.

use crate::sensor_noise::SensorNoiseProfile;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CaptureCandidate {
    pub shutter_seconds: f32,
    pub iso: u32,
    pub focus_diopters: f32,
    pub readout_ms: f32,
    pub settle_ms: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RadianceProbe {
    /// Current normalized scene-radiance posterior mean.
    pub mean: f32,
    /// Current posterior variance in normalized radiance units.
    pub variance: f32,
    /// Relative importance or represented pixel fraction.
    pub weight: f32,
    /// Local RGGB site index.
    pub cfa_site: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FocusProbe {
    pub mean_diopters: f32,
    pub variance_diopters2: f32,
    pub weight: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturePosterior {
    pub radiance: Vec<RadianceProbe>,
    pub focus: Vec<FocusProbe>,
    /// Sensor exposure used to anchor `RadianceProbe::mean`.
    pub radiance_anchor_exposure: f32,
    pub current_focus_diopters: f32,
    pub motion_pixels_per_second: f32,
    pub elapsed_ms: f32,
    pub thermal_load: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AdaptivePlannerConfig {
    pub aperture: f32,
    pub sensor_range_dn: f32,
    pub remaining_time_ms: f32,
    pub maximum_motion_blur_px: f32,
    pub maximum_thermal_load: f32,
    pub thermal_load_per_second: f32,
    pub lens_ms_per_diopter: f32,
    /// Width of one informative focus observation in diopters.
    pub focus_psf_sigma_diopters: f32,
    pub focus_measurement_variance: f32,
    pub minimum_hdr_information_nats: f32,
    pub minimum_focus_information_nats: f32,
}

impl Default for AdaptivePlannerConfig {
    fn default() -> Self {
        Self {
            aperture: 8.0,
            sensor_range_dn: 14_303.0,
            remaining_time_ms: 10_000.0,
            maximum_motion_blur_px: 0.5,
            maximum_thermal_load: 1.0,
            thermal_load_per_second: 0.01,
            lens_ms_per_diopter: 80.0,
            focus_psf_sigma_diopters: 0.15,
            focus_measurement_variance: 0.0025,
            minimum_hdr_information_nats: 0.01,
            minimum_focus_information_nats: 0.01,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CandidateUtility {
    pub candidate: CaptureCandidate,
    pub hdr_information_nats: f32,
    pub focus_information_nats: f32,
    pub capture_cost_ms: f32,
    pub utility_per_ms: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturePlanDecision {
    pub selected: Option<CandidateUtility>,
    pub stop_hdr: bool,
    pub stop_focus: bool,
    pub rejected_motion: usize,
    pub rejected_budget: usize,
    pub rejected_calibration: usize,
}

pub fn plan_next_capture(
    posterior: &CapturePosterior,
    candidates: &[CaptureCandidate],
    sensor_profile: &SensorNoiseProfile,
    config: AdaptivePlannerConfig,
) -> Result<CapturePlanDecision> {
    validate_inputs(posterior, candidates, sensor_profile, config)?;
    let mut utilities = Vec::with_capacity(candidates.len());
    let mut rejected_motion = 0usize;
    let mut rejected_budget = 0usize;
    let mut rejected_calibration = 0usize;

    for &candidate in candidates {
        let Some(noise) = sensor_profile.model_for_iso(candidate.iso) else {
            rejected_calibration += 1;
            continue;
        };
        let lens_ms = (candidate.focus_diopters - posterior.current_focus_diopters).abs()
            * config.lens_ms_per_diopter;
        let capture_cost_ms = candidate.shutter_seconds * 1000.0
            + candidate.readout_ms
            + candidate.settle_ms
            + lens_ms;
        let thermal_after =
            posterior.thermal_load + capture_cost_ms * 0.001 * config.thermal_load_per_second;
        if capture_cost_ms > config.remaining_time_ms
            || posterior.elapsed_ms + capture_cost_ms < posterior.elapsed_ms
            || thermal_after > config.maximum_thermal_load
        {
            rejected_budget += 1;
            continue;
        }
        if candidate.shutter_seconds * posterior.motion_pixels_per_second
            > config.maximum_motion_blur_px
        {
            rejected_motion += 1;
            continue;
        }

        let exposure = candidate.shutter_seconds * candidate.iso as f32
            / (100.0 * config.aperture * config.aperture);
        let exposure_ratio = exposure / posterior.radiance_anchor_exposure;
        let saturation = noise.saturation_signal(config.sensor_range_dn);
        let mut hdr_information_nats = 0.0f32;
        let mut focus_snr = 0.0f32;
        let mut focus_snr_weight = 0.0f32;
        for probe in &posterior.radiance {
            let signal = probe.mean * exposure_ratio;
            if signal >= saturation {
                continue;
            }
            let sensor_variance =
                noise.normalized_variance(probe.cfa_site, signal, config.sensor_range_dn);
            let measurement_variance =
                sensor_variance / (exposure_ratio * exposure_ratio).max(1e-20);
            hdr_information_nats +=
                gaussian_information_gain(probe.variance, measurement_variance) * probe.weight;
            let snr = signal / sensor_variance.sqrt().max(1e-12);
            focus_snr += probe.weight * snr / (snr + 8.0);
            focus_snr_weight += probe.weight;
        }
        let focus_exposure_quality = if focus_snr_weight > 0.0 {
            focus_snr / focus_snr_weight
        } else {
            0.0
        };
        let focus_information_nats = posterior
            .focus
            .iter()
            .map(|probe| {
                let offset = candidate.focus_diopters - probe.mean_diopters;
                let support = (-0.5 * offset * offset
                    / (config.focus_psf_sigma_diopters * config.focus_psf_sigma_diopters))
                    .exp();
                let measurement_variance = config.focus_measurement_variance
                    / (support * focus_exposure_quality).max(1e-6);
                gaussian_information_gain(probe.variance_diopters2, measurement_variance)
                    * probe.weight
            })
            .sum();
        utilities.push(CandidateUtility {
            candidate,
            hdr_information_nats,
            focus_information_nats,
            capture_cost_ms,
            utility_per_ms: (hdr_information_nats + focus_information_nats)
                / capture_cost_ms.max(1e-3),
        });
    }

    let maximum_hdr = utilities
        .iter()
        .map(|utility| utility.hdr_information_nats)
        .fold(0.0f32, f32::max);
    let maximum_focus = utilities
        .iter()
        .map(|utility| utility.focus_information_nats)
        .fold(0.0f32, f32::max);
    let stop_hdr = maximum_hdr < config.minimum_hdr_information_nats;
    let stop_focus = maximum_focus < config.minimum_focus_information_nats;
    let selected = utilities
        .into_iter()
        .filter(|utility| {
            (!stop_hdr && utility.hdr_information_nats >= config.minimum_hdr_information_nats)
                || (!stop_focus
                    && utility.focus_information_nats >= config.minimum_focus_information_nats)
        })
        .max_by(compare_utility);

    Ok(CapturePlanDecision {
        selected,
        stop_hdr,
        stop_focus,
        rejected_motion,
        rejected_budget,
        rejected_calibration,
    })
}

fn gaussian_information_gain(prior_variance: f32, measurement_variance: f32) -> f32 {
    if prior_variance <= 0.0 || measurement_variance <= 0.0 {
        return 0.0;
    }
    0.5 * (1.0 + prior_variance / measurement_variance).ln()
}

fn compare_utility(left: &CandidateUtility, right: &CandidateUtility) -> std::cmp::Ordering {
    left.utility_per_ms
        .total_cmp(&right.utility_per_ms)
        .then_with(|| right.candidate.iso.cmp(&left.candidate.iso))
        .then_with(|| {
            right
                .candidate
                .shutter_seconds
                .total_cmp(&left.candidate.shutter_seconds)
        })
        .then_with(|| {
            right
                .candidate
                .focus_diopters
                .total_cmp(&left.candidate.focus_diopters)
        })
}

fn validate_inputs(
    posterior: &CapturePosterior,
    candidates: &[CaptureCandidate],
    sensor_profile: &SensorNoiseProfile,
    config: AdaptivePlannerConfig,
) -> Result<()> {
    sensor_profile.validate()?;
    if candidates.is_empty() {
        anyhow::bail!("Adaptive capture requires at least one camera-supported candidate");
    }
    if posterior.radiance.is_empty() && posterior.focus.is_empty() {
        anyhow::bail!("Adaptive capture posterior has no HDR or focus evidence");
    }
    let positive = [
        config.aperture,
        config.sensor_range_dn,
        config.remaining_time_ms,
        config.maximum_motion_blur_px,
        config.maximum_thermal_load,
        config.focus_psf_sigma_diopters,
        config.focus_measurement_variance,
    ];
    if positive
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
    {
        anyhow::bail!("Adaptive capture configuration contains invalid positive bounds");
    }
    let nonnegative = [
        config.thermal_load_per_second,
        config.lens_ms_per_diopter,
        config.minimum_hdr_information_nats,
        config.minimum_focus_information_nats,
    ];
    if nonnegative
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        anyhow::bail!("Adaptive capture configuration contains invalid nonnegative bounds");
    }
    if !posterior.current_focus_diopters.is_finite()
        || !posterior.radiance_anchor_exposure.is_finite()
        || posterior.radiance_anchor_exposure <= 0.0
        || !posterior.motion_pixels_per_second.is_finite()
        || posterior.motion_pixels_per_second < 0.0
        || !posterior.elapsed_ms.is_finite()
        || posterior.elapsed_ms < 0.0
        || !posterior.thermal_load.is_finite()
        || posterior.thermal_load < 0.0
    {
        anyhow::bail!("Adaptive capture posterior contains invalid runtime state");
    }
    for (index, candidate) in candidates.iter().enumerate() {
        if !candidate.shutter_seconds.is_finite()
            || candidate.shutter_seconds <= 0.0
            || candidate.iso == 0
            || !candidate.focus_diopters.is_finite()
            || candidate.focus_diopters <= 0.0
            || !candidate.readout_ms.is_finite()
            || candidate.readout_ms < 0.0
            || !candidate.settle_ms.is_finite()
            || candidate.settle_ms < 0.0
        {
            anyhow::bail!("Adaptive capture candidate {index} is invalid");
        }
    }
    for (index, probe) in posterior.radiance.iter().enumerate() {
        if !probe.mean.is_finite()
            || probe.mean < 0.0
            || !probe.variance.is_finite()
            || probe.variance < 0.0
            || !probe.weight.is_finite()
            || probe.weight < 0.0
            || probe.cfa_site > 3
        {
            anyhow::bail!("Radiance posterior probe {index} is invalid");
        }
    }
    for (index, probe) in posterior.focus.iter().enumerate() {
        if !probe.mean_diopters.is_finite()
            || probe.mean_diopters <= 0.0
            || !probe.variance_diopters2.is_finite()
            || probe.variance_diopters2 < 0.0
            || !probe.weight.is_finite()
            || probe.weight < 0.0
        {
            anyhow::bail!("Focus posterior probe {index} is invalid");
        }
    }
    sensor_profile
        .iso_models
        .first()
        .context("Adaptive capture requires a nonempty sensor calibration")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensor_noise::{IsoNoiseModel, SensorNoiseModel, SENSOR_NOISE_PROFILE_SCHEMA};

    fn profile() -> SensorNoiseProfile {
        SensorNoiseProfile {
            schema: SENSOR_NOISE_PROFILE_SCHEMA.to_string(),
            camera_make: "Nikon".to_string(),
            camera_model: "Z9".to_string(),
            bits_per_sample: 14,
            calibration_id: "sha256:planner-test".to_string(),
            iso_models: [64u32, 100, 400]
                .into_iter()
                .map(|iso| IsoNoiseModel {
                    iso,
                    model: SensorNoiseModel {
                        read_noise_dn: [2.0; 4],
                        electrons_per_dn: [0.8; 4],
                        black_drift_dn: [0.25; 4],
                        saturation_margin_dn: 16.0,
                        calibrated: true,
                    },
                })
                .collect(),
        }
    }

    fn candidate(shutter_seconds: f32, iso: u32, focus_diopters: f32) -> CaptureCandidate {
        CaptureCandidate {
            shutter_seconds,
            iso,
            focus_diopters,
            readout_ms: 20.0,
            settle_ms: 5.0,
        }
    }

    fn posterior() -> CapturePosterior {
        CapturePosterior {
            radiance: vec![RadianceProbe {
                mean: 0.2,
                variance: 0.2,
                weight: 1.0,
                cfa_site: 1,
            }],
            focus: vec![FocusProbe {
                mean_diopters: 2.0,
                variance_diopters2: 0.2,
                weight: 1.0,
            }],
            radiance_anchor_exposure: 0.01 / 64.0,
            current_focus_diopters: 1.0,
            motion_pixels_per_second: 0.0,
            elapsed_ms: 0.0,
            thermal_load: 0.0,
        }
    }

    #[test]
    fn selects_information_per_time_and_is_order_independent() {
        let candidates = [
            candidate(0.02, 100, 1.0),
            candidate(0.01, 100, 2.0),
            candidate(0.04, 100, 3.0),
        ];
        let first =
            plan_next_capture(&posterior(), &candidates, &profile(), Default::default()).unwrap();
        let reversed: Vec<_> = candidates.into_iter().rev().collect();
        let second =
            plan_next_capture(&posterior(), &reversed, &profile(), Default::default()).unwrap();
        assert_eq!(
            first.selected.unwrap().candidate,
            second.selected.unwrap().candidate
        );
    }

    #[test]
    fn motion_rejects_long_shutters_before_scoring() {
        let mut state = posterior();
        state.motion_pixels_per_second = 100.0;
        let decision = plan_next_capture(
            &state,
            &[candidate(0.02, 100, 2.0), candidate(0.002, 400, 2.0)],
            &profile(),
            AdaptivePlannerConfig {
                maximum_motion_blur_px: 0.5,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(decision.rejected_motion, 1);
        assert_eq!(decision.selected.unwrap().candidate.iso, 400);
    }

    #[test]
    fn hdr_and_focus_stop_independently() {
        let mut state = posterior();
        state.radiance[0].variance = 0.0;
        let decision = plan_next_capture(
            &state,
            &[candidate(0.01, 100, 2.0)],
            &profile(),
            Default::default(),
        )
        .unwrap();
        assert!(decision.stop_hdr);
        assert!(!decision.stop_focus);
        assert!(decision.selected.is_some());
    }

    #[test]
    fn uncalibrated_iso_candidates_are_not_silently_interpolated() {
        let decision = plan_next_capture(
            &posterior(),
            &[candidate(0.01, 200, 2.0)],
            &profile(),
            Default::default(),
        )
        .unwrap();
        assert_eq!(decision.rejected_calibration, 1);
        assert!(decision.selected.is_none());
        assert!(decision.stop_hdr);
        assert!(decision.stop_focus);
    }
}
