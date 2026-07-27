//! Memory-bounded HDR and focus fusion directly from native RAW group storage.
//!
//! Full-resolution inputs remain in the reusable `u16` arena. Only compact
//! green-channel analysis images use `f64` for FFT alignment; calibrated
//! radiance, focus measures, depth, and confidence remain `f32`.

use crate::align_raw::align_phasecorr_gray_with_scale;
use crate::smart_loader::NativeFrameGroup;
use crate::types::Meta;
use anyhow::{Context, Result};
use ndarray::{Array2, Array3};
use rayon::prelude::*;

const RGGB: [u8; 4] = [0, 1, 1, 2];

#[derive(Debug, Clone)]
pub struct NativeFusionConfig {
    /// Width and height of independently processed output bands/tiles.
    pub tile_size: usize,
    /// Context used by the focus operator. Values below two are promoted.
    pub halo: usize,
    /// Maximum edge of compact alignment images.
    pub analysis_max_dimension: usize,
    /// Pyramid levels used by the compact alignment implementation.
    pub alignment_levels: usize,
    /// Reject uncertain focus-plane transforms below this normalized score.
    pub minimum_alignment_score: f32,
    /// Optional sensor black-point override; metadata profile is the default.
    pub black_level: Option<f32>,
    /// Optional sensor saturation override; metadata profile is the default.
    pub white_level: Option<f32>,
    /// Approximate sensor read noise in native digital numbers.
    pub read_noise_dn: f32,
    /// Sensor-domain highlight taper begins at this normalized value.
    pub highlight_rolloff_start: f32,
    /// Apply confidence- and edge-aware depth regularization.
    pub regularize_depth: bool,
}

