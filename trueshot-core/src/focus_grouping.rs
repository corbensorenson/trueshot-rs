//! Automatic focus plane and exposure detection from timestamps and shutter speeds
//!
//! This module analyzes image metadata (timestamps, shutter speeds) to automatically
//! detect the number of focus planes and exposures per plane without hardcoding.

use anyhow::Result;
use std::path::PathBuf;

/// Result of timestamp analysis
#[derive(Debug, Clone)]
pub struct FocusExposurePattern {
    /// Number of focus planes detected
    pub focus_planes: usize,
    /// Number of exposures per focus plane
    pub exposures_per_plane: usize,
    /// Total number of images
    pub total_images: usize,
    /// Representative frame index for each focus plane (middle exposure)
    pub representative_indices: Vec<usize>,
}

/// Frame metadata needed for grouping
#[derive(Debug, Clone)]
pub struct FrameMetadata {
    pub path: PathBuf,
    pub timestamp_ms: u64,
    pub shutter_speed: f64,
}

/// Analyze timestamps and shutter speeds to detect focus/exposure pattern
///
/// This function detects the capture pattern by analyzing:
/// 1. Time gaps between consecutive frames (small gaps = same focus plane, large gaps = different focus plane)
/// 2. Shutter speed patterns (repeating pattern indicates exposures per plane)
///
/// # Arguments
/// * `metadata` - Frame metadata sorted by capture time
///
/// # Returns
/// * Pattern describing focus planes and exposures
pub fn analyze_focus_exposure_pattern(metadata: &[FrameMetadata]) -> Result<FocusExposurePattern> {
    let total_images = metadata.len();
    
    if total_images == 0 {
        anyhow::bail!("No images to analyze");
    }
    
    if total_images == 1 {
        return Ok(FocusExposurePattern {
            focus_planes: 1,
            exposures_per_plane: 1,
            total_images: 1,
            representative_indices: vec![0],
        });
    }
    
    tracing::info!("Analyzing {} images for focus/exposure pattern", total_images);
    
    // Sort by timestamp (should already be sorted, but ensure it)
    let mut sorted_metadata = metadata.to_vec();
    sorted_metadata.sort_by_key(|m| m.timestamp_ms);
    
    // Calculate time differences between consecutive images
    let mut time_diffs = Vec::new();
    for i in 1..sorted_metadata.len() {
        let diff_ms = sorted_metadata[i].timestamp_ms.saturating_sub(sorted_metadata[i-1].timestamp_ms);
        time_diffs.push(diff_ms);
    }
    
    // Detect exposures per plane by finding repeating shutter speed pattern
    let exposures_per_plane = detect_exposures_per_plane(&sorted_metadata);
    
    tracing::info!("Detected {} exposures per focus plane", exposures_per_plane);
    
    // Calculate focus planes
    let focus_planes = if total_images % exposures_per_plane == 0 {
        total_images / exposures_per_plane
    } else {
        // Best fit
        (total_images + exposures_per_plane - 1) / exposures_per_plane
    };
    
    tracing::info!("Detected {} focus planes × {} exposures = {} images (actual: {})",
                   focus_planes, exposures_per_plane, focus_planes * exposures_per_plane, total_images);
    
    // Calculate representative indices (middle exposure of each focus plane)
    let middle_exp_idx = exposures_per_plane / 2;
    let representative_indices: Vec<usize> = (0..focus_planes)
        .map(|focus_idx| focus_idx * exposures_per_plane + middle_exp_idx)
        .filter(|&idx| idx < total_images)
        .collect();
    
    tracing::info!("Representative frames: {:?}", representative_indices);
    
    Ok(FocusExposurePattern {
        focus_planes,
        exposures_per_plane,
        total_images,
        representative_indices,
    })
}

