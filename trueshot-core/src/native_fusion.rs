//! Memory-bounded HDR and focus fusion directly from native RAW group storage.
//!
//! Full-resolution inputs remain in the reusable `u16` arena. Only compact
//! green-channel analysis images use `f64` for FFT alignment; calibrated
//! radiance, focus measures, depth, and confidence remain `f32`.

use crate::align_raw::{align_phasecorr_gray, align_phasecorr_gray_with_scale};
#[cfg(all(test, target_arch = "aarch64", target_os = "macos"))]
use crate::focus_evidence::compute_focus_metric_apple_neon;
use crate::focus_evidence::{
    active_focus_kernel, compute_focus_metric, compute_trimmed_focus_mean,
};
#[cfg(test)]
use crate::focus_evidence::{compute_focus_metric_scalar, sorted_trimmed_focus_mean_at};
use crate::fusion_edit::{FusionEditBinding, FusionEditDocument, FusionEditSelector};
use crate::lens_psf::LensPsfProfile;
use crate::sensor_correction::SensorCorrectionProfile;
use crate::sensor_noise::{SensorNoiseModel, SensorNoiseProfile};
use crate::smart_loader::NativeFrameGroup;
use crate::types::Meta;
use anyhow::{Context, Result};
use ndarray::{Array2, Array3};
use rayon::prelude::*;

const RGGB: [u8; 4] = [0, 1, 1, 2];
const MAX_HDR_EXPOSURES: usize = 32;
const MAX_CENSORED_POSTERIOR_ITERATIONS: usize = 8;
const CENSOR_CONFLICT_SIGMA: f64 = 6.0;

pub const FUSION_FLAG_CENSORED: u8 = 1 << 0;
pub const FUSION_FLAG_OUTLIER_REJECTED: u8 = 1 << 1;
pub const FUSION_FLAG_SOURCE_FALLBACK: u8 = 1 << 2;
pub const FUSION_FLAG_UNCALIBRATED_NOISE: u8 = 1 << 3;
pub const FUSION_FLAG_CENSOR_CONFLICT: u8 = 1 << 4;
pub const FUSION_FLAG_VISIBILITY_CORRECTED: u8 = 1 << 5;
pub const FUSION_FLAG_BRACKET_ALIGNED: u8 = 1 << 6;
pub const FUSION_FLAG_DISOCCLUDED: u8 = 1 << 7;
pub const SENSOR_CORRECTION_FLAT_FIELD: u8 = 1 << 0;
pub const SENSOR_CORRECTION_DEFECT_REPAIRED: u8 = 1 << 1;

pub const FREQUENCY_FLAG_SEPARATED: u8 = 1 << 0;
pub const FREQUENCY_FLAG_SPLIT_SOURCES: u8 = 1 << 1;
pub const FREQUENCY_FLAG_DETAIL_REFERENCE: u8 = 1 << 2;
pub const FREQUENCY_FLAG_DETAIL_SINGLE_SOURCE: u8 = 1 << 3;
pub const FREQUENCY_FLAG_MEASUREMENT_CLAMPED: u8 = 1 << 4;

pub const BOUNDARY_TRIMAP_INTERIOR: u8 = 0;
pub const BOUNDARY_TRIMAP_PSF_SUPPORT: u8 = 128;
pub const BOUNDARY_TRIMAP_CROSSING_CORE: u8 = 255;
pub const FUSION_EDIT_MEASURED_SOURCE: u8 = 255;
pub const FUSION_EDIT_GLARE_CONSTRAINED: u8 = 192;
pub const FUSION_EDIT_BOUNDARY_CONSTRAINED: u8 = 128;
pub const FUSION_EDIT_BOUNDARY_CORE_CONSTRAINED: u8 = 64;

#[derive(Debug, Clone)]
pub struct NativeFusionConfig {
    /// Width and height of independently processed output bands/tiles.
    pub tile_size: usize,
    /// Context used by the focus operator. Values below two are promoted.
    pub halo: usize,
    /// Globally aligned block edge for low-resolution regional focus evidence.
    pub focus_coarse_stride: usize,
    /// Native Laplacian residual contribution at detail edges.
    pub focus_detail_edge_weight: f32,
    /// Use the deterministic ARM64 NEON focus stencil on Apple Silicon.
    pub apple_simd_focus: bool,
    /// Exclude saturated-core bloom from focus evidence without altering
    /// measured archival radiance.
    pub glare_aware_focus: bool,
    /// Conservative green-channel glare spread at the sensor. Verified pixel
    /// pitch converts this physical support into native pixels.
    pub glare_spread_um: f32,
    /// Explicit pixel support used when sensor pitch is unavailable and the
    /// hard cap for bounded scratch memory.
    pub glare_fallback_radius_pixels: usize,
    /// Maximum fraction of focus evidence removed at certain glare pixels.
    pub glare_focus_suppression: f32,
    /// Maximum edge of compact alignment images.
    pub analysis_max_dimension: usize,
    /// Pyramid levels used by the compact alignment implementation.
    pub alignment_levels: usize,
    /// Reject uncertain focus-plane transforms below this normalized score.
    pub minimum_alignment_score: f32,
    /// Refine HDR bracket motion only in compact tiles with unexplained
    /// exposure-normalized gradient residuals.
    pub selective_local_alignment: bool,
    /// Edge of one local-motion cell in compact green-analysis pixels.
    pub local_alignment_cell_size: usize,
    /// Maximum residual search around the global bracket shift, in compact
    /// green-analysis pixels.
    pub local_alignment_search_radius: usize,
    /// Tiles already exceeding this gradient agreement remain on the global
    /// model and avoid unnecessary local search.
    pub local_alignment_trigger_score: f32,
    /// Minimum bidirectionally consistent gradient score for local motion.
    pub minimum_local_alignment_score: f32,
    /// Maximum forward/backward disagreement in compact analysis pixels.
    pub disocclusion_consistency_threshold: f32,
    /// Optional sensor black-point override; metadata profile is the default.
    pub black_level: Option<f32>,
    /// Optional sensor saturation override; metadata profile is the default.
    pub white_level: Option<f32>,
    /// Approximate sensor read noise in native digital numbers.
    ///
    /// Used only by the explicitly flagged conservative fallback when no
    /// measured `sensor_noise_profile` is supplied.
    pub read_noise_dn: f32,
    /// Exact camera/bit-depth/per-ISO photon-transfer calibration.
    pub sensor_noise_profile: Option<SensorNoiseProfile>,
    /// Exact camera/geometry/optics spatial gain and defect calibration.
    pub sensor_correction_profile: Option<SensorCorrectionProfile>,
    /// Exact camera/lens/aperture/focus breathing, pupil, and field-PSF
    /// calibration. Missing calibration is explicitly reported as ideal-only.
    pub lens_psf_profile: Option<LensPsfProfile>,
    /// Apply confidence- and edge-aware depth regularization.
    pub regularize_depth: bool,
    /// Re-sample corrected focus hypotheses so regularization affects pixels,
    /// not only the exported depth map.
    pub depth_consistent_refusion: bool,
    /// Project the focus-selection surface onto the aperture-valid set when
    /// verified physical sensor geometry is available.
    pub aperture_visibility_correction: bool,
    /// Detect physically mixed foreground/background support and anchor it to
    /// one measured focus plane instead of interpolating incompatible rays.
    pub mixed_pixel_trimap: bool,
    /// Hard bound for physical mixed-pixel support and refusion work.
    pub trimap_max_radius_pixels: usize,
    /// Robust bracket-motion rejection strength. Zero disables rejection.
    pub deghost_strength: f32,
    /// Independently fuse same-CFA low-frequency radiance and high-frequency
    /// detail only where bracket disagreement exceeds calibrated noise.
    pub frequency_separated_deghosting: bool,
    /// Minimum median absolute deviation, in aggregate noise sigma, before the
    /// sparse frequency-separated path is allowed to alter a pixel.
    pub frequency_separation_trigger_sigma: f32,
    /// Optional source-bound operator revision. Every edited pixel is sampled
    /// from one aligned, uncensored measured frame after automatic fusion.
    pub fusion_edit: Option<FusionEditDocument>,
}