impl Default for NativeFusionConfig {
    fn default() -> Self {
        Self {
            tile_size: 256,
            halo: 3,
            analysis_max_dimension: 512,
            // The legacy multiscale implementation does not warp residuals
            // between levels. One high-resolution compact FFT is exact.
            alignment_levels: 1,
            minimum_alignment_score: 0.08,
            black_level: None,
            white_level: None,
            read_noise_dn: 3.0,
            highlight_rolloff_start: 0.88,
            regularize_depth: true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PlaneTransform {
    /// Shift that must be applied to this plane to align it to the reference.
    pub shift_x: f32,
    pub shift_y: f32,
    /// Source magnification sampled around the image center.
    pub source_scale: f32,
    /// Normalized cross-correlation after applying the transform.
    pub quality: f32,
    /// False means low-confidence estimation was rejected in favor of identity.
    pub accepted: bool,
}

impl PlaneTransform {
    pub fn identity() -> Self {
        Self {
            shift_x: 0.0,
            shift_y: 0.0,
            source_scale: 1.0,
            quality: 1.0,
            accepted: true,
        }
    }

    #[inline]
    fn source_coordinate(self, x: f32, y: f32, width: usize, height: usize) -> (f32, f32) {
        let center_x = (width.saturating_sub(1)) as f32 * 0.5;
        let center_y = (height.saturating_sub(1)) as f32 * 0.5;
        (
            center_x + (x - center_x) * self.source_scale - self.shift_x,
            center_y + (y - center_y) * self.source_scale - self.shift_y,
        )
    }
}

#[derive(Debug)]
pub struct NativeFusionResult {
    /// Linear, white-balanced Bayer mosaic in normalized scene-radiance space.
    pub bayer: Array3<f32>,
    /// Normalized focus depth, near/far ordering follows capture order.
    pub depth: Array2<f32>,
    /// Separation between the best and second-best focus hypotheses.
    pub confidence: Array2<f32>,
    /// Conservative object mask inferred from the shared crop border.
    pub foreground_mask: Array2<u8>,
    /// One transform per focus plane, shared by all bracketed exposures.
    pub transforms: Vec<PlaneTransform>,
    /// Shortest sensor exposure used as the radiance normalization anchor.
    pub radiance_anchor: f32,
}

impl NativeFusionResult {
    pub fn size_bytes(&self) -> usize {
        self.bayer.len() * std::mem::size_of::<f32>()
            + self.depth.len() * std::mem::size_of::<f32>()
            + self.confidence.len() * std::mem::size_of::<f32>()
            + self.foreground_mask.len()
            + self.transforms.len() * std::mem::size_of::<PlaneTransform>()
    }
}

#[derive(Debug, Clone, Copy)]
struct FrameCalibration {
    black: f32,
    inverse_range: f32,
    exposure: f32,
    wb_by_site: [f32; 4],
}

/// Fuse one ordered focus/HDR group without materializing calibrated or aligned
/// full-frame copies.
pub fn fuse_native_group(
    group: &NativeFrameGroup<'_>,
    meta: &Meta,
    config: &NativeFusionConfig,
) -> Result<NativeFusionResult> {
    validate_group(group, meta, config)?;

    let focus_steps = usize::from(meta.focus_steps).max(1);
    let exposures_per_focus = group.len() / focus_steps;
    let calibrations = build_calibrations(group, config)?;
    let radiance_anchor = calibrations
        .iter()
        .map(|calibration| calibration.exposure)
        .fold(f32::INFINITY, f32::min);
    if !radiance_anchor.is_finite() || radiance_anchor <= 0.0 {
        anyhow::bail!("Capture group has no valid exposure calibration");
    }

    let transforms =
        estimate_plane_transforms(group, meta, config, &calibrations, exposures_per_focus)?;

    let width = group.width;
    let height = group.height;
    let pixel_count = width
        .checked_mul(height)
        .context("Native fusion dimensions overflow")?;
    let mut bayer = vec![0.0f32; pixel_count];
    let mut depth = vec![0.0f32; pixel_count];
    let mut confidence = vec![0.0f32; pixel_count];
    let band_rows = config.tile_size.max(16);
    let band_len = width
        .checked_mul(band_rows)
        .context("Native fusion band dimensions overflow")?;

    bayer
        .par_chunks_mut(band_len)
        .zip(depth.par_chunks_mut(band_len))
        .zip(confidence.par_chunks_mut(band_len))
        .enumerate()
        .try_for_each(
            |(band_index, ((bayer_band, depth_band), confidence_band))| -> Result<()> {
                let y0 = band_index * band_rows;
                let y1 = (y0 + bayer_band.len() / width).min(height);
                process_band(
                    group,
                    &calibrations,
                    &transforms,
                    focus_steps,
                    exposures_per_focus,
                    radiance_anchor,
                    config,
                    y0,
                    y1,
                    bayer_band,
                    depth_band,
                    confidence_band,
                )
            },
        )?;

    if config.regularize_depth && focus_steps > 1 {
        regularize_depth_map(&bayer, &mut depth, &confidence, width, height);
    }
    let foreground_mask = infer_foreground_mask(&bayer, width, height);

    Ok(NativeFusionResult {
        bayer: Array3::from_shape_vec((height, width, 1), bayer)
            .context("Unable to shape fused Bayer output")?,
        depth: Array2::from_shape_vec((height, width), depth)
            .context("Unable to shape fused depth output")?,
        confidence: Array2::from_shape_vec((height, width), confidence)
            .context("Unable to shape fused confidence output")?,
        foreground_mask: Array2::from_shape_vec((height, width), foreground_mask)
            .context("Unable to shape fused foreground mask")?,
        transforms,
        radiance_anchor,
    })
}

fn validate_group(
    group: &NativeFrameGroup<'_>,
    meta: &Meta,
    config: &NativeFusionConfig,
) -> Result<()> {
    if group.is_empty() || group.width == 0 || group.height == 0 {
        anyhow::bail!("Cannot fuse an empty native frame group");
    }
    let focus_steps = usize::from(meta.focus_steps).max(1);
    if group.len() % focus_steps != 0 {
        anyhow::bail!(
            "Capture group has {} frames, not an integral {} focus planes",
            group.len(),
            focus_steps
        );
    }
    if group.metadata.len() != group.len() {
        anyhow::bail!("Native group metadata is incomplete");
    }
    if config.black_level.is_some_and(|level| level < 0.0) || config.read_noise_dn <= 0.0 {
        anyhow::bail!("Native fusion calibration values must be positive");
    }
    if group.metadata[0].cfa_pattern != RGGB {
        anyhow::bail!(
            "Native AHD currently requires RGGB CFA, found {:?}",
            group.metadata[0].cfa_pattern
        );
    }
    Ok(())
}

fn build_calibrations(
    group: &NativeFrameGroup<'_>,
    config: &NativeFusionConfig,
) -> Result<Vec<FrameCalibration>> {
    group
        .metadata
        .iter()
        .enumerate()
        .map(|(index, metadata)| {
            let profile = metadata.sensor_levels.with_context(|| {
                format!(
                    "Frame {} has no verified sensor calibration for {} {} {}-bit RAW",
                    index, metadata.camera_make, metadata.camera_model, metadata.bits_per_sample
                )
            })?;
            let black = config.black_level.unwrap_or(f32::from(profile.black));
            let white = config.white_level.unwrap_or(f32::from(profile.white));
            if white <= black {
                anyhow::bail!(
                    "Frame {} white level {} does not exceed black level {}",
                    index,
                    white,
                    black
                );
            }
            let shutter = metadata.exposure_time.unwrap_or(1.0 / 125.0).max(1e-7);
            let aperture = metadata.aperture.unwrap_or(5.6).max(0.1) as f64;
            let iso = metadata.iso.unwrap_or(100).max(1) as f64;
            // Sensor exposure is proportional to irradiance integration time
            // and analog gain, and inversely proportional to f-number squared.
            let exposure = (shutter * (iso / 100.0) / aperture.powi(2)) as f32;
            let green = ((metadata.cam_mul[1] + metadata.cam_mul[3]) * 0.5).max(1e-6);
            Ok(FrameCalibration {
                black,
                inverse_range: 1.0 / (white - black),
                exposure,
                // Local RGGB site order: R, G1, G2, B.
                wb_by_site: [
                    metadata.cam_mul[0] / green,
                    metadata.cam_mul[1] / green,
                    metadata.cam_mul[3] / green,
                    metadata.cam_mul[2] / green,
                ],
            })
        })
        .collect()
}

fn estimate_plane_transforms(
    group: &NativeFrameGroup<'_>,
    meta: &Meta,
    config: &NativeFusionConfig,
    calibrations: &[FrameCalibration],
    exposures_per_focus: usize,
) -> Result<Vec<PlaneTransform>> {
    let focus_steps = usize::from(meta.focus_steps).max(1);
    if focus_steps == 1 || group.width < 32 || group.height < 32 {
        return Ok(vec![PlaneTransform::identity(); focus_steps]);
    }

    let reference_focus = usize::from(meta.ref_focus).min(focus_steps - 1);
    let target_exposure = median(
        &mut calibrations
            .iter()
            .map(|calibration| calibration.exposure)
            .collect::<Vec<_>>(),
    );
    let selected_frames: Vec<usize> = (0..focus_steps)
        .map(|focus| {
            let start = focus * exposures_per_focus;
            (start..start + exposures_per_focus)
                .min_by(|&left, &right| {
                    exposure_log_distance(calibrations[left].exposure, target_exposure).total_cmp(
                        &exposure_log_distance(calibrations[right].exposure, target_exposure),
                    )
                })
                .unwrap_or(start)
        })
        .collect();

    let analysis_stride = analysis_stride(group.width, group.height, config);
    let reference = build_green_analysis(
        group,
        selected_frames[reference_focus],
        calibrations[selected_frames[reference_focus]],
        analysis_stride,
    )?;
    let mut transforms = Vec::with_capacity(focus_steps);
    for focus in 0..focus_steps {
        if focus == reference_focus {
            transforms.push(PlaneTransform::identity());
            continue;
        }
        let frame_index = selected_frames[focus];
        let analysis = build_green_analysis(
            group,
            frame_index,
            calibrations[frame_index],
            analysis_stride,
        )?;
        let levels = config.alignment_levels.max(1);
        let (dx, dy, scale) = align_phasecorr_gray_with_scale(&reference, &analysis, levels);
        let quality = transformed_ncc(&reference, &analysis, dx, dy, scale) as f32;
        let full_resolution_factor = (analysis_stride * 2) as f32;
        let shift_x = dx as f32 * full_resolution_factor;
        let shift_y = dy as f32 * full_resolution_factor;
        let scale = (scale as f32).clamp(0.95, 1.05);
        let plausible_shift = shift_x.abs() <= group.width as f32 * 0.12
            && shift_y.abs() <= group.height as f32 * 0.12;
        let accepted = quality >= config.minimum_alignment_score && plausible_shift;
        if accepted {
            transforms.push(PlaneTransform {
                shift_x,
                shift_y,
                source_scale: scale,
                quality,
                accepted: true,
            });
        } else {
            tracing::warn!(
                "Rejected focus-plane {} transform dx={:.2} dy={:.2} scale={:.5} NCC={:.3}",
                focus,
                shift_x,
                shift_y,
                scale,
                quality
            );
            transforms.push(PlaneTransform {
                quality,
                accepted: false,
                ..PlaneTransform::identity()
            });
        }
    }
    Ok(transforms)
}

fn analysis_stride(width: usize, height: usize, config: &NativeFusionConfig) -> usize {
    let green_edge = (width / 2).max(height / 2);
    let maximum = config.analysis_max_dimension.max(32);
    green_edge.div_ceil(maximum).max(1)
}

fn build_green_analysis(
    group: &NativeFrameGroup<'_>,
    frame_index: usize,
    calibration: FrameCalibration,
    stride: usize,
) -> Result<Array2<f64>> {
    let frame = group
        .frame(frame_index)
        .with_context(|| format!("Missing native frame {}", frame_index))?;
    let green_width = group.width / 2;
    let green_height = group.height / 2;
    let width = green_width / stride;
    let height = green_height / stride;
    if width < 8 || height < 8 {
        anyhow::bail!("Native group is too small for compact alignment");
    }

    let mut output = Array2::<f64>::zeros((height, width));
    for ay in 0..height {
        for ax in 0..width {
            let mut sum = 0.0f64;
            let mut count = 0usize;
            for block_y in 0..stride {
                for block_x in 0..stride {
                    let x = (ax * stride + block_x) * 2;
                    let y = (ay * stride + block_y) * 2;
                    let g1 = normalize_raw(frame[y * group.width + x + 1], calibration);
                    let g2 = normalize_raw(frame[(y + 1) * group.width + x], calibration);
                    sum += ((g1 + g2) * 0.5) as f64;
                    count += 1;
                }
            }
            output[[ay, ax]] = sum / count as f64;
        }
    }

    let mean = output.iter().sum::<f64>() / output.len() as f64;
    let variance = output
        .iter()
        .map(|value| {
            let centered = *value - mean;
            centered * centered
        })
        .sum::<f64>()
        / output.len() as f64;
    let inverse_stddev = 1.0 / variance.sqrt().max(1e-8);
    output.mapv_inplace(|value| (value - mean) * inverse_stddev);
    Ok(output)
}

fn transformed_ncc(
    reference: &Array2<f64>,
    frame: &Array2<f64>,
    shift_x: f64,
    shift_y: f64,
    source_scale: f64,
) -> f64 {
    let (height, width) = reference.dim();
    let center_x = (width.saturating_sub(1)) as f64 * 0.5;
    let center_y = (height.saturating_sub(1)) as f64 * 0.5;
    let margin_x = width / 8;
    let margin_y = height / 8;
    let mut dot = 0.0;
    let mut ref_energy = 0.0;
    let mut frame_energy = 0.0;
    let mut count = 0usize;
    for y in margin_y..height.saturating_sub(margin_y) {
        for x in margin_x..width.saturating_sub(margin_x) {
            let source_x = center_x + (x as f64 - center_x) * source_scale - shift_x;
            let source_y = center_y + (y as f64 - center_y) * source_scale - shift_y;
            if let Some(sample) = bilinear_f64(frame, source_x, source_y) {
                let reference_value = reference[[y, x]];
                dot += reference_value * sample;
                ref_energy += reference_value * reference_value;
                frame_energy += sample * sample;
                count += 1;
            }
        }
    }
    if count < 64 || ref_energy <= 1e-12 || frame_energy <= 1e-12 {
        return -1.0;
    }
    dot / (ref_energy * frame_energy).sqrt()
}

fn process_band(
    group: &NativeFrameGroup<'_>,
    calibrations: &[FrameCalibration],
    transforms: &[PlaneTransform],
    focus_steps: usize,
    exposures_per_focus: usize,
    radiance_anchor: f32,
    config: &NativeFusionConfig,
    band_y0: usize,
    band_y1: usize,
    bayer_output: &mut [f32],
    depth_output: &mut [f32],
    confidence_output: &mut [f32],
) -> Result<()> {
    let width = group.width;
    let halo = config.halo.max(2);
    let tile_size = config.tile_size.max(16);
    for x0 in (0..width).step_by(tile_size) {
        let x1 = (x0 + tile_size).min(width);
        let ext_x0 = x0.saturating_sub(halo);
        let ext_y0 = band_y0.saturating_sub(halo);
        let ext_x1 = (x1 + halo).min(width);
        let ext_y1 = (band_y1 + halo).min(group.height);
        let ext_width = ext_x1 - ext_x0;
        let ext_height = ext_y1 - ext_y0;
        let ext_pixels = ext_width * ext_height;
        let tile_width = x1 - x0;
        let tile_height = band_y1 - band_y0;
        let tile_pixels = tile_width * tile_height;

        let mut plane_bayer = vec![0.0f32; ext_pixels];
        let mut plane_valid = vec![false; ext_pixels];
        let mut green = vec![0.0f32; ext_pixels];
        let mut focus_metric = vec![0.0f32; ext_pixels];
        let mut best_score = vec![f32::NEG_INFINITY; tile_pixels];
        let mut second_score = vec![f32::NEG_INFINITY; tile_pixels];
        let mut best_value = vec![0.0f32; tile_pixels];
        let mut second_value = vec![0.0f32; tile_pixels];
        let mut best_plane = vec![0u16; tile_pixels];
        let mut second_plane = vec![0u16; tile_pixels];

        for focus in 0..focus_steps {
            plane_bayer.fill(0.0);
            plane_valid.fill(false);
            green.fill(0.0);
            focus_metric.fill(0.0);
            let frame_start = focus * exposures_per_focus;
            let frame_end = frame_start + exposures_per_focus;
            for local_y in 0..ext_height {
                let output_y = ext_y0 + local_y;
                for local_x in 0..ext_width {
                    let output_x = ext_x0 + local_x;
                    let index = local_y * ext_width + local_x;
                    if let Some(value) = fuse_hdr_sample(
                        group,
                        calibrations,
                        frame_start,
                        frame_end,
                        transforms[focus],
                        output_x,
                        output_y,
                        radiance_anchor,
                        config,
                    ) {
                        plane_bayer[index] = value;
                        plane_valid[index] = true;
                    }
                }
            }

            interpolate_green_mosaic(
                &plane_bayer,
                &plane_valid,
                &mut green,
                ext_width,
                ext_height,
                ext_x0,
                ext_y0,
            );
            compute_focus_metric(
                &green,
                &plane_valid,
                &mut focus_metric,
                ext_width,
                ext_height,
            );

            for tile_y in 0..tile_height {
                let ext_y = band_y0 + tile_y - ext_y0;
                for tile_x in 0..tile_width {
                    let ext_x = x0 + tile_x - ext_x0;
                    let ext_index = ext_y * ext_width + ext_x;
                    if !plane_valid[ext_index] {
                        continue;
                    }
                    let metric =
                        smoothed_metric(&focus_metric, ext_width, ext_height, ext_x, ext_y);
                    let tile_index = tile_y * tile_width + tile_x;
                    let value = plane_bayer[ext_index];
                    if metric > best_score[tile_index] {
                        second_score[tile_index] = best_score[tile_index];
                        second_value[tile_index] = best_value[tile_index];
                        second_plane[tile_index] = best_plane[tile_index];
                        best_score[tile_index] = metric;
                        best_value[tile_index] = value;
                        best_plane[tile_index] = focus as u16;
                    } else if metric > second_score[tile_index] {
                        second_score[tile_index] = metric;
                        second_value[tile_index] = value;
                        second_plane[tile_index] = focus as u16;
                    }
                }
            }
        }

        let depth_denominator = focus_steps.saturating_sub(1).max(1) as f32;
        for tile_y in 0..tile_height {
            for tile_x in 0..tile_width {
                let tile_index = tile_y * tile_width + tile_x;
                let output_index = tile_y * width + x0 + tile_x;
                if !best_score[tile_index].is_finite() {
                    continue;
                }
                let second_is_valid = second_score[tile_index].is_finite();
                let second = if second_is_valid {
                    second_score[tile_index].max(0.0)
                } else {
                    0.0
                };
                let best = best_score[tile_index].max(0.0);
                let separation = ((best - second) / (best + second + 1e-8)).clamp(0.0, 1.0);
                let best_weight = if second_is_valid {
                    0.5 + 0.5 * separation
                } else {
                    1.0
                };
                bayer_output[output_index] = best_value[tile_index] * best_weight
                    + second_value[tile_index] * (1.0 - best_weight);
                let focus_position = best_plane[tile_index] as f32 * best_weight
                    + second_plane[tile_index] as f32 * (1.0 - best_weight);
                depth_output[output_index] = focus_position / depth_denominator;
                confidence_output[output_index] = separation;
            }
        }
    }
    Ok(())
}

fn fuse_hdr_sample(
    group: &NativeFrameGroup<'_>,
    calibrations: &[FrameCalibration],
    frame_start: usize,
    frame_end: usize,
    transform: PlaneTransform,
    output_x: usize,
    output_y: usize,
    radiance_anchor: f32,
    config: &NativeFusionConfig,
) -> Option<f32> {
    let (source_x, source_y) =
        transform.source_coordinate(output_x as f32, output_y as f32, group.width, group.height);
    let site = ((output_y & 1) << 1) | (output_x & 1);
    let mut weighted_sum = 0.0f32;
    let mut weighted_square_sum = 0.0f32;
    let mut weight_sum = 0.0f32;
    let mut fallback = None::<(f32, f32)>;

    for frame_index in frame_start..frame_end {
        let frame = group.frame(frame_index)?;
        let calibration = calibrations[frame_index];
        let raw = sample_same_cfa(
            frame,
            group.width,
            group.height,
            source_x,
            source_y,
            output_x & 1,
            output_y & 1,
        )?;
        let signal = ((raw - calibration.black) * calibration.inverse_range).max(0.0);
        let radiance =
            signal * radiance_anchor / calibration.exposure * calibration.wb_by_site[site];
        let fallback_score = calibration.exposure * (1.0 - signal.min(1.0));
        if fallback
            .map(|(score, _)| fallback_score > score)
            .unwrap_or(true)
        {
            fallback = Some((fallback_score, radiance));
        }

        let read_noise = config.read_noise_dn * calibration.inverse_range;
        let shadow_confidence = smoothstep(read_noise * 2.0, read_noise * 12.0, signal);
        let highlight_confidence = 1.0 - smoothstep(config.highlight_rolloff_start, 0.995, signal);
        let shot_variance = signal.max(0.0) * calibration.inverse_range;
        let variance = read_noise * read_noise + shot_variance + 1e-12;
        let relative_exposure = calibration.exposure / radiance_anchor;
        let weight = relative_exposure
            * relative_exposure
            * shadow_confidence
            * highlight_confidence
            * highlight_confidence
            / variance;
        if weight.is_finite() && weight > 0.0 {
            weighted_sum += weight * radiance;
            weighted_square_sum += weight * radiance * radiance;
            weight_sum += weight;
        }
    }

    if weight_sum <= 0.0 {
        return fallback.map(|(_, value)| value);
    }
    let initial_mean = weighted_sum / weight_sum;
    let variance = (weighted_square_sum / weight_sum - initial_mean * initial_mean).max(0.0);
    let robust_scale = variance.sqrt().max(2e-4);
    let mut robust_sum = 0.0f32;
    let mut robust_weight_sum = 0.0f32;

    // A second pass rejects bracket motion and hot-pixel outliers without
    // allocating a sample vector for every output pixel.
    for frame_index in frame_start..frame_end {
        let frame = group.frame(frame_index)?;
        let calibration = calibrations[frame_index];
        let raw = sample_same_cfa(
            frame,
            group.width,
            group.height,
            source_x,
            source_y,
            output_x & 1,
            output_y & 1,
        )?;
        let signal = ((raw - calibration.black) * calibration.inverse_range).max(0.0);
        let radiance =
            signal * radiance_anchor / calibration.exposure * calibration.wb_by_site[site];
        let read_noise = config.read_noise_dn * calibration.inverse_range;
        let shadow_confidence = smoothstep(read_noise * 2.0, read_noise * 12.0, signal);
        let highlight_confidence = 1.0 - smoothstep(config.highlight_rolloff_start, 0.995, signal);
        let shot_variance = signal.max(0.0) * calibration.inverse_range;
        let sensor_variance = read_noise * read_noise + shot_variance + 1e-12;
        let relative_exposure = calibration.exposure / radiance_anchor;
        let sensor_weight = relative_exposure
            * relative_exposure
            * shadow_confidence
            * highlight_confidence
            * highlight_confidence
            / sensor_variance;
        let normalized_residual = (radiance - initial_mean) / (4.685 * robust_scale);
        let robust_weight = if normalized_residual.abs() < 1.0 {
            let remaining = 1.0 - normalized_residual * normalized_residual;
            remaining * remaining
        } else {
            0.0
        };
        let weight = sensor_weight * robust_weight;
        if weight.is_finite() && weight > 0.0 {
            robust_sum += weight * radiance;
            robust_weight_sum += weight;
        }
    }

    Some(if robust_weight_sum > 0.0 {
        robust_sum / robust_weight_sum
    } else {
        initial_mean
    })
}

fn sample_same_cfa(
    frame: &[u16],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    parity_x: usize,
    parity_y: usize,
) -> Option<f32> {
    if x < -2.0 || y < -2.0 || x > width as f32 + 1.0 || y > height as f32 + 1.0 {
        return None;
    }
    let (x0, x1, tx) = same_parity_axis(x, parity_x, width)?;
    let (y0, y1, ty) = same_parity_axis(y, parity_y, height)?;
    let p00 = frame[y0 * width + x0] as f32;
    let p10 = frame[y0 * width + x1] as f32;
    let p01 = frame[y1 * width + x0] as f32;
    let p11 = frame[y1 * width + x1] as f32;
    let top = p00 + (p10 - p00) * tx;
    let bottom = p01 + (p11 - p01) * tx;
    Some(top + (bottom - top) * ty)
}

fn same_parity_axis(coordinate: f32, parity: usize, limit: usize) -> Option<(usize, usize, f32)> {
    if limit <= parity {
        return None;
    }
    let first = parity;
    let last = if (limit - 1) & 1 == parity {
        limit - 1
    } else {
        limit.checked_sub(2)?
    };
    let grid = (coordinate - parity as f32) * 0.5;
    let lower_grid = grid.floor();
    let mut lower = parity as isize + lower_grid as isize * 2;
    lower = lower.clamp(first as isize, last as isize);
    let upper = (lower + 2).min(last as isize);
    let fraction = if upper == lower {
        0.0
    } else {
        ((coordinate - lower as f32) / (upper - lower) as f32).clamp(0.0, 1.0)
    };
    Some((lower as usize, upper as usize, fraction))
}

fn interpolate_green_mosaic(
    bayer: &[f32],
    valid: &[bool],
    green: &mut [f32],
    width: usize,
    height: usize,
    origin_x: usize,
    origin_y: usize,
) {
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if !valid[index] {
                continue;
            }
            if ((origin_x + x) ^ (origin_y + y)) & 1 == 1 {
                green[index] = bayer[index];
                continue;
            }
            let mut sum = 0.0;
            let mut count = 0.0;
            if x > 0 && valid[index - 1] {
                sum += bayer[index - 1];
                count += 1.0;
            }
            if x + 1 < width && valid[index + 1] {
                sum += bayer[index + 1];
                count += 1.0;
            }
            if y > 0 && valid[index - width] {
                sum += bayer[index - width];
                count += 1.0;
            }
            if y + 1 < height && valid[index + width] {
                sum += bayer[index + width];
                count += 1.0;
            }
            green[index] = if count > 0.0 {
                sum / count
            } else {
                bayer[index]
            };
        }
    }
}

fn compute_focus_metric(
    green: &[f32],
    valid: &[bool],
    metric: &mut [f32],
    width: usize,
    height: usize,
) {
    if width < 3 || height < 3 {
        return;
    }
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let index = y * width + x;
            if !valid[index] {
                continue;
            }
            let center = green[index];
            let horizontal = (green[index - 1] - 2.0 * center + green[index + 1]).abs();
            let vertical = (green[index - width] - 2.0 * center + green[index + width]).abs();
            let gradient_x = (green[index + 1] - green[index - 1]).abs() * 0.5;
            let gradient_y = (green[index + width] - green[index - width]).abs() * 0.5;
            let response = horizontal + vertical + 0.35 * (gradient_x + gradient_y);
            // Normalize signal-dependent shot noise so bright smooth regions do
            // not automatically outrank darker, genuinely focused structure.
            metric[index] = response / (0.0015 + 0.02 * center.max(0.0).sqrt());
        }
    }
}

