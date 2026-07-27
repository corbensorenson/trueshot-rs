//! Selective Bayer loading from Nikon Z9 NEF files using custom parser.
//!
//! This module provides TRUE selective loading of raw Bayer data from specific regions
//! of interest, using the Z9NefParser to decompress only the required ROI.

use crate::nef::parser::{Z9Metadata, Z9NefParser};
use crate::nef::raw_data::{RawBuffer, Roi};
use crate::types::{BayerFrame, FrameMeta, Rect};
use anyhow::{Context, Result};
use ndarray::Array3;
use std::path::Path;

/// Sidecar-free result for high-throughput NEF crop pipelines.
///
/// Pixels remain in their native `u16` CFA representation so callers do not
/// pay for `f64` expansion unless a later algorithm actually requires it.
pub struct NativeNefRoi {
    pub raw: RawBuffer,
    pub rect: Rect,
    pub metadata: Z9Metadata,
}

/// Load full Bayer frame from Z9 NEF file
///
/// Decodes the complete raw image and converts to linear f64 RGGB format.
/// Black level is subtracted and values are normalized to [0, 1].
pub fn load_bayer_frame(path: &Path) -> Result<BayerFrame> {
    tracing::info!("Loading full Bayer frame from {:?}", path);

    let start = std::time::Instant::now();

    // Parse NEF file
    let mut parser = Z9NefParser::new(path);
    parser
        .parse()
        .with_context(|| format!("Failed to parse NEF file: {:?}", path))?;

    let metadata = parser
        .get_metadata()
        .context("Failed to get NEF metadata")?;

    tracing::debug!(
        "NEF metadata: {}x{}, {} bits, compression={}",
        metadata.width,
        metadata.height,
        metadata.bits_per_sample,
        metadata.compression
    );

    // Load full raw data
    let raw_buffer = parser.load_full().context("Failed to load full raw data")?;

    tracing::debug!(
        "Loaded {} pixels in {:.1}ms",
        raw_buffer.data.len(),
        start.elapsed().as_secs_f64() * 1000.0
    );

    // Convert to BayerFrame
    convert_raw_buffer_to_bayer_frame(raw_buffer, metadata, path)
}

/// Load selective region from Bayer frame (TRUE ROI optimization)
///
/// Uses Z9NefParser to emit only the specified rectangle and stop entropy
/// decoding after its final row.
pub fn selective_bayer_load(path: &Path, rect: &Rect) -> Result<BayerFrame> {
    tracing::info!("Selective load from {:?}, rect: {:?}", path, rect);

    let start = std::time::Instant::now();

    // Parse NEF file
    let mut parser = Z9NefParser::new(path);
    parser
        .parse()
        .with_context(|| format!("Failed to parse NEF file: {:?}", path))?;

    let metadata = parser
        .get_metadata()
        .context("Failed to get NEF metadata")?;

    // Create ROI from Rect
    let (x0, y0, x1, y1) = rect.to_bounds();
    let roi = Roi::new(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);

    tracing::debug!(
        "ROI: {}x{} at ({}, {}) - {:.1}% of full image",
        roi.width,
        roi.height,
        roi.x,
        roi.y,
        (roi.width * roi.height) as f64 / (metadata.width * metadata.height) as f64 * 100.0
    );

    // TRUE selective loading - only decompresses ROI!
    let raw_buffer = parser.load_roi(&roi, None).context("Failed to load ROI")?;

    tracing::info!(
        "Loaded ROI: {}x{} ({} pixels) in {:.1}ms",
        raw_buffer.width,
        raw_buffer.height,
        raw_buffer.data.len(),
        start.elapsed().as_secs_f64() * 1000.0
    );

    // Convert to BayerFrame
    convert_raw_buffer_to_bayer_frame(raw_buffer, metadata, path)
}