impl Default for NativeFusionConfig {
    fn default() -> Self {
        Self {
            tile_size: 256,
            halo: 3,
            focus_coarse_stride: 4,
            focus_detail_edge_weight: 1.0,
            apple_simd_focus: true,
            glare_aware_focus: true,
            glare_spread_um: 80.0,
            glare_fallback_radius_pixels: 20,
            glare_focus_suppression: 1.0,
            analysis_max_dimension: 512,
            // The legacy multiscale implementation does not warp residuals
            // between levels. One high-resolution compact FFT is exact.
            alignment_levels: 1,
            minimum_alignment_score: 0.08,
            selective_local_alignment: true,
            local_alignment_cell_size: 24,
            local_alignment_search_radius: 3,
            local_alignment_trigger_score: 0.90,
            minimum_local_alignment_score: 0.55,
            disocclusion_consistency_threshold: 0.75,
            black_level: None,
            white_level: None,
            read_noise_dn: 3.0,
            sensor_noise_profile: None,
            sensor_correction_profile: None,
            lens_psf_profile: None,
            regularize_depth: true,
            depth_consistent_refusion: true,
            aperture_visibility_correction: true,
            mixed_pixel_trimap: true,
            trimap_max_radius_pixels: 64,
            deghost_strength: 1.0,
            frequency_separated_deghosting: true,
            frequency_separation_trigger_sigma: 6.0,
            fusion_edit: None,
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

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct FrameAlignmentSummary {
    pub frame_index: u16,
    pub focus_plane: u16,
    pub reference_frame: bool,
    /// Bracket translation in full-resolution Bayer pixels.
    pub shift_x: f32,
    pub shift_y: f32,
    pub global_quality: f32,
    pub global_accepted: bool,
    pub local_aligned_cells: u32,
    pub disoccluded_cells: u32,
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

#[derive(Debug, Clone)]
struct FrameWarp {
    plane: PlaneTransform,
    bracket_shift_x: f32,
    bracket_shift_y: f32,
    global_accepted: bool,
    reference_frame: bool,
    local: Option<LocalMotionField>,
}

impl FrameWarp {
    fn identity(plane: PlaneTransform, reference_frame: bool) -> Self {
        Self {
            plane,
            bracket_shift_x: 0.0,
            bracket_shift_y: 0.0,
            global_accepted: true,
            reference_frame,
            local: None,
        }
    }

    fn source_coordinate_from_plane(&self, plane_x: f32, plane_y: f32) -> WarpedCoordinate {
        if !self.reference_frame && !self.global_accepted {
            return WarpedCoordinate {
                x: plane_x,
                y: plane_y,
                aligned: false,
                disoccluded: true,
            };
        }
        let local = self
            .local
            .as_ref()
            .map_or_else(LocalMotionSample::default, |field| {
                field.sample(plane_x, plane_y)
            });
        if local.disoccluded {
            return WarpedCoordinate {
                x: plane_x,
                y: plane_y,
                aligned: false,
                disoccluded: true,
            };
        }
        WarpedCoordinate {
            x: plane_x - self.bracket_shift_x - local.shift_x,
            y: plane_y - self.bracket_shift_y - local.shift_y,
            aligned: !self.reference_frame
                && (local.aligned
                    || self.global_accepted
                        && (self.bracket_shift_x.abs() > 1e-4
                            || self.bracket_shift_y.abs() > 1e-4)),
            disoccluded: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct LocalMotionCell {
    /// Residual alignment shift in full-resolution Bayer pixels.
    shift_x: f32,
    shift_y: f32,
    confidence: f32,
    active: bool,
    disoccluded: bool,
}

#[derive(Debug, Clone)]
struct LocalMotionField {
    cells: Vec<LocalMotionCell>,
    grid_width: usize,
    grid_height: usize,
    cell_size_analysis: usize,
    raw_pixels_per_analysis_pixel: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct LocalMotionSample {
    shift_x: f32,
    shift_y: f32,
    aligned: bool,
    disoccluded: bool,
}

#[derive(Debug, Clone, Copy)]
struct WarpedCoordinate {
    x: f32,
    y: f32,
    aligned: bool,
    disoccluded: bool,
}

impl LocalMotionField {
    fn sample(&self, raw_x: f32, raw_y: f32) -> LocalMotionSample {
        if self.cells.is_empty() || self.grid_width == 0 || self.grid_height == 0 {
            return LocalMotionSample::default();
        }
        let cell_size = self.cell_size_analysis as f32;
        let analysis_x = raw_x / self.raw_pixels_per_analysis_pixel;
        let analysis_y = raw_y / self.raw_pixels_per_analysis_pixel;
        let grid_x = analysis_x / cell_size - 0.5;
        let grid_y = analysis_y / cell_size - 0.5;
        let nearest_x = grid_x
            .round()
            .clamp(0.0, self.grid_width.saturating_sub(1) as f32) as usize;
        let nearest_y = grid_y
            .round()
            .clamp(0.0, self.grid_height.saturating_sub(1) as f32) as usize;
        let nearest = self.cells[nearest_y * self.grid_width + nearest_x];
        if nearest.disoccluded {
            return LocalMotionSample {
                disoccluded: true,
                ..LocalMotionSample::default()
            };
        }
        if !nearest.active {
            return LocalMotionSample::default();
        }

        let x0 = grid_x
            .floor()
            .clamp(0.0, self.grid_width.saturating_sub(1) as f32) as usize;
        let y0 = grid_y
            .floor()
            .clamp(0.0, self.grid_height.saturating_sub(1) as f32) as usize;
        let x1 = (x0 + 1).min(self.grid_width - 1);
        let y1 = (y0 + 1).min(self.grid_height - 1);
        let tx = (grid_x - x0 as f32).clamp(0.0, 1.0);
        let ty = (grid_y - y0 as f32).clamp(0.0, 1.0);
        let candidates = [
            (x0, y0, (1.0 - tx) * (1.0 - ty)),
            (x1, y0, tx * (1.0 - ty)),
            (x0, y1, (1.0 - tx) * ty),
            (x1, y1, tx * ty),
        ];
        let shift_tolerance =
            self.raw_pixels_per_analysis_pixel * self.cell_size_analysis as f32 * 0.5;
        let mut shift_x = 0.0f32;
        let mut shift_y = 0.0f32;
        let mut weight_sum = 0.0f32;
        for (x, y, geometric_weight) in candidates {
            let cell = self.cells[y * self.grid_width + x];
            if geometric_weight <= 0.0
                || cell.disoccluded
                || (cell.active
                    && ((cell.shift_x - nearest.shift_x).abs() > shift_tolerance
                        || (cell.shift_y - nearest.shift_y).abs() > shift_tolerance))
            {
                continue;
            }
            let confidence = if cell.active {
                cell.confidence.max(0.1)
            } else {
                1.0
            };
            let weight = geometric_weight * confidence;
            shift_x += weight * cell.shift_x;
            shift_y += weight * cell.shift_y;
            weight_sum += weight;
        }
        if weight_sum <= 1e-8 {
            return LocalMotionSample {
                shift_x: nearest.shift_x,
                shift_y: nearest.shift_y,
                aligned: true,
                disoccluded: false,
            };
        }
        LocalMotionSample {
            shift_x: shift_x / weight_sum,
            shift_y: shift_y / weight_sum,
            aligned: true,
            disoccluded: false,
        }
    }
}

#[derive(Debug)]
pub struct NativeFusionResult {
    /// Linear, white-balanced Bayer mosaic in normalized scene-radiance space.
    pub bayer: Array3<f32>,
    /// Normalized continuous focus coordinate; ordering follows capture order.
    pub depth: Array2<f32>,
    /// Physical subject distance in meters when complete lens metadata permits
    /// a nonuniform diopter-space focus model.
    pub metric_depth_m: Option<Array2<f32>>,
    /// Per-plane focus coordinates in inverse meters. Empty means the pipeline
    /// explicitly fell back to normalized capture indices.
    pub focus_diopters: Vec<f32>,
    /// Separation between the best and second-best focus hypotheses.
    pub confidence: Array2<f32>,
    /// Absolute one-standard-deviation uncertainty in anchored radiance units.
    pub radiance_uncertainty: Array2<f32>,
    /// Dominant source frame index, or `u16::MAX` for a lower-bound fallback.
    pub source_map: Array2<u16>,
    /// Dominant source of reconstructed native-frequency detail. This can
    /// differ from `source_map` when low and high frequencies use different
    /// measured bracket inliers.
    pub detail_source_map: Array2<u16>,
    /// Bitwise `FUSION_FLAG_*` evidence/provenance state.
    pub fusion_flags: Array2<u8>,
    /// Independent bitwise `FREQUENCY_FLAG_*` state. Kept separate because the
    /// original fusion provenance byte is fully allocated.
    pub frequency_flags: Array2<u8>,
    /// Per-pixel spatial correction provenance.
    pub sensor_correction_map: Array2<u8>,
    /// Maximum measured glare/bloom evidence across focus hypotheses.
    ///
    /// Zero is no detected influence and 255 is a censored glare core. This
    /// diagnostic never represents generated or reconstructed image content.
    pub glare_map: Array2<u8>,
    /// Physical mixed-pixel boundary state: 0 interior, 128 PSF support, and
    /// 255 at a measured depth crossing.
    pub boundary_trimap: Array2<u8>,
    /// Exact operator edit provenance. Present only for an edited revision;
    /// 255 means the final CFA value came from the document's measured frame.
    pub fusion_edit_map: Option<Array2<u8>>,
    /// Conservative object mask inferred from the shared crop border.
    pub foreground_mask: Array2<u8>,
    /// One transform per focus plane, shared by all bracketed exposures.
    pub transforms: Vec<PlaneTransform>,
    /// Per-bracket global/local alignment evidence retained for diagnostics.
    pub frame_alignments: Vec<FrameAlignmentSummary>,
    /// Shortest sensor exposure used as the radiance normalization anchor.
    pub radiance_anchor: f32,
    /// True only when every frame used an exact retained per-ISO profile.
    pub noise_model_calibrated: bool,
    /// Digest identity of the applied spatial correction artifact.
    pub sensor_correction_id: Option<String>,
    /// Digest identity of the applied lens breathing/pupil/field-PSF artifact.
    pub lens_psf_calibration_id: Option<String>,
    /// True only when physical depth, visibility, and boundary support used a
    /// retained lens-specific PSF calibration.
    pub lens_psf_calibrated: bool,
    /// Pixels whose selected source required persistent-defect replacement.
    pub defect_repaired_pixels: usize,
    /// Pixels re-sampled after physical/regularized focus correction, including
    /// single-plane mixed-boundary anchoring.
    pub depth_refusion_pixels: usize,
    /// Pixels whose focus coordinate was changed by the aperture visibility
    /// projection, including sub-plane changes.
    pub visibility_adjusted_pixels: usize,
    /// True when verified sensor geometry permitted the visibility projection.
    pub visibility_constrained: bool,
    /// Pixels inside physical boundary support.
    pub mixed_boundary_pixels: usize,
    /// Mixed pixels anchored to one traceable measured focus plane.
    pub boundary_source_fallback_pixels: usize,
    /// Maximum physical boundary support used in native pixels.
    pub trimap_radius_pixels: usize,
    /// True when the physical trimap did not require radius truncation.
    pub trimap_physical_scale: bool,
    /// Glare support used by focus inference in native pixels.
    pub glare_radius_pixels: usize,
    /// True when verified sensor pitch converted physical glare support.
    pub glare_physical_scale: bool,
    /// Pixels with nonzero glare evidence.
    pub glare_affected_pixels: usize,
    /// Pixels reconstructed from independently selected low/detail evidence.
    pub frequency_separated_pixels: usize,
    /// Frequency-separated pixels whose detail came from one measured frame.
    pub detail_single_source_pixels: usize,
    /// Frequency-separated pixels whose detail fell back to the reference.
    pub detail_reference_pixels: usize,
    /// Number of pixels replaced by a validated measured-source edit.
    pub operator_override_pixels: usize,
    /// Deterministic identity of the applied edit document.
    pub fusion_edit_digest: Option<String>,
    /// Base fusion report to which this immutable revision is bound.
    pub fusion_edit_base_report_sha256: Option<String>,
    /// Focus evidence kernel selected for this run.
    pub focus_kernel: &'static str,
}

impl NativeFusionResult {
    pub fn size_bytes(&self) -> usize {
        self.bayer.len() * std::mem::size_of::<f32>()
            + self.depth.len() * std::mem::size_of::<f32>()
            + self
                .metric_depth_m
                .as_ref()
                .map_or(0, |depth| depth.len() * std::mem::size_of::<f32>())
            + self.focus_diopters.len() * std::mem::size_of::<f32>()
            + self.confidence.len() * std::mem::size_of::<f32>()
            + self.radiance_uncertainty.len() * std::mem::size_of::<f32>()
            + self.source_map.len() * std::mem::size_of::<u16>()
            + self.detail_source_map.len() * std::mem::size_of::<u16>()
            + self.fusion_flags.len()
            + self.frequency_flags.len()
            + self.sensor_correction_map.len()
            + self.glare_map.len()
            + self.boundary_trimap.len()
            + self.fusion_edit_map.as_ref().map_or(0, |map| map.len())
            + self.foreground_mask.len()
            + self.transforms.len() * std::mem::size_of::<PlaneTransform>()
            + self.frame_alignments.len() * std::mem::size_of::<FrameAlignmentSummary>()
            + self.lens_psf_calibration_id.as_ref().map_or(0, String::len)
    }
}

/// Build a bounded, user-visible provenance overlay while preserving exact
/// source/flag maps separately for archival inspection.
pub fn fusion_provenance_preview(
    source_map: &Array2<u16>,
    fusion_flags: &Array2<u8>,
    max_dimension: usize,
) -> Result<(Array3<u8>, Array2<u8>)> {
    fusion_provenance_preview_impl(source_map, None, fusion_flags, None, max_dimension)
}

/// Build the production overlay with independent low/detail source awareness.
pub fn fusion_provenance_preview_with_frequency(
    source_map: &Array2<u16>,
    detail_source_map: &Array2<u16>,
    fusion_flags: &Array2<u8>,
    frequency_flags: &Array2<u8>,
    max_dimension: usize,
) -> Result<(Array3<u8>, Array2<u8>)> {
    fusion_provenance_preview_impl(
        source_map,
        Some(detail_source_map),
        fusion_flags,
        Some(frequency_flags),
        max_dimension,
    )
}

fn fusion_provenance_preview_impl(
    source_map: &Array2<u16>,
    detail_source_map: Option<&Array2<u16>>,
    fusion_flags: &Array2<u8>,
    frequency_flags: Option<&Array2<u8>>,
    max_dimension: usize,
) -> Result<(Array3<u8>, Array2<u8>)> {
    if source_map.dim() != fusion_flags.dim()
        || detail_source_map.is_some_and(|map| map.dim() != source_map.dim())
        || frequency_flags.is_some_and(|map| map.dim() != source_map.dim())
        || source_map.is_empty()
        || max_dimension == 0
    {
        anyhow::bail!("Fusion provenance preview dimensions are invalid");
    }
    let (height, width) = source_map.dim();
    let scale = (max_dimension as f64 / width.max(height) as f64).min(1.0);
    let output_width = ((width as f64 * scale).round() as usize).max(1);
    let output_height = ((height as f64 * scale).round() as usize).max(1);
    let mut rgb = vec![0u8; output_width * output_height * 3];
    let mut alpha = vec![0u8; output_width * output_height];

    for output_y in 0..output_height {
        let source_y0 = (output_y * height / output_height).min(height - 1);
        let source_y1 = ((output_y + 1) * height)
            .div_ceil(output_height)
            .min(height);
        for output_x in 0..output_width {
            let source_x0 = (output_x * width / output_width).min(width - 1);
            let source_x1 = ((output_x + 1) * width).div_ceil(output_width).min(width);
            let mut combined_flags = 0u8;
            let mut combined_frequency_flags = 0u8;
            for y in source_y0..source_y1 {
                for x in source_x0..source_x1 {
                    combined_flags |= fusion_flags[[y, x]];
                    if let Some(frequency_flags) = frequency_flags {
                        combined_frequency_flags |= frequency_flags[[y, x]];
                    }
                }
            }
            let center_x = ((source_x0 + source_x1) / 2).min(width - 1);
            let center_y = ((source_y0 + source_y1) / 2).min(height - 1);
            let source = source_map[[center_y, center_x]];
            let detail_source = detail_source_map.map_or(source, |map| map[[center_y, center_x]]);
            let (color, opacity) = provenance_color(
                source,
                detail_source,
                combined_flags,
                combined_frequency_flags,
            );
            let index = output_y * output_width + output_x;
            rgb[index * 3..index * 3 + 3].copy_from_slice(&color);
            alpha[index] = opacity;
        }
    }

    Ok((
        Array3::from_shape_vec((output_height, output_width, 3), rgb)
            .context("Shape fusion provenance RGB preview")?,
        Array2::from_shape_vec((output_height, output_width), alpha)
            .context("Shape fusion provenance alpha preview")?,
    ))
}

fn provenance_color(
    source: u16,
    detail_source: u16,
    flags: u8,
    frequency_flags: u8,
) -> ([u8; 3], u8) {
    if flags & FUSION_FLAG_DISOCCLUDED != 0 {
        return ([235, 55, 210], 230);
    }
    if flags & FUSION_FLAG_SOURCE_FALLBACK != 0 {
        return ([245, 55, 65], 230);
    }
    if flags & FUSION_FLAG_CENSOR_CONFLICT != 0 {
        return ([255, 50, 125], 225);
    }
    if frequency_flags & FREQUENCY_FLAG_MEASUREMENT_CLAMPED != 0 {
        return ([255, 75, 145], 205);
    }
    if frequency_flags & FREQUENCY_FLAG_DETAIL_REFERENCE != 0 {
        return ([255, 95, 45], 195);
    }
    if frequency_flags & FREQUENCY_FLAG_SPLIT_SOURCES != 0 || source != detail_source {
        return ([55, 220, 145], 180);
    }
    if frequency_flags & FREQUENCY_FLAG_SEPARATED != 0 {
        return ([60, 190, 245], 155);
    }
    if flags & FUSION_FLAG_OUTLIER_REJECTED != 0 {
        return ([255, 125, 35], 220);
    }
    if flags & FUSION_FLAG_CENSORED != 0 {
        return ([250, 205, 45], 210);
    }
    if flags & FUSION_FLAG_BRACKET_ALIGNED != 0 {
        return ([20, 205, 230], 150);
    }
    if flags & FUSION_FLAG_VISIBILITY_CORRECTED != 0 {
        return ([55, 120, 245], 150);
    }
    if flags & FUSION_FLAG_UNCALIBRATED_NOISE != 0 {
        return ([150, 155, 165], 130);
    }
    if source == u16::MAX {
        return ([0, 0, 0], 0);
    }
    // Stable categorical palette for source-frame awareness.
    let hash = u32::from(source).wrapping_mul(2_654_435_761);
    (
        [
            70 + ((hash >> 16) & 0x7f) as u8,
            70 + ((hash >> 8) & 0x7f) as u8,
            70 + (hash & 0x7f) as u8,
        ],
        72,
    )
}

#[derive(Debug, Clone, Copy)]
struct FrameCalibration {
    black: f32,
    inverse_range: f32,
    exposure: f32,
    wb_by_site: [f32; 4],
    noise_model: SensorNoiseModel,
}

#[derive(Debug, Clone, Copy, Default)]
struct HdrSample {
    radiance: f32,
    variance: f32,
    read_variance: f32,
    shot_variance_per_radiance: f32,
    lower_bound: f32,
    fallback_score: f32,
    frame_index: u16,
    censored: bool,
    reference_frame: bool,
    correction_flags: u8,
}

#[derive(Debug, Clone, Copy)]
struct HdrEstimate {
    radiance: f32,
    uncertainty: f32,
    source_index: u16,
    detail_source_index: u16,
    flags: u8,
    frequency_flags: u8,
    correction_flags: u8,
}

#[derive(Debug, Clone, Copy)]
struct CensoredPosterior {
    radiance: f32,
    uncertainty: f32,
    used_censored: bool,
    censor_conflict: bool,
    correction_flags: u8,
}

#[derive(Debug, Clone, Copy, Default)]
struct FrequencySample {
    low: f32,
    detail: f32,
    low_variance: f32,
    detail_variance: f32,
    frame_index: u16,
    reference_frame: bool,
    correction_flags: u8,
}

#[derive(Debug, Clone, Copy)]
struct FrequencyComponent {
    value: f32,
    uncertainty: f32,
    source_index: u16,
    contributors: usize,
    reference_fallback: bool,
    correction_flags: u8,
}

#[derive(Debug, Clone, Copy)]
struct FrequencyEstimate {
    radiance: f32,
    uncertainty: f32,
    low_source_index: u16,
    detail_source_index: u16,
    flags: u8,
    correction_flags: u8,
}

#[derive(Debug, Clone, Copy, Default)]
struct HdrFrameObservation {
    sample: Option<HdrSample>,
    aligned: bool,
    disoccluded: bool,
}

#[derive(Debug, Clone)]
struct PhysicalFocusModel {
    distances_m: Vec<f32>,
    diopters: Vec<f32>,
    sensor_distances_mm: Vec<f32>,
    focal_length_mm: f32,
    aperture: f32,
    pixel_pitch_mm: Option<f32>,
    sensor_origin_x: f32,
    sensor_origin_y: f32,
    lens_psf_profile: Option<LensPsfProfile>,
}

impl PhysicalFocusModel {
    fn distance_at_index(&self, index: f32) -> f32 {
        if self.diopters.len() == 1 {
            return self.distances_m[0];
        }
        let lower = index
            .floor()
            .clamp(0.0, self.diopters.len().saturating_sub(1) as f32) as usize;
        let upper = (lower + 1).min(self.diopters.len() - 1);
        let fraction = (index - lower as f32).clamp(0.0, 1.0);
        let diopter =
            self.diopters[lower] + (self.diopters[upper] - self.diopters[lower]) * fraction;
        1.0 / diopter.max(1e-8)
    }

    fn index_at_diopter(&self, best: usize, diopter: f32) -> f32 {
        let (lower, upper) = if diopter_between(
            diopter,
            self.diopters[best.saturating_sub(1)],
            self.diopters[best],
        ) {
            (best - 1, best)
        } else {
            (best, best + 1)
        };
        let denominator = self.diopters[upper] - self.diopters[lower];
        if denominator.abs() <= 1e-8 {
            return best as f32;
        }
        lower as f32 + ((diopter - self.diopters[lower]) / denominator).clamp(0.0, 1.0)
    }

    fn psf_sampling_balance(&self, best: usize, local_x: f32, local_y: f32) -> f32 {
        if best == 0 || best + 1 >= self.distances_m.len() {
            return 1.0;
        }
        let left = self.defocus_circle_mm(
            self.distances_m[best],
            self.distances_m[best - 1],
            local_x,
            local_y,
        );
        let right = self.defocus_circle_mm(
            self.distances_m[best],
            self.distances_m[best + 1],
            local_x,
            local_y,
        );
        left.min(right) / left.max(right).max(1e-8)
    }

    fn sensor_distance_at_index(&self, index: f32) -> f32 {
        if self.lens_psf_profile.is_some() {
            interpolate_indexed(&self.sensor_distances_mm, index)
        } else {
            object_to_sensor_distance_mm(self.focal_length_mm, self.distance_at_index(index))
        }
    }

    fn index_at_sensor_distance(&self, sensor_distance_mm: f32) -> f32 {
        if self.lens_psf_profile.is_some() {
            index_in_monotonic_coordinates(&self.sensor_distances_mm, sensor_distance_mm)
        } else {
            let object_distance_mm = self.focal_length_mm * sensor_distance_mm
                / (sensor_distance_mm - self.focal_length_mm);
            index_in_monotonic_coordinates(&self.diopters, 1000.0 / object_distance_mm)
        }
    }

    fn defocus_circle_mm(
        &self,
        focused_distance_m: f32,
        subject_distance_m: f32,
        local_x: f32,
        local_y: f32,
    ) -> f32 {
        if let Some(profile) = &self.lens_psf_profile {
            let radius = profile.field_radius(
                self.sensor_origin_x + local_x,
                self.sensor_origin_y + local_y,
            );
            profile.defocus_circle_mm(focused_distance_m, subject_distance_m, radius)
        } else {
            defocus_circle_mm(
                self.focal_length_mm,
                self.aperture,
                focused_distance_m,
                subject_distance_m,
            )
        }
    }

    fn conservative_aperture_radius_mm(&self) -> f32 {
        if let Some(profile) = &self.lens_psf_profile {
            self.distances_m
                .iter()
                .map(|distance| profile.conservative_aperture_radius_mm(*distance))
                .fold(0.0, f32::max)
        } else {
            self.focal_length_mm / (2.0 * self.aperture)
        }
    }
}

fn interpolate_indexed(values: &[f32], index: f32) -> f32 {
    let lower = index
        .floor()
        .clamp(0.0, values.len().saturating_sub(1) as f32) as usize;
    let upper = (lower + 1).min(values.len() - 1);
    let fraction = (index - lower as f32).clamp(0.0, 1.0);
    values[lower] + (values[upper] - values[lower]) * fraction
}

fn index_in_monotonic_coordinates(coordinates: &[f32], value: f32) -> f32 {
    if coordinates.len() <= 1 {
        return 0.0;
    }
    let last_index = coordinates.len() - 1;
    let ascending = coordinates[0] < coordinates[last_index];
    if (ascending && value <= coordinates[0]) || (!ascending && value >= coordinates[0]) {
        return 0.0;
    }
    if (ascending && value >= coordinates[last_index])
        || (!ascending && value <= coordinates[last_index])
    {
        return last_index as f32;
    }
    let mut low = 0usize;
    let mut high = last_index;
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if (ascending && coordinates[middle] <= value)
            || (!ascending && coordinates[middle] >= value)
        {
            low = middle;
        } else {
            high = middle;
        }
    }
    let denominator = coordinates[high] - coordinates[low];
    if denominator.abs() <= 1e-8 {
        low as f32
    } else {
        low as f32 + ((value - coordinates[low]) / denominator).clamp(0.0, 1.0)
    }
}

/// Fuse one ordered focus/HDR group without materializing calibrated or aligned
/// full-frame copies.
pub fn fuse_native_group(
    group: &NativeFrameGroup<'_>,
    meta: &Meta,
    config: &NativeFusionConfig,
) -> Result<NativeFusionResult> {
    fuse_native_group_with_id(group, meta, config, None)
}

/// Fuse a capture group with its stable content identity. A bound identity is
/// mandatory when a measured-source edit document is present.
pub fn fuse_native_group_with_id(
    group: &NativeFrameGroup<'_>,
    meta: &Meta,
    config: &NativeFusionConfig,
    capture_group_id: Option<&str>,
) -> Result<NativeFusionResult> {
    validate_group(group, meta, config)?;

    let focus_steps = usize::from(meta.focus_steps).max(1);
    let exposures_per_focus = group.len() / focus_steps;
    let calibrations = build_calibrations(group, config)?;
    let focus_model = physical_focus_model(
        group,
        focus_steps,
        exposures_per_focus,
        config.lens_psf_profile.as_ref(),
    );
    let (glare_radius_pixels, glare_physical_scale) = resolve_glare_radius_pixels(group, config);
    let radiance_anchor = calibrations
        .iter()
        .map(|calibration| calibration.exposure)
        .fold(f32::INFINITY, f32::min);
    if !radiance_anchor.is_finite() || radiance_anchor <= 0.0 {
        anyhow::bail!("Capture group has no valid exposure calibration");
    }

    let transforms =
        estimate_plane_transforms(group, meta, config, &calibrations, exposures_per_focus)?;
    let (frame_warps, frame_alignments) = estimate_frame_warps(
        group,
        config,
        &calibrations,
        &transforms,
        focus_steps,
        exposures_per_focus,
    )?;

    let width = group.width;
    let height = group.height;
    if let Some(edit) = &config.fusion_edit {
        let capture_group_id =
            capture_group_id.context("Fusion edits require a bound capture-group identity")?;
        let (crop_origin_x, crop_origin_y, _, _) = group.rect.to_bounds();
        edit.validate_binding(FusionEditBinding {
            capture_group_id,
            width,
            height,
            crop_origin_x,
            crop_origin_y,
            frame_count: group.len(),
        })?;
    }
    let pixel_count = width
        .checked_mul(height)
        .context("Native fusion dimensions overflow")?;
    let mut bayer = vec![0.0f32; pixel_count];
    let mut depth = vec![0.0f32; pixel_count];
    let mut confidence = vec![0.0f32; pixel_count];
    let mut radiance_uncertainty = vec![f32::INFINITY; pixel_count];
    let mut source_map = vec![u16::MAX; pixel_count];
    let mut detail_source_map = vec![u16::MAX; pixel_count];
    let mut fusion_flags = vec![0u8; pixel_count];
    let mut frequency_flags = vec![0u8; pixel_count];
    let mut glare_map = vec![0u8; pixel_count];
    let mut sensor_correction_map = vec![0u8; pixel_count];
    let band_rows = config.tile_size.max(16);
    let band_len = width
        .checked_mul(band_rows)
        .context("Native fusion band dimensions overflow")?;

    bayer
        .par_chunks_mut(band_len)
        .zip(depth.par_chunks_mut(band_len))
        .zip(confidence.par_chunks_mut(band_len))
        .zip(radiance_uncertainty.par_chunks_mut(band_len))
        .zip(source_map.par_chunks_mut(band_len))
        .zip(detail_source_map.par_chunks_mut(band_len))
        .zip(fusion_flags.par_chunks_mut(band_len))
        .zip(frequency_flags.par_chunks_mut(band_len))
        .zip(glare_map.par_chunks_mut(band_len))
        .zip(sensor_correction_map.par_chunks_mut(band_len))
        .enumerate()
        .try_for_each(
            |(
                band_index,
                (
                    (
                        (
                            (
                                (
                                    (
                                        (
                                            ((bayer_band, depth_band), confidence_band),
                                            uncertainty_band,
                                        ),
                                        source_band,
                                    ),
                                    detail_source_band,
                                ),
                                flags_band,
                            ),
                            frequency_band,
                        ),
                        glare_band,
                    ),
                    correction_band,
                ),
            )|
             -> Result<()> {
                let y0 = band_index * band_rows;
                let y1 = (y0 + bayer_band.len() / width).min(height);
                process_band(
                    group,
                    &calibrations,
                    &frame_warps,
                    focus_model.as_ref(),
                    focus_steps,
                    exposures_per_focus,
                    radiance_anchor,
                    config,
                    glare_radius_pixels,
                    y0,
                    y1,
                    bayer_band,
                    depth_band,
                    confidence_band,
                    uncertainty_band,
                    source_band,
                    detail_source_band,
                    flags_band,
                    frequency_band,
                    glare_band,
                    correction_band,
                )
            },
        )?;

    let visibility_constrained = focus_steps > 1
        && config.aperture_visibility_correction
        && focus_model
            .as_ref()
            .is_some_and(|model| model.pixel_pitch_mm.is_some());
    let physical_trimap_enabled = focus_steps > 1
        && config.mixed_pixel_trimap
        && focus_model
            .as_ref()
            .is_some_and(|model| model.pixel_pitch_mm.is_some());
    let needs_depth_correction = focus_steps > 1
        && (config.regularize_depth || visibility_constrained || physical_trimap_enabled);
    let mut depth_refusion_pixels = 0;
    let mut visibility_adjusted_pixels = 0;
    let mut visibility_mask = None;
    let mut boundary_trimap = vec![BOUNDARY_TRIMAP_INTERIOR; pixel_count];
    let mut mixed_boundary_pixels = 0;
    let mut boundary_source_fallback_pixels = 0;
    let mut trimap_radius_pixels = 0;
    let mut trimap_physical_scale = false;
    if needs_depth_correction {
        let source_depth =
            (config.depth_consistent_refusion || physical_trimap_enabled).then(|| depth.clone());
        if config.regularize_depth {
            regularize_depth_map(&bayer, &mut depth, &confidence, width, height);
        }
        if visibility_constrained {
            let (adjusted_pixels, correction_mask) = project_aperture_visibility(
                &mut depth,
                focus_model
                    .as_ref()
                    .expect("visibility model was checked above"),
                width,
                height,
            );
            visibility_adjusted_pixels = adjusted_pixels;
            visibility_mask = Some(correction_mask);
        }
        if physical_trimap_enabled {
            let summary = build_physical_boundary_trimap(
                source_depth.as_deref().unwrap_or(&depth),
                &depth,
                focus_model
                    .as_ref()
                    .expect("physical trimap model was checked above"),
                width,
                height,
                config.trimap_max_radius_pixels,
            );
            boundary_trimap = summary.map;
            mixed_boundary_pixels = summary.mixed_pixels;
            trimap_radius_pixels = summary.maximum_radius_pixels;
            trimap_physical_scale = summary.fully_physical;
        }
        if config.depth_consistent_refusion {
            let source_depth = source_depth
                .as_deref()
                .expect("depth refusion retains the uncorrected depth surface");
            let (refused, boundary_fallbacks) = refuse_regularized_depth(
                group,
                &calibrations,
                &frame_warps,
                focus_steps,
                exposures_per_focus,
                radiance_anchor,
                config,
                source_depth,
                &depth,
                &boundary_trimap,
                &mut bayer,
                &mut radiance_uncertainty,
                &mut source_map,
                &mut detail_source_map,
                &mut fusion_flags,
                &mut frequency_flags,
                &mut sensor_correction_map,
            )?;
            depth_refusion_pixels = refused;
            boundary_source_fallback_pixels = boundary_fallbacks;
        }
    }
    if let Some(visibility_mask) = &visibility_mask {
        fusion_flags
            .iter_mut()
            .zip(visibility_mask)
            .for_each(|(flags, corrected)| {
                if *corrected {
                    *flags |= FUSION_FLAG_VISIBILITY_CORRECTED;
                }
            });
    }
    let (fusion_edit_map, operator_override_pixels) = if let Some(edit) = &config.fusion_edit {
        let mut edit_map = vec![0u8; pixel_count];
        let edited = apply_measured_source_edits(
            edit,
            group,
            &calibrations,
            &frame_warps,
            exposures_per_focus,
            radiance_anchor,
            config,
            &mut bayer,
            &mut depth,
            &mut confidence,
            &mut radiance_uncertainty,
            &mut source_map,
            &mut detail_source_map,
            &mut fusion_flags,
            &mut frequency_flags,
            &mut sensor_correction_map,
            &glare_map,
            &boundary_trimap,
            glare_physical_scale,
            trimap_physical_scale,
            &mut edit_map,
        )?;
        (Some(edit_map), edited)
    } else {
        (None, 0)
    };
    let foreground_mask = infer_foreground_mask(&bayer, width, height);
    let glare_affected_pixels = glare_map.iter().filter(|value| **value != 0).count();
    let frequency_separated_pixels = frequency_flags
        .iter()
        .filter(|flags| **flags & FREQUENCY_FLAG_SEPARATED != 0)
        .count();
    let detail_single_source_pixels = frequency_flags
        .iter()
        .filter(|flags| **flags & FREQUENCY_FLAG_DETAIL_SINGLE_SOURCE != 0)
        .count();
    let detail_reference_pixels = frequency_flags
        .iter()
        .filter(|flags| **flags & FREQUENCY_FLAG_DETAIL_REFERENCE != 0)
        .count();
    let defect_repaired_pixels = sensor_correction_map
        .iter()
        .filter(|flags| **flags & SENSOR_CORRECTION_DEFECT_REPAIRED != 0)
        .count();
    let metric_depth_m = focus_model.as_ref().map(|model| {
        Array2::from_shape_vec(
            (height, width),
            depth
                .iter()
                .map(|value| {
                    model.distance_at_index(
                        value.clamp(0.0, 1.0) * focus_steps.saturating_sub(1) as f32,
                    )
                })
                .collect(),
        )
        .expect("metric depth shape matches normalized depth")
    });
    let focus_diopters = focus_model
        .as_ref()
        .map_or_else(Vec::new, |model| model.diopters.clone());

    Ok(NativeFusionResult {
        bayer: Array3::from_shape_vec((height, width, 1), bayer)
            .context("Unable to shape fused Bayer output")?,
        depth: Array2::from_shape_vec((height, width), depth)
            .context("Unable to shape fused depth output")?,
        metric_depth_m,
        focus_diopters,
        confidence: Array2::from_shape_vec((height, width), confidence)
            .context("Unable to shape fused confidence output")?,
        radiance_uncertainty: Array2::from_shape_vec((height, width), radiance_uncertainty)
            .context("Unable to shape radiance uncertainty output")?,
        source_map: Array2::from_shape_vec((height, width), source_map)
            .context("Unable to shape source map output")?,
        detail_source_map: Array2::from_shape_vec((height, width), detail_source_map)
            .context("Unable to shape detail source map output")?,
        fusion_flags: Array2::from_shape_vec((height, width), fusion_flags)
            .context("Unable to shape fusion flags output")?,
        frequency_flags: Array2::from_shape_vec((height, width), frequency_flags)
            .context("Unable to shape frequency flags output")?,
        sensor_correction_map: Array2::from_shape_vec((height, width), sensor_correction_map)
            .context("Unable to shape sensor correction map output")?,
        glare_map: Array2::from_shape_vec((height, width), glare_map)
            .context("Unable to shape glare diagnostic output")?,
        boundary_trimap: Array2::from_shape_vec((height, width), boundary_trimap)
            .context("Unable to shape physical boundary trimap output")?,
        fusion_edit_map: fusion_edit_map
            .map(|map| {
                Array2::from_shape_vec((height, width), map)
                    .context("Unable to shape fusion edit provenance")
            })
            .transpose()?,
        foreground_mask: Array2::from_shape_vec((height, width), foreground_mask)
            .context("Unable to shape fused foreground mask")?,
        transforms,
        frame_alignments,
        radiance_anchor,
        noise_model_calibrated: calibrations
            .iter()
            .all(|calibration| calibration.noise_model.calibrated),
        sensor_correction_id: config
            .sensor_correction_profile
            .as_ref()
            .map(|profile| profile.calibration_id.clone()),
        lens_psf_calibration_id: focus_model
            .as_ref()
            .and_then(|model| model.lens_psf_profile.as_ref())
            .map(|profile| profile.calibration_id.clone()),
        lens_psf_calibrated: focus_model
            .as_ref()
            .is_some_and(|model| model.lens_psf_profile.is_some()),
        defect_repaired_pixels,
        depth_refusion_pixels,
        visibility_adjusted_pixels,
        visibility_constrained,
        mixed_boundary_pixels,
        boundary_source_fallback_pixels,
        trimap_radius_pixels,
        trimap_physical_scale,
        glare_radius_pixels,
        glare_physical_scale,
        glare_affected_pixels,
        frequency_separated_pixels,
        detail_single_source_pixels,
        detail_reference_pixels,
        operator_override_pixels,
        fusion_edit_digest: config
            .fusion_edit
            .as_ref()
            .map(FusionEditDocument::digest)
            .transpose()?,
        fusion_edit_base_report_sha256: config
            .fusion_edit
            .as_ref()
            .map(|edit| edit.base_report_sha256.clone()),
        focus_kernel: active_focus_kernel(config.apple_simd_focus),
    })
}

#[allow(clippy::too_many_arguments)]
fn apply_measured_source_edits(
    edit: &FusionEditDocument,
    group: &NativeFrameGroup<'_>,
    calibrations: &[FrameCalibration],
    frame_warps: &[FrameWarp],
    exposures_per_focus: usize,
    radiance_anchor: f32,
    config: &NativeFusionConfig,
    bayer: &mut [f32],
    depth: &mut [f32],
    confidence: &mut [f32],
    uncertainty: &mut [f32],
    source_map: &mut [u16],
    detail_source_map: &mut [u16],
    fusion_flags: &mut [u8],
    frequency_flags: &mut [u8],
    sensor_correction_map: &mut [u8],
    glare_map: &[u8],
    boundary_trimap: &[u8],
    glare_physical_scale: bool,
    trimap_physical_scale: bool,
    edit_map: &mut [u8],
) -> Result<usize> {
    let width = group.width;
    let focus_steps = group.len() / exposures_per_focus;
    let depth_denominator = focus_steps.saturating_sub(1).max(1) as f32;
    let mut edited = 0usize;

    for operation in &edit.operations {
        match operation.selector {
            FusionEditSelector::GlareAffected if !glare_physical_scale => anyhow::bail!(
                "Fusion edit {} requires verified sensor-pitch glare support; pixel fallback is not physically qualified",
                operation.id
            ),
            FusionEditSelector::BoundaryAffected | FusionEditSelector::BoundaryCrossingCore
                if !trimap_physical_scale =>
            {
                anyhow::bail!(
                    "Fusion edit {} requires a verified aperture/PSF boundary trimap",
                    operation.id
                )
            }
            _ => {}
        }
        let frame_index = usize::from(operation.source_frame);
        let focus_plane = frame_index / exposures_per_focus;
        let frame_start = focus_plane * exposures_per_focus;
        let x1 = usize::try_from(operation.rect.x + operation.rect.width)?;
        let y1 = usize::try_from(operation.rect.y + operation.rect.height)?;
        let mut operation_pixels = 0usize;
        for y in usize::try_from(operation.rect.y)?..y1 {
            for x in usize::try_from(operation.rect.x)?..x1 {
                let index = y * width + x;
                let physical_focus_plane =
                    (depth[index].clamp(0.0, 1.0) * depth_denominator).round() as usize;
                if !fusion_edit_selector_matches(
                    operation.selector,
                    glare_map[index],
                    boundary_trimap[index],
                    focus_plane,
                    physical_focus_plane,
                ) {
                    continue;
                }
                let observation = sample_hdr_frame(
                    group,
                    calibrations,
                    frame_warps,
                    frame_start,
                    frame_index,
                    x,
                    y,
                    radiance_anchor,
                    config,
                );
                let sample = observation.sample.with_context(|| {
                    format!(
                        "Fusion edit {} source frame {} is unavailable or disoccluded at ({}, {})",
                        operation.id, frame_index, x, y
                    )
                })?;
                if sample.censored {
                    anyhow::bail!(
                        "Fusion edit {} source frame {} is clipped at ({}, {}); archival edits cannot invent highlight radiance",
                        operation.id,
                        frame_index,
                        x,
                        y
                    );
                }
                bayer[index] = sample.radiance;
                depth[index] = focus_plane as f32 / depth_denominator;
                // Algorithmic focus confidence no longer describes this
                // operator-selected measurement.
                confidence[index] = 0.0;
                uncertainty[index] = sample.variance.sqrt();
                source_map[index] = operation.source_frame;
                detail_source_map[index] = operation.source_frame;
                fusion_flags[index] = if observation.aligned {
                    FUSION_FLAG_BRACKET_ALIGNED
                } else {
                    0
                } | if calibrations[frame_index].noise_model.calibrated {
                    0
                } else {
                    FUSION_FLAG_UNCALIBRATED_NOISE
                };
                frequency_flags[index] = 0;
                sensor_correction_map[index] = sample.correction_flags;
                edit_map[index] = fusion_edit_selector_map_value(operation.selector);
                edited += 1;
                operation_pixels += 1;
            }
        }
        if operation_pixels == 0 {
            anyhow::bail!(
                "Fusion edit {} selector {:?} matched no recomputed physical evidence inside its rectangle",
                operation.id,
                operation.selector
            );
        }
    }
    Ok(edited)
}

fn fusion_edit_selector_matches(
    selector: FusionEditSelector,
    glare: u8,
    boundary: u8,
    selected_focus_plane: usize,
    physical_focus_plane: usize,
) -> bool {
    match selector {
        FusionEditSelector::Rectangle => true,
        FusionEditSelector::GlareAffected => glare != 0,
        FusionEditSelector::BoundaryAffected => {
            boundary != BOUNDARY_TRIMAP_INTERIOR && selected_focus_plane == physical_focus_plane
        }
        FusionEditSelector::BoundaryCrossingCore => {
            boundary == BOUNDARY_TRIMAP_CROSSING_CORE
                && selected_focus_plane == physical_focus_plane
        }
    }
}

fn fusion_edit_selector_map_value(selector: FusionEditSelector) -> u8 {
    match selector {
        FusionEditSelector::Rectangle => FUSION_EDIT_MEASURED_SOURCE,
        FusionEditSelector::GlareAffected => FUSION_EDIT_GLARE_CONSTRAINED,
        FusionEditSelector::BoundaryAffected => FUSION_EDIT_BOUNDARY_CONSTRAINED,
        FusionEditSelector::BoundaryCrossingCore => FUSION_EDIT_BOUNDARY_CORE_CONSTRAINED,
    }
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
    if config.black_level.is_some_and(|level| level < 0.0)
        || (config.sensor_noise_profile.is_none() && config.read_noise_dn <= 0.0)
    {
        anyhow::bail!("Native fusion calibration values must be positive");
    }
    if let Some(profile) = &config.sensor_noise_profile {
        profile.validate()?;
    }
    if let Some(profile) = &config.sensor_correction_profile {
        profile.validate()?;
    }
    if let Some(profile) = &config.lens_psf_profile {
        profile.validate()?;
    }
    if !group.rect.x.is_finite()
        || !group.rect.y.is_finite()
        || !group.rect.width.is_finite()
        || !group.rect.height.is_finite()
        || group.rect.x < 0.0
        || group.rect.y < 0.0
    {
        anyhow::bail!("Native group crop geometry is invalid");
    }
    let (crop_x0, crop_y0, crop_x1, crop_y1) = group.rect.to_bounds();
    if crop_x1.saturating_sub(crop_x0) != group.width
        || crop_y1.saturating_sub(crop_y0) != group.height
        || crop_x0 & 1 != 0
        || crop_y0 & 1 != 0
    {
        anyhow::bail!("Native group crop must exactly match an even-origin Bayer ROI");
    }
    let exposures_per_focus = group.len() / focus_steps;
    if exposures_per_focus > MAX_HDR_EXPOSURES {
        anyhow::bail!(
            "Native fusion supports at most {} HDR exposures per focus plane, got {}",
            MAX_HDR_EXPOSURES,
            exposures_per_focus
        );
    }
    if !config.deghost_strength.is_finite() || !(0.0..=2.0).contains(&config.deghost_strength) {
        anyhow::bail!("Native fusion deghost strength must be between 0 and 2");
    }
    if !config.frequency_separation_trigger_sigma.is_finite()
        || !(1.0..=64.0).contains(&config.frequency_separation_trigger_sigma)
    {
        anyhow::bail!("Native fusion frequency-separation trigger must be between 1 and 64 sigma");
    }
    if !(1..=16).contains(&config.focus_coarse_stride)
        || !config.focus_detail_edge_weight.is_finite()
        || !(0.0..=2.0).contains(&config.focus_detail_edge_weight)
    {
        anyhow::bail!("Native fusion scale-decoupled focus configuration is invalid");
    }
    if !config.glare_spread_um.is_finite()
        || !(1.0..=2_000.0).contains(&config.glare_spread_um)
        || !(2..=256).contains(&config.glare_fallback_radius_pixels)
        || !config.glare_focus_suppression.is_finite()
        || !(0.0..=1.0).contains(&config.glare_focus_suppression)
    {
        anyhow::bail!("Native fusion glare configuration is invalid");
    }
    if !(1..=256).contains(&config.trimap_max_radius_pixels) {
        anyhow::bail!("Native fusion physical boundary trimap configuration is invalid");
    }
    if !(8..=128).contains(&config.local_alignment_cell_size)
        || config.local_alignment_search_radius > 12
        || !config.local_alignment_trigger_score.is_finite()
        || !(0.5..=1.0).contains(&config.local_alignment_trigger_score)
        || !config.minimum_local_alignment_score.is_finite()
        || !(0.0..=1.0).contains(&config.minimum_local_alignment_score)
        || config.minimum_local_alignment_score > config.local_alignment_trigger_score
        || !config.disocclusion_consistency_threshold.is_finite()
        || !(0.1..=4.0).contains(&config.disocclusion_consistency_threshold)
    {
        anyhow::bail!("Native fusion selective local-alignment configuration is invalid");
    }
    let reference = &group.metadata[0];
    for (index, metadata) in group.metadata.iter().enumerate() {
        if metadata.cfa_pattern != RGGB {
            anyhow::bail!(
                "Native AHD requires RGGB CFA; frame {} is {:?}",
                index,
                metadata.cfa_pattern
            );
        }
        if metadata.bits_per_sample != reference.bits_per_sample
            || metadata.sensor_levels != reference.sensor_levels
        {
            anyhow::bail!("Native group mixes incompatible sensor encodings");
        }
        if metadata
            .cam_mul
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
        {
            anyhow::bail!("Frame {} has invalid white-balance calibration", index);
        }
        if crop_x1 > metadata.width as usize || crop_y1 > metadata.height as usize {
            anyhow::bail!(
                "Native group crop exceeds frame {} sensor dimensions",
                index
            );
        }
        if exposures_per_focus > 1
            && (metadata.exposure_time.is_none()
                || metadata.aperture.is_none()
                || metadata.iso.is_none())
        {
            anyhow::bail!(
                "HDR frame {} is missing shutter, aperture, or ISO metadata",
                index
            );
        }
        if let Some(profile) = &config.sensor_noise_profile {
            if !profile.matches(
                &metadata.camera_make,
                &metadata.camera_model,
                metadata.bits_per_sample,
            ) {
                anyhow::bail!(
                    "Sensor noise profile {} does not match frame {} camera identity",
                    profile.calibration_id,
                    index
                );
            }
            let iso = metadata
                .iso
                .context("Calibrated noise model requires ISO metadata")?;
            if profile.model_for_iso(iso).is_none() {
                anyhow::bail!(
                    "Sensor noise profile {} has no exact ISO {} model for frame {}",
                    profile.calibration_id,
                    iso,
                    index
                );
            }
        }
        if let Some(profile) = &config.sensor_correction_profile {
            let aperture = metadata
                .aperture
                .context("Spatial sensor correction requires aperture metadata")?;
            let focal_length = metadata
                .focal_length
                .context("Spatial sensor correction requires focal-length metadata")?;
            let lens_model = metadata
                .lens_model
                .as_deref()
                .context("Spatial sensor correction requires lens-model metadata")?;
            let focus_distance = metadata
                .focus_distance
                .context("Spatial sensor correction requires focus-distance metadata")?;
            if !profile.matches(
                &metadata.camera_make,
                &metadata.camera_model,
                metadata.bits_per_sample,
                metadata.width,
                metadata.height,
                lens_model,
                aperture,
                focal_length,
                focus_distance,
            ) {
                anyhow::bail!(
                    "Sensor correction profile {} does not match frame {} camera, geometry, or optics",
                    profile.calibration_id,
                    index
                );
            }
        }
        if let Some(profile) = &config.lens_psf_profile {
            let aperture = metadata
                .aperture
                .context("Lens PSF calibration requires aperture metadata")?;
            let focal_length = metadata
                .focal_length
                .context("Lens PSF calibration requires focal-length metadata")?;
            let lens_model = metadata
                .lens_model
                .as_deref()
                .context("Lens PSF calibration requires lens-model metadata")?;
            let focus_distance = metadata
                .focus_distance
                .context("Lens PSF calibration requires focus-distance metadata")?;
            if !profile.matches(
                &metadata.camera_make,
                &metadata.camera_model,
                metadata.width,
                metadata.height,
                lens_model,
                focal_length,
                aperture,
                focus_distance,
            ) {
                anyhow::bail!(
                    "Lens PSF profile {} does not match frame {} camera, sensor, lens, aperture, focal length, or focus envelope",
                    profile.calibration_id,
                    index
                );
            }
        }
    }
    Ok(())
}

fn physical_focus_model(
    group: &NativeFrameGroup<'_>,
    focus_steps: usize,
    exposures_per_focus: usize,
    lens_psf_profile: Option<&LensPsfProfile>,
) -> Option<PhysicalFocusModel> {
    if focus_steps < 2 {
        return None;
    }
    let reference = group.metadata.first()?;
    let focal_length_mm = reference.focal_length?;
    let aperture = reference.aperture?;
    if !focal_length_mm.is_finite()
        || focal_length_mm <= 0.0
        || !aperture.is_finite()
        || aperture <= 0.0
    {
        tracing::warn!("Physical focus model disabled: invalid focal length or aperture");
        return None;
    }

    let minimum_distance_m = focal_length_mm * 0.001 * 1.01;
    let mut distances_m = Vec::with_capacity(focus_steps);
    for focus in 0..focus_steps {
        let start = focus * exposures_per_focus;
        let distance = group.metadata.get(start)?.focus_distance?;
        if !distance.is_finite() || distance <= minimum_distance_m {
            tracing::warn!(
                "Physical focus model disabled: plane {} has invalid focus distance {:?}",
                focus,
                distance
            );
            return None;
        }
        for metadata in &group.metadata[start..start + exposures_per_focus] {
            let candidate = metadata.focus_distance?;
            let tolerance = (distance.abs() * 0.005).max(0.001);
            if !candidate.is_finite()
                || (candidate - distance).abs() > tolerance
                || metadata
                    .focal_length
                    .map_or(true, |value| (value - focal_length_mm).abs() > 0.05)
                || metadata
                    .aperture
                    .map_or(true, |value| (value - aperture).abs() > 0.01)
            {
                tracing::warn!(
                    "Physical focus model disabled: inconsistent lens metadata in plane {}",
                    focus
                );
                return None;
            }
        }
        distances_m.push(distance);
    }

    let diopters: Vec<f32> = distances_m.iter().map(|distance| 1.0 / distance).collect();
    let increasing = diopters.windows(2).all(|pair| pair[1] - pair[0] > 1e-6);
    let decreasing = diopters.windows(2).all(|pair| pair[0] - pair[1] > 1e-6);
    if !increasing && !decreasing {
        tracing::warn!(
            "Physical focus model disabled: focus distances are duplicated or non-monotonic"
        );
        return None;
    }
    let pixel_pitch_mm = reference
        .sensor_geometry
        .map(|geometry| geometry.pixel_pitch_um * 0.001)
        .filter(|pitch| pitch.is_finite() && *pitch > 0.0)
        .and_then(|pitch| {
            let consistent = group.metadata.iter().all(|metadata| {
                metadata.sensor_geometry.is_some_and(|geometry| {
                    (geometry.pixel_pitch_um * 0.001 - pitch).abs() <= pitch * 0.001
                })
            });
            if consistent {
                Some(pitch)
            } else {
                tracing::warn!(
                    "Aperture visibility correction disabled: inconsistent sensor geometry"
                );
                None
            }
        });
    let sensor_distances_mm = distances_m
        .iter()
        .map(|distance| {
            lens_psf_profile.map_or_else(
                || object_to_sensor_distance_mm(focal_length_mm, *distance),
                |profile| profile.sensor_distance_mm(*distance),
            )
        })
        .collect::<Vec<_>>();
    let sensor_distances_monotonic = sensor_distances_mm
        .windows(2)
        .all(|pair| pair[1] - pair[0] > 1e-7)
        || sensor_distances_mm
            .windows(2)
            .all(|pair| pair[0] - pair[1] > 1e-7);
    if !sensor_distances_monotonic {
        tracing::warn!(
            "Physical focus model disabled: calibrated sensor distances are non-monotonic"
        );
        return None;
    }
    let (sensor_origin_x, sensor_origin_y, _, _) = group.rect.to_bounds();
    Some(PhysicalFocusModel {
        distances_m,
        diopters,
        sensor_distances_mm,
        focal_length_mm,
        aperture,
        pixel_pitch_mm,
        sensor_origin_x: sensor_origin_x as f32,
        sensor_origin_y: sensor_origin_y as f32,
        lens_psf_profile: lens_psf_profile.cloned(),
    })
}

fn resolve_glare_radius_pixels(
    group: &NativeFrameGroup<'_>,
    config: &NativeFusionConfig,
) -> (usize, bool) {
    if !config.glare_aware_focus || config.glare_focus_suppression == 0.0 {
        return (0, false);
    }
    let Some(reference_pitch_um) = group
        .metadata
        .first()
        .and_then(|metadata| metadata.sensor_geometry)
        .map(|geometry| geometry.pixel_pitch_um)
        .filter(|pitch| pitch.is_finite() && *pitch > 0.0)
    else {
        return (config.glare_fallback_radius_pixels, false);
    };
    let geometry_is_consistent = group.metadata.iter().all(|metadata| {
        metadata.sensor_geometry.is_some_and(|geometry| {
            geometry.pixel_pitch_um.is_finite()
                && (geometry.pixel_pitch_um - reference_pitch_um).abs()
                    <= reference_pitch_um * 0.001
        })
    });
    if !geometry_is_consistent {
        tracing::warn!(
            "Glare focus exclusion uses explicit pixel fallback: inconsistent sensor geometry"
        );
        return (config.glare_fallback_radius_pixels, false);
    }
    let physical_radius = (config.glare_spread_um / reference_pitch_um)
        .ceil()
        .max(2.0);
    if physical_radius > config.glare_fallback_radius_pixels as f32 {
        tracing::warn!(
            physical_radius,
            radius_cap = config.glare_fallback_radius_pixels,
            "Physical glare support exceeded bounded pixel cap"
        );
        return (config.glare_fallback_radius_pixels, false);
    }
    (physical_radius as usize, true)
}

fn diopter_between(value: f32, left: f32, right: f32) -> bool {
    value >= left.min(right) && value <= left.max(right)
}

/// Thin-lens defocus-circle diameter at the sensor plane.
fn defocus_circle_mm(
    focal_length_mm: f32,
    aperture: f32,
    focused_distance_m: f32,
    subject_distance_m: f32,
) -> f32 {
    let focused_mm = focused_distance_m * 1000.0;
    let subject_mm = subject_distance_m * 1000.0;
    let focused_image_mm = focal_length_mm * focused_mm / (focused_mm - focal_length_mm);
    let subject_image_mm = focal_length_mm * subject_mm / (subject_mm - focal_length_mm);
    let entrance_pupil_mm = focal_length_mm / aperture;
    entrance_pupil_mm * (focused_image_mm - subject_image_mm).abs() / subject_image_mm
}

fn object_to_sensor_distance_mm(focal_length_mm: f32, object_distance_m: f32) -> f32 {
    let object_distance_mm = object_distance_m * 1000.0;
    focal_length_mm * object_distance_mm / (object_distance_mm - focal_length_mm)
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
            let noise_model = if let Some(profile) = &config.sensor_noise_profile {
                profile.model_for_iso(iso as u32).with_context(|| {
                    format!(
                        "Noise profile {} has no ISO {} model",
                        profile.calibration_id, iso
                    )
                })?
            } else {
                SensorNoiseModel::conservative(config.read_noise_dn)
            };
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
                noise_model,
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
    let selected_frames = select_alignment_frames(calibrations, focus_steps, exposures_per_focus);

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

fn select_alignment_frames(
    calibrations: &[FrameCalibration],
    focus_steps: usize,
    exposures_per_focus: usize,
) -> Vec<usize> {
    let target_exposure = median(
        &mut calibrations
            .iter()
            .map(|calibration| calibration.exposure)
            .collect::<Vec<_>>(),
    );
    (0..focus_steps)
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
        .collect()
}

fn estimate_frame_warps(
    group: &NativeFrameGroup<'_>,
    config: &NativeFusionConfig,
    calibrations: &[FrameCalibration],
    transforms: &[PlaneTransform],
    focus_steps: usize,
    exposures_per_focus: usize,
) -> Result<(Vec<FrameWarp>, Vec<FrameAlignmentSummary>)> {
    let selected_frames = select_alignment_frames(calibrations, focus_steps, exposures_per_focus);
    let analysis_stride = analysis_stride(group.width, group.height, config);
    let raw_pixels_per_analysis_pixel = (analysis_stride * 2) as f32;
    let compact_alignment_available = group.width >= 32 && group.height >= 32;
    let mut warps = Vec::with_capacity(group.len());
    let mut summaries = Vec::with_capacity(group.len());

    for focus in 0..focus_steps {
        let reference_frame = selected_frames[focus];
        let reference_analysis = if exposures_per_focus > 1 && compact_alignment_available {
            Some(build_green_analysis(
                group,
                reference_frame,
                calibrations[reference_frame],
                analysis_stride,
            )?)
        } else {
            None
        };
        let reference_gradient = reference_analysis
            .as_ref()
            .map(GradientAnalysis::from_image);
        let frame_start = focus * exposures_per_focus;
        for frame_index in frame_start..frame_start + exposures_per_focus {
            if frame_index == reference_frame || reference_analysis.is_none() {
                warps.push(FrameWarp::identity(
                    transforms[focus],
                    frame_index == reference_frame,
                ));
                summaries.push(FrameAlignmentSummary {
                    frame_index: u16::try_from(frame_index).unwrap_or(u16::MAX),
                    focus_plane: u16::try_from(focus).unwrap_or(u16::MAX),
                    reference_frame: frame_index == reference_frame,
                    shift_x: 0.0,
                    shift_y: 0.0,
                    global_quality: 1.0,
                    global_accepted: true,
                    local_aligned_cells: 0,
                    disoccluded_cells: 0,
                });
                continue;
            }

            let reference = reference_analysis
                .as_ref()
                .expect("reference analysis was checked above");
            let analysis = build_green_analysis(
                group,
                frame_index,
                calibrations[frame_index],
                analysis_stride,
            )?;
            let identity_quality = transformed_ncc(reference, &analysis, 0.0, 0.0, 1.0) as f32;
            let (dx, dy, quality) = if identity_quality >= config.local_alignment_trigger_score {
                (0.0, 0.0, identity_quality)
            } else {
                let (dx, dy) =
                    align_phasecorr_gray(reference, &analysis, config.alignment_levels.max(1));
                (
                    dx,
                    dy,
                    transformed_ncc(reference, &analysis, dx, dy, 1.0) as f32,
                )
            };
            let shift_x = dx as f32 * raw_pixels_per_analysis_pixel;
            let shift_y = dy as f32 * raw_pixels_per_analysis_pixel;
            let plausible_shift = shift_x.abs() <= group.width as f32 * 0.12
                && shift_y.abs() <= group.height as f32 * 0.12;
            let accepted = quality >= config.minimum_alignment_score && plausible_shift;
            let (local, local_aligned_cells, disoccluded_cells) =
                if config.selective_local_alignment && accepted {
                    let frame_gradient = GradientAnalysis::from_image(&analysis);
                    let field = estimate_local_motion_field_from_gradients(
                        reference_gradient
                            .as_ref()
                            .expect("reference gradient accompanies reference analysis"),
                        &frame_gradient,
                        dx as f32,
                        dy as f32,
                        raw_pixels_per_analysis_pixel,
                        config,
                    );
                    let aligned = field.cells.iter().filter(|cell| cell.active).count();
                    let disoccluded = field.cells.iter().filter(|cell| cell.disoccluded).count();
                    (
                        (aligned > 0 || disoccluded > 0).then_some(field),
                        aligned,
                        disoccluded,
                    )
                } else {
                    (None, 0, 0)
                };
            if !accepted {
                tracing::warn!(
                    "Rejected bracket frame {} transform dx={:.2} dy={:.2} NCC={:.3}",
                    frame_index,
                    shift_x,
                    shift_y,
                    quality
                );
            }
            warps.push(FrameWarp {
                plane: transforms[focus],
                bracket_shift_x: if accepted { shift_x } else { 0.0 },
                bracket_shift_y: if accepted { shift_y } else { 0.0 },
                global_accepted: accepted,
                reference_frame: false,
                local,
            });
            summaries.push(FrameAlignmentSummary {
                frame_index: u16::try_from(frame_index).unwrap_or(u16::MAX),
                focus_plane: u16::try_from(focus).unwrap_or(u16::MAX),
                reference_frame: false,
                shift_x: if accepted { shift_x } else { 0.0 },
                shift_y: if accepted { shift_y } else { 0.0 },
                global_quality: quality,
                global_accepted: accepted,
                local_aligned_cells: u32::try_from(local_aligned_cells).unwrap_or(u32::MAX),
                disoccluded_cells: u32::try_from(disoccluded_cells).unwrap_or(u32::MAX),
            });
        }
    }
    Ok((warps, summaries))
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

#[derive(Debug)]
struct GradientAnalysis {
    width: usize,
    height: usize,
    gx: Vec<f32>,
    gy: Vec<f32>,
}

impl GradientAnalysis {
    fn from_image(image: &Array2<f64>) -> Self {
        let (height, width) = image.dim();
        let mut gx = vec![0.0f32; width * height];
        let mut gy = vec![0.0f32; width * height];
        for y in 0..height {
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(height - 1);
            for x in 0..width {
                let x0 = x.saturating_sub(1);
                let x1 = (x + 1).min(width - 1);
                let index = y * width + x;
                gx[index] = ((image[[y, x1]] - image[[y, x0]]) * 0.5) as f32;
                gy[index] = ((image[[y1, x]] - image[[y0, x]]) * 0.5) as f32;
            }
        }
        Self {
            width,
            height,
            gx,
            gy,
        }
    }

    fn sample(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        if x < 0.0
            || y < 0.0
            || x > self.width.saturating_sub(1) as f32
            || y > self.height.saturating_sub(1) as f32
        {
            return None;
        }
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = x - x0 as f32;
        let ty = y - y0 as f32;
        let bilinear = |values: &[f32]| {
            let top = values[y0 * self.width + x0]
                + (values[y0 * self.width + x1] - values[y0 * self.width + x0]) * tx;
            let bottom = values[y1 * self.width + x0]
                + (values[y1 * self.width + x1] - values[y1 * self.width + x0]) * tx;
            top + (bottom - top) * ty
        };
        Some((bilinear(&self.gx), bilinear(&self.gy)))
    }
}

#[derive(Debug, Clone, Copy)]
struct PatchAlignment {
    shift_x: f32,
    shift_y: f32,
    score: f32,
    texture: f32,
}

#[cfg(test)]
fn estimate_local_motion_field(
    reference: &Array2<f64>,
    frame: &Array2<f64>,
    global_shift_x: f32,
    global_shift_y: f32,
    raw_pixels_per_analysis_pixel: f32,
    config: &NativeFusionConfig,
) -> LocalMotionField {
    let reference_gradient = GradientAnalysis::from_image(reference);
    let frame_gradient = GradientAnalysis::from_image(frame);
    estimate_local_motion_field_from_gradients(
        &reference_gradient,
        &frame_gradient,
        global_shift_x,
        global_shift_y,
        raw_pixels_per_analysis_pixel,
        config,
    )
}

fn estimate_local_motion_field_from_gradients(
    reference_gradient: &GradientAnalysis,
    frame_gradient: &GradientAnalysis,
    global_shift_x: f32,
    global_shift_y: f32,
    raw_pixels_per_analysis_pixel: f32,
    config: &NativeFusionConfig,
) -> LocalMotionField {
    let width = reference_gradient.width;
    let height = reference_gradient.height;
    let cell_size = config.local_alignment_cell_size;
    let grid_width = width.div_ceil(cell_size);
    let grid_height = height.div_ceil(cell_size);
    let patch_radius = (cell_size / 4).clamp(3, 12);
    let mut cells = Vec::with_capacity(grid_width * grid_height);

    for grid_y in 0..grid_height {
        let center_y = ((grid_y * cell_size + cell_size / 2).min(height - 1)) as f32;
        for grid_x in 0..grid_width {
            let center_x = ((grid_x * cell_size + cell_size / 2).min(width - 1)) as f32;
            let baseline = gradient_patch_score(
                reference_gradient,
                frame_gradient,
                center_x,
                center_y,
                global_shift_x,
                global_shift_y,
                patch_radius,
            );
            if baseline.texture < 1e-4 || baseline.score >= config.local_alignment_trigger_score {
                cells.push(LocalMotionCell::default());
                continue;
            }

            let forward = search_gradient_patch(
                reference_gradient,
                frame_gradient,
                center_x,
                center_y,
                global_shift_x,
                global_shift_y,
                config.local_alignment_search_radius,
                patch_radius,
            );
            if forward.score < config.minimum_local_alignment_score {
                cells.push(LocalMotionCell {
                    disoccluded: true,
                    ..LocalMotionCell::default()
                });
                continue;
            }
            let source_x = center_x - forward.shift_x;
            let source_y = center_y - forward.shift_y;
            let reverse = search_gradient_patch(
                frame_gradient,
                reference_gradient,
                source_x,
                source_y,
                -forward.shift_x,
                -forward.shift_y,
                config.local_alignment_search_radius,
                patch_radius,
            );
            let consistency = ((forward.shift_x + reverse.shift_x).powi(2)
                + (forward.shift_y + reverse.shift_y).powi(2))
            .sqrt();
            if reverse.score < config.minimum_local_alignment_score
                || consistency > config.disocclusion_consistency_threshold
            {
                cells.push(LocalMotionCell {
                    disoccluded: true,
                    ..LocalMotionCell::default()
                });
                continue;
            }

            let residual_x = forward.shift_x - global_shift_x;
            let residual_y = forward.shift_y - global_shift_y;
            let residual_norm = (residual_x * residual_x + residual_y * residual_y).sqrt();
            let improvement = forward.score - baseline.score;
            let active = residual_norm > 0.05 && improvement > 0.015;
            let confidence = ((forward.score - config.minimum_local_alignment_score)
                / (1.0 - config.minimum_local_alignment_score).max(1e-6))
            .clamp(0.0, 1.0)
                * (1.0 - consistency / config.disocclusion_consistency_threshold.max(1e-6))
                    .clamp(0.0, 1.0);
            cells.push(LocalMotionCell {
                shift_x: if active {
                    residual_x * raw_pixels_per_analysis_pixel
                } else {
                    0.0
                },
                shift_y: if active {
                    residual_y * raw_pixels_per_analysis_pixel
                } else {
                    0.0
                },
                confidence,
                active,
                disoccluded: false,
            });
        }
    }

    LocalMotionField {
        cells,
        grid_width,
        grid_height,
        cell_size_analysis: cell_size,
        raw_pixels_per_analysis_pixel,
    }
}

fn search_gradient_patch(
    reference: &GradientAnalysis,
    frame: &GradientAnalysis,
    center_x: f32,
    center_y: f32,
    initial_shift_x: f32,
    initial_shift_y: f32,
    search_radius: usize,
    patch_radius: usize,
) -> PatchAlignment {
    let mut best = gradient_patch_score(
        reference,
        frame,
        center_x,
        center_y,
        initial_shift_x,
        initial_shift_y,
        patch_radius,
    );
    best.shift_x = initial_shift_x;
    best.shift_y = initial_shift_y;
    let radius = search_radius as i32;
    for offset_y in -radius..=radius {
        for offset_x in -radius..=radius {
            let shift_x = initial_shift_x + offset_x as f32;
            let shift_y = initial_shift_y + offset_y as f32;
            let mut candidate = gradient_patch_score(
                reference,
                frame,
                center_x,
                center_y,
                shift_x,
                shift_y,
                patch_radius,
            );
            candidate.shift_x = shift_x;
            candidate.shift_y = shift_y;
            if candidate.score > best.score {
                best = candidate;
            }
        }
    }

    let x_minus = gradient_patch_score(
        reference,
        frame,
        center_x,
        center_y,
        best.shift_x - 1.0,
        best.shift_y,
        patch_radius,
    )
    .score;
    let x_plus = gradient_patch_score(
        reference,
        frame,
        center_x,
        center_y,
        best.shift_x + 1.0,
        best.shift_y,
        patch_radius,
    )
    .score;
    let y_minus = gradient_patch_score(
        reference,
        frame,
        center_x,
        center_y,
        best.shift_x,
        best.shift_y - 1.0,
        patch_radius,
    )
    .score;
    let y_plus = gradient_patch_score(
        reference,
        frame,
        center_x,
        center_y,
        best.shift_x,
        best.shift_y + 1.0,
        patch_radius,
    )
    .score;
    best.shift_x += parabolic_peak_offset(x_minus, best.score, x_plus);
    best.shift_y += parabolic_peak_offset(y_minus, best.score, y_plus);
    let refined = gradient_patch_score(
        reference,
        frame,
        center_x,
        center_y,
        best.shift_x,
        best.shift_y,
        patch_radius,
    );
    PatchAlignment {
        score: refined.score,
        texture: refined.texture,
        ..best
    }
}

fn parabolic_peak_offset(left: f32, center: f32, right: f32) -> f32 {
    if !left.is_finite() || !center.is_finite() || !right.is_finite() {
        return 0.0;
    }
    let denominator = left - 2.0 * center + right;
    if denominator >= -1e-6 {
        0.0
    } else {
        (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
    }
}

fn gradient_patch_score(
    reference: &GradientAnalysis,
    frame: &GradientAnalysis,
    center_x: f32,
    center_y: f32,
    shift_x: f32,
    shift_y: f32,
    radius: usize,
) -> PatchAlignment {
    let radius = radius as i32;
    let center_x = center_x.round() as i32;
    let center_y = center_y.round() as i32;
    let mut dot = 0.0f64;
    let mut reference_energy = 0.0f64;
    let mut frame_energy = 0.0f64;
    let mut count = 0usize;
    for offset_y in -radius..=radius {
        let y = center_y + offset_y;
        if y < 0 || y >= reference.height as i32 {
            continue;
        }
        for offset_x in -radius..=radius {
            let x = center_x + offset_x;
            if x < 0 || x >= reference.width as i32 {
                continue;
            }
            let index = y as usize * reference.width + x as usize;
            let reference_x = reference.gx[index];
            let reference_y = reference.gy[index];
            let Some((frame_x, frame_y)) = frame.sample(x as f32 - shift_x, y as f32 - shift_y)
            else {
                continue;
            };
            dot += f64::from(reference_x * frame_x + reference_y * frame_y);
            reference_energy += f64::from(reference_x * reference_x + reference_y * reference_y);
            frame_energy += f64::from(frame_x * frame_x + frame_y * frame_y);
            count += 1;
        }
    }
    let denominator = (reference_energy * frame_energy).sqrt();
    let score = if count >= 25 && denominator > 1e-12 {
        (dot / denominator).clamp(-1.0, 1.0) as f32
    } else {
        -1.0
    };
    PatchAlignment {
        shift_x,
        shift_y,
        score,
        texture: if count > 0 {
            (reference_energy / count as f64) as f32
        } else {
            0.0
        },
    }
}

fn process_band(
    group: &NativeFrameGroup<'_>,
    calibrations: &[FrameCalibration],
    frame_warps: &[FrameWarp],
    focus_model: Option<&PhysicalFocusModel>,
    focus_steps: usize,
    exposures_per_focus: usize,
    radiance_anchor: f32,
    config: &NativeFusionConfig,
    glare_radius_pixels: usize,
    band_y0: usize,
    band_y1: usize,
    bayer_output: &mut [f32],
    depth_output: &mut [f32],
    confidence_output: &mut [f32],
    uncertainty_output: &mut [f32],
    source_output: &mut [u16],
    detail_source_output: &mut [u16],
    flags_output: &mut [u8],
    frequency_output: &mut [u8],
    glare_output: &mut [u8],
    correction_output: &mut [u8],
) -> Result<()> {
    let width = group.width;
    let halo = config
        .halo
        .max(config.focus_coarse_stride.saturating_mul(2))
        .max(glare_radius_pixels)
        .max(2);
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
        let mut plane_uncertainty = vec![f32::INFINITY; ext_pixels];
        let mut plane_source = vec![u16::MAX; ext_pixels];
        let mut plane_detail_source = vec![u16::MAX; ext_pixels];
        let mut plane_flags = vec![0u8; ext_pixels];
        let mut plane_frequency = vec![0u8; ext_pixels];
        let mut plane_correction = vec![0u8; ext_pixels];
        let mut green = vec![0.0f32; ext_pixels];
        let mut focus_metric = vec![0.0f32; ext_pixels];
        let mut smoothed_focus = vec![0.0f32; ext_pixels];
        let mut plane_glare = vec![0.0f32; ext_pixels];
        let mut best_score = vec![f32::NEG_INFINITY; tile_pixels];
        let mut second_score = vec![f32::NEG_INFINITY; tile_pixels];
        let mut best_detail_score = vec![f32::NEG_INFINITY; tile_pixels];
        let mut second_detail_score = vec![f32::NEG_INFINITY; tile_pixels];
        let mut best_value = vec![0.0f32; tile_pixels];
        let mut second_value = vec![0.0f32; tile_pixels];
        let mut best_uncertainty = vec![f32::INFINITY; tile_pixels];
        let mut second_uncertainty = vec![f32::INFINITY; tile_pixels];
        let mut best_source = vec![u16::MAX; tile_pixels];
        let mut second_source = vec![u16::MAX; tile_pixels];
        let mut best_hdr_detail_source = vec![u16::MAX; tile_pixels];
        let mut second_hdr_detail_source = vec![u16::MAX; tile_pixels];
        let mut best_flags = vec![0u8; tile_pixels];
        let mut second_flags = vec![0u8; tile_pixels];
        let mut best_frequency = vec![0u8; tile_pixels];
        let mut second_frequency = vec![0u8; tile_pixels];
        let mut best_correction = vec![0u8; tile_pixels];
        let mut second_correction = vec![0u8; tile_pixels];
        let mut best_plane = vec![0u16; tile_pixels];
        let mut second_plane = vec![0u16; tile_pixels];
        let mut best_glare = vec![0.0f32; tile_pixels];
        let mut second_glare = vec![0.0f32; tile_pixels];
        let mut maximum_glare = vec![0.0f32; tile_pixels];
        let mut previous_score = vec![f32::NEG_INFINITY; tile_pixels];
        let mut left_score = vec![f32::NEG_INFINITY; tile_pixels];
        let mut right_score = vec![f32::NEG_INFINITY; tile_pixels];

        for focus in 0..focus_steps {
            plane_bayer.fill(0.0);
            plane_valid.fill(false);
            plane_uncertainty.fill(f32::INFINITY);
            plane_source.fill(u16::MAX);
            plane_detail_source.fill(u16::MAX);
            plane_flags.fill(0);
            plane_frequency.fill(0);
            plane_correction.fill(0);
            green.fill(0.0);
            focus_metric.fill(0.0);
            smoothed_focus.fill(0.0);
            plane_glare.fill(0.0);
            let frame_start = focus * exposures_per_focus;
            let frame_end = frame_start + exposures_per_focus;
            for local_y in 0..ext_height {
                let output_y = ext_y0 + local_y;
                for local_x in 0..ext_width {
                    let output_x = ext_x0 + local_x;
                    let index = local_y * ext_width + local_x;
                    if let Some(estimate) = fuse_hdr_sample(
                        group,
                        calibrations,
                        frame_warps,
                        frame_start,
                        frame_end,
                        output_x,
                        output_y,
                        radiance_anchor,
                        config,
                    ) {
                        plane_bayer[index] = estimate.radiance;
                        plane_uncertainty[index] = estimate.uncertainty;
                        plane_source[index] = estimate.source_index;
                        plane_detail_source[index] = estimate.detail_source_index;
                        plane_flags[index] = estimate.flags;
                        plane_frequency[index] = estimate.frequency_flags;
                        plane_correction[index] = estimate.correction_flags;
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
                config.apple_simd_focus,
            );
            if glare_radius_pixels > 0 {
                detect_glare_likelihood(
                    &plane_bayer,
                    &plane_valid,
                    &plane_flags,
                    &mut plane_glare,
                    ext_width,
                    ext_height,
                    glare_radius_pixels,
                );
                focus_metric
                    .iter_mut()
                    .zip(&plane_glare)
                    .for_each(|(metric, glare)| {
                        *metric *= (1.0 - config.glare_focus_suppression * glare).clamp(0.0, 1.0);
                    });
            }
            compute_trimmed_focus_mean(&focus_metric, &mut smoothed_focus, ext_width, ext_height);
            let coarse_focus = CoarseFocusGrid::build(
                &focus_metric,
                &plane_valid,
                ext_width,
                ext_height,
                ext_x0,
                ext_y0,
                config.focus_coarse_stride,
            );

            for tile_y in 0..tile_height {
                let ext_y = band_y0 + tile_y - ext_y0;
                for tile_x in 0..tile_width {
                    let ext_x = x0 + tile_x - ext_x0;
                    let ext_index = ext_y * ext_width + ext_x;
                    let tile_index = tile_y * tile_width + tile_x;
                    if !plane_valid[ext_index] {
                        previous_score[tile_index] = f32::NEG_INFINITY;
                        continue;
                    }
                    let fine_metric = smoothed_focus[ext_index];
                    let coarse_metric = coarse_focus.sample(x0 + tile_x, band_y0 + tile_y);
                    let detail_residual = (fine_metric - coarse_metric).max(0.0);
                    let edge_gate =
                        (detail_residual / (0.25 * coarse_metric + 1e-8)).clamp(0.0, 1.0);
                    let metric = coarse_metric
                        + config.focus_detail_edge_weight * edge_gate * detail_residual;
                    let value = plane_bayer[ext_index];
                    let glare = plane_glare[ext_index];
                    maximum_glare[tile_index] = maximum_glare[tile_index].max(glare);
                    if best_score[tile_index].is_finite()
                        && usize::from(best_plane[tile_index]) + 1 == focus
                    {
                        right_score[tile_index] = metric;
                    }
                    if metric > best_score[tile_index] {
                        second_score[tile_index] = best_score[tile_index];
                        second_detail_score[tile_index] = best_detail_score[tile_index];
                        second_value[tile_index] = best_value[tile_index];
                        second_uncertainty[tile_index] = best_uncertainty[tile_index];
                        second_source[tile_index] = best_source[tile_index];
                        second_hdr_detail_source[tile_index] = best_hdr_detail_source[tile_index];
                        second_flags[tile_index] = best_flags[tile_index];
                        second_frequency[tile_index] = best_frequency[tile_index];
                        second_correction[tile_index] = best_correction[tile_index];
                        second_plane[tile_index] = best_plane[tile_index];
                        second_glare[tile_index] = best_glare[tile_index];
                        best_score[tile_index] = metric;
                        best_detail_score[tile_index] = fine_metric;
                        best_value[tile_index] = value;
                        best_uncertainty[tile_index] = plane_uncertainty[ext_index];
                        best_source[tile_index] = plane_source[ext_index];
                        best_hdr_detail_source[tile_index] = plane_detail_source[ext_index];
                        best_flags[tile_index] = plane_flags[ext_index];
                        best_frequency[tile_index] = plane_frequency[ext_index];
                        best_correction[tile_index] = plane_correction[ext_index];
                        best_plane[tile_index] = focus as u16;
                        best_glare[tile_index] = glare;
                        left_score[tile_index] = if focus > 0 {
                            previous_score[tile_index]
                        } else {
                            f32::NEG_INFINITY
                        };
                        right_score[tile_index] = f32::NEG_INFINITY;
                    } else if metric > second_score[tile_index] {
                        second_score[tile_index] = metric;
                        second_detail_score[tile_index] = fine_metric;
                        second_value[tile_index] = value;
                        second_uncertainty[tile_index] = plane_uncertainty[ext_index];
                        second_source[tile_index] = plane_source[ext_index];
                        second_hdr_detail_source[tile_index] = plane_detail_source[ext_index];
                        second_flags[tile_index] = plane_flags[ext_index];
                        second_frequency[tile_index] = plane_frequency[ext_index];
                        second_correction[tile_index] = plane_correction[ext_index];
                        second_plane[tile_index] = focus as u16;
                        second_glare[tile_index] = glare;
                    }
                    previous_score[tile_index] = metric;
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
                let regional_separation =
                    ((best - second) / (best + second + 1e-8)).clamp(0.0, 1.0);
                let best_detail = best_detail_score[tile_index].max(0.0);
                let second_detail = second_detail_score[tile_index].max(0.0);
                let detail_separation = if second_detail_score[tile_index].is_finite() {
                    ((best_detail - second_detail) / (best_detail + second_detail + 1e-8))
                        .clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let separation = regional_separation.max(detail_separation);
                let best_weight = if second_is_valid {
                    0.5 + 0.5 * separation
                } else {
                    1.0
                };
                bayer_output[output_index] = best_value[tile_index] * best_weight
                    + second_value[tile_index] * (1.0 - best_weight);
                let second_weight = 1.0 - best_weight;
                let selected_glare =
                    best_glare[tile_index] * best_weight + second_glare[tile_index] * second_weight;
                uncertainty_output[output_index] = if second_is_valid {
                    ((best_weight * best_uncertainty[tile_index]).powi(2)
                        + (second_weight * second_uncertainty[tile_index]).powi(2))
                    .sqrt()
                } else {
                    best_uncertainty[tile_index]
                };
                if best_weight >= second_weight {
                    source_output[output_index] = best_source[tile_index];
                    detail_source_output[output_index] = best_hdr_detail_source[tile_index];
                } else {
                    source_output[output_index] = second_source[tile_index];
                    detail_source_output[output_index] = second_hdr_detail_source[tile_index];
                }
                flags_output[output_index] = best_flags[tile_index]
                    | if second_is_valid {
                        second_flags[tile_index]
                    } else {
                        0
                    };
                frequency_output[output_index] = best_frequency[tile_index]
                    | if second_is_valid {
                        second_frequency[tile_index]
                    } else {
                        0
                    };
                correction_output[output_index] = best_correction[tile_index]
                    | if second_is_valid {
                        second_correction[tile_index]
                    } else {
                        0
                    };
                let discrete_focus_position = best_plane[tile_index] as f32 * best_weight
                    + second_plane[tile_index] as f32 * (1.0 - best_weight);
                let best_focus = usize::from(best_plane[tile_index]);
                let focus_position = focus_model
                    .and_then(|model| {
                        subplane_focus_position(
                            model,
                            best_focus,
                            left_score[tile_index],
                            best_score[tile_index],
                            right_score[tile_index],
                        )
                    })
                    .unwrap_or(discrete_focus_position);
                depth_output[output_index] = focus_position / depth_denominator;
                confidence_output[output_index] =
                    if let Some(model) = focus_model {
                        separation
                            * (0.5
                                + 0.5
                                    * model.psf_sampling_balance(
                                        best_focus,
                                        (x0 + tile_x) as f32,
                                        (band_y0 + tile_y) as f32,
                                    ))
                    } else {
                        separation
                    } * (1.0 - config.glare_focus_suppression * selected_glare).clamp(0.0, 1.0);
                glare_output[output_index] = (maximum_glare[tile_index] * 255.0)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
    Ok(())
}

fn subplane_focus_position(
    model: &PhysicalFocusModel,
    best: usize,
    left_score: f32,
    best_score: f32,
    right_score: f32,
) -> Option<f32> {
    if best == 0
        || best + 1 >= model.diopters.len()
        || !left_score.is_finite()
        || !best_score.is_finite()
        || !right_score.is_finite()
    {
        return None;
    }
    let x0 = model.diopters[best - 1];
    let x1 = model.diopters[best];
    let x2 = model.diopters[best + 1];
    let y0 = left_score.max(1e-12).ln();
    let y1 = best_score.max(1e-12).ln();
    let y2 = right_score.max(1e-12).ln();
    let d0 = (x0 - x1) * (x0 - x2);
    let d1 = (x1 - x0) * (x1 - x2);
    let d2 = (x2 - x0) * (x2 - x1);
    if d0.abs() <= 1e-12 || d1.abs() <= 1e-12 || d2.abs() <= 1e-12 {
        return None;
    }
    let quadratic = y0 / d0 + y1 / d1 + y2 / d2;
    let linear = -y0 * (x1 + x2) / d0 - y1 * (x0 + x2) / d1 - y2 * (x0 + x1) / d2;
    if !quadratic.is_finite() || quadratic >= -1e-8 || !linear.is_finite() {
        return None;
    }
    let vertex = -linear / (2.0 * quadratic);
    if !vertex.is_finite()
        || vertex < x0.min(x2)
        || vertex > x0.max(x2)
        || !diopter_between(vertex, x0, x1) && !diopter_between(vertex, x1, x2)
    {
        return None;
    }
    Some(model.index_at_diopter(best, vertex))
}

#[allow(clippy::too_many_arguments)]
fn sample_hdr_frame(
    group: &NativeFrameGroup<'_>,
    calibrations: &[FrameCalibration],
    frame_warps: &[FrameWarp],
    frame_start: usize,
    frame_index: usize,
    output_x: usize,
    output_y: usize,
    radiance_anchor: f32,
    config: &NativeFusionConfig,
) -> HdrFrameObservation {
    let Some(frame) = group.frame(frame_index) else {
        return HdrFrameObservation {
            disoccluded: true,
            ..HdrFrameObservation::default()
        };
    };
    let Some(calibration) = calibrations.get(frame_index).copied() else {
        return HdrFrameObservation {
            disoccluded: true,
            ..HdrFrameObservation::default()
        };
    };
    let Some(plane_warp) = frame_warps.get(frame_start) else {
        return HdrFrameObservation {
            disoccluded: true,
            ..HdrFrameObservation::default()
        };
    };
    let Some(warp) = frame_warps.get(frame_index) else {
        return HdrFrameObservation {
            disoccluded: true,
            ..HdrFrameObservation::default()
        };
    };
    let (plane_x, plane_y) = plane_warp.plane.source_coordinate(
        output_x as f32,
        output_y as f32,
        group.width,
        group.height,
    );
    let coordinate = warp.source_coordinate_from_plane(plane_x, plane_y);
    if coordinate.disoccluded {
        return HdrFrameObservation {
            disoccluded: true,
            ..HdrFrameObservation::default()
        };
    }

    let (crop_x0, crop_y0, _, _) = group.rect.to_bounds();
    let sensor_output_x = crop_x0 + output_x;
    let sensor_output_y = crop_y0 + output_y;
    let site = ((sensor_output_y & 1) << 1) | (sensor_output_x & 1);
    let Some((raw, sample_correction_flags)) = sample_corrected_same_cfa(
        frame,
        group.width,
        group.height,
        coordinate.x,
        coordinate.y,
        output_x & 1,
        output_y & 1,
        crop_x0,
        crop_y0,
        config.sensor_correction_profile.as_ref(),
    ) else {
        return HdrFrameObservation {
            aligned: coordinate.aligned,
            disoccluded: true,
            sample: None,
        };
    };
    let range_dn = calibration.inverse_range.recip();
    let sensor_signal = ((raw - calibration.black) * calibration.inverse_range).max(0.0);
    let gain = config
        .sensor_correction_profile
        .as_ref()
        .map_or(1.0, |profile| {
            profile.gain_at(
                crop_x0 as f32 + coordinate.x,
                crop_y0 as f32 + coordinate.y,
                site,
            )
        });
    let signal = sensor_signal * gain;
    let radiance_scale = radiance_anchor / calibration.exposure * calibration.wb_by_site[site];
    let saturation_signal = calibration.noise_model.saturation_signal(range_dn);
    let censored = sensor_signal >= saturation_signal;
    let radiance = signal * radiance_scale;
    let radiance_gain = gain * radiance_scale;
    let read_variance_dn = calibration.noise_model.read_noise_dn[site].powi(2)
        + calibration.noise_model.black_drift_dn[site].powi(2);
    let read_variance = (read_variance_dn * (radiance_gain / range_dn).powi(2)).max(1e-16);
    let shot_variance_per_radiance =
        (radiance_gain / (calibration.noise_model.electrons_per_dn[site] * range_dn)).max(0.0);
    let variance = (read_variance
        + shot_variance_per_radiance
            * if censored {
                saturation_signal * radiance_gain
            } else {
                radiance
            })
    .max(1e-16);
    let sample = (radiance.is_finite() && variance.is_finite()).then_some(HdrSample {
        radiance,
        variance,
        read_variance,
        shot_variance_per_radiance,
        lower_bound: saturation_signal * gain * radiance_scale,
        fallback_score: calibration.exposure * (1.0 - sensor_signal.min(1.0)),
        frame_index: u16::try_from(frame_index).unwrap_or(u16::MAX),
        censored,
        reference_frame: warp.reference_frame,
        correction_flags: sample_correction_flags
            | (u8::from(config.sensor_correction_profile.is_some()) * SENSOR_CORRECTION_FLAT_FIELD),
    });
    let disoccluded = sample.is_none();
    HdrFrameObservation {
        sample,
        aligned: coordinate.aligned,
        disoccluded,
    }
}

fn fuse_hdr_sample(
    group: &NativeFrameGroup<'_>,
    calibrations: &[FrameCalibration],
    frame_warps: &[FrameWarp],
    frame_start: usize,
    frame_end: usize,
    output_x: usize,
    output_y: usize,
    radiance_anchor: f32,
    config: &NativeFusionConfig,
) -> Option<HdrEstimate> {
    let mut samples = [HdrSample::default(); MAX_HDR_EXPOSURES];
    let mut sample_count = 0usize;
    let mut alignment_flags = 0u8;

    for frame_index in frame_start..frame_end {
        let observation = sample_hdr_frame(
            group,
            calibrations,
            frame_warps,
            frame_start,
            frame_index,
            output_x,
            output_y,
            radiance_anchor,
            config,
        );
        if observation.disoccluded {
            alignment_flags |= FUSION_FLAG_DISOCCLUDED;
        }
        if observation.aligned {
            alignment_flags |= FUSION_FLAG_BRACKET_ALIGNED;
        }
        if let Some(sample) = observation.sample.filter(|_| sample_count < samples.len()) {
            samples[sample_count] = sample;
            sample_count += 1;
        }
    }

    if sample_count == 0 {
        return None;
    }

    let valid = &samples[..sample_count];
    let mut flags = alignment_flags
        | if valid.iter().any(|sample| sample.censored) {
            FUSION_FLAG_CENSORED
        } else {
            0
        };
    if valid.iter().any(|sample| {
        !calibrations[usize::from(sample.frame_index)]
            .noise_model
            .calibrated
    }) {
        flags |= FUSION_FLAG_UNCALIBRATED_NOISE;
    }

    let mut uncensored = [HdrSample::default(); MAX_HDR_EXPOSURES];
    let mut uncensored_count = 0usize;
    for sample in valid.iter().copied().filter(|sample| !sample.censored) {
        uncensored[uncensored_count] = sample;
        uncensored_count += 1;
    }
    if uncensored_count == 0 {
        let strongest = valid
            .iter()
            .max_by(|left, right| left.lower_bound.total_cmp(&right.lower_bound))?;
        return Some(HdrEstimate {
            radiance: strongest.lower_bound,
            uncertainty: f32::INFINITY,
            source_index: u16::MAX,
            detail_source_index: u16::MAX,
            flags: flags | FUSION_FLAG_SOURCE_FALLBACK,
            frequency_flags: 0,
            correction_flags: strongest.correction_flags,
        });
    }

    let usable = &mut uncensored[..uncensored_count];
    let center = sample_median(usable);
    let mut deviations = [HdrSample::default(); MAX_HDR_EXPOSURES];
    for (output, sample) in deviations.iter_mut().zip(usable.iter()) {
        output.radiance = (sample.radiance - center).abs();
    }
    let mad = sample_median(&mut deviations[..uncensored_count]);
    let noise_floor = usable
        .iter()
        .map(|sample| sample.variance.sqrt())
        .sum::<f32>()
        / uncensored_count as f32;
    let robust_scale = (1.4826 * mad).max(2.5 * noise_floor).max(2e-6);
    let cutoff = if config.deghost_strength > 0.0 {
        4.685 / config.deghost_strength.max(1e-3)
    } else {
        f32::INFINITY
    };
    let mut robust_sum = 0.0;
    let mut robust_weight_sum = 0.0;
    let mut dominant_weight = -1.0f32;
    let mut source_index = u16::MAX;
    let mut rejected = false;
    let mut correction_flags = 0u8;
    let mut robust_factors = [0.0f32; MAX_HDR_EXPOSURES];
    for (sample_index, sample) in usable.iter().enumerate() {
        let normalized_residual = if cutoff.is_finite() {
            (sample.radiance - center) / (cutoff * robust_scale)
        } else {
            0.0
        };
        let robust_weight = if normalized_residual.abs() < 1.0 {
            let remaining = 1.0 - normalized_residual * normalized_residual;
            remaining * remaining
        } else {
            rejected = true;
            0.0
        };
        robust_factors[sample_index] = robust_weight;
        let weight = robust_weight / sample.variance;
        if weight > 0.0 {
            correction_flags |= sample.correction_flags;
        }
        robust_sum += weight * sample.radiance;
        robust_weight_sum += weight;
        if weight > dominant_weight {
            dominant_weight = weight;
            source_index = sample.frame_index;
        }
    }

    if robust_weight_sum > 0.0 {
        let mut radiance = robust_sum / robust_weight_sum;
        let mut uncertainty = robust_weight_sum.recip().sqrt();
        let weighted_residual = usable
            .iter()
            .zip(robust_factors)
            .filter(|(_, robust_factor)| *robust_factor > 0.0)
            .map(|(sample, robust_factor)| {
                let residual = sample.radiance - radiance;
                robust_factor * residual * residual / sample.variance
            })
            .sum::<f32>();
        let effective_count = robust_factors[..uncensored_count]
            .iter()
            .filter(|factor| **factor > 0.0)
            .count();
        let residual_inflation = if effective_count > 1 {
            (weighted_residual / (effective_count - 1) as f32)
                .max(1.0)
                .sqrt()
        } else {
            1.0
        };
        uncertainty *= residual_inflation;

        if valid.iter().any(|sample| sample.censored) {
            let posterior = solve_censored_poisson_gaussian(
                usable,
                &robust_factors[..uncensored_count],
                valid,
                radiance,
                uncertainty,
                residual_inflation,
            );
            radiance = posterior.radiance;
            uncertainty = posterior.uncertainty;
            if posterior.used_censored {
                correction_flags |= posterior.correction_flags;
            }
            if posterior.censor_conflict {
                flags |= FUSION_FLAG_CENSOR_CONFLICT;
            }
        }
        let mut detail_source_index = source_index;
        let mut frequency_flags = 0u8;
        let frequency_triggered =
            mad > config.frequency_separation_trigger_sigma * noise_floor.max(1e-8);
        if config.frequency_separated_deghosting
            && config.deghost_strength > 0.0
            && uncensored_count >= 2
            && !valid.iter().any(|sample| sample.censored)
            && (rejected || alignment_flags & FUSION_FLAG_DISOCCLUDED != 0 || frequency_triggered)
        {
            if let Some(frequency) = frequency_separated_estimate(
                group,
                calibrations,
                frame_warps,
                frame_start,
                output_x,
                output_y,
                radiance_anchor,
                usable,
                config,
            ) {
                radiance = frequency.radiance;
                uncertainty = uncertainty.max(frequency.uncertainty);
                source_index = frequency.low_source_index;
                detail_source_index = frequency.detail_source_index;
                frequency_flags = frequency.flags;
                correction_flags |= frequency.correction_flags;
            }
        }
        if rejected {
            flags |= FUSION_FLAG_OUTLIER_REJECTED;
        }
        if flags & FUSION_FLAG_DISOCCLUDED != 0
            && valid
                .iter()
                .any(|sample| sample.frame_index == source_index && sample.reference_frame)
        {
            flags |= FUSION_FLAG_SOURCE_FALLBACK;
        }
        Some(HdrEstimate {
            radiance,
            uncertainty,
            source_index,
            detail_source_index,
            flags,
            frequency_flags,
            correction_flags,
        })
    } else {
        usable
            .iter()
            .max_by(|left, right| left.fallback_score.total_cmp(&right.fallback_score))
            .map(|sample| HdrEstimate {
                radiance: sample.radiance,
                uncertainty: sample.variance.sqrt(),
                source_index: sample.frame_index,
                detail_source_index: sample.frame_index,
                flags: flags | FUSION_FLAG_SOURCE_FALLBACK | FUSION_FLAG_OUTLIER_REJECTED,
                frequency_flags: 0,
                correction_flags: sample.correction_flags,
            })
    }
}

#[allow(clippy::too_many_arguments)]
fn frequency_separated_estimate(
    group: &NativeFrameGroup<'_>,
    calibrations: &[FrameCalibration],
    frame_warps: &[FrameWarp],
    frame_start: usize,
    output_x: usize,
    output_y: usize,
    radiance_anchor: f32,
    center_samples: &[HdrSample],
    config: &NativeFusionConfig,
) -> Option<FrequencyEstimate> {
    let mut samples = [FrequencySample::default(); MAX_HDR_EXPOSURES];
    let mut count = 0usize;
    for center in center_samples {
        let sample = same_cfa_frequency_sample(
            group,
            calibrations,
            frame_warps,
            frame_start,
            output_x,
            output_y,
            radiance_anchor,
            *center,
            config,
        )?;
        samples[count] = sample;
        count += 1;
    }
    if count < 2 {
        return None;
    }
    let samples = &samples[..count];
    let low = robust_frequency_component(samples, true, config.deghost_strength)?;
    let detail = robust_frequency_component(samples, false, config.deghost_strength)?;
    let minimum = center_samples
        .iter()
        .map(|sample| sample.radiance)
        .min_by(f32::total_cmp)?;
    let maximum = center_samples
        .iter()
        .map(|sample| sample.radiance)
        .max_by(f32::total_cmp)?;
    let unconstrained = low.value + detail.value;
    let radiance = unconstrained.clamp(minimum, maximum).max(0.0);
    let mut flags = FREQUENCY_FLAG_SEPARATED;
    if low.source_index != detail.source_index {
        flags |= FREQUENCY_FLAG_SPLIT_SOURCES;
    }
    if detail.contributors == 1 {
        flags |= FREQUENCY_FLAG_DETAIL_SINGLE_SOURCE;
    }
    if detail.reference_fallback {
        flags |= FREQUENCY_FLAG_DETAIL_REFERENCE;
    }
    if (radiance - unconstrained).abs() > 1e-8 * unconstrained.abs().max(1.0) {
        flags |= FREQUENCY_FLAG_MEASUREMENT_CLAMPED;
    }
    Some(FrequencyEstimate {
        radiance,
        // Low and detail are correlated. Summing standard deviations is a
        // conservative Cauchy-Schwarz bound that cannot understate variance.
        uncertainty: low.uncertainty + detail.uncertainty,
        low_source_index: low.source_index,
        detail_source_index: detail.source_index,
        flags,
        correction_flags: low.correction_flags | detail.correction_flags,
    })
}

#[allow(clippy::too_many_arguments)]
fn same_cfa_frequency_sample(
    group: &NativeFrameGroup<'_>,
    calibrations: &[FrameCalibration],
    frame_warps: &[FrameWarp],
    frame_start: usize,
    output_x: usize,
    output_y: usize,
    radiance_anchor: f32,
    center: HdrSample,
    config: &NativeFusionConfig,
) -> Option<FrequencySample> {
    const OFFSETS: [(isize, isize, f32); 9] = [
        (-2, -2, 1.0),
        (0, -2, 2.0),
        (2, -2, 1.0),
        (-2, 0, 2.0),
        (0, 0, 4.0),
        (2, 0, 2.0),
        (-2, 2, 1.0),
        (0, 2, 2.0),
        (2, 2, 1.0),
    ];
    let frame_index = usize::from(center.frame_index);
    let mut spatial_samples = [HdrSample::default(); 9];
    let mut spatial_weights = [0.0f32; 9];
    let mut count = 0usize;
    let mut weight_sum = 0.0f32;
    let mut correction_flags = center.correction_flags;
    for (dx, dy, weight) in OFFSETS {
        let sample = if dx == 0 && dy == 0 {
            Some(center)
        } else {
            let Some(neighbor_x) = output_x.checked_add_signed(dx) else {
                continue;
            };
            let Some(neighbor_y) = output_y.checked_add_signed(dy) else {
                continue;
            };
            if neighbor_x >= group.width || neighbor_y >= group.height {
                continue;
            }
            sample_hdr_frame(
                group,
                calibrations,
                frame_warps,
                frame_start,
                frame_index,
                neighbor_x,
                neighbor_y,
                radiance_anchor,
                config,
            )
            .sample
        };
        let Some(sample) = sample.filter(|sample| !sample.censored) else {
            continue;
        };
        spatial_samples[count] = sample;
        spatial_weights[count] = weight;
        count += 1;
        weight_sum += weight;
        correction_flags |= sample.correction_flags;
    }
    if count < 5 || weight_sum < 8.0 {
        return None;
    }
    let inverse_weight = weight_sum.recip();
    let mut low = 0.0f32;
    let mut low_variance = 0.0f32;
    for (sample, weight) in spatial_samples[..count]
        .iter()
        .zip(&spatial_weights[..count])
    {
        let coefficient = *weight * inverse_weight;
        low += coefficient * sample.radiance;
        low_variance += coefficient * coefficient * sample.variance;
    }
    let center_coefficient = 4.0 * inverse_weight;
    let detail_variance =
        (center.variance + low_variance - 2.0 * center_coefficient * center.variance).max(1e-16);
    Some(FrequencySample {
        low,
        detail: center.radiance - low,
        low_variance: low_variance.max(1e-16),
        detail_variance,
        frame_index: center.frame_index,
        reference_frame: center.reference_frame,
        correction_flags,
    })
}

fn robust_frequency_component(
    samples: &[FrequencySample],
    low_component: bool,
    deghost_strength: f32,
) -> Option<FrequencyComponent> {
    let mut values = [0.0f32; MAX_HDR_EXPOSURES];
    let mut variances = [0.0f32; MAX_HDR_EXPOSURES];
    for (index, sample) in samples.iter().enumerate() {
        values[index] = if low_component {
            sample.low
        } else {
            sample.detail
        };
        variances[index] = if low_component {
            sample.low_variance
        } else {
            sample.detail_variance
        }
        .max(1e-16);
    }
    let mut median_values = values;
    let center = median_f32(&mut median_values[..samples.len()]);
    let mut deviations = [0.0f32; MAX_HDR_EXPOSURES];
    for (index, value) in values[..samples.len()].iter().enumerate() {
        deviations[index] = (*value - center).abs();
    }
    let mad = median_f32(&mut deviations[..samples.len()]);
    let noise_floor = variances[..samples.len()]
        .iter()
        .map(|variance| variance.sqrt())
        .sum::<f32>()
        / samples.len() as f32;
    let scale = (1.4826 * mad).max(2.5 * noise_floor).max(2e-6);
    let cutoff = 4.685 / deghost_strength.max(1e-3);
    let mut weighted_sum = 0.0f32;
    let mut weight_sum = 0.0f32;
    let mut dominant_weight = -1.0f32;
    let mut source_index = u16::MAX;
    let mut correction_flags = 0u8;
    let mut contributors = 0usize;
    let mut robust_factors = [0.0f32; MAX_HDR_EXPOSURES];
    for (index, sample) in samples.iter().enumerate() {
        let residual = (values[index] - center) / (cutoff * scale);
        let robust = if residual.abs() < 1.0 {
            let remaining = 1.0 - residual * residual;
            remaining * remaining
        } else {
            0.0
        };
        robust_factors[index] = robust;
        let weight = robust / variances[index];
        if weight <= 0.0 {
            continue;
        }
        contributors += 1;
        correction_flags |= sample.correction_flags;
        weighted_sum += weight * values[index];
        weight_sum += weight;
        if weight > dominant_weight {
            dominant_weight = weight;
            source_index = sample.frame_index;
        }
    }
    if weight_sum <= 0.0 {
        return None;
    }
    let value = weighted_sum / weight_sum;
    let weighted_residual = samples
        .iter()
        .enumerate()
        .filter(|(index, _)| robust_factors[*index] > 0.0)
        .map(|(index, _)| {
            robust_factors[index] * (values[index] - value).powi(2) / variances[index]
        })
        .sum::<f32>();
    let inflation = if contributors > 1 {
        (weighted_residual / (contributors - 1) as f32)
            .max(1.0)
            .sqrt()
    } else {
        1.0
    };
    let reference_fallback = contributors == 1
        && samples
            .iter()
            .any(|sample| sample.frame_index == source_index && sample.reference_frame);
    Some(FrequencyComponent {
        value,
        uncertainty: weight_sum.recip().sqrt() * inflation,
        source_index,
        contributors,
        reference_fallback,
        correction_flags,
    })
}

fn median_f32(values: &mut [f32]) -> f32 {
    match values {
        [] => return 0.0,
        [value] => return *value,
        [left, right] => return 0.5 * (*left + *right),
        [first, second, third] => {
            return *first + *second + *third
                - first.min(*second).min(*third)
                - first.max(*second).max(*third);
        }
        _ => {}
    }
    let middle = values.len() / 2;
    values.select_nth_unstable_by(middle, f32::total_cmp);
    if values.len() & 1 == 1 {
        values[middle]
    } else {
        let lower = values[..middle]
            .iter()
            .copied()
            .max_by(f32::total_cmp)
            .unwrap_or(values[middle]);
        0.5 * (lower + values[middle])
    }
}

fn solve_censored_poisson_gaussian(
    uncensored: &[HdrSample],
    robust_factors: &[f32],
    all_samples: &[HdrSample],
    initial_radiance: f32,
    initial_uncertainty: f32,
    residual_inflation: f32,
) -> CensoredPosterior {
    let mut accepted_censored = [false; MAX_HDR_EXPOSURES];
    let mut used_censored = false;
    let mut censor_conflict = false;
    let mut correction_flags = 0u8;
    let initial = f64::from(initial_radiance.max(0.0));
    let initial_variance = f64::from(initial_uncertainty.max(1e-8)).powi(2);

    for (index, sample) in all_samples.iter().enumerate() {
        if !sample.censored {
            continue;
        }
        let constraint_variance =
            hdr_variance_at(*sample, initial.max(f64::from(sample.lower_bound)));
        let compatibility_sigma = (initial_variance + constraint_variance).sqrt();
        if f64::from(sample.lower_bound) > initial + CENSOR_CONFLICT_SIGMA * compatibility_sigma {
            censor_conflict = true;
            continue;
        }
        accepted_censored[index] = true;
        used_censored = true;
        correction_flags |= sample.correction_flags;
    }

    if !used_censored {
        return CensoredPosterior {
            radiance: initial_radiance,
            uncertainty: initial_uncertainty,
            used_censored,
            censor_conflict,
            correction_flags,
        };
    }

    let mut radiance = initial;
    for _ in 0..MAX_CENSORED_POSTERIOR_ITERATIONS {
        let (score, information) = censored_posterior_terms(
            radiance,
            uncensored,
            robust_factors,
            all_samples,
            &accepted_censored,
        );
        if !score.is_finite() || !information.is_finite() || information <= 0.0 {
            break;
        }
        let posterior_sigma = information.recip().sqrt();
        let maximum_step = (4.0 * posterior_sigma.max(initial_variance.sqrt())).max(1e-8);
        let step = (score / information).clamp(-maximum_step, maximum_step);
        let next = (radiance + step).max(0.0);
        radiance = next;
        if step.abs() <= 1e-7 * radiance.abs().max(1.0) {
            break;
        }
    }

    let (_, information) = censored_posterior_terms(
        radiance,
        uncensored,
        robust_factors,
        all_samples,
        &accepted_censored,
    );
    let uncertainty = if information.is_finite() && information > 0.0 {
        information.recip().sqrt() * f64::from(residual_inflation.max(1.0))
    } else {
        f64::from(initial_uncertainty)
    };
    CensoredPosterior {
        radiance: radiance as f32,
        uncertainty: uncertainty as f32,
        used_censored,
        censor_conflict,
        correction_flags,
    }
}

fn censored_posterior_terms(
    radiance: f64,
    uncensored: &[HdrSample],
    robust_factors: &[f32],
    all_samples: &[HdrSample],
    accepted_censored: &[bool; MAX_HDR_EXPOSURES],
) -> (f64, f64) {
    let mut score = 0.0f64;
    let mut information = 0.0f64;
    for (sample, robust_factor) in uncensored.iter().zip(robust_factors) {
        let robust_factor = f64::from(*robust_factor);
        if robust_factor <= 0.0 {
            continue;
        }
        let variance = hdr_variance_at(*sample, radiance);
        let shot = f64::from(sample.shot_variance_per_radiance.max(0.0));
        let residual = f64::from(sample.radiance) - radiance;
        score += robust_factor
            * (residual / variance
                + 0.5 * shot * (residual * residual / (variance * variance) - variance.recip()));
        // Fisher information is positive and stable for heteroscedastic
        // Poisson-Gaussian observations.
        information +=
            robust_factor * (variance.recip() + 0.5 * shot * shot / (variance * variance));
    }

    for (index, sample) in all_samples.iter().enumerate() {
        if !accepted_censored[index] {
            continue;
        }
        let variance = hdr_variance_at(*sample, radiance);
        let sigma = variance.sqrt();
        let shot = f64::from(sample.shot_variance_per_radiance.max(0.0));
        let distance = radiance - f64::from(sample.lower_bound);
        let z = distance / sigma;
        let z_prime = variance.recip().sqrt() - 0.5 * distance * shot / (variance * sigma);
        let z_second = -shot / (variance * sigma)
            + 0.75 * distance * shot * shot / (variance * variance * sigma);
        let inverse_mills = inverse_mills_ratio(z);
        score += inverse_mills * z_prime;
        let observed_information =
            inverse_mills * (z + inverse_mills) * z_prime * z_prime - inverse_mills * z_second;
        information += observed_information.max(0.0);
    }
    (score, information)
}

fn hdr_variance_at(sample: HdrSample, radiance: f64) -> f64 {
    let read = f64::from(sample.read_variance.max(0.0));
    let shot = f64::from(sample.shot_variance_per_radiance.max(0.0));
    let modeled = read + shot * radiance.max(0.0);
    if modeled > 0.0 {
        modeled.max(1e-16)
    } else {
        f64::from(sample.variance).max(1e-16)
    }
}

fn inverse_mills_ratio(z: f64) -> f64 {
    if z < -8.0 {
        let x = -z;
        let inverse = x.recip();
        return x + inverse - 2.0 * inverse.powi(3) + 10.0 * inverse.powi(5);
    }
    let density = (-0.5 * z * z).exp() * 0.398_942_280_401_432_7;
    if z > 8.0 {
        return density;
    }
    density / normal_cdf(z).max(f64::MIN_POSITIVE)
}

fn normal_cdf(z: f64) -> f64 {
    let absolute = z.abs();
    let t = 1.0 / (1.0 + 0.231_641_9 * absolute);
    let polynomial = t
        * (0.319_381_530
            + t * (-0.356_563_782
                + t * (1.781_477_937 + t * (-1.821_255_978 + t * 1.330_274_429))));
    let lower_tail = 0.398_942_280_401_432_7 * (-0.5 * absolute * absolute).exp() * polynomial;
    if z >= 0.0 {
        1.0 - lower_tail
    } else {
        lower_tail
    }
}

fn sample_median(samples: &mut [HdrSample]) -> f32 {
    match samples {
        [] => return 0.0,
        [sample] => return sample.radiance,
        [left, right] => return (left.radiance + right.radiance) * 0.5,
        [first, second, third] => {
            let minimum = first.radiance.min(second.radiance).min(third.radiance);
            let maximum = first.radiance.max(second.radiance).max(third.radiance);
            return first.radiance + second.radiance + third.radiance - minimum - maximum;
        }
        _ => {}
    }
    let middle = samples.len() / 2;
    samples.select_nth_unstable_by(middle, |left, right| {
        left.radiance.total_cmp(&right.radiance)
    });
    if samples.len() % 2 == 1 {
        samples[middle].radiance
    } else {
        let lower = samples[..middle]
            .iter()
            .map(|sample| sample.radiance)
            .max_by(f32::total_cmp)
            .unwrap_or(samples[middle].radiance);
        (lower + samples[middle].radiance) * 0.5
    }
}

#[allow(clippy::too_many_arguments)]
fn refuse_regularized_depth(
    group: &NativeFrameGroup<'_>,
    calibrations: &[FrameCalibration],
    frame_warps: &[FrameWarp],
    focus_steps: usize,
    exposures_per_focus: usize,
    radiance_anchor: f32,
    config: &NativeFusionConfig,
    source_depth: &[f32],
    regularized_depth: &[f32],
    boundary_trimap: &[u8],
    bayer: &mut [f32],
    uncertainty: &mut [f32],
    source_map: &mut [u16],
    detail_source_map: &mut [u16],
    fusion_flags: &mut [u8],
    frequency_flags: &mut [u8],
    sensor_correction_map: &mut [u8],
) -> Result<(usize, usize)> {
    let width = group.width;
    let focus_denominator = (focus_steps - 1) as f32;
    let (refusion_pixels, boundary_source_fallback_pixels) = bayer
        .par_chunks_mut(width)
        .zip(uncertainty.par_chunks_mut(width))
        .zip(source_map.par_chunks_mut(width))
        .zip(detail_source_map.par_chunks_mut(width))
        .zip(fusion_flags.par_chunks_mut(width))
        .zip(frequency_flags.par_chunks_mut(width))
        .zip(sensor_correction_map.par_chunks_mut(width))
        .enumerate()
        .map(
            |(
                y,
                (
                    (
                        (
                            (((output_row, uncertainty_row), source_row), detail_source_row),
                            flags_row,
                        ),
                        frequency_row,
                    ),
                    correction_row,
                ),
            )|
             -> Result<(usize, usize)> {
                let mut changed = 0usize;
                let mut boundary_fallbacks = 0usize;
                for (x, output) in output_row.iter_mut().enumerate() {
                    let index = y * width + x;
                    let source_focus = source_depth[index].clamp(0.0, 1.0) * focus_denominator;
                    let regularized_focus =
                        regularized_depth[index].clamp(0.0, 1.0) * focus_denominator;
                    let mixed_boundary = boundary_trimap
                        .get(index)
                        .is_some_and(|state| *state != BOUNDARY_TRIMAP_INTERIOR);
                    if !mixed_boundary && source_focus.round() == regularized_focus.round() {
                        continue;
                    }
                    let focus_position = if mixed_boundary {
                        regularized_focus.round()
                    } else {
                        regularized_focus
                    };
                    let lower_focus = focus_position.floor() as usize;
                    let upper_focus = focus_position.ceil() as usize;
                    let fraction = focus_position - lower_focus as f32;
                    let sample = |focus: usize| {
                        let frame_start = focus * exposures_per_focus;
                        fuse_hdr_sample(
                            group,
                            calibrations,
                            frame_warps,
                            frame_start,
                            frame_start + exposures_per_focus,
                            x,
                            y,
                            radiance_anchor,
                            config,
                        )
                    };
                    let estimate = if lower_focus == upper_focus {
                        sample(lower_focus)
                    } else {
                        match (sample(lower_focus), sample(upper_focus)) {
                            (Some(lower), Some(upper)) => Some(HdrEstimate {
                                radiance: lower.radiance
                                    + (upper.radiance - lower.radiance) * fraction,
                                uncertainty: ((lower.uncertainty * (1.0 - fraction)).powi(2)
                                    + (upper.uncertainty * fraction).powi(2))
                                .sqrt(),
                                source_index: if fraction < 0.5 {
                                    lower.source_index
                                } else {
                                    upper.source_index
                                },
                                detail_source_index: if fraction < 0.5 {
                                    lower.detail_source_index
                                } else {
                                    upper.detail_source_index
                                },
                                flags: lower.flags | upper.flags,
                                frequency_flags: lower.frequency_flags | upper.frequency_flags,
                                correction_flags: lower.correction_flags | upper.correction_flags,
                            }),
                            (Some(value), None) | (None, Some(value)) => Some(value),
                            (None, None) => None,
                        }
                    };
                    if let Some(estimate) = estimate {
                        *output = estimate.radiance;
                        uncertainty_row[x] = estimate.uncertainty;
                        source_row[x] = estimate.source_index;
                        detail_source_row[x] = estimate.detail_source_index;
                        flags_row[x] = estimate.flags
                            | if mixed_boundary {
                                FUSION_FLAG_SOURCE_FALLBACK
                            } else {
                                0
                            };
                        frequency_row[x] = estimate.frequency_flags;
                        correction_row[x] = estimate.correction_flags;
                        changed += 1;
                        boundary_fallbacks += usize::from(mixed_boundary);
                    }
                }
                Ok((changed, boundary_fallbacks))
            },
        )
        .try_reduce(
            || (0usize, 0usize),
            |left, right| Ok((left.0 + right.0, left.1 + right.1)),
        )?;
    tracing::info!(
        "Depth-consistent refusion corrected {} / {} pixels ({:.2}%); {} physical boundary pixels anchored to one measured plane",
        refusion_pixels,
        bayer.len(),
        refusion_pixels as f64 * 100.0 / bayer.len() as f64,
        boundary_source_fallback_pixels,
    );
    Ok((refusion_pixels, boundary_source_fallback_pixels))
}

#[allow(clippy::too_many_arguments)]
fn sample_corrected_same_cfa(
    frame: &[u16],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    parity_x: usize,
    parity_y: usize,
    sensor_origin_x: usize,
    sensor_origin_y: usize,
    correction: Option<&SensorCorrectionProfile>,
) -> Option<(f32, u8)> {
    if correction.is_none() {
        return sample_same_cfa(frame, width, height, x, y, parity_x, parity_y)
            .map(|value| (value, 0));
    }
    if x < -2.0 || y < -2.0 || x > width as f32 + 1.0 || y > height as f32 + 1.0 {
        return None;
    }
    let correction = correction?;
    let (x0, x1, tx) = same_parity_axis(x, parity_x, width)?;
    let (y0, y1, ty) = same_parity_axis(y, parity_y, height)?;
    let mut repaired = false;
    let mut sample = |sample_x: usize, sample_y: usize| {
        corrected_raw_pixel(
            frame,
            width,
            height,
            sample_x,
            sample_y,
            sensor_origin_x,
            sensor_origin_y,
            correction,
        )
        .map(|(value, was_repaired)| {
            repaired |= was_repaired;
            value
        })
    };
    let p00 = sample(x0, y0)?;
    let p10 = sample(x1, y0)?;
    let p01 = sample(x0, y1)?;
    let p11 = sample(x1, y1)?;
    let top = p00 + (p10 - p00) * tx;
    let bottom = p01 + (p11 - p01) * tx;
    Some((
        top + (bottom - top) * ty,
        if repaired {
            SENSOR_CORRECTION_DEFECT_REPAIRED
        } else {
            0
        },
    ))
}

#[allow(clippy::too_many_arguments)]
fn corrected_raw_pixel(
    frame: &[u16],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    sensor_origin_x: usize,
    sensor_origin_y: usize,
    correction: &SensorCorrectionProfile,
) -> Option<(f32, bool)> {
    let sensor_x = sensor_origin_x.checked_add(x)?;
    let sensor_y = sensor_origin_y.checked_add(y)?;
    let value = f32::from(*frame.get(y.checked_mul(width)?.checked_add(x)?)?);
    if !correction.is_defective(sensor_x, sensor_y) {
        return Some((value, false));
    }
    let mut neighbors = [0.0f32; 24];
    let mut count = 0usize;
    for dy in [-4isize, -2, 0, 2, 4] {
        for dx in [-4isize, -2, 0, 2, 4] {
            if dx == 0 && dy == 0 {
                continue;
            }
            let Some(neighbor_x) = x.checked_add_signed(dx) else {
                continue;
            };
            let Some(neighbor_y) = y.checked_add_signed(dy) else {
                continue;
            };
            if neighbor_x >= width || neighbor_y >= height {
                continue;
            }
            let full_x = sensor_origin_x.checked_add(neighbor_x)?;
            let full_y = sensor_origin_y.checked_add(neighbor_y)?;
            if correction.is_defective(full_x, full_y) {
                continue;
            }
            neighbors[count] =
                f32::from(*frame.get(neighbor_y.checked_mul(width)?.checked_add(neighbor_x)?)?);
            count += 1;
        }
    }
    if count < 4 {
        return None;
    }
    neighbors[..count].sort_unstable_by(f32::total_cmp);
    let middle = count / 2;
    let replacement = if count & 1 == 0 {
        0.5 * (neighbors[middle - 1] + neighbors[middle])
    } else {
        neighbors[middle]
    };
    Some((replacement, true))
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

fn detect_glare_likelihood(
    radiance: &[f32],
    valid: &[bool],
    flags: &[u8],
    output: &mut [f32],
    width: usize,
    height: usize,
    radius: usize,
) {
    if radius == 0 || width == 0 || height == 0 {
        return;
    }
    let pixel_count = width * height;
    debug_assert_eq!(radiance.len(), pixel_count);
    debug_assert_eq!(valid.len(), pixel_count);
    debug_assert_eq!(flags.len(), pixel_count);
    debug_assert_eq!(output.len(), pixel_count);

    let mut distance = vec![f32::INFINITY; pixel_count];
    let mut has_core = false;
    for index in 0..pixel_count {
        if valid[index] && flags[index] & FUSION_FLAG_CENSORED != 0 {
            distance[index] = 0.0;
            has_core = true;
        }
    }
    if !has_core {
        return;
    }

    // Deterministic eight-neighbor chamfer distance. The support is used only
    // to suppress focus evidence; archival radiance is never modified.
    let diagonal = std::f32::consts::SQRT_2;
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let mut best = distance[index];
            if x > 0 {
                best = best.min(distance[index - 1] + 1.0);
            }
            if y > 0 {
                best = best.min(distance[index - width] + 1.0);
                if x > 0 {
                    best = best.min(distance[index - width - 1] + diagonal);
                }
                if x + 1 < width {
                    best = best.min(distance[index - width + 1] + diagonal);
                }
            }
            distance[index] = best;
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let index = y * width + x;
            let mut best = distance[index];
            if x + 1 < width {
                best = best.min(distance[index + 1] + 1.0);
            }
            if y + 1 < height {
                best = best.min(distance[index + width] + 1.0);
                if x > 0 {
                    best = best.min(distance[index + width - 1] + diagonal);
                }
                if x + 1 < width {
                    best = best.min(distance[index + width + 1] + diagonal);
                }
            }
            distance[index] = best;
        }
    }

    let integral_width = width + 1;
    let mut sum_integral = vec![0.0f64; integral_width * (height + 1)];
    let mut count_integral = vec![0u32; integral_width * (height + 1)];
    for y in 0..height {
        let mut row_sum = 0.0f64;
        let mut row_count = 0u32;
        for x in 0..width {
            let index = y * width + x;
            if valid[index] && radiance[index].is_finite() {
                row_sum += f64::from(radiance[index].max(0.0));
                row_count += 1;
            }
            let integral_index = (y + 1) * integral_width + x + 1;
            sum_integral[integral_index] = sum_integral[y * integral_width + x + 1] + row_sum;
            count_integral[integral_index] = count_integral[y * integral_width + x + 1] + row_count;
        }
    }

    let near_radius = (radius / 4).max(2);
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if !valid[index] || distance[index] > radius as f32 {
                continue;
            }
            if distance[index] == 0.0 {
                output[index] = 1.0;
                continue;
            }
            let near = integral_box_mean(
                &sum_integral,
                &count_integral,
                integral_width,
                width,
                height,
                x,
                y,
                near_radius,
            );
            let wide = integral_box_mean(
                &sum_integral,
                &count_integral,
                integral_width,
                width,
                height,
                x,
                y,
                radius,
            );
            let bloom_excess = ((near - wide) / (near.abs() + 0.02)).clamp(0.0, 1.0);
            let inconsistent_bracket = f32::from(flags[index] & FUSION_FLAG_OUTLIER_REJECTED != 0);
            let proximity = (1.0 - distance[index] / radius as f32).clamp(0.0, 1.0);
            let evidence = 0.45 + 0.45 * bloom_excess + 0.10 * inconsistent_bracket;
            output[index] = (proximity * evidence).clamp(0.0, 1.0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn integral_box_mean(
    sum: &[f64],
    count: &[u32],
    integral_width: usize,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    radius: usize,
) -> f32 {
    let x0 = x.saturating_sub(radius);
    let y0 = y.saturating_sub(radius);
    let x1 = (x + radius + 1).min(width);
    let y1 = (y + radius + 1).min(height);
    let top_left = y0 * integral_width + x0;
    let top_right = y0 * integral_width + x1;
    let bottom_left = y1 * integral_width + x0;
    let bottom_right = y1 * integral_width + x1;
    let total = sum[bottom_right] + sum[top_left] - sum[top_right] - sum[bottom_left];
    let samples = i64::from(count[bottom_right]) + i64::from(count[top_left])
        - i64::from(count[top_right])
        - i64::from(count[bottom_left]);
    if samples <= 0 {
        0.0
    } else {
        (total / samples as f64) as f32
    }
}

struct CoarseFocusGrid {
    values: Vec<f32>,
    width: usize,
    height: usize,
    first_block_x: usize,
    first_block_y: usize,
    stride: usize,
}

impl CoarseFocusGrid {
    fn build(
        metric: &[f32],
        valid: &[bool],
        width: usize,
        height: usize,
        origin_x: usize,
        origin_y: usize,
        stride: usize,
    ) -> Self {
        let stride = stride.max(1);
        let first_block_x = origin_x / stride;
        let first_block_y = origin_y / stride;
        let last_block_x = (origin_x + width.saturating_sub(1)) / stride;
        let last_block_y = (origin_y + height.saturating_sub(1)) / stride;
        let grid_width = last_block_x - first_block_x + 1;
        let grid_height = last_block_y - first_block_y + 1;
        let mut values = vec![0.0f32; grid_width * grid_height];

        for grid_y in 0..grid_height {
            let block_y = first_block_y + grid_y;
            let global_y0 = (block_y * stride).max(origin_y);
            let global_y1 = ((block_y + 1) * stride).min(origin_y + height);
            for grid_x in 0..grid_width {
                let block_x = first_block_x + grid_x;
                let global_x0 = (block_x * stride).max(origin_x);
                let global_x1 = ((block_x + 1) * stride).min(origin_x + width);
                let mut sum = 0.0f32;
                let mut count = 0usize;
                for global_y in global_y0..global_y1 {
                    let local_y = global_y - origin_y;
                    for global_x in global_x0..global_x1 {
                        let local_x = global_x - origin_x;
                        let index = local_y * width + local_x;
                        if valid[index] {
                            sum += metric[index];
                            count += 1;
                        }
                    }
                }
                values[grid_y * grid_width + grid_x] = sum / count.max(1) as f32;
            }
        }

        Self {
            values,
            width: grid_width,
            height: grid_height,
            first_block_x,
            first_block_y,
            stride,
        }
    }

    fn sample(&self, global_x: usize, global_y: usize) -> f32 {
        let block_x = global_x / self.stride;
        let block_y = global_y / self.stride;
        let x = block_x
            .saturating_sub(self.first_block_x)
            .min(self.width - 1);
        let y = block_y
            .saturating_sub(self.first_block_y)
            .min(self.height - 1);
        self.values[y * self.width + x]
    }
}

struct BoundaryTrimapSummary {
    map: Vec<u8>,
    mixed_pixels: usize,
    maximum_radius_pixels: usize,
    fully_physical: bool,
}

/// Construct a bounded physical mixed-pixel support around depth crossings.
///
/// A crossing receives the larger of the two directional thin-lens defocus
/// diameters. Using that diameter as support radius is the conservative
/// two-times theoretical blur-radius rule recommended for compound lenses.
/// A max-plus distance transform expands all variable-radius seeds in linear
/// time without allocating a full focus-label volume.
fn build_physical_boundary_trimap(
    source_depth: &[f32],
    corrected_depth: &[f32],
    model: &PhysicalFocusModel,
    width: usize,
    height: usize,
    radius_cap: usize,
) -> BoundaryTrimapSummary {
    let mut map = vec![BOUNDARY_TRIMAP_INTERIOR; width.saturating_mul(height)];
    let Some(pixel_pitch_mm) = model.pixel_pitch_mm else {
        return BoundaryTrimapSummary {
            map,
            mixed_pixels: 0,
            maximum_radius_pixels: 0,
            fully_physical: false,
        };
    };
    if width == 0
        || height == 0
        || source_depth.len() != width * height
        || corrected_depth.len() != width * height
        || model.distances_m.len() < 2
        || radius_cap == 0
    {
        return BoundaryTrimapSummary {
            map,
            mixed_pixels: 0,
            maximum_radius_pixels: 0,
            fully_physical: false,
        };
    }

    let focus_denominator = (model.distances_m.len() - 1) as f32;
    let mut support = vec![f32::NEG_INFINITY; map.len()];
    let mut maximum_radius_pixels = 0usize;
    let mut fully_physical = true;
    let physical_radius = |left: usize, right: usize, surface: &[f32]| {
        let left_focus = surface[left].clamp(0.0, 1.0) * focus_denominator;
        let right_focus = surface[right].clamp(0.0, 1.0) * focus_denominator;
        let left_distance = model.distance_at_index(left_focus);
        let right_distance = model.distance_at_index(right_focus);
        let local_x = ((left % width) + (right % width)) as f32 * 0.5;
        let local_y = ((left / width) + (right / width)) as f32 * 0.5;
        model
            .defocus_circle_mm(left_distance, right_distance, local_x, local_y)
            .max(model.defocus_circle_mm(right_distance, left_distance, local_x, local_y))
            / pixel_pitch_mm
    };
    let mut seed_pair = |left: usize, right: usize| {
        let measured_radius = physical_radius(left, right, source_depth);
        if !measured_radius.is_finite() || measured_radius < 0.5 {
            return;
        }
        // Projection can conservatively enlarge support at an existing
        // measured crossing, but cannot create a new crossing core.
        let projected_radius = physical_radius(left, right, corrected_depth);
        let physical_radius = measured_radius.max(projected_radius);
        let unbounded_radius = physical_radius.ceil().max(1.0) as usize;
        let radius = unbounded_radius.min(radius_cap);
        fully_physical &= unbounded_radius <= radius_cap;
        maximum_radius_pixels = maximum_radius_pixels.max(radius);
        map[left] = BOUNDARY_TRIMAP_CROSSING_CORE;
        map[right] = BOUNDARY_TRIMAP_CROSSING_CORE;
        support[left] = support[left].max(radius as f32);
        support[right] = support[right].max(radius as f32);
    };

    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if x + 1 < width {
                seed_pair(index, index + 1);
            }
            if y + 1 < height {
                seed_pair(index, index + width);
                if x > 0 {
                    seed_pair(index, index + width - 1);
                }
                if x + 1 < width {
                    seed_pair(index, index + width + 1);
                }
            }
        }
    }

    let diagonal = std::f32::consts::SQRT_2;
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let mut value = support[index];
            if x > 0 {
                value = value.max(support[index - 1] - 1.0);
            }
            if y > 0 {
                value = value.max(support[index - width] - 1.0);
                if x > 0 {
                    value = value.max(support[index - width - 1] - diagonal);
                }
                if x + 1 < width {
                    value = value.max(support[index - width + 1] - diagonal);
                }
            }
            support[index] = value;
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let index = y * width + x;
            let mut value = support[index];
            if x + 1 < width {
                value = value.max(support[index + 1] - 1.0);
            }
            if y + 1 < height {
                value = value.max(support[index + width] - 1.0);
                if x > 0 {
                    value = value.max(support[index + width - 1] - diagonal);
                }
                if x + 1 < width {
                    value = value.max(support[index + width + 1] - diagonal);
                }
            }
            support[index] = value;
        }
    }

    for (state, support) in map.iter_mut().zip(support) {
        if *state == BOUNDARY_TRIMAP_INTERIOR && support >= 0.0 {
            *state = BOUNDARY_TRIMAP_PSF_SUPPORT;
        }
    }
    let mixed_pixels = map
        .iter()
        .filter(|state| **state != BOUNDARY_TRIMAP_INTERIOR)
        .count();
    tracing::info!(
        mixed_pixels,
        maximum_radius_pixels,
        fully_physical,
        "Physical mixed-pixel boundary trimap built"
    );
    BoundaryTrimapSummary {
        map,
        mixed_pixels,
        maximum_radius_pixels,
        fully_physical,
    }
}

/// Foreground-favored projection of the continuous sensor-distance surface
/// onto the aperture-valid set from Jacobs, Baek, and Levoy. In log sensor
/// distance, the one-sided constraint is a max-plus distance transform and
/// can be solved with two bounded raster passes instead of per-label dilation.
fn project_aperture_visibility(
    depth: &mut [f32],
    model: &PhysicalFocusModel,
    width: usize,
    height: usize,
) -> (usize, Vec<bool>) {
    let Some(pixel_pitch_mm) = model.pixel_pitch_mm else {
        return (0, vec![false; depth.len()]);
    };
    if width == 0 || height == 0 || depth.len() != width * height || model.distances_m.len() < 2 {
        return (0, vec![false; depth.len()]);
    }
    let aperture_radius_mm = model.conservative_aperture_radius_mm();
    // The paper recommends doubling the theoretical halo extent for real
    // compound lenses whose pupils deviate from the paraxial thin-lens model.
    let conservative_aperture_radius_mm = 2.0 * aperture_radius_mm;
    let axial_cost = pixel_pitch_mm / conservative_aperture_radius_mm;
    let diagonal_cost = axial_cost * std::f32::consts::SQRT_2;
    if !axial_cost.is_finite() || axial_cost <= 0.0 {
        return (0, vec![false; depth.len()]);
    }

    let focus_denominator = (model.distances_m.len() - 1) as f32;
    let original = depth.to_vec();
    let mut log_sensor_surface: Vec<f32> = depth
        .iter()
        .map(|value| {
            model
                .sensor_distance_at_index(value.clamp(0.0, 1.0) * focus_denominator)
                .ln()
        })
        .collect();

    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let mut value = log_sensor_surface[index];
            if x > 0 {
                value = value.max(log_sensor_surface[index - 1] - axial_cost);
            }
            if y > 0 {
                value = value.max(log_sensor_surface[index - width] - axial_cost);
                if x > 0 {
                    value = value.max(log_sensor_surface[index - width - 1] - diagonal_cost);
                }
                if x + 1 < width {
                    value = value.max(log_sensor_surface[index - width + 1] - diagonal_cost);
                }
            }
            log_sensor_surface[index] = value;
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let index = y * width + x;
            let mut value = log_sensor_surface[index];
            if x + 1 < width {
                value = value.max(log_sensor_surface[index + 1] - axial_cost);
            }
            if y + 1 < height {
                value = value.max(log_sensor_surface[index + width] - axial_cost);
                if x > 0 {
                    value = value.max(log_sensor_surface[index + width - 1] - diagonal_cost);
                }
                if x + 1 < width {
                    value = value.max(log_sensor_surface[index + width + 1] - diagonal_cost);
                }
            }
            log_sensor_surface[index] = value;
        }
    }

    let mut adjusted = 0usize;
    let mut correction_mask = Vec::with_capacity(depth.len());
    for ((output, original), log_sensor_distance) in
        depth.iter_mut().zip(original).zip(log_sensor_surface)
    {
        let corrected_index = model.index_at_sensor_distance(log_sensor_distance.exp());
        let corrected_depth = corrected_index / focus_denominator;
        let corrected = (corrected_depth - original).abs() > 1e-5;
        adjusted += usize::from(corrected);
        correction_mask.push(corrected);
        *output = corrected_depth;
    }
    tracing::info!(
        "Aperture visibility projection adjusted {} / {} pixels ({:.2}%)",
        adjusted,
        depth.len(),
        adjusted as f64 * 100.0 / depth.len() as f64
    );
    (adjusted, correction_mask)
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
    use crate::fusion_edit::{
        FusionEditDocument, FusionEditOperation, FusionEditReason, FusionEditRect,
        FusionEditSelector, FUSION_EDIT_SCHEMA,
    };
    use crate::lens_psf::{LensPsfFocusKnot, LensPsfProfile, LENS_PSF_PROFILE_SCHEMA};
    use crate::nef::parser::{SensorGeometry, SensorLevels, Z9Metadata};
    use crate::sensor_correction::{
        DefectPixel, SensorCorrectionProfile, SENSOR_CORRECTION_PROFILE_SCHEMA,
    };
    use crate::sensor_noise::{
        IsoNoiseModel, SensorNoiseModel, SensorNoiseProfile, SENSOR_NOISE_PROFILE_SCHEMA,
    };
    use crate::smart_loader::NativeFrameGroup;
    use crate::types::Rect;

    fn metadata(exposure_time: f64) -> Z9Metadata {
        Z9Metadata {
            width: 8_256,
            height: 5_504,
            bits_per_sample: 14,
            compression: 1,
            cfa_pattern: RGGB,
            camera_make: "Nikon".to_string(),
            camera_model: "Z9".to_string(),
            sensor_levels: Some(SensorLevels {
                black: 0,
                white: 16_383,
            }),
            sensor_geometry: Some(SensorGeometry {
                pixel_pitch_um: 35_900.0 / 8_256.0,
            }),
            strip_offsets: vec![],
            strip_byte_counts: vec![],
            rows_per_strip: 5_504,
            cam_mul: [2.0, 1.0, 1.5, 1.0],
            timestamp: None,
            exposure_time: Some(exposure_time),
            aperture: Some(1.0),
            iso: Some(100),
            focal_length: Some(105.0),
            focus_distance: Some(1.0),
            lens_model: Some("NIKKOR Z MC 105mm f/2.8 VR S".to_string()),
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

    fn calibrated_correction_profile(
        gain: f32,
        defects: Vec<DefectPixel>,
    ) -> SensorCorrectionProfile {
        SensorCorrectionProfile {
            schema: SENSOR_CORRECTION_PROFILE_SCHEMA.to_string(),
            camera_make: "Nikon".to_string(),
            camera_model: "Z9".to_string(),
            bits_per_sample: 14,
            sensor_width: 8_256,
            sensor_height: 5_504,
            lens_model: "NIKKOR Z MC 105mm f/2.8 VR S".to_string(),
            aperture: 1.0,
            focal_length_mm: 105.0,
            focus_distance_min_m: 0.8,
            focus_distance_max_m: 1.2,
            grid_width: 2,
            grid_height: 2,
            gain_by_cell_and_site: vec![gain; 16],
            defects,
            fit_flat_pairs: 2,
            holdout_flat_pairs: 2,
            raw_holdout_p95_relative_error: 0.2,
            corrected_holdout_p95_relative_error: 0.01,
            calibration_id: "sha256:synthetic-spatial-correction".to_string(),
        }
    }

    fn calibrated_lens_psf_profile() -> LensPsfProfile {
        LensPsfProfile {
            schema: LENS_PSF_PROFILE_SCHEMA.to_string(),
            camera_make: "Nikon".to_string(),
            camera_model: "Z9".to_string(),
            sensor_width: 8_256,
            sensor_height: 5_504,
            lens_model: "NIKKOR Z MC 105mm f/2.8 VR S".to_string(),
            nominal_focal_length_mm: 105.0,
            aperture: 1.0,
            measurement_set_digest:
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
            radius_knots: vec![0.0, 0.5, 1.0],
            focus_knots: [0.5f32, 1.0, 2.0]
                .into_iter()
                .enumerate()
                .map(|(index, focus_distance_m)| LensPsfFocusKnot {
                    focus_distance_m,
                    effective_focal_length_mm: 112.0 - index as f32 * 2.5,
                    entrance_pupil_scale: 1.20 - index as f32 * 0.05,
                    radial_psf_scale: vec![1.0, 1.08, 1.24],
                })
                .collect(),
            fit_measurements: 18,
            holdout_measurements: 18,
            ideal_holdout_p95_relative_error: 0.25,
            calibrated_holdout_p95_relative_error: 0.01,
            maximum_holdout_p95_relative_error: 0.05,
            minimum_holdout_p95_error_reduction: 0.50,
            calibration_id: "sha256:synthetic-lens-psf".to_string(),
        }
    }

    #[test]
    fn provenance_preview_preserves_sparse_critical_flags_when_downsampled() {
        let mut sources = Array2::from_elem((4, 4), 3u16);
        sources[[2, 2]] = 7;
        let mut flags = Array2::zeros((4, 4));
        flags[[0, 3]] = FUSION_FLAG_DISOCCLUDED | FUSION_FLAG_SOURCE_FALLBACK;

        let (rgb, alpha) = fusion_provenance_preview(&sources, &flags, 1).unwrap();

        assert_eq!(rgb.dim(), (1, 1, 3));
        assert_eq!(
            [rgb[[0, 0, 0]], rgb[[0, 0, 1]], rgb[[0, 0, 2]]],
            [235, 55, 210]
        );
        assert_eq!(alpha[[0, 0]], 230);
    }

    #[test]
    fn provenance_preview_exposes_sparse_frequency_source_splits() {
        let sources = Array2::from_elem((4, 4), 1u16);
        let mut detail_sources = sources.clone();
        detail_sources[[2, 2]] = 2;
        let flags = Array2::zeros((4, 4));
        let mut frequency = Array2::zeros((4, 4));
        frequency[[0, 3]] = FREQUENCY_FLAG_SEPARATED | FREQUENCY_FLAG_SPLIT_SOURCES;

        let (rgb, alpha) = fusion_provenance_preview_with_frequency(
            &sources,
            &detail_sources,
            &flags,
            &frequency,
            1,
        )
        .unwrap();

        assert_eq!(rgb.dim(), (1, 1, 3));
        assert_eq!(
            [rgb[[0, 0, 0]], rgb[[0, 0, 1]], rgb[[0, 0, 2]]],
            [55, 220, 145]
        );
        assert_eq!(alpha[[0, 0]], 180);
    }

    #[test]
    fn trimmed_focus_mean_matches_the_sorted_reference() {
        let width = 37;
        let height = 29;
        let metric = (0..width * height)
            .map(|index| {
                let x = (index % width) as f32;
                let y = (index / width) as f32;
                (0.7 * (x * 0.19).sin() + 0.3 * (y * 0.31).cos() + 1.0).max(0.0)
            })
            .collect::<Vec<_>>();
        let mut optimized = vec![0.0f32; metric.len()];
        compute_trimmed_focus_mean(&metric, &mut optimized, width, height);
        for y in 0..height {
            for x in 0..width {
                let reference = sorted_trimmed_focus_mean_at(&metric, width, height, x, y);
                let observed = optimized[y * width + x];
                assert!(
                    (observed - reference).abs() <= 2e-6 * (1.0 + reference.abs()),
                    "trimmed mean mismatch at ({x}, {y}): {observed} vs {reference}"
                );
            }
        }
    }

    #[test]
    #[should_panic(expected = "focus evidence buffers are smaller")]
    fn focus_kernel_rejects_undersized_buffers_before_simd_dispatch() {
        let mut metric = vec![0.0f32; 15];
        compute_focus_metric(&[0.0; 15], &[true; 15], &mut metric, 4, 4, true);
    }

    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    #[test]
    fn apple_neon_focus_matches_scalar_with_invalid_lanes() {
        let width = 257;
        let height = 129;
        let green = (0..width * height)
            .map(|index| {
                let x = (index % width) as f32;
                let y = (index / width) as f32;
                (0.35 + 0.2 * (x * 0.17).sin() + 0.1 * (y * 0.23).cos()).max(0.0)
            })
            .collect::<Vec<_>>();
        let valid = (0..green.len())
            .map(|index| index % 97 != 0 && index % 193 != 0)
            .collect::<Vec<_>>();
        let mut scalar = vec![0.0f32; green.len()];
        let mut neon = vec![0.0f32; green.len()];
        compute_focus_metric_scalar(&green, &valid, &mut scalar, width, height);
        // SAFETY: NEON is mandatory on AArch64 and all buffers have matching
        // validated dimensions.
        unsafe {
            compute_focus_metric_apple_neon(&green, &valid, &mut neon, width, height);
        }
        for (index, (reference, observed)) in scalar.iter().zip(&neon).enumerate() {
            assert!(
                (observed - reference).abs() <= 5e-6 * (1.0 + reference.abs()),
                "NEON focus mismatch at {index}: {observed} vs {reference}"
            );
        }
    }

    #[test]
    fn rejected_global_bracket_fails_closed() {
        let warp = FrameWarp {
            plane: PlaneTransform::identity(),
            bracket_shift_x: 0.0,
            bracket_shift_y: 0.0,
            global_accepted: false,
            reference_frame: false,
            local: None,
        };

        let coordinate = warp.source_coordinate_from_plane(12.0, 8.0);

        assert!(coordinate.disoccluded);
        assert!(!coordinate.aligned);
    }

    fn physical_model(distances_m: &[f32]) -> PhysicalFocusModel {
        let focal_length_mm = 105.0;
        PhysicalFocusModel {
            distances_m: distances_m.to_vec(),
            diopters: distances_m.iter().map(|distance| 1.0 / distance).collect(),
            sensor_distances_mm: distances_m
                .iter()
                .map(|distance| object_to_sensor_distance_mm(focal_length_mm, *distance))
                .collect(),
            focal_length_mm,
            aperture: 8.0,
            pixel_pitch_mm: Some(35.9 / 8_256.0),
            sensor_origin_x: 0.0,
            sensor_origin_y: 0.0,
            lens_psf_profile: None,
        }
    }

    #[test]
    fn nonuniform_diopter_fit_recovers_the_physical_peak() {
        let model = physical_model(&[0.25, 0.5, 1.0]);
        let target_diopter = 2.3f32;
        let score = |diopter: f32| (-(diopter - target_diopter).powi(2) / 1.7).exp();
        let index = subplane_focus_position(
            &model,
            1,
            score(model.diopters[0]),
            score(model.diopters[1]),
            score(model.diopters[2]),
        )
        .unwrap();
        assert!((index - 0.85).abs() < 1e-4, "index was {index}");
        assert!((model.distance_at_index(index) - 1.0 / target_diopter).abs() < 1e-4);
    }

    #[test]
    fn physical_focus_model_requires_complete_monotonic_metadata() {
        let pixels = vec![2048u16; 3 * 16 * 16];
        let mut metadata = vec![metadata(1.0), metadata(1.0), metadata(1.0)];
        for (frame, distance) in metadata.iter_mut().zip([0.25, 0.5, 1.0]) {
            frame.focus_distance = Some(distance);
        }
        let group = NativeFrameGroup::from_parts(
            &pixels,
            3,
            16,
            16,
            Rect::new(0.0, 0.0, 16.0, 16.0),
            metadata.clone(),
        )
        .unwrap();
        let model = physical_focus_model(&group, 3, 1, None).unwrap();
        assert_eq!(model.diopters, vec![4.0, 2.0, 1.0]);
        assert!(model.pixel_pitch_mm.is_some());

        metadata[2].focus_distance = Some(0.4);
        let nonmonotonic = NativeFrameGroup::from_parts(
            &pixels,
            3,
            16,
            16,
            Rect::new(0.0, 0.0, 16.0, 16.0),
            metadata,
        )
        .unwrap();
        assert!(physical_focus_model(&nonmonotonic, 3, 1, None).is_none());
    }

    #[test]
    fn calibrated_lens_psf_changes_physical_geometry_and_is_attributed() {
        let width = 16;
        let height = 16;
        let pixels = (0..3 * width * height)
            .map(|index| 2_000 + (index % (width * height)) as u16)
            .collect::<Vec<_>>();
        let mut frame_metadata = vec![metadata(1.0), metadata(1.0), metadata(1.0)];
        for (frame, distance) in frame_metadata.iter_mut().zip([0.5, 1.0, 2.0]) {
            frame.focus_distance = Some(distance);
        }
        let group = NativeFrameGroup::from_parts(
            &pixels,
            3,
            width,
            height,
            Rect::new(8_000.0, 5_200.0, width as f64, height as f64),
            frame_metadata,
        )
        .unwrap();
        let profile = calibrated_lens_psf_profile();
        let ideal = physical_focus_model(&group, 3, 1, None).unwrap();
        let calibrated = physical_focus_model(&group, 3, 1, Some(&profile)).unwrap();
        let ideal_blur = ideal.defocus_circle_mm(1.0, 1.2, 8.0, 8.0);
        let calibrated_blur = calibrated.defocus_circle_mm(1.0, 1.2, 8.0, 8.0);
        assert!(calibrated_blur > ideal_blur * 1.25);
        let center_blur = profile.defocus_circle_mm(1.0, 1.2, 0.0);
        let corner_blur = profile.defocus_circle_mm(1.0, 1.2, 1.0);
        assert!(corner_blur > center_blur * 1.20);
        assert!(
            (calibrated.sensor_distance_at_index(1.0) - ideal.sensor_distance_at_index(1.0)).abs()
                > 0.1
        );

        let base = NativeFusionConfig {
            lens_psf_profile: Some(profile),
            regularize_depth: false,
            tile_size: 16,
            ..Default::default()
        };
        let result = fuse_native_group(&group, &meta(3, 1), &base).unwrap();
        let untiled = fuse_native_group(
            &group,
            &meta(3, 1),
            &NativeFusionConfig {
                tile_size: 128,
                ..base
            },
        )
        .unwrap();
        assert!(result.lens_psf_calibrated);
        assert_eq!(
            result.lens_psf_calibration_id.as_deref(),
            Some("sha256:synthetic-lens-psf")
        );
        assert_eq!(result.bayer, untiled.bayer);
        assert_eq!(result.depth, untiled.depth);
        assert_eq!(result.boundary_trimap, untiled.boundary_trimap);
    }

    #[test]
    fn lens_psf_identity_mismatch_fails_before_fusion() {
        let pixels = vec![2048u16; 3 * 16 * 16];
        let mut frame_metadata = vec![metadata(1.0), metadata(1.0), metadata(1.0)];
        for (frame, distance) in frame_metadata.iter_mut().zip([0.5, 1.0, 2.0]) {
            frame.focus_distance = Some(distance);
        }
        let group = NativeFrameGroup::from_parts(
            &pixels,
            3,
            16,
            16,
            Rect::new(0.0, 0.0, 16.0, 16.0),
            frame_metadata,
        )
        .unwrap();
        let mut profile = calibrated_lens_psf_profile();
        profile.camera_model = "Different Camera".to_string();
        let error = fuse_native_group(
            &group,
            &meta(3, 1),
            &NativeFusionConfig {
                lens_psf_profile: Some(profile),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not match frame"));
    }

    #[test]
    fn wider_apertures_produce_larger_defocus_circles() {
        let wide = defocus_circle_mm(105.0, 2.8, 0.5, 0.6);
        let stopped_down = defocus_circle_mm(105.0, 11.0, 0.5, 0.6);
        assert!(wide > stopped_down * 3.9);
    }

    #[test]
    fn physical_boundary_trimap_scales_with_aperture_and_reports_truncation() {
        let width = 64;
        let height = 16;
        let mut depth = vec![0.0f32; width * height];
        for row in depth.chunks_mut(width) {
            row[width / 2..].fill(1.0);
        }
        let mut wide = physical_model(&[2.0, 2.1]);
        wide.aperture = 2.0;
        let mut stopped_down = wide.clone();
        stopped_down.aperture = 16.0;
        let wide_trimap = build_physical_boundary_trimap(&depth, &depth, &wide, width, height, 64);
        let stopped_trimap =
            build_physical_boundary_trimap(&depth, &depth, &stopped_down, width, height, 64);

        assert!(wide_trimap.fully_physical);
        assert!(stopped_trimap.fully_physical);
        assert!(wide_trimap.maximum_radius_pixels > stopped_trimap.maximum_radius_pixels);
        assert!(wide_trimap.mixed_pixels > stopped_trimap.mixed_pixels);
        assert_eq!(
            wide_trimap.map[height / 2 * width + width / 2 - 1],
            BOUNDARY_TRIMAP_CROSSING_CORE
        );
        assert_eq!(
            wide_trimap.map[height / 2 * width + width / 2],
            BOUNDARY_TRIMAP_CROSSING_CORE
        );
        assert!(wide_trimap.map.contains(&BOUNDARY_TRIMAP_PSF_SUPPORT));

        let near_model = physical_model(&[0.5, 0.8]);
        let truncated =
            build_physical_boundary_trimap(&depth, &depth, &near_model, width, height, 8);
        assert_eq!(truncated.maximum_radius_pixels, 8);
        assert!(!truncated.fully_physical);
    }

    #[test]
    fn aperture_projection_enforces_the_sensor_surface_slope_bound() {
        let model = physical_model(&[0.5, 0.8]);
        let width = 1024;
        let mut depth = vec![1.0f32; width];
        depth[..width / 2].fill(0.0);
        let (adjusted, correction_mask) = project_aperture_visibility(&mut depth, &model, width, 1);
        assert!(adjusted > 0);
        assert_eq!(
            correction_mask
                .iter()
                .filter(|corrected| **corrected)
                .count(),
            adjusted
        );
        assert_eq!(depth[width / 4], 0.0, "foreground anchors moved");
        assert!(
            depth[width - 1] > 0.99,
            "distant background was needlessly changed"
        );

        let pitch = model.pixel_pitch_mm.unwrap();
        let conservative_aperture_radius = 2.0 * model.focal_length_mm / (2.0 * model.aperture);
        let maximum_log_step = pitch / conservative_aperture_radius;
        let sensor_surface: Vec<f32> = depth
            .iter()
            .map(|value| model.sensor_distance_at_index(*value).ln())
            .collect();
        let observed = sensor_surface
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            observed <= maximum_log_step * 1.001,
            "visibility slope {observed} exceeded {maximum_log_step}"
        );
    }

    #[test]
    fn aperture_projection_preserves_a_one_pixel_foreground_structure() {
        let model = physical_model(&[0.5, 0.8]);
        let width = 1024;
        let center = width / 2;
        let mut depth = vec![1.0f32; width];
        depth[center] = 0.0;
        let (adjusted, correction_mask) = project_aperture_visibility(&mut depth, &model, width, 1);
        assert!(adjusted > 0);
        assert!(!correction_mask[center]);
        assert_eq!(depth[center], 0.0);
        assert!(depth[center - 1] < 1.0);
        assert!(depth[center + 1] < 1.0);
    }

    #[test]
    fn aperture_projection_enforces_two_dimensional_chamfer_bounds() {
        let model = physical_model(&[0.5, 0.8]);
        let width = 96;
        let height = 80;
        let center = (height / 2) * width + width / 2;
        let mut depth = vec![1.0f32; width * height];
        depth[center] = 0.0;
        project_aperture_visibility(&mut depth, &model, width, height);

        let pitch = model.pixel_pitch_mm.unwrap();
        let conservative_aperture_radius = 2.0 * model.focal_length_mm / (2.0 * model.aperture);
        let axial_bound = pitch / conservative_aperture_radius;
        let diagonal_bound = axial_bound * std::f32::consts::SQRT_2;
        let sensor_surface: Vec<f32> = depth
            .iter()
            .map(|value| model.sensor_distance_at_index(*value).ln())
            .collect();
        for y in 0..height {
            for x in 0..width {
                let index = y * width + x;
                if x + 1 < width {
                    assert!(
                        (sensor_surface[index + 1] - sensor_surface[index]).abs()
                            <= axial_bound * 1.001
                    );
                }
                if y + 1 < height {
                    assert!(
                        (sensor_surface[index + width] - sensor_surface[index]).abs()
                            <= axial_bound * 1.001
                    );
                    if x + 1 < width {
                        assert!(
                            (sensor_surface[index + width + 1] - sensor_surface[index]).abs()
                                <= diagonal_bound * 1.001
                        );
                    }
                }
            }
        }
    }

    fn alignment_texture(width: usize, height: usize) -> Array2<f64> {
        Array2::from_shape_fn((height, width), |(y, x)| {
            let hash = ((x * 73 + y * 151 + x * y * 17) % 251) as f64 / 251.0;
            hash + (x as f64 * 0.17).sin() * 0.4 + (y as f64 * 0.11).cos() * 0.3
        })
    }

    #[test]
    fn selective_local_alignment_recovers_bidirectionally_consistent_motion() {
        let width = 96;
        let height = 80;
        let reference = alignment_texture(width, height);
        let frame = Array2::from_shape_fn((height, width), |(y, x)| {
            reference[[y, x.saturating_sub(2)]]
        });
        let config = NativeFusionConfig {
            local_alignment_cell_size: 16,
            local_alignment_search_radius: 4,
            local_alignment_trigger_score: 0.999,
            minimum_local_alignment_score: 0.4,
            ..NativeFusionConfig::default()
        };
        let field = estimate_local_motion_field(&reference, &frame, 0.0, 0.0, 1.0, &config);
        let sample = field.sample(48.0, 40.0);
        assert!(sample.aligned);
        assert!(!sample.disoccluded);
        assert!(
            (sample.shift_x + 2.0).abs() < 0.35,
            "local shift was {}",
            sample.shift_x
        );
        assert!(sample.shift_y.abs() < 0.35);
    }

    #[test]
    fn disocclusion_uses_traceable_reference_fallback() {
        let width = 64;
        let height = 64;
        let white = 16_383.0f32;
        let exposures = [1.0f64, 2.0, 4.0];
        let mut pixels = Vec::with_capacity(width * height * exposures.len());
        for (frame_index, exposure) in exposures.iter().enumerate() {
            for y in 0..height {
                for x in 0..width {
                    let texture =
                        (((x * 37 + y * 67 + x * y * 3) % 97) as f32 / 97.0) * 0.08 + 0.06;
                    let moving_region = (22..42).contains(&x) && (22..42).contains(&y);
                    let radiance = if frame_index != 1 && moving_region {
                        if frame_index == 0 {
                            0.42
                        } else {
                            0.01
                        }
                    } else {
                        texture
                    };
                    pixels.push((radiance * *exposure as f32 * white).min(white).round() as u16);
                }
            }
        }
        let group = NativeFrameGroup::from_parts(
            &pixels,
            exposures.len(),
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            exposures
                .iter()
                .map(|exposure| metadata(*exposure))
                .collect(),
        )
        .unwrap();
        let result = fuse_native_group(
            &group,
            &meta(1, exposures.len()),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(white),
                tile_size: 32,
                local_alignment_cell_size: 8,
                local_alignment_search_radius: 3,
                local_alignment_trigger_score: 0.98,
                minimum_local_alignment_score: 0.45,
                regularize_depth: false,
                ..NativeFusionConfig::default()
            },
        )
        .unwrap();
        let x = 33;
        let y = 32;
        let flags = result.fusion_flags[[y, x]];
        assert_ne!(flags & FUSION_FLAG_DISOCCLUDED, 0);
        assert_ne!(flags & FUSION_FLAG_SOURCE_FALLBACK, 0);
        assert_eq!(result.source_map[[y, x]], 1);
        assert!(result
            .frame_alignments
            .iter()
            .filter(|summary| !summary.reference_frame)
            .all(|summary| !summary.global_accepted || summary.disoccluded_cells > 0));
    }

    #[test]
    fn coarse_focus_regions_are_globally_aligned_and_compact() {
        let full_width = 32;
        let full_height = 24;
        let stride = 4;
        let metric: Vec<f32> = (0..full_height)
            .flat_map(|y| (0..full_width).map(move |x| (x / stride + 10 * (y / stride)) as f32))
            .collect();
        let valid = vec![true; metric.len()];
        let full = CoarseFocusGrid::build(&metric, &valid, full_width, full_height, 0, 0, stride);
        assert_eq!(
            full.values.len(),
            (full_width / stride) * (full_height / stride)
        );

        let origin_x = 5;
        let origin_y = 3;
        let crop_width = 20;
        let crop_height = 17;
        let mut cropped_metric = Vec::with_capacity(crop_width * crop_height);
        for y in origin_y..origin_y + crop_height {
            cropped_metric.extend_from_slice(
                &metric[y * full_width + origin_x..y * full_width + origin_x + crop_width],
            );
        }
        let cropped = CoarseFocusGrid::build(
            &cropped_metric,
            &vec![true; cropped_metric.len()],
            crop_width,
            crop_height,
            origin_x,
            origin_y,
            stride,
        );
        for y in 4..20 {
            for x in 8..24 {
                assert_eq!(cropped.sample(x, y), full.sample(x, y));
            }
        }
    }

    #[test]
    fn glare_exclusion_reduces_false_focus_energy_without_changing_radiance() {
        let width = 96;
        let height = 80;
        let center_x = width / 2;
        let center_y = height / 2;
        let radius = 20;
        let mut radiance = vec![0.0f32; width * height];
        let valid = vec![true; width * height];
        let mut flags = vec![0u8; width * height];
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - center_x as f32;
                let dy = y as f32 - center_y as f32;
                let distance2 = dx * dx + dy * dy;
                let bloom = 0.72 * (-distance2 / 110.0).exp();
                let index = y * width + x;
                radiance[index] = 0.08 + bloom;
                if distance2 <= 9.0 {
                    radiance[index] = 1.0;
                    flags[index] |= FUSION_FLAG_CENSORED;
                }
            }
        }
        let original_radiance = radiance.clone();
        let mut focus_metric = vec![0.0f32; radiance.len()];
        compute_focus_metric(&radiance, &valid, &mut focus_metric, width, height, true);
        let mut glare = vec![0.0f32; radiance.len()];
        detect_glare_likelihood(&radiance, &valid, &flags, &mut glare, width, height, radius);
        let protected_metric = focus_metric
            .iter()
            .zip(&glare)
            .map(|(metric, glare)| metric * (1.0 - glare))
            .collect::<Vec<_>>();

        let mut unprotected_energy = 0.0f64;
        let mut protected_energy = 0.0f64;
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let distance = ((x as f32 - center_x as f32).powi(2)
                    + (y as f32 - center_y as f32).powi(2))
                .sqrt();
                if (4.0..=radius as f32).contains(&distance) {
                    let index = y * width + x;
                    unprotected_energy += f64::from(focus_metric[index]);
                    protected_energy += f64::from(protected_metric[index]);
                }
            }
        }
        assert_eq!(radiance, original_radiance);
        assert_eq!(glare[center_y * width + center_x], 1.0);
        eprintln!(
            "glare annulus focus energy: {:.3} -> {:.3} ({:.1}% retained)",
            unprotected_energy,
            protected_energy,
            100.0 * protected_energy / unprotected_energy.max(f64::MIN_POSITIVE)
        );
        assert!(
            protected_energy <= unprotected_energy * 0.72,
            "glare focus energy was not materially reduced: {unprotected_energy} -> {protected_energy}"
        );
        assert_eq!(glare[width + 1], 0.0);
    }

    #[test]
    fn glare_diagnostics_are_physical_and_tile_invariant() {
        let width = 72;
        let height = 64;
        let white = 16_383.0f32;
        let center_x = width / 2;
        let center_y = height / 2;
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let distance2 =
                    (x as f32 - center_x as f32).powi(2) + (y as f32 - center_y as f32).powi(2);
                let signal = if distance2 <= 9.0 {
                    1.0
                } else {
                    0.08 + 0.70 * (-distance2 / 100.0).exp()
                };
                pixels.push((signal * white).round().clamp(0.0, white) as u16);
            }
        }
        let mut frame_metadata = metadata(1.0);
        frame_metadata.width = width as u32;
        frame_metadata.height = height as u32;
        frame_metadata.cam_mul = [1.0; 4];
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
            white_level: Some(white),
            regularize_depth: false,
            ..NativeFusionConfig::default()
        };
        let tiled = fuse_native_group(
            &group,
            &meta(1, 1),
            &NativeFusionConfig {
                tile_size: 16,
                ..base.clone()
            },
        )
        .unwrap();
        let bounded = fuse_native_group(
            &group,
            &meta(1, 1),
            &NativeFusionConfig {
                glare_spread_um: 2_000.0,
                glare_fallback_radius_pixels: 256,
                ..base.clone()
            },
        )
        .unwrap();
        let untiled = fuse_native_group(
            &group,
            &meta(1, 1),
            &NativeFusionConfig {
                tile_size: 128,
                ..base
            },
        )
        .unwrap();

        assert!(tiled.glare_physical_scale);
        assert_eq!(tiled.glare_radius_pixels, 19);
        assert!(tiled.glare_affected_pixels > 0);
        assert_eq!(tiled.glare_map[[center_y, center_x]], 255);
        assert_eq!(tiled.glare_map, untiled.glare_map);
        assert_eq!(tiled.bayer, untiled.bayer);
        assert_eq!(bounded.glare_radius_pixels, 256);
        assert!(!bounded.glare_physical_scale);

        let glare_rect = FusionEditRect {
            x: (center_x + 4) as u32,
            y: center_y as u32,
            width: 24,
            height: 1,
        };
        let glare_edit = FusionEditDocument {
            schema: FUSION_EDIT_SCHEMA.to_string(),
            capture_group_id: "e".repeat(64),
            base_report_sha256: "f".repeat(64),
            width: width as u32,
            height: height as u32,
            crop_origin_x: 0,
            crop_origin_y: 0,
            frame_count: 1,
            operations: vec![FusionEditOperation {
                id: "physical-glare".to_string(),
                rect: glare_rect,
                source_frame: 0,
                reason: FusionEditReason::Glare,
                selector: FusionEditSelector::GlareAffected,
                note: None,
            }],
        };
        let constrained = fuse_native_group_with_id(
            &group,
            &meta(1, 1),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(white),
                regularize_depth: false,
                fusion_edit: Some(glare_edit.clone()),
                ..NativeFusionConfig::default()
            },
            Some(&"e".repeat(64)),
        )
        .unwrap();
        let constrained_map = constrained.fusion_edit_map.as_ref().unwrap();
        let expected_pixels = (glare_rect.x as usize..(glare_rect.x + glare_rect.width) as usize)
            .filter(|x| tiled.glare_map[[center_y, *x]] != 0)
            .count();
        assert!(expected_pixels > 0);
        assert!(expected_pixels < glare_rect.width as usize);
        assert_eq!(constrained.operator_override_pixels, expected_pixels);
        for x in 0..width {
            let selected = x >= glare_rect.x as usize
                && x < (glare_rect.x + glare_rect.width) as usize
                && tiled.glare_map[[center_y, x]] != 0;
            assert_eq!(
                constrained_map[[center_y, x]],
                if selected {
                    FUSION_EDIT_GLARE_CONSTRAINED
                } else {
                    0
                }
            );
        }

        let fallback_error = fuse_native_group_with_id(
            &group,
            &meta(1, 1),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(white),
                regularize_depth: false,
                glare_spread_um: 2_000.0,
                glare_fallback_radius_pixels: 256,
                fusion_edit: Some(glare_edit),
                ..NativeFusionConfig::default()
            },
            Some(&"e".repeat(64)),
        )
        .unwrap_err();
        assert!(fallback_error
            .to_string()
            .contains("pixel fallback is not physically qualified"));

        let empty_edit = FusionEditDocument {
            schema: FUSION_EDIT_SCHEMA.to_string(),
            capture_group_id: "e".repeat(64),
            base_report_sha256: "f".repeat(64),
            width: width as u32,
            height: height as u32,
            crop_origin_x: 0,
            crop_origin_y: 0,
            frame_count: 1,
            operations: vec![FusionEditOperation {
                id: "no-glare-evidence".to_string(),
                rect: FusionEditRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                source_frame: 0,
                reason: FusionEditReason::Glare,
                selector: FusionEditSelector::GlareAffected,
                note: None,
            }],
        };
        empty_edit.validate().unwrap();
        let empty_error = fuse_native_group_with_id(
            &group,
            &meta(1, 1),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(white),
                regularize_depth: false,
                fusion_edit: Some(empty_edit),
                ..NativeFusionConfig::default()
            },
            Some(&"e".repeat(64)),
        )
        .unwrap_err();
        assert!(empty_error
            .to_string()
            .contains("matched no recomputed physical evidence"));
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
    fn hdr_deghost_rejects_a_moving_bracket_outlier() {
        let width = 16;
        let height = 16;
        let white = 16_383.0;
        let exposures = [1.0, 2.0, 4.0];
        let scene = 0.1f32;
        let signals = [scene, scene * 2.0, 0.92];
        let mut pixels = Vec::new();
        for signal in signals {
            let raw = (signal * white).round() as u16;
            pixels.extend(std::iter::repeat(raw).take(width * height));
        }
        let metadata = exposures.iter().map(|&value| metadata(value)).collect();
        let group = NativeFrameGroup::from_parts(
            &pixels,
            exposures.len(),
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            metadata,
        )
        .unwrap();
        let base = NativeFusionConfig {
            black_level: Some(0.0),
            white_level: Some(white),
            tile_size: 16,
            regularize_depth: false,
            ..Default::default()
        };
        let unprotected = fuse_native_group(
            &group,
            &meta(1, exposures.len()),
            &NativeFusionConfig {
                deghost_strength: 0.0,
                ..base.clone()
            },
        )
        .unwrap();
        let protected = fuse_native_group(&group, &meta(1, exposures.len()), &base).unwrap();
        let unprotected_error = (unprotected.bayer[[8, 9, 0]] - scene).abs();
        let protected_error = (protected.bayer[[8, 9, 0]] - scene).abs();
        assert!(
            protected_error < 2e-3,
            "deghosted radiance error was {protected_error}"
        );
        assert!(
            protected_error * 5.0 < unprotected_error,
            "deghosting did not materially improve {unprotected_error} -> {protected_error}"
        );
    }

    #[test]
    fn frequency_deghost_is_an_exact_static_fast_path_across_tiles() {
        let width = 48;
        let height = 24;
        let white = 16_383.0;
        let exposures = [1.0f64, 1.5, 2.0];
        let mut pixels = Vec::with_capacity(width * height * exposures.len());
        for exposure in exposures {
            for y in 0..height {
                for x in 0..width {
                    let radiance =
                        0.08 + ((x * 17 + y * 31 + x * y * 3) % 101) as f32 / 101.0 * 0.18;
                    pixels.push((radiance * exposure as f32 * white).round() as u16);
                }
            }
        }
        let group = NativeFrameGroup::from_parts(
            &pixels,
            exposures.len(),
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            exposures
                .iter()
                .map(|exposure| metadata(*exposure))
                .collect(),
        )
        .unwrap();
        let base = NativeFusionConfig {
            black_level: Some(0.0),
            white_level: Some(white),
            tile_size: 16,
            regularize_depth: false,
            selective_local_alignment: false,
            ..NativeFusionConfig::default()
        };
        let bypass = fuse_native_group(
            &group,
            &meta(1, exposures.len()),
            &NativeFusionConfig {
                frequency_separated_deghosting: false,
                ..base.clone()
            },
        )
        .unwrap();
        let protected = fuse_native_group(&group, &meta(1, exposures.len()), &base).unwrap();
        let wide_tile = fuse_native_group(
            &group,
            &meta(1, exposures.len()),
            &NativeFusionConfig {
                tile_size: 64,
                ..base
            },
        )
        .unwrap();

        assert_eq!(protected.bayer, bypass.bayer);
        assert_eq!(protected.bayer, wide_tile.bayer);
        assert!(protected.frequency_flags.iter().all(|flags| *flags == 0));
        assert_eq!(protected.frequency_flags, wide_tile.frequency_flags);
        assert_eq!(protected.detail_source_map, protected.source_map);
        assert_eq!(protected.detail_source_map, wide_tile.detail_source_map);
        assert_eq!(protected.frequency_separated_pixels, 0);
    }

    #[test]
    fn frequency_deghost_separates_illumination_and_detail_without_invention() {
        let width = 48;
        let height = 24;
        let white = 16_383.0;
        let exposures = [1.0f64, 1.5, 2.0];
        let mut truth = vec![0.0f32; width * height];
        let mut pixels = Vec::with_capacity(width * height * exposures.len());
        for (frame_index, exposure) in exposures.iter().enumerate() {
            for y in 0..height {
                for x in 0..width {
                    let detail = if ((x / 2 + y / 2) & 1) == 0 {
                        0.05
                    } else {
                        -0.05
                    };
                    let ground_truth = 0.2 + detail;
                    truth[y * width + x] = ground_truth;
                    let radiance = match frame_index {
                        0 => ground_truth,
                        1 => 0.2,
                        _ => 0.3 + detail,
                    };
                    pixels.push((radiance * *exposure as f32 * white).round() as u16);
                }
            }
        }
        let group = NativeFrameGroup::from_parts(
            &pixels,
            exposures.len(),
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            exposures
                .iter()
                .map(|exposure| metadata(*exposure))
                .collect(),
        )
        .unwrap();
        let base = NativeFusionConfig {
            black_level: Some(0.0),
            white_level: Some(white),
            tile_size: 16,
            regularize_depth: false,
            selective_local_alignment: false,
            frequency_separation_trigger_sigma: 4.0,
            ..NativeFusionConfig::default()
        };
        let single_band = fuse_native_group(
            &group,
            &meta(1, exposures.len()),
            &NativeFusionConfig {
                frequency_separated_deghosting: false,
                ..base.clone()
            },
        )
        .unwrap();
        let separated = fuse_native_group(&group, &meta(1, exposures.len()), &base).unwrap();
        let wide_tile = fuse_native_group(
            &group,
            &meta(1, exposures.len()),
            &NativeFusionConfig {
                tile_size: 64,
                ..base
            },
        )
        .unwrap();
        let error = |result: &NativeFusionResult| {
            let mut sum = 0.0f64;
            let mut count = 0usize;
            for y in 4..height - 4 {
                for x in 4..width - 4 {
                    if (x ^ y) & 1 == 1 {
                        sum += f64::from((result.bayer[[y, x, 0]] - truth[y * width + x]).abs());
                        count += 1;
                    }
                }
            }
            sum / count as f64
        };
        let single_band_error = error(&single_band);
        let separated_error = error(&separated);
        eprintln!(
            "frequency-separated dynamic HDR error: {single_band_error:.8} -> {separated_error:.8}"
        );
        assert!(
            separated_error * 2.0 < single_band_error,
            "frequency separation did not halve dynamic error"
        );
        assert!(
            separated_error < 0.003,
            "frequency-separated radiance error was {separated_error}"
        );
        assert!(separated.frequency_separated_pixels > width * height / 2);
        assert!(separated
            .frequency_flags
            .iter()
            .any(|flags| *flags & FREQUENCY_FLAG_SPLIT_SOURCES != 0));
        assert!(separated
            .detail_source_map
            .iter()
            .all(|source| *source != u16::MAX));
        for y in 0..height {
            for x in 0..width {
                let mut measured = [0.0f32; 3];
                let site = ((y & 1) << 1) | (x & 1);
                let white_balance = [2.0, 1.0, 1.0, 1.5][site];
                for (frame, exposure) in exposures.iter().enumerate() {
                    measured[frame] = pixels[frame * width * height + y * width + x] as f32
                        / white
                        / *exposure as f32
                        * white_balance;
                }
                let output = separated.bayer[[y, x, 0]];
                assert!(
                    output >= measured.into_iter().fold(f32::INFINITY, f32::min) - 1e-6
                        && output <= measured.into_iter().fold(f32::NEG_INFINITY, f32::max) + 1e-6,
                    "frequency recombination escaped measured envelope at ({x}, {y})"
                );
            }
        }
        assert_eq!(separated.bayer, wide_tile.bayer);
        assert_eq!(separated.detail_source_map, wide_tile.detail_source_map);
        assert_eq!(separated.frequency_flags, wide_tile.frequency_flags);
    }

    #[test]
    fn depth_refusion_changes_the_bayer_output() {
        let width = 16;
        let height = 16;
        let white = 16_383.0;
        let scenes = [0.1f32, 0.4f32];
        let mut pixels = Vec::new();
        for scene in scenes {
            let raw = (scene * white).round() as u16;
            pixels.extend(std::iter::repeat(raw).take(width * height));
        }
        let group = NativeFrameGroup::from_parts(
            &pixels,
            scenes.len(),
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            vec![metadata(1.0), metadata(1.0)],
        )
        .unwrap();
        let config = NativeFusionConfig {
            black_level: Some(0.0),
            white_level: Some(white),
            regularize_depth: true,
            ..Default::default()
        };
        let calibrations = build_calibrations(&group, &config).unwrap();
        let mut bayer = vec![scene_site_value(scenes[0], 1); width * height];
        let mut uncertainty = vec![0.0; width * height];
        let mut source_map = vec![0; width * height];
        let mut detail_source_map = vec![0; width * height];
        let mut fusion_flags = vec![0; width * height];
        let mut frequency_flags = vec![0; width * height];
        let mut sensor_correction_map = vec![0; width * height];
        let source_depth = vec![0.0; width * height];
        let mut regularized_depth = source_depth.clone();
        regularized_depth[8 * width + 9] = 1.0;
        let frame_warps = vec![
            FrameWarp::identity(PlaneTransform::identity(), true),
            FrameWarp::identity(PlaneTransform::identity(), true),
        ];
        refuse_regularized_depth(
            &group,
            &calibrations,
            &frame_warps,
            2,
            1,
            1.0,
            &config,
            &source_depth,
            &regularized_depth,
            &vec![BOUNDARY_TRIMAP_INTERIOR; width * height],
            &mut bayer,
            &mut uncertainty,
            &mut source_map,
            &mut detail_source_map,
            &mut fusion_flags,
            &mut frequency_flags,
            &mut sensor_correction_map,
        )
        .unwrap();
        assert!((bayer[8 * width + 8] - scenes[0]).abs() < 1e-6);
        assert!((bayer[8 * width + 9] - scenes[1]).abs() < 2e-3);
    }

    #[test]
    fn mixed_boundary_anchors_one_source_and_reduces_halo_energy() {
        let width = 64;
        let height = 16;
        let white = 16_383.0;
        let scenes = [0.12f32, 0.48f32];
        let mut pixels = Vec::with_capacity(2 * width * height);
        for scene in scenes {
            let raw = (scene * white).round() as u16;
            pixels.extend(std::iter::repeat(raw).take(width * height));
        }
        let group = NativeFrameGroup::from_parts(
            &pixels,
            scenes.len(),
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            vec![metadata(1.0), metadata(1.0)],
        )
        .unwrap();
        let config = NativeFusionConfig {
            black_level: Some(0.0),
            white_level: Some(white),
            regularize_depth: false,
            ..Default::default()
        };
        let calibrations = build_calibrations(&group, &config).unwrap();
        let frame_warps = vec![
            FrameWarp::identity(PlaneTransform::identity(), true),
            FrameWarp::identity(PlaneTransform::identity(), true),
        ];
        let mut depth = vec![0.0f32; width * height];
        for row in depth.chunks_mut(width) {
            row[width / 2..].fill(1.0);
        }
        let model = physical_model(&[2.0, 2.1]);
        let trimap = build_physical_boundary_trimap(&depth, &depth, &model, width, height, 64);
        assert!(trimap.mixed_pixels > 0);

        let truth = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    let scene = scenes[usize::from(x >= width / 2)];
                    scene_site_value(scene, ((y & 1) << 1) | (x & 1))
                })
            })
            .collect::<Vec<_>>();
        let mut halo = truth.clone();
        for y in 0..height {
            for x in width / 2 - 3..width / 2 + 3 {
                let site = ((y & 1) << 1) | (x & 1);
                halo[y * width + x] =
                    0.5 * (scene_site_value(scenes[0], site) + scene_site_value(scenes[1], site));
            }
        }
        let mut unprotected = halo.clone();
        let mut protected = halo;
        let mut unprotected_uncertainty = vec![0.0; width * height];
        let mut protected_uncertainty = vec![0.0; width * height];
        let mut unprotected_source = vec![u16::MAX; width * height];
        let mut protected_source = vec![u16::MAX; width * height];
        let mut unprotected_detail_source = vec![u16::MAX; width * height];
        let mut protected_detail_source = vec![u16::MAX; width * height];
        let mut unprotected_flags = vec![0; width * height];
        let mut protected_flags = vec![0; width * height];
        let mut unprotected_frequency = vec![0; width * height];
        let mut protected_frequency = vec![0; width * height];
        let mut unprotected_correction = vec![0; width * height];
        let mut protected_correction = vec![0; width * height];
        let empty_trimap = vec![BOUNDARY_TRIMAP_INTERIOR; width * height];
        let (_, unprotected_fallbacks) = refuse_regularized_depth(
            &group,
            &calibrations,
            &frame_warps,
            2,
            1,
            1.0,
            &config,
            &depth,
            &depth,
            &empty_trimap,
            &mut unprotected,
            &mut unprotected_uncertainty,
            &mut unprotected_source,
            &mut unprotected_detail_source,
            &mut unprotected_flags,
            &mut unprotected_frequency,
            &mut unprotected_correction,
        )
        .unwrap();
        let (_, protected_fallbacks) = refuse_regularized_depth(
            &group,
            &calibrations,
            &frame_warps,
            2,
            1,
            1.0,
            &config,
            &depth,
            &depth,
            &trimap.map,
            &mut protected,
            &mut protected_uncertainty,
            &mut protected_source,
            &mut protected_detail_source,
            &mut protected_flags,
            &mut protected_frequency,
            &mut protected_correction,
        )
        .unwrap();

        let halo_energy = |values: &[f32]| {
            values
                .iter()
                .zip(&truth)
                .zip(&trimap.map)
                .filter(|(_, state)| **state != BOUNDARY_TRIMAP_INTERIOR)
                .map(|((value, truth), _)| f64::from((value - truth).abs()))
                .sum::<f64>()
        };
        let unprotected_energy = halo_energy(&unprotected);
        let protected_energy = halo_energy(&protected);
        eprintln!("mixed-boundary halo energy: {unprotected_energy:.6} -> {protected_energy:.6}");
        assert_eq!(unprotected_fallbacks, 0);
        assert_eq!(protected_fallbacks, trimap.mixed_pixels);
        assert!(
            protected_energy <= unprotected_energy * 0.5,
            "physical boundary fallback missed the 50% halo gate"
        );
        assert!(protected_flags
            .iter()
            .zip(&trimap.map)
            .filter(|(_, state)| **state != BOUNDARY_TRIMAP_INTERIOR)
            .all(|(flags, _)| *flags & FUSION_FLAG_SOURCE_FALLBACK != 0));
        assert!(protected_source
            .iter()
            .zip(&trimap.map)
            .filter(|(_, state)| **state != BOUNDARY_TRIMAP_INTERIOR)
            .all(|(source, _)| *source != u16::MAX));
    }