fn smoothed_metric(metric: &[f32], width: usize, height: usize, x: usize, y: usize) -> f32 {
    let x0 = x.saturating_sub(1);
    let y0 = y.saturating_sub(1);
    let x1 = (x + 1).min(width - 1);
    let y1 = (y + 1).min(height - 1);
    let mut values = [0.0f32; 9];
    let mut count = 0usize;
    for yy in y0..=y1 {
        for xx in x0..=x1 {
            values[count] = metric[yy * width + xx];
            count += 1;
        }
    }
    // A trimmed local mean is robust to single-pixel noise and hot pixels.
    values[..count].sort_unstable_by(f32::total_cmp);
    let trim = usize::from(count >= 7);
    let kept = &values[trim..count - trim];
    kept.iter().sum::<f32>() / kept.len().max(1) as f32
}

fn regularize_depth_map(
    bayer: &[f32],
    depth: &mut Vec<f32>,
    confidence: &[f32],
    width: usize,
    height: usize,
) {
    if width < 3 || height < 3 {
        return;
    }
    let source = depth.clone();
    depth
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, output_row)| {
            for x in 0..width {
                let index = y * width + x;
                let confidence_here = confidence[index];
                if confidence_here >= 0.85 {
                    output_row[x] = source[index];
                    continue;
                }
                let center_green = fused_green(bayer, width, height, x, y);
                let mut weighted_depth = 0.0;
                let mut weight_sum = 0.0;
                let y0 = y.saturating_sub(2);
                let y1 = (y + 2).min(height - 1);
                let x0 = x.saturating_sub(2);
                let x1 = (x + 2).min(width - 1);
                for yy in y0..=y1 {
                    for xx in x0..=x1 {
                        let neighbor = yy * width + xx;
                        let distance = x.abs_diff(xx) + y.abs_diff(yy);
                        let spatial = match distance {
                            0 => 1.0,
                            1 => 0.7,
                            2 => 0.45,
                            3 => 0.25,
                            _ => 0.15,
                        };
                        let green = fused_green(bayer, width, height, xx, yy);
                        let edge = 1.0 / (1.0 + 80.0 * (green - center_green).abs());
                        let weight = spatial * edge * (0.1 + confidence[neighbor]);
                        weighted_depth += weight * source[neighbor];
                        weight_sum += weight;
                    }
                }
                let filtered = if weight_sum > 0.0 {
                    weighted_depth / weight_sum
                } else {
                    source[index]
                };
                let blend = (1.0 - confidence_here).clamp(0.0, 0.8);
                output_row[x] = source[index] * (1.0 - blend) + filtered * blend;
            }
        });
}

