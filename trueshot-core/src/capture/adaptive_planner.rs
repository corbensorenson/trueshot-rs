//! Deterministic information-gain planning for HDR and focus acquisition.
//!
//! The planner operates on compact posterior summaries, never image content,
//! and selects only camera-declared candidates with exact sensor calibration.

use crate::sensor_noise::SensorNoiseProfile;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const ADAPTIVE_CAPTURE_PROVENANCE_SCHEMA: &str = "trueshot.adaptive-capture.v1";
const MAX_CAPTURE_CANDIDATES: usize = 100_000;

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
    /// Stable spatial/CFA identity used for measured posterior assimilation.
    #[serde(default)]
    pub probe_id: u32,
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
    /// Stable spatial identity used for measured posterior assimilation.
    #[serde(default)]
    pub probe_id: u32,
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
#[serde(default)]
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
    /// Stop HDR once every represented radiance probe reaches this variance.
    pub target_radiance_variance: f32,
    /// Stop focus once every represented focus probe reaches this variance.
    pub target_focus_variance_diopters2: f32,
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
            target_radiance_variance: 0.001,
            target_focus_variance_diopters2: 0.001,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateRejectionReason {
    MissingExactIsoCalibration,
    TimeBudget,
    ThermalBudget,
    MotionBlur,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CandidateEvaluation {
    Eligible {
        utility: CandidateUtility,
    },
    Rejected {
        candidate: CaptureCandidate,
        reason: CandidateRejectionReason,
        predicted_cost_ms: Option<f32>,
    },
}

impl CandidateEvaluation {
    pub fn candidate(self) -> CaptureCandidate {
        match self {
            Self::Eligible { utility } => utility.candidate,
            Self::Rejected { candidate, .. } => candidate,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturePlanDecision {
    pub selected: Option<CandidateUtility>,
    pub stop_hdr: bool,
    pub stop_focus: bool,
    pub hdr_target_reached: bool,
    pub focus_target_reached: bool,
    pub rejected_motion: usize,
    pub rejected_budget: usize,
    pub rejected_calibration: usize,
    /// Canonically ordered record for every supplied candidate.
    pub evaluations: Vec<CandidateEvaluation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdaptiveCaptureIteration {
    pub iteration: u32,
    pub posterior: CapturePosterior,
    pub decision: CapturePlanDecision,
    /// Populated only after the selected measurement is retained in the group.
    pub executed_frame_index: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdaptiveCaptureProvenance {
    pub schema: String,
    pub sensor_calibration_id: String,
    pub iterations: Vec<AdaptiveCaptureIteration>,
    pub stop_hdr: bool,
    pub stop_focus: bool,
    pub termination: Option<AdaptiveCaptureTermination>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdaptiveCaptureTermination {
    QualityTargetsReached,
    MarginalInformationExhausted,
    ResourceBudgetExhausted,
    OperatorStopped,
    HardwareFailure,
}

impl AdaptiveCaptureProvenance {
    pub fn new(sensor_profile: &SensorNoiseProfile) -> Result<Self> {
        sensor_profile.validate()?;
        Ok(Self {
            schema: ADAPTIVE_CAPTURE_PROVENANCE_SCHEMA.to_string(),
            sensor_calibration_id: sensor_profile.calibration_id.clone(),
            iterations: Vec::new(),
            stop_hdr: false,
            stop_focus: false,
            termination: None,
        })
    }

    pub fn record(
        &mut self,
        posterior: CapturePosterior,
        decision: CapturePlanDecision,
        executed_frame_index: Option<u32>,
    ) -> Result<()> {
        if self.termination.is_some() || (self.stop_hdr && self.stop_focus) {
            anyhow::bail!("Adaptive capture provenance cannot accept another decision");
        }
        if decision.selected.is_none() && executed_frame_index.is_some() {
            anyhow::bail!("A stopped planner decision cannot execute a frame");
        }
        let iteration =
            u32::try_from(self.iterations.len()).context("Adaptive capture trace is too long")?;
        self.stop_hdr = decision.stop_hdr;
        self.stop_focus = decision.stop_focus;
        self.iterations.push(AdaptiveCaptureIteration {
            iteration,
            posterior,
            decision,
            executed_frame_index,
        });
        Ok(())
    }

    pub fn finish(&mut self, termination: AdaptiveCaptureTermination) -> Result<()> {
        if self.termination.is_some() {
            anyhow::bail!("Adaptive capture provenance is already terminated");
        }
        let final_decision = &self
            .iterations
            .last()
            .context("Cannot finish adaptive capture before its first decision")?
            .decision;
        match termination {
            AdaptiveCaptureTermination::QualityTargetsReached
                if !(final_decision.hdr_target_reached && final_decision.focus_target_reached) =>
            {
                anyhow::bail!("Adaptive capture did not reach both declared quality targets")
            }
            AdaptiveCaptureTermination::MarginalInformationExhausted
                if !(final_decision.stop_hdr && final_decision.stop_focus) =>
            {
                anyhow::bail!("Adaptive capture still has an active information objective")
            }
            AdaptiveCaptureTermination::ResourceBudgetExhausted
                if final_decision.selected.is_some() || final_decision.rejected_budget == 0 =>
            {
                anyhow::bail!("Adaptive capture has no evidence of budget exhaustion")
            }
            _ => {}
        }
        self.termination = Some(termination);
        Ok(())
    }

    pub fn validate(&self, frame_count: usize) -> Result<()> {
        self.validate_partial(frame_count)?;
        if self.termination.is_none() {
            anyhow::bail!("Adaptive capture provenance has no termination reason");
        }
        let mut verified = self.clone();
        let termination = verified
            .termination
            .take()
            .context("Adaptive capture termination disappeared")?;
        verified.finish(termination)?;
        Ok(())
    }

    /// Validate either an active or completed trace. Active traces may be
    /// empty before the first planned measurement executes, but every recorded
    /// active iteration must have a retained frame.
    pub fn validate_partial(&self, frame_count: usize) -> Result<()> {
        if self.schema != ADAPTIVE_CAPTURE_PROVENANCE_SCHEMA
            || self.sensor_calibration_id.trim().is_empty()
        {
            anyhow::bail!("Adaptive capture provenance identity is invalid");
        }
        if self.iterations.is_empty() {
            if self.termination.is_some() || self.stop_hdr || self.stop_focus {
                anyhow::bail!("Empty adaptive capture provenance has invalid stopping state");
            }
            return Ok(());
        }
        let mut executed = std::collections::BTreeSet::new();
        for (expected, iteration) in self.iterations.iter().enumerate() {
            if iteration.iteration as usize != expected {
                anyhow::bail!("Adaptive capture iteration ordering is invalid");
            }
            validate_posterior(&iteration.posterior)?;
            validate_decision(&iteration.decision)?;
            if iteration.decision.selected.is_none() && iteration.executed_frame_index.is_some() {
                anyhow::bail!("Stopped adaptive iteration records an executed frame");
            }
            if let Some(index) = iteration.executed_frame_index {
                if index as usize >= frame_count || !executed.insert(index) {
                    anyhow::bail!("Adaptive capture frame index is invalid or duplicated");
                }
            }
        }
        let final_decision = &self
            .iterations
            .last()
            .context("Adaptive capture provenance has no final decision")?
            .decision;
        if self.stop_hdr != final_decision.stop_hdr || self.stop_focus != final_decision.stop_focus
        {
            anyhow::bail!("Adaptive capture final stopping state is inconsistent");
        }
        if self.termination.is_none() {
            if self.stop_hdr && self.stop_focus {
                anyhow::bail!("Active adaptive capture trace has already stopped both objectives");
            }
            if self.iterations.iter().any(|iteration| {
                iteration.decision.selected.is_none() || iteration.executed_frame_index.is_none()
            }) {
                anyhow::bail!("Active adaptive capture trace contains an unexecuted decision");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateBuildReport {
    pub candidates: Vec<CaptureCandidate>,
    pub rejected_shutter_options: Vec<String>,
    pub rejected_iso_options: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraOptionSelection {
    pub shutter_speed: String,
    pub iso: String,
}

/// Resolve a numeric planner candidate back to the exact strings declared by
/// the camera. This is deliberately not a formatter: camera adapters require
/// byte-for-byte option values for verified setting readback.
pub fn resolve_camera_candidate_options(
    shutter_options: &[String],
    iso_options: &[String],
    candidate: CaptureCandidate,
) -> Result<CameraOptionSelection> {
    validate_candidate(candidate)?;
    let shutter_speed = shutter_options
        .iter()
        .filter(|option| {
            parse_shutter_seconds(option)
                .is_some_and(|value| value.to_bits() == candidate.shutter_seconds.to_bits())
        })
        .min()
        .cloned()
        .context("Selected shutter is no longer declared by the camera")?;
    let iso = iso_options
        .iter()
        .filter(|option| parse_iso(option) == Some(candidate.iso))
        .min()
        .cloned()
        .context("Selected ISO is no longer declared by the camera")?;
    Ok(CameraOptionSelection { shutter_speed, iso })
}

/// Convert camera-declared option strings into a bounded, canonical candidate
/// set. Invalid/automatic modes remain visible in the report and are never
/// guessed into numeric values.
pub fn build_camera_candidates(
    shutter_options: &[String],
    iso_options: &[String],
    focus_diopters: &[f32],
    readout_ms: f32,
    settle_ms: f32,
) -> Result<CandidateBuildReport> {
    if !readout_ms.is_finite() || readout_ms < 0.0 || !settle_ms.is_finite() || settle_ms < 0.0 {
        anyhow::bail!("Camera candidate latency must be finite and nonnegative");
    }
    if focus_diopters.is_empty()
        || focus_diopters
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
    {
        anyhow::bail!("Camera candidate focus coordinates are invalid");
    }

    let mut shutters = Vec::new();
    let mut rejected_shutter_options = Vec::new();
    for option in shutter_options {
        if let Some(value) = parse_shutter_seconds(option) {
            shutters.push(value);
        } else {
            rejected_shutter_options.push(option.clone());
        }
    }
    shutters.sort_unstable_by(f32::total_cmp);
    shutters.dedup_by(|left, right| left.to_bits() == right.to_bits());

    let mut isos = Vec::new();
    let mut rejected_iso_options = Vec::new();
    for option in iso_options {
        if let Some(value) = parse_iso(option) {
            isos.push(value);
        } else {
            rejected_iso_options.push(option.clone());
        }
    }
    isos.sort_unstable();
    isos.dedup();

    if shutters.is_empty() || isos.is_empty() {
        anyhow::bail!("Camera declared no numeric shutter/ISO candidate combination");
    }
    let mut focus = focus_diopters.to_vec();
    focus.sort_unstable_by(f32::total_cmp);
    focus.dedup_by(|left, right| left.to_bits() == right.to_bits());
    let candidate_count = shutters
        .len()
        .checked_mul(isos.len())
        .and_then(|count| count.checked_mul(focus.len()))
        .context("Camera candidate count overflow")?;
    if candidate_count > MAX_CAPTURE_CANDIDATES {
        anyhow::bail!(
            "Camera candidate grid has {candidate_count} entries; limit is {MAX_CAPTURE_CANDIDATES}"
        );
    }
    let mut candidates = Vec::with_capacity(candidate_count);
    for shutter_seconds in shutters {
        for &iso in &isos {
            for &focus_diopters in &focus {
                candidates.push(CaptureCandidate {
                    shutter_seconds,
                    iso,
                    focus_diopters,
                    readout_ms,
                    settle_ms,
                });
            }
        }
    }
    candidates.sort_unstable_by(compare_candidate);
    Ok(CandidateBuildReport {
        candidates,
        rejected_shutter_options,
        rejected_iso_options,
    })
}

pub fn plan_next_capture(
    posterior: &CapturePosterior,
    candidates: &[CaptureCandidate],
    sensor_profile: &SensorNoiseProfile,
    config: AdaptivePlannerConfig,
) -> Result<CapturePlanDecision> {
    validate_inputs(posterior, candidates, sensor_profile, config)?;
    let mut utilities = Vec::with_capacity(candidates.len());
    let mut evaluations = Vec::with_capacity(candidates.len());
    let mut rejected_motion = 0usize;
    let mut rejected_budget = 0usize;
    let mut rejected_calibration = 0usize;

    for &candidate in candidates {
        let Some(noise) = sensor_profile.model_for_iso(candidate.iso) else {
            rejected_calibration += 1;
            evaluations.push(CandidateEvaluation::Rejected {
                candidate,
                reason: CandidateRejectionReason::MissingExactIsoCalibration,
                predicted_cost_ms: None,
            });
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
        {
            rejected_budget += 1;
            evaluations.push(CandidateEvaluation::Rejected {
                candidate,
                reason: CandidateRejectionReason::TimeBudget,
                predicted_cost_ms: finite_value(capture_cost_ms),
            });
            continue;
        }
        if thermal_after > config.maximum_thermal_load {
            rejected_budget += 1;
            evaluations.push(CandidateEvaluation::Rejected {
                candidate,
                reason: CandidateRejectionReason::ThermalBudget,
                predicted_cost_ms: finite_value(capture_cost_ms),
            });
            continue;
        }
        if candidate.shutter_seconds * posterior.motion_pixels_per_second
            > config.maximum_motion_blur_px
        {
            rejected_motion += 1;
            evaluations.push(CandidateEvaluation::Rejected {
                candidate,
                reason: CandidateRejectionReason::MotionBlur,
                predicted_cost_ms: finite_value(capture_cost_ms),
            });
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
        let utility = CandidateUtility {
            candidate,
            hdr_information_nats,
            focus_information_nats,
            capture_cost_ms,
            utility_per_ms: (hdr_information_nats + focus_information_nats)
                / capture_cost_ms.max(1e-3),
        };
        utilities.push(utility);
        evaluations.push(CandidateEvaluation::Eligible { utility });
    }

    let maximum_hdr = utilities
        .iter()
        .map(|utility| utility.hdr_information_nats)
        .fold(0.0f32, f32::max);
    let maximum_focus = utilities
        .iter()
        .map(|utility| utility.focus_information_nats)
        .fold(0.0f32, f32::max);
    let hdr_target_reached = posterior.radiance.is_empty()
        || posterior
            .radiance
            .iter()
            .filter(|probe| probe.weight > 0.0)
            .all(|probe| probe.variance <= config.target_radiance_variance);
    let focus_target_reached = posterior.focus.is_empty()
        || posterior
            .focus
            .iter()
            .filter(|probe| probe.weight > 0.0)
            .all(|probe| probe.variance_diopters2 <= config.target_focus_variance_diopters2);
    let stop_hdr = hdr_target_reached || maximum_hdr < config.minimum_hdr_information_nats;
    let stop_focus = focus_target_reached || maximum_focus < config.minimum_focus_information_nats;
    let selected = utilities
        .into_iter()
        .filter(|utility| {
            (!stop_hdr && utility.hdr_information_nats >= config.minimum_hdr_information_nats)
                || (!stop_focus
                    && utility.focus_information_nats >= config.minimum_focus_information_nats)
        })
        .max_by(compare_utility);
    evaluations
        .sort_unstable_by(|left, right| compare_candidate(&left.candidate(), &right.candidate()));

    let decision = CapturePlanDecision {
        selected,
        stop_hdr,
        stop_focus,
        hdr_target_reached,
        focus_target_reached,
        rejected_motion,
        rejected_budget,
        rejected_calibration,
        evaluations,
    };
    validate_decision(&decision)?;
    Ok(decision)
}

fn gaussian_information_gain(prior_variance: f32, measurement_variance: f32) -> f32 {
    if prior_variance <= 0.0 || measurement_variance <= 0.0 {
        return 0.0;
    }
    0.5 * (1.0 + prior_variance / measurement_variance).ln()
}

fn parse_shutter_seconds(value: &str) -> Option<f32> {
    let normalized = value.trim().trim_end_matches(['s', 'S', '"']).trim();
    if normalized.is_empty()
        || normalized.eq_ignore_ascii_case("auto")
        || normalized.eq_ignore_ascii_case("bulb")
        || normalized.eq_ignore_ascii_case("time")
    {
        return None;
    }
    let seconds = if let Some((numerator, denominator)) = normalized.split_once('/') {
        let numerator = numerator.trim().parse::<f32>().ok()?;
        let denominator = denominator.trim().parse::<f32>().ok()?;
        (denominator > 0.0).then_some(numerator / denominator)?
    } else {
        normalized.parse::<f32>().ok()?
    };
    (seconds.is_finite() && seconds > 0.0).then_some(seconds)
}

fn parse_iso(value: &str) -> Option<u32> {
    let normalized = value.trim();
    if normalized.eq_ignore_ascii_case("auto") {
        return None;
    }
    let numeric = normalized
        .strip_prefix("ISO")
        .or_else(|| normalized.strip_prefix("iso"))
        .unwrap_or(normalized)
        .trim();
    numeric.parse::<u32>().ok().filter(|parsed| *parsed > 0)
}

fn compare_candidate(left: &CaptureCandidate, right: &CaptureCandidate) -> std::cmp::Ordering {
    left.shutter_seconds
        .total_cmp(&right.shutter_seconds)
        .then_with(|| left.iso.cmp(&right.iso))
        .then_with(|| left.focus_diopters.total_cmp(&right.focus_diopters))
        .then_with(|| left.readout_ms.total_cmp(&right.readout_ms))
        .then_with(|| left.settle_ms.total_cmp(&right.settle_ms))
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

fn validate_decision(decision: &CapturePlanDecision) -> Result<()> {
    if decision.evaluations.is_empty() {
        anyhow::bail!("Adaptive capture decision has no candidate evaluations");
    }
    let motion = decision
        .evaluations
        .iter()
        .filter(|evaluation| {
            matches!(
                evaluation,
                CandidateEvaluation::Rejected {
                    reason: CandidateRejectionReason::MotionBlur,
                    ..
                }
            )
        })
        .count();
    let budget = decision
        .evaluations
        .iter()
        .filter(|evaluation| {
            matches!(
                evaluation,
                CandidateEvaluation::Rejected {
                    reason: CandidateRejectionReason::TimeBudget
                        | CandidateRejectionReason::ThermalBudget,
                    ..
                }
            )
        })
        .count();
    let calibration = decision
        .evaluations
        .iter()
        .filter(|evaluation| {
            matches!(
                evaluation,
                CandidateEvaluation::Rejected {
                    reason: CandidateRejectionReason::MissingExactIsoCalibration,
                    ..
                }
            )
        })
        .count();
    if motion != decision.rejected_motion
        || budget != decision.rejected_budget
        || calibration != decision.rejected_calibration
    {
        anyhow::bail!("Adaptive capture rejection counters do not match evaluations");
    }
    for evaluation in &decision.evaluations {
        validate_candidate(evaluation.candidate())?;
        match evaluation {
            CandidateEvaluation::Eligible { utility } => {
                if !utility.hdr_information_nats.is_finite()
                    || utility.hdr_information_nats < 0.0
                    || !utility.focus_information_nats.is_finite()
                    || utility.focus_information_nats < 0.0
                    || !utility.capture_cost_ms.is_finite()
                    || utility.capture_cost_ms <= 0.0
                    || !utility.utility_per_ms.is_finite()
                    || utility.utility_per_ms < 0.0
                {
                    anyhow::bail!("Adaptive capture utility is invalid");
                }
            }
            CandidateEvaluation::Rejected {
                predicted_cost_ms, ..
            } => {
                if predicted_cost_ms.is_some_and(|cost| !cost.is_finite() || cost < 0.0) {
                    anyhow::bail!("Adaptive capture rejected cost is invalid");
                }
            }
        }
    }
    if decision
        .evaluations
        .windows(2)
        .any(|pair| !compare_candidate(&pair[0].candidate(), &pair[1].candidate()).is_lt())
    {
        anyhow::bail!("Adaptive capture evaluations are not unique and canonically ordered");
    }
    if (decision.hdr_target_reached && !decision.stop_hdr)
        || (decision.focus_target_reached && !decision.stop_focus)
    {
        anyhow::bail!("Adaptive capture quality target did not stop its objective");
    }
    if let Some(selected) = decision.selected {
        if !decision.evaluations.iter().any(|evaluation| {
            matches!(
                evaluation,
                CandidateEvaluation::Eligible { utility } if *utility == selected
            )
        }) {
            anyhow::bail!("Adaptive capture selection is absent from candidate evaluations");
        }
    } else if !decision.stop_hdr || !decision.stop_focus {
        anyhow::bail!("Adaptive planner returned no action before both objectives stopped");
    }
    Ok(())
}

fn finite_value(value: f32) -> Option<f32> {
    value.is_finite().then_some(value)
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
        config.target_radiance_variance,
        config.target_focus_variance_diopters2,
        config.minimum_hdr_information_nats,
        config.minimum_focus_information_nats,
    ];
    if nonnegative
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        anyhow::bail!("Adaptive capture configuration contains invalid nonnegative bounds");
    }
    validate_posterior(posterior)?;
    for (index, candidate) in candidates.iter().enumerate() {
        validate_candidate(*candidate)
            .with_context(|| format!("Adaptive capture candidate {index} is invalid"))?;
    }
    let mut canonical = candidates.to_vec();
    canonical.sort_unstable_by(compare_candidate);
    if canonical
        .windows(2)
        .any(|pair| compare_candidate(&pair[0], &pair[1]).is_eq())
    {
        anyhow::bail!("Adaptive capture candidates must be unique");
    }
    sensor_profile
        .iso_models
        .first()
        .context("Adaptive capture requires a nonempty sensor calibration")?;
    Ok(())
}

fn validate_posterior(posterior: &CapturePosterior) -> Result<()> {
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
            || probe.mean_diopters < 0.0
            || !probe.variance_diopters2.is_finite()
            || probe.variance_diopters2 < 0.0
            || !probe.weight.is_finite()
            || probe.weight < 0.0
        {
            anyhow::bail!("Focus posterior probe {index} is invalid");
        }
    }
    Ok(())
}

fn validate_candidate(candidate: CaptureCandidate) -> Result<()> {
    if !candidate.shutter_seconds.is_finite()
        || candidate.shutter_seconds <= 0.0
        || candidate.iso == 0
        || !candidate.focus_diopters.is_finite()
        || candidate.focus_diopters < 0.0
        || !candidate.readout_ms.is_finite()
        || candidate.readout_ms < 0.0
        || !candidate.settle_ms.is_finite()
        || candidate.settle_ms < 0.0
    {
        anyhow::bail!("Capture candidate contains invalid settings or latency");
    }
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
                probe_id: 0,
                mean: 0.2,
                variance: 0.2,
                weight: 1.0,
                cfa_site: 1,
            }],
            focus: vec![FocusProbe {
                probe_id: 0,
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

    #[test]
    fn camera_options_build_a_canonical_auditable_grid() {
        let report = build_camera_candidates(
            &[
                "1/125".to_string(),
                "0.5s".to_string(),
                "Bulb".to_string(),
                "1/125".to_string(),
            ],
            &["ISO 100".to_string(), "400".to_string(), "Auto".to_string()],
            &[2.0, 0.0, 2.0],
            20.0,
            5.0,
        )
        .unwrap();
        assert_eq!(report.candidates.len(), 8);
        assert_eq!(report.rejected_shutter_options, ["Bulb"]);
        assert_eq!(report.rejected_iso_options, ["Auto"]);
        assert!(report
            .candidates
            .iter()
            .any(|candidate| candidate.focus_diopters == 0.0));
        assert!(report
            .candidates
            .windows(2)
            .all(|pair| !compare_candidate(&pair[0], &pair[1]).is_gt()));
    }

    #[test]
    fn selected_candidate_round_trips_to_declared_camera_strings() {
        let selected = resolve_camera_candidate_options(
            &["Auto".to_string(), "0.01".to_string(), "1/100".to_string()],
            &["Auto".to_string(), "ISO 100".to_string(), "100".to_string()],
            candidate(0.01, 100, 1.0),
        )
        .unwrap();
        assert_eq!(selected.shutter_speed, "0.01");
        assert_eq!(selected.iso, "100");
    }

    #[test]
    fn selected_candidate_fails_if_capabilities_changed() {
        assert!(resolve_camera_candidate_options(
            &["1/50".to_string()],
            &["100".to_string()],
            candidate(0.01, 100, 1.0),
        )
        .is_err());
    }

    #[test]
    fn infinity_focus_is_valid_in_candidates_and_posteriors() {
        let mut state = posterior();
        state.current_focus_diopters = 0.0;
        state.focus[0].mean_diopters = 0.0;
        let decision = plan_next_capture(
            &state,
            &[candidate(0.01, 100, 0.0)],
            &profile(),
            Default::default(),
        )
        .unwrap();
        assert_eq!(decision.selected.unwrap().candidate.focus_diopters, 0.0);
    }

    #[test]
    fn every_candidate_has_a_canonical_reason_or_utility() {
        let mut state = posterior();
        state.motion_pixels_per_second = 100.0;
        let decision = plan_next_capture(
            &state,
            &[
                candidate(0.02, 100, 2.0),
                candidate(0.002, 400, 2.0),
                candidate(0.001, 200, 1.0),
            ],
            &profile(),
            Default::default(),
        )
        .unwrap();
        assert_eq!(decision.evaluations.len(), 3);
        assert_eq!(decision.rejected_motion, 1);
        assert_eq!(decision.rejected_calibration, 1);
        assert!(matches!(
            decision.evaluations[0],
            CandidateEvaluation::Rejected {
                reason: CandidateRejectionReason::MissingExactIsoCalibration,
                ..
            }
        ));
        assert!(matches!(
            decision.evaluations[1],
            CandidateEvaluation::Eligible { .. }
        ));
        assert!(matches!(
            decision.evaluations[2],
            CandidateEvaluation::Rejected {
                reason: CandidateRejectionReason::MotionBlur,
                ..
            }
        ));
    }

    fn assimilate(
        posterior: &mut CapturePosterior,
        utility: CandidateUtility,
        config: &mut AdaptivePlannerConfig,
    ) {
        for probe in &mut posterior.radiance {
            probe.variance *= (-2.0 * utility.hdr_information_nats).exp();
        }
        for probe in &mut posterior.focus {
            probe.variance_diopters2 *= (-2.0 * utility.focus_information_nats).exp();
        }
        posterior.current_focus_diopters = utility.candidate.focus_diopters;
        posterior.elapsed_ms += utility.capture_cost_ms;
        posterior.thermal_load += utility.capture_cost_ms * 0.001 * config.thermal_load_per_second;
        config.remaining_time_ms = (config.remaining_time_ms - utility.capture_cost_ms).max(0.0);
    }

    #[test]
    fn closed_loop_reaches_quality_target_faster_than_fixed_grid() {
        let focus = [0.8, 1.2, 1.6, 2.0, 2.4, 2.8, 3.2];
        let shutters = [0.002, 0.008, 0.032];
        let candidates = shutters
            .into_iter()
            .flat_map(|shutter| {
                focus
                    .into_iter()
                    .map(move |diopters| candidate(shutter, 100, diopters))
            })
            .collect::<Vec<_>>();
        let mut state = posterior();
        state.radiance[0].variance = 0.5;
        state.focus[0].variance_diopters2 = 0.5;
        let mut config = AdaptivePlannerConfig {
            remaining_time_ms: 60_000.0,
            target_radiance_variance: 0.005,
            target_focus_variance_diopters2: 0.005,
            minimum_hdr_information_nats: 0.015,
            minimum_focus_information_nats: 0.015,
            ..Default::default()
        };
        let mut remaining = candidates.clone();
        let mut adaptive_cost = 0.0f32;
        for _ in 0..candidates.len() {
            let decision = plan_next_capture(&state, &remaining, &profile(), config).unwrap();
            let Some(selected) = decision.selected else {
                assert!(decision.stop_hdr && decision.stop_focus);
                break;
            };
            adaptive_cost += selected.capture_cost_ms;
            remaining.retain(|candidate| *candidate != selected.candidate);
            assimilate(&mut state, selected, &mut config);
        }

        let mut fixed_state = posterior();
        fixed_state.radiance[0].variance = 0.5;
        fixed_state.focus[0].variance_diopters2 = 0.5;
        let mut fixed_config = AdaptivePlannerConfig {
            remaining_time_ms: 60_000.0,
            target_radiance_variance: 0.0,
            target_focus_variance_diopters2: 0.0,
            minimum_hdr_information_nats: 0.0,
            minimum_focus_information_nats: 0.0,
            ..Default::default()
        };
        let mut fixed_cost = 0.0f32;
        for focus_diopters in focus {
            for shutter_seconds in shutters {
                let action = candidate(shutter_seconds, 100, focus_diopters);
                let decision =
                    plan_next_capture(&fixed_state, &[action], &profile(), fixed_config).unwrap();
                let selected = decision.selected.unwrap();
                fixed_cost += selected.capture_cost_ms;
                assimilate(&mut fixed_state, selected, &mut fixed_config);
            }
        }

        println!(
            "adaptive={adaptive_cost:.1}ms fixed={fixed_cost:.1}ms radiance_var={:.6} focus_var={:.6}",
            state.radiance[0].variance, state.focus[0].variance_diopters2
        );
        assert!(state.radiance[0].variance <= 0.005);
        assert!(state.focus[0].variance_diopters2 <= 0.005);
        assert!(fixed_state.radiance[0].variance <= 0.005);
        assert!(fixed_state.focus[0].variance_diopters2 <= 0.005);
        assert!(
            adaptive_cost <= fixed_cost * 0.80,
            "adaptive {adaptive_cost}ms fixed {fixed_cost}ms"
        );
    }

    #[test]
    fn provenance_validates_execution_and_final_stopping_state() {
        let sensor_profile = profile();
        let mut trace = AdaptiveCaptureProvenance::new(&sensor_profile).unwrap();
        let state = posterior();
        let decision = plan_next_capture(
            &state,
            &[candidate(0.01, 100, 2.0)],
            &sensor_profile,
            Default::default(),
        )
        .unwrap();
        trace.record(state.clone(), decision, Some(0)).unwrap();

        let mut stopped = state;
        stopped.radiance[0].variance = 0.0;
        stopped.focus[0].variance_diopters2 = 0.0;
        let decision = plan_next_capture(
            &stopped,
            &[candidate(0.01, 100, 2.0)],
            &sensor_profile,
            Default::default(),
        )
        .unwrap();
        trace.record(stopped, decision, None).unwrap();
        trace
            .finish(AdaptiveCaptureTermination::QualityTargetsReached)
            .unwrap();
        trace.validate(1).unwrap();
        assert!(trace.stop_hdr && trace.stop_focus);
        serde_json::to_string_pretty(&trace).unwrap();

        let mut unterminated = trace.clone();
        unterminated.termination = None;
        assert!(unterminated.validate(1).is_err());

        let mut duplicated = trace;
        let duplicate = duplicated.iterations[0].decision.evaluations[0];
        duplicated.iterations[0]
            .decision
            .evaluations
            .push(duplicate);
        duplicated.iterations[0]
            .decision
            .evaluations
            .sort_unstable_by(|left, right| {
                compare_candidate(&left.candidate(), &right.candidate())
            });
        assert!(duplicated.validate(1).is_err());
    }
}
