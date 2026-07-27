//! SmartLoader: Intelligent image loading and sequence management
//!
//! Responsibilities:
//! - Scanning directory for valid RAW files
//! - Grouping files into sequences (time + focus/exposure pattern)
//! - Automatic object detection for ROI (Region of Interest)
//! - Selective loading of RAW data (loading only the object, not black background)
//! - Resource-aware loading (checking RAM before loading)

use crate::capture_manifest::{
    discover_capture_manifest, CaptureGroup, CaptureGroupSource, CaptureManifestReader,
};
use crate::exif_parser::{group_sequences_with_key, scan_nef_files};
use crate::nef::parser::{Z9Metadata, Z9NefParser};
use crate::object_detection::detect_object_bbox_with_key;
use crate::raw_io::{
    load_bayer_frame_with_key, load_nef_roi_into_with_key, selective_bayer_load_with_key,
};
use crate::timed_scope;
use crate::timing::HierarchicalTimer;
use crate::types::{BayerFrame, ProcessingOptions, Rect, Sequence};
use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use zeroize::Zeroizing;

pub struct SmartLoader {
    options: ProcessingOptions,
    decode_pool: Option<rayon::ThreadPool>,
    encrypted_raw_key: Option<Arc<Zeroizing<[u8; 32]>>>,
}

/// One preview-derived crop shared by an entire focus/HDR capture group.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SequenceCropPlan {
    pub reference_index: usize,
    pub rect: Option<Rect>,
}

/// Reusable native storage for one ordered HDR/focus group.
#[derive(Debug, Default)]
pub struct NativeGroupArena {
    storage: Vec<u16>,
}

impl NativeGroupArena {
    pub fn capacity_bytes(&self) -> usize {
        self.storage.capacity() * std::mem::size_of::<u16>()
    }

    /// Return retained native storage before a larger downstream stage.
    ///
    /// Ordinary ROI groups keep their allocation for reuse. Full-sensor groups
    /// can release the arena once fusion no longer borrows it so postprocessing
    /// does not overlap an unnecessary multi-gigabyte input buffer.
    pub fn release(&mut self) -> usize {
        let released = self.capacity_bytes();
        self.storage = Vec::new();
        released
    }

    fn prepare(&mut self, frame_count: usize, width: usize, height: usize) -> Result<&mut [u16]> {
        let frame_pixels = width
            .checked_mul(height)
            .context("Native group frame dimensions overflow")?;
        let required_pixels = frame_pixels
            .checked_mul(frame_count)
            .context("Native group dimensions overflow")?;
        self.storage.resize(required_pixels, 0);
        Ok(&mut self.storage)
    }
}

/// Borrowed view of a fully decoded, order-preserving native capture group.
pub struct NativeFrameGroup<'a> {
    pixels: &'a [u16],
    frame_count: usize,
    pub width: usize,
    pub height: usize,
    pub rect: Rect,
    pub metadata: Vec<Z9Metadata>,
}

impl<'a> NativeFrameGroup<'a> {
    pub fn len(&self) -> usize {
        self.frame_count
    }

    pub fn is_empty(&self) -> bool {
        self.frame_count == 0
    }

    pub fn frame(&self, index: usize) -> Option<&[u16]> {
        if index >= self.frame_count {
            return None;
        }
        let frame_pixels = self.width.checked_mul(self.height)?;
        let start = index.checked_mul(frame_pixels)?;
        self.pixels.get(start..start.checked_add(frame_pixels)?)
    }

    pub fn size_bytes(&self) -> usize {
        std::mem::size_of_val(self.pixels)
    }

    pub(crate) fn from_parts(
        pixels: &'a [u16],
        frame_count: usize,
        width: usize,
        height: usize,
        rect: Rect,
        metadata: Vec<Z9Metadata>,
    ) -> Result<Self> {
        let expected = frame_count
            .checked_mul(width)
            .and_then(|pixels| pixels.checked_mul(height))
            .context("Native frame group dimensions overflow")?;
        if pixels.len() != expected {
            anyhow::bail!(
                "Native frame group has {} pixels, expected {}",
                pixels.len(),
                expected
            );
        }
        if metadata.len() != frame_count {
            anyhow::bail!(
                "Native frame group has {} metadata records for {} frames",
                metadata.len(),
                frame_count
            );
        }
        Ok(Self {
            pixels,
            frame_count,
            width,
            height,
            rect,
            metadata,
        })
    }
}

