//! Retained-target extraction for lens breathing and field-PSF calibration.
//!
//! The extractor works directly on native linear Bayer samples. A capture plan
//! supplies source hashes, known target geometry, and bounded search ROIs;
//! optical measurements are fitted deterministically from the retained data.

use crate::lens_psf::{
    CalibrationSplit, LensPsfMeasurement, LensPsfMeasurementSet, LensPsfSourceRecord,
    LENS_PSF_MEASUREMENTS_SCHEMA, MAX_LENS_PSF_ARTIFACT_BYTES,
};
use crate::nef::parser::{SensorLevels, Z9Metadata, Z9NefParser};
use crate::nef::raw_data::{RawBuffer, Roi};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::f32::consts::{FRAC_PI_2, PI};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub const LENS_PSF_EXTRACTION_PLAN_SCHEMA: &str = "trueshot.lens-psf-extraction-plan.v1";
pub const LENS_PSF_EXTRACTION_REPORT_SCHEMA: &str = "trueshot.lens-psf-extraction-report.v1";
pub const LENS_PSF_EXTRACTION_METHOD: &str = "native_bayer_analytic_disk_esf_v1";

const MAX_CAPTURES: usize = 4_096;
const MAX_TARGETS_PER_CAPTURE: usize = 16;
const RADIUS_KNOT_TOLERANCE: f32 = 1e-4;
const ESF_OVERSAMPLING: f32 = 8.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LensPsfExtractionConfig {
    pub minimum_roi_edge: u32,
    pub maximum_roi_edge: u32,
    pub maximum_decoded_pixels_per_target: u64,
    pub minimum_green_samples: usize,
    pub minimum_contrast: f32,
    pub maximum_clipped_fraction: f32,
    pub minimum_gradient_coherence: f32,
    pub minimum_slant_degrees: f32,
    pub maximum_slant_degrees: f32,
    pub maximum_fit_residual_relative: f32,
    pub maximum_diameter_relative_uncertainty: f32,
    pub maximum_edge_pair_diameter_disagreement: f32,
    pub maximum_parallel_angle_degrees: f32,
    pub maximum_radius_knot_deviation: f32,
    pub maximum_focus_metadata_relative_disagreement: f32,
    pub maximum_focus_distance_relative_uncertainty: f32,
    pub maximum_defocus_radius_px: f32,
    pub minimum_edge_separation_px: f32,
}

impl Default for LensPsfExtractionConfig {
    fn default() -> Self {
        Self {
            minimum_roi_edge: 32,
            maximum_roi_edge: 768,
            maximum_decoded_pixels_per_target: 1_048_576,
            minimum_green_samples: 256,
            minimum_contrast: 0.08,
            maximum_clipped_fraction: 0.05,
            minimum_gradient_coherence: 0.60,
            minimum_slant_degrees: 1.5,
            maximum_slant_degrees: 20.0,
            maximum_fit_residual_relative: 0.05,
            maximum_diameter_relative_uncertainty: 0.15,
            maximum_edge_pair_diameter_disagreement: 0.12,
            maximum_parallel_angle_degrees: 1.5,
            maximum_radius_knot_deviation: 0.05,
            maximum_focus_metadata_relative_disagreement: 0.10,
            maximum_focus_distance_relative_uncertainty: 0.02,
            maximum_defocus_radius_px: 64.0,
            minimum_edge_separation_px: 8.0,
        }
    }
}

