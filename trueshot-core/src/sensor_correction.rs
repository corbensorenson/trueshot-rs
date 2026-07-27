//! Calibrated spatial sensor correction for native RAW inference.
//!
//! Flat-field response is represented by a compact per-CFA gain grid in full
//! sensor coordinates. Persistent outliers are retained separately as defect
//! coordinates so they can be replaced before interpolation rather than
//! blurred into neighboring pixels.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

pub const SENSOR_CORRECTION_PROFILE_SCHEMA: &str = "trueshot.sensor-correction.v1";
pub const MAX_SENSOR_CORRECTION_PROFILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_GRID_EDGE: usize = 128;
const MAX_DEFECT_PIXELS: usize = 65_536;
const MAX_CALIBRATION_PAIRS: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefectPixel {
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorCorrectionProfile {
    pub schema: String,
    pub camera_make: String,
    pub camera_model: String,
    pub bits_per_sample: u16,
    pub sensor_width: u32,
    pub sensor_height: u32,
    pub lens_model: String,
    pub aperture: f32,
    pub focal_length_mm: f32,
    pub focus_distance_min_m: f32,
    pub focus_distance_max_m: f32,
    pub grid_width: u16,
    pub grid_height: u16,
    /// Row-major grid with four local RGGB gains per cell.
    pub gain_by_cell_and_site: Vec<f32>,
    /// Sorted full-sensor coordinates.
    pub defects: Vec<DefectPixel>,
    pub fit_flat_pairs: u32,
    pub holdout_flat_pairs: u32,
    pub raw_holdout_p95_relative_error: f32,
    pub corrected_holdout_p95_relative_error: f32,
    #[serde(skip)]
    pub calibration_id: String,
}

impl SensorCorrectionProfile {
    pub fn validate(&self) -> Result<()> {
        if self.schema != SENSOR_CORRECTION_PROFILE_SCHEMA {
            anyhow::bail!(
                "Unsupported sensor correction schema {}; expected {}",
                self.schema,
                SENSOR_CORRECTION_PROFILE_SCHEMA
            );
        }
        let grid_width = usize::from(self.grid_width);
        let grid_height = usize::from(self.grid_height);
        let expected_gains = grid_width
            .checked_mul(grid_height)
            .and_then(|cells| cells.checked_mul(4))
            .context("Sensor correction grid dimensions overflow")?;
        if self.camera_make.trim().is_empty()
            || self.camera_model.trim().is_empty()
            || self.calibration_id.trim().is_empty()
            || self.bits_per_sample == 0
            || self.lens_model.trim().is_empty()
            || self.sensor_width < 16
            || self.sensor_height < 16
            || !(2..=MAX_GRID_EDGE).contains(&grid_width)
            || !(2..=MAX_GRID_EDGE).contains(&grid_height)
            || self.gain_by_cell_and_site.len() != expected_gains
            || self.defects.len() > MAX_DEFECT_PIXELS
            || self.fit_flat_pairs < 2
            || self.holdout_flat_pairs < 2
        {
            anyhow::bail!("Sensor correction profile identity or dimensions are invalid");
        }
        if !self.aperture.is_finite()
            || self.aperture <= 0.0
            || !self.focal_length_mm.is_finite()
            || self.focal_length_mm <= 0.0
            || !self.focus_distance_min_m.is_finite()
            || !self.focus_distance_max_m.is_finite()
            || self.focus_distance_min_m <= 0.0
            || self.focus_distance_max_m < self.focus_distance_min_m
            || !self.raw_holdout_p95_relative_error.is_finite()
            || !self.corrected_holdout_p95_relative_error.is_finite()
            || self
                .gain_by_cell_and_site
                .iter()
                .any(|gain| !gain.is_finite() || !(0.25..=4.0).contains(gain))
        {
            anyhow::bail!("Sensor correction profile contains invalid optical or gain values");
        }
        if self
            .defects
            .iter()
            .any(|pixel| pixel.x >= self.sensor_width || pixel.y >= self.sensor_height)
            || self.defects.windows(2).any(|pair| {
                defect_key(pair[0], self.sensor_width) >= defect_key(pair[1], self.sensor_width)
            })
        {
            anyhow::bail!("Sensor correction defects must be unique, sorted, and in bounds");
        }
        Ok(())
    }

    pub fn matches(
        &self,
        make: &str,
        model: &str,
        bits_per_sample: u16,
        width: u32,
        height: u32,
        lens_model: &str,
        aperture: f32,
        focal_length_mm: f32,
        focus_distance_m: f32,
    ) -> bool {
        normalized_camera_name(&self.camera_make) == normalized_camera_name(make)
            && normalized_camera_name(&self.camera_model) == normalized_camera_name(model)
            && self.bits_per_sample == bits_per_sample
            && self.sensor_width == width
            && self.sensor_height == height
            && normalized_camera_name(&self.lens_model) == normalized_camera_name(lens_model)
            && relative_match(self.aperture, aperture, 0.01)
            && relative_match(self.focal_length_mm, focal_length_mm, 0.005)
            && focus_in_envelope(
                self.focus_distance_min_m,
                self.focus_distance_max_m,
                focus_distance_m,
            )
    }

    pub fn gain_at(&self, sensor_x: f32, sensor_y: f32, site: usize) -> f32 {
        let grid_width = usize::from(self.grid_width);
        let grid_height = usize::from(self.grid_height);
        let gx = ((sensor_x + 0.5) / self.sensor_width as f32 * grid_width as f32 - 0.5)
            .clamp(0.0, (grid_width - 1) as f32);
        let gy = ((sensor_y + 0.5) / self.sensor_height as f32 * grid_height as f32 - 0.5)
            .clamp(0.0, (grid_height - 1) as f32);
        let x0 = gx.floor() as usize;
        let y0 = gy.floor() as usize;
        let x1 = (x0 + 1).min(grid_width - 1);
        let y1 = (y0 + 1).min(grid_height - 1);
        let tx = gx - x0 as f32;
        let ty = gy - y0 as f32;
        let sample =
            |x: usize, y: usize| self.gain_by_cell_and_site[(y * grid_width + x) * 4 + site.min(3)];
        let top = sample(x0, y0) + (sample(x1, y0) - sample(x0, y0)) * tx;
        let bottom = sample(x0, y1) + (sample(x1, y1) - sample(x0, y1)) * tx;
        top + (bottom - top) * ty
    }

    pub fn is_defective(&self, sensor_x: usize, sensor_y: usize) -> bool {
        if sensor_x >= self.sensor_width as usize || sensor_y >= self.sensor_height as usize {
            return false;
        }
        let target = sensor_y as u64 * u64::from(self.sensor_width) + sensor_x as u64;
        self.defects
            .binary_search_by_key(&target, |pixel| defect_key(*pixel, self.sensor_width))
            .is_ok()
    }

    pub fn load_json(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path)?;
        if metadata.len() > MAX_SENSOR_CORRECTION_PROFILE_BYTES {
            anyhow::bail!(
                "Sensor correction profile {} is {} bytes; limit is {}",
                path.display(),
                metadata.len(),
                MAX_SENSOR_CORRECTION_PROFILE_BYTES
            );
        }
        let bytes = std::fs::read(path)?;
        let mut profile: Self = serde_json::from_slice(&bytes)?;
        profile.calibration_id = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
        profile.validate()?;
        Ok(profile)
    }

    pub fn save_json(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec_pretty(self)?;
        if bytes.len() as u64 > MAX_SENSOR_CORRECTION_PROFILE_BYTES {
            anyhow::bail!("Serialized sensor correction profile exceeds the size limit");
        }
        let partial = path.with_extension(format!("partial-{}", std::process::id()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&partial)?;
        let write_result = (|| -> Result<()> {
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&partial, path)?;
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        write_result
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SensorCorrectionCalibrationConfig {
    pub grid_width: usize,
    pub grid_height: usize,
    pub minimum_fit_pairs: usize,
    pub minimum_holdout_pairs: usize,
    pub maximum_corrected_p95_relative_error: f32,
    pub minimum_error_reduction: f32,
    pub persistent_defect_fraction: f32,
    pub defect_low_ratio: f32,
    pub defect_high_ratio: f32,
    pub maximum_pair_disagreement: f32,
    pub minimum_flat_signal_fraction: f32,
    pub maximum_flat_signal_fraction: f32,
}

impl Default for SensorCorrectionCalibrationConfig {
    fn default() -> Self {
        Self {
            grid_width: 32,
            grid_height: 24,
            minimum_fit_pairs: 2,
            minimum_holdout_pairs: 2,
            maximum_corrected_p95_relative_error: 0.03,
            minimum_error_reduction: 0.50,
            persistent_defect_fraction: 0.75,
            defect_low_ratio: 0.45,
            defect_high_ratio: 1.75,
            maximum_pair_disagreement: 0.10,
            minimum_flat_signal_fraction: 0.10,
            maximum_flat_signal_fraction: 0.85,
        }
    }
}

impl SensorCorrectionCalibrationConfig {
    pub fn validate(&self) -> Result<()> {
        if !(2..=MAX_GRID_EDGE).contains(&self.grid_width)
            || !(2..=MAX_GRID_EDGE).contains(&self.grid_height)
            || !(2..=64).contains(&self.minimum_fit_pairs)
            || !(2..=64).contains(&self.minimum_holdout_pairs)
            || !self.maximum_corrected_p95_relative_error.is_finite()
            || !(0.005..=0.20).contains(&self.maximum_corrected_p95_relative_error)
            || !self.minimum_error_reduction.is_finite()
            || !(0.10..=0.90).contains(&self.minimum_error_reduction)
            || !self.persistent_defect_fraction.is_finite()
            || !(0.50..=1.0).contains(&self.persistent_defect_fraction)
            || !self.defect_low_ratio.is_finite()
            || !(0.05..=0.75).contains(&self.defect_low_ratio)
            || !self.defect_high_ratio.is_finite()
            || !(1.25..=8.0).contains(&self.defect_high_ratio)
            || self.defect_low_ratio >= 1.0
            || self.defect_high_ratio <= 1.0
            || !self.maximum_pair_disagreement.is_finite()
            || !(0.01..=0.50).contains(&self.maximum_pair_disagreement)
            || !self.minimum_flat_signal_fraction.is_finite()
            || !self.maximum_flat_signal_fraction.is_finite()
            || !(0.02..=0.40).contains(&self.minimum_flat_signal_fraction)
            || !(0.50..=0.95).contains(&self.maximum_flat_signal_fraction)
            || self.minimum_flat_signal_fraction >= self.maximum_flat_signal_fraction
        {
            anyhow::bail!("Sensor correction calibration configuration is invalid");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorrectionCalibrationSplit {
    Fit,
    Holdout,
}

#[derive(Debug)]
pub struct SensorCorrectionCalibrationOutcome {
    pub profile: Option<SensorCorrectionProfile>,
    pub failures: Vec<String>,
}

pub struct SensorCorrectionAccumulator {
    camera_make: String,
    camera_model: String,
    bits_per_sample: u16,
    width: usize,
    height: usize,
    black_level: u16,
    white_level: u16,
    aperture: f32,
    focal_length_mm: f32,
    focus_distance_min_m: f32,
    focus_distance_max_m: f32,
    lens_model: String,
    config: SensorCorrectionCalibrationConfig,
    fit_log_response_sum: Vec<f64>,
    fit_response_count: Vec<u32>,
    holdout_responses: Vec<Vec<f32>>,
    defect_hits: HashMap<u64, u32>,
    fit_pairs: usize,
}

impl SensorCorrectionAccumulator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        camera_make: String,
        camera_model: String,
        bits_per_sample: u16,
        width: usize,
        height: usize,
        black_level: u16,
        white_level: u16,
        lens_model: String,
        aperture: f32,
        focal_length_mm: f32,
        focus_distance_min_m: f32,
        focus_distance_max_m: f32,
        config: SensorCorrectionCalibrationConfig,
    ) -> Result<Self> {
        config.validate()?;
        if camera_make.trim().is_empty()
            || camera_model.trim().is_empty()
            || bits_per_sample == 0
            || width < 16
            || height < 16
            || white_level <= black_level
            || config.grid_width > width.div_ceil(2)
            || config.grid_height > height.div_ceil(2)
            || lens_model.trim().is_empty()
            || !aperture.is_finite()
            || aperture <= 0.0
            || !focal_length_mm.is_finite()
            || focal_length_mm <= 0.0
            || !focus_distance_min_m.is_finite()
            || !focus_distance_max_m.is_finite()
            || focus_distance_min_m <= 0.0
            || focus_distance_max_m < focus_distance_min_m
        {
            anyhow::bail!("Sensor correction calibration identity is invalid");
        }
        let values = config
            .grid_width
            .checked_mul(config.grid_height)
            .and_then(|cells| cells.checked_mul(4))
            .context("Sensor correction grid dimensions overflow")?;
        Ok(Self {
            camera_make,
            camera_model,
            bits_per_sample,
            width,
            height,
            black_level,
            white_level,
            aperture,
            focal_length_mm,
            focus_distance_min_m,
            focus_distance_max_m,
            lens_model,
            config,
            fit_log_response_sum: vec![0.0; values],
            fit_response_count: vec![0; values],
            holdout_responses: Vec::new(),
            defect_hits: HashMap::new(),
            fit_pairs: 0,
        })
    }

    pub fn add_flat_pair(
        &mut self,
        first: &[u16],
        second: &[u16],
        split: CorrectionCalibrationSplit,
    ) -> Result<()> {
        match split {
            CorrectionCalibrationSplit::Fit if self.fit_pairs >= MAX_CALIBRATION_PAIRS => {
                anyhow::bail!("Spatial correction fit-pair limit exceeded")
            }
            CorrectionCalibrationSplit::Holdout
                if self.holdout_responses.len() >= MAX_CALIBRATION_PAIRS =>
            {
                anyhow::bail!("Spatial correction holdout-pair limit exceeded")
            }
            _ => {}
        }
        let expected = self
            .width
            .checked_mul(self.height)
            .context("Correction calibration dimensions overflow")?;
        if first.len() != expected || second.len() != expected {
            anyhow::bail!("Correction calibration frame dimensions do not match the sensor");
        }
        let Some((response, site_targets)) = self.summarize_response(first, second)? else {
            return Ok(());
        };
        match split {
            CorrectionCalibrationSplit::Fit => {
                for (index, value) in response.iter().copied().enumerate() {
                    if value.is_finite() && value > 0.0 {
                        self.fit_log_response_sum[index] += f64::from(value.ln());
                        self.fit_response_count[index] += 1;
                    }
                }
                self.detect_defects(first, second, &response, site_targets)?;
                self.fit_pairs += 1;
            }
            CorrectionCalibrationSplit::Holdout => self.holdout_responses.push(response),
        }
        Ok(())
    }

    pub fn evaluate(self) -> Result<SensorCorrectionCalibrationOutcome> {
        let mut failures = Vec::new();
        if self.fit_pairs < self.config.minimum_fit_pairs {
            failures.push(format!(
                "fit flat pairs {} < {}",
                self.fit_pairs, self.config.minimum_fit_pairs
            ));
        }
        if self.holdout_responses.len() < self.config.minimum_holdout_pairs {
            failures.push(format!(
                "holdout flat pairs {} < {}",
                self.holdout_responses.len(),
                self.config.minimum_holdout_pairs
            ));
        }
        let mut gains = Vec::with_capacity(self.fit_log_response_sum.len());
        for (sum, count) in self
            .fit_log_response_sum
            .iter()
            .zip(&self.fit_response_count)
        {
            if *count == 0 {
                gains.push(f32::NAN);
            } else {
                gains.push((-sum / f64::from(*count)).exp() as f32);
            }
        }
        if gains
            .iter()
            .any(|gain| !gain.is_finite() || !(0.25..=4.0).contains(gain))
        {
            failures.push("fit gain grid contains missing or unsafe values".to_string());
        }

        let mut raw_errors = Vec::new();
        let mut corrected_errors = Vec::new();
        for response in &self.holdout_responses {
            for (index, value) in response.iter().copied().enumerate() {
                if value.is_finite() && value > 0.0 && gains[index].is_finite() {
                    raw_errors.push((value - 1.0).abs());
                    corrected_errors.push((value * gains[index] - 1.0).abs());
                }
            }
        }
        let raw_p95 = percentile(&mut raw_errors, 0.95);
        let corrected_p95 = percentile(&mut corrected_errors, 0.95);
        if !corrected_p95.is_finite()
            || corrected_p95 > self.config.maximum_corrected_p95_relative_error
        {
            failures.push(format!(
                "corrected holdout p95 {:.5} exceeds {:.5}",
                corrected_p95, self.config.maximum_corrected_p95_relative_error
            ));
        }
        if raw_p95.is_finite()
            && corrected_p95 > raw_p95 * (1.0 - self.config.minimum_error_reduction)
        {
            failures.push(format!(
                "holdout error reduction {:.1}% is below {:.1}%",
                (1.0 - corrected_p95 / raw_p95.max(1e-9)) * 100.0,
                self.config.minimum_error_reduction * 100.0
            ));
        }

        let required_hits =
            (self.fit_pairs as f32 * self.config.persistent_defect_fraction).ceil() as u32;
        let mut defects = self
            .defect_hits
            .into_iter()
            .filter_map(|(key, hits)| {
                (hits >= required_hits).then_some(DefectPixel {
                    x: (key % self.width as u64) as u32,
                    y: (key / self.width as u64) as u32,
                })
            })
            .collect::<Vec<_>>();
        defects.sort_unstable_by_key(|pixel| defect_key(*pixel, self.width as u32));
        if defects.len() > MAX_DEFECT_PIXELS {
            failures.push(format!(
                "persistent defect count {} exceeds {}",
                defects.len(),
                MAX_DEFECT_PIXELS
            ));
        }
        if !failures.is_empty() {
            return Ok(SensorCorrectionCalibrationOutcome {
                profile: None,
                failures,
            });
        }
        let profile = SensorCorrectionProfile {
            schema: SENSOR_CORRECTION_PROFILE_SCHEMA.to_string(),
            camera_make: self.camera_make,
            camera_model: self.camera_model,
            bits_per_sample: self.bits_per_sample,
            sensor_width: self.width as u32,
            sensor_height: self.height as u32,
            lens_model: self.lens_model,
            aperture: self.aperture,
            focal_length_mm: self.focal_length_mm,
            focus_distance_min_m: self.focus_distance_min_m,
            focus_distance_max_m: self.focus_distance_max_m,
            grid_width: self.config.grid_width as u16,
            grid_height: self.config.grid_height as u16,
            gain_by_cell_and_site: gains,
            defects,
            fit_flat_pairs: self.fit_pairs as u32,
            holdout_flat_pairs: self.holdout_responses.len() as u32,
            raw_holdout_p95_relative_error: raw_p95,
            corrected_holdout_p95_relative_error: corrected_p95,
            calibration_id: "unpublished:spatial-flat-field".to_string(),
        };
        profile.validate()?;
        Ok(SensorCorrectionCalibrationOutcome {
            profile: Some(profile),
            failures,
        })
    }

    fn summarize_response(
        &self,
        first: &[u16],
        second: &[u16],
    ) -> Result<Option<(Vec<f32>, [f32; 4])>> {
        let cells = self.config.grid_width * self.config.grid_height;
        let mut sums = vec![0.0f64; cells * 4];
        let mut counts = vec![0u32; cells * 4];
        let black = f32::from(self.black_level);
        let ceiling = f32::from(self.white_level - self.black_level) * 0.95;
        for y in 0..self.height {
            let grid_y = y * self.config.grid_height / self.height;
            for x in 0..self.width {
                let grid_x = x * self.config.grid_width / self.width;
                let site = ((y & 1) << 1) | (x & 1);
                let index = (grid_y * self.config.grid_width + grid_x) * 4 + site;
                let first_signal = (f32::from(first[y * self.width + x]) - black).max(0.0);
                let second_signal = (f32::from(second[y * self.width + x]) - black).max(0.0);
                let signal = 0.5 * (first_signal + second_signal);
                if signal > 0.0 && signal < ceiling {
                    sums[index] += f64::from(signal);
                    counts[index] += 1;
                }
            }
        }
        let mut response = sums
            .into_iter()
            .zip(counts)
            .map(|(sum, count)| {
                if count == 0 {
                    f32::NAN
                } else {
                    (sum / f64::from(count)) as f32
                }
            })
            .collect::<Vec<_>>();
        let mut site_targets = [0.0f32; 4];
        for site in 0..4 {
            let mut center = Vec::new();
            for grid_y in self.config.grid_height / 3..self.config.grid_height * 2 / 3 {
                for grid_x in self.config.grid_width / 3..self.config.grid_width * 2 / 3 {
                    let value = response[(grid_y * self.config.grid_width + grid_x) * 4 + site];
                    if value.is_finite() && value > 0.0 {
                        center.push(value);
                    }
                }
            }
            let target = percentile(&mut center, 0.5);
            if !target.is_finite() || target <= 0.0 {
                anyhow::bail!("Flat-field center has no valid CFA site {site} evidence");
            }
            site_targets[site] = target;
            response
                .iter_mut()
                .skip(site)
                .step_by(4)
                .for_each(|value| *value /= target);
        }
        let range = f32::from(self.white_level - self.black_level);
        let minimum = range * self.config.minimum_flat_signal_fraction;
        let maximum = range * self.config.maximum_flat_signal_fraction;
        if site_targets
            .iter()
            .any(|target| *target < minimum || *target > maximum)
        {
            return Ok(None);
        }
        Ok(Some((response, site_targets)))
    }

    fn detect_defects(
        &mut self,
        first: &[u16],
        second: &[u16],
        response: &[f32],
        site_targets: [f32; 4],
    ) -> Result<()> {
        let black = f32::from(self.black_level);
        let range = f32::from(self.white_level - self.black_level);
        for y in 0..self.height {
            let grid_y = y * self.config.grid_height / self.height;
            for x in 0..self.width {
                let grid_x = x * self.config.grid_width / self.width;
                let site = ((y & 1) << 1) | (x & 1);
                let expected_ratio =
                    response[(grid_y * self.config.grid_width + grid_x) * 4 + site];
                if !expected_ratio.is_finite() || expected_ratio <= 0.0 {
                    continue;
                }
                let index = y * self.width + x;
                let first_signal = (f32::from(first[index]) - black).max(0.0);
                let second_signal = (f32::from(second[index]) - black).max(0.0);
                let mean = 0.5 * (first_signal + second_signal);
                let expected = site_targets[site] * expected_ratio;
                if expected < range * 0.01 {
                    continue;
                }
                let disagreement = (first_signal - second_signal).abs() / expected.max(1.0);
                let ratio = mean / expected.max(1.0);
                if disagreement <= self.config.maximum_pair_disagreement
                    && (ratio <= self.config.defect_low_ratio
                        || ratio >= self.config.defect_high_ratio)
                {
                    if self.defect_hits.len() >= MAX_DEFECT_PIXELS * 16
                        && !self.defect_hits.contains_key(&(index as u64))
                    {
                        anyhow::bail!("Defect candidate set exceeded its calibration bound");
                    }
                    let hits = self.defect_hits.entry(index as u64).or_default();
                    *hits = hits
                        .checked_add(1)
                        .context("Defect evidence count overflow")?;
                }
            }
        }
        Ok(())
    }
}

fn relative_match(expected: f32, observed: f32, tolerance: f32) -> bool {
    observed.is_finite()
        && observed > 0.0
        && (expected - observed).abs() <= expected.abs().max(observed.abs()) * tolerance
}

fn focus_in_envelope(minimum: f32, maximum: f32, observed: f32) -> bool {
    if !observed.is_finite() || observed <= 0.0 {
        return false;
    }
    let tolerance = (maximum * 0.02).max(0.005);
    observed >= (minimum - tolerance).max(f32::MIN_POSITIVE) && observed <= maximum + tolerance
}

fn defect_key(pixel: DefectPixel, width: u32) -> u64 {
    u64::from(pixel.y) * u64::from(width) + u64::from(pixel.x)
}

fn normalized_camera_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn percentile(values: &mut [f32], fraction: f32) -> f32 {
    if values.is_empty() {
        return f32::NAN;
    }
    values.sort_unstable_by(f32::total_cmp);
    values[((values.len() - 1) as f32 * fraction).round() as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn synthetic_flat(width: usize, height: usize, phase: f32) -> Vec<u16> {
        let mut frame = vec![0u16; width * height];
        for y in 0..height {
            for x in 0..width {
                let nx = (x as f32 + 0.5) / width as f32 * 2.0 - 1.0;
                let ny = (y as f32 + 0.5) / height as f32 * 2.0 - 1.0;
                let shading = 1.0 - 0.38 * (nx * nx + ny * ny).min(1.0);
                let noise = 1.5 * ((x as f32 * 0.71 + y as f32 * 0.37 + phase).sin());
                frame[y * width + x] = (1_000.0 + 8_000.0 * shading + noise).round() as u16;
            }
        }
        frame[13 * width + 17] = 1_000;
        frame
    }

    #[test]
    fn calibrated_grid_flattens_holdout_and_finds_persistent_defect() {
        let width = 96;
        let height = 72;
        let mut accumulator = SensorCorrectionAccumulator::new(
            "NIKON CORPORATION".to_string(),
            "NIKON Z 9".to_string(),
            14,
            width,
            height,
            1_000,
            15_000,
            "NIKKOR Z MC 105mm f/2.8 VR S".to_string(),
            5.6,
            105.0,
            0.8,
            1.2,
            SensorCorrectionCalibrationConfig {
                grid_width: 12,
                grid_height: 9,
                maximum_corrected_p95_relative_error: 0.02,
                ..Default::default()
            },
        )
        .unwrap();
        for pair in 0..4 {
            let split = if pair < 2 {
                CorrectionCalibrationSplit::Fit
            } else {
                CorrectionCalibrationSplit::Holdout
            };
            accumulator
                .add_flat_pair(
                    &synthetic_flat(width, height, pair as f32),
                    &synthetic_flat(width, height, pair as f32 + 0.2),
                    split,
                )
                .unwrap();
        }
        let outcome = accumulator.evaluate().unwrap();
        assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
        let profile = outcome.profile.unwrap();
        assert!(profile.corrected_holdout_p95_relative_error < 0.02);
        assert!(
            profile.corrected_holdout_p95_relative_error
                < profile.raw_holdout_p95_relative_error * 0.5
        );
        assert!(profile.is_defective(17, 13));
        assert!(profile.gain_at(0.0, 0.0, 0) > 1.2);
    }

    #[test]
    fn profile_round_trip_is_digest_bound_and_optics_specific() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("correction.json");
        let profile = SensorCorrectionProfile {
            schema: SENSOR_CORRECTION_PROFILE_SCHEMA.to_string(),
            camera_make: "NIKON CORPORATION".to_string(),
            camera_model: "NIKON Z 9".to_string(),
            bits_per_sample: 14,
            sensor_width: 32,
            sensor_height: 24,
            lens_model: "NIKKOR Z MC 105mm f/2.8 VR S".to_string(),
            aperture: 8.0,
            focal_length_mm: 105.0,
            focus_distance_min_m: 0.8,
            focus_distance_max_m: 1.2,
            grid_width: 2,
            grid_height: 2,
            gain_by_cell_and_site: vec![1.0; 16],
            defects: vec![DefectPixel { x: 3, y: 7 }],
            fit_flat_pairs: 2,
            holdout_flat_pairs: 2,
            raw_holdout_p95_relative_error: 0.2,
            corrected_holdout_p95_relative_error: 0.01,
            calibration_id: "unpublished:test".to_string(),
        };
        profile.save_json(&path).unwrap();
        let loaded = SensorCorrectionProfile::load_json(&path).unwrap();
        assert!(loaded.calibration_id.starts_with("sha256:"));
        assert!(loaded.matches(
            "nikon corporation",
            "nikon z9",
            14,
            32,
            24,
            "NIKKOR Z MC 105mm f/2.8 VR S",
            8.0,
            105.0,
            1.0
        ));
        assert!(!loaded.matches(
            "nikon corporation",
            "nikon z9",
            14,
            32,
            24,
            "different lens",
            8.0,
            105.0,
            1.0
        ));
        assert!(!loaded.matches(
            "nikon corporation",
            "nikon z9",
            14,
            32,
            24,
            "NIKKOR Z MC 105mm f/2.8 VR S",
            8.0,
            105.0,
            2.0
        ));
    }

    #[test]
    fn shadow_flats_do_not_bias_the_spatial_fit() {
        let width = 32;
        let height = 24;
        let mut accumulator = SensorCorrectionAccumulator::new(
            "NIKON CORPORATION".to_string(),
            "NIKON Z 9".to_string(),
            14,
            width,
            height,
            1_000,
            15_000,
            "NIKKOR Z MC 105mm f/2.8 VR S".to_string(),
            5.6,
            105.0,
            0.8,
            1.2,
            SensorCorrectionCalibrationConfig {
                grid_width: 4,
                grid_height: 3,
                ..Default::default()
            },
        )
        .unwrap();
        let shadow = vec![1_500u16; width * height];
        for split in [
            CorrectionCalibrationSplit::Fit,
            CorrectionCalibrationSplit::Fit,
            CorrectionCalibrationSplit::Holdout,
            CorrectionCalibrationSplit::Holdout,
        ] {
            accumulator.add_flat_pair(&shadow, &shadow, split).unwrap();
        }
        let outcome = accumulator.evaluate().unwrap();
        assert!(outcome.profile.is_none());
        assert!(outcome
            .failures
            .iter()
            .any(|failure| failure.contains("fit flat pairs 0")));
        assert!(outcome
            .failures
            .iter()
            .any(|failure| failure.contains("holdout flat pairs 0")));
    }
}