fn infer_foreground_mask(bayer: &[f32], width: usize, height: usize) -> Vec<u8> {
    if width < 8 || height < 8 {
        return vec![255; width * height];
    }
    let sample_step = ((width + height) / 2048).max(1);
    let mut border = Vec::with_capacity((width + height) * 2 / sample_step + 4);
    for x in (0..width).step_by(sample_step) {
        border.push(fused_green(bayer, width, height, x, 0));
        border.push(fused_green(bayer, width, height, x, height - 1));
    }
    for y in (0..height).step_by(sample_step) {
        border.push(fused_green(bayer, width, height, 0, y));
        border.push(fused_green(bayer, width, height, width - 1, y));
    }
    let background = median(&mut border);
    let mut deviations: Vec<f32> = border
        .iter()
        .map(|value| (*value - background).abs())
        .collect();
    let background_mad = median(&mut deviations);
    let threshold = (background_mad * 6.0).max(0.012);

    let mut center = Vec::new();
    for y in (height / 4..height * 3 / 4).step_by(8) {
        for x in (width / 4..width * 3 / 4).step_by(8) {
            center.push(fused_green(bayer, width, height, x, y));
        }
    }
    let center_level = median(&mut center);
    let bright_object = center_level >= background;
    let mut mask = vec![0u8; width * height];
    mask.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for x in 0..width {
            let green = fused_green(bayer, width, height, x, y);
            let foreground = if bright_object {
                green > background + threshold
            } else {
                green < background - threshold
            };
            row[x] = if foreground { 255 } else { 0 };
        }
    });

    let source = mask.clone();
    mask.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for x in 0..width {
            let mut votes = 0usize;
            let mut samples = 0usize;
            for yy in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                for xx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                    votes += usize::from(source[yy * width + xx] != 0);
                    samples += 1;
                }
            }
            row[x] = if votes * 2 >= samples { 255 } else { 0 };
        }
    });

    let foreground = mask.iter().filter(|&&value| value != 0).count();
    let coverage = foreground as f32 / mask.len() as f32;
    if !(0.005..=0.995).contains(&coverage) {
        tracing::warn!(
            "Foreground inference was uncertain ({:.2}% coverage); retaining the full shared ROI",
            coverage * 100.0
        );
        mask.fill(255);
    }
    mask
}

