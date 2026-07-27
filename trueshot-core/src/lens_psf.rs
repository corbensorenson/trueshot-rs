//! Digest-bound lens breathing, pupil, and field-PSF calibration.
//!
//! The profile corrects the paraxial thin-lens model from retained target
//! measurements. It remains deterministic and measurement-only: no learned or
//! generated image content is involved.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub const LENS_PSF_PROFILE_SCHEMA: &str = "trueshot.lens-psf.v1";
pub const LENS_PSF_MEASUREMENTS_SCHEMA: &str = "trueshot.lens-psf-measurements.v1";
pub const MAX_LENS_PSF_ARTIFACT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_FOCUS_KNOTS: usize = 64;
const MAX_RADIUS_KNOTS: usize = 16;
const MAX_MEASUREMENTS: usize = 65_536;
const RADIUS_KNOT_TOLERANCE: f32 = 1e-4;
static PARTIAL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LensPsfFocusKnot {
    pub focus_distance_m: f32,
    pub effective_focal_length_mm: f32,
    /// Entrance-pupil diameter relative to `nominal_focal_length / f_number`.
    pub entrance_pupil_scale: f32,
    /// Residual field-dependent PSF scale at each profile radius knot.
    pub radial_psf_scale: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LensPsfProfile {
    pub schema: String,
    pub camera_make: String,
    pub camera_model: String,
    pub sensor_width: u32,
    pub sensor_height: u32,
    pub lens_model: String,
    pub nominal_focal_length_mm: f32,
    pub aperture: f32,
    /// Canonical digest of the source-hashed fit/holdout measurement set.
    pub measurement_set_digest: String,
    pub radius_knots: Vec<f32>,
    pub focus_knots: Vec<LensPsfFocusKnot>,
    pub fit_measurements: u32,
    pub holdout_measurements: u32,
    pub ideal_holdout_p95_relative_error: f32,
    pub calibrated_holdout_p95_relative_error: f32,
    pub maximum_holdout_p95_relative_error: f32,
    pub minimum_holdout_p95_error_reduction: f32,
    #[serde(skip)]
    pub calibration_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibratedLensOptics {
    pub effective_focal_length_mm: f32,
    pub entrance_pupil_mm: f32,
    pub radial_psf_scale: f32,
}

impl LensPsfProfile {
    pub fn validate(&self) -> Result<()> {
        if self.schema != LENS_PSF_PROFILE_SCHEMA {
            anyhow::bail!(
                "Unsupported lens PSF schema {}; expected {}",
                self.schema,
                LENS_PSF_PROFILE_SCHEMA
            );
        }
        if self.camera_make.trim().is_empty()
            || self.camera_model.trim().is_empty()
            || self.lens_model.trim().is_empty()
            || self.calibration_id.trim().is_empty()
            || self.sensor_width < 16
            || self.sensor_height < 16
            || !self.nominal_focal_length_mm.is_finite()
            || self.nominal_focal_length_mm <= 0.0
            || !self.aperture.is_finite()
            || self.aperture <= 0.0
            || self
                .measurement_set_digest
                .strip_prefix("sha256:")
                .map_or(true, |digest| !valid_sha256(digest))
            || !(2..=MAX_RADIUS_KNOTS).contains(&self.radius_knots.len())
            || !(2..=MAX_FOCUS_KNOTS).contains(&self.focus_knots.len())
            || self.fit_measurements < 4
            || self.fit_measurements as usize > MAX_MEASUREMENTS
            || self.holdout_measurements < 4
            || self.holdout_measurements as usize > MAX_MEASUREMENTS
            || !self.ideal_holdout_p95_relative_error.is_finite()
            || !self.calibrated_holdout_p95_relative_error.is_finite()
            || self.ideal_holdout_p95_relative_error < 0.0
            || self.calibrated_holdout_p95_relative_error < 0.0
            || self.calibrated_holdout_p95_relative_error > self.ideal_holdout_p95_relative_error
            || !self.maximum_holdout_p95_relative_error.is_finite()
            || !(0.005..=0.25).contains(&self.maximum_holdout_p95_relative_error)
            || !self.minimum_holdout_p95_error_reduction.is_finite()
            || !(0.1..=0.95).contains(&self.minimum_holdout_p95_error_reduction)
            || self.calibrated_holdout_p95_relative_error > self.maximum_holdout_p95_relative_error
        {
            anyhow::bail!("Lens PSF profile identity, dimensions, or metrics are invalid");
        }
        let error_reduction = if self.ideal_holdout_p95_relative_error > 1e-8 {
            1.0 - self.calibrated_holdout_p95_relative_error / self.ideal_holdout_p95_relative_error
        } else {
            0.0
        };
        if error_reduction < self.minimum_holdout_p95_error_reduction {
            anyhow::bail!("Lens PSF profile does not satisfy its holdout improvement gate");
        }
        if self.radius_knots[0].abs() > 1e-6
            || (self.radius_knots[self.radius_knots.len() - 1] - 1.0).abs() > 1e-6
            || self
                .radius_knots
                .iter()
                .any(|radius| !radius.is_finite() || !(0.0..=1.0).contains(radius))
            || self
                .radius_knots
                .windows(2)
                .any(|pair| pair[1] - pair[0] <= 1e-5)
        {
            anyhow::bail!("Lens PSF radius knots must be strictly increasing from 0 to 1");
        }
        for knot in &self.focus_knots {
            if !knot.focus_distance_m.is_finite()
                || knot.focus_distance_m <= self.nominal_focal_length_mm * 0.001 * 1.01
                || !knot.effective_focal_length_mm.is_finite()
                || !(self.nominal_focal_length_mm * 0.5..=self.nominal_focal_length_mm * 1.5)
                    .contains(&knot.effective_focal_length_mm)
                || !knot.entrance_pupil_scale.is_finite()
                || !(0.25..=4.0).contains(&knot.entrance_pupil_scale)
                || knot.radial_psf_scale.len() != self.radius_knots.len()
                || knot
                    .radial_psf_scale
                    .iter()
                    .any(|scale| !scale.is_finite() || !(0.25..=4.0).contains(scale))
            {
                anyhow::bail!("Lens PSF focus knot contains invalid optical values");
            }
        }
        if self
            .focus_knots
            .windows(2)
            .any(|pair| pair[1].focus_distance_m - pair[0].focus_distance_m <= 1e-5)
        {
            anyhow::bail!("Lens PSF focus knots must be strictly increasing in meters");
        }
        let sensor_distances = self
            .focus_knots
            .iter()
            .map(|knot| image_distance_mm(knot.effective_focal_length_mm, knot.focus_distance_m))
            .collect::<Vec<_>>();
        let increasing = sensor_distances
            .windows(2)
            .all(|pair| pair[1] - pair[0] > 1e-7);
        let decreasing = sensor_distances
            .windows(2)
            .all(|pair| pair[0] - pair[1] > 1e-7);
        if !increasing && !decreasing {
            anyhow::bail!("Lens PSF calibrated sensor distances must be strictly monotonic");
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn matches(
        &self,
        camera_make: &str,
        camera_model: &str,
        sensor_width: u32,
        sensor_height: u32,
        lens_model: &str,
        focal_length_mm: f32,
        aperture: f32,
        focus_distance_m: f32,
    ) -> bool {
        normalized_identity(&self.camera_make) == normalized_identity(camera_make)
            && normalized_identity(&self.camera_model) == normalized_identity(camera_model)
            && normalized_identity(&self.lens_model) == normalized_identity(lens_model)
            && self.sensor_width == sensor_width
            && self.sensor_height == sensor_height
            && relative_match(self.nominal_focal_length_mm, focal_length_mm, 0.005)
            && relative_match(self.aperture, aperture, 0.01)
            && focus_distance_m >= self.focus_knots[0].focus_distance_m * 0.995
            && focus_distance_m
                <= self.focus_knots[self.focus_knots.len() - 1].focus_distance_m * 1.005
    }

    pub fn optics_at(&self, focus_distance_m: f32, field_radius: f32) -> CalibratedLensOptics {
        let (lower, upper, fraction) = bracket_focus_diopters(&self.focus_knots, focus_distance_m);
        let left = &self.focus_knots[lower];
        let right = &self.focus_knots[upper];
        let effective_focal_length_mm = lerp(
            left.effective_focal_length_mm,
            right.effective_focal_length_mm,
            fraction,
        );
        let entrance_pupil_scale = lerp(
            left.entrance_pupil_scale,
            right.entrance_pupil_scale,
            fraction,
        );
        let left_radial = interpolate_knots(
            &self.radius_knots,
            &left.radial_psf_scale,
            field_radius.clamp(0.0, 1.0),
        );
        let right_radial = interpolate_knots(
            &self.radius_knots,
            &right.radial_psf_scale,
            field_radius.clamp(0.0, 1.0),
        );
        CalibratedLensOptics {
            effective_focal_length_mm,
            entrance_pupil_mm: self.nominal_focal_length_mm / self.aperture * entrance_pupil_scale,
            radial_psf_scale: lerp(left_radial, right_radial, fraction),
        }
    }

    pub fn defocus_circle_mm(
        &self,
        focused_distance_m: f32,
        subject_distance_m: f32,
        field_radius: f32,
    ) -> f32 {
        let optics = self.optics_at(focused_distance_m, field_radius);
        defocus_with_optics(
            optics.effective_focal_length_mm,
            optics.entrance_pupil_mm,
            focused_distance_m,
            subject_distance_m,
        ) * optics.radial_psf_scale
    }

    pub fn sensor_distance_mm(&self, focused_distance_m: f32) -> f32 {
        let focal = self
            .optics_at(focused_distance_m, 0.0)
            .effective_focal_length_mm;
        image_distance_mm(focal, focused_distance_m)
    }

    pub fn conservative_aperture_radius_mm(&self, focused_distance_m: f32) -> f32 {
        let mut maximum = 0.0f32;
        for &radius in &self.radius_knots {
            let optics = self.optics_at(focused_distance_m, radius);
            maximum = maximum.max(optics.entrance_pupil_mm * optics.radial_psf_scale * 0.5);
        }
        maximum
    }

    pub fn field_radius(&self, sensor_x: f32, sensor_y: f32) -> f32 {
        normalized_field_radius(
            sensor_x,
            sensor_y,
            self.sensor_width as f32,
            self.sensor_height as f32,
        )
    }

    pub fn load_json(path: &Path) -> Result<Self> {
        Self::load_json_with_key(path, None)
    }

    pub fn load_json_with_key(path: &Path, encrypted_key: Option<&[u8; 32]>) -> Result<Self> {
        let bytes = if path.extension().and_then(|value| value.to_str()) == Some("enc") {
            let key = encrypted_key.context("Encrypted lens PSF profile requires a key")?;
            trueshot_storage::encrypted::decrypt_to_vec(
                path,
                key,
                MAX_LENS_PSF_ARTIFACT_BYTES as usize,
            )?
        } else {
            bounded_read(path, "Lens PSF profile")?
        };
        Self::from_json_bytes(&bytes)
    }

    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > MAX_LENS_PSF_ARTIFACT_BYTES {
            anyhow::bail!("Lens PSF profile exceeds the size limit");
        }
        let mut profile: Self = serde_json::from_slice(bytes)?;
        profile.calibration_id = format!("sha256:{}", hex::encode(Sha256::digest(bytes)));
        profile.validate()?;
        Ok(profile)
    }

    pub fn save_json(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if path.exists() {
            anyhow::bail!("Refusing to overwrite lens PSF profile {}", path.display());
        }
        atomic_json(path, self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationSplit {
    Fit,
    Holdout,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LensPsfMeasurement {
    /// SHA-256 of the retained calibration image or measurement source.
    pub source_sha256: String,
    pub split: CalibrationSplit,
    pub focus_distance_m: f32,
    /// Auditable origin of the focus coordinate, when supplied.
    #[serde(default)]
    pub focus_distance_source: Option<String>,
    /// One-sigma uncertainty for an independently measured focus coordinate.
    #[serde(default)]
    pub focus_distance_uncertainty_m: Option<f32>,
    pub subject_distance_m: f32,
    pub field_radius: f32,
    pub effective_focal_length_mm: f32,
    pub observed_defocus_diameter_px: f32,
    pub pixel_pitch_um: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LensPsfSourceRecord {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LensPsfMeasurementSet {
    pub schema: String,
    pub camera_make: String,
    pub camera_model: String,
    pub sensor_width: u32,
    pub sensor_height: u32,
    pub lens_model: String,
    pub nominal_focal_length_mm: f32,
    pub aperture: f32,
    pub target_id: String,
    pub measurement_method: String,
    pub radius_knots: Vec<f32>,
    pub sources: Vec<LensPsfSourceRecord>,
    pub measurements: Vec<LensPsfMeasurement>,
}

impl LensPsfMeasurementSet {
    pub fn load_json(path: &Path) -> Result<Self> {
        let bytes = bounded_read(path, "Lens PSF measurements")?;
        let set: Self = serde_json::from_slice(&bytes)?;
        set.validate()?;
        Ok(set)
    }

    pub fn save_json(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if path.exists() {
            anyhow::bail!(
                "Refusing to overwrite lens PSF measurements {}",
                path.display()
            );
        }
        atomic_json(path, self)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != LENS_PSF_MEASUREMENTS_SCHEMA
            || self.camera_make.trim().is_empty()
            || self.camera_model.trim().is_empty()
            || self.lens_model.trim().is_empty()
            || self.target_id.trim().is_empty()
            || self.measurement_method.trim().is_empty()
            || self.sensor_width < 16
            || self.sensor_height < 16
            || !self.nominal_focal_length_mm.is_finite()
            || self.nominal_focal_length_mm <= 0.0
            || !self.aperture.is_finite()
            || self.aperture <= 0.0
            || !(2..=MAX_RADIUS_KNOTS).contains(&self.radius_knots.len())
            || self.measurements.len() > MAX_MEASUREMENTS
            || self.sources.is_empty()
            || self.sources.len() > MAX_MEASUREMENTS
        {
            anyhow::bail!("Lens PSF measurement-set identity or dimensions are invalid");
        }
        if self.radius_knots[0].abs() > 1e-6
            || (self.radius_knots[self.radius_knots.len() - 1] - 1.0).abs() > 1e-6
            || self
                .radius_knots
                .iter()
                .any(|radius| !radius.is_finite() || !(0.0..=1.0).contains(radius))
            || self
                .radius_knots
                .windows(2)
                .any(|pair| pair[1] - pair[0] <= 1e-5)
        {
            anyhow::bail!("Measurement radius knots must increase strictly from 0 to 1");
        }
        let mut source_hashes = HashSet::with_capacity(self.sources.len());
        let mut source_paths = HashSet::with_capacity(self.sources.len());
        for source in &self.sources {
            if source.path.trim().is_empty()
                || !valid_sha256(&source.sha256)
                || !source_hashes.insert(source.sha256.as_str())
                || !source_paths.insert(source.path.as_str())
            {
                anyhow::bail!(
                    "Lens PSF sources require unique retained paths and lowercase SHA-256 values"
                );
            }
        }
        let mut referenced_sources = HashSet::with_capacity(self.sources.len());
        let mut source_splits = HashMap::with_capacity(self.sources.len());
        for measurement in &self.measurements {
            let radius_index = nearest_knot(&self.radius_knots, measurement.field_radius);
            let valid_focus_provenance = match (
                measurement.focus_distance_source.as_deref(),
                measurement.focus_distance_uncertainty_m,
            ) {
                (None, None) | (Some("exif_subject_distance"), None) => true,
                (Some("independent_measured"), Some(uncertainty)) => {
                    uncertainty.is_finite()
                        && uncertainty > 0.0
                        && uncertainty / measurement.focus_distance_m <= 0.25
                }
                _ => false,
            };
            if !source_hashes.contains(measurement.source_sha256.as_str())
                || !measurement.focus_distance_m.is_finite()
                || measurement.focus_distance_m <= self.nominal_focal_length_mm * 0.001 * 1.01
                || !valid_focus_provenance
                || !measurement.subject_distance_m.is_finite()
                || measurement.subject_distance_m <= self.nominal_focal_length_mm * 0.001 * 1.01
                || !measurement.field_radius.is_finite()
                || !(0.0..=1.0).contains(&measurement.field_radius)
                || (measurement.field_radius - self.radius_knots[radius_index]).abs()
                    > RADIUS_KNOT_TOLERANCE
                || !measurement.effective_focal_length_mm.is_finite()
                || !(self.nominal_focal_length_mm * 0.5..=self.nominal_focal_length_mm * 1.5)
                    .contains(&measurement.effective_focal_length_mm)
                || !measurement.observed_defocus_diameter_px.is_finite()
                || measurement.observed_defocus_diameter_px < 0.5
                || !measurement.pixel_pitch_um.is_finite()
                || !(0.5..=20.0).contains(&measurement.pixel_pitch_um)
            {
                anyhow::bail!("Lens PSF measurement contains invalid physical evidence");
            }
            referenced_sources.insert(measurement.source_sha256.as_str());
            if source_splits
                .insert(measurement.source_sha256.as_str(), measurement.split)
                .is_some_and(|split| split != measurement.split)
            {
                anyhow::bail!("Lens PSF fit and holdout splits must use disjoint retained sources");
            }
        }
        if referenced_sources.len() != source_hashes.len() {
            anyhow::bail!("Lens PSF source manifest contains unreferenced evidence");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LensPsfCalibrationConfig {
    pub minimum_fit_samples_per_cell: usize,
    pub minimum_holdout_samples_per_cell: usize,
    pub minimum_holdout_measurements: usize,
    pub maximum_calibrated_p95_relative_error: f32,
    pub minimum_p95_error_reduction: f32,
    pub focus_group_relative_tolerance: f32,
}

impl Default for LensPsfCalibrationConfig {
    fn default() -> Self {
        Self {
            minimum_fit_samples_per_cell: 2,
            minimum_holdout_samples_per_cell: 1,
            minimum_holdout_measurements: 12,
            maximum_calibrated_p95_relative_error: 0.05,
            minimum_p95_error_reduction: 0.50,
            focus_group_relative_tolerance: 0.002,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LensPsfCalibrationReport {
    pub passed: bool,
    pub fit_measurements: usize,
    pub holdout_measurements: usize,
    pub focus_knots: usize,
    pub radius_knots: usize,
    pub ideal_holdout_p95_relative_error: f32,
    pub calibrated_holdout_p95_relative_error: f32,
    pub error_reduction: f32,
    pub failures: Vec<String>,
}

pub fn calibrate_lens_psf(
    set: &LensPsfMeasurementSet,
    config: &LensPsfCalibrationConfig,
) -> Result<(Option<LensPsfProfile>, LensPsfCalibrationReport)> {
    set.validate()?;
    validate_calibration_config(config)?;
    let mut focus_centers = set
        .measurements
        .iter()
        .filter(|measurement| measurement.split == CalibrationSplit::Fit)
        .map(|measurement| measurement.focus_distance_m)
        .collect::<Vec<_>>();
    focus_centers.sort_by(f32::total_cmp);
    let mut grouped = Vec::<f32>::new();
    for focus in focus_centers {
        if grouped.last().map_or(true, |previous| {
            (focus - *previous).abs()
                > previous.abs().max(focus.abs()) * config.focus_group_relative_tolerance
        }) {
            grouped.push(focus);
        }
    }
    if !(2..=MAX_FOCUS_KNOTS).contains(&grouped.len()) {
        anyhow::bail!("Lens PSF calibration requires 2-{MAX_FOCUS_KNOTS} fit focus groups");
    }

    let mut focus_knots = Vec::with_capacity(grouped.len());
    let mut failures = Vec::new();
    for &focus in &grouped {
        let group = set
            .measurements
            .iter()
            .filter(|measurement| {
                measurement.split == CalibrationSplit::Fit
                    && same_focus(
                        measurement.focus_distance_m,
                        focus,
                        config.focus_group_relative_tolerance,
                    )
            })
            .collect::<Vec<_>>();
        let mut focal_samples = group
            .iter()
            .map(|measurement| measurement.effective_focal_length_mm)
            .collect::<Vec<_>>();
        let effective_focal_length_mm = median(&mut focal_samples)
            .context("Focus group has no effective-focal-length measurements")?;
        let mut combined_scales = Vec::with_capacity(set.radius_knots.len());
        for (radius_index, &radius) in set.radius_knots.iter().enumerate() {
            let mut samples = group
                .iter()
                .filter(|measurement| {
                    nearest_knot(&set.radius_knots, measurement.field_radius) == radius_index
                })
                .filter_map(|measurement| {
                    let ideal = defocus_with_optics(
                        effective_focal_length_mm,
                        set.nominal_focal_length_mm / set.aperture,
                        measurement.focus_distance_m,
                        measurement.subject_distance_m,
                    );
                    let observed = measurement.observed_defocus_diameter_px
                        * measurement.pixel_pitch_um
                        * 0.001;
                    (ideal > 1e-8).then_some(observed / ideal)
                })
                .collect::<Vec<_>>();
            if samples.len() < config.minimum_fit_samples_per_cell {
                failures.push(format!(
                    "focus {focus:.6} m radius {radius:.3} has {} fit samples; require {}",
                    samples.len(),
                    config.minimum_fit_samples_per_cell
                ));
                combined_scales.push(1.0);
            } else {
                combined_scales.push(median(&mut samples).unwrap_or(1.0));
            }
        }
        let entrance_pupil_scale = combined_scales[0];
        let radial_psf_scale = combined_scales
            .iter()
            .map(|scale| scale / entrance_pupil_scale.max(1e-8))
            .collect();
        focus_knots.push(LensPsfFocusKnot {
            focus_distance_m: focus,
            effective_focal_length_mm,
            entrance_pupil_scale,
            radial_psf_scale,
        });
    }

    let fit_measurements = set
        .measurements
        .iter()
        .filter(|measurement| measurement.split == CalibrationSplit::Fit)
        .count();
    let holdout = set
        .measurements
        .iter()
        .filter(|measurement| measurement.split == CalibrationSplit::Holdout)
        .collect::<Vec<_>>();
    if holdout.len() < config.minimum_holdout_measurements {
        failures.push(format!(
            "{} holdout measurements; require {}",
            holdout.len(),
            config.minimum_holdout_measurements
        ));
    }
    for &focus in &grouped {
        for (radius_index, &radius) in set.radius_knots.iter().enumerate() {
            let samples = holdout
                .iter()
                .filter(|measurement| {
                    same_focus(
                        measurement.focus_distance_m,
                        focus,
                        config.focus_group_relative_tolerance,
                    ) && nearest_knot(&set.radius_knots, measurement.field_radius) == radius_index
                })
                .count();
            if samples < config.minimum_holdout_samples_per_cell {
                failures.push(format!(
                    "focus {focus:.6} m radius {radius:.3} has {samples} holdout samples; require {}",
                    config.minimum_holdout_samples_per_cell
                ));
            }
        }
    }
    let mut profile = LensPsfProfile {
        schema: LENS_PSF_PROFILE_SCHEMA.to_string(),
        camera_make: set.camera_make.clone(),
        camera_model: set.camera_model.clone(),
        sensor_width: set.sensor_width,
        sensor_height: set.sensor_height,
        lens_model: set.lens_model.clone(),
        nominal_focal_length_mm: set.nominal_focal_length_mm,
        aperture: set.aperture,
        measurement_set_digest: format!(
            "sha256:{}",
            hex::encode(Sha256::digest(serde_json::to_vec(set)?))
        ),
        radius_knots: set.radius_knots.clone(),
        focus_knots,
        fit_measurements: u32::try_from(fit_measurements).unwrap_or(u32::MAX),
        holdout_measurements: u32::try_from(holdout.len()).unwrap_or(u32::MAX),
        ideal_holdout_p95_relative_error: 0.0,
        calibrated_holdout_p95_relative_error: 0.0,
        maximum_holdout_p95_relative_error: config.maximum_calibrated_p95_relative_error,
        minimum_holdout_p95_error_reduction: config.minimum_p95_error_reduction,
        calibration_id: "unpublished:lens-psf".to_string(),
    };
    let mut ideal_errors = Vec::with_capacity(holdout.len());
    let mut calibrated_errors = Vec::with_capacity(holdout.len());
    for measurement in holdout {
        let observed =
            measurement.observed_defocus_diameter_px * measurement.pixel_pitch_um * 0.001;
        let ideal = defocus_with_optics(
            set.nominal_focal_length_mm,
            set.nominal_focal_length_mm / set.aperture,
            measurement.focus_distance_m,
            measurement.subject_distance_m,
        );
        let calibrated = profile.defocus_circle_mm(
            measurement.focus_distance_m,
            measurement.subject_distance_m,
            measurement.field_radius,
        );
        let denominator = observed.max(1e-8);
        ideal_errors.push((ideal - observed).abs() / denominator);
        calibrated_errors.push((calibrated - observed).abs() / denominator);
    }
    let ideal_p95 = percentile(&mut ideal_errors, 0.95);
    let calibrated_p95 = percentile(&mut calibrated_errors, 0.95);
    let reduction = if ideal_p95 > 1e-8 {
        1.0 - calibrated_p95 / ideal_p95
    } else {
        0.0
    };
    profile.ideal_holdout_p95_relative_error = ideal_p95;
    profile.calibrated_holdout_p95_relative_error = calibrated_p95;
    if calibrated_p95 > config.maximum_calibrated_p95_relative_error {
        failures.push(format!(
            "calibrated holdout p95 {calibrated_p95:.6} exceeds {:.6}",
            config.maximum_calibrated_p95_relative_error
        ));
    }
    if reduction < config.minimum_p95_error_reduction {
        failures.push(format!(
            "holdout p95 reduction {reduction:.3} is below {:.3}",
            config.minimum_p95_error_reduction
        ));
    }
    if let Err(error) = profile.validate() {
        failures.push(format!("profile validation failed: {error:#}"));
    }
    let report = LensPsfCalibrationReport {
        passed: failures.is_empty(),
        fit_measurements,
        holdout_measurements: profile.holdout_measurements as usize,
        focus_knots: profile.focus_knots.len(),
        radius_knots: profile.radius_knots.len(),
        ideal_holdout_p95_relative_error: ideal_p95,
        calibrated_holdout_p95_relative_error: calibrated_p95,
        error_reduction: reduction,
        failures,
    };
    Ok((report.passed.then_some(profile), report))
}

fn validate_calibration_config(config: &LensPsfCalibrationConfig) -> Result<()> {
    if !(1..=32).contains(&config.minimum_fit_samples_per_cell)
        || !(1..=32).contains(&config.minimum_holdout_samples_per_cell)
        || !(4..=MAX_MEASUREMENTS).contains(&config.minimum_holdout_measurements)
        || !config.maximum_calibrated_p95_relative_error.is_finite()
        || !(0.005..=0.25).contains(&config.maximum_calibrated_p95_relative_error)
        || !config.minimum_p95_error_reduction.is_finite()
        || !(0.1..=0.95).contains(&config.minimum_p95_error_reduction)
        || !config.focus_group_relative_tolerance.is_finite()
        || !(0.0001..=0.02).contains(&config.focus_group_relative_tolerance)
    {
        anyhow::bail!("Lens PSF calibration configuration is invalid");
    }
    Ok(())
}

fn defocus_with_optics(
    effective_focal_length_mm: f32,
    entrance_pupil_mm: f32,
    focused_distance_m: f32,
    subject_distance_m: f32,
) -> f32 {
    let focused_image = image_distance_mm(effective_focal_length_mm, focused_distance_m);
    let subject_image = image_distance_mm(effective_focal_length_mm, subject_distance_m);
    entrance_pupil_mm * (focused_image - subject_image).abs() / subject_image.max(1e-8)
}

fn image_distance_mm(focal_length_mm: f32, object_distance_m: f32) -> f32 {
    let object_mm = object_distance_m * 1000.0;
    focal_length_mm * object_mm / (object_mm - focal_length_mm)
}

fn bracket_focus_diopters(knots: &[LensPsfFocusKnot], distance_m: f32) -> (usize, usize, f32) {
    if distance_m <= knots[0].focus_distance_m {
        return (0, 0, 0.0);
    }
    let last = knots.len() - 1;
    if distance_m >= knots[last].focus_distance_m {
        return (last, last, 0.0);
    }
    let upper = knots.partition_point(|knot| knot.focus_distance_m < distance_m);
    let lower = upper - 1;
    let value = 1.0 / distance_m;
    let left = 1.0 / knots[lower].focus_distance_m;
    let right = 1.0 / knots[upper].focus_distance_m;
    let fraction = ((value - left) / (right - left)).clamp(0.0, 1.0);
    (lower, upper, fraction)
}

fn interpolate_knots(coordinates: &[f32], values: &[f32], value: f32) -> f32 {
    if value <= coordinates[0] {
        return values[0];
    }
    let last = coordinates.len() - 1;
    if value >= coordinates[last] {
        return values[last];
    }
    let upper = coordinates.partition_point(|coordinate| *coordinate < value);
    let lower = upper - 1;
    let fraction = (value - coordinates[lower]) / (coordinates[upper] - coordinates[lower]);
    lerp(values[lower], values[upper], fraction)
}

fn normalized_field_radius(x: f32, y: f32, width: f32, height: f32) -> f32 {
    let center_x = (width - 1.0) * 0.5;
    let center_y = (height - 1.0) * 0.5;
    let radius = ((x - center_x).powi(2) + (y - center_y).powi(2)).sqrt();
    let corner = (center_x * center_x + center_y * center_y).sqrt().max(1e-8);
    (radius / corner).clamp(0.0, 1.0)
}

fn nearest_knot(knots: &[f32], value: f32) -> usize {
    knots
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| (*left - value).abs().total_cmp(&(*right - value).abs()))
        .map_or(0, |(index, _)| index)
}

fn median(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) * 0.5
    } else {
        values[middle]
    })
}

fn percentile(values: &mut [f32], quantile: f32) -> f32 {
    if values.is_empty() {
        return f32::INFINITY;
    }
    values.sort_by(f32::total_cmp);
    let index = ((values.len() - 1) as f32 * quantile).round() as usize;
    values[index.min(values.len() - 1)]
}

fn same_focus(left: f32, right: f32, tolerance: f32) -> bool {
    (left - right).abs() <= left.abs().max(right.abs()) * tolerance
}

fn relative_match(expected: f32, actual: f32, tolerance: f32) -> bool {
    expected.is_finite()
        && actual.is_finite()
        && (expected - actual).abs() <= expected.abs().max(actual.abs()) * tolerance
}

fn normalized_identity(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn lerp(left: f32, right: f32, fraction: f32) -> f32 {
    left + (right - left) * fraction
}

fn bounded_read(path: &Path, label: &str) -> Result<Vec<u8>> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_LENS_PSF_ARTIFACT_BYTES {
        anyhow::bail!(
            "{label} {} is {} bytes; limit is {}",
            path.display(),
            metadata.len(),
            MAX_LENS_PSF_ARTIFACT_BYTES
        );
    }
    Ok(std::fs::read(path)?)
}

fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec_pretty(value)?;
    if bytes.len() as u64 > MAX_LENS_PSF_ARTIFACT_BYTES {
        anyhow::bail!("Serialized lens PSF artifact exceeds the size limit");
    }
    let sequence = PARTIAL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let partial = path.with_extension(format!("partial-{}-{sequence}", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)?;
    let result = (|| -> Result<()> {
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        std::fs::hard_link(&partial, path).with_context(|| {
            format!(
                "Publish lens PSF artifact without replacing {}",
                path.display()
            )
        })?;
        std::fs::remove_file(&partial)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn synthetic_measurements() -> LensPsfMeasurementSet {
        let nominal_focal = 50.0;
        let aperture = 4.0;
        let radius_knots = vec![0.0, 0.5, 1.0];
        let mut measurements = Vec::new();
        for (focus_index, focus) in [0.5f32, 1.0, 2.0].into_iter().enumerate() {
            let effective = nominal_focal * (1.08 - focus_index as f32 * 0.03);
            let pupil = 1.18 - focus_index as f32 * 0.04;
            for (radius_index, radius) in radius_knots.iter().copied().enumerate() {
                let radial = 1.0 + radius * radius * 0.22;
                for sample in 0..4 {
                    let split = if sample < 2 {
                        CalibrationSplit::Fit
                    } else {
                        CalibrationSplit::Holdout
                    };
                    let subject = focus * if sample % 2 == 0 { 1.22 } else { 0.82 };
                    let ideal = defocus_with_optics(
                        effective,
                        nominal_focal / aperture * pupil,
                        focus,
                        subject,
                    );
                    let perturbation = if sample == 2 { 1.003 } else { 0.997 };
                    measurements.push(LensPsfMeasurement {
                        source_sha256: String::new(),
                        split,
                        focus_distance_m: focus,
                        focus_distance_source: None,
                        focus_distance_uncertainty_m: None,
                        subject_distance_m: subject,
                        field_radius: radius,
                        effective_focal_length_mm: effective,
                        observed_defocus_diameter_px: ideal * radial * perturbation / 0.00435,
                        pixel_pitch_um: 4.35,
                    });
                }
                assert_eq!(radius_index, nearest_knot(&radius_knots, radius));
            }
        }
        let sources = measurements
            .iter_mut()
            .enumerate()
            .map(|(index, measurement)| {
                let sha256 = format!("{:064x}", index + 1);
                measurement.source_sha256.clone_from(&sha256);
                LensPsfSourceRecord {
                    path: format!("retained/calibration-{index:04}.nef"),
                    sha256,
                }
            })
            .collect();
        LensPsfMeasurementSet {
            schema: LENS_PSF_MEASUREMENTS_SCHEMA.to_string(),
            camera_make: "NIKON CORPORATION".to_string(),
            camera_model: "NIKON Z 9".to_string(),
            sensor_width: 8256,
            sensor_height: 5504,
            lens_model: "NIKKOR Z 50mm f/1.8 S".to_string(),
            nominal_focal_length_mm: nominal_focal,
            aperture,
            target_id: "iso-12233-depth-target-v1".to_string(),
            measurement_method: "slanted_edge_esf_defocus_diameter_v1".to_string(),
            radius_knots,
            sources,
            measurements,
        }
    }

    fn prune_unreferenced_sources(set: &mut LensPsfMeasurementSet) {
        let referenced = set
            .measurements
            .iter()
            .map(|measurement| measurement.source_sha256.clone())
            .collect::<HashSet<_>>();
        set.sources
            .retain(|source| referenced.contains(&source.sha256));
    }

    #[test]
    fn calibrated_profile_recovers_breathing_pupil_and_field_psf_on_holdout() {
        let set = synthetic_measurements();
        let (profile, report) =
            calibrate_lens_psf(&set, &LensPsfCalibrationConfig::default()).unwrap();
        println!(
            "lens PSF synthetic holdout: ideal_p95={:.8} calibrated_p95={:.8} reduction={:.8}",
            report.ideal_holdout_p95_relative_error,
            report.calibrated_holdout_p95_relative_error,
            report.error_reduction
        );
        assert!(report.passed, "{:?}", report.failures);
        assert!(report.ideal_holdout_p95_relative_error > 0.15);
        assert!(report.calibrated_holdout_p95_relative_error < 0.01);
        assert!(report.error_reduction > 0.90);
        let profile = profile.unwrap();
        let optics = profile.optics_at(1.0, 1.0);
        assert!((optics.effective_focal_length_mm - 52.5).abs() < 0.01);
        assert!(optics.entrance_pupil_mm > 13.0);
        assert!((optics.radial_psf_scale - 1.22).abs() < 0.01);
    }

    #[test]
    fn profile_identity_and_focus_envelope_fail_closed() {
        let (profile, _) =
            calibrate_lens_psf(&synthetic_measurements(), &Default::default()).unwrap();
        let profile = profile.unwrap();
        assert!(profile.matches(
            "NIKON CORPORATION",
            "NIKON Z 9",
            8256,
            5504,
            "NIKKOR Z 50mm f/1.8 S",
            50.0,
            4.0,
            1.0
        ));
        assert!(!profile.matches(
            "NIKON CORPORATION",
            "NIKON Z 8",
            8256,
            5504,
            "NIKKOR Z 50mm f/1.8 S",
            50.0,
            4.0,
            1.0
        ));
        assert!(!profile.matches(
            "NIKON CORPORATION",
            "NIKON Z 9",
            8256,
            5504,
            "NIKKOR Z 50mm f/1.8 S",
            50.0,
            4.0,
            4.0
        ));

        let mut weak_profile = profile;
        weak_profile.calibrated_holdout_p95_relative_error =
            weak_profile.ideal_holdout_p95_relative_error * 0.9;
        assert!(weak_profile.validate().is_err());
    }

    #[test]
    fn published_profile_is_digest_bound_and_round_trips() {
        let (profile, _) =
            calibrate_lens_psf(&synthetic_measurements(), &Default::default()).unwrap();
        let directory = tempdir().unwrap();
        let path = directory.path().join("lens-psf.json");
        profile.unwrap().save_json(&path).unwrap();
        let loaded = LensPsfProfile::load_json(&path).unwrap();
        assert!(loaded.calibration_id.starts_with("sha256:"));
        assert_eq!(loaded.focus_knots.len(), 3);
        assert_eq!(loaded.radius_knots, vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn concurrent_profile_publication_never_clobbers() {
        use std::sync::{Arc, Barrier};

        let (profile, _) =
            calibrate_lens_psf(&synthetic_measurements(), &Default::default()).unwrap();
        let profile = Arc::new(profile.unwrap());
        let directory = tempdir().unwrap();
        let path = Arc::new(directory.path().join("lens-psf.json"));
        let barrier = Arc::new(Barrier::new(3));
        let handles = (0..2)
            .map(|_| {
                let profile = Arc::clone(&profile);
                let path = Arc::clone(&path);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    atomic_json(path.as_ref(), profile.as_ref())
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let successes = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(Result::is_ok)
            .count();
        assert_eq!(successes, 1);
        LensPsfProfile::load_json(&path).unwrap();
    }

    #[test]
    fn missing_fit_cell_prevents_publication() {
        let mut set = synthetic_measurements();
        set.measurements.retain(|measurement| {
            !(measurement.split == CalibrationSplit::Fit
                && measurement.focus_distance_m == 1.0
                && measurement.field_radius == 1.0)
        });
        prune_unreferenced_sources(&mut set);
        let (profile, report) =
            calibrate_lens_psf(&set, &LensPsfCalibrationConfig::default()).unwrap();
        assert!(profile.is_none());
        assert!(!report.passed);
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.contains("fit samples")));
    }

    #[test]
    fn measurement_sources_are_complete_unique_and_digest_bound() {
        let mut set = synthetic_measurements();
        set.measurements[0].source_sha256 = "0".repeat(64);
        assert!(set.validate().is_err());

        let mut set = synthetic_measurements();
        let duplicate = set.sources[0].sha256.clone();
        set.sources[1].sha256 = duplicate;
        assert!(set.validate().is_err());

        let mut set = synthetic_measurements();
        set.sources.clear();
        assert!(set.validate().is_err());

        let mut set = synthetic_measurements();
        set.sources.push(LensPsfSourceRecord {
            path: "retained/unreferenced.nef".to_string(),
            sha256: "f".repeat(64),
        });
        assert!(set.validate().is_err());

        let mut set = synthetic_measurements();
        set.measurements[1].source_sha256 = set.measurements[0].source_sha256.clone();
        set.measurements[1].split = match set.measurements[0].split {
            CalibrationSplit::Fit => CalibrationSplit::Holdout,
            CalibrationSplit::Holdout => CalibrationSplit::Fit,
        };
        assert!(set
            .validate()
            .unwrap_err()
            .to_string()
            .contains("disjoint retained sources"));

        let mut set = synthetic_measurements();
        set.measurements[0].focus_distance_source = Some("independent_measured".to_string());
        assert!(set.validate().is_err());

        let mut set = synthetic_measurements();
        set.measurements[0].focus_distance_source = Some("independent_measured".to_string());
        set.measurements[0].focus_distance_uncertainty_m = Some(0.001);
        assert!(set.validate().is_ok());
    }

    #[test]
    fn holdout_must_cover_every_fit_cell() {
        let mut set = synthetic_measurements();
        set.measurements.retain(|measurement| {
            !(measurement.split == CalibrationSplit::Holdout
                && measurement.focus_distance_m == 1.0
                && measurement.field_radius == 1.0)
        });
        prune_unreferenced_sources(&mut set);
        let (profile, report) =
            calibrate_lens_psf(&set, &LensPsfCalibrationConfig::default()).unwrap();
        assert!(profile.is_none());
        assert!(!report.passed);
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.contains("holdout samples")));
    }
}
