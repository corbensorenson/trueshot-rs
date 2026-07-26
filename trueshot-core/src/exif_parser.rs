//! EXIF parsing and sequence grouping for Nikon Z9 raw stacks.
//!
//! Groups NEF files into sequences based on TIME-BASED grouping (30s window)
//! and exposure/focus pattern detection (F1E1, F1E2, F1E3, F2E1, ...).

use crate::types::{Meta, Sequence};
use crate::nef::parser::Z9NefParser;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Group NEF files into sequences using TIME-BASED grouping
///
/// Uses the proven approach from trueshot-core and original pixelcollapse:
/// 1. Extract metadata using Z9NefParser (fast, no image decode)
/// 2. Sort by timestamp
/// 3. Group by 30-second time windows
/// 4. Detect focus steps and exposures within each group
/// 5. Apply pattern: F1E1, F1E2, F1E3, F2E1, F2E2, F2E3, ...
pub fn group_sequences(paths: &[PathBuf]) -> Result<Vec<Sequence>> {
    tracing::info!("Grouping {} files into sequences (time-based)", paths.len());

    // Step 1: Extract metadata from all files using Z9NefParser (fast!)
    let mut file_metas = Vec::new();
    for path in paths {
        match extract_z9_metadata(path) {
            Ok(meta) => file_metas.push(meta),
            Err(e) => {
                tracing::warn!("Failed to extract metadata from {:?}: {}", path, e);
            }
        }
    }

    if file_metas.is_empty() {
        return Ok(Vec::new());
    }

    // Step 2: Sort by timestamp
    file_metas.sort_by_key(|m| m.timestamp_ms);

    tracing::debug!("Sorted {} files by timestamp", file_metas.len());
    tracing::debug!("First file: {:?} at {}ms",
                   file_metas[0].path.file_name(), file_metas[0].timestamp_ms);
    tracing::debug!("Last file: {:?} at {}ms",
                   file_metas.last().unwrap().path.file_name(),
                   file_metas.last().unwrap().timestamp_ms);

    // Step 3: Group by 30-second time windows
    let time_groups = group_by_time_window(&file_metas, 30_000); // 30 seconds in ms

    tracing::info!("Created {} time-based groups", time_groups.len());

    // Step 4: Convert each time group to a sequence
    let mut sequences = Vec::new();
    for (group_idx, group) in time_groups.into_iter().enumerate() {
        match create_sequence_from_group(group, group_idx) {
            Ok(seq) => sequences.push(seq),
            Err(e) => {
                tracing::warn!("Failed to create sequence from group {}: {}", group_idx, e);
            }
        }
    }

    tracing::info!("Created {} sequences", sequences.len());
    Ok(sequences)
}

/// File metadata extracted from Z9 NEF
#[derive(Debug, Clone)]
pub struct FileMeta {
    pub path: PathBuf,
    pub timestamp_ms: u64,      // Milliseconds since epoch
    pub exposure_time: f64,     // Seconds (shutter speed)
    pub aperture: f64,          // F-number
    pub iso: u32,
    pub exposure_ev: f64,       // Calculated EV
    pub cam_mul: [f32; 4],      // Camera white balance multipliers
}

/// Extract metadata using Z9NefParser (fast, no image decode)
pub fn extract_z9_metadata(path: &Path) -> Result<FileMeta> {
    let mut parser = Z9NefParser::new(path);
    parser.parse()
        .with_context(|| format!("Failed to parse NEF: {:?}", path))?;

    let metadata = parser.get_metadata()
        .with_context(|| format!("Failed to get metadata: {:?}", path))?;

    // Extract timestamp (milliseconds since epoch)
    let timestamp_ms = metadata.timestamp
        .map(|t| t.timestamp_millis() as u64)
        .unwrap_or(0);

    let exposure_time = metadata.exposure_time.unwrap_or(1.0 / 125.0);
    let aperture = metadata.aperture.unwrap_or(5.6) as f64;
    let iso = metadata.iso.unwrap_or(100);

    // Calculate EV (relative to 1/125s, f/5.6, ISO 100)
    let exposure_ev = (exposure_time / (1.0 / 125.0)).log2()
        + (aperture / 5.6).powi(2).log2()
        - (iso as f64 / 100.0).log2();

    Ok(FileMeta {
        path: path.to_path_buf(),
        timestamp_ms,
        exposure_time,
        aperture,
        iso,
        exposure_ev,
        cam_mul: metadata.cam_mul,
    })
}

/// Group files by time window (30 seconds)
///
/// Files within 30 seconds of each other are considered part of the same sequence.
/// This is the proven approach from trueshot-core.
fn group_by_time_window(metas: &[FileMeta], window_ms: u64) -> Vec<Vec<FileMeta>> {
    if metas.is_empty() {
        return Vec::new();
    }

    let mut groups = Vec::new();
    let mut current_group = vec![metas[0].clone()];
    let mut last_timestamp = metas[0].timestamp_ms;

    for meta in &metas[1..] {
        let time_diff = meta.timestamp_ms.saturating_sub(last_timestamp);

        if time_diff > window_ms {
            // Time gap detected - start new group
            tracing::debug!("Time gap detected: {}ms, starting new group", time_diff);
            groups.push(std::mem::take(&mut current_group));
        }

        current_group.push(meta.clone());
        last_timestamp = meta.timestamp_ms;
    }

    if !current_group.is_empty() {
        groups.push(current_group);
    }

    groups
}