impl SmartLoader {
    pub fn new(options: ProcessingOptions) -> Self {
        let workers = options
            .max_parallel_sequences
            .unwrap_or_else(|| num_cpus::get_physical().clamp(1, 8))
            .max(1);
        let decode_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .thread_name(|index| format!("trueshot-nef-{index}"))
            .build()
            .map_err(|error| {
                tracing::warn!("Failed to build bounded NEF pool: {error}");
                error
            })
            .ok();
        Self {
            options,
            decode_pool,
            encrypted_raw_key: None,
        }
    }

    pub fn with_encrypted_raw_key(mut self, key: [u8; 32]) -> Self {
        self.encrypted_raw_key = Some(Arc::new(Zeroizing::new(key)));
        self
    }

    fn encrypted_raw_key(&self) -> Option<&[u8; 32]> {
        self.encrypted_raw_key.as_deref().map(|key| &**key)
    }

    /// Scan directory and group into sequences
    pub fn scan_and_group(&self, input_dir: &Path) -> Result<Vec<Sequence>> {
        tracing::info!("Scanning directory: {:?}", input_dir);
        let nef_files = scan_nef_files(input_dir)?;

        if nef_files.is_empty() {
            anyhow::bail!("No NEF files found in {:?}", input_dir);
        }

        tracing::info!("Found {} NEF files", nef_files.len());
        let sequences = group_sequences_with_key(&nef_files, self.encrypted_raw_key())?;
        tracing::info!("Grouped into {} sequences", sequences.len());

        Ok(sequences)
    }

    /// Open a bounded group stream. Capture manifests are preferred; the
    /// materializing metadata scanner remains available for legacy folders.
    pub fn open_capture_groups(&self, input_dir: &Path) -> Result<CaptureGroupSource> {
        if let Some(path) = discover_capture_manifest(input_dir) {
            tracing::info!("Streaming capture groups from {}", path.display());
            return Ok(CaptureGroupSource::Manifest(CaptureManifestReader::open(
                &path,
            )?));
        }
        tracing::warn!(
            "No {} found; using legacy materializing import",
            crate::capture_manifest::DEFAULT_CAPTURE_MANIFEST
        );
        let groups = self
            .scan_and_group(input_dir)?
            .into_iter()
            .map(CaptureGroup::from_sequence)
            .collect::<Vec<_>>();
        Ok(CaptureGroupSource::Legacy(groups.into_iter()))
    }

    /// Build the authoritative capture-time manifest record, including the
    /// single preview-derived crop plan shared by the complete group.
    pub fn prepare_capture_group(
        &self,
        sequence: Sequence,
        timer: &mut HierarchicalTimer,
    ) -> Result<CaptureGroup> {
        let crop_plan = self.sequence_crop_plan(&sequence, timer)?;
        let mut group = CaptureGroup::from_sequence(sequence);
        group.crop_plan = Some(crop_plan);
        Ok(group)
    }

    /// Build the immutable crop plan shared by every frame in a sequence.
    pub fn sequence_crop_plan(
        &self,
        sequence: &Sequence,
        timer: &mut HierarchicalTimer,
    ) -> Result<SequenceCropPlan> {
        if sequence.paths.is_empty() {
            anyhow::bail!("Cannot load an empty capture sequence");
        }

        let reference_index = sequence_crop_reference_index(
            sequence.meta.focus_steps as usize,
            sequence.meta.shutter_speeds.len(),
            sequence.paths.len(),
        );
        let ref_path = &sequence.paths[reference_index];
        let rect = if !self.options.full_decode {
            timed_scope!(timer, "detect_bbox", {
                tracing::info!(
                    "Using reference image for detection: {:?}",
                    ref_path.file_name()
                );
                match detect_object_bbox_with_key(ref_path, self.encrypted_raw_key()) {
                    Ok(bbox) => {
                        let coverage_pixels = bbox.width * bbox.height;
                        tracing::info!(
                            "Object bbox detected: {:.0}x{:.0} at ({:.0}, {:.0}) - {} px",
                            bbox.width,
                            bbox.height,
                            bbox.x,
                            bbox.y,
                            coverage_pixels as u64
                        );
                        Some(bbox)
                    }
                    Err(e) => {
                        tracing::warn!("Object detection failed: {}, falling back to full load", e);
                        None
                    }
                }
            })
        } else {
            tracing::info!("Full decode requested, skipping object detection");
            None
        };
        let plan = SequenceCropPlan {
            reference_index,
            rect,
        };
        tracing::debug!(
            "Sequence crop plan: reference={} shared_roi={:?}",
            plan.reference_index,
            plan.rect
        );
        Ok(plan)
    }