#[inline]
fn fused_green(bayer: &[f32], width: usize, height: usize, x: usize, y: usize) -> f32 {
    let index = y * width + x;
    if (x ^ y) & 1 == 1 {
        return bayer[index];
    }
    let mut sum = 0.0;
    let mut count = 0.0;
    if x > 0 {
        sum += bayer[index - 1];
        count += 1.0;
    }
    if x + 1 < width {
        sum += bayer[index + 1];
        count += 1.0;
    }
    if y > 0 {
        sum += bayer[index - width];
        count += 1.0;
    }
    if y + 1 < height {
        sum += bayer[index + width];
        count += 1.0;
    }
    if count > 0.0 {
        sum / count
    } else {
        bayer[index]
    }
}

#[inline]
fn normalize_raw(value: u16, calibration: FrameCalibration) -> f32 {
    ((value as f32 - calibration.black) * calibration.inverse_range).max(0.0)
}

#[inline]
fn exposure_log_distance(exposure: f32, target: f32) -> f32 {
    (exposure.max(1e-12).ln() - target.max(1e-12).ln()).abs()
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    if edge1 <= edge0 {
        return f32::from(value >= edge1);
    }
    let normalized = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    normalized * normalized * (3.0 - 2.0 * normalized)
}

fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let middle = values.len() / 2;
    values.select_nth_unstable_by(middle, f32::total_cmp);
    if values.len() % 2 == 1 {
        values[middle]
    } else {
        let lower = values[..middle]
            .iter()
            .copied()
            .max_by(f32::total_cmp)
            .unwrap_or(values[middle]);
        (lower + values[middle]) * 0.5
    }
}