impl LensPsfExtractionConfig {
    pub fn validate(&self) -> Result<()> {
        if !(16..=512).contains(&self.minimum_roi_edge)
            || self.maximum_roi_edge < self.minimum_roi_edge
            || self.maximum_roi_edge > 2_048
            || !(1_024..=16_777_216).contains(&self.maximum_decoded_pixels_per_target)
            || !(64..=1_000_000).contains(&self.minimum_green_samples)
            || !unit_interval(self.minimum_contrast)
            || self.minimum_contrast < 0.01
            || !unit_interval(self.maximum_clipped_fraction)
            || self.maximum_clipped_fraction > 0.25
            || !unit_interval(self.minimum_gradient_coherence)
            || self.minimum_gradient_coherence < 0.1
            || !self.minimum_slant_degrees.is_finite()
            || !self.maximum_slant_degrees.is_finite()
            || self.minimum_slant_degrees < 0.5
            || self.maximum_slant_degrees > 30.0
            || self.minimum_slant_degrees >= self.maximum_slant_degrees
            || !unit_interval(self.maximum_fit_residual_relative)
            || self.maximum_fit_residual_relative > 0.25
            || !unit_interval(self.maximum_diameter_relative_uncertainty)
            || self.maximum_diameter_relative_uncertainty > 0.5
            || !unit_interval(self.maximum_edge_pair_diameter_disagreement)
            || self.maximum_edge_pair_diameter_disagreement > 0.5
            || !self.maximum_parallel_angle_degrees.is_finite()
            || !(0.1..=5.0).contains(&self.maximum_parallel_angle_degrees)
            || !unit_interval(self.maximum_radius_knot_deviation)
            || self.maximum_radius_knot_deviation > 0.25
            || !unit_interval(self.maximum_focus_metadata_relative_disagreement)
            || self.maximum_focus_metadata_relative_disagreement > 0.5
            || !unit_interval(self.maximum_focus_distance_relative_uncertainty)
            || self.maximum_focus_distance_relative_uncertainty > 0.25
            || !self.maximum_defocus_radius_px.is_finite()
            || !(1.0..=256.0).contains(&self.maximum_defocus_radius_px)
            || !self.minimum_edge_separation_px.is_finite()
            || !(2.0..=1_024.0).contains(&self.minimum_edge_separation_px)
        {
            anyhow::bail!("Lens PSF extraction configuration is invalid");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensorRoi {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl SensorRoi {
    fn right(self) -> Option<u32> {
        self.x.checked_add(self.width)
    }

    fn bottom(self) -> Option<u32> {
        self.y.checked_add(self.height)
    }

    fn validate(
        self,
        sensor_width: u32,
        sensor_height: u32,
        config: &LensPsfExtractionConfig,
    ) -> Result<()> {
        if !(config.minimum_roi_edge..=config.maximum_roi_edge).contains(&self.width)
            || !(config.minimum_roi_edge..=config.maximum_roi_edge).contains(&self.height)
            || self.right().map_or(true, |right| right > sensor_width)
            || self.bottom().map_or(true, |bottom| bottom > sensor_height)
        {
            anyhow::bail!("Lens PSF edge ROI is empty, oversized, or outside the sensor");
        }
        Ok(())
    }

    fn union(self, other: Self) -> Result<Self> {
        let right = self
            .right()
            .context("Lens PSF ROI horizontal extent overflow")?
            .max(
                other
                    .right()
                    .context("Lens PSF ROI horizontal extent overflow")?,
            );
        let bottom = self
            .bottom()
            .context("Lens PSF ROI vertical extent overflow")?
            .max(
                other
                    .bottom()
                    .context("Lens PSF ROI vertical extent overflow")?,
            );
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Ok(Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        })
    }

    fn center(self) -> [f32; 2] {
        [
            self.x as f32 + (self.width as f32 - 1.0) * 0.5,
            self.y as f32 + (self.height as f32 - 1.0) * 0.5,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LensPsfEdgePairTarget {
    pub target_id: String,
    pub radius_knot: f32,
    /// Known target-plane separation perpendicular to the two printed edges.
    pub projected_edge_separation_mm: f32,
    pub first_edge_roi: SensorRoi,
    pub second_edge_roi: SensorRoi,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LensPsfExtractionCapture {
    /// Relative path below the CLI-provided capture root.
    pub path: String,
    pub sha256: String,
    pub split: CalibrationSplit,
    /// Independently measured lens focus distance. Prefer this over quantized camera telemetry.
    #[serde(default)]
    pub measured_focus_distance_m: Option<f32>,
    /// One-sigma uncertainty for `measured_focus_distance_m`.
    #[serde(default)]
    pub measured_focus_distance_uncertainty_m: Option<f32>,
    pub subject_distance_m: f32,
    pub targets: Vec<LensPsfEdgePairTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LensPsfExtractionPlan {
    pub schema: String,
    pub target_id: String,
    pub nominal_focal_length_mm: f32,
    pub aperture: f32,
    pub radius_knots: Vec<f32>,
    #[serde(default)]
    pub config: LensPsfExtractionConfig,
    pub captures: Vec<LensPsfExtractionCapture>,
}

impl LensPsfExtractionPlan {
    pub fn load_json(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path)?;
        if metadata.len() > MAX_LENS_PSF_ARTIFACT_BYTES {
            anyhow::bail!("Lens PSF extraction plan exceeds the artifact size limit");
        }
        let plan: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<()> {
        self.config.validate()?;
        if self.schema != LENS_PSF_EXTRACTION_PLAN_SCHEMA
            || self.target_id.trim().is_empty()
            || !self.nominal_focal_length_mm.is_finite()
            || !(1.0..=2_000.0).contains(&self.nominal_focal_length_mm)
            || !self.aperture.is_finite()
            || !(0.5..=128.0).contains(&self.aperture)
            || !(2..=MAX_TARGETS_PER_CAPTURE).contains(&self.radius_knots.len())
            || self.captures.is_empty()
            || self.captures.len() > MAX_CAPTURES
        {
            anyhow::bail!("Lens PSF extraction plan identity or dimensions are invalid");
        }
        if self.radius_knots[0].abs() > RADIUS_KNOT_TOLERANCE
            || (self.radius_knots[self.radius_knots.len() - 1] - 1.0).abs() > RADIUS_KNOT_TOLERANCE
            || self
                .radius_knots
                .iter()
                .any(|radius| !radius.is_finite() || !(0.0..=1.0).contains(radius))
            || self
                .radius_knots
                .windows(2)
                .any(|pair| pair[1] - pair[0] <= RADIUS_KNOT_TOLERANCE)
        {
            anyhow::bail!("Lens PSF extraction radius knots must increase from zero to one");
        }
        let mut paths = HashSet::with_capacity(self.captures.len());
        let mut hashes = HashSet::with_capacity(self.captures.len());
        let mut fit = 0usize;
        let mut holdout = 0usize;
        for capture in &self.captures {
            validate_relative_path(&capture.path)?;
            if !valid_sha256(&capture.sha256)
                || !paths.insert(capture.path.as_str())
                || !hashes.insert(capture.sha256.as_str())
                || !valid_measured_focus(capture, self)
                || !capture.subject_distance_m.is_finite()
                || capture.subject_distance_m <= self.nominal_focal_length_mm * 0.001 * 1.01
                || capture.targets.len() != self.radius_knots.len()
            {
                anyhow::bail!("Lens PSF extraction capture provenance or geometry is invalid");
            }
            match capture.split {
                CalibrationSplit::Fit => fit += 1,
                CalibrationSplit::Holdout => holdout += 1,
            }
            let mut target_ids = HashSet::with_capacity(capture.targets.len());
            let mut target_radii = Vec::with_capacity(capture.targets.len());
            for target in &capture.targets {
                if target.target_id.trim().is_empty()
                    || !target_ids.insert(target.target_id.as_str())
                    || !target.radius_knot.is_finite()
                    || nearest_knot(&self.radius_knots, target.radius_knot).map_or(true, |index| {
                        (target.radius_knot - self.radius_knots[index]).abs()
                            > RADIUS_KNOT_TOLERANCE
                    })
                    || !target.projected_edge_separation_mm.is_finite()
                    || !(0.1..=1_000.0).contains(&target.projected_edge_separation_mm)
                {
                    anyhow::bail!("Lens PSF extraction target identity or geometry is invalid");
                }
                target_radii.push(target.radius_knot);
            }
            target_radii.sort_by(f32::total_cmp);
            if target_radii
                .iter()
                .zip(&self.radius_knots)
                .any(|(actual, expected)| (actual - expected).abs() > RADIUS_KNOT_TOLERANCE)
            {
                anyhow::bail!("Every capture must contain each declared radius knot exactly once");
            }
        }
        if fit < 2 || holdout < 2 {
            anyhow::bail!("Lens PSF extraction requires at least two fit and two holdout captures");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LensPsfEdgeDiagnostic {
    pub edge: String,
    pub accepted_green_samples: usize,
    pub clipped_fraction: f32,
    pub contrast: f32,
    pub normal_angle_degrees: f32,
    pub slant_degrees: f32,
    pub gradient_coherence: f32,
    pub diameter_px: f32,
    pub diameter_uncertainty_px: f32,
    pub fit_residual_relative: f32,
    pub model_mtf50_cycles_per_pixel: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LensPsfTargetDiagnostic {
    pub capture_path: String,
    pub target_id: String,
    pub radius_knot: f32,
    pub measured_field_radius: f32,
    pub decoded_roi: SensorRoi,
    pub decoded_pixels: u64,
    pub edge_separation_px: f32,
    pub effective_focal_length_mm: f32,
    pub observed_defocus_diameter_px: f32,
    pub diameter_uncertainty_px: f32,
    pub parallel_angle_degrees: f32,
    pub pair_diameter_disagreement: f32,
    pub first_edge: LensPsfEdgeDiagnostic,
    pub second_edge: LensPsfEdgeDiagnostic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LensPsfFocusDistanceDiagnostic {
    pub capture_path: String,
    pub selected_distance_m: f32,
    pub selected_source: String,
    pub selected_uncertainty_m: Option<f32>,
    pub metadata_distance_m: Option<f32>,
    pub metadata_relative_disagreement: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LensPsfExtractionReport {
    pub schema: String,
    pub passed: bool,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    pub sensor_width: Option<u32>,
    pub sensor_height: Option<u32>,
    pub captures: usize,
    pub measurements: usize,
    pub decoded_pixels: u64,
    pub full_frame_equivalent_pixels: u64,
    pub decode_fraction: f64,
    pub focus_distances: Vec<LensPsfFocusDistanceDiagnostic>,
    pub diagnostics: Vec<LensPsfTargetDiagnostic>,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone)]
struct ExtractionIdentity {
    camera_make: String,
    camera_model: String,
    sensor_width: u32,
    sensor_height: u32,
    lens_model: String,
    pixel_pitch_um: f32,
}

#[derive(Debug, Clone, Copy)]
struct EdgeFit {
    normal: [f32; 2],
    point: [f32; 2],
    diameter_px: f32,
    uncertainty_px: f32,
}

#[derive(Debug, Clone, Copy)]
struct EsfSample {
    x: f32,
    y: f32,
    value: f32,
}

#[derive(Debug, Clone, Copy)]
struct EsfPoint {
    position: f32,
    value: f32,
    weight: f32,
}

#[derive(Debug, Clone, Copy)]
struct FitCore {
    center: f32,
    radius: f32,
    contrast: f32,
    residual_relative: f32,
}

pub fn extract_lens_psf_measurements(
    plan: &LensPsfExtractionPlan,
    capture_root: &Path,
) -> Result<(Option<LensPsfMeasurementSet>, LensPsfExtractionReport)> {
    plan.validate()?;
    let root = capture_root
        .canonicalize()
        .with_context(|| format!("Resolve capture root {}", capture_root.display()))?;
    let mut report = LensPsfExtractionReport {
        schema: LENS_PSF_EXTRACTION_REPORT_SCHEMA.to_string(),
        passed: false,
        camera_make: None,
        camera_model: None,
        lens_model: None,
        sensor_width: None,
        sensor_height: None,
        captures: plan.captures.len(),
        measurements: 0,
        decoded_pixels: 0,
        full_frame_equivalent_pixels: 0,
        decode_fraction: 0.0,
        focus_distances: Vec::with_capacity(plan.captures.len()),
        diagnostics: Vec::new(),
        failures: Vec::new(),
    };
    let mut identity = None::<ExtractionIdentity>;
    let mut sources = Vec::with_capacity(plan.captures.len());
    let mut measurements = Vec::with_capacity(plan.captures.len() * plan.radius_knots.len());

    for capture in &plan.captures {
        let source_path = resolve_capture_path(&root, &capture.path)?;
        let digest = sha256_file(&source_path)?;
        if digest != capture.sha256 {
            anyhow::bail!(
                "Retained source digest mismatch for {}: expected {}, measured {}",
                capture.path,
                capture.sha256,
                digest
            );
        }
        let mut parser = Z9NefParser::new(&source_path);
        parser
            .parse()
            .with_context(|| format!("Parse retained lens target {}", source_path.display()))?;
        let metadata = parser.get_metadata()?.clone();
        let current = extraction_identity(&metadata, plan)?;
        if let Some(reference) = &identity {
            validate_extraction_identity(reference, &current, &metadata, plan, &capture.path)?;
        } else {
            report.camera_make = Some(current.camera_make.clone());
            report.camera_model = Some(current.camera_model.clone());
            report.lens_model = Some(current.lens_model.clone());
            report.sensor_width = Some(current.sensor_width);
            report.sensor_height = Some(current.sensor_height);
            identity = Some(current.clone());
        }
        let focus = select_focus_distance(capture, metadata.focus_distance, &plan.config)
            .with_context(|| format!("Resolve focus distance for {}", capture.path))?;
        let focus_distance_m = focus.selected_distance_m;
        let focus_distance_source = focus.selected_source.clone();
        let focus_distance_uncertainty_m = focus.selected_uncertainty_m;
        report.focus_distances.push(focus);
        report.full_frame_equivalent_pixels = report
            .full_frame_equivalent_pixels
            .saturating_add(u64::from(metadata.width) * u64::from(metadata.height));
        sources.push(LensPsfSourceRecord {
            path: capture.path.clone(),
            sha256: digest,
        });

        for target in &capture.targets {
            let result = extract_target(
                &parser,
                &metadata,
                target,
                capture.subject_distance_m,
                &capture.path,
                &plan.config,
            );
            match result {
                Ok(diagnostic) => {
                    report.decoded_pixels = report
                        .decoded_pixels
                        .saturating_add(diagnostic.decoded_pixels);
                    measurements.push(LensPsfMeasurement {
                        source_sha256: capture.sha256.clone(),
                        split: capture.split,
                        focus_distance_m,
                        focus_distance_source: Some(focus_distance_source.clone()),
                        focus_distance_uncertainty_m,
                        subject_distance_m: capture.subject_distance_m,
                        field_radius: target.radius_knot,
                        effective_focal_length_mm: diagnostic.effective_focal_length_mm,
                        observed_defocus_diameter_px: diagnostic.observed_defocus_diameter_px,
                        pixel_pitch_um: current.pixel_pitch_um,
                    });
                    report.diagnostics.push(diagnostic);
                }
                Err(error) => report.failures.push(format!(
                    "{} target {} failed: {error:#}",
                    capture.path, target.target_id
                )),
            }
        }
    }

    report.measurements = measurements.len();
    report.decode_fraction = if report.full_frame_equivalent_pixels == 0 {
        0.0
    } else {
        report.decoded_pixels as f64 / report.full_frame_equivalent_pixels as f64
    };
    if !report.failures.is_empty() {
        return Ok((None, report));
    }
    let identity = identity.context("Lens PSF extraction produced no camera identity")?;
    let set = LensPsfMeasurementSet {
        schema: LENS_PSF_MEASUREMENTS_SCHEMA.to_string(),
        camera_make: identity.camera_make,
        camera_model: identity.camera_model,
        sensor_width: identity.sensor_width,
        sensor_height: identity.sensor_height,
        lens_model: identity.lens_model,
        nominal_focal_length_mm: plan.nominal_focal_length_mm,
        aperture: plan.aperture,
        target_id: plan.target_id.clone(),
        measurement_method: LENS_PSF_EXTRACTION_METHOD.to_string(),
        radius_knots: plan.radius_knots.clone(),
        sources,
        measurements,
    };
    set.validate()?;
    report.passed = true;
    Ok((Some(set), report))
}

fn valid_measured_focus(capture: &LensPsfExtractionCapture, plan: &LensPsfExtractionPlan) -> bool {
    match (
        capture.measured_focus_distance_m,
        capture.measured_focus_distance_uncertainty_m,
    ) {
        (None, None) => true,
        (Some(distance), Some(uncertainty)) => {
            distance.is_finite()
                && distance > plan.nominal_focal_length_mm * 0.001 * 1.01
                && uncertainty.is_finite()
                && uncertainty > 0.0
                && uncertainty / distance <= plan.config.maximum_focus_distance_relative_uncertainty
        }
        _ => false,
    }
}

fn select_focus_distance(
    capture: &LensPsfExtractionCapture,
    metadata_distance_m: Option<f32>,
    config: &LensPsfExtractionConfig,
) -> Result<LensPsfFocusDistanceDiagnostic> {
    let metadata_distance_m =
        metadata_distance_m.filter(|distance| distance.is_finite() && *distance > 0.0);
    if let (Some(distance), Some(uncertainty)) = (
        capture.measured_focus_distance_m,
        capture.measured_focus_distance_uncertainty_m,
    ) {
        let disagreement = metadata_distance_m.map(|metadata| {
            (metadata - distance).abs() / metadata.abs().max(distance.abs()).max(f32::EPSILON)
        });
        if disagreement
            .is_some_and(|relative| relative > config.maximum_focus_metadata_relative_disagreement)
        {
            anyhow::bail!(
                "independent focus distance disagrees with camera metadata by {:.3}%, above the {:.3}% gate",
                disagreement.unwrap_or_default() * 100.0,
                config.maximum_focus_metadata_relative_disagreement * 100.0
            );
        }
        return Ok(LensPsfFocusDistanceDiagnostic {
            capture_path: capture.path.clone(),
            selected_distance_m: distance,
            selected_source: "independent_measured".to_string(),
            selected_uncertainty_m: Some(uncertainty),
            metadata_distance_m,
            metadata_relative_disagreement: disagreement,
        });
    }
    let distance = metadata_distance_m
        .with_context(|| format!("Capture {} has no usable focus distance", capture.path))?;
    Ok(LensPsfFocusDistanceDiagnostic {
        capture_path: capture.path.clone(),
        selected_distance_m: distance,
        selected_source: "exif_subject_distance".to_string(),
        selected_uncertainty_m: None,
        metadata_distance_m: Some(distance),
        metadata_relative_disagreement: Some(0.0),
    })
}

fn extraction_identity(
    metadata: &Z9Metadata,
    plan: &LensPsfExtractionPlan,
) -> Result<ExtractionIdentity> {
    let focal = metadata
        .focal_length
        .context("Lens PSF extraction requires focal-length metadata")?;
    let aperture = metadata
        .aperture
        .context("Lens PSF extraction requires aperture metadata")?;
    let lens_model = metadata
        .lens_model
        .clone()
        .context("Lens PSF extraction requires lens-model metadata")?;
    let geometry = metadata
        .sensor_geometry
        .context("Lens PSF extraction requires verified sensor pitch")?;
    if !relative_match(plan.nominal_focal_length_mm, focal, 0.005)
        || !relative_match(plan.aperture, aperture, 0.01)
        || metadata.cfa_pattern != [0, 1, 1, 2]
    {
        anyhow::bail!("Capture optics or CFA do not match the extraction plan");
    }
    Ok(ExtractionIdentity {
        camera_make: metadata.camera_make.clone(),
        camera_model: metadata.camera_model.clone(),
        sensor_width: metadata.width,
        sensor_height: metadata.height,
        lens_model,
        pixel_pitch_um: geometry.pixel_pitch_um,
    })
}

fn validate_extraction_identity(
    reference: &ExtractionIdentity,
    current: &ExtractionIdentity,
    metadata: &Z9Metadata,
    plan: &LensPsfExtractionPlan,
    path: &str,
) -> Result<()> {
    if normalize_identity(&reference.camera_make) != normalize_identity(&current.camera_make)
        || normalize_identity(&reference.camera_model) != normalize_identity(&current.camera_model)
        || normalize_identity(&reference.lens_model) != normalize_identity(&current.lens_model)
        || reference.sensor_width != current.sensor_width
        || reference.sensor_height != current.sensor_height
        || !relative_match(reference.pixel_pitch_um, current.pixel_pitch_um, 0.001)
        || !relative_match(
            plan.nominal_focal_length_mm,
            metadata.focal_length.unwrap_or_default(),
            0.005,
        )
        || !relative_match(plan.aperture, metadata.aperture.unwrap_or_default(), 0.01)
    {
        anyhow::bail!("Capture {path} does not match the retained camera/lens identity");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn extract_target(
    parser: &Z9NefParser,
    metadata: &Z9Metadata,
    target: &LensPsfEdgePairTarget,
    subject_distance_m: f32,
    capture_path: &str,
    config: &LensPsfExtractionConfig,
) -> Result<LensPsfTargetDiagnostic> {
    target
        .first_edge_roi
        .validate(metadata.width, metadata.height, config)?;
    target
        .second_edge_roi
        .validate(metadata.width, metadata.height, config)?;
    let decoded_roi = target.first_edge_roi.union(target.second_edge_roi)?;
    let decoded_pixels = u64::from(decoded_roi.width) * u64::from(decoded_roi.height);
    if decoded_pixels > config.maximum_decoded_pixels_per_target {
        anyhow::bail!(
            "Target ROI union has {decoded_pixels} pixels; limit is {}",
            config.maximum_decoded_pixels_per_target
        );
    }
    let raw = parser.load_roi(
        &Roi::new(
            decoded_roi.x,
            decoded_roi.y,
            decoded_roi.width,
            decoded_roi.height,
        ),
        None,
    )?;
    let levels = metadata
        .sensor_levels
        .context("Lens PSF extraction requires verified black/white levels")?;
    let first = fit_slanted_edge(
        &raw,
        [decoded_roi.x, decoded_roi.y],
        target.first_edge_roi,
        levels,
        metadata.cfa_pattern,
        "first",
        config,
    )?;
    let second = fit_slanted_edge(
        &raw,
        [decoded_roi.x, decoded_roi.y],
        target.second_edge_roi,
        levels,
        metadata.cfa_pattern,
        "second",
        config,
    )?;
    let (normal, parallel_angle_degrees) = common_normal(first.0.normal, second.0.normal);
    if parallel_angle_degrees > config.maximum_parallel_angle_degrees {
        anyhow::bail!(
            "Edge pair differs by {parallel_angle_degrees:.3} degrees; limit is {:.3}",
            config.maximum_parallel_angle_degrees
        );
    }
    let delta = [
        second.0.point[0] - first.0.point[0],
        second.0.point[1] - first.0.point[1],
    ];
    let edge_separation_px = dot(normal, delta).abs();
    if edge_separation_px < config.minimum_edge_separation_px {
        anyhow::bail!("Measured edge separation is too small");
    }
    let pitch_mm = metadata
        .sensor_geometry
        .context("Lens PSF extraction requires sensor geometry")?
        .pixel_pitch_um
        * 0.001;
    let magnification = edge_separation_px * pitch_mm / target.projected_edge_separation_mm;
    let subject_mm = subject_distance_m * 1_000.0;
    let effective_focal_length_mm = magnification * subject_mm / (1.0 + magnification);
    if !effective_focal_length_mm.is_finite()
        || !(metadata.focal_length.unwrap_or_default() * 0.5
            ..=metadata.focal_length.unwrap_or_default() * 1.5)
            .contains(&effective_focal_length_mm)
    {
        anyhow::bail!(
            "Geometry-derived effective focal length {effective_focal_length_mm:.3} mm is implausible"
        );
    }
    let observed_defocus_diameter_px = (first.0.diameter_px + second.0.diameter_px) * 0.5;
    let pair_diameter_disagreement =
        (first.0.diameter_px - second.0.diameter_px).abs() / observed_defocus_diameter_px.max(1e-8);
    if pair_diameter_disagreement > config.maximum_edge_pair_diameter_disagreement {
        anyhow::bail!(
            "Edge-pair diameter disagreement {pair_diameter_disagreement:.4} exceeds {:.4}",
            config.maximum_edge_pair_diameter_disagreement
        );
    }
    let diameter_uncertainty_px =
        0.5 * (first.0.uncertainty_px.powi(2) + second.0.uncertainty_px.powi(2)).sqrt();
    if diameter_uncertainty_px / observed_defocus_diameter_px.max(1e-8)
        > config.maximum_diameter_relative_uncertainty
    {
        anyhow::bail!("Combined defocus-diameter uncertainty exceeds the declared gate");
    }
    let midpoint = [
        (first.0.point[0] + second.0.point[0]) * 0.5,
        (first.0.point[1] + second.0.point[1]) * 0.5,
    ];
    let measured_field_radius =
        normalized_field_radius(midpoint[0], midpoint[1], metadata.width, metadata.height);
    if (measured_field_radius - target.radius_knot).abs() > config.maximum_radius_knot_deviation {
        anyhow::bail!(
            "Measured field radius {measured_field_radius:.4} differs from knot {:.4} by more than {:.4}",
            target.radius_knot,
            config.maximum_radius_knot_deviation
        );
    }
    let diagnostic = LensPsfTargetDiagnostic {
        capture_path: capture_path.to_string(),
        target_id: target.target_id.clone(),
        radius_knot: target.radius_knot,
        measured_field_radius,
        decoded_roi,
        decoded_pixels,
        edge_separation_px,
        effective_focal_length_mm,
        observed_defocus_diameter_px,
        diameter_uncertainty_px,
        parallel_angle_degrees,
        pair_diameter_disagreement,
        first_edge: first.1,
        second_edge: second.1,
    };
    Ok(diagnostic)
}

#[allow(clippy::too_many_arguments)]
fn fit_slanted_edge(
    raw: &RawBuffer,
    raw_origin: [u32; 2],
    roi: SensorRoi,
    levels: SensorLevels,
    cfa_pattern: [u8; 4],
    edge_name: &str,
    config: &LensPsfExtractionConfig,
) -> Result<(EdgeFit, LensPsfEdgeDiagnostic)> {
    let range = f32::from(levels.white.saturating_sub(levels.black));
    if range <= 0.0 {
        anyhow::bail!("Sensor black/white range is invalid");
    }
    let mut samples = Vec::with_capacity((roi.width as usize * roi.height as usize) / 2);
    let mut clipped = 0usize;
    let mut green = 0usize;
    for sensor_y in roi.y..roi.bottom().context("ROI overflow")? {
        for sensor_x in roi.x..roi.right().context("ROI overflow")? {
            if cfa_color(cfa_pattern, sensor_x, sensor_y) != 1 {
                continue;
            }
            green += 1;
            let value = raw_value(raw, raw_origin, sensor_x, sensor_y)
                .context("Edge ROI is outside its decoded Bayer union")?;
            if value <= levels.black.saturating_add(2) || value >= levels.white.saturating_sub(2) {
                clipped += 1;
                continue;
            }
            samples.push(EsfSample {
                x: sensor_x as f32,
                y: sensor_y as f32,
                value: (f32::from(value) - f32::from(levels.black)) / range,
            });
        }
    }
    if samples.len() < config.minimum_green_samples {
        anyhow::bail!(
            "{} accepted green samples; require {}",
            samples.len(),
            config.minimum_green_samples
        );
    }
    let clipped_fraction = clipped as f32 / green.max(1) as f32;
    if clipped_fraction > config.maximum_clipped_fraction {
        anyhow::bail!(
            "Clipped fraction {clipped_fraction:.4} exceeds {:.4}",
            config.maximum_clipped_fraction
        );
    }
    let (initial_normal, coherence) =
        estimate_edge_normal(raw, raw_origin, roi, levels, cfa_pattern)?;
    if coherence < config.minimum_gradient_coherence {
        anyhow::bail!(
            "Gradient coherence {coherence:.4} is below {:.4}",
            config.minimum_gradient_coherence
        );
    }
    let (normal, fit) = refine_edge_normal(&samples, initial_normal, config)?;
    let slant_degrees = edge_slant_degrees(normal);
    if !(config.minimum_slant_degrees..=config.maximum_slant_degrees).contains(&slant_degrees) {
        anyhow::bail!(
            "Edge slant {slant_degrees:.3} degrees is outside {:.3}-{:.3}",
            config.minimum_slant_degrees,
            config.maximum_slant_degrees
        );
    }
    if fit.residual_relative > config.maximum_fit_residual_relative {
        anyhow::bail!(
            "Relative analytic disk fit residual {:.5} exceeds {:.5}",
            fit.residual_relative,
            config.maximum_fit_residual_relative
        );
    }
    let tangent = [-normal[1], normal[0]];
    let mut ordered = samples
        .iter()
        .map(|sample| dot(tangent, [sample.x, sample.y]))
        .collect::<Vec<_>>();
    ordered.sort_by(f32::total_cmp);
    let mut segment_diameters = Vec::new();
    for segment in 0..4 {
        let start = ordered[ordered.len() * segment / 4];
        let end = ordered[(ordered.len() * (segment + 1) / 4).min(ordered.len() - 1)];
        let segment_samples = samples
            .iter()
            .copied()
            .filter(|sample| {
                let coordinate = dot(tangent, [sample.x, sample.y]);
                coordinate >= start && coordinate <= end
            })
            .collect::<Vec<_>>();
        if segment_samples.len() >= config.minimum_green_samples / 8 {
            if let Ok(segment_fit) = fit_projected_esf(&segment_samples, normal, config) {
                segment_diameters.push(segment_fit.radius * 2.0);
            }
        }
    }
    if segment_diameters.len() < 3 {
        anyhow::bail!("Too few along-edge segments support uncertainty estimation");
    }
    let diameter_px = fit.radius * 2.0;
    let uncertainty_px = robust_sigma(&mut segment_diameters);
    if uncertainty_px / diameter_px.max(1e-8) > config.maximum_diameter_relative_uncertainty {
        anyhow::bail!("Defocus-diameter uncertainty exceeds the declared gate");
    }
    let roi_center = roi.center();
    let projection = dot(normal, roi_center);
    let point = [
        roi_center[0] + normal[0] * (fit.center - projection),
        roi_center[1] + normal[1] * (fit.center - projection),
    ];
    let normal_angle_degrees = normal[1].atan2(normal[0]).to_degrees();
    let diagnostic = LensPsfEdgeDiagnostic {
        edge: edge_name.to_string(),
        accepted_green_samples: samples.len(),
        clipped_fraction,
        contrast: fit.contrast.abs(),
        normal_angle_degrees,
        slant_degrees,
        gradient_coherence: coherence,
        diameter_px,
        diameter_uncertainty_px: uncertainty_px,
        fit_residual_relative: fit.residual_relative,
        model_mtf50_cycles_per_pixel: disk_mtf50(diameter_px),
    };
    Ok((
        EdgeFit {
            normal,
            point,
            diameter_px,
            uncertainty_px,
        },
        diagnostic,
    ))
}

fn refine_edge_normal(
    samples: &[EsfSample],
    initial_normal: [f32; 2],
    config: &LensPsfExtractionConfig,
) -> Result<([f32; 2], FitCore)> {
    let mut angle = initial_normal[1].atan2(initial_normal[0]);
    let mut normal = initial_normal;
    let mut fit = fit_projected_esf(samples, normal, config)?;
    for step_degrees in [0.20f32, 0.04] {
        let step = step_degrees.to_radians();
        let center = angle;
        for offset in -5..=5 {
            let candidate_angle = center + offset as f32 * step;
            let candidate_normal = [candidate_angle.cos(), candidate_angle.sin()];
            let candidate_fit = fit_projected_esf(samples, candidate_normal, config)?;
            if candidate_fit.residual_relative < fit.residual_relative {
                angle = candidate_angle;
                normal = candidate_normal;
                fit = candidate_fit;
            }
        }
    }
    Ok((normal, fit))
}

fn estimate_edge_normal(
    raw: &RawBuffer,
    raw_origin: [u32; 2],
    roi: SensorRoi,
    levels: SensorLevels,
    cfa_pattern: [u8; 4],
) -> Result<([f32; 2], f32)> {
    let range = f32::from(levels.white.saturating_sub(levels.black)).max(1.0);
    let mut xx = 0.0f64;
    let mut xy = 0.0f64;
    let mut yy = 0.0f64;
    let left = roi.x.saturating_add(2);
    let top = roi.y.saturating_add(2);
    let right = roi.right().context("ROI overflow")?.saturating_sub(2);
    let bottom = roi.bottom().context("ROI overflow")?.saturating_sub(2);
    for sensor_y in top..bottom {
        for sensor_x in left..right {
            if cfa_color(cfa_pattern, sensor_x, sensor_y) != 1 {
                continue;
            }
            let Some(x0) = raw_value(raw, raw_origin, sensor_x - 2, sensor_y) else {
                continue;
            };
            let Some(x1) = raw_value(raw, raw_origin, sensor_x + 2, sensor_y) else {
                continue;
            };
            let Some(y0) = raw_value(raw, raw_origin, sensor_x, sensor_y - 2) else {
                continue;
            };
            let Some(y1) = raw_value(raw, raw_origin, sensor_x, sensor_y + 2) else {
                continue;
            };
            let gx = (f32::from(x1) - f32::from(x0)) / (4.0 * range);
            let gy = (f32::from(y1) - f32::from(y0)) / (4.0 * range);
            let magnitude2 = gx * gx + gy * gy;
            if magnitude2 < 1e-8 {
                continue;
            }
            xx += f64::from(gx * gx);
            xy += f64::from(gx * gy);
            yy += f64::from(gy * gy);
        }
    }
    let trace = xx + yy;
    if !trace.is_finite() || trace <= 1e-10 {
        anyhow::bail!("Edge ROI has no measurable gradient energy");
    }
    let discriminant = ((xx - yy) * (xx - yy) + 4.0 * xy * xy).sqrt();
    let coherence = (discriminant / trace).clamp(0.0, 1.0) as f32;
    let angle = 0.5 * (2.0 * xy).atan2(xx - yy);
    Ok(([angle.cos() as f32, angle.sin() as f32], coherence))
}

fn fit_projected_esf(
    samples: &[EsfSample],
    normal: [f32; 2],
    config: &LensPsfExtractionConfig,
) -> Result<FitCore> {
    let mut values = samples
        .iter()
        .map(|sample| sample.value)
        .collect::<Vec<_>>();
    let low = percentile(&mut values, 0.05);
    let high = percentile(&mut values, 0.95);
    let contrast = high - low;
    if contrast < config.minimum_contrast {
        anyhow::bail!(
            "Edge contrast {contrast:.5} is below {:.5}",
            config.minimum_contrast
        );
    }
    let mut projections = samples
        .iter()
        .map(|sample| dot(normal, [sample.x, sample.y]))
        .collect::<Vec<_>>();
    projections.sort_by(f32::total_cmp);
    let min_projection = projections[0];
    let max_projection = projections[projections.len() - 1];
    let bin_count = (((max_projection - min_projection) * ESF_OVERSAMPLING).ceil() as usize + 1)
        .clamp(8, 65_536);
    let mut sums = vec![0.0f64; bin_count];
    let mut counts = vec![0u32; bin_count];
    for sample in samples {
        let projection = dot(normal, [sample.x, sample.y]);
        let index = (((projection - min_projection) * ESF_OVERSAMPLING).floor() as usize)
            .min(bin_count - 1);
        sums[index] += f64::from(sample.value);
        counts[index] += 1;
    }
    let points = sums
        .iter()
        .zip(&counts)
        .enumerate()
        .filter(|(_, (_, count))| **count > 0)
        .map(|(index, (sum, count))| EsfPoint {
            position: min_projection + (index as f32 + 0.5) / ESF_OVERSAMPLING,
            value: (*sum / f64::from(*count)) as f32,
            weight: (*count as f32).sqrt(),
        })
        .collect::<Vec<_>>();
    if points.len() < 24 {
        anyhow::bail!("Edge has too few populated supersampled ESF bins");
    }
    let mut center = points[points.len() / 2].position;
    let mut steepest = 0.0f32;
    for pair in points.windows(2) {
        let distance = pair[1].position - pair[0].position;
        if distance <= 0.0 || distance > 1.0 {
            continue;
        }
        let slope = (pair[1].value - pair[0].value).abs() / distance;
        if slope > steepest {
            steepest = slope;
            center = (pair[0].position + pair[1].position) * 0.5;
        }
    }
    if steepest <= 1e-6 {
        anyhow::bail!("Edge ESF has no stable transition");
    }
    let mut best = None::<FitCore>;
    let mut radius = 0.25f32;
    while radius <= config.maximum_defocus_radius_px {
        for center_step in -12..=12 {
            let candidate_center = center + center_step as f32 * 0.25;
            let candidate = evaluate_disk_fit(&points, candidate_center, radius)?;
            if best.map_or(true, |current| {
                candidate.residual_relative < current.residual_relative
            }) {
                best = Some(candidate);
            }
        }
        radius *= 1.18;
    }
    let mut best = best.context("No analytic disk fit candidates were evaluated")?;
    let mut center_step = 0.125f32;
    let mut radius_step = (best.radius * 0.12).max(0.125);
    for _ in 0..5 {
        let current = best;
        for center_delta in -2..=2 {
            for radius_delta in -2..=2 {
                let candidate_radius = (current.radius + radius_delta as f32 * radius_step)
                    .clamp(0.20, config.maximum_defocus_radius_px);
                let candidate = evaluate_disk_fit(
                    &points,
                    current.center + center_delta as f32 * center_step,
                    candidate_radius,
                )?;
                if candidate.residual_relative < best.residual_relative {
                    best = candidate;
                }
            }
        }
        center_step *= 0.4;
        radius_step *= 0.4;
    }
    if best.contrast.abs() < config.minimum_contrast {
        anyhow::bail!("Analytic disk fit collapsed below the contrast gate");
    }
    Ok(best)
}

fn evaluate_disk_fit(points: &[EsfPoint], center: f32, radius: f32) -> Result<FitCore> {
    let mut sum_w = 0.0f64;
    let mut sum_x = 0.0f64;
    let mut sum_y = 0.0f64;
    let mut sum_xx = 0.0f64;
    let mut sum_xy = 0.0f64;
    for point in points {
        let model = disk_edge_cdf((point.position - center) / radius);
        let proximity = (-0.5 * ((point.position - center) / (radius + 0.5)).powi(2)).exp();
        let weight = f64::from(point.weight * (1.0 + 8.0 * proximity));
        sum_w += weight;
        sum_x += weight * f64::from(model);
        sum_y += weight * f64::from(point.value);
        sum_xx += weight * f64::from(model * model);
        sum_xy += weight * f64::from(model * point.value);
    }
    let denominator = sum_w * sum_xx - sum_x * sum_x;
    if denominator.abs() <= 1e-12 {
        anyhow::bail!("Analytic disk fit is singular");
    }
    let contrast = ((sum_w * sum_xy - sum_x * sum_y) / denominator) as f32;
    let low = ((sum_y - f64::from(contrast) * sum_x) / sum_w) as f32;
    let scale = contrast.abs().max(1e-6);
    let mut objective = 0.0f64;
    let mut weight_total = 0.0f64;
    for point in points {
        let model = low + contrast * disk_edge_cdf((point.position - center) / radius);
        let normalized = (point.value - model) / scale;
        let proximity = (-0.5 * ((point.position - center) / (radius + 0.5)).powi(2)).exp();
        let weight = f64::from(point.weight * (1.0 + 8.0 * proximity));
        let absolute = normalized.abs();
        let huber = if absolute <= 0.03 {
            0.5 * normalized * normalized
        } else {
            0.03 * (absolute - 0.015)
        };
        objective += weight * f64::from(huber);
        weight_total += weight;
    }
    Ok(FitCore {
        center,
        radius,
        contrast,
        residual_relative: (2.0 * objective / weight_total.max(1e-12)).sqrt() as f32,
    })
}

fn disk_edge_cdf(value: f32) -> f32 {
    if value <= -1.0 {
        0.0
    } else if value >= 1.0 {
        1.0
    } else {
        0.5 + (value.asin() + value * (1.0 - value * value).sqrt()) / PI
    }
}

fn disk_mtf50(diameter_px: f32) -> f32 {
    if diameter_px <= 0.0 {
        return 0.5;
    }
    let radius = diameter_px * 0.5;
    let mtf = |frequency: f32| {
        const STEPS: usize = 256;
        let step = 2.0 * radius / STEPS as f32;
        let mut sum = 0.0f64;
        let mut dc = 0.0f64;
        for index in 0..=STEPS {
            let x = -radius + index as f32 * step;
            let chord = 2.0 * (radius * radius - x * x).max(0.0).sqrt();
            let coefficient = if index == 0 || index == STEPS {
                1.0
            } else if index & 1 == 0 {
                2.0
            } else {
                4.0
            };
            let weighted = f64::from(coefficient * chord);
            dc += weighted;
            sum += weighted * f64::from((2.0 * PI * frequency * x).cos());
        }
        (sum / dc.max(1e-12)).abs() as f32
    };
    if mtf(0.5) > 0.5 {
        return 0.5;
    }
    let mut low = 0.0;
    let mut high = 0.5;
    for _ in 0..32 {
        let middle = (low + high) * 0.5;
        if mtf(middle) > 0.5 {
            low = middle;
        } else {
            high = middle;
        }
    }
    (low + high) * 0.5
}

fn common_normal(first: [f32; 2], mut second: [f32; 2]) -> ([f32; 2], f32) {
    let alignment = dot(first, second);
    if alignment < 0.0 {
        second = [-second[0], -second[1]];
    }
    let cosine = dot(first, second).clamp(-1.0, 1.0);
    let angle = cosine.acos().to_degrees();
    let sum = [first[0] + second[0], first[1] + second[1]];
    let length = (sum[0] * sum[0] + sum[1] * sum[1]).sqrt().max(1e-8);
    ([sum[0] / length, sum[1] / length], angle)
}

fn edge_slant_degrees(normal: [f32; 2]) -> f32 {
    let angle = normal[1].atan2(normal[0]).abs().rem_euclid(FRAC_PI_2);
    angle.min(FRAC_PI_2 - angle).to_degrees()
}

fn normalized_field_radius(x: f32, y: f32, width: u32, height: u32) -> f32 {
    let center_x = (width as f32 - 1.0) * 0.5;
    let center_y = (height as f32 - 1.0) * 0.5;
    let radius = ((x - center_x).powi(2) + (y - center_y).powi(2)).sqrt();
    let corner = (center_x * center_x + center_y * center_y).sqrt().max(1e-8);
    (radius / corner).clamp(0.0, 1.0)
}

fn raw_value(raw: &RawBuffer, origin: [u32; 2], sensor_x: u32, sensor_y: u32) -> Option<u16> {
    let x = sensor_x.checked_sub(origin[0])?;
    let y = sensor_y.checked_sub(origin[1])?;
    raw.get_pixel(x, y)
}

fn cfa_color(pattern: [u8; 4], sensor_x: u32, sensor_y: u32) -> u8 {
    pattern[((sensor_y & 1) * 2 + (sensor_x & 1)) as usize]
}

fn resolve_capture_path(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_relative_path(relative)?;
    let path = root.join(relative);
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Resolve retained source {}", path.display()))?;
    if !canonical.starts_with(root) || !canonical.is_file() {
        anyhow::bail!("Retained source escapes the capture root or is not a regular file");
    }
    Ok(canonical)
}

fn validate_relative_path(path: &str) -> Result<()> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        anyhow::bail!("Retained lens target paths must be safe relative paths");
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn robust_sigma(values: &mut [f32]) -> f32 {
    let median = percentile(values, 0.5);
    let mut deviations = values
        .iter()
        .map(|value| (value - median).abs())
        .collect::<Vec<_>>();
    1.4826 * percentile(&mut deviations, 0.5)
}

fn percentile(values: &mut [f32], quantile: f32) -> f32 {
    values.sort_by(f32::total_cmp);
    let index = ((values.len() - 1) as f32 * quantile).round() as usize;
    values[index.min(values.len() - 1)]
}

fn nearest_knot(knots: &[f32], value: f32) -> Option<usize> {
    knots
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| (*left - value).abs().total_cmp(&(*right - value).abs()))
        .map(|(index, _)| index)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn relative_match(expected: f32, actual: f32, tolerance: f32) -> bool {
    expected.is_finite()
        && actual.is_finite()
        && (expected - actual).abs() <= expected.abs().max(actual.abs()) * tolerance
}

fn normalize_identity(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn unit_interval(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

fn dot(left: [f32; 2], right: [f32; 2]) -> f32 {
    left[0] * right[0] + left[1] * right[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_bar(
        width: u32,
        height: u32,
        normal_angle_degrees: f32,
        first_x_at_center: f32,
        second_x_at_center: f32,
        diameter_px: f32,
        noise: f32,
    ) -> (RawBuffer, f32) {
        let levels = SensorLevels {
            black: 1_000,
            white: 15_000,
        };
        let angle = normal_angle_degrees.to_radians();
        let normal = [angle.cos(), angle.sin()];
        let center_y = (height as f32 - 1.0) * 0.5;
        let first_c = normal[0] * first_x_at_center + normal[1] * center_y;
        let second_c = normal[0] * second_x_at_center + normal[1] * center_y;
        let radius = diameter_px * 0.5;
        let mut raw = RawBuffer::new(width, height, [0, 1, 1, 2], 14);
        for y in 0..height {
            for x in 0..width {
                let projection = dot(normal, [x as f32, y as f32]);
                let bar = disk_edge_cdf((projection - first_c) / radius)
                    - disk_edge_cdf((projection - second_c) / radius);
                let deterministic_noise =
                    (((x * 17 + y * 31 + x * y * 3) % 29) as f32 - 14.0) / 14.0 * noise;
                let signal = (0.18 + 0.64 * bar + deterministic_noise).clamp(0.02, 0.98);
                let value =
                    f32::from(levels.black) + signal * f32::from(levels.white - levels.black);
                raw.set_pixel(x, y, value.round() as u16);
            }
        }
        let separation = (second_c - first_c).abs();
        (raw, separation)
    }

    fn synthetic_target(
        separation_px: f32,
        subject_distance_m: f32,
        effective_focal_length_mm: f32,
    ) -> LensPsfEdgePairTarget {
        let pitch_mm = 4.35 * 0.001;
        let subject_mm = subject_distance_m * 1_000.0;
        let magnification = effective_focal_length_mm / (subject_mm - effective_focal_length_mm);
        LensPsfEdgePairTarget {
            target_id: "center-bar".to_string(),
            radius_knot: 0.0,
            projected_edge_separation_mm: separation_px * pitch_mm / magnification,
            first_edge_roi: SensorRoi {
                x: 34,
                y: 16,
                width: 52,
                height: 128,
            },
            second_edge_roi: SensorRoi {
                x: 104,
                y: 16,
                width: 52,
                height: 128,
            },
        }
    }

    #[test]
    fn analytic_disk_fit_recovers_psf_and_effective_focal_length() {
        let diameter = 6.0;
        let subject_distance = 1.0;
        let effective_focal = 52.5;
        let (raw, separation) = synthetic_bar(192, 160, 7.0, 60.0, 130.0, diameter, 0.0025);
        let target = synthetic_target(separation, subject_distance, effective_focal);
        let config = LensPsfExtractionConfig {
            maximum_radius_knot_deviation: 1.0,
            ..Default::default()
        };
        let metadata = Z9Metadata {
            width: 192,
            height: 160,
            bits_per_sample: 14,
            compression: 1,
            cfa_pattern: [0, 1, 1, 2],
            camera_make: "Nikon".to_string(),
            camera_model: "Synthetic".to_string(),
            sensor_levels: Some(SensorLevels {
                black: 1_000,
                white: 15_000,
            }),
            sensor_geometry: Some(crate::nef::parser::SensorGeometry {
                pixel_pitch_um: 4.35,
            }),
            strip_offsets: Vec::new(),
            strip_byte_counts: Vec::new(),
            rows_per_strip: 160,
            cam_mul: [1.0; 4],
            timestamp: None,
            exposure_time: Some(1.0 / 125.0),
            aperture: Some(4.0),
            iso: Some(100),
            focal_length: Some(50.0),
            focus_distance: Some(0.8),
            lens_model: Some("Synthetic 50mm".to_string()),
        };
        let first = fit_slanted_edge(
            &raw,
            [0, 0],
            target.first_edge_roi,
            metadata.sensor_levels.unwrap(),
            metadata.cfa_pattern,
            "first",
            &config,
        )
        .unwrap();
        let second = fit_slanted_edge(
            &raw,
            [0, 0],
            target.second_edge_roi,
            metadata.sensor_levels.unwrap(),
            metadata.cfa_pattern,
            "second",
            &config,
        )
        .unwrap();
        let (normal, angle) = common_normal(first.0.normal, second.0.normal);
        let measured_separation = dot(
            normal,
            [
                second.0.point[0] - first.0.point[0],
                second.0.point[1] - first.0.point[1],
            ],
        )
        .abs();
        let pitch_mm = 0.00435;
        let magnification = measured_separation * pitch_mm / target.projected_edge_separation_mm;
        let measured_focal = magnification * subject_distance * 1_000.0 / (1.0 + magnification);
        let measured_diameter = (first.0.diameter_px + second.0.diameter_px) * 0.5;
        println!(
            "disk extraction: diameter={measured_diameter:.5} focal={measured_focal:.5} parallel={angle:.5}"
        );
        assert!((measured_diameter - diameter).abs() / diameter < 0.03);
        assert!((measured_focal - effective_focal).abs() / effective_focal < 0.01);
        assert!(angle < 0.2);
        assert!(first.1.fit_residual_relative < 0.02);
        assert!(first.1.model_mtf50_cycles_per_pixel > 0.05);
    }

    #[test]
    fn extraction_plan_rejects_cross_split_reuse_and_missing_radius_cells() {
        let target = LensPsfEdgePairTarget {
            target_id: "target".to_string(),
            radius_knot: 0.0,
            projected_edge_separation_mm: 10.0,
            first_edge_roi: SensorRoi {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            },
            second_edge_roi: SensorRoi {
                x: 64,
                y: 0,
                width: 64,
                height: 64,
            },
        };
        let capture = |index: usize, split| LensPsfExtractionCapture {
            path: format!("capture-{index}.nef"),
            sha256: format!("{:064x}", index + 1),
            split,
            measured_focus_distance_m: Some(0.8 + index as f32 * 0.01),
            measured_focus_distance_uncertainty_m: Some(0.001),
            subject_distance_m: 1.0,
            targets: vec![
                target.clone(),
                LensPsfEdgePairTarget {
                    target_id: "corner".to_string(),
                    radius_knot: 1.0,
                    ..target.clone()
                },
            ],
        };
        let mut plan = LensPsfExtractionPlan {
            schema: LENS_PSF_EXTRACTION_PLAN_SCHEMA.to_string(),
            target_id: "disk-edge-v1".to_string(),
            nominal_focal_length_mm: 50.0,
            aperture: 4.0,
            radius_knots: vec![0.0, 1.0],
            config: Default::default(),
            captures: vec![
                capture(0, CalibrationSplit::Fit),
                capture(1, CalibrationSplit::Fit),
                capture(2, CalibrationSplit::Holdout),
                capture(3, CalibrationSplit::Holdout),
            ],
        };
        plan.validate().unwrap();
        plan.captures[3].sha256 = plan.captures[0].sha256.clone();
        assert!(plan.validate().is_err());

        plan.captures[3].sha256 = format!("{:064x}", 4);
        plan.captures[0].targets.pop();
        assert!(plan.validate().is_err());
    }

    #[test]
    fn independent_focus_distance_is_uncertainty_and_metadata_gated() {
        let capture = LensPsfExtractionCapture {
            path: "capture.nef".to_string(),
            sha256: format!("{:064x}", 1),
            split: CalibrationSplit::Fit,
            measured_focus_distance_m: Some(0.8),
            measured_focus_distance_uncertainty_m: Some(0.004),
            subject_distance_m: 1.0,
            targets: Vec::new(),
        };
        let config = LensPsfExtractionConfig::default();
        let selected = select_focus_distance(&capture, Some(0.82), &config).unwrap();
        assert_eq!(selected.selected_source, "independent_measured");
        assert_eq!(selected.selected_distance_m, 0.8);
        assert!(selected.metadata_relative_disagreement.unwrap() < 0.03);

        assert!(select_focus_distance(&capture, Some(1.2), &config).is_err());
        let mut invalid = capture.clone();
        invalid.measured_focus_distance_uncertainty_m = None;
        let plan = LensPsfExtractionPlan {
            schema: LENS_PSF_EXTRACTION_PLAN_SCHEMA.to_string(),
            target_id: "target".to_string(),
            nominal_focal_length_mm: 50.0,
            aperture: 4.0,
            radius_knots: vec![0.0, 1.0],
            config,
            captures: Vec::new(),
        };
        assert!(!valid_measured_focus(&invalid, &plan));
    }

    #[test]
    fn axis_aligned_and_clipped_edges_fail_quality_gates() {
        let (raw, _) = synthetic_bar(192, 160, 0.0, 60.0, 130.0, 6.0, 0.0);
        let target = synthetic_target(70.0, 1.0, 52.5);
        let error = fit_slanted_edge(
            &raw,
            [0, 0],
            target.first_edge_roi,
            SensorLevels {
                black: 1_000,
                white: 15_000,
            },
            [0, 1, 1, 2],
            "axis",
            &Default::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("slant"));

        let mut clipped = raw;
        clipped.data.fill(15_000);
        assert!(fit_slanted_edge(
            &clipped,
            [0, 0],
            target.first_edge_roi,
            SensorLevels {
                black: 1_000,
                white: 15_000,
            },
            [0, 1, 1, 2],
            "clipped",
            &Default::default(),
        )
        .is_err());
    }

    #[test]
    fn analytic_disk_fit_is_stable_across_slant_and_blur_range() {
        let config = LensPsfExtractionConfig {
            maximum_diameter_relative_uncertainty: 0.20,
            ..Default::default()
        };
        let mut maximum_relative_error = 0.0f32;
        for (diameter, angle) in [(2.5, 3.0), (5.0, 7.0), (10.0, 12.0), (16.0, 17.0)] {
            let (raw, separation) = synthetic_bar(192, 160, angle, 60.0, 130.0, diameter, 0.0015);
            let target = synthetic_target(separation, 1.0, 52.5);
            let first = fit_slanted_edge(
                &raw,
                [0, 0],
                target.first_edge_roi,
                SensorLevels {
                    black: 1_000,
                    white: 15_000,
                },
                [0, 1, 1, 2],
                "sweep",
                &config,
            )
            .unwrap();
            let relative_error = (first.0.diameter_px - diameter).abs() / diameter;
            maximum_relative_error = maximum_relative_error.max(relative_error);
            assert!(relative_error < 0.05, "{diameter} px at {angle} degrees");
            assert!(first.1.fit_residual_relative < 0.025);
        }
        println!("disk extraction sweep maximum relative error={maximum_relative_error:.6}");
    }
}