/// Convert RawBuffer to BayerFrame
fn convert_raw_buffer_to_bayer_frame(
    raw_buffer: RawBuffer,
    metadata: &crate::nef::parser::Z9Metadata,
    path: &Path,
) -> Result<BayerFrame> {
    let height = raw_buffer.height as usize;
    let width = raw_buffer.width as usize;

    let sensor_levels = metadata.sensor_levels.with_context(|| {
        format!(
            "No verified sensor calibration for {} {} {}-bit RAW",
            metadata.camera_make, metadata.camera_model, metadata.bits_per_sample
        )
    })?;
    let black_level = f64::from(sensor_levels.black);
    let white_level = f64::from(sensor_levels.white);
    if white_level <= black_level {
        anyhow::bail!(
            "Invalid sensor calibration: black={} white={}",
            black_level,
            white_level
        );
    }

    // Calculate exposure scale FIRST (for HDR)
    let shutter_speed = metadata.exposure_time.unwrap_or(1.0 / 125.0);
    let iso = metadata.iso.unwrap_or(100) as u16;
    let aperture = metadata.aperture.unwrap_or(5.6) as f64;

    let exposure_ev = (shutter_speed / (1.0 / 125.0)).log2() + (aperture / 5.6).powi(2).log2()
        - (iso as f64 / 100.0).log2();

    // For HDR: scale pixel values by exposure to preserve relative brightness
    let exposure_scale = 2.0f64.powf(exposure_ev);
    tracing::info!(
        "Exposure: EV={:.2}, scale={:.3}x (shutter={:.6}s, ISO={}, f/{:.1})",
        exposure_ev,
        exposure_scale,
        shutter_speed,
        iso,
        aperture
    );

    // Create output array (H×W×1 for single-channel Bayer data)
    // This is the CORRECT format for Bayer CFA data - all pixels in one channel
    // The CFA pattern is determined by pixel position (row, col), not by channel
    let mut bayer = Array3::<f64>::zeros((height, width, 1));

    // Convert u16 data to f64 Bayer array
    // RGGB pattern: each pixel has ONE value, color determined by position
    for y in 0..height {
        for x in 0..width {
            let pixel_value = raw_buffer.get_pixel(x as u32, y as u32).unwrap_or(0) as f64;

            // Subtract black level and normalize
            let linear = ((pixel_value - black_level) / (white_level - black_level)).max(0.0);

            // Scale by exposure to preserve HDR information
            // This ensures all exposures are on the same brightness scale
            let hdr_scaled = linear * exposure_scale;

            // ALL pixels go in channel 0 (single-channel Bayer format)
            bayer[[y, x, 0]] = hdr_scaled;
        }
    }

    // BISECT DEBUG: Check raw Bayer data from NEF
    let mut r_sum = 0.0;
    let mut g_sum = 0.0;
    let mut b_sum = 0.0;
    let mut r_count = 0;
    let mut g_count = 0;
    let mut b_count = 0;

    for y in 0..height {
        for x in 0..width {
            let val = bayer[[y, x, 0]];
            match (y % 2, x % 2) {
                (0, 0) => {
                    r_sum += val;
                    r_count += 1;
                } // R
                (0, 1) | (1, 0) => {
                    g_sum += val;
                    g_count += 1;
                } // G
                (1, 1) => {
                    b_sum += val;
                    b_count += 1;
                } // B
                _ => {}
            }
        }
    }

    let r_avg = r_sum / r_count as f64;
    let g_avg = g_sum / g_count as f64;
    let b_avg = b_sum / b_count as f64;

    tracing::debug!(
        "Raw Bayer stats (file: {:?}): R={:.4}, G={:.4}, B={:.4}, R/G={:.3}, B/G={:.3}",
        path.file_name(),
        r_avg,
        g_avg,
        b_avg,
        r_avg / g_avg,
        b_avg / g_avg
    );

    // Create FrameMeta (exposure values already calculated above)
    let focal_length = metadata.focal_length.unwrap_or(105.0) as f64;

    // Parse filename for rotation and vantage
    let filename = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let (rotation_deg, vantage) = parse_filename_metadata(filename);

    let meta = FrameMeta {
        path: path.to_path_buf(),
        focus_position: 0, // Will be set by sequence grouper
        focus_step: 0,     // Will be set by sequence grouper
        exposure_ev,
        shutter_speed,
        iso,
        aperture,
        focal_length,
        rotation_deg,
        vantage,
        black_level,
        cam_mul: metadata.cam_mul, // Camera white balance multipliers
    };

    Ok(BayerFrame { data: bayer, meta })
}

/// Parse rotation and vantage from filename
/// Expected format: "bone1_low_000.nef" or similar
fn parse_filename_metadata(filename: &str) -> (f32, String) {
    let parts: Vec<&str> = filename.split('_').collect();

    let vantage = if parts.len() > 1 {
        parts[1].to_string()
    } else {
        "mid".to_string()
    };

    let rotation = if parts.len() > 2 {
        parts[2].parse::<u32>().unwrap_or(0) as f32 * 10.0 // Assume 10° increments
    } else {
        0.0
    };

    (rotation, vantage)
}