/// Detect number of exposures per focus plane by analyzing shutter speed patterns
fn detect_exposures_per_plane(metadata: &[FrameMetadata]) -> usize {
    if metadata.len() < 2 {
        return 1;
    }
    
    // Collect unique shutter speeds
    let mut shutter_speeds: Vec<f64> = metadata.iter().map(|m| m.shutter_speed).collect();
    shutter_speeds.sort_by(|a, b| a.partial_cmp(b).unwrap());
    shutter_speeds.dedup_by(|a, b| (*a - *b).abs() < 0.0001);
    
    let unique_exposures = shutter_speeds.len();
    
    tracing::info!("Found {} unique shutter speeds: {:?}", unique_exposures, shutter_speeds);
    
    // If we have 3 unique shutter speeds, it's likely 3 exposures per plane (HDR)
    // If we have 1 unique shutter speed, it's 1 exposure per plane (focus stack only)
    // Otherwise, try to detect the pattern
    
    if (2..=5).contains(&unique_exposures) {
        // Verify the pattern repeats
        let pattern_length = unique_exposures;
        let mut pattern_matches = true;
        
        for i in 0..metadata.len().saturating_sub(pattern_length) {
            let current_speed = metadata[i].shutter_speed;
            let next_cycle_speed = metadata[i + pattern_length].shutter_speed;
            
            if (current_speed - next_cycle_speed).abs() > 0.0001 {
                pattern_matches = false;
                break;
            }
        }
        
        if pattern_matches {
            tracing::info!("Detected repeating pattern of {} exposures", pattern_length);
            return pattern_length;
        }
    }
    
    // Fallback: assume 3 exposures per plane (common HDR pattern)
    if unique_exposures >= 3 {
        3
    } else if unique_exposures == 2 {
        2
    } else {
        1
    }
}

/// Get frame metadata from a sequence of paths (fast - no image decompression!)
pub fn extract_frame_metadata(paths: &[PathBuf]) -> Result<Vec<FrameMetadata>> {
    use crate::exif_parser::extract_z9_metadata;
    use rayon::prelude::*;

    tracing::info!("Extracting metadata from {} files (fast mode - no decompression)...", paths.len());

    let metadata: Vec<FrameMetadata> = paths
        .par_iter()
        .filter_map(|path| {
            // Extract metadata WITHOUT decompressing the image (fast!)
            match extract_z9_metadata(path) {
                Ok(file_meta) => {
                    Some(FrameMetadata {
                        path: path.clone(),
                        timestamp_ms: file_meta.timestamp_ms,
                        shutter_speed: file_meta.exposure_time, // exposure_time is shutter speed in seconds
                    })
                }
                Err(e) => {
                    tracing::warn!("Failed to extract metadata from {:?}: {}", path, e);
                    None
                }
            }
        })
        .collect();

    if metadata.is_empty() {
        anyhow::bail!("No valid frame metadata extracted");
    }

    tracing::info!("Extracted metadata from {} files", metadata.len());

    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_detect_3_exposures_20_focus() {
        // Simulate 20 focus planes × 3 exposures = 60 images
        let metadata: Vec<FrameMetadata> = (0..60)
            .map(|i| {
                let exp_idx = i % 3;
                let shutter_speed = match exp_idx {
                    0 => 1.0 / 60.0,
                    1 => 1.0 / 125.0,
                    2 => 1.0 / 250.0,
                    _ => 1.0 / 60.0,
                };
                
                FrameMetadata {
                    path: PathBuf::from(format!("image_{:04}.nef", i)),
                    timestamp_ms: i as u64 * 100,
                    shutter_speed,
                }
            })
            .collect();
        
        let pattern = analyze_focus_exposure_pattern(&metadata).unwrap();
        
        assert_eq!(pattern.focus_planes, 20);
        assert_eq!(pattern.exposures_per_plane, 3);
        assert_eq!(pattern.total_images, 60);
        assert_eq!(pattern.representative_indices.len(), 20);
    }
    
    #[test]
    fn test_detect_3_exposures_7_focus() {
        // Simulate 7 focus planes × 3 exposures = 21 images
        let metadata: Vec<FrameMetadata> = (0..21)
            .map(|i| {
                let exp_idx = i % 3;
                let shutter_speed = match exp_idx {
                    0 => 1.0 / 60.0,
                    1 => 1.0 / 125.0,
                    2 => 1.0 / 250.0,
                    _ => 1.0 / 60.0,
                };
                
                FrameMetadata {
                    path: PathBuf::from(format!("image_{:04}.nef", i)),
                    timestamp_ms: i as u64 * 100,
                    shutter_speed,
                }
            })
            .collect();
        
        let pattern = analyze_focus_exposure_pattern(&metadata).unwrap();
        
        assert_eq!(pattern.focus_planes, 7);
        assert_eq!(pattern.exposures_per_plane, 3);
        assert_eq!(pattern.total_images, 21);
    }
}

