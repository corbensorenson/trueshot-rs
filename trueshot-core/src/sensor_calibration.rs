//! Auditable paired-frame photon-transfer calibration.
//!
//! Spatial variance in a flat field confounds temporal sensor noise with PRNU,
//! lens shading, and illumination gradients. TrueShot instead uses differences
//! between repeated dark/flat frames, keeps whole pairs in either fit or
//! holdout evidence, and publishes a calibrated model only when every CFA site
//! passes preregistered variance and residual-coverage gates.

use crate::sensor_noise::{IsoNoiseModel, SensorNoiseModel};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const SENSOR_CALIBRATION_ISO_REPORT_SCHEMA: &str = "trueshot.sensor-calibration.iso.v1";
const MINIMUM_SITE_SAMPLES: usize = 1024;
const NORMAL_90: f32 = 1.644_853_6;
const NORMAL_95: f32 = 1.959_964;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationSplit {
    Fit,
    Holdout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SensorCalibrationConfig {
    /// Hard cap retained from each pair and CFA site.
    pub max_samples_per_pair_per_site: usize,
    /// Minimum independently captured flat-field levels.
    pub minimum_flat_levels: usize,
    /// Minimum dark pairs in each split. Dark-frame center drift needs more
    /// independent captures than the spatially dense photon-transfer fit.
    pub minimum_dark_pairs_per_split: usize,
    /// Minimum per-level flat pairs in each split.
    pub minimum_flat_pairs_per_split: usize,
    /// Maximum held-out relative variance error for every fitted CFA site.
    pub maximum_variance_relative_error: f32,
    /// Allowed absolute error around nominal Gaussian interval coverage.
    pub coverage_absolute_tolerance: f32,
    /// Reject flat pairs whose exposure-normalized site means disagree more.
    pub maximum_pair_mean_mismatch: f32,
    /// Flat evidence must reach this fraction of sensor range.
    pub minimum_peak_signal_fraction: f32,
    /// Exclude near-censored samples from the shot-noise regression.
    pub fit_signal_ceiling_fraction: f32,
    /// One-sided high-signal noise tail reserved below encoded white.
    pub saturation_tail_sigma: f32,
}

impl Default for SensorCalibrationConfig {
    fn default() -> Self {
        Self {
            max_samples_per_pair_per_site: 32_768,
            minimum_flat_levels: 5,
            minimum_dark_pairs_per_split: 4,
            minimum_flat_pairs_per_split: 2,
            maximum_variance_relative_error: 0.10,
            coverage_absolute_tolerance: 0.03,
            maximum_pair_mean_mismatch: 0.01,
            minimum_peak_signal_fraction: 0.90,
            fit_signal_ceiling_fraction: 0.92,
            saturation_tail_sigma: 4.0,
        }
    }
}

impl SensorCalibrationConfig {
    pub fn validate(&self) -> Result<()> {
        if self.max_samples_per_pair_per_site < MINIMUM_SITE_SAMPLES
            || !(3..=32).contains(&self.minimum_flat_levels)
            || !(2..=32).contains(&self.minimum_dark_pairs_per_split)
            || !(1..=16).contains(&self.minimum_flat_pairs_per_split)
            || !self.maximum_variance_relative_error.is_finite()
            || !(0.01..=0.50).contains(&self.maximum_variance_relative_error)
            || !self.coverage_absolute_tolerance.is_finite()
            || !(0.005..=0.10).contains(&self.coverage_absolute_tolerance)
            || !self.maximum_pair_mean_mismatch.is_finite()
            || !(0.001..=0.10).contains(&self.maximum_pair_mean_mismatch)
            || !self.minimum_peak_signal_fraction.is_finite()
            || !(0.50..=0.98).contains(&self.minimum_peak_signal_fraction)
            || !self.fit_signal_ceiling_fraction.is_finite()
            || !(0.50..=0.98).contains(&self.fit_signal_ceiling_fraction)
            || self.fit_signal_ceiling_fraction < self.minimum_peak_signal_fraction
            || !self.saturation_tail_sigma.is_finite()
            || !(3.0..=8.0).contains(&self.saturation_tail_sigma)
        {
            anyhow::bail!("Sensor calibration configuration is invalid");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct NoiseSample {
    signal_dn: f32,
    residual_dn: f32,
}

#[derive(Debug)]
struct PairEvidence {
    samples: [Vec<NoiseSample>; 4],
    fixed_pattern_residuals_dn: [Vec<f32>; 4],
    frame_centers_dn: [[f32; 4]; 2],
    level: Option<u32>,
    peak_signal_dn: [f32; 4],
    mean_mismatch: [f32; 4],
}

impl PairEvidence {
    fn empty(level: Option<u32>) -> Self {
        Self {
            samples: std::array::from_fn(|_| Vec::new()),
            fixed_pattern_residuals_dn: std::array::from_fn(|_| Vec::new()),
            frame_centers_dn: [[0.0; 4]; 2],
            level,
            peak_signal_dn: [0.0; 4],
            mean_mismatch: [0.0; 4],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteCalibrationReport {
    pub cfa_site: usize,
    pub read_noise_dn: f32,
    pub fixed_pattern_dn: f32,
    pub frame_black_drift_dn: f32,
    pub black_drift_dn: f32,
    pub electrons_per_dn: f32,
    pub fit_bins: usize,
    pub holdout_bins: usize,
    pub maximum_fit_variance_relative_error: Option<f32>,
    pub dark_holdout_variance_relative_error: Option<f32>,
    pub fixed_pattern_holdout_variance_relative_error: Option<f32>,
    pub maximum_holdout_variance_relative_error: Option<f32>,
    pub holdout_coverage_90: f32,
    pub holdout_coverage_95: f32,
    pub holdout_samples: usize,
    pub passed: bool,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsoCalibrationReport {
    pub schema: String,
    pub iso: u32,
    pub width: usize,
    pub height: usize,
    pub black_level_dn: u16,
    pub white_level_dn: u16,
    pub fit_dark_pairs: usize,
    pub holdout_dark_pairs: usize,
    pub fit_flat_pairs: usize,
    pub holdout_flat_pairs: usize,
    pub flat_levels: usize,
    pub peak_signal_fraction: f32,
    pub saturation_margin_dn: f32,
    pub sites: Vec<SiteCalibrationReport>,
    pub passed: bool,
    pub failures: Vec<String>,
}

#[derive(Debug)]
pub struct IsoCalibrationOutcome {
    pub model: Option<IsoNoiseModel>,
    pub report: IsoCalibrationReport,
}

/// Incremental, bounded evidence collector for one exact camera ISO.
pub struct SensorCalibrationAccumulator {
    iso: u32,
    width: usize,
    height: usize,
    black_level: u16,
    white_level: u16,
    config: SensorCalibrationConfig,
    fit_dark: Vec<PairEvidence>,
    holdout_dark: Vec<PairEvidence>,
    fit_flat: Vec<PairEvidence>,
    holdout_flat: Vec<PairEvidence>,
}

impl SensorCalibrationAccumulator {
    pub fn new(
        iso: u32,
        width: usize,
        height: usize,
        black_level: u16,
        white_level: u16,
        config: SensorCalibrationConfig,
    ) -> Result<Self> {
        config.validate()?;
        if iso == 0 || width < 16 || height < 16 || white_level <= black_level {
            anyhow::bail!("Sensor calibration identity or dimensions are invalid");
        }
        Ok(Self {
            iso,
            width,
            height,
            black_level,
            white_level,
            config,
            fit_dark: Vec::new(),
            holdout_dark: Vec::new(),
            fit_flat: Vec::new(),
            holdout_flat: Vec::new(),
        })
    }

    pub fn add_dark_pair(
        &mut self,
        first: &[u16],
        second: &[u16],
        split: CalibrationSplit,
    ) -> Result<()> {
        self.validate_frame_pair(first, second)?;
        let evidence = summarize_dark_pair(
            first,
            second,
            self.width,
            self.height,
            self.config.max_samples_per_pair_per_site,
        )?;
        self.dark_split_mut(split).push(evidence);
        Ok(())
    }

    pub fn add_flat_pair(
        &mut self,
        level: u32,
        first: &[u16],
        second: &[u16],
        split: CalibrationSplit,
    ) -> Result<()> {
        self.validate_frame_pair(first, second)?;
        let evidence = summarize_flat_pair(
            level,
            first,
            second,
            self.width,
            self.height,
            self.black_level,
            self.white_level,
            &self.config,
        )?;
        self.flat_split_mut(split).push(evidence);
        Ok(())
    }

    pub fn evaluate(self) -> Result<IsoCalibrationOutcome> {
        let mut failures = Vec::new();
        let fit_levels = distinct_levels(&self.fit_flat);
        let holdout_levels = distinct_levels(&self.holdout_flat);
        if self.fit_dark.len() < self.config.minimum_dark_pairs_per_split {
            failures.push(format!(
                "fit dark pairs {} < {}",
                self.fit_dark.len(),
                self.config.minimum_dark_pairs_per_split
            ));
        }
        if self.holdout_dark.len() < self.config.minimum_dark_pairs_per_split {
            failures.push(format!(
                "holdout dark pairs {} < {}",
                self.holdout_dark.len(),
                self.config.minimum_dark_pairs_per_split
            ));
        }
        if fit_levels.len() < self.config.minimum_flat_levels
            || holdout_levels.len() < self.config.minimum_flat_levels
        {
            failures.push(format!(
                "flat levels fit/holdout {}/{} < {}",
                fit_levels.len(),
                holdout_levels.len(),
                self.config.minimum_flat_levels
            ));
        }
        let independent_fit_levels =
            independent_signal_level_count(&self.fit_flat, self.black_level, self.white_level);
        let independent_holdout_levels =
            independent_signal_level_count(&self.holdout_flat, self.black_level, self.white_level);
        if independent_fit_levels < self.config.minimum_flat_levels
            || independent_holdout_levels < self.config.minimum_flat_levels
        {
            failures.push(format!(
                "independent signal levels fit/holdout {independent_fit_levels}/{independent_holdout_levels} < {}",
                self.config.minimum_flat_levels
            ));
        }
        for level in fit_levels.union(&holdout_levels) {
            let fit_count = self
                .fit_flat
                .iter()
                .filter(|pair| pair.level == Some(*level))
                .count();
            let holdout_count = self
                .holdout_flat
                .iter()
                .filter(|pair| pair.level == Some(*level))
                .count();
            if fit_count < self.config.minimum_flat_pairs_per_split
                || holdout_count < self.config.minimum_flat_pairs_per_split
            {
                failures.push(format!(
                    "flat level {level} has fit/holdout pairs {fit_count}/{holdout_count}"
                ));
            }
        }

        let range_dn = f32::from(self.white_level - self.black_level);
        let peak_signal_dn = self
            .fit_flat
            .iter()
            .chain(&self.holdout_flat)
            .flat_map(|pair| pair.peak_signal_dn)
            .fold(0.0f32, f32::max);
        let peak_signal_fraction = peak_signal_dn / range_dn;
        if peak_signal_fraction < self.config.minimum_peak_signal_fraction {
            failures.push(format!(
                "peak flat signal {:.3} < {:.3} of sensor range",
                peak_signal_fraction, self.config.minimum_peak_signal_fraction
            ));
        }
        let mut read_noise_dn = [0.0f32; 4];
        let mut black_drift_dn = [0.0f32; 4];
        let mut electrons_per_dn = [0.0f32; 4];
        let mut site_reports = Vec::with_capacity(4);
        for site in 0..4 {
            let mut site_failures = Vec::new();
            let fit_dark_samples = collect_samples(&self.fit_dark, site);
            let holdout_dark_samples = collect_samples(&self.holdout_dark, site);
            let fit_flat_samples = collect_samples(&self.fit_flat, site);
            let holdout_flat_samples = collect_samples(&self.holdout_flat, site);
            let dark_centers = self
                .fit_dark
                .iter()
                .flat_map(|pair| {
                    [
                        pair.frame_centers_dn[0][site],
                        pair.frame_centers_dn[1][site],
                    ]
                })
                .collect::<Vec<_>>();
            let fit_fixed_pattern = average_fixed_pattern(&self.fit_dark, site);
            let holdout_fixed_pattern = average_fixed_pattern(&self.holdout_dark, site);

            let read = robust_sigma(
                &fit_dark_samples
                    .iter()
                    .map(|sample| sample.residual_dn)
                    .collect::<Vec<_>>(),
            );
            let frame_drift = robust_sigma(&dark_centers);
            if !read.is_finite() || read <= 0.0 {
                site_failures.push("read-noise estimate is invalid".to_string());
            }
            if !frame_drift.is_finite() || frame_drift < 0.0 {
                site_failures.push("black-drift estimate is invalid".to_string());
            }
            let fit_pattern_noise_variance =
                read.powi(2) / (2.0 * self.fit_dark.len().max(1) as f32);
            let fixed_pattern_variance =
                (robust_second_moment(&fit_fixed_pattern) - fit_pattern_noise_variance).max(0.0);
            let fixed_pattern = fixed_pattern_variance.sqrt();
            let drift = (fixed_pattern_variance + frame_drift.max(0.0).powi(2)).sqrt();
            read_noise_dn[site] = read.max(1e-6);
            black_drift_dn[site] = drift.max(0.0);
            let dark_holdout_variance = robust_second_moment(
                &holdout_dark_samples
                    .iter()
                    .map(|sample| sample.residual_dn)
                    .collect::<Vec<_>>(),
            );
            let dark_holdout_error = (dark_holdout_variance - read_noise_dn[site].powi(2)).abs()
                / read_noise_dn[site].powi(2).max(1e-6);
            if !dark_holdout_error.is_finite()
                || dark_holdout_error > self.config.maximum_variance_relative_error
            {
                site_failures.push(format!(
                    "held-out dark variance error {:.3} > {:.3}",
                    dark_holdout_error, self.config.maximum_variance_relative_error
                ));
            }
            let holdout_pattern_noise_variance =
                read_noise_dn[site].powi(2) / (2.0 * self.holdout_dark.len().max(1) as f32);
            let heldout_fixed_variance = (robust_second_moment(&holdout_fixed_pattern)
                - holdout_pattern_noise_variance)
                .max(0.0);
            // Normalize repeatability by total dark variance. Dividing by a
            // tiny sub-read-noise pattern term is statistically unstable and
            // rejects models whose effect on runtime uncertainty is negligible.
            let fixed_pattern_error = (heldout_fixed_variance - fixed_pattern_variance).abs()
                / (read_noise_dn[site].powi(2) + fixed_pattern_variance).max(1e-6);
            if !fixed_pattern_error.is_finite()
                || fixed_pattern_error > self.config.maximum_variance_relative_error
            {
                site_failures.push(format!(
                    "held-out fixed-pattern variance error {:.3} > {:.3}",
                    fixed_pattern_error, self.config.maximum_variance_relative_error
                ));
            }

            // Pair differencing removes persistent pattern and frame-center
            // offsets. Fit temporal read + shot variance here; retain those
            // independently measured biases in the runtime single-frame model.
            let temporal_base_variance = read_noise_dn[site].powi(2);
            let fit_bins = variance_bins(
                &fit_flat_samples,
                range_dn * self.config.fit_signal_ceiling_fraction,
            );
            let holdout_bins = variance_bins(
                &holdout_flat_samples,
                range_dn * self.config.fit_signal_ceiling_fraction,
            );
            let slope = robust_shot_slope(&fit_bins, temporal_base_variance);
            if !slope.is_finite() || slope <= 0.0 {
                site_failures.push("shot-noise slope is invalid".to_string());
            }
            electrons_per_dn[site] = slope.max(1e-9).recip();

            let maximum_fit_error =
                maximum_variance_error(&fit_bins, temporal_base_variance, slope)
                    .unwrap_or(f32::INFINITY);
            let maximum_holdout_error =
                maximum_variance_error(&holdout_bins, temporal_base_variance, slope)
                    .unwrap_or(f32::INFINITY);
            if !maximum_fit_error.is_finite() {
                site_failures.push("fit variance diagnostics are unavailable".to_string());
            }
            if !maximum_holdout_error.is_finite()
                || maximum_holdout_error > self.config.maximum_variance_relative_error
            {
                site_failures.push(format!(
                    "held-out variance error {:.3} > {:.3}",
                    maximum_holdout_error, self.config.maximum_variance_relative_error
                ));
            }

            let (coverage_90, coverage_95, holdout_samples) = residual_coverage(
                &holdout_dark_samples,
                &holdout_flat_samples,
                read_noise_dn[site],
                slope,
                range_dn * self.config.fit_signal_ceiling_fraction,
            );
            if (coverage_90 - 0.90).abs() > self.config.coverage_absolute_tolerance {
                site_failures.push(format!(
                    "90% residual coverage {:.3} outside tolerance",
                    coverage_90
                ));
            }
            if (coverage_95 - 0.95).abs() > self.config.coverage_absolute_tolerance {
                site_failures.push(format!(
                    "95% residual coverage {:.3} outside tolerance",
                    coverage_95
                ));
            }

            let passed = site_failures.is_empty();
            site_reports.push(SiteCalibrationReport {
                cfa_site: site,
                read_noise_dn: read_noise_dn[site],
                fixed_pattern_dn: fixed_pattern,
                frame_black_drift_dn: frame_drift.max(0.0),
                black_drift_dn: black_drift_dn[site],
                electrons_per_dn: electrons_per_dn[site],
                fit_bins: fit_bins.len(),
                holdout_bins: holdout_bins.len(),
                maximum_fit_variance_relative_error: finite_value(maximum_fit_error),
                dark_holdout_variance_relative_error: finite_value(dark_holdout_error),
                fixed_pattern_holdout_variance_relative_error: finite_value(fixed_pattern_error),
                maximum_holdout_variance_relative_error: finite_value(maximum_holdout_error),
                holdout_coverage_90: coverage_90,
                holdout_coverage_95: coverage_95,
                holdout_samples,
                passed,
                failures: site_failures,
            });
        }
        if site_reports.iter().any(|site| !site.passed) {
            failures.push("one or more CFA sites failed calibration gates".to_string());
        }

        let saturation_margin_dn = (0..4)
            .map(|site| {
                let variance = read_noise_dn[site].powi(2)
                    + black_drift_dn[site].powi(2)
                    + range_dn / electrons_per_dn[site].max(1e-9);
                self.config.saturation_tail_sigma * variance.max(0.0).sqrt()
            })
            .fold(16.0f32, f32::max)
            .ceil()
            .clamp(16.0, range_dn * 0.10);
        let passed = failures.is_empty();
        let model = passed.then_some(IsoNoiseModel {
            iso: self.iso,
            model: SensorNoiseModel {
                read_noise_dn,
                electrons_per_dn,
                black_drift_dn,
                saturation_margin_dn,
                calibrated: true,
            },
        });
        let report = IsoCalibrationReport {
            schema: SENSOR_CALIBRATION_ISO_REPORT_SCHEMA.to_string(),
            iso: self.iso,
            width: self.width,
            height: self.height,
            black_level_dn: self.black_level,
            white_level_dn: self.white_level,
            fit_dark_pairs: self.fit_dark.len(),
            holdout_dark_pairs: self.holdout_dark.len(),
            fit_flat_pairs: self.fit_flat.len(),
            holdout_flat_pairs: self.holdout_flat.len(),
            flat_levels: fit_levels.union(&holdout_levels).count(),
            peak_signal_fraction,
            saturation_margin_dn,
            sites: site_reports,
            passed,
            failures,
        };
        Ok(IsoCalibrationOutcome { model, report })
    }

    fn validate_frame_pair(&self, first: &[u16], second: &[u16]) -> Result<()> {
        let expected = self
            .width
            .checked_mul(self.height)
            .context("Calibration dimensions overflow")?;
        if first.len() != expected || second.len() != expected {
            anyhow::bail!(
                "Calibration pair has {}/{} pixels, expected {}",
                first.len(),
                second.len(),
                expected
            );
        }
        Ok(())
    }

    fn dark_split_mut(&mut self, split: CalibrationSplit) -> &mut Vec<PairEvidence> {
        match split {
            CalibrationSplit::Fit => &mut self.fit_dark,
            CalibrationSplit::Holdout => &mut self.holdout_dark,
        }
    }

    fn flat_split_mut(&mut self, split: CalibrationSplit) -> &mut Vec<PairEvidence> {
        match split {
            CalibrationSplit::Fit => &mut self.fit_flat,
            CalibrationSplit::Holdout => &mut self.holdout_flat,
        }
    }
}

fn summarize_dark_pair(
    first: &[u16],
    second: &[u16],
    width: usize,
    height: usize,
    maximum_samples: usize,
) -> Result<PairEvidence> {
    let sampled = sample_cfa_pair(first, second, width, height, maximum_samples);
    let mut evidence = PairEvidence::empty(None);
    for (site, pairs) in sampled.into_iter().enumerate() {
        if pairs.len() < MINIMUM_SITE_SAMPLES {
            anyhow::bail!("Dark pair CFA site {site} has only {} samples", pairs.len());
        }
        let first_values = pairs
            .iter()
            .map(|pair| f32::from(pair.0))
            .collect::<Vec<_>>();
        let second_values = pairs
            .iter()
            .map(|pair| f32::from(pair.1))
            .collect::<Vec<_>>();
        let first_center = robust_location(&first_values);
        let second_center = robust_location(&second_values);
        evidence.frame_centers_dn[0][site] = first_center;
        evidence.frame_centers_dn[1][site] = second_center;
        evidence.fixed_pattern_residuals_dn[site] = pairs
            .iter()
            .map(|(first, second)| {
                0.5 * (f32::from(*first) + f32::from(*second) - first_center - second_center)
            })
            .collect();
        evidence.samples[site] = pairs
            .into_iter()
            .map(|(first, second)| NoiseSample {
                signal_dn: 0.0,
                residual_dn: ((f32::from(first) - first_center)
                    - (f32::from(second) - second_center))
                    * std::f32::consts::FRAC_1_SQRT_2,
            })
            .collect();
    }
    Ok(evidence)
}

fn summarize_flat_pair(
    level: u32,
    first: &[u16],
    second: &[u16],
    width: usize,
    height: usize,
    black_level: u16,
    white_level: u16,
    config: &SensorCalibrationConfig,
) -> Result<PairEvidence> {
    let sampled = sample_cfa_pair(
        first,
        second,
        width,
        height,
        config.max_samples_per_pair_per_site,
    );
    let mut evidence = PairEvidence::empty(Some(level));
    let black = f32::from(black_level);
    let white = f32::from(white_level);
    let range = white - black;
    for (site, pairs) in sampled.into_iter().enumerate() {
        if pairs.len() < MINIMUM_SITE_SAMPLES {
            anyhow::bail!("Flat pair CFA site {site} has only {} samples", pairs.len());
        }
        let first_signal = pairs
            .iter()
            .map(|pair| (f32::from(pair.0) - black).max(0.0))
            .collect::<Vec<_>>();
        let second_signal = pairs
            .iter()
            .map(|pair| (f32::from(pair.1) - black).max(0.0))
            .collect::<Vec<_>>();
        let first_center = robust_location(&first_signal);
        let second_center = robust_location(&second_signal);
        let target = 0.5 * (first_center + second_center);
        let mismatch = (first_center - second_center).abs() / target.max(1.0);
        evidence.mean_mismatch[site] = mismatch;
        if target <= range * 0.01 {
            anyhow::bail!("Flat level {level} CFA site {site} is indistinguishable from dark");
        }
        if mismatch > config.maximum_pair_mean_mismatch {
            anyhow::bail!(
                "Flat level {level} CFA site {site} pair mismatch {:.4} exceeds {:.4}",
                mismatch,
                config.maximum_pair_mean_mismatch
            );
        }
        let first_scale = target / first_center.max(1.0);
        let second_scale = target / second_center.max(1.0);
        let residual_normalizer = (first_scale * first_scale + second_scale * second_scale).sqrt();
        let mut peak_values = Vec::with_capacity(pairs.len());
        let mut site_samples = Vec::with_capacity(pairs.len());
        for ((first, second), (first_dn, second_dn)) in pairs
            .into_iter()
            .zip(first_signal.into_iter().zip(second_signal))
        {
            let normalized_first = first_scale * first_dn;
            let normalized_second = second_scale * second_dn;
            let signal = 0.5 * (normalized_first + normalized_second);
            peak_values.push(signal);
            if f32::from(first) < white
                && f32::from(second) < white
                && signal <= range * config.fit_signal_ceiling_fraction
            {
                site_samples.push(NoiseSample {
                    signal_dn: signal,
                    residual_dn: (normalized_first - normalized_second) / residual_normalizer,
                });
            }
        }
        evidence.frame_centers_dn[0][site] = first_center + black;
        evidence.frame_centers_dn[1][site] = second_center + black;
        evidence.peak_signal_dn[site] = percentile(&mut peak_values, 0.999);
        evidence.samples[site] = site_samples;
    }
    Ok(evidence)
}

fn sample_cfa_pair(
    first: &[u16],
    second: &[u16],
    width: usize,
    height: usize,
    maximum_samples: usize,
) -> [Vec<(u16, u16)>; 4] {
    std::array::from_fn(|site| {
        let x_parity = site & 1;
        let y_parity = site >> 1;
        let site_width = (width + 1 - x_parity) / 2;
        let site_height = (height + 1 - y_parity) / 2;
        let total = site_width * site_height;
        let stride = total.div_ceil(maximum_samples).max(1);
        (0..total)
            .step_by(stride)
            .filter_map(|linear| {
                let site_x = linear % site_width;
                let site_y = linear / site_width;
                let x = site_x * 2 + x_parity;
                let y = site_y * 2 + y_parity;
                if x >= width || y >= height {
                    return None;
                }
                let index = y * width + x;
                Some((first[index], second[index]))
            })
            .take(maximum_samples)
            .collect()
    })
}

#[derive(Debug, Clone, Copy)]
struct VarianceBin {
    signal_dn: f32,
    variance_dn2: f32,
    count: usize,
}

fn variance_bins(samples: &[NoiseSample], signal_ceiling: f32) -> Vec<VarianceBin> {
    const BINS: usize = 32;
    let mut grouped: [Vec<f32>; BINS] = std::array::from_fn(|_| Vec::new());
    let mut signal_sum = [0.0f64; BINS];
    for sample in samples {
        if !sample.signal_dn.is_finite()
            || !sample.residual_dn.is_finite()
            || sample.signal_dn <= 0.0
            || sample.signal_dn > signal_ceiling
        {
            continue;
        }
        let bin = ((sample.signal_dn / signal_ceiling * BINS as f32) as usize).min(BINS - 1);
        grouped[bin].push(sample.residual_dn);
        signal_sum[bin] += f64::from(sample.signal_dn);
    }
    grouped
        .into_iter()
        .enumerate()
        .filter_map(|(bin, residuals)| {
            if residuals.len() < 256 {
                return None;
            }
            Some(VarianceBin {
                signal_dn: (signal_sum[bin] / residuals.len() as f64) as f32,
                variance_dn2: robust_second_moment(&residuals),
                count: residuals.len(),
            })
        })
        .collect()
}

fn robust_shot_slope(bins: &[VarianceBin], base_variance: f32) -> f32 {
    let mut ratios = bins
        .iter()
        .filter_map(|bin| {
            let excess = bin.variance_dn2 - base_variance;
            (excess > 0.0 && bin.signal_dn > 0.0).then_some((excess / bin.signal_dn, bin.count))
        })
        .collect::<Vec<_>>();
    if ratios.is_empty() {
        return f32::NAN;
    }
    ratios.sort_by(|left, right| left.0.total_cmp(&right.0));
    let total_weight = ratios.iter().map(|value| value.1).sum::<usize>();
    let mut cumulative = 0usize;
    let mut slope = ratios[ratios.len() / 2].0;
    for (ratio, weight) in ratios {
        cumulative += weight;
        if cumulative * 2 >= total_weight {
            slope = ratio;
            break;
        }
    }

    for _ in 0..8 {
        let mut numerator = 0.0f64;
        let mut denominator = 0.0f64;
        for bin in bins {
            let predicted = base_variance + slope * bin.signal_dn;
            let relative = (bin.variance_dn2 - predicted) / predicted.max(1e-6);
            let huber = if relative.abs() <= 0.10 {
                1.0
            } else {
                0.10 / relative.abs()
            };
            let weight = huber * bin.count as f32;
            numerator +=
                f64::from(weight * bin.signal_dn * (bin.variance_dn2 - base_variance).max(0.0));
            denominator += f64::from(weight * bin.signal_dn * bin.signal_dn);
        }
        if denominator > 0.0 {
            slope = (numerator / denominator) as f32;
        }
    }
    slope
}

fn maximum_variance_error(bins: &[VarianceBin], base_variance: f32, slope: f32) -> Option<f32> {
    bins.iter()
        .map(|bin| {
            let predicted = base_variance + slope * bin.signal_dn;
            (bin.variance_dn2 - predicted).abs() / predicted.max(1e-6)
        })
        .reduce(f32::max)
}

fn residual_coverage(
    dark: &[NoiseSample],
    flat: &[NoiseSample],
    read_noise_dn: f32,
    slope: f32,
    signal_ceiling: f32,
) -> (f32, f32, usize) {
    let mut inside_90 = 0usize;
    let mut inside_95 = 0usize;
    let mut count = 0usize;
    for sample in dark.iter().chain(flat) {
        if sample.signal_dn > signal_ceiling || !sample.residual_dn.is_finite() {
            continue;
        }
        let variance = read_noise_dn.powi(2)
            + if sample.signal_dn > 0.0 {
                slope * sample.signal_dn
            } else {
                0.0
            };
        let z = sample.residual_dn.abs() / variance.max(1e-9).sqrt();
        inside_90 += usize::from(z <= NORMAL_90);
        inside_95 += usize::from(z <= NORMAL_95);
        count += 1;
    }
    if count == 0 {
        return (0.0, 0.0, 0);
    }
    (
        inside_90 as f32 / count as f32,
        inside_95 as f32 / count as f32,
        count,
    )
}

fn collect_samples(pairs: &[PairEvidence], site: usize) -> Vec<NoiseSample> {
    pairs
        .iter()
        .flat_map(|pair| pair.samples[site].iter().copied())
        .collect()
}

fn average_fixed_pattern(pairs: &[PairEvidence], site: usize) -> Vec<f32> {
    let Some(length) = pairs
        .iter()
        .map(|pair| pair.fixed_pattern_residuals_dn[site].len())
        .min()
    else {
        return Vec::new();
    };
    let divisor = pairs.len() as f32;
    (0..length)
        .map(|index| {
            pairs
                .iter()
                .map(|pair| pair.fixed_pattern_residuals_dn[site][index])
                .sum::<f32>()
                / divisor
        })
        .collect()
}

fn distinct_levels(pairs: &[PairEvidence]) -> std::collections::BTreeSet<u32> {
    pairs.iter().filter_map(|pair| pair.level).collect()
}

fn independent_signal_level_count(
    pairs: &[PairEvidence],
    black_level: u16,
    white_level: u16,
) -> usize {
    let mut by_level: std::collections::BTreeMap<u32, Vec<f32>> = std::collections::BTreeMap::new();
    for pair in pairs {
        if let Some(level) = pair.level {
            let mean = pair
                .frame_centers_dn
                .into_iter()
                .flatten()
                .map(|value| value - f32::from(black_level))
                .sum::<f32>()
                / 8.0;
            by_level.entry(level).or_default().push(mean);
        }
    }
    let mut signals = by_level
        .into_values()
        .map(|values| robust_location(&values))
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    signals.sort_unstable_by(f32::total_cmp);
    let minimum_separation = f32::from(white_level - black_level) * 0.02;
    let mut independent = Vec::<f32>::new();
    for signal in signals {
        if independent
            .last()
            .map_or(true, |previous| signal - *previous >= minimum_separation)
        {
            independent.push(signal);
        }
    }
    independent.len()
}

fn robust_location(values: &[f32]) -> f32 {
    if values.is_empty() {
        return f32::NAN;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable_by(f32::total_cmp);
    let trim = (sorted.len() / 20).min(sorted.len().saturating_sub(1) / 2);
    let retained = &sorted[trim..sorted.len() - trim];
    retained.iter().map(|value| f64::from(*value)).sum::<f64>() as f32 / retained.len() as f32
}

fn robust_sigma(values: &[f32]) -> f32 {
    if values.len() < 2 {
        return f32::NAN;
    }
    let mut centered = values.to_vec();
    let center = percentile(&mut centered, 0.5);
    let mut deviations = values
        .iter()
        .map(|value| (value - center).abs())
        .collect::<Vec<_>>();
    let mad = percentile(&mut deviations, 0.5);
    let initial = (mad / 0.674_489_74).max(1e-6);
    let cutoff = initial * 6.0;
    let retained = values
        .iter()
        .filter(|value| (**value - center).abs() <= cutoff)
        .collect::<Vec<_>>();
    if retained.len() < 2 {
        return initial;
    }
    (retained
        .iter()
        .map(|value| {
            let centered = **value - center;
            f64::from(centered * centered)
        })
        .sum::<f64>()
        / retained.len() as f64)
        .sqrt() as f32
}

fn robust_second_moment(values: &[f32]) -> f32 {
    let scale = robust_sigma(values);
    if !scale.is_finite() || scale <= 0.0 {
        return f32::NAN;
    }
    let cutoff = scale * 6.0;
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for value in values {
        if value.abs() <= cutoff {
            sum += f64::from(value * value);
            count += 1;
        }
    }
    if count == 0 {
        f32::NAN
    } else {
        (sum / count as f64) as f32
    }
}

fn percentile(values: &mut [f32], fraction: f32) -> f32 {
    if values.is_empty() {
        return f32::NAN;
    }
    let index = ((values.len() - 1) as f32 * fraction.clamp(0.0, 1.0)).round() as usize;
    values.select_nth_unstable_by(index, f32::total_cmp);
    values[index]
}

fn finite_value(value: f32) -> Option<f32> {
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct DeterministicNoise {
        state: u64,
        spare: Option<f32>,
    }

    impl DeterministicNoise {
        fn new(seed: u64) -> Self {
            Self {
                state: seed,
                spare: None,
            }
        }

        fn uniform(&mut self) -> f32 {
            self.state ^= self.state << 13;
            self.state ^= self.state >> 7;
            self.state ^= self.state << 17;
            ((self.state >> 40) as f32 + 0.5) / ((1u32 << 24) as f32)
        }

        fn normal(&mut self) -> f32 {
            if let Some(value) = self.spare.take() {
                return value;
            }
            let radius = (-2.0 * self.uniform().max(1e-7).ln()).sqrt();
            let angle = std::f32::consts::TAU * self.uniform();
            self.spare = Some(radius * angle.sin());
            radius * angle.cos()
        }
    }

    fn synthetic_frame(
        width: usize,
        height: usize,
        black: u16,
        white: u16,
        signal_fraction: f32,
        read: [f32; 4],
        electrons_per_dn: [f32; 4],
        drift: [f32; 4],
        rng: &mut DeterministicNoise,
    ) -> Vec<u16> {
        let range = f32::from(white - black);
        let frame_drift: [f32; 4] = std::array::from_fn(|site| 0.15 * drift[site] * rng.normal());
        (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    let site = ((y & 1) << 1) | (x & 1);
                    (site, x, y)
                })
            })
            .map(|(site, x, y)| {
                let shading = 0.92 + 0.08 * ((x * 37 + y * 19 + x * y) % 997) as f32 / 996.0;
                let signal = signal_fraction * range * shading;
                let sigma = (read[site].powi(2) + signal / electrons_per_dn[site]).sqrt();
                let pattern_hash = (x as u64).wrapping_mul(0x9e37_79b1)
                    ^ (y as u64).wrapping_mul(0x85eb_ca77)
                    ^ (site as u64).wrapping_mul(0xc2b2_ae3d);
                let pattern_unit =
                    ((pattern_hash & 0xffff) as f32 / 65_535.0 - 0.5) * 12.0f32.sqrt();
                let fixed_pattern = drift[site] * pattern_unit;
                let value = f32::from(black)
                    + fixed_pattern
                    + frame_drift[site]
                    + signal
                    + sigma * rng.normal();
                value.round().clamp(0.0, f32::from(white)) as u16
            })
            .collect()
    }

    #[test]
    fn paired_photon_transfer_recovers_noise_and_passes_holdout_gates() {
        let width = 256;
        let height = 256;
        let black = 1000;
        let white = 16_000;
        let read = [2.0, 2.2, 2.4, 2.7];
        let gain = [0.65, 0.75, 0.85, 0.95];
        let drift = [0.2, 0.25, 0.3, 0.35];
        let config = SensorCalibrationConfig {
            max_samples_per_pair_per_site: 16_384,
            maximum_variance_relative_error: 0.12,
            coverage_absolute_tolerance: 0.035,
            ..SensorCalibrationConfig::default()
        };
        let mut accumulator =
            SensorCalibrationAccumulator::new(100, width, height, black, white, config).unwrap();
        let mut rng = DeterministicNoise::new(0x4d59_5df4_d0f3_3173);

        for pair in 0..8 {
            let first = synthetic_frame(
                width, height, black, white, 0.0, read, gain, drift, &mut rng,
            );
            let second = synthetic_frame(
                width, height, black, white, 0.0, read, gain, drift, &mut rng,
            );
            let split = if pair & 1 == 0 {
                CalibrationSplit::Fit
            } else {
                CalibrationSplit::Holdout
            };
            accumulator.add_dark_pair(&first, &second, split).unwrap();
        }
        for (level, signal) in [0.05, 0.15, 0.30, 0.50, 0.70, 0.86, 0.95]
            .into_iter()
            .enumerate()
        {
            for pair in 0..4 {
                let first = synthetic_frame(
                    width, height, black, white, signal, read, gain, drift, &mut rng,
                );
                let second = synthetic_frame(
                    width, height, black, white, signal, read, gain, drift, &mut rng,
                );
                let split = if pair & 1 == 0 {
                    CalibrationSplit::Fit
                } else {
                    CalibrationSplit::Holdout
                };
                accumulator
                    .add_flat_pair(level as u32, &first, &second, split)
                    .unwrap();
            }
        }

        let outcome = accumulator.evaluate().unwrap();
        if !outcome.report.passed {
            panic!(
                "calibration failed: {}",
                serde_json::to_string_pretty(&outcome.report).unwrap()
            );
        }
        let model = outcome.model.unwrap().model;
        for site in 0..4 {
            println!(
                "site {site}: read={:.4} fixed={:.4} frame_drift={:.4} combined_drift={:.4} e/DN={:.4} dark_var_error={:.4} holdout_var_error={:.4} coverage90={:.4} coverage95={:.4}",
                model.read_noise_dn[site],
                outcome.report.sites[site].fixed_pattern_dn,
                outcome.report.sites[site].frame_black_drift_dn,
                model.black_drift_dn[site],
                model.electrons_per_dn[site],
                outcome.report.sites[site]
                    .dark_holdout_variance_relative_error
                    .unwrap(),
                outcome.report.sites[site]
                    .maximum_holdout_variance_relative_error
                    .unwrap(),
                outcome.report.sites[site].holdout_coverage_90,
                outcome.report.sites[site].holdout_coverage_95,
            );
            assert!(
                (model.electrons_per_dn[site] / gain[site] - 1.0).abs() < 0.10,
                "site {site}: fitted {} expected {}",
                model.electrons_per_dn[site],
                gain[site]
            );
            assert!(
                outcome.report.sites[site]
                    .maximum_holdout_variance_relative_error
                    .unwrap()
                    < 0.12
            );
        }
    }

    #[test]
    fn incomplete_evidence_never_publishes_a_calibrated_model() {
        let config = SensorCalibrationConfig {
            minimum_dark_pairs_per_split: 2,
            minimum_flat_pairs_per_split: 1,
            ..SensorCalibrationConfig::default()
        };
        let mut accumulator =
            SensorCalibrationAccumulator::new(100, 64, 64, 1000, 16_000, config).unwrap();
        let frame = vec![1000u16; 64 * 64];
        accumulator
            .add_dark_pair(&frame, &frame, CalibrationSplit::Fit)
            .unwrap();
        accumulator
            .add_dark_pair(&frame, &frame, CalibrationSplit::Holdout)
            .unwrap();
        let outcome = accumulator.evaluate().unwrap();
        assert!(!outcome.report.passed);
        assert!(outcome.model.is_none());
        assert!(outcome
            .report
            .failures
            .iter()
            .any(|failure| failure.contains("flat levels")));
        assert!(outcome.report.sites[0]
            .maximum_fit_variance_relative_error
            .is_none());
        serde_json::to_string_pretty(&outcome.report).unwrap();
    }

    #[test]
    fn underexposed_flat_ramp_never_publishes_a_profile() {
        let width = 64;
        let height = 64;
        let black = 1000;
        let white = 16_000;
        let read = [2.0, 2.2, 2.4, 2.7];
        let gain = [0.65, 0.75, 0.85, 0.95];
        let drift = [0.2, 0.25, 0.3, 0.35];
        let config = SensorCalibrationConfig {
            max_samples_per_pair_per_site: 1024,
            minimum_flat_levels: 3,
            minimum_dark_pairs_per_split: 2,
            minimum_flat_pairs_per_split: 1,
            maximum_variance_relative_error: 0.25,
            coverage_absolute_tolerance: 0.10,
            ..SensorCalibrationConfig::default()
        };
        let mut accumulator =
            SensorCalibrationAccumulator::new(100, width, height, black, white, config).unwrap();
        let mut rng = DeterministicNoise::new(0x1134_f00d_a11c_e55e);
        for pair in 0..4 {
            let first = synthetic_frame(
                width, height, black, white, 0.0, read, gain, drift, &mut rng,
            );
            let second = synthetic_frame(
                width, height, black, white, 0.0, read, gain, drift, &mut rng,
            );
            accumulator
                .add_dark_pair(
                    &first,
                    &second,
                    if pair & 1 == 0 {
                        CalibrationSplit::Fit
                    } else {
                        CalibrationSplit::Holdout
                    },
                )
                .unwrap();
        }
        for (level, signal) in [0.10, 0.30, 0.60].into_iter().enumerate() {
            for pair in 0..2 {
                let first = synthetic_frame(
                    width, height, black, white, signal, read, gain, drift, &mut rng,
                );
                let second = synthetic_frame(
                    width, height, black, white, signal, read, gain, drift, &mut rng,
                );
                accumulator
                    .add_flat_pair(
                        level as u32,
                        &first,
                        &second,
                        if pair == 0 {
                            CalibrationSplit::Fit
                        } else {
                            CalibrationSplit::Holdout
                        },
                    )
                    .unwrap();
            }
        }
        let outcome = accumulator.evaluate().unwrap();
        assert!(outcome.model.is_none());
        assert!(outcome
            .report
            .failures
            .iter()
            .any(|failure| failure.contains("peak flat signal")));
    }
}
