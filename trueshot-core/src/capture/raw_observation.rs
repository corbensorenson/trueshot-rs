//! Bounded, calibrated posterior observations from selectively decoded RAW ROIs.
//!
//! Radiance evidence is accumulated per spatial tile and CFA site. Saturated
//! samples remain one-sided lower bounds. Focus evidence uses same-CFA
//! two-pixel gradients, whitened by the exact sensor model, and only produces a
//! sub-plane posterior after three distinct measured diopter planes exist.

use super::{CaptureCandidate, CapturePosterior, FocusProbe, RadianceProbe};
use crate::nef::parser::{Z9Metadata, Z9NefParser};
use crate::nef::raw_data::{RawBuffer, Roi};
use crate::sensor_noise::{SensorNoiseModel, SensorNoiseProfile};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

const MAX_PROBE_TILES: usize = 4_096;
const MAX_FOCUS_PLANES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RawObservationConfig {
    pub tile_columns: u16,
    pub tile_rows: u16,
    pub maximum_samples_per_tile_site: u16,
    pub minimum_radiance_samples: u16,
    pub minimum_focus_samples: u16,
}

impl Default for RawObservationConfig {
    fn default() -> Self {
        Self {
            tile_columns: 8,
            tile_rows: 8,
            maximum_samples_per_tile_site: 256,
            minimum_radiance_samples: 8,
            minimum_focus_samples: 24,
        }
    }
}