    fn scene_site_value(scene: f32, site: usize) -> f32 {
        scene * [2.0, 1.0, 1.0, 1.5][site]
    }

    #[test]
    fn synthetic_focus_stack_has_a_quality_floor() {
        let width = 128;
        let height = 96;
        let white = 16_383.0;
        let mut sharp = vec![0.0f32; width * height];
        for y in 0..height {
            for x in 0..width {
                let checker = if (x / 3 + y / 3) & 1 == 0 {
                    0.16
                } else {
                    -0.16
                };
                let wave = 0.08 * ((x as f32 * 0.31).sin() + (y as f32 * 0.23).cos());
                sharp[y * width + x] = (0.42 + checker + wave).clamp(0.04, 0.86);
            }
        }
        let blurred = box_blur(&sharp, width, height, 4);
        let mut pixels = Vec::with_capacity(2 * width * height);
        for focus in 0..2 {
            for y in 0..height {
                for x in 0..width {
                    let in_focus = usize::from(x >= width / 2) == focus;
                    let value = if in_focus {
                        sharp[y * width + x]
                    } else {
                        blurred[y * width + x]
                    };
                    pixels.push((value * white).round() as u16);
                }
            }
        }
        let mut frame_metadata = vec![metadata(1.0), metadata(1.0)];
        for metadata in &mut frame_metadata {
            metadata.width = width as u32;
            metadata.height = height as u32;
            metadata.cam_mul = [1.0; 4];
        }
        let mut physical_metadata = frame_metadata.clone();
        for (metadata, distance) in physical_metadata.iter_mut().zip([0.5, 0.8]) {
            metadata.focus_distance = Some(distance);
            metadata.aperture = Some(8.0);
        }
        let group = NativeFrameGroup::from_parts(
            &pixels,
            2,
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            frame_metadata,
        )
        .unwrap();
        let result = fuse_native_group(
            &group,
            &meta(2, 1),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(white),
                tile_size: 32,
                minimum_alignment_score: 2.0,
                ..Default::default()
            },
        )
        .unwrap();
        let scalar_result = fuse_native_group(
            &group,
            &meta(2, 1),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(white),
                tile_size: 32,
                minimum_alignment_score: 2.0,
                apple_simd_focus: false,
                ..Default::default()
            },
        )
        .unwrap();
        let maximum_bayer_error = result
            .bayer
            .iter()
            .zip(&scalar_result.bayer)
            .map(|(left, right)| (left - right).abs())
            .fold(0.0f32, f32::max);
        let maximum_depth_error = result
            .depth
            .iter()
            .zip(&scalar_result.depth)
            .map(|(left, right)| (left - right).abs())
            .fold(0.0f32, f32::max);
        assert!(maximum_bayer_error <= 2e-5);
        assert!(maximum_depth_error <= 2e-5);
        #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
        assert_eq!(result.focus_kernel, "apple-neon-f32-v1");
        #[cfg(not(all(target_arch = "aarch64", target_os = "macos")))]
        assert_eq!(result.focus_kernel, "portable-scalar-f32-v1");
        assert_eq!(scalar_result.focus_kernel, "portable-scalar-f32-v1");

        let mut squared_error = 0.0f64;
        let mut sample_count = 0usize;
        let mut correct_depth = 0usize;
        let mut depth_count = 0usize;
        for y in 8..height - 8 {
            for x in 8..width - 8 {
                if x.abs_diff(width / 2) < 8 || (x ^ y) & 1 == 0 {
                    continue;
                }
                let error = result.bayer[[y, x, 0]] - sharp[y * width + x];
                squared_error += f64::from(error * error);
                sample_count += 1;
                let expected_far = x >= width / 2;
                correct_depth += usize::from((result.depth[[y, x]] >= 0.5) == expected_far);
                depth_count += 1;
            }
        }
        let mse = squared_error / sample_count as f64;
        let psnr = 10.0 * (1.0 / mse).log10();
        let depth_accuracy = correct_depth as f64 / depth_count as f64;
        eprintln!(
            "synthetic focus quality: PSNR={psnr:.3}dB depth_accuracy={:.3}%",
            depth_accuracy * 100.0
        );
        assert!(psnr >= 44.0, "focus stack PSNR regressed to {psnr:.3} dB");
        assert!(
            depth_accuracy >= 0.98,
            "focus depth accuracy regressed to {:.2}%",
            depth_accuracy * 100.0
        );

        let physical_group = NativeFrameGroup::from_parts(
            &pixels,
            2,
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            physical_metadata,
        )
        .unwrap();
        let physical_result = fuse_native_group(
            &physical_group,
            &meta(2, 1),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(white),
                tile_size: 32,
                minimum_alignment_score: 2.0,
                regularize_depth: false,
                trimap_max_radius_pixels: 256,
                ..Default::default()
            },
        )
        .unwrap();
        let physical_result_wide_tile = fuse_native_group(
            &physical_group,
            &meta(2, 1),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(white),
                tile_size: 64,
                minimum_alignment_score: 2.0,
                regularize_depth: false,
                trimap_max_radius_pixels: 256,
                ..Default::default()
            },
        )
        .unwrap();
        let physical_diagnostics_only = fuse_native_group(
            &physical_group,
            &meta(2, 1),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(white),
                tile_size: 32,
                minimum_alignment_score: 2.0,
                regularize_depth: false,
                depth_consistent_refusion: false,
                trimap_max_radius_pixels: 256,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(physical_result.visibility_constrained);
        assert!(physical_result.visibility_adjusted_pixels > 0);
        assert!(physical_result.mixed_boundary_pixels > 0);
        assert!(physical_result.boundary_source_fallback_pixels > 0);
        assert!(physical_diagnostics_only.mixed_boundary_pixels > 0);
        assert_eq!(physical_diagnostics_only.boundary_source_fallback_pixels, 0);
        assert_eq!(physical_diagnostics_only.depth_refusion_pixels, 0);
        assert_eq!(
            physical_result.boundary_trimap,
            physical_result_wide_tile.boundary_trimap
        );
        assert_eq!(physical_result.bayer, physical_result_wide_tile.bayer);
        let flagged = physical_result
            .fusion_flags
            .iter()
            .filter(|flags| **flags & FUSION_FLAG_VISIBILITY_CORRECTED != 0)
            .count();
        assert_eq!(flagged, physical_result.visibility_adjusted_pixels);

        let boundary_revision = fuse_native_group_with_id(
            &physical_group,
            &meta(2, 1),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(white),
                tile_size: 32,
                minimum_alignment_score: 2.0,
                regularize_depth: false,
                trimap_max_radius_pixels: 256,
                fusion_edit: Some(FusionEditDocument {
                    schema: FUSION_EDIT_SCHEMA.to_string(),
                    capture_group_id: "9".repeat(64),
                    base_report_sha256: "8".repeat(64),
                    width: width as u32,
                    height: height as u32,
                    crop_origin_x: 0,
                    crop_origin_y: 0,
                    frame_count: 2,
                    operations: vec![FusionEditOperation {
                        id: "physical-boundary".to_string(),
                        rect: FusionEditRect {
                            x: 0,
                            y: 0,
                            width: width as u32,
                            height: height as u32,
                        },
                        source_frame: 0,
                        reason: FusionEditReason::Boundary,
                        selector: FusionEditSelector::BoundaryAffected,
                        note: None,
                    }],
                }),
                ..Default::default()
            },
            Some(&"9".repeat(64)),
        )
        .unwrap();
        let expected_boundary_pixels = physical_result
            .boundary_trimap
            .iter()
            .zip(&physical_result.depth)
            .filter(|(state, depth)| {
                **state != BOUNDARY_TRIMAP_INTERIOR && depth.round() as usize == 0
            })
            .count();
        assert!(expected_boundary_pixels > 0);
        assert_eq!(
            boundary_revision.operator_override_pixels,
            expected_boundary_pixels
        );
        for (edit_state, (boundary_state, depth)) in boundary_revision
            .fusion_edit_map
            .as_ref()
            .unwrap()
            .iter()
            .zip(
                physical_result
                    .boundary_trimap
                    .iter()
                    .zip(&physical_result.depth),
            )
        {
            assert_eq!(
                *edit_state,
                if *boundary_state != BOUNDARY_TRIMAP_INTERIOR && depth.round() as usize == 0 {
                    FUSION_EDIT_BOUNDARY_CONSTRAINED
                } else {
                    0
                }
            );
        }

        let incompatible_focus_error = fuse_native_group_with_id(
            &physical_group,
            &meta(2, 1),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(white),
                tile_size: 32,
                minimum_alignment_score: 2.0,
                regularize_depth: false,
                trimap_max_radius_pixels: 256,
                fusion_edit: Some(FusionEditDocument {
                    schema: FUSION_EDIT_SCHEMA.to_string(),
                    capture_group_id: "5".repeat(64),
                    base_report_sha256: "4".repeat(64),
                    width: width as u32,
                    height: height as u32,
                    crop_origin_x: 0,
                    crop_origin_y: 0,
                    frame_count: 2,
                    operations: vec![FusionEditOperation {
                        id: "incompatible-boundary-focus".to_string(),
                        rect: FusionEditRect {
                            x: 0,
                            y: 0,
                            width: width as u32,
                            height: height as u32,
                        },
                        source_frame: 1,
                        reason: FusionEditReason::Boundary,
                        selector: FusionEditSelector::BoundaryAffected,
                        note: None,
                    }],
                }),
                ..Default::default()
            },
            Some(&"5".repeat(64)),
        )
        .unwrap_err();
        assert!(incompatible_focus_error
            .to_string()
            .contains("matched no recomputed physical evidence"));

        let boundary_core_revision = fuse_native_group_with_id(
            &physical_group,
            &meta(2, 1),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(white),
                tile_size: 32,
                minimum_alignment_score: 2.0,
                regularize_depth: false,
                trimap_max_radius_pixels: 256,
                fusion_edit: Some(FusionEditDocument {
                    schema: FUSION_EDIT_SCHEMA.to_string(),
                    capture_group_id: "7".repeat(64),
                    base_report_sha256: "6".repeat(64),
                    width: width as u32,
                    height: height as u32,
                    crop_origin_x: 0,
                    crop_origin_y: 0,
                    frame_count: 2,
                    operations: vec![FusionEditOperation {
                        id: "physical-boundary-core".to_string(),
                        rect: FusionEditRect {
                            x: 0,
                            y: 0,
                            width: width as u32,
                            height: height as u32,
                        },
                        source_frame: 0,
                        reason: FusionEditReason::Boundary,
                        selector: FusionEditSelector::BoundaryCrossingCore,
                        note: None,
                    }],
                }),
                ..Default::default()
            },
            Some(&"7".repeat(64)),
        )
        .unwrap();
        let expected_core_pixels = physical_result
            .boundary_trimap
            .iter()
            .zip(&physical_result.depth)
            .filter(|(state, depth)| {
                **state == BOUNDARY_TRIMAP_CROSSING_CORE && depth.round() as usize == 0
            })
            .count();
        assert!(expected_core_pixels > 0);
        assert_eq!(
            boundary_core_revision.operator_override_pixels,
            expected_core_pixels
        );
        for (edit_state, (boundary_state, depth)) in boundary_core_revision
            .fusion_edit_map
            .as_ref()
            .unwrap()
            .iter()
            .zip(
                physical_result
                    .boundary_trimap
                    .iter()
                    .zip(&physical_result.depth),
            )
        {
            assert_eq!(
                *edit_state,
                if *boundary_state == BOUNDARY_TRIMAP_CROSSING_CORE && depth.round() as usize == 0 {
                    FUSION_EDIT_BOUNDARY_CORE_CONSTRAINED
                } else {
                    0
                }
            );
        }
    }

    fn box_blur(input: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
        let mut output = vec![0.0; input.len()];
        for y in 0..height {
            for x in 0..width {
                let mut sum = 0.0;
                let mut count = 0usize;
                for yy in y.saturating_sub(radius)..=(y + radius).min(height - 1) {
                    for xx in x.saturating_sub(radius)..=(x + radius).min(width - 1) {
                        sum += input[yy * width + xx];
                        count += 1;
                    }
                }
                output[y * width + x] = sum / count as f32;
            }
        }
        output
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
    fn hdr_groups_fail_closed_without_complete_exposure_metadata() {
        let pixels = vec![2048u16; 2 * 16 * 16];
        let mut incomplete = metadata(1.0);
        incomplete.iso = None;
        let group = NativeFrameGroup::from_parts(
            &pixels,
            2,
            16,
            16,
            Rect::new(0.0, 0.0, 16.0, 16.0),
            vec![metadata(1.0), incomplete],
        )
        .unwrap();
        let error = fuse_native_group(
            &group,
            &meta(1, 2),
            &NativeFusionConfig {
                regularize_depth: false,
                ..NativeFusionConfig::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("missing shutter"));
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
    fn calibrated_spatial_correction_flattens_signal_repairs_defects_and_is_traceable() {
        let width = 16;
        let height = 16;
        let nominal_raw = 3_277u16;
        let mut pixels = vec![nominal_raw; width * height];
        pixels[8 * width + 8] = 15_000;
        let mut frame_metadata = metadata(1.0);
        frame_metadata.cam_mul = [1.0; 4];
        let group = NativeFrameGroup::from_parts(
            &pixels,
            1,
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            vec![frame_metadata],
        )
        .unwrap();
        let profile = calibrated_correction_profile(1.5, vec![DefectPixel { x: 8, y: 8 }]);
        let result = fuse_native_group(
            &group,
            &meta(1, 1),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(16_383.0),
                regularize_depth: false,
                depth_consistent_refusion: false,
                sensor_correction_profile: Some(profile),
                ..Default::default()
            },
        )
        .unwrap();
        let expected = f32::from(nominal_raw) / 16_383.0 * 1.5;
        assert!((result.bayer[[8, 8, 0]] - expected).abs() < 1e-5);
        assert_eq!(
            result.sensor_correction_map[[8, 8]]
                & (SENSOR_CORRECTION_FLAT_FIELD | SENSOR_CORRECTION_DEFECT_REPAIRED),
            SENSOR_CORRECTION_FLAT_FIELD | SENSOR_CORRECTION_DEFECT_REPAIRED
        );
        assert!(result.defect_repaired_pixels >= 1);
        assert_eq!(
            result.sensor_correction_id.as_deref(),
            Some("sha256:synthetic-spatial-correction")
        );
        assert!(result.radiance_uncertainty[[8, 8]].is_finite());
    }

    #[test]
    fn spatial_correction_and_bayer_crop_mismatch_fail_closed() {
        let width = 16;
        let height = 16;
        let pixels = vec![2_000u16; width * height];
        let group = NativeFrameGroup::from_parts(
            &pixels,
            1,
            width,
            height,
            Rect::new(1.0, 0.0, width as f64, height as f64),
            vec![metadata(1.0)],
        )
        .unwrap();
        assert!(fuse_native_group(
            &group,
            &meta(1, 1),
            &NativeFusionConfig {
                sensor_correction_profile: Some(calibrated_correction_profile(1.0, vec![])),
                ..Default::default()
            },
        )
        .is_err());

        let even_group = NativeFrameGroup::from_parts(
            &pixels,
            1,
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            vec![metadata(1.0)],
        )
        .unwrap();
        let mut wrong_optics = calibrated_correction_profile(1.0, vec![]);
        wrong_optics.aperture = 8.0;
        assert!(fuse_native_group(
            &even_group,
            &meta(1, 1),
            &NativeFusionConfig {
                sensor_correction_profile: Some(wrong_optics),
                ..Default::default()
            },
        )
        .is_err());

        let mut outside_focus = metadata(1.0);
        outside_focus.focus_distance = Some(4.0);
        let outside_focus_group = NativeFrameGroup::from_parts(
            &pixels,
            1,
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            vec![outside_focus],
        )
        .unwrap();
        assert!(fuse_native_group(
            &outside_focus_group,
            &meta(1, 1),
            &NativeFusionConfig {
                sensor_correction_profile: Some(calibrated_correction_profile(1.0, vec![])),
                ..Default::default()
            },
        )
        .is_err());
    }

    fn calibrated_noise_profile(iso: u32) -> SensorNoiseProfile {
        SensorNoiseProfile {
            schema: SENSOR_NOISE_PROFILE_SCHEMA.to_string(),
            camera_make: "Nikon".to_string(),
            camera_model: "Z9".to_string(),
            bits_per_sample: 14,
            calibration_id: "sha256:synthetic-test".to_string(),
            iso_models: vec![IsoNoiseModel {
                iso,
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

    fn posterior_sample(radiance: f32, lower_bound: f32, censored: bool) -> HdrSample {
        let read_variance = 4e-4;
        let shot_variance_per_radiance = 2e-3;
        let modeled_radiance = if censored { lower_bound } else { radiance };
        HdrSample {
            radiance,
            variance: read_variance + shot_variance_per_radiance * modeled_radiance,
            read_variance,
            shot_variance_per_radiance,
            lower_bound,
            censored,
            frame_index: 0,
            ..HdrSample::default()
        }
    }

    #[test]
    fn inverse_mills_ratio_matches_known_survival_scores() {
        assert!((inverse_mills_ratio(0.0) - 0.797_884_560_8).abs() < 1e-6);
        assert!((inverse_mills_ratio(-2.0) - 2.373_215_533).abs() < 2e-5);
        assert!((inverse_mills_ratio(2.0) - 0.055_247_863).abs() < 2e-6);
        assert!(inverse_mills_ratio(-20.0).is_finite());
    }

    #[test]
    fn censored_likelihood_softly_updates_radiance_and_information() {
        let uncensored = [posterior_sample(0.48, 0.0, false)];
        let censored = posterior_sample(0.52, 0.52, true);
        let all_samples = [uncensored[0], censored];
        let initial_uncertainty = uncensored[0].variance.sqrt();
        let first = solve_censored_poisson_gaussian(
            &uncensored,
            &[1.0],
            &all_samples,
            uncensored[0].radiance,
            initial_uncertainty,
            1.0,
        );
        let second = solve_censored_poisson_gaussian(
            &uncensored,
            &[1.0],
            &all_samples,
            uncensored[0].radiance,
            initial_uncertainty,
            1.0,
        );
        assert_eq!(first.radiance, second.radiance);
        assert_eq!(first.uncertainty, second.uncertainty);
        assert!(first.used_censored);
        assert!(!first.censor_conflict);
        assert!(
            (0.48..0.60).contains(&first.radiance),
            "survival posterior was {}",
            first.radiance
        );
        assert_ne!(
            first.radiance, censored.lower_bound,
            "censor evidence must not degrade to a hard lower-bound clamp"
        );
        assert!(first.uncertainty < initial_uncertainty);
    }

    #[test]
    fn inconsistent_censor_constraint_is_rejected_without_bias() {
        let uncensored = [posterior_sample(0.48, 0.0, false)];
        let conflict = posterior_sample(2.0, 2.0, true);
        let posterior = solve_censored_poisson_gaussian(
            &uncensored,
            &[1.0],
            &[uncensored[0], conflict],
            uncensored[0].radiance,
            uncensored[0].variance.sqrt(),
            1.0,
        );
        assert!(!posterior.used_censored);
        assert!(posterior.censor_conflict);
        assert_eq!(posterior.radiance, uncensored[0].radiance);
    }

    #[test]
    fn censored_posterior_intervals_are_calibrated_conditionally() {
        struct Noise {
            state: u64,
            spare: Option<f32>,
        }
        impl Noise {
            fn normal(&mut self) -> f32 {
                if let Some(value) = self.spare.take() {
                    return value;
                }
                let uniform = |state: &mut u64| {
                    *state ^= *state << 13;
                    *state ^= *state >> 7;
                    *state ^= *state << 17;
                    ((*state >> 40) as f32 + 0.5) / ((1u32 << 24) as f32)
                };
                let radius = (-2.0 * uniform(&mut self.state).max(1e-7).ln()).sqrt();
                let angle = std::f32::consts::TAU * uniform(&mut self.state);
                self.spare = Some(radius * angle.sin());
                radius * angle.cos()
            }
        }

        let truth = 0.56f32;
        let censor_threshold = 0.54f32;
        let model = posterior_sample(truth, 0.0, false);
        let sigma = model.variance.sqrt();
        let censored_model = posterior_sample(censor_threshold, censor_threshold, true);
        let mut noise = Noise {
            state: 0x243f_6a88_85a3_08d3,
            spare: None,
        };
        let mut censored_trials = 0usize;
        let mut covered = 0usize;
        let mut conflicts = 0usize;
        let mut bias = 0.0f64;
        for _ in 0..40_000 {
            let uncensored = [
                posterior_sample(truth + sigma * noise.normal(), 0.0, false),
                posterior_sample(truth + sigma * noise.normal(), 0.0, false),
                posterior_sample(truth + sigma * noise.normal(), 0.0, false),
            ];
            let censored_observation = truth + sigma * noise.normal();
            if censored_observation < censor_threshold {
                continue;
            }
            let initial_radiance =
                uncensored.iter().map(|sample| sample.radiance).sum::<f32>() / 3.0;
            let initial_uncertainty = (model.variance / 3.0).sqrt();
            let all_samples = [uncensored[0], uncensored[1], uncensored[2], censored_model];
            let posterior = solve_censored_poisson_gaussian(
                &uncensored,
                &[1.0; 3],
                &all_samples,
                initial_radiance,
                initial_uncertainty,
                1.0,
            );
            assert!(posterior.radiance.is_finite());
            assert!(posterior.uncertainty.is_finite() && posterior.uncertainty > 0.0);
            conflicts += usize::from(posterior.censor_conflict);
            covered += usize::from(
                (posterior.radiance - truth).abs() <= 1.959_964 * posterior.uncertainty,
            );
            bias += f64::from(posterior.radiance - truth);
            censored_trials += 1;
        }

        let coverage = covered as f64 / censored_trials as f64;
        let mean_bias = bias / censored_trials as f64;
        let conflict_rate = conflicts as f64 / censored_trials as f64;
        println!(
            "conditional censored posterior: trials={censored_trials}, \
             coverage={:.3}%, bias={mean_bias:.6}, conflict={:.4}%",
            coverage * 100.0,
            conflict_rate * 100.0
        );
        assert!(censored_trials > 25_000);
        assert!(
            (0.925..=0.975).contains(&coverage),
            "nominal 95% conditional coverage was {coverage:.4}"
        );
        assert!(
            mean_bias.abs() <= 0.15 * f64::from(sigma),
            "conditional bias {mean_bias:.6} exceeded 0.15 sigma"
        );
        assert!(
            conflict_rate <= 0.0005,
            "compatible censor constraints were rejected at rate {conflict_rate:.6}"
        );
    }

    #[test]
    fn censored_samples_are_lower_bounds_not_low_biased_measurements() {
        let width = 16;
        let height = 16;
        let white = 16_383.0;
        let scene = 0.6f32;
        let exposures = [1.0, 4.0];
        let mut pixels = Vec::new();
        for exposure in exposures {
            let signal = (scene * exposure as f32).min(1.0);
            let raw = (signal * white).round() as u16;
            pixels.extend(std::iter::repeat(raw).take(width * height));
        }
        let group = NativeFrameGroup::from_parts(
            &pixels,
            exposures.len(),
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            exposures.iter().map(|&value| metadata(value)).collect(),
        )
        .unwrap();
        let result = fuse_native_group(
            &group,
            &meta(1, exposures.len()),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(white),
                sensor_noise_profile: Some(calibrated_noise_profile(100)),
                regularize_depth: false,
                ..NativeFusionConfig::default()
            },
        )
        .unwrap();
        let y = 8;
        let x = 9;
        assert!((result.bayer[[y, x, 0]] - scene).abs() < 2e-3);
        assert_ne!(result.fusion_flags[[y, x]] & FUSION_FLAG_CENSORED, 0);
        assert_eq!(
            result.fusion_flags[[y, x]] & FUSION_FLAG_UNCALIBRATED_NOISE,
            0
        );
        assert!(result.radiance_uncertainty[[y, x]].is_finite());
        assert!(result.noise_model_calibrated);
    }

    #[test]
    fn measured_source_edits_are_exact_bound_and_deterministic() {
        let width = 16;
        let height = 16;
        let white = 16_383.0;
        let mut pixels = vec![(0.2 * white) as u16; width * height];
        pixels.extend(std::iter::repeat_n((0.8 * white) as u16, width * height));
        let group = NativeFrameGroup::from_parts(
            &pixels,
            2,
            width,
            height,
            Rect::new(12.0, 20.0, width as f64, height as f64),
            vec![metadata(1.0), metadata(2.0)],
        )
        .unwrap();
        let edit = FusionEditDocument {
            schema: FUSION_EDIT_SCHEMA.to_string(),
            capture_group_id: "a".repeat(64),
            base_report_sha256: "b".repeat(64),
            width: width as u32,
            height: height as u32,
            crop_origin_x: 12,
            crop_origin_y: 20,
            frame_count: 2,
            operations: vec![FusionEditOperation {
                id: "moving-highlight".to_string(),
                rect: FusionEditRect {
                    x: 4,
                    y: 5,
                    width: 3,
                    height: 2,
                },
                source_frame: 1,
                reason: FusionEditReason::Motion,
                selector: FusionEditSelector::Rectangle,
                note: None,
            }],
        };
        let config = NativeFusionConfig {
            black_level: Some(0.0),
            white_level: Some(white),
            regularize_depth: false,
            fusion_edit: Some(edit.clone()),
            ..NativeFusionConfig::default()
        };

        let first =
            fuse_native_group_with_id(&group, &meta(1, 2), &config, Some(&"a".repeat(64))).unwrap();
        let second =
            fuse_native_group_with_id(&group, &meta(1, 2), &config, Some(&"a".repeat(64))).unwrap();

        assert_eq!(first.operator_override_pixels, 6);
        assert_eq!(first.fusion_edit_digest, Some(edit.digest().unwrap()));
        assert_eq!(
            first.fusion_edit_base_report_sha256.as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        let map = first.fusion_edit_map.as_ref().unwrap();
        assert_eq!(map.iter().filter(|value| **value != 0).count(), 6);
        assert_eq!(map[[5, 4]], FUSION_EDIT_MEASURED_SOURCE);
        assert_eq!(first.source_map[[5, 4]], 1);
        assert_eq!(first.detail_source_map[[5, 4]], 1);
        assert!((first.bayer[[5, 4, 0]] - 0.4).abs() < 2e-4);
        assert_eq!(first.bayer, second.bayer);
        assert_eq!(first.source_map, second.source_map);
        assert_eq!(first.fusion_edit_map, second.fusion_edit_map);
    }

    #[test]
    fn measured_source_edits_reject_clipped_sources_atomically() {
        let width = 16;
        let height = 16;
        let white = 16_383u16;
        let pixels = vec![white; width * height];
        let group = NativeFrameGroup::from_parts(
            &pixels,
            1,
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            vec![metadata(1.0)],
        )
        .unwrap();
        let config = NativeFusionConfig {
            black_level: Some(0.0),
            white_level: Some(f32::from(white)),
            fusion_edit: Some(FusionEditDocument {
                schema: FUSION_EDIT_SCHEMA.to_string(),
                capture_group_id: "c".repeat(64),
                base_report_sha256: "d".repeat(64),
                width: width as u32,
                height: height as u32,
                crop_origin_x: 0,
                crop_origin_y: 0,
                frame_count: 1,
                operations: vec![FusionEditOperation {
                    id: "invalid-highlight".to_string(),
                    rect: FusionEditRect {
                        x: 8,
                        y: 8,
                        width: 1,
                        height: 1,
                    },
                    source_frame: 0,
                    reason: FusionEditReason::Glare,
                    selector: FusionEditSelector::GlareAffected,
                    note: None,
                }],
            }),
            ..NativeFusionConfig::default()
        };

        let error = fuse_native_group_with_id(&group, &meta(1, 1), &config, Some(&"c".repeat(64)))
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("cannot invent highlight radiance"));
    }

    #[test]
    fn all_censored_samples_return_an_attributed_lower_bound() {
        let width = 16;
        let height = 16;
        let white = 16_383u16;
        let pixels = vec![white; 2 * width * height];
        let group = NativeFrameGroup::from_parts(
            &pixels,
            2,
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            vec![metadata(1.0), metadata(2.0)],
        )
        .unwrap();
        let result = fuse_native_group(
            &group,
            &meta(1, 2),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(f32::from(white)),
                sensor_noise_profile: Some(calibrated_noise_profile(100)),
                regularize_depth: false,
                ..NativeFusionConfig::default()
            },
        )
        .unwrap();
        let flags = result.fusion_flags[[8, 9]];
        assert_ne!(flags & FUSION_FLAG_CENSORED, 0);
        assert_ne!(flags & FUSION_FLAG_SOURCE_FALLBACK, 0);
        assert_eq!(result.source_map[[8, 9]], u16::MAX);
        assert!(result.radiance_uncertainty[[8, 9]].is_infinite());
        assert!(result.bayer[[8, 9, 0]] > 0.99);
    }

    #[test]
    fn independent_samples_reduce_posterior_uncertainty() {
        let width = 16;
        let height = 16;
        let white = 16_383.0;
        let raw = (0.2 * white) as u16;
        let run = |count: usize| {
            let pixels = vec![raw; count * width * height];
            let group = NativeFrameGroup::from_parts(
                &pixels,
                count,
                width,
                height,
                Rect::new(0.0, 0.0, width as f64, height as f64),
                (0..count).map(|_| metadata(1.0)).collect(),
            )
            .unwrap();
            fuse_native_group(
                &group,
                &meta(1, count),
                &NativeFusionConfig {
                    black_level: Some(0.0),
                    white_level: Some(white),
                    sensor_noise_profile: Some(calibrated_noise_profile(100)),
                    regularize_depth: false,
                    ..NativeFusionConfig::default()
                },
            )
            .unwrap()
            .radiance_uncertainty[[8, 9]]
        };
        let one = run(1);
        let four = run(4);
        assert!(
            four < one * 0.51,
            "four-frame uncertainty {four} did not approach half of {one}"
        );
    }

    #[test]
    fn calibrated_posterior_intervals_have_empirical_coverage() {
        struct Noise {
            state: u64,
            spare: Option<f32>,
        }
        impl Noise {
            fn normal(&mut self) -> f32 {
                if let Some(value) = self.spare.take() {
                    return value;
                }
                let uniform = |state: &mut u64| {
                    *state ^= *state << 13;
                    *state ^= *state >> 7;
                    *state ^= *state << 17;
                    ((*state >> 40) as f32 + 0.5) / ((1u32 << 24) as f32)
                };
                let radius = (-2.0 * uniform(&mut self.state).max(1e-7).ln()).sqrt();
                let angle = std::f32::consts::TAU * uniform(&mut self.state);
                self.spare = Some(radius * angle.sin());
                radius * angle.cos()
            }
        }

        let width = 64;
        let height = 64;
        let frame_count = 4;
        let white = 16_383.0f32;
        let model = calibrated_noise_profile(100);
        let sensor_model = model.model_for_iso(100).unwrap();
        let mut noise = Noise {
            state: 0x9e37_79b9_7f4a_7c15,
            spare: None,
        };
        let mut expected = vec![0.0f32; width * height];
        let mut pixels = Vec::with_capacity(frame_count * width * height);
        for frame in 0..frame_count {
            for y in 0..height {
                for x in 0..width {
                    let site = ((y & 1) << 1) | (x & 1);
                    let signal = 0.08 + 0.70 * ((x * 31 + y * 47 + x * y * 3) % 997) as f32 / 996.0;
                    if frame == 0 {
                        let green = 1.0;
                        let white_balance = match site {
                            0 => 2.0 / green,
                            1 | 2 => 1.0,
                            _ => 1.5 / green,
                        };
                        expected[y * width + x] = signal * white_balance;
                    }
                    let signal_dn = signal * white;
                    let sigma_dn = (sensor_model.read_noise_dn[site].powi(2)
                        + sensor_model.black_drift_dn[site].powi(2)
                        + signal_dn / sensor_model.electrons_per_dn[site])
                        .sqrt();
                    pixels.push(
                        (signal_dn + sigma_dn * noise.normal())
                            .round()
                            .clamp(0.0, white) as u16,
                    );
                }
            }
        }
        let mut frame_metadata = Vec::with_capacity(frame_count);
        for _ in 0..frame_count {
            let mut metadata = metadata(1.0);
            metadata.width = width as u32;
            metadata.height = height as u32;
            frame_metadata.push(metadata);
        }
        let group = NativeFrameGroup::from_parts(
            &pixels,
            frame_count,
            width,
            height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
            frame_metadata,
        )
        .unwrap();
        let result = fuse_native_group(
            &group,
            &meta(1, frame_count),
            &NativeFusionConfig {
                black_level: Some(0.0),
                white_level: Some(white),
                sensor_noise_profile: Some(model),
                selective_local_alignment: false,
                regularize_depth: false,
                ..NativeFusionConfig::default()
            },
        )
        .unwrap();
        let mut covered = 0usize;
        let mut count = 0usize;
        for y in 2..height - 2 {
            for x in 2..width - 2 {
                let uncertainty = result.radiance_uncertainty[[y, x]];
                let estimate = result.bayer[[y, x, 0]];
                if uncertainty.is_finite() && uncertainty > 0.0 {
                    covered += usize::from(
                        (estimate - expected[y * width + x]).abs() <= 1.959_964 * uncertainty,
                    );
                    count += 1;
                }
            }
        }
        let coverage = covered as f32 / count as f32;
        println!(
            "calibrated posterior coverage: {:.3}% ({covered}/{count})",
            coverage * 100.0
        );
        assert!(
            (0.925..=0.975).contains(&coverage),
            "nominal 95% posterior coverage was {coverage:.4}"
        );
    }

    #[test]
    fn calibrated_profiles_require_an_exact_iso_entry() {
        let pixels = vec![2048u16; 16 * 16];
        let mut frame_metadata = metadata(1.0);
        frame_metadata.iso = Some(200);
        let group = NativeFrameGroup::from_parts(
            &pixels,
            1,
            16,
            16,
            Rect::new(0.0, 0.0, 16.0, 16.0),
            vec![frame_metadata],
        )
        .unwrap();
        let error = fuse_native_group(
            &group,
            &meta(1, 1),
            &NativeFusionConfig {
                sensor_noise_profile: Some(calibrated_noise_profile(100)),
                ..NativeFusionConfig::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("no exact ISO 200"));
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
