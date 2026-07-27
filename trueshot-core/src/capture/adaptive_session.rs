//! Stateful measured-RAW orchestration for adaptive HDR and focus capture.
//!
//! This module does not control a camera. It provides the fail-closed state
//! machine used by local camera adapters: plan one measurement, verify and
//! assimilate its completed RAW, update measured runtime state, then plan the
//! next measurement.

use super::{
    plan_next_capture, verify_observation_candidate, AdaptiveCaptureProvenance,
    AdaptiveCaptureTermination, AdaptivePlannerConfig, CaptureCandidate, CapturePlanDecision,
    CapturePosterior, FocusProbe, RawAssimilationReport, RawCaptureObservation,
    RawPosteriorAccumulator,
};
use crate::sensor_noise::SensorNoiseProfile;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CaptureRuntimeTelemetry {
    /// Measured wall time from initiating this camera action through durable
    /// local publication of the completed RAW.
    pub capture_elapsed_ms: f32,
    pub motion_pixels_per_second: f32,
    pub thermal_load: f32,
}

impl CaptureRuntimeTelemetry {
    fn validate(self) -> Result<Self> {
        if !self.capture_elapsed_ms.is_finite()
            || self.capture_elapsed_ms < 0.0
            || !self.motion_pixels_per_second.is_finite()
            || self.motion_pixels_per_second < 0.0
            || !self.thermal_load.is_finite()
            || self.thermal_load < 0.0
        {
            anyhow::bail!("Adaptive capture runtime telemetry is invalid");
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdaptiveSessionStatus {
    pub decision: CapturePlanDecision,
    pub posterior: CapturePosterior,
    pub retained_frame_count: u32,
    pub termination: Option<AdaptiveCaptureTermination>,
}

pub struct MeasuredAdaptiveSession {
    sensor_profile: SensorNoiseProfile,
    candidates: Vec<CaptureCandidate>,
    planner_config: AdaptivePlannerConfig,
    posterior: CapturePosterior,
    accumulator: RawPosteriorAccumulator,
    provenance: AdaptiveCaptureProvenance,
    decision: CapturePlanDecision,
    retained_frame_count: u32,
}

impl MeasuredAdaptiveSession {
    /// Start from one already-retained reference RAW observation. The
    /// reference is frame zero but is not attributed to a planner decision.
    pub fn start(
        reference: RawCaptureObservation,
        sensor_profile: SensorNoiseProfile,
        candidates: Vec<CaptureCandidate>,
        mut planner_config: AdaptivePlannerConfig,
    ) -> Result<Self> {
        validate_profile_observation(&sensor_profile, &reference)?;
        let current_focus = reference.focus_diopters.unwrap_or(0.0);
        planner_config.aperture = reference
            .aperture
            .context("Adaptive reference RAW has no physical aperture")?;
        planner_config.sensor_range_dn = reference.sensor_range_dn;
        let mut posterior = CapturePosterior {
            radiance: Vec::new(),
            focus: Vec::new(),
            radiance_anchor_exposure: reference.radiance_anchor_exposure,
            current_focus_diopters: current_focus,
            motion_pixels_per_second: 0.0,
            elapsed_ms: 0.0,
            thermal_load: 0.0,
        };
        let mut accumulator = RawPosteriorAccumulator::new(reference.radiance_anchor_exposure)?;
        accumulator.assimilate(&mut posterior, &reference)?;
        seed_focus_prior(&mut posterior, &reference, &candidates);
        let provenance = AdaptiveCaptureProvenance::new(&sensor_profile)?;
        let decision = plan_next_capture(&posterior, &candidates, &sensor_profile, planner_config)?;
        let mut session = Self {
            sensor_profile,
            candidates,
            planner_config,
            posterior,
            accumulator,
            provenance,
            decision,
            retained_frame_count: 1,
        };
        session.finish_if_stopped()?;
        Ok(session)
    }

    pub fn status(&self) -> AdaptiveSessionStatus {
        AdaptiveSessionStatus {
            decision: self.decision.clone(),
            posterior: self.posterior.clone(),
            retained_frame_count: self.retained_frame_count,
            termination: self.provenance.termination,
        }
    }

    pub fn provenance(&self) -> &AdaptiveCaptureProvenance {
        &self.provenance
    }

    pub fn next_candidate(&self) -> Option<CaptureCandidate> {
        self.decision.selected.map(|utility| utility.candidate)
    }

    pub fn is_complete(&self) -> bool {
        self.provenance.termination.is_some()
    }

    /// Assimilate the completed RAW selected by the current decision.
    pub fn assimilate_selected(
        &mut self,
        observation: RawCaptureObservation,
        telemetry: CaptureRuntimeTelemetry,
    ) -> Result<RawAssimilationReport> {
        if self.is_complete() {
            anyhow::bail!("Adaptive capture session is already complete");
        }
        let candidate = self
            .next_candidate()
            .context("Adaptive capture has no pending measurement")?;
        validate_profile_observation(&self.sensor_profile, &observation)?;
        verify_observation_candidate(&observation, candidate)?;
        let telemetry = telemetry.validate()?;

        let previous_posterior = self.posterior.clone();
        let previous_decision = self.decision.clone();
        let mut next_posterior = self.posterior.clone();
        let mut next_accumulator = self.accumulator.clone();
        let report = next_accumulator.assimilate(&mut next_posterior, &observation)?;
        next_posterior.elapsed_ms += telemetry.capture_elapsed_ms;
        if !next_posterior.elapsed_ms.is_finite() {
            anyhow::bail!("Adaptive capture elapsed time overflowed");
        }
        next_posterior.motion_pixels_per_second = telemetry.motion_pixels_per_second;
        next_posterior.thermal_load = telemetry.thermal_load;
        let mut next_config = self.planner_config;
        next_config.remaining_time_ms =
            (next_config.remaining_time_ms - telemetry.capture_elapsed_ms).max(f32::MIN_POSITIVE);

        let frame_index = self.retained_frame_count;
        let next_frame_count = self
            .retained_frame_count
            .checked_add(1)
            .context("Adaptive capture frame count overflow")?;
        let mut next_provenance = self.provenance.clone();
        next_provenance.record(previous_posterior, previous_decision, Some(frame_index))?;
        let next_decision = plan_next_capture(
            &next_posterior,
            &self.candidates,
            &self.sensor_profile,
            next_config,
        )?;
        finish_stopped_decision(
            &mut next_provenance,
            &next_posterior,
            &next_decision,
            next_frame_count,
        )?;

        // Commit the complete transition only after observation, planning, and
        // provenance validation all succeed.
        self.posterior = next_posterior;
        self.accumulator = next_accumulator;
        self.planner_config = next_config;
        self.provenance = next_provenance;
        self.decision = next_decision;
        self.retained_frame_count = next_frame_count;
        Ok(report)
    }

    /// Stop without claiming that the currently staged measurement executed.
    pub fn terminate(&mut self, reason: AdaptiveCaptureTermination) -> Result<()> {
        if self.is_complete() {
            anyhow::bail!("Adaptive capture session is already complete");
        }
        if !matches!(
            reason,
            AdaptiveCaptureTermination::OperatorStopped
                | AdaptiveCaptureTermination::HardwareFailure
        ) {
            anyhow::bail!("Only operator or hardware termination can interrupt a live session");
        }
        self.provenance
            .record(self.posterior.clone(), self.decision.clone(), None)?;
        self.provenance.finish(reason)?;
        self.provenance
            .validate(self.retained_frame_count as usize)?;
        Ok(())
    }

    fn finish_if_stopped(&mut self) -> Result<()> {
        finish_stopped_decision(
            &mut self.provenance,
            &self.posterior,
            &self.decision,
            self.retained_frame_count,
        )
    }
}

fn finish_stopped_decision(
    provenance: &mut AdaptiveCaptureProvenance,
    posterior: &CapturePosterior,
    decision: &CapturePlanDecision,
    retained_frame_count: u32,
) -> Result<()> {
    if decision.selected.is_some() {
        return Ok(());
    }
    provenance.record(posterior.clone(), decision.clone(), None)?;
    let termination = if decision.hdr_target_reached && decision.focus_target_reached {
        AdaptiveCaptureTermination::QualityTargetsReached
    } else if decision.rejected_budget > 0 {
        AdaptiveCaptureTermination::ResourceBudgetExhausted
    } else if decision.rejected_calibration == decision.evaluations.len() {
        AdaptiveCaptureTermination::HardwareFailure
    } else {
        AdaptiveCaptureTermination::MarginalInformationExhausted
    };
    provenance.finish(termination)?;
    provenance.validate(retained_frame_count as usize)?;
    Ok(())
}

fn validate_profile_observation(
    sensor_profile: &SensorNoiseProfile,
    observation: &RawCaptureObservation,
) -> Result<()> {
    sensor_profile.validate()?;
    if observation.sensor_calibration_id != sensor_profile.calibration_id
        || !sensor_profile.matches(
            &observation.camera_make,
            &observation.camera_model,
            observation.bits_per_sample,
        )
        || sensor_profile.model_for_iso(observation.iso).is_none()
    {
        anyhow::bail!("RAW observation does not match the session sensor calibration");
    }
    Ok(())
}

fn seed_focus_prior(
    posterior: &mut CapturePosterior,
    reference: &RawCaptureObservation,
    candidates: &[CaptureCandidate],
) {
    if !posterior.focus.is_empty() || reference.focus.is_empty() {
        return;
    }
    let mut coordinates = candidates
        .iter()
        .map(|candidate| candidate.focus_diopters)
        .collect::<Vec<_>>();
    coordinates.sort_unstable_by(f32::total_cmp);
    coordinates.dedup_by(|left, right| left.to_bits() == right.to_bits());
    if coordinates.len() < 2 {
        return;
    }
    let mean = coordinates.iter().sum::<f32>() / coordinates.len() as f32;
    let variance = coordinates
        .iter()
        .map(|coordinate| (coordinate - mean).powi(2))
        .sum::<f32>()
        / coordinates.len() as f32;
    if !variance.is_finite() || variance <= 0.0 {
        return;
    }
    posterior.focus = reference
        .focus
        .iter()
        .map(|probe| FocusProbe {
            probe_id: probe.probe_id,
            mean_diopters: mean,
            variance_diopters2: variance,
            weight: probe.weight,
        })
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{FocusResponseObservation, RadianceObservation};
    use crate::sensor_noise::{IsoNoiseModel, SensorNoiseModel, SENSOR_NOISE_PROFILE_SCHEMA};

    fn profile() -> SensorNoiseProfile {
        SensorNoiseProfile {
            schema: SENSOR_NOISE_PROFILE_SCHEMA.to_string(),
            camera_make: "Nikon".to_string(),
            camera_model: "Z9".to_string(),
            bits_per_sample: 14,
            calibration_id: "sha256:adaptive-session-test".to_string(),
            iso_models: vec![IsoNoiseModel {
                iso: 100,
                model: SensorNoiseModel {
                    read_noise_dn: [2.0; 4],
                    electrons_per_dn: [0.8; 4],
                    black_drift_dn: [0.25; 4],
                    saturation_margin_dn: 16.0,
                    calibrated: true,
                },
            }],
        }
    }

    fn observation(shutter: f32, focus: f32, variance: f32) -> RawCaptureObservation {
        RawCaptureObservation {
            camera_make: "Nikon".to_string(),
            camera_model: "Z9".to_string(),
            bits_per_sample: 14,
            sensor_calibration_id: profile().calibration_id,
            sensor_range_dn: 14_303.0,
            iso: 100,
            exposure_seconds: shutter,
            sensor_exposure: shutter / 64.0,
            focus_diopters: Some(focus),
            focal_length_mm: Some(105.0),
            aperture: Some(8.0),
            roi: [0, 0, 64, 64],
            radiance_anchor_exposure: 0.01 / 64.0,
            radiance: vec![RadianceObservation {
                probe_id: 0,
                cfa_site: 0,
                weight: 1.0,
                mean: Some(0.2),
                variance: Some(variance),
                lower_bound: None,
                valid_samples: 64,
                censored_samples: 0,
            }],
            focus: vec![FocusResponseObservation {
                probe_id: 0,
                weight: 1.0,
                score: 0.5,
                variance: 0.01,
                sample_count: 64,
            }],
        }
    }

    fn candidates() -> Vec<CaptureCandidate> {
        [1.0, 2.0]
            .into_iter()
            .map(|focus_diopters| CaptureCandidate {
                shutter_seconds: 0.01,
                iso: 100,
                focus_diopters,
                readout_ms: 20.0,
                settle_ms: 5.0,
            })
            .collect()
    }

    #[test]
    fn session_verifies_assimilates_and_retains_valid_provenance() {
        let mut session = MeasuredAdaptiveSession::start(
            observation(0.01, 1.0, 0.05),
            profile(),
            candidates(),
            AdaptivePlannerConfig {
                target_radiance_variance: 1e-6,
                target_focus_variance_diopters2: 1e-6,
                minimum_hdr_information_nats: 0.0,
                minimum_focus_information_nats: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        let selected = session.next_candidate().unwrap();
        session
            .assimilate_selected(
                observation(selected.shutter_seconds, selected.focus_diopters, 0.01),
                CaptureRuntimeTelemetry {
                    capture_elapsed_ms: 31.0,
                    motion_pixels_per_second: 0.1,
                    thermal_load: 0.02,
                },
            )
            .unwrap();
        assert_eq!(session.status().retained_frame_count, 2);
        if !session.is_complete() {
            session
                .terminate(AdaptiveCaptureTermination::OperatorStopped)
                .unwrap();
        }
        session.provenance().validate(2).unwrap();
    }

    #[test]
    fn rejected_raw_does_not_advance_session() {
        let mut session = MeasuredAdaptiveSession::start(
            observation(0.01, 1.0, 0.05),
            profile(),
            candidates(),
            AdaptivePlannerConfig {
                target_radiance_variance: 1e-6,
                target_focus_variance_diopters2: 1e-6,
                minimum_hdr_information_nats: 0.0,
                minimum_focus_information_nats: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        let before = session.status();
        let mut wrong = observation(0.02, 1.0, 0.01);
        wrong.sensor_exposure = 0.02 / 64.0;
        assert!(session
            .assimilate_selected(
                wrong,
                CaptureRuntimeTelemetry {
                    capture_elapsed_ms: 31.0,
                    motion_pixels_per_second: 0.0,
                    thermal_load: 0.0,
                },
            )
            .is_err());
        assert_eq!(session.status(), before);
    }
}