impl RawObservationConfig {
    fn validate(self) -> Result<Self> {
        let tiles = usize::from(self.tile_columns)
            .checked_mul(usize::from(self.tile_rows))
            .context("RAW observation tile count overflow")?;
        if self.tile_columns == 0
            || self.tile_rows == 0
            || tiles > MAX_PROBE_TILES
            || self.maximum_samples_per_tile_site < self.minimum_radiance_samples
            || self.maximum_samples_per_tile_site == 0
            || self.minimum_focus_samples == 0
        {
            anyhow::bail!("RAW observation configuration is invalid");
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RadianceObservation {
    pub probe_id: u32,
    pub cfa_site: usize,
    pub weight: f32,
    pub mean: Option<f32>,
    pub variance: Option<f32>,
    pub lower_bound: Option<f32>,
    pub valid_samples: u32,
    pub censored_samples: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FocusResponseObservation {
    pub probe_id: u32,
    pub weight: f32,
    pub score: f32,
    pub variance: f32,
    pub sample_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawCaptureObservation {
    pub camera_make: String,
    pub camera_model: String,
    pub bits_per_sample: u16,
    pub sensor_calibration_id: String,
    pub iso: u32,
    pub exposure_seconds: f32,
    pub focus_diopters: Option<f32>,
    pub focal_length_mm: Option<f32>,
    pub aperture: Option<f32>,
    pub roi: [u32; 4],
    pub radiance_anchor_exposure: f32,
    pub radiance: Vec<RadianceObservation>,
    pub focus: Vec<FocusResponseObservation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RawAssimilationReport {
    pub radiance_updates: u32,
    pub censored_constraints: u32,
    pub censor_conflicts: u32,
    pub focus_updates: u32,
    pub accumulated_focus_planes: u16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct FocusPlaneEvidence {
    diopters: f32,
    score: f32,
    variance: f32,
    weight: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct ObservationIdentity {
    camera_make: String,
    camera_model: String,
    bits_per_sample: u16,
    sensor_calibration_id: String,
    focal_length_mm: Option<f32>,
    aperture: Option<f32>,
    roi: [u32; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawPosteriorAccumulator {
    pub radiance_anchor_exposure: f32,
    identity: Option<ObservationIdentity>,
    focus_planes: BTreeMap<u32, Vec<FocusPlaneEvidence>>,
}

impl RawPosteriorAccumulator {
    pub fn new(radiance_anchor_exposure: f32) -> Result<Self> {
        if !radiance_anchor_exposure.is_finite() || radiance_anchor_exposure <= 0.0 {
            anyhow::bail!("RAW posterior anchor exposure must be finite and positive");
        }
        Ok(Self {
            radiance_anchor_exposure,
            identity: None,
            focus_planes: BTreeMap::new(),
        })
    }

    pub fn assimilate(
        &mut self,
        posterior: &mut CapturePosterior,
        observation: &RawCaptureObservation,
    ) -> Result<RawAssimilationReport> {
        validate_observation(observation)?;
        self.validate_identity(observation)?;
        if relative_error(
            observation.radiance_anchor_exposure,
            self.radiance_anchor_exposure,
        ) > 1e-6
        {
            anyhow::bail!("RAW observation anchor exposure changed during capture");
        }
        if posterior.radiance.is_empty() {
            posterior.radiance_anchor_exposure = self.radiance_anchor_exposure;
        } else if relative_error(
            posterior.radiance_anchor_exposure,
            self.radiance_anchor_exposure,
        ) > 1e-6
        {
            anyhow::bail!("Capture posterior uses a different radiance anchor");
        }

        let mut report = RawAssimilationReport {
            radiance_updates: 0,
            censored_constraints: 0,
            censor_conflicts: 0,
            focus_updates: 0,
            accumulated_focus_planes: 0,
        };
        for measurement in &observation.radiance {
            assimilate_radiance_probe(posterior, *measurement, &mut report)?;
        }
        posterior
            .radiance
            .sort_unstable_by_key(|probe| probe.probe_id);

        if let Some(diopters) = observation.focus_diopters {
            posterior.current_focus_diopters = diopters;
            for evidence in &observation.focus {
                let planes = self.focus_planes.entry(evidence.probe_id).or_default();
                insert_focus_plane(
                    planes,
                    FocusPlaneEvidence {
                        diopters,
                        score: evidence.score,
                        variance: evidence.variance,
                        weight: evidence.weight,
                    },
                )?;
            }
            report.accumulated_focus_planes = self
                .focus_planes
                .values()
                .map(Vec::len)
                .max()
                .unwrap_or(0)
                .try_into()
                .unwrap_or(u16::MAX);
            for (&probe_id, planes) in &self.focus_planes {
                let Some((mean_diopters, variance_diopters2)) = fit_focus_posterior(planes) else {
                    continue;
                };
                upsert_focus_probe(
                    posterior,
                    FocusProbe {
                        probe_id,
                        mean_diopters,
                        variance_diopters2,
                        weight: planes[0].weight,
                    },
                );
                report.focus_updates += 1;
            }
            posterior.focus.sort_unstable_by_key(|probe| probe.probe_id);
        }
        Ok(report)
    }

    fn validate_identity(&mut self, observation: &RawCaptureObservation) -> Result<()> {
        let incoming = ObservationIdentity {
            camera_make: observation.camera_make.clone(),
            camera_model: observation.camera_model.clone(),
            bits_per_sample: observation.bits_per_sample,
            sensor_calibration_id: observation.sensor_calibration_id.clone(),
            focal_length_mm: observation.focal_length_mm,
            aperture: observation.aperture,
            roi: observation.roi,
        };
        let Some(identity) = &self.identity else {
            self.identity = Some(incoming);
            return Ok(());
        };
        if identity.camera_make != incoming.camera_make
            || identity.camera_model != incoming.camera_model
            || identity.bits_per_sample != incoming.bits_per_sample
            || identity.sensor_calibration_id != incoming.sensor_calibration_id
            || identity.roi != incoming.roi
            || !optional_nearly_equal(identity.focal_length_mm, incoming.focal_length_mm, 0.001)
            || !optional_nearly_equal(identity.aperture, incoming.aperture, 0.001)
        {
            anyhow::bail!(
                "RAW observation camera, calibration, ROI, or optical identity changed during capture"
            );
        }
        Ok(())
    }
}

pub fn observe_nef_roi(
    path: &Path,
    roi: Roi,
    sensor_profile: &SensorNoiseProfile,
    radiance_anchor_exposure: f32,
    config: RawObservationConfig,
) -> Result<RawCaptureObservation> {
    let mut parser = Z9NefParser::new(path);
    parser
        .parse()
        .with_context(|| format!("Parse adaptive RAW observation {}", path.display()))?;
    if !parser.supports_selective_loading() {
        anyhow::bail!(
            "{} does not support verified selective RAW observation",
            path.display()
        );
    }
    let metadata = parser.get_metadata()?.clone();
    let raw = parser
        .load_roi(&roi, None)
        .with_context(|| format!("Decode adaptive RAW ROI {}", path.display()))?;
    observe_raw_roi(
        &raw,
        (roi.x, roi.y),
        &metadata,
        sensor_profile,
        radiance_anchor_exposure,
        config,
    )
}

pub fn observe_raw_roi(
    raw: &RawBuffer,
    roi_origin: (u32, u32),
    metadata: &Z9Metadata,
    sensor_profile: &SensorNoiseProfile,
    radiance_anchor_exposure: f32,
    config: RawObservationConfig,
) -> Result<RawCaptureObservation> {
    let config = config.validate()?;
    sensor_profile.validate()?;
    if !sensor_profile.matches(
        &metadata.camera_make,
        &metadata.camera_model,
        metadata.bits_per_sample,
    ) {
        anyhow::bail!("Sensor profile does not match RAW observation camera identity");
    }
    let iso = metadata
        .iso
        .context("Adaptive RAW observation requires ISO metadata")?;
    let noise = sensor_profile
        .model_for_iso(iso)
        .with_context(|| format!("Sensor profile has no exact ISO {iso} model"))?;
    let exposure_seconds = metadata
        .exposure_time
        .context("Adaptive RAW observation requires exposure metadata")?
        as f32;
    if !exposure_seconds.is_finite()
        || exposure_seconds <= 0.0
        || !radiance_anchor_exposure.is_finite()
        || radiance_anchor_exposure <= 0.0
    {
        anyhow::bail!("Adaptive RAW observation exposure is invalid");
    }
    let levels = metadata
        .sensor_levels
        .context("Adaptive RAW observation requires verified sensor levels")?;
    if raw.width == 0
        || raw.height == 0
        || raw.data.len() != raw.width as usize * raw.height as usize
        || levels.white <= levels.black
    {
        anyhow::bail!("Adaptive RAW observation buffer or sensor levels are invalid");
    }
    let focus_diopters = metadata
        .focus_distance
        .and_then(|distance| (distance.is_finite() && distance > 0.0).then_some(1.0 / distance));
    let tile_count = usize::from(config.tile_columns) * usize::from(config.tile_rows);
    let mut radiance = Vec::with_capacity(tile_count * 4);
    let mut focus = Vec::with_capacity(tile_count);
    let total_pixels = (raw.width as f32 * raw.height as f32).max(1.0);
    let range_dn = f32::from(levels.white - levels.black);
    let exposure_ratio = exposure_seconds / radiance_anchor_exposure;

    for tile_y in 0..u32::from(config.tile_rows) {
        let y0 = raw.height * tile_y / u32::from(config.tile_rows);
        let y1 = raw.height * (tile_y + 1) / u32::from(config.tile_rows);
        for tile_x in 0..u32::from(config.tile_columns) {
            let x0 = raw.width * tile_x / u32::from(config.tile_columns);
            let x1 = raw.width * (tile_x + 1) / u32::from(config.tile_columns);
            let tile_id = tile_y * u32::from(config.tile_columns) + tile_x;
            let tile_pixels = (x1 - x0) as f32 * (y1 - y0) as f32;
            let tile_weight = tile_pixels / total_pixels;
            let sample_step =
                cfa_sample_step(x1 - x0, y1 - y0, config.maximum_samples_per_tile_site);
            for site in 0..4usize {
                radiance.push(observe_radiance_site(
                    raw,
                    roi_origin,
                    [x0, y0, x1, y1],
                    tile_id * 4 + site as u32,
                    site,
                    tile_weight * 0.25,
                    sample_step,
                    config.minimum_radiance_samples,
                    levels.black,
                    range_dn,
                    exposure_ratio,
                    noise,
                ));
            }
            if let Some(evidence) = observe_focus_tile(
                raw,
                roi_origin,
                [x0, y0, x1, y1],
                tile_id,
                tile_weight,
                sample_step,
                config.minimum_focus_samples,
                levels.black,
                levels.white,
                range_dn,
                exposure_ratio,
                noise,
            ) {
                focus.push(evidence);
            }
        }
    }

    let observation = RawCaptureObservation {
        camera_make: metadata.camera_make.clone(),
        camera_model: metadata.camera_model.clone(),
        bits_per_sample: metadata.bits_per_sample,
        sensor_calibration_id: sensor_profile.calibration_id.clone(),
        iso,
        exposure_seconds,
        focus_diopters,
        focal_length_mm: metadata.focal_length,
        aperture: metadata.aperture,
        roi: [roi_origin.0, roi_origin.1, raw.width, raw.height],
        radiance_anchor_exposure,
        radiance,
        focus,
    };
    validate_observation(&observation)?;
    Ok(observation)
}

pub fn verify_observation_candidate(
    observation: &RawCaptureObservation,
    candidate: CaptureCandidate,
) -> Result<()> {
    validate_observation(observation)?;
    if observation.iso != candidate.iso
        || relative_error(observation.exposure_seconds, candidate.shutter_seconds) > 0.01
    {
        anyhow::bail!("Captured RAW exposure/ISO does not match the selected candidate");
    }
    let actual_focus = observation
        .focus_diopters
        .context("Captured RAW has no verifiable focus distance")?;
    let tolerance = (candidate.focus_diopters.abs() * 0.01).max(0.01);
    if (actual_focus - candidate.focus_diopters).abs() > tolerance {
        anyhow::bail!(
            "Captured RAW focus is {:.4}D; selected candidate was {:.4}D",
            actual_focus,
            candidate.focus_diopters
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn observe_radiance_site(
    raw: &RawBuffer,
    origin: (u32, u32),
    bounds: [u32; 4],
    probe_id: u32,
    site: usize,
    weight: f32,
    step: u32,
    minimum_samples: u16,
    black: u16,
    range_dn: f32,
    exposure_ratio: f32,
    noise: SensorNoiseModel,
) -> RadianceObservation {
    let mut signal_sum = 0.0f64;
    let mut variance_sum = 0.0f64;
    let mut valid = 0u32;
    let mut censored = 0u32;
    let saturation = noise.saturation_signal(range_dn);
    for_each_cfa_sample(bounds, origin, site, step, |x, y| {
        let value = raw.data[y as usize * raw.width as usize + x as usize];
        let signal = (f32::from(value.saturating_sub(black)) / range_dn).max(0.0);
        if signal >= saturation {
            censored += 1;
        } else {
            signal_sum += f64::from(signal);
            variance_sum += f64::from(noise.normalized_variance(site, signal, range_dn));
            valid += 1;
        }
    });
    // A clipped sample only bounds its own radiance. The regional mean is a
    // valid lower bound only when every retained sample in this probe clips.
    let lower_bound = (censored > 0 && valid == 0).then_some(saturation / exposure_ratio);
    let enough = valid >= u32::from(minimum_samples);
    let mean = enough.then_some((signal_sum / f64::from(valid)) as f32 / exposure_ratio);
    let variance =
        enough.then_some((variance_sum / f64::from(valid).powi(2)) as f32 / exposure_ratio.powi(2));
    RadianceObservation {
        probe_id,
        cfa_site: site,
        weight,
        mean,
        variance,
        lower_bound,
        valid_samples: valid,
        censored_samples: censored,
    }
}

#[allow(clippy::too_many_arguments)]
fn observe_focus_tile(
    raw: &RawBuffer,
    origin: (u32, u32),
    bounds: [u32; 4],
    probe_id: u32,
    weight: f32,
    step: u32,
    minimum_samples: u16,
    black: u16,
    white: u16,
    range_dn: f32,
    exposure_ratio: f32,
    noise: SensorNoiseModel,
) -> Option<FocusResponseObservation> {
    let mut sum = 0.0f64;
    let mut square_sum = 0.0f64;
    let mut count = 0u32;
    for site in 0..4usize {
        for_each_cfa_sample(bounds, origin, site, step, |x, y| {
            for (other_x, other_y) in [(x + 2, y), (x, y + 2)] {
                if other_x >= bounds[2] || other_y >= bounds[3] {
                    continue;
                }
                let first = raw.data[y as usize * raw.width as usize + x as usize];
                let second = raw.data[other_y as usize * raw.width as usize + other_x as usize];
                let saturation_dn = f32::from(white) - noise.saturation_margin_dn.max(0.0);
                if f32::from(first) >= saturation_dn || f32::from(second) >= saturation_dn {
                    continue;
                }
                let first_signal = f32::from(first.saturating_sub(black)) / range_dn;
                let second_signal = f32::from(second.saturating_sub(black)) / range_dn;
                let difference = (first_signal - second_signal) / exposure_ratio;
                let noise_variance = (noise.normalized_variance(site, first_signal, range_dn)
                    + noise.normalized_variance(site, second_signal, range_dn))
                    / exposure_ratio.powi(2);
                let whitened_energy = (difference * difference - noise_variance).max(0.0);
                sum += f64::from(whitened_energy);
                square_sum += f64::from(whitened_energy * whitened_energy);
                count += 1;
            }
        });
    }
    if count < u32::from(minimum_samples) || sum <= 0.0 {
        return None;
    }
    let score = (sum / f64::from(count)) as f32;
    let sample_variance =
        (square_sum / f64::from(count) - (sum / f64::from(count)).powi(2)).max(0.0);
    let variance = (sample_variance / f64::from(count)) as f32;
    Some(FocusResponseObservation {
        probe_id,
        weight,
        score,
        variance: variance.max(f32::EPSILON * score.max(1e-8)),
        sample_count: count,
    })
}

fn cfa_sample_step(width: u32, height: u32, budget_per_site: u16) -> u32 {
    let pixels_per_site = u64::from(width) * u64::from(height) / 4;
    let ratio = pixels_per_site.div_ceil(u64::from(budget_per_site)).max(1);
    let lattice = (ratio as f64).sqrt().ceil() as u32;
    lattice.max(1) * 2
}

fn for_each_cfa_sample(
    bounds: [u32; 4],
    origin: (u32, u32),
    site: usize,
    step: u32,
    mut visit: impl FnMut(u32, u32),
) {
    let target_x = (site & 1) as u32;
    let target_y = ((site >> 1) & 1) as u32;
    let mut y = bounds[1];
    while (origin.1 + y) & 1 != target_y {
        y += 1;
    }
    while y < bounds[3] {
        let mut x = bounds[0];
        while (origin.0 + x) & 1 != target_x {
            x += 1;
        }
        while x < bounds[2] {
            visit(x, y);
            x = x.saturating_add(step);
        }
        y = y.saturating_add(step);
    }
}

fn assimilate_radiance_probe(
    posterior: &mut CapturePosterior,
    measurement: RadianceObservation,
    report: &mut RawAssimilationReport,
) -> Result<()> {
    let existing = posterior
        .radiance
        .iter_mut()
        .find(|probe| probe.probe_id == measurement.probe_id);
    match (existing, measurement.mean, measurement.variance) {
        (Some(probe), Some(mean), Some(variance)) => {
            if probe.cfa_site != measurement.cfa_site {
                anyhow::bail!("RAW radiance probe CFA identity changed");
            }
            let prior_precision = 1.0 / probe.variance.max(f32::MIN_POSITIVE);
            let measurement_precision = 1.0 / variance.max(f32::MIN_POSITIVE);
            probe.mean = (probe.mean * prior_precision + mean * measurement_precision)
                / (prior_precision + measurement_precision);
            probe.variance = 1.0 / (prior_precision + measurement_precision);
            probe.weight = measurement.weight;
            report.radiance_updates += 1;
        }
        (None, Some(mean), Some(variance)) => {
            posterior.radiance.push(RadianceProbe {
                probe_id: measurement.probe_id,
                mean,
                variance,
                weight: measurement.weight,
                cfa_site: measurement.cfa_site,
            });
            report.radiance_updates += 1;
        }
        _ => {}
    }
    if let Some(lower_bound) = measurement.lower_bound {
        if let Some(probe) = posterior
            .radiance
            .iter_mut()
            .find(|probe| probe.probe_id == measurement.probe_id)
        {
            let sigma = probe.variance.max(0.0).sqrt();
            let alpha = (lower_bound - probe.mean) / sigma.max(f32::MIN_POSITIVE);
            if alpha > 6.0 {
                report.censor_conflicts += 1;
            } else if alpha > -6.0 {
                let survival = standard_normal_cdf(-alpha).max(f32::MIN_POSITIVE);
                let inverse_mills = standard_normal_pdf(alpha) / survival;
                probe.mean += sigma * inverse_mills;
                probe.variance *=
                    (1.0 + alpha * inverse_mills - inverse_mills * inverse_mills).max(1e-6);
                report.censored_constraints += 1;
            }
        }
    }
    Ok(())
}

fn insert_focus_plane(
    planes: &mut Vec<FocusPlaneEvidence>,
    evidence: FocusPlaneEvidence,
) -> Result<()> {
    if let Some(existing) = planes.iter_mut().find(|existing| {
        (existing.diopters - evidence.diopters).abs()
            <= existing
                .diopters
                .abs()
                .max(evidence.diopters.abs())
                .mul_add(0.001, 1e-4)
    }) {
        if relative_error(existing.weight, evidence.weight) > 1e-6 {
            anyhow::bail!("Adaptive focus probe weight changed during capture");
        }
        let old_precision = 1.0 / existing.variance.max(f32::MIN_POSITIVE);
        let new_precision = 1.0 / evidence.variance.max(f32::MIN_POSITIVE);
        existing.diopters = (existing.diopters * old_precision + evidence.diopters * new_precision)
            / (old_precision + new_precision);
        existing.score = (existing.score * old_precision + evidence.score * new_precision)
            / (old_precision + new_precision);
        existing.variance = 1.0 / (old_precision + new_precision);
        planes.sort_unstable_by(|left, right| left.diopters.total_cmp(&right.diopters));
        return Ok(());
    }
    if planes.len() >= MAX_FOCUS_PLANES {
        anyhow::bail!("Adaptive focus evidence exceeds the bounded plane limit");
    }
    planes.push(evidence);
    planes.sort_unstable_by(|left, right| left.diopters.total_cmp(&right.diopters));
    Ok(())
}

fn fit_focus_posterior(planes: &[FocusPlaneEvidence]) -> Option<(f32, f32)> {
    if planes.len() < 3 {
        return None;
    }
    let best = planes
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| left.score.total_cmp(&right.score))?
        .0;
    if best == 0 || best + 1 >= planes.len() {
        return None;
    }
    let local = [planes[best - 1], planes[best], planes[best + 1]];
    let x = [local[0].diopters, local[1].diopters, local[2].diopters];
    let y = [
        local[0].score.max(1e-12).ln(),
        local[1].score.max(1e-12).ln(),
        local[2].score.max(1e-12).ln(),
    ];
    let variance_y = [
        local[0].variance / local[0].score.max(1e-12).powi(2),
        local[1].variance / local[1].score.max(1e-12).powi(2),
        local[2].variance / local[2].score.max(1e-12).powi(2),
    ];
    let (vertex, quadratic, linear, constant) = log_quadratic_vertex(x, y)?;
    let mut vertex_variance = 0.0f32;
    for index in 0..3 {
        let epsilon = variance_y[index].max(1e-10).sqrt().max(1e-4) * 0.1;
        let mut plus = y;
        let mut minus = y;
        plus[index] += epsilon;
        minus[index] -= epsilon;
        let plus_vertex = log_quadratic_vertex(x, plus)?.0;
        let minus_vertex = log_quadratic_vertex(x, minus)?.0;
        let derivative = (plus_vertex - minus_vertex) / (2.0 * epsilon);
        vertex_variance += derivative * derivative * variance_y[index];
    }
    let mut residual_sum = 0.0f32;
    let mut residual_count = 0usize;
    for plane in planes {
        if plane.score <= 0.0 {
            continue;
        }
        let predicted = quadratic * plane.diopters.powi(2) + linear * plane.diopters + constant;
        residual_sum += (plane.score.ln() - predicted).powi(2);
        residual_count += 1;
    }
    let residual_variance = residual_sum / residual_count.saturating_sub(3).max(1) as f32;
    let measurement_scale = variance_y.iter().copied().sum::<f32>() / variance_y.len() as f32;
    let inflation = 1.0 + residual_variance / measurement_scale.max(1e-8);
    let variance = (vertex_variance * inflation).max(1e-8);
    (vertex.is_finite() && variance.is_finite()).then_some((vertex, variance))
}

fn log_quadratic_vertex(x: [f32; 3], y: [f32; 3]) -> Option<(f32, f32, f32, f32)> {
    let d0 = (x[0] - x[1]) * (x[0] - x[2]);
    let d1 = (x[1] - x[0]) * (x[1] - x[2]);
    let d2 = (x[2] - x[0]) * (x[2] - x[1]);
    if d0.abs() <= 1e-12 || d1.abs() <= 1e-12 || d2.abs() <= 1e-12 {
        return None;
    }
    let quadratic = y[0] / d0 + y[1] / d1 + y[2] / d2;
    let linear = -y[0] * (x[1] + x[2]) / d0 - y[1] * (x[0] + x[2]) / d1 - y[2] * (x[0] + x[1]) / d2;
    let constant = y[0] * x[1] * x[2] / d0 + y[1] * x[0] * x[2] / d1 + y[2] * x[0] * x[1] / d2;
    if !quadratic.is_finite() || quadratic >= -1e-8 || !linear.is_finite() {
        return None;
    }
    let vertex = -linear / (2.0 * quadratic);
    let low = x[0].min(x[2]);
    let high = x[0].max(x[2]);
    (vertex.is_finite() && vertex >= low && vertex <= high)
        .then_some((vertex, quadratic, linear, constant))
}

fn upsert_focus_probe(posterior: &mut CapturePosterior, measurement: FocusProbe) {
    if let Some(existing) = posterior
        .focus
        .iter_mut()
        .find(|probe| probe.probe_id == measurement.probe_id)
    {
        *existing = measurement;
    } else {
        posterior.focus.push(measurement);
    }
}

fn validate_observation(observation: &RawCaptureObservation) -> Result<()> {
    if observation.camera_make.trim().is_empty()
        || observation.camera_model.trim().is_empty()
        || observation.sensor_calibration_id.trim().is_empty()
        || observation.bits_per_sample == 0
        || observation.iso == 0
        || !observation.exposure_seconds.is_finite()
        || observation.exposure_seconds <= 0.0
        || !observation.radiance_anchor_exposure.is_finite()
        || observation.radiance_anchor_exposure <= 0.0
        || observation.radiance.is_empty()
        || observation.radiance.len() > MAX_PROBE_TILES * 4
        || observation.focus.len() > MAX_PROBE_TILES
        || observation.roi[2] == 0
        || observation.roi[3] == 0
    {
        anyhow::bail!("RAW capture observation identity or exposure is invalid");
    }
    let mut radiance_ids = observation
        .radiance
        .iter()
        .map(|measurement| measurement.probe_id)
        .collect::<Vec<_>>();
    radiance_ids.sort_unstable();
    if radiance_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        anyhow::bail!("RAW radiance observation probe identities are duplicated");
    }
    for measurement in &observation.radiance {
        if measurement.cfa_site > 3
            || !measurement.weight.is_finite()
            || measurement.weight < 0.0
            || measurement
                .mean
                .is_some_and(|value| !value.is_finite() || value < 0.0)
            || measurement
                .variance
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
            || measurement
                .lower_bound
                .is_some_and(|value| !value.is_finite() || value < 0.0)
            || measurement.mean.is_some() != measurement.variance.is_some()
        {
            anyhow::bail!("RAW radiance observation contains invalid evidence");
        }
    }
    let mut focus_ids = observation
        .focus
        .iter()
        .map(|measurement| measurement.probe_id)
        .collect::<Vec<_>>();
    focus_ids.sort_unstable();
    if focus_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        anyhow::bail!("RAW focus observation probe identities are duplicated");
    }
    for measurement in &observation.focus {
        if !measurement.weight.is_finite()
            || measurement.weight < 0.0
            || !measurement.score.is_finite()
            || measurement.score <= 0.0
            || !measurement.variance.is_finite()
            || measurement.variance <= 0.0
            || measurement.sample_count == 0
        {
            anyhow::bail!("RAW focus observation contains invalid evidence");
        }
    }
    if observation
        .focus_diopters
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        anyhow::bail!("RAW observation focus distance is invalid");
    }
    if !observation.focus.is_empty()
        && (observation.focus_diopters.is_none()
            || observation
                .focal_length_mm
                .map_or(true, |value| !value.is_finite() || value <= 0.0)
            || observation
                .aperture
                .map_or(true, |value| !value.is_finite() || value <= 0.0))
    {
        anyhow::bail!("RAW focus evidence requires physical focus, focal length, and aperture");
    }
    Ok(())
}

fn relative_error(left: f32, right: f32) -> f32 {
    (left - right).abs() / left.abs().max(right.abs()).max(f32::MIN_POSITIVE)
}

fn optional_nearly_equal(left: Option<f32>, right: Option<f32>, tolerance: f32) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => relative_error(left, right) <= tolerance,
        (None, None) => true,
        _ => false,
    }
}

fn standard_normal_pdf(value: f32) -> f32 {
    (-0.5 * value * value).exp() * 0.398_942_3
}

fn standard_normal_cdf(value: f32) -> f32 {
    // Abramowitz-Stegun 26.2.17. This is deterministic and accurate enough
    // for the bounded |alpha| <= 6 censor update used above.
    let absolute = value.abs();
    let t = 1.0 / absolute.mul_add(0.231_641_9, 1.0);
    let polynomial = t
        * (0.319_381_54
            + t * (-0.356_563_78 + t * (1.781_477_9 + t * (-1.821_256 + t * 1.330_274_5))));
    let lower_tail = standard_normal_pdf(absolute) * polynomial;
    if value >= 0.0 {
        1.0 - lower_tail
    } else {
        lower_tail
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nef::parser::{SensorGeometry, SensorLevels};
    use crate::sensor_noise::{IsoNoiseModel, SensorNoiseModel, SENSOR_NOISE_PROFILE_SCHEMA};

    fn profile() -> SensorNoiseProfile {
        SensorNoiseProfile {
            schema: SENSOR_NOISE_PROFILE_SCHEMA.to_string(),
            camera_make: "NIKON CORPORATION".to_string(),
            camera_model: "NIKON Z 9".to_string(),
            bits_per_sample: 14,
            calibration_id: "sha256:raw-observation-test".to_string(),
            iso_models: vec![IsoNoiseModel {
                iso: 100,
                model: SensorNoiseModel {
                    read_noise_dn: [2.0; 4],
                    electrons_per_dn: [0.8; 4],
                    black_drift_dn: [0.5; 4],
                    saturation_margin_dn: 16.0,
                    calibrated: true,
                },
            }],
        }
    }

    fn metadata(exposure: f64, distance: f32) -> Z9Metadata {
        Z9Metadata {
            width: 64,
            height: 64,
            bits_per_sample: 14,
            compression: 34713,
            cfa_pattern: [0, 1, 1, 2],
            camera_make: "NIKON CORPORATION".to_string(),
            camera_model: "NIKON Z 9".to_string(),
            sensor_levels: Some(SensorLevels {
                black: 1008,
                white: 15311,
            }),
            sensor_geometry: Some(SensorGeometry {
                pixel_pitch_um: 4.35,
            }),
            strip_offsets: vec![],
            strip_byte_counts: vec![],
            rows_per_strip: 64,
            cam_mul: [1.0; 4],
            timestamp: None,
            exposure_time: Some(exposure),
            aperture: Some(8.0),
            iso: Some(100),
            focal_length: Some(105.0),
            focus_distance: Some(distance),
        }
    }

    fn textured_raw(exposure_ratio: f32, blur: usize) -> RawBuffer {
        let mut raw = RawBuffer::new(64, 64, [0, 1, 1, 2], 14);
        for y in 0..64usize {
            for x in 0..64usize {
                let block = ((x / blur.max(1)) + (y / blur.max(1))) & 1;
                let signal = if block == 0 { 0.12 } else { 0.32 };
                raw.data[y * 64 + x] = (1008.0 + signal * exposure_ratio * 14_303.0) as u16;
            }
        }
        raw
    }

    fn empty_posterior(anchor: f32) -> CapturePosterior {
        CapturePosterior {
            radiance: vec![],
            focus: vec![],
            radiance_anchor_exposure: anchor,
            current_focus_diopters: 0.0,
            motion_pixels_per_second: 0.0,
            elapsed_ms: 0.0,
            thermal_load: 0.0,
        }
    }

    #[test]
    fn radiance_observation_is_exposure_invariant_and_cfa_stable_for_odd_roi() {
        let anchor = 0.01;
        let config = RawObservationConfig {
            tile_columns: 2,
            tile_rows: 2,
            ..Default::default()
        };
        let short = observe_raw_roi(
            &textured_raw(1.0, 2),
            (1, 3),
            &metadata(0.01, 0.5),
            &profile(),
            anchor,
            config,
        )
        .unwrap();
        let long = observe_raw_roi(
            &textured_raw(2.0, 2),
            (1, 3),
            &metadata(0.02, 0.5),
            &profile(),
            anchor,
            config,
        )
        .unwrap();
        for (left, right) in short.radiance.iter().zip(&long.radiance) {
            assert_eq!(left.probe_id, right.probe_id);
            assert_eq!(left.cfa_site, right.cfa_site);
            assert!((left.mean.unwrap() - right.mean.unwrap()).abs() < 2e-4);
        }
    }

    #[test]
    fn calibrated_observations_reduce_radiance_uncertainty() {
        let anchor = 0.01;
        let observation = observe_raw_roi(
            &textured_raw(1.0, 2),
            (0, 0),
            &metadata(0.01, 0.5),
            &profile(),
            anchor,
            RawObservationConfig {
                tile_columns: 1,
                tile_rows: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let mut posterior = empty_posterior(anchor);
        let mut accumulator = RawPosteriorAccumulator::new(anchor).unwrap();
        accumulator
            .assimilate(&mut posterior, &observation)
            .unwrap();
        let first = posterior.radiance[0].variance;
        accumulator
            .assimilate(&mut posterior, &observation)
            .unwrap();
        assert!(posterior.radiance[0].variance < first * 0.51);
    }

    #[test]
    fn censored_observation_never_pulls_a_prior_downward() {
        let anchor = 0.01;
        let mut raw = RawBuffer::new(32, 32, [0, 1, 1, 2], 14);
        raw.data.fill(15_311);
        let observation = observe_raw_roi(
            &raw,
            (0, 0),
            &metadata(0.01, 0.5),
            &profile(),
            anchor,
            RawObservationConfig {
                tile_columns: 1,
                tile_rows: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let mut posterior = empty_posterior(anchor);
        posterior.radiance.push(RadianceProbe {
            probe_id: 0,
            mean: 0.5,
            variance: 0.04,
            weight: 0.25,
            cfa_site: 0,
        });
        let mut accumulator = RawPosteriorAccumulator::new(anchor).unwrap();
        let report = accumulator
            .assimilate(&mut posterior, &observation)
            .unwrap();
        assert!(posterior.radiance[0].mean >= 0.5);
        assert!(posterior.radiance[0].variance < 0.04);
        assert_eq!(report.censored_constraints, 1);
    }

    #[test]
    fn mixed_clipping_is_not_misrepresented_as_a_tile_mean_bound() {
        let anchor = 0.01;
        let mut raw = textured_raw(1.0, 2);
        for y in 0..raw.height as usize {
            for x in (0..raw.width as usize).step_by(8) {
                raw.data[y * raw.width as usize + x] = 15_311;
            }
        }
        let observation = observe_raw_roi(
            &raw,
            (0, 0),
            &metadata(0.01, 0.5),
            &profile(),
            anchor,
            RawObservationConfig {
                tile_columns: 1,
                tile_rows: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(observation
            .radiance
            .iter()
            .any(|probe| probe.censored_samples > 0 && probe.valid_samples > 0));
        assert!(observation
            .radiance
            .iter()
            .filter(|probe| probe.valid_samples > 0)
            .all(|probe| probe.lower_bound.is_none()));
    }

    #[test]
    fn accumulator_rejects_cross_capture_identity_drift() {
        let anchor = 0.01;
        let observation = observe_raw_roi(
            &textured_raw(1.0, 2),
            (0, 0),
            &metadata(0.01, 0.5),
            &profile(),
            anchor,
            RawObservationConfig {
                tile_columns: 1,
                tile_rows: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let mut accumulator = RawPosteriorAccumulator::new(anchor).unwrap();
        let mut posterior = empty_posterior(anchor);
        accumulator
            .assimilate(&mut posterior, &observation)
            .unwrap();

        for mutation in 0..3 {
            let mut changed = observation.clone();
            match mutation {
                0 => changed.roi[0] += 2,
                1 => changed.sensor_calibration_id.push_str("-wrong"),
                _ => changed.aperture = Some(11.0),
            }
            assert!(accumulator.assimilate(&mut posterior, &changed).is_err());
        }
    }

    #[test]
    fn focus_metadata_jitter_merges_one_physical_plane() {
        let mut planes = Vec::new();
        insert_focus_plane(
            &mut planes,
            FocusPlaneEvidence {
                diopters: 2.0,
                score: 0.8,
                variance: 0.02,
                weight: 0.5,
            },
        )
        .unwrap();
        insert_focus_plane(
            &mut planes,
            FocusPlaneEvidence {
                diopters: 2.001,
                score: 0.9,
                variance: 0.01,
                weight: 0.5,
            },
        )
        .unwrap();
        assert_eq!(planes.len(), 1);
        assert!(planes[0].variance < 0.01);
    }

    #[test]
    fn measured_nonuniform_focus_planes_recover_subplane_peak() {
        let mut accumulator = RawPosteriorAccumulator::new(0.01).unwrap();
        let mut posterior = empty_posterior(0.01);
        let target = 2.3f32;
        for &diopters in &[1.0f32, 2.0, 3.5, 5.0] {
            let score = (-(diopters - target).powi(2) / 1.2).exp();
            let observation = RawCaptureObservation {
                camera_make: "NIKON CORPORATION".to_string(),
                camera_model: "NIKON Z 9".to_string(),
                bits_per_sample: 14,
                sensor_calibration_id: profile().calibration_id,
                iso: 100,
                exposure_seconds: 0.01,
                focus_diopters: Some(diopters),
                focal_length_mm: Some(105.0),
                aperture: Some(8.0),
                roi: [0, 0, 64, 64],
                radiance_anchor_exposure: 0.01,
                radiance: vec![RadianceObservation {
                    probe_id: 0,
                    cfa_site: 0,
                    weight: 1.0,
                    mean: Some(0.2),
                    variance: Some(0.01),
                    lower_bound: None,
                    valid_samples: 64,
                    censored_samples: 0,
                }],
                focus: vec![FocusResponseObservation {
                    probe_id: 0,
                    weight: 1.0,
                    score,
                    variance: score * score * 1e-4,
                    sample_count: 128,
                }],
            };
            accumulator
                .assimilate(&mut posterior, &observation)
                .unwrap();
        }
        assert_eq!(posterior.focus.len(), 1);
        assert!((posterior.focus[0].mean_diopters - target).abs() < 1e-3);
        assert!(posterior.focus[0].variance_diopters2 < 0.01);
    }

    #[test]
    fn candidate_verification_rejects_wrong_focus_or_exposure() {
        let observation = RawCaptureObservation {
            camera_make: "NIKON".to_string(),
            camera_model: "Z9".to_string(),
            bits_per_sample: 14,
            sensor_calibration_id: "sha256:test".to_string(),
            iso: 100,
            exposure_seconds: 0.01,
            focus_diopters: Some(2.0),
            focal_length_mm: Some(105.0),
            aperture: Some(8.0),
            roi: [0, 0, 16, 16],
            radiance_anchor_exposure: 0.01,
            radiance: vec![RadianceObservation {
                probe_id: 0,
                cfa_site: 0,
                weight: 1.0,
                mean: Some(0.2),
                variance: Some(0.01),
                lower_bound: None,
                valid_samples: 16,
                censored_samples: 0,
            }],
            focus: vec![],
        };
        let candidate = CaptureCandidate {
            shutter_seconds: 0.01,
            iso: 100,
            focus_diopters: 2.0,
            readout_ms: 20.0,
            settle_ms: 5.0,
        };
        verify_observation_candidate(&observation, candidate).unwrap();
        assert!(verify_observation_candidate(
            &observation,
            CaptureCandidate {
                shutter_seconds: 0.02,
                ..candidate
            }
        )
        .is_err());
        assert!(verify_observation_candidate(
            &observation,
            CaptureCandidate {
                focus_diopters: 3.0,
                ..candidate
            }
        )
        .is_err());
    }
}