fn bilinear_f64(image: &Array2<f64>, x: f64, y: f64) -> Option<f64> {
    let (height, width) = image.dim();
    if x < 0.0 || y < 0.0 || x > (width - 1) as f64 || y > (height - 1) as f64 {
        return None;
    }
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = x - x0 as f64;
    let ty = y - y0 as f64;
    let top = image[[y0, x0]] + (image[[y0, x1]] - image[[y0, x0]]) * tx;
    let bottom = image[[y1, x0]] + (image[[y1, x1]] - image[[y1, x0]]) * tx;
    Some(top + (bottom - top) * ty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nef::parser::{SensorLevels, Z9Metadata};
    use crate::smart_loader::NativeFrameGroup;
    use crate::types::Rect;

    fn metadata(exposure_time: f64) -> Z9Metadata {
        Z9Metadata {
            width: 16,
            height: 16,
            bits_per_sample: 14,
            compression: 1,
            cfa_pattern: RGGB,
            camera_make: "Nikon".to_string(),
            camera_model: "Z9".to_string(),
            sensor_levels: Some(SensorLevels {
                black: 0,
                white: 16_383,
            }),
            strip_offsets: vec![],
            strip_byte_counts: vec![],
            rows_per_strip: 16,
            cam_mul: [2.0, 1.0, 1.5, 1.0],
            timestamp: None,
            exposure_time: Some(exposure_time),
            aperture: Some(1.0),
            iso: Some(100),
            focal_length: Some(105.0),
            focus_distance: None,
        }
    }

    fn meta(focus_steps: u8, exposures: usize) -> Meta {
        Meta {
            focus_steps,
            exposures: vec![0.0; exposures],
            shutter_speeds: vec![1.0; exposures],
            ref_focus: focus_steps / 2,
            ref_exp: 0.0,
            rot_deg: 0.0,
            vantage: "mid".to_string(),
            burst_factor: 1,
            bone_id: "test".to_string(),
            cam_mul: [2.0, 1.0, 1.5, 1.0],
        }
    }

    #[test]
    fn hdr_radiance_is_exposure_invariant() {
        let width = 16;
        let height = 16;
        let white = 16_383.0;
        let exposures = [1.0, 2.0, 4.0];
        let scene = 0.2f32;
        let mut pixels = Vec::new();
        for exposure in exposures {
            let raw = (scene * exposure as f32 * white).round() as u16;
            pixels.extend(std::iter::repeat(raw).take(width * height));
        }
        let metadata = exposures.iter().map(|&value| metadata(value)).collect();
        let group = NativeFrameGroup::from_parts(
            &pixels,
            3,
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            metadata,
        )
        .unwrap();
        let config = NativeFusionConfig {
            black_level: Some(0.0),
            white_level: Some(white),
            tile_size: 16,
            regularize_depth: false,
            ..Default::default()
        };
        let result = fuse_native_group(&group, &meta(1, 3), &config).unwrap();
        // Green sites are not altered by white balance.
        assert!((result.bayer[[8, 9, 0]] - scene).abs() < 2e-3);
        // Red sites receive the lazy 2x camera multiplier.
        assert!((result.bayer[[8, 8, 0]] - scene * 2.0).abs() < 3e-3);
    }

    #[test]
    fn metadata_sensor_levels_are_the_default_calibration() {
        let pixels = vec![1008u16; 16 * 16];
        let mut frame_metadata = metadata(1.0);
        frame_metadata.sensor_levels = Some(SensorLevels {
            black: 1008,
            white: 15311,
        });
        let group = NativeFrameGroup::from_parts(
            &pixels,
            1,
            16,
            16,
            Rect::new(0.0, 0.0, 16.0, 16.0),
            vec![frame_metadata],
        )
        .unwrap();

        let calibrations = build_calibrations(&group, &NativeFusionConfig::default()).unwrap();
        assert_eq!(calibrations[0].black, 1008.0);
        assert!((calibrations[0].inverse_range - 1.0 / (15311.0 - 1008.0)).abs() < 1e-9);
        assert_eq!(normalize_raw(1008, calibrations[0]), 0.0);
        assert!((normalize_raw(15311, calibrations[0]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn tile_boundaries_do_not_change_fusion() {
        let width = 34;
        let height = 30;
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                pixels.push((1000 + x * 17 + y * 23) as u16);
            }
        }
        let mut frame_metadata = metadata(1.0);
        frame_metadata.width = width as u32;
        frame_metadata.height = height as u32;
        let group = NativeFrameGroup::from_parts(
            &pixels,
            1,
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            vec![frame_metadata],
        )
        .unwrap();
        let base = NativeFusionConfig {
            black_level: Some(0.0),
            white_level: Some(16_383.0),
            tile_size: 16,
            regularize_depth: false,
            ..Default::default()
        };
        let tiled = fuse_native_group(&group, &meta(1, 1), &base).unwrap();
        let untiled = fuse_native_group(
            &group,
            &meta(1, 1),
            &NativeFusionConfig {
                tile_size: 128,
                ..base
            },
        )
        .unwrap();
        let maximum_difference = tiled
            .bayer
            .iter()
            .zip(untiled.bayer.iter())
            .map(|(left, right)| (left - right).abs())
            .fold(0.0f32, f32::max);
        assert_eq!(maximum_difference, 0.0);
    }

    #[test]
    fn same_cfa_sampler_never_mixes_color_sites() {
        let width = 8;
        let height = 8;
        let mut frame = vec![0u16; width * height];
        for y in 0..height {
            for x in 0..width {
                frame[y * width + x] = match (y & 1, x & 1) {
                    (0, 0) => 100,
                    (1, 1) => 400,
                    _ => 200,
                };
            }
        }
        assert_eq!(
            sample_same_cfa(&frame, width, height, 3.2, 4.7, 0, 0),
            Some(100.0)
        );
        assert_eq!(
            sample_same_cfa(&frame, width, height, 3.2, 4.7, 1, 1),
            Some(400.0)
        );
    }
}
