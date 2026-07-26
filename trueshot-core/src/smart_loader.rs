//! SmartLoader: Intelligent image loading and sequence management
//!
//! Responsibilities:
//! - Scanning directory for valid RAW files
//! - Grouping files into sequences (time + focus/exposure pattern)
//! - Automatic object detection for ROI (Region of Interest)
//! - Selective loading of RAW data (loading only the object, not black background)
//! - Resource-aware loading (checking RAM before loading)

use crate::types::{Sequence, BayerFrame, ProcessingOptions};
use crate::exif_parser::{scan_nef_files, group_sequences};
use crate::object_detection::detect_object_bbox;
use crate::raw_io::{load_bayer_frame, selective_bayer_load};
use crate::timing::HierarchicalTimer;
use crate::timed_scope;
use anyhow::Result;
use rayon::prelude::*;
use std::path::Path;

pub struct SmartLoader {
    options: ProcessingOptions,
}

impl SmartLoader {
    pub fn new(options: ProcessingOptions) -> Self {
        Self { options }
    }

    /// Scan directory and group into sequences
    pub fn scan_and_group(&self, input_dir: &Path) -> Result<Vec<Sequence>> {
        tracing::info!("Scanning directory: {:?}", input_dir);
        let nef_files = scan_nef_files(input_dir)?;
        
        if nef_files.is_empty() {
            anyhow::bail!("No NEF files found in {:?}", input_dir);
        }

        tracing::info!("Found {} NEF files", nef_files.len());
        let sequences = group_sequences(&nef_files)?;
        tracing::info!("Grouped into {} sequences", sequences.len());
        
        Ok(sequences)
    }

    /// Load a sequence of frames, optionally using selective loading
    pub fn load_sequence(&self, sequence: &Sequence, _timer: &mut HierarchicalTimer) -> Result<Vec<BayerFrame>> {
        tracing::debug!("Loading sequence: {} ({} frames)", sequence.meta.bone_id, sequence.len());

        // 1. Find global reference image for object detection
        // User notes: "choose the focus group that is at the furthest plane away, usually the last focus group... 
        // within the chosen focus group we choose an image with lots of visible features, usually the longest exposure time"
        let num_exposures = sequence.meta.shutter_speeds.len();
        let num_focus_steps = sequence.meta.focus_steps as usize;
        
        // Sequence pattern: F1E1, F1E2, F1E3, F2E1, ...
        // Furthest focus group is the last one
        // Longest exposure is the last one in that group (assuming sorted by shutter speed)
        let ref_idx = if num_exposures > 0 {
            (num_focus_steps.saturating_sub(1) * num_exposures) + (num_exposures.saturating_sub(1))
        } else {
            0
        };
        let ref_path = &sequence.paths[ref_idx.min(sequence.paths.len() - 1)];

        // 2. Detect object bbox ONCE from reference frame (timed)
        // Only if full_decode is FALSE (default)
        let bbox = if !self.options.full_decode {
            timed_scope!(timer, "detect_bbox", {
                tracing::info!("Using reference image for detection: {:?}", ref_path.file_name());
                match detect_object_bbox(ref_path) {
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

        // 2. Load all frames into memory
        let frames: Vec<_> = timed_scope!(timer, "load_frames", {
            sequence.paths.par_iter()
                .enumerate()
                .filter_map(|(i, path)| {
                    let result = if let Some(ref b) = bbox {
                        selective_bayer_load(path, b)
                    } else {
                        load_bayer_frame(path)
                    };

                    match result {
                        Ok(frame) => Some(frame),
                        Err(e) => {
                            tracing::error!("Failed to load frame {} ({:?}): {}", i, path, e);
                            None
                        }
                    }
                })
                .collect()
        });

        if frames.is_empty() {
            anyhow::bail!("Failed to load any frames for sequence {}", sequence.meta.bone_id);
        }
        
        if frames.len() != sequence.len() {
            tracing::warn!("Loaded {}/{} frames for sequence {}", 
                          frames.len(), sequence.len(), sequence.meta.bone_id);
        }

        Ok(frames)
    }
}