    /// Resolve detection fallback into a concrete rectangle so orchestration
    /// can reserve exact memory before native decoding begins.
    pub fn resolved_sequence_crop_plan(
        &self,
        sequence: &Sequence,
        persisted_crop_plan: Option<SequenceCropPlan>,
        timer: &mut HierarchicalTimer,
    ) -> Result<SequenceCropPlan> {
        let mut plan = match persisted_crop_plan {
            Some(plan) => {
                if plan.reference_index >= sequence.len() {
                    anyhow::bail!(
                        "Persisted crop reference {} is outside {}-frame group",
                        plan.reference_index,
                        sequence.len()
                    );
                }
                plan
            }
            None => self.sequence_crop_plan(sequence, timer)?,
        };
        if plan.rect.is_none() {
            let reference_path = &sequence.paths[plan.reference_index];
            let mut parser = Z9NefParser::for_path(reference_path, self.encrypted_raw_key())?;
            parser.parse().with_context(|| {
                format!(
                    "Failed to parse full-frame reference {}",
                    reference_path.display()
                )
            })?;
            let metadata = parser.get_metadata()?;
            plan.rect = Some(Rect::new(
                0.0,
                0.0,
                metadata.width as f64,
                metadata.height as f64,
            ));
        }
        Ok(plan)
    }

    /// Load a sequence through the legacy `f64` frame representation.
    ///
    /// Failures preserve frame ordering and fail the group rather than shifting
    /// focus/exposure indices.
    pub fn load_sequence(
        &self,
        sequence: &Sequence,
        timer: &mut HierarchicalTimer,
    ) -> Result<Vec<BayerFrame>> {
        tracing::debug!(
            "Loading sequence: {} ({} frames)",
            sequence.meta.bone_id,
            sequence.len()
        );
        let crop_plan = self.sequence_crop_plan(sequence, timer)?;

        let results: Vec<Result<BayerFrame>> = timed_scope!(timer, "load_frames", {
            let load = || {
                sequence
                    .paths
                    .par_iter()
                    .enumerate()
                    .map(|(index, path)| {
                        let frame = if let Some(ref b) = crop_plan.rect {
                            selective_bayer_load_with_key(path, b, self.encrypted_raw_key())
                        } else {
                            load_bayer_frame_with_key(path, self.encrypted_raw_key())
                        };
                        frame.with_context(|| {
                            format!(
                                "Failed to load sequence frame {} from {}",
                                index,
                                path.display()
                            )
                        })
                    })
                    .collect()
            };
            if let Some(pool) = &self.decode_pool {
                pool.install(load)
            } else {
                load()
            }
        });

        let mut frames = Vec::with_capacity(results.len());
        for result in results {
            frames.push(result?);
        }
        Ok(frames)
    }