/// Load Bayer frame with automatic object detection and selective loading
///
/// Detects the object bounding box from preview, then loads only that region.
/// Falls back to full load if detection fails.
pub fn load_bayer_frame_with_detection(path: &Path) -> Result<BayerFrame> {
    tracing::info!("Loading with automatic object detection: {:?}", path);

    let detected = load_detected_nef_roi(path)?;
    convert_raw_buffer_to_bayer_frame(detected.raw, &detected.metadata, path)
}

/// Parse once, detect from the scaled embedded preview, and decode the native
/// Bayer crop without creating a persistent per-image index.
///
/// This is for ungrouped files. HDR/focus groups should detect one reference
/// frame and call [`load_nef_roi_native`] with the shared rectangle.
pub fn load_detected_nef_roi(path: &Path) -> Result<NativeNefRoi> {
    let mut parser = Z9NefParser::new(path);
    parser
        .parse()
        .with_context(|| format!("Failed to parse NEF file: {:?}", path))?;

    let rect = match crate::object_detection::detect_object_bbox_with_parser(&mut parser) {
        Ok(rect) => {
            tracing::info!("Object detected, using selective loading");
            rect
        }
        Err(e) => {
            tracing::warn!("Object detection failed ({}), falling back to full load", e);
            let metadata = parser.get_metadata()?;
            Rect::new(0.0, 0.0, metadata.width as f64, metadata.height as f64)
        }
    };

    load_nef_roi_with_parser(&parser, path, rect)
}

/// Decode a known crop directly to native `u16` CFA pixels.
///
/// Use this for every frame in an HDR/focus group after deriving one crop from
/// the group's `SequenceCropPlan`.
pub fn load_nef_roi_native(path: &Path, rect: Rect) -> Result<NativeNefRoi> {
    let mut parser = Z9NefParser::new(path);
    parser
        .parse()
        .with_context(|| format!("Failed to parse NEF file: {:?}", path))?;
    load_nef_roi_with_parser(&parser, path, rect)
}

/// Decode a known crop directly into caller-owned native CFA storage.
///
/// Returns the parsed metadata while avoiding the `RawBuffer` allocation used
/// by standalone image loads.
pub fn load_nef_roi_into(path: &Path, rect: Rect, destination: &mut [u16]) -> Result<Z9Metadata> {
    let mut parser = Z9NefParser::new(path);
    parser
        .parse()
        .with_context(|| format!("Failed to parse NEF file: {:?}", path))?;
    let roi = roi_from_rect(rect)?;
    parser
        .load_roi_into(&roi, destination)
        .with_context(|| format!("Failed to decode NEF ROI into group storage: {:?}", path))?;
    Ok(parser.get_metadata()?.clone())
}

fn load_nef_roi_with_parser(parser: &Z9NefParser, path: &Path, rect: Rect) -> Result<NativeNefRoi> {
    let roi = roi_from_rect(rect)?;
    let raw = parser
        .load_roi(&roi, None)
        .with_context(|| format!("Failed to load NEF ROI from {:?}", path))?;
    let metadata = parser.get_metadata()?.clone();
    Ok(NativeNefRoi {
        raw,
        rect,
        metadata,
    })
}

fn roi_from_rect(rect: Rect) -> Result<Roi> {
    let (x0, y0, x1, y1) = rect.to_bounds();
    let width = x1.checked_sub(x0).context("NEF ROI has negative width")?;
    let height = y1.checked_sub(y0).context("NEF ROI has negative height")?;
    if width == 0 || height == 0 {
        anyhow::bail!("NEF ROI is empty");
    }
    Ok(Roi::new(
        u32::try_from(x0).context("NEF ROI x exceeds u32")?,
        u32::try_from(y0).context("NEF ROI y exceeds u32")?,
        u32::try_from(width).context("NEF ROI width exceeds u32")?,
        u32::try_from(height).context("NEF ROI height exceeds u32")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rect_bounds() {
        let rect = Rect::new(10.5, 20.3, 100.0, 200.0);
        let (x0, y0, x1, y1) = rect.to_bounds();
        assert_eq!(x0, 10);
        assert_eq!(y0, 20);
        assert_eq!(x1, 111);
        assert_eq!(y1, 221);
    }

    #[test]
    fn test_rect_adjust() {
        let rect = Rect::new(10.0, 10.0, 100.0, 100.0);
        let eroded = rect.adjust(-0.1);
        assert!(eroded.width < rect.width);
        assert!(eroded.height < rect.height);
    }
}