/// Create a sequence from a time-based group
///
/// Detects focus steps and exposures using SHUTTER SPEED (not EV).
/// This is the proven approach from trueshot-core and original pixelcollapse.
/// Pattern: F1E1, F1E2, F1E3, F2E1, F2E2, F2E3, ..., F7E1, F7E2, F7E3
fn create_sequence_from_group(group: Vec<FileMeta>, group_idx: usize) -> Result<Sequence> {
    if group.is_empty() {
        anyhow::bail!("Empty group");
    }

    tracing::debug!("Creating sequence from group {} with {} files", group_idx, group.len());

    // Detect unique SHUTTER SPEEDS (exposure_time in seconds)
    // This is the key - we group by shutter speed, not EV!
    let mut unique_shutter_speeds: Vec<f64> = group.iter()
        .map(|m| m.exposure_time)
        .collect();
    unique_shutter_speeds.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // Deduplicate with tolerance (0.0001 seconds = 0.1ms)
    unique_shutter_speeds.dedup_by(|a, b| (*a - *b).abs() < 0.0001);

    let num_exposures = unique_shutter_speeds.len();
    let total_images = group.len();

    tracing::debug!("Unique shutter speeds: {:?}",
                   unique_shutter_speeds.iter()
                       .map(|t| format!("1/{:.0}", 1.0/t))
                       .collect::<Vec<_>>());

    // Calculate focus steps: total_images / num_exposures
    // Pattern: F1E1, F1E2, F1E3, F2E1, F2E2, F2E3, ..., F7E1, F7E2, F7E3
    let (focus_steps, exposures, shutter_speeds) = if num_exposures > 0 && total_images % num_exposures == 0 {
        // Perfect pattern detected
        let focus_steps = total_images / num_exposures;

        // Convert shutter speeds to EV values for Meta (DEPRECATED - kept for compatibility)
        let evs: Vec<f64> = unique_shutter_speeds.iter()
            .map(|&exp_time| {
                // Calculate EV relative to reference (1/125s, f/5.6, ISO 100)
                (exp_time / (1.0 / 125.0)).log2()
            })
            .collect();

        (focus_steps, evs, unique_shutter_speeds.clone())
    } else {
        // Fallback: assume all focus stacking, no exposure bracketing
        tracing::warn!("No clear pattern detected, assuming {} focus steps × 1 exposure", total_images);
        (total_images, vec![0.0], vec![1.0 / 125.0])
    };

    tracing::info!("Detected: {} focus steps × {} exposures = {} images",
                  focus_steps, num_exposures, total_images);

    // Parse filename for metadata (from first file)
    let first_path = &group[0].path;
    let filename = first_path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let (rotation_deg, vantage, bone_id) = parse_filename(filename);

    // Extract camera white balance from first file
    let cam_mul = group.first()
        .map(|m| m.cam_mul)
        .unwrap_or([1.0, 1.0, 1.0, 1.0]);

    // Create sequence metadata
    let seq_meta = Meta {
        focus_steps: focus_steps as u8,
        exposures,
        shutter_speeds,
        ref_focus: (focus_steps / 2) as u8, // Middle focus
        ref_exp: 0.0, // Middle exposure (closest to 0 EV)
        rot_deg: rotation_deg,
        vantage,
        burst_factor: 1,
        bone_id,
        cam_mul,
    };

    let sequence = Sequence {
        paths: group.iter().map(|m| m.path.clone()).collect(),
        meta: seq_meta,
    };

    Ok(sequence)
}

/// Parse filename for metadata
/// Expected formats:
/// - "bone1_low_000.nef"
/// - "_Z9Z5232.NEF" (generic)
fn parse_filename(filename: &str) -> (f32, String, String) {
    let parts: Vec<&str> = filename.split('_').collect();

    if parts.len() >= 3 {
        // Format: bone1_low_000
        let bone_id = parts[0].to_string();
        let vantage = parts[1].to_string();
        let rotation = parts[2].parse::<u32>().unwrap_or(0) as f32 * 10.0;
        (rotation, vantage, bone_id)
    } else {
        // Generic format: use filename as bone_id
        let bone_id = filename.to_string();
        (0.0, "mid".to_string(), bone_id)
    }
}

/// Scan directory for NEF files (recursively)
pub fn scan_nef_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut nef_files = Vec::new();
    scan_nef_files_recursive(dir, &mut nef_files)?;
    nef_files.sort();
    Ok(nef_files)
}

/// Recursively scan directory for NEF files
fn scan_nef_files_recursive(dir: &Path, nef_files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext.eq_ignore_ascii_case("nef") {
                    nef_files.push(path);
                }
            }
        } else if path.is_dir() {
            // Recursively scan subdirectories
            scan_nef_files_recursive(&path, nef_files)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_filename() {
        let (rot, vantage, bone_id) = parse_filename("bone1_low_000");
        assert_eq!(rot, 0.0);
        assert_eq!(vantage, "low");
        assert_eq!(bone_id, "bone1");
        
        let (rot, vantage, bone_id) = parse_filename("bone2_high_036");
        assert_eq!(rot, 360.0);
        assert_eq!(vantage, "high");
        assert_eq!(bone_id, "bone2");
    }

    #[test]
    fn test_parse_generic_filename() {
        let (rot, vantage, bone_id) = parse_filename("_Z9Z5232");
        assert_eq!(rot, 0.0);
        assert_eq!(vantage, "mid");
        assert_eq!(bone_id, "_Z9Z5232");
    }
}