    /// Decode a sequence directly into one reusable contiguous native arena.
    pub fn load_sequence_native_into<'a>(
        &self,
        sequence: &Sequence,
        arena: &'a mut NativeGroupArena,
        timer: &mut HierarchicalTimer,
    ) -> Result<NativeFrameGroup<'a>> {
        self.load_sequence_native_with_plan_into(sequence, None, arena, timer)
    }

    /// Decode a sequence using a capture-time crop plan when one is present.
    pub fn load_sequence_native_with_plan_into<'a>(
        &self,
        sequence: &Sequence,
        persisted_crop_plan: Option<SequenceCropPlan>,
        arena: &'a mut NativeGroupArena,
        timer: &mut HierarchicalTimer,
    ) -> Result<NativeFrameGroup<'a>> {
        tracing::debug!(
            "Loading native sequence: {} ({} frames)",
            sequence.meta.bone_id,
            sequence.len()
        );
        let crop_plan = self.resolved_sequence_crop_plan(sequence, persisted_crop_plan, timer)?;
        let rect = crop_plan
            .rect
            .context("Resolved sequence crop plan has no rectangle")?;

        let (x0, y0, x1, y1) = rect.to_bounds();
        let width = x1
            .checked_sub(x0)
            .context("Native group ROI width underflow")?;
        let height = y1
            .checked_sub(y0)
            .context("Native group ROI height underflow")?;
        if width == 0 || height == 0 {
            anyhow::bail!("Native group ROI is empty");
        }

        let frame_pixels = width
            .checked_mul(height)
            .context("Native group frame dimensions overflow")?;
        let storage = arena.prepare(sequence.len(), width, height)?;
        let results: Vec<Result<Z9Metadata>> = timed_scope!(timer, "load_native_frames", {
            let mut load = || {
                storage
                    .par_chunks_mut(frame_pixels)
                    .zip(sequence.paths.par_iter())
                    .enumerate()
                    .map(|(index, (slot, path))| {
                        load_nef_roi_into_with_key(path, rect, slot, self.encrypted_raw_key())
                            .with_context(|| {
                                format!(
                                    "Failed to fill native sequence slot {} from {}",
                                    index,
                                    path.display()
                                )
                            })
                    })
                    .collect()
            };
            if let Some(pool) = &self.decode_pool {
                pool.install(load)
            } else {
                load()
            }
        });

        let mut metadata = Vec::with_capacity(results.len());
        for result in results {
            metadata.push(result?);
        }
        if let Some(reference) = metadata.first() {
            for (index, frame) in metadata.iter().enumerate().skip(1) {
                if frame.bits_per_sample != reference.bits_per_sample
                    || frame.cfa_pattern != reference.cfa_pattern
                {
                    anyhow::bail!(
                        "Native group frame {} has incompatible {}-bit CFA {:?}; expected {}-bit {:?}",
                        index,
                        frame.bits_per_sample,
                        frame.cfa_pattern,
                        reference.bits_per_sample,
                        reference.cfa_pattern
                    );
                }
            }
        }

        NativeFrameGroup::from_parts(storage, sequence.len(), width, height, rect, metadata)
    }
}

fn sequence_crop_reference_index(
    focus_steps: usize,
    exposures_per_focus: usize,
    frame_count: usize,
) -> usize {
    if frame_count == 0 || exposures_per_focus == 0 {
        return 0;
    }
    let furthest_focus = focus_steps.saturating_sub(1);
    let longest_exposure = exposures_per_focus - 1;
    (furthest_focus * exposures_per_focus + longest_exposure).min(frame_count - 1)
}

#[cfg(test)]
mod tests {
    use super::{sequence_crop_reference_index, NativeFrameGroup, NativeGroupArena};
    use crate::types::Rect;

    #[test]
    fn crop_reference_is_furthest_focus_longest_exposure() {
        assert_eq!(sequence_crop_reference_index(7, 3, 21), 20);
        assert_eq!(sequence_crop_reference_index(4, 5, 20), 19);
    }

    #[test]
    fn crop_reference_clamps_incomplete_groups() {
        assert_eq!(sequence_crop_reference_index(7, 3, 19), 18);
        assert_eq!(sequence_crop_reference_index(0, 0, 0), 0);
    }

    #[test]
    fn native_group_arena_reuses_capacity() {
        let mut arena = NativeGroupArena::default();
        {
            let storage = arena.prepare(21, 1310, 1304).unwrap();
            assert_eq!(storage.len(), 21 * 1310 * 1304);
        }
        let capacity = arena.capacity_bytes();
        {
            let storage = arena.prepare(3, 256, 256).unwrap();
            assert_eq!(storage.len(), 3 * 256 * 256);
        }
        assert_eq!(arena.capacity_bytes(), capacity);
    }

    #[test]
    fn native_group_arena_releases_oversized_storage() {
        let mut arena = NativeGroupArena::default();
        arena.prepare(3, 64, 32).unwrap();
        let retained = arena.capacity_bytes();
        assert!(retained >= 3 * 64 * 32 * std::mem::size_of::<u16>());
        assert_eq!(arena.release(), retained);
        assert_eq!(arena.capacity_bytes(), 0);
    }

    #[test]
    fn native_group_frames_are_contiguous_and_ordered() {
        let pixels = [1u16, 2, 3, 4, 5, 6, 7, 8];
        let group = NativeFrameGroup {
            pixels: &pixels,
            frame_count: 2,
            width: 2,
            height: 2,
            rect: Rect::new(0.0, 0.0, 2.0, 2.0),
            metadata: Vec::new(),
        };
        assert_eq!(group.frame(0), Some(&pixels[0..4]));
        assert_eq!(group.frame(1), Some(&pixels[4..8]));
        assert_eq!(group.frame(2), None);
    }
}
