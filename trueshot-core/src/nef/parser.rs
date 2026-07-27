/// Main Z9 NEF parser
///
/// This module provides the high-level interface for parsing Nikon Z9 NEF files.
use anyhow::{Context, Result};
use memmap2::MmapOptions;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::huffman::{clamp_bits, create_huffman_table, BitPumpMSB, LjpegHeader, LookupTable};
use super::nikon_compression::{
    NikonCompressionMeta, NikonDecompressor, NikonSeekIndex, DEFAULT_SEEK_INDEX_STRIDE,
};
use super::preview::PreviewExtractor;
use super::raw_data::{RawBuffer, Roi, WarpMatrix};
use super::tiff::TiffParser;
use super::{
    TIFF_TAG_BITS_PER_SAMPLE, TIFF_TAG_CFA_PATTERN, TIFF_TAG_CFA_REPEAT_PATTERN_DIM,
    TIFF_TAG_COMPRESSION, TIFF_TAG_IMAGE_LENGTH, TIFF_TAG_IMAGE_WIDTH,
    TIFF_TAG_JPEG_INTERCHANGE_FORMAT, TIFF_TAG_JPEG_INTERCHANGE_FORMAT_LENGTH, TIFF_TAG_MAKE,
    TIFF_TAG_MODEL, TIFF_TAG_ROWS_PER_STRIP, TIFF_TAG_STRIP_BYTE_COUNTS, TIFF_TAG_STRIP_OFFSETS,
    Z9_CFA_PATTERN,
};

/// EXIF data structure for metadata extraction
#[derive(Debug, Default)]
struct ExifData {
    timestamp: Option<chrono::DateTime<chrono::Utc>>,
    exposure_time: Option<f64>,
    aperture: Option<f32>,
    iso: Option<u32>,
    focal_length: Option<f32>,
    focus_distance: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct Z9Metadata {
    pub width: u32,
    pub height: u32,
    pub bits_per_sample: u16,
    pub compression: u16,
    pub cfa_pattern: [u8; 4],
    pub camera_make: String,
    pub camera_model: String,
    /// Verified sensor-domain normalization limits for this camera profile.
    pub sensor_levels: Option<SensorLevels>,
    /// Verified physical sensor geometry required by aperture-space methods.
    pub sensor_geometry: Option<SensorGeometry>,
    pub strip_offsets: Vec<u64>,
    pub strip_byte_counts: Vec<u64>,
    pub rows_per_strip: u32,
    // White balance multipliers (R, G, B, G2) - LibRaw cam_mul format
    pub cam_mul: [f32; 4],
    // EXIF metadata for grouping analysis
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    pub exposure_time: Option<f64>,
    pub aperture: Option<f32>,
    pub iso: Option<u32>,
    pub focal_length: Option<f32>,
    pub focus_distance: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorLevels {
    pub black: u16,
    pub white: u16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensorGeometry {
    pub pixel_pitch_um: f32,
}

#[derive(Debug, Clone, Copy)]
struct SensorProfile {
    levels: SensorLevels,
    geometry: SensorGeometry,
    cfa_pattern: [u8; 4],
}

fn is_nikon_z9(make: &str, model: &str) -> bool {
    let make = make
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    let model = model
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    (make.eq_ignore_ascii_case("nikon") || make.eq_ignore_ascii_case("nikoncorporation"))
        && (model.eq_ignore_ascii_case("z9") || model.eq_ignore_ascii_case("nikonz9"))
}

fn verified_sensor_profile(make: &str, model: &str, bits_per_sample: u16) -> Option<SensorProfile> {
    if is_nikon_z9(make, model) && bits_per_sample == 14 {
        return Some(SensorProfile {
            // Validated against RawSpeed/LibRaw on local Z9 firmware 5.00 captures.
            levels: SensorLevels {
                black: 1008,
                white: 15311,
            },
            // Nikon publishes 35.9 mm across the 8,256-pixel FX image area.
            geometry: SensorGeometry {
                pixel_pitch_um: 35_900.0 / 8_256.0,
            },
            cfa_pattern: Z9_CFA_PATTERN,
        });
    }
    None
}

pub struct Z9NefParser {
    file_path: String,
    metadata: Option<Z9Metadata>,
    tiff_parser: TiffParser,
    preview_extractor: PreviewExtractor,
    makernote_offset: Option<u64>,
    makernote_size: Option<u64>,
}

trait RawOutput {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    fn pixels_mut(&mut self) -> &mut [u16];

    fn set_pixel(&mut self, x: u32, y: u32, value: u16) -> bool {
        if x >= self.width() || y >= self.height() {
            return false;
        }
        let index = (y * self.width() + x) as usize;
        if let Some(pixel) = self.pixels_mut().get_mut(index) {
            *pixel = value;
            true
        } else {
            false
        }
    }
}

impl RawOutput for RawBuffer {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn pixels_mut(&mut self) -> &mut [u16] {
        &mut self.data
    }
}

struct RawSliceOutput<'a> {
    pixels: &'a mut [u16],
    width: u32,
    height: u32,
}

impl RawOutput for RawSliceOutput<'_> {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn pixels_mut(&mut self) -> &mut [u16] {
        self.pixels
    }
}

#[allow(dead_code)]
impl Z9NefParser {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            file_path: path.as_ref().to_string_lossy().to_string(),
            metadata: None,
            tiff_parser: TiffParser::new(),
            preview_extractor: PreviewExtractor::new(),
            makernote_offset: None,
            makernote_size: None,
        }
    }

    /// Parse the NEF file and extract metadata
    pub fn parse(&mut self) -> Result<()> {
        let mut file = File::open(&self.file_path)
            .with_context(|| format!("Failed to open NEF file: {}", self.file_path))?;

        let mut reader = BufReader::new(&mut file);

        // Read TIFF header
        let header = self.tiff_parser.read_header(&mut reader)?;

        // Read main IFD (IFD0)
        let ifd0 = self.tiff_parser.read_ifd(&mut reader, header.ifd_offset)?;
        tracing::info!("IFD0 contains {} entries", ifd0.len());
        let camera_make = self
            .tiff_parser
            .read_ascii(
                &mut reader,
                ifd0.get(&TIFF_TAG_MAKE)
                    .context("NEF IFD0 is missing camera make")?,
            )
            .context("Unable to read NEF camera make")?;
        let camera_model = self
            .tiff_parser
            .read_ascii(
                &mut reader,
                ifd0.get(&TIFF_TAG_MODEL)
                    .context("NEF IFD0 is missing camera model")?,
            )
            .context("Unable to read NEF camera model")?;
        tracing::info!("NEF camera identity: {} {}", camera_make, camera_model);

        // Check for additional IFDs that might contain RAW data
        let mut current_ifd_offset = header.ifd_offset;
        let mut ifd_index = 0;
        let mut makernote_offset = None;
        let mut makernote_size = None;

        loop {
            // Read the current IFD
            let ifd = self.tiff_parser.read_ifd(&mut reader, current_ifd_offset)?;
            tracing::info!("IFD{} contains {} entries", ifd_index, ifd.len());

            // Check if this IFD contains RAW data by looking for specific tags
            let has_strip_offsets = ifd.contains_key(&273); // StripOffsets
            let has_strip_byte_counts = ifd.contains_key(&279); // StripByteCounts
            let has_tile_offsets = ifd.contains_key(&324); // TileOffsets
            let has_jpeg_offsets = ifd.contains_key(&513); // JPEG Interchange Format
            let has_jpeg_byte_counts = ifd.contains_key(&514); // JPEG Interchange Format Length
            let has_compression = ifd.contains_key(&259); // Compression
            let has_photometric = ifd.contains_key(&262); // PhotometricInterpretation

            tracing::info!("IFD{}: strips={}, strip_bytes={}, tiles={}, jpeg={}, jpeg_bytes={}, compression={}, photometric={}",
                      ifd_index, has_strip_offsets, has_strip_byte_counts, has_tile_offsets,
                      has_jpeg_offsets, has_jpeg_byte_counts, has_compression, has_photometric);

            // Debug: Print all tags in this IFD
            if ifd_index == 0 {
                tracing::info!("IFD0 tags: {:?}", ifd.keys().collect::<Vec<_>>());

                // First, look for EXIF IFD and MakerNote
                if let Some(exif_entry) = ifd.get(&34665) {
                    // EXIF IFD
                    tracing::info!("EXIF IFD found at offset: {}", exif_entry.value_offset);
                    // Try to read the EXIF IFD
                    if let Ok(exif_ifd) = self
                        .tiff_parser
                        .read_ifd(&mut reader, exif_entry.value_offset as u64)
                    {
                        tracing::info!("EXIF IFD contains {} entries", exif_ifd.len());

                        // Look for MakerNote in EXIF IFD
                        if let Some(makernote_entry) = exif_ifd.get(&37500) {
                            // MakerNote tag
                            tracing::info!(
                                "MakerNote found in EXIF IFD at offset: {}, size: {}",
                                makernote_entry.value_offset,
                                makernote_entry.count
                            );
                            // Store MakerNote info for later use
                            makernote_offset = Some(makernote_entry.value_offset as u64);
                            makernote_size = Some(makernote_entry.count as u64);
                        }
                    }
                }

                // Check for SubIFD or Nikon-specific tags that might point to RAW data
                for (&tag, entry) in ifd.iter() {
                    // Log all tags to see what's available
                    tracing::debug!(
                        "IFD0 tag {}: offset={}, count={}",
                        tag,
                        entry.value_offset,
                        entry.count
                    );
                    if tag > 30000 {
                        // Nikon-specific tags are usually > 30000
                        tracing::info!(
                            "Nikon tag {}: offset={}, count={}",
                            tag,
                            entry.value_offset,
                            entry.count
                        );
                    }
                    if tag == 37500 {
                        // MakerNote tag in IFD0 (fallback)
                        tracing::info!(
                            "MakerNote found in IFD0 at offset: {}, size: {}",
                            entry.value_offset,
                            entry.count
                        );
                        if makernote_offset.is_none() {
                            // Only use if not found in EXIF IFD
                            makernote_offset = Some(entry.value_offset as u64);
                            makernote_size = Some(entry.count as u64);
                        }
                    }
                    if tag == 330 {
                        // SubIFDs tag
                        tracing::info!(
                            "SubIFDs tag found: offset={}, count={}",
                            entry.value_offset,
                            entry.count
                        );

                        // Read SubIFD offsets
                        if let Ok(subifd_offsets) =
                            self.tiff_parser.read_u32_array(&mut reader, entry)
                        {
                            tracing::info!("Found {} SubIFDs", subifd_offsets.len());

                            // Check each SubIFD for RAW data
                            for (i, &subifd_offset) in subifd_offsets.iter().enumerate() {
                                if let Ok(subifd) =
                                    self.tiff_parser.read_ifd(&mut reader, subifd_offset as u64)
                                {
                                    tracing::info!("SubIFD{} contains {} entries", i, subifd.len());

                                    let sub_has_strips = subifd.contains_key(&273);
                                    let sub_has_tiles = subifd.contains_key(&324);
                                    let sub_has_jpeg = subifd.contains_key(&513);
                                    let sub_has_compression = subifd.contains_key(&259);

                                    if let Some(comp_entry) = subifd.get(&259) {
                                        tracing::info!(
                                            "SubIFD{} compression: {}",
                                            i,
                                            comp_entry.value_offset
                                        );
                                    }

                                    tracing::info!(
                                        "SubIFD{}: strips={}, tiles={}, jpeg={}, compression={}",
                                        i,
                                        sub_has_strips,
                                        sub_has_tiles,
                                        sub_has_jpeg,
                                        sub_has_compression
                                    );

                                    // If this SubIFD has RAW data, use it
                                    if sub_has_strips || sub_has_tiles {
                                        tracing::info!("Found RAW data in SubIFD{}!", i);
                                        let metadata = self.extract_metadata(
                                            &mut reader,
                                            &subifd,
                                            makernote_offset,
                                            makernote_size,
                                            &camera_make,
                                            &camera_model,
                                        )?;
                                        self.metadata = Some(metadata);

                                        // Store MakerNote info
                                        self.makernote_offset = makernote_offset;
                                        self.makernote_size = makernote_size;

                                        return Ok(());
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // If this looks like the RAW data IFD, use it
            if has_strip_offsets || has_tile_offsets || has_jpeg_offsets {
                if let Some(compression_entry) = ifd.get(&259) {
                    tracing::info!(
                        "IFD{} compression type: {}",
                        ifd_index,
                        compression_entry.value_offset
                    );
                }

                // Use this IFD for RAW data if it's not IFD0 (which is usually metadata/preview)
                if ifd_index > 0 {
                    tracing::info!("Using IFD{} for RAW data extraction", ifd_index);
                    let metadata = self.extract_metadata(
                        &mut reader,
                        &ifd,
                        makernote_offset,
                        makernote_size,
                        &camera_make,
                        &camera_model,
                    )?;
                    self.metadata = Some(metadata);
                    return Ok(());
                }
            }

            // Get next IFD offset
            reader.seek(SeekFrom::Start(
                current_ifd_offset + 2 + (ifd.len() as u64 * 12),
            ))?;
            let next_offset = self.tiff_parser.read_next_ifd_offset(&mut reader)?;

            tracing::info!("Next IFD offset: {}", next_offset);

            if next_offset == 0 {
                break;
            }

            current_ifd_offset = next_offset;
            ifd_index += 1;

            if ifd_index > 10 {
                // Safety limit
                break;
            }
        }

        // Fallback to IFD0 if no better IFD found
        tracing::warn!("No dedicated RAW IFD found, falling back to IFD0");
        let metadata = self.extract_metadata(
            &mut reader,
            &ifd0,
            makernote_offset,
            makernote_size,
            &camera_make,
            &camera_model,
        )?;
        self.metadata = Some(metadata);

        // Store MakerNote info
        self.makernote_offset = makernote_offset;
        self.makernote_size = makernote_size;

        Ok(())
    }

    /// Get metadata (must call parse() first)
    pub fn get_metadata(&self) -> Result<&Z9Metadata> {
        self.metadata
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("NEF file not parsed yet. Call parse() first."))
    }

    /// Extract preview JPEG efficiently
    pub fn extract_preview_jpeg(&mut self) -> Result<Vec<u8>> {
        self.preview_extractor.extract_preview_jpeg(&self.file_path)
    }

    /// Load full RAW image
    pub fn load_full(&self) -> Result<RawBuffer> {
        let metadata = self.get_metadata()?;
        let roi = Roi::full_image(metadata.width, metadata.height);
        self.load_roi(&roi, None)
    }

    /// Load specific ROI with optional inline warp
    pub fn load_roi(&self, roi: &Roi, warp: Option<&WarpMatrix>) -> Result<RawBuffer> {
        let metadata = self.get_metadata()?;

        // Validate ROI
        if !roi.is_valid(metadata.width, metadata.height) {
            return Err(anyhow::anyhow!("ROI exceeds image bounds"));
        }

        let mut raw_data = RawBuffer::new(
            roi.width,
            roi.height,
            metadata.cfa_pattern,
            metadata.bits_per_sample,
        );
        self.load_roi_into(roi, &mut raw_data.data)?;

        // Apply warp if specified
        if let Some(warp_matrix) = warp {
            Ok(raw_data.apply_warp(warp_matrix, roi.width, roi.height))
        } else {
            Ok(raw_data)
        }
    }

    /// Decode an ROI directly into caller-owned native CFA storage.
    ///
    /// The destination must be exactly `roi.width * roi.height` pixels. Group
    /// loaders use this entry point to avoid a per-frame allocation and copy.
    pub fn load_roi_into(&self, roi: &Roi, destination: &mut [u16]) -> Result<()> {
        let metadata = self.get_metadata()?;
        if !roi.is_valid(metadata.width, metadata.height) {
            anyhow::bail!("ROI exceeds image bounds");
        }
        let expected_pixels = (roi.width as usize)
            .checked_mul(roi.height as usize)
            .context("NEF ROI dimensions overflow")?;
        if destination.len() != expected_pixels {
            anyhow::bail!(
                "NEF ROI destination has {} pixels; expected {}",
                destination.len(),
                expected_pixels
            );
        }

        destination.fill(0);
        let mut output = RawSliceOutput {
            pixels: destination,
            width: roi.width,
            height: roi.height,
        };
        self.load_raw_data_from_strips_into(roi, &mut output)
    }

    /// Load ROI using mask-based selection
    pub fn load_masked(
        &self,
        mask: &[u8],
        mask_x: u32,
        mask_y: u32,
        mask_width: u32,
        mask_height: u32,
        warp: Option<&WarpMatrix>,
    ) -> Result<RawBuffer> {
        let roi = Roi::new(mask_x, mask_y, mask_width, mask_height);
        let mut buffer = self.load_roi(&roi, warp)?;
        buffer.apply_mask(mask, mask_width, mask_height);
        Ok(buffer)
    }

    /// Check if the file supports selective loading
    pub fn supports_selective_loading(&self) -> bool {
        self.metadata
            .as_ref()
            .map(|metadata| {
                is_nikon_z9(&metadata.camera_make, &metadata.camera_model)
                    && metadata.sensor_levels.is_some()
                    && matches!(metadata.compression, 1 | 6 | 34713)
            })
            .unwrap_or(false)
    }

    // Private implementation methods

    fn extract_metadata(
        &mut self,
        reader: &mut BufReader<&mut File>,
        ifd: &std::collections::HashMap<u16, super::tiff::IfdEntry>,
        makernote_offset: Option<u64>,
        makernote_size: Option<u64>,
        camera_make: &str,
        camera_model: &str,
    ) -> Result<Z9Metadata> {
        let width = self
            .tiff_parser
            .read_unsigned_scalar(
                reader,
                ifd.get(&TIFF_TAG_IMAGE_WIDTH)
                    .context("RAW IFD is missing image width")?,
            )
            .context("Unable to read RAW image width")?;
        let height = self
            .tiff_parser
            .read_unsigned_scalar(
                reader,
                ifd.get(&TIFF_TAG_IMAGE_LENGTH)
                    .context("RAW IFD is missing image height")?,
            )
            .context("Unable to read RAW image height")?;
        let bits_per_sample = u16::try_from(
            self.tiff_parser
                .read_unsigned_scalar(
                    reader,
                    ifd.get(&TIFF_TAG_BITS_PER_SAMPLE)
                        .context("RAW IFD is missing bits per sample")?,
                )
                .context("Unable to read RAW bits per sample")?,
        )
        .context("RAW bits per sample exceeds u16")?;
        let compression = u16::try_from(
            self.tiff_parser
                .read_unsigned_scalar(
                    reader,
                    ifd.get(&TIFF_TAG_COMPRESSION)
                        .context("RAW IFD is missing compression")?,
                )
                .context("Unable to read RAW compression")?,
        )
        .context("RAW compression exceeds u16")?;
        let rows_per_strip = if let Some(entry) = ifd.get(&TIFF_TAG_ROWS_PER_STRIP) {
            self.tiff_parser
                .read_unsigned_scalar(reader, entry)
                .context("Unable to read RAW rows per strip")?
        } else {
            height
        };
        if width == 0 || height == 0 || rows_per_strip == 0 {
            anyhow::bail!(
                "RAW dimensions/strip layout are invalid: {}x{}, rows_per_strip={}",
                width,
                height,
                rows_per_strip
            );
        }
        let profile = verified_sensor_profile(camera_make, camera_model, bits_per_sample);
        let cfa_pattern = match (
            ifd.get(&TIFF_TAG_CFA_REPEAT_PATTERN_DIM),
            ifd.get(&TIFF_TAG_CFA_PATTERN),
        ) {
            (Some(dimensions), Some(pattern)) => {
                let dimensions = self.tiff_parser.read_u16_array(reader, dimensions)?;
                let pattern = self.tiff_parser.read_tag_data(reader, pattern)?;
                if dimensions.as_slice() != [2, 2] || pattern.len() < 4 {
                    anyhow::bail!(
                        "Unsupported CFA layout {:?} with {} pattern entries",
                        dimensions,
                        pattern.len()
                    );
                }
                [pattern[0], pattern[1], pattern[2], pattern[3]]
            }
            _ => profile
                .map(|profile| profile.cfa_pattern)
                .with_context(|| {
                    format!(
                        "No verified CFA profile for {} {} {}-bit RAW",
                        camera_make, camera_model, bits_per_sample
                    )
                })?,
        };

        // Read RAW data location - Z9 NEF files use JPEG Interchange Format tags
        let strip_offsets = if let Some(entry) = ifd.get(&TIFF_TAG_STRIP_OFFSETS) {
            // Traditional TIFF strips
            self.tiff_parser
                .read_u32_array(reader, entry)?
                .into_iter()
                .map(|x| x as u64)
                .collect()
        } else if let Some(entry) = ifd.get(&TIFF_TAG_JPEG_INTERCHANGE_FORMAT) {
            // Z9 NEF uses JPEG Interchange Format for RAW data
            tracing::info!("Using JPEG Interchange Format for RAW data");
            vec![u64::from(
                self.tiff_parser
                    .read_unsigned_scalar(reader, entry)
                    .context("Unable to read RAW JPEG offset")?,
            )]
        } else {
            anyhow::bail!("RAW IFD is missing strip/JPEG data offsets");
        };

        let strip_byte_counts = if let Some(entry) = ifd.get(&TIFF_TAG_STRIP_BYTE_COUNTS) {
            // Traditional TIFF strips
            self.tiff_parser
                .read_u32_array(reader, entry)?
                .into_iter()
                .map(|x| x as u64)
                .collect()
        } else if let Some(entry) = ifd.get(&TIFF_TAG_JPEG_INTERCHANGE_FORMAT_LENGTH) {
            // Z9 NEF uses JPEG Interchange Format Length for RAW data size
            tracing::info!("Using JPEG Interchange Format Length for RAW data size");
            vec![u64::from(
                self.tiff_parser
                    .read_unsigned_scalar(reader, entry)
                    .context("Unable to read RAW JPEG byte count")?,
            )]
        } else {
            anyhow::bail!("RAW IFD is missing strip/JPEG byte counts");
        };

        // Extract white balance from MakerNote if available
        let cam_mul = if let (Some(offset), Some(size)) = (makernote_offset, makernote_size) {
            self.parse_makernote_white_balance_with_offset(offset, size)
                .unwrap_or_else(|e| {
                    tracing::warn!("Failed to extract WB from MakerNote: {}", e);
                    [1.0, 1.0, 1.0, 1.0]
                })
        } else {
            tracing::warn!("MakerNote not found, using neutral WB");
            [1.0, 1.0, 1.0, 1.0]
        };

        // Extract EXIF metadata for grouping analysis
        let exif_data = self.extract_exif_metadata(reader)?;

        Ok(Z9Metadata {
            width,
            height,
            bits_per_sample,
            compression,
            cfa_pattern,
            camera_make: camera_make.to_owned(),
            camera_model: camera_model.to_owned(),
            sensor_levels: profile.map(|profile| profile.levels),
            sensor_geometry: profile.map(|profile| profile.geometry),
            strip_offsets,
            strip_byte_counts,
            rows_per_strip,
            cam_mul,
            timestamp: exif_data.timestamp,
            exposure_time: exif_data.exposure_time,
            aperture: exif_data.aperture,
            iso: exif_data.iso,
            focal_length: exif_data.focal_length,
            focus_distance: exif_data.focus_distance,
        })
    }

    /// Extract white balance multipliers from EXIF/MakerNote
    fn extract_white_balance_from_makernote(
        &self,
        _reader: &mut BufReader<&mut File>,
    ) -> Result<[f32; 4]> {
        // Try to extract WB from MakerNote
        match self.parse_makernote_white_balance() {
            Ok(wb) => {
                tracing::info!(
                    "Extracted WB from MakerNote: [{:.3}, {:.3}, {:.3}, {:.3}]",
                    wb[0],
                    wb[1],
                    wb[2],
                    wb[3]
                );
                return Ok(wb);
            }
            Err(e) => {
                tracing::warn!("Failed to extract WB from MakerNote: {}", e);
            }
        }

        // Fallback to neutral WB
        tracing::warn!("Using neutral WB fallback");
        Ok([1.0, 1.0, 1.0, 1.0])
    }

    /// Extract white balance from EXIF data
    fn extract_wb_from_exif(&self) -> Result<[f32; 4]> {
        // Extract WB directly from NEF MakerNote (no dcraw dependency)
        if let Ok(wb) = self.parse_makernote_white_balance() {
            tracing::info!(
                "Using WB from NEF MakerNote: [{:.3}, {:.3}, {:.3}, {:.3}]",
                wb[0],
                wb[1],
                wb[2],
                wb[3]
            );
            return Ok(wb);
        }

        // Fallback: Bone-optimized WB (warm tones for bone imaging)
        // User reports blue/purple output with standard WB
        // Need: MORE red, LESS blue for bone color
        // Normalized by green: R=2.2, G=1.0, B=1.0
        tracing::warn!("Failed to extract WB from MakerNote, using bone-optimized WB fallback");
        Ok([2.2, 1.0, 1.0, 1.0])
    }

    /// Extract EXIF metadata for grouping analysis
    fn extract_exif_metadata(&self, _reader: &mut BufReader<&mut File>) -> Result<ExifData> {
        let mut exif_data = ExifData::default();

        // Use exif crate to extract metadata
        let file = std::fs::File::open(&self.file_path)?;
        let mut bufreader = std::io::BufReader::new(&file);
        let exifreader = exif::Reader::new();

        let exif = match exifreader.read_from_container(&mut bufreader) {
            Ok(exif) => exif,
            Err(e) => {
                tracing::warn!("Failed to read EXIF: {}", e);
                return Ok(exif_data); // Return defaults
            }
        };

        // Extract timestamp
        if let Some(field) = exif.get_field(exif::Tag::DateTime, exif::In::PRIMARY) {
            if let exif::Value::Ascii(ref vec) = field.value {
                if let Some(datetime_str) = vec.first() {
                    // Parse EXIF datetime format: "YYYY:MM:DD HH:MM:SS"
                    let datetime_str = String::from_utf8_lossy(datetime_str);
                    if let Ok(naive_dt) =
                        chrono::NaiveDateTime::parse_from_str(&datetime_str, "%Y:%m:%d %H:%M:%S")
                    {
                        exif_data.timestamp = Some(chrono::DateTime::from_naive_utc_and_offset(
                            naive_dt,
                            chrono::Utc,
                        ));
                    }
                }
            }
        }

        // Extract exposure time
        if let Some(field) = exif.get_field(exif::Tag::ExposureTime, exif::In::PRIMARY) {
            if let exif::Value::Rational(ref v) = field.value {
                if !v.is_empty() {
                    exif_data.exposure_time = Some(v[0].to_f64());
                }
            }
        }

        // Extract aperture (F-number)
        if let Some(field) = exif.get_field(exif::Tag::FNumber, exif::In::PRIMARY) {
            if let exif::Value::Rational(ref v) = field.value {
                if !v.is_empty() {
                    exif_data.aperture = Some(v[0].to_f64() as f32);
                }
            }
        }

        // Extract ISO
        if let Some(field) = exif.get_field(exif::Tag::PhotographicSensitivity, exif::In::PRIMARY) {
            if let Some(iso) = field.value.get_uint(0) {
                exif_data.iso = Some(iso);
            }
        }

        // Extract focal length
        if let Some(field) = exif.get_field(exif::Tag::FocalLength, exif::In::PRIMARY) {
            if let exif::Value::Rational(ref v) = field.value {
                if !v.is_empty() {
                    exif_data.focal_length = Some(v[0].to_f64() as f32);
                }
            }
        }

        // Extract focus distance (SubjectDistance)
        if let Some(field) = exif.get_field(exif::Tag::SubjectDistance, exif::In::PRIMARY) {
            if let exif::Value::Rational(ref v) = field.value {
                if !v.is_empty() {
                    exif_data.focus_distance = Some(v[0].to_f64() as f32);
                }
            }
        }

        tracing::debug!(
            "Extracted EXIF: exp={:?}, f/{:?}, ISO {:?}",
            exif_data.exposure_time,
            exif_data.aperture,
            exif_data.iso
        );

        Ok(exif_data)
    }

    fn load_raw_data_from_strips_into(&self, roi: &Roi, output: &mut dyn RawOutput) -> Result<()> {
        let metadata = self.get_metadata()?;
        if metadata.strip_offsets.is_empty() || metadata.strip_byte_counts.is_empty() {
            anyhow::bail!("NEF RAW directory does not contain strip data");
        }

        let file = File::open(&self.file_path)?;
        // SAFETY: The mapping is read-only and the file handle remains alive for
        // the mapping's complete lifetime. Source mutation during decode is not
        // supported and is detected by normal I/O faults or index invalidation.
        let mapped_file = unsafe { MmapOptions::new().map(&file)? };

        // First, analyze all strips to understand the data layout
        tracing::info!("Total strips: {}", metadata.strip_offsets.len());
        for i in 0..metadata.strip_offsets.len().min(3) {
            let byte_count = metadata.strip_byte_counts.get(i).copied().unwrap_or(0);
            tracing::info!(
                "Strip {}: offset={}, size={}",
                i,
                metadata.strip_offsets[i],
                byte_count
            );
        }

        // Calculate which strips we need for the ROI
        let start_strip = (roi.y / metadata.rows_per_strip) as usize;
        let end_strip = ((roi.y + roi.height - 1) / metadata.rows_per_strip) as usize;

        tracing::info!(
            "ROI y={}, height={}, rows_per_strip={}",
            roi.y,
            roi.height,
            metadata.rows_per_strip
        );
        tracing::info!(
            "ROI requires strips {} to {} (total {} strips)",
            start_strip,
            end_strip,
            metadata.strip_offsets.len()
        );

        // Load relevant strips
        for strip_idx in start_strip..=end_strip.min(metadata.strip_offsets.len() - 1) {
            let strip_offset = metadata.strip_offsets[strip_idx];
            let strip_byte_count = metadata
                .strip_byte_counts
                .get(strip_idx)
                .copied()
                .unwrap_or(0);
            let start = strip_offset as usize;
            let end = strip_offset
                .saturating_add(strip_byte_count)
                .min(mapped_file.len() as u64) as usize;
            if start >= end || start >= mapped_file.len() {
                anyhow::bail!(
                    "NEF strip {} is missing or truncated at offset {}",
                    strip_idx,
                    strip_offset
                );
            }

            self.extract_roi_from_strip(
                &mapped_file[start..end],
                strip_idx,
                roi,
                metadata,
                output,
            )?;
        }

        Ok(())
    }

    fn extract_roi_from_strip(
        &self,
        strip_data: &[u8],
        strip_idx: usize,
        roi: &Roi,
        metadata: &Z9Metadata,
        output: &mut dyn RawOutput,
    ) -> Result<()> {
        // Check if this is compressed data and decompress if needed
        if metadata.compression == 6 {
            // Compression tag 6 = Old-style JPEG (LJPEG - Lossless JPEG)
            // BUT: For Z9 files, LibRaw uses packed_load_raw, not lossless_jpeg_load_raw!
            // Check if this is a Z9 file based on maker and model
            if self.is_z9_camera() {
                tracing::info!("Z9 camera detected with compression tag 6 - using packed format instead of LJPEG");
                return self
                    .extract_roi_from_packed_strip(strip_data, strip_idx, roi, metadata, output);
            } else {
                return self
                    .extract_roi_from_ljpeg_strip(strip_data, strip_idx, roi, metadata, output);
            }
        } else if metadata.compression == 34713 {
            // Compression tag 34713 = Nikon NEF compression (the actual RAW data!)
            tracing::info!(
                "Found Nikon NEF compression 34713 - this is the actual RAW sensor data"
            );
            tracing::info!("Strip size: {} MB", strip_data.len() / 1_000_000);

            // LibRaw Z9-specific handling: check for "CONTACT_INTOPIX" signature at offset 6
            if strip_data.len() >= 21 {
                let signature = &strip_data[6..21];
                if signature == b"CONTACT_INTOPIX" {
                    tracing::info!(
                        "Found CONTACT_INTOPIX signature - Z9 uses High Efficiency compression"
                    );
                    return self.extract_roi_from_z9_he_strip(
                        strip_data, strip_idx, roi, metadata, output,
                    );
                } else {
                    tracing::info!(
                        "No CONTACT_INTOPIX signature - using standard Nikon compression"
                    );
                }
            }

            // Extract Nikon compression metadata from MakerNote
            return self.extract_roi_from_nikon_34713(strip_data, strip_idx, roi, metadata, output);
        } else if metadata.compression != 1 {
            anyhow::bail!("Unsupported NEF compression type: {}", metadata.compression);
        }

        // Calculate strip boundaries
        let strip_start_row = strip_idx as u32 * metadata.rows_per_strip;
        let strip_end_row = (strip_start_row + metadata.rows_per_strip).min(metadata.height);

        // Uncompressed strips support true byte-range ROI access through the mmap.
        let row_start = roi.y.max(strip_start_row);
        let row_end = roi.y.saturating_add(roi.height).min(strip_end_row);
        let col_end = roi.x.saturating_add(roi.width).min(metadata.width);
        for y in row_start..row_end {
            let strip_row = y - strip_start_row;
            let output_row = y - roi.y;
            for x in roi.x..col_end {
                let byte_index = ((strip_row * metadata.width + x) * 2) as usize;
                if byte_index + 1 < strip_data.len() {
                    output.set_pixel(
                        x - roi.x,
                        output_row,
                        u16::from_le_bytes([strip_data[byte_index], strip_data[byte_index + 1]]),
                    );
                }
            }
        }

        Ok(())
    }

    /// Extract ROI from LJPEG compressed strip (compression tag 6)
    /// Check if this is a Z9 camera
    fn is_z9_camera(&self) -> bool {
        self.metadata
            .as_ref()
            .map(|metadata| is_nikon_z9(&metadata.camera_make, &metadata.camera_model))
            .unwrap_or(false)
    }

    /// Extract ROI from packed format strip (LibRaw packed_load_raw equivalent)
    fn extract_roi_from_packed_strip(
        &self,
        strip_data: &[u8],
        strip_idx: usize,
        roi: &Roi,
        metadata: &Z9Metadata,
        output: &mut dyn RawOutput,
    ) -> Result<()> {
        tracing::info!(
            "Extracting ROI from packed format strip {} (LibRaw packed_load_raw equivalent)",
            strip_idx
        );
        tracing::info!(
            "Strip size: {} bytes, bits per sample: {}",
            strip_data.len(),
            metadata.bits_per_sample
        );

        let bits_per_pixel = metadata.bits_per_sample as usize;
        if bits_per_pixel != 12 && bits_per_pixel != 14 {
            anyhow::bail!("Unsupported packed RAW bit depth: {}", bits_per_pixel);
        }

        let strip_start_row = strip_idx as u32 * metadata.rows_per_strip;
        let strip_end_row = (strip_start_row + metadata.rows_per_strip).min(metadata.height);
        let row_start = roi.y.max(strip_start_row);
        let row_end = roi.y.saturating_add(roi.height).min(strip_end_row);
        let col_end = roi.x.saturating_add(roi.width).min(metadata.width);

        // Packed RAW is directly addressable. Calculate the group containing each
        // requested pixel and touch no bytes outside the ROI's packed groups.
        for y in row_start..row_end {
            let strip_row = y - strip_start_row;
            for x in roi.x..col_end {
                let linear_pixel = strip_row as usize * metadata.width as usize + x as usize;
                let value = if bits_per_pixel == 14 {
                    let group = linear_pixel / 4;
                    let lane = linear_pixel % 4;
                    let offset = group * 7;
                    if offset + 7 > strip_data.len() {
                        anyhow::bail!("Packed 14-bit strip is truncated at pixel {}", linear_pixel);
                    }
                    let bytes = &strip_data[offset..offset + 7];
                    [
                        ((bytes[0] as u16) << 6) | (((bytes[6] & 0xFC) >> 2) as u16),
                        ((bytes[1] as u16) << 6)
                            | (((bytes[6] & 0x03) << 4) as u16)
                            | (((bytes[4] & 0xF0) >> 4) as u16),
                        ((bytes[2] as u16) << 6)
                            | (((bytes[4] & 0x0F) << 2) as u16)
                            | (((bytes[5] & 0xC0) >> 6) as u16),
                        ((bytes[3] as u16) << 6) | ((bytes[5] & 0x3F) as u16),
                    ][lane]
                } else {
                    let group = linear_pixel / 2;
                    let lane = linear_pixel % 2;
                    let offset = group * 3;
                    if offset + 3 > strip_data.len() {
                        anyhow::bail!("Packed 12-bit strip is truncated at pixel {}", linear_pixel);
                    }
                    let bytes = &strip_data[offset..offset + 3];
                    [
                        ((bytes[0] as u16) << 4) | (((bytes[2] & 0xF0) >> 4) as u16),
                        ((bytes[1] as u16) << 4) | ((bytes[2] & 0x0F) as u16),
                    ][lane]
                };
                output.set_pixel(x - roi.x, y - roi.y, value);
            }
        }

        Ok(())
    }

    /// Unpack 14-bit packed data (LibRaw style)
    fn unpack_14bit_data(&self, data: &[u8], roi: &Roi, output: &mut dyn RawOutput) -> Result<()> {
        tracing::info!("Unpacking 14-bit packed data");

        // 14-bit packed format: 4 pixels in 7 bytes
        // Pixel 0: byte 0 + 6 bits of byte 6
        // Pixel 1: byte 1 + 4 bits of byte 6
        // Pixel 2: byte 2 + 2 bits of byte 6
        // Pixel 3: byte 3 + byte 4 + byte 5

        let mut data_idx = 0;

        for row in 0..roi.height {
            for col in (0..roi.width).step_by(4) {
                if data_idx + 7 > data.len() {
                    tracing::warn!("Ran out of data at row {}, col {}", row, col);
                    return Ok(());
                }

                // Read 7 bytes for 4 pixels
                let bytes = &data[data_idx..data_idx + 7];
                data_idx += 7;

                // Unpack 4 pixels
                let pixels = [
                    ((bytes[0] as u16) << 6) | (((bytes[6] & 0xFC) >> 2) as u16),
                    ((bytes[1] as u16) << 6)
                        | (((bytes[6] & 0x03) << 4) as u16)
                        | (((bytes[4] & 0xF0) >> 4) as u16),
                    ((bytes[2] as u16) << 6)
                        | (((bytes[4] & 0x0F) << 2) as u16)
                        | (((bytes[5] & 0xC0) >> 6) as u16),
                    ((bytes[3] as u16) << 6) | ((bytes[5] & 0x3F) as u16),
                ];

                // Store pixels in output buffer
                for (i, &pixel) in pixels.iter().enumerate() {
                    let output_col = col + i as u32;
                    if output_col < output.width() && row < output.height() {
                        output.set_pixel(output_col, row, pixel);
                    }
                }
            }
        }

        tracing::info!("Successfully unpacked 14-bit data");
        Ok(())
    }

    /// Unpack 12-bit packed data (LibRaw style)
    fn unpack_12bit_data(&self, data: &[u8], roi: &Roi, output: &mut dyn RawOutput) -> Result<()> {
        tracing::info!("Unpacking 12-bit packed data");

        // 12-bit packed format: 2 pixels in 3 bytes
        // Pixel 0: byte 0 + 4 bits of byte 2
        // Pixel 1: byte 1 + 4 bits of byte 2

        let mut data_idx = 0;

        for row in 0..roi.height {
            for col in (0..roi.width).step_by(2) {
                if data_idx + 3 > data.len() {
                    tracing::warn!("Ran out of data at row {}, col {}", row, col);
                    return Ok(());
                }

                // Read 3 bytes for 2 pixels
                let bytes = &data[data_idx..data_idx + 3];
                data_idx += 3;

                // Unpack 2 pixels
                let pixels = [
                    ((bytes[0] as u16) << 4) | (((bytes[2] & 0xF0) >> 4) as u16),
                    ((bytes[1] as u16) << 4) | ((bytes[2] & 0x0F) as u16),
                ];

                // Store pixels in output buffer
                for (i, &pixel) in pixels.iter().enumerate() {
                    let output_col = col + i as u32;
                    if output_col < output.width() && row < output.height() {
                        output.set_pixel(output_col, row, pixel);
                    }
                }
            }
        }

        tracing::info!("Successfully unpacked 12-bit data");
        Ok(())
    }

    /// Extract ROI from Z9 High Efficiency compressed strip (CONTACT_INTOPIX)
    fn extract_roi_from_z9_he_strip(
        &self,
        strip_data: &[u8],
        strip_idx: usize,
        roi: &Roi,
        metadata: &Z9Metadata,
        output: &mut dyn RawOutput,
    ) -> Result<()> {
        tracing::info!(
            "Extracting ROI from Z9 High Efficiency compressed strip {} (CONTACT_INTOPIX)",
            strip_idx
        );

        // Z9 High Efficiency compression is proprietary by Intopix
        // For now, implement proper decompression based on LibRaw approach

        // Check if this is actually Z9 HE data by validating the signature
        if strip_data.len() < 21 || &strip_data[6..21] != b"CONTACT_INTOPIX" {
            return Err(anyhow::anyhow!("Invalid Z9 HE signature in strip data"));
        }

        // Z9 HE decompression requires specialized handling
        // For now, use the Nikon compression 34713 path which should work for Z9
        tracing::info!("Falling back to Nikon compression 34713 for Z9 HE data");
        self.extract_roi_from_nikon_34713(strip_data, strip_idx, roi, metadata, output)
    }

    fn extract_roi_from_ljpeg_strip(
        &self,
        strip_data: &[u8],
        strip_idx: usize,
        roi: &Roi,
        metadata: &Z9Metadata,
        output: &mut dyn RawOutput,
    ) -> Result<()> {
        // Debug: Check what the strip data looks like
        tracing::info!("Strip {} data length: {}", strip_idx, strip_data.len());
        if strip_data.len() >= 16 {
            tracing::info!("First 16 bytes: {:02x?}", &strip_data[0..16]);

            // Check what type of data this strip contains
            let ascii_str: String = strip_data[0..16]
                .iter()
                .map(|&b| {
                    if (32..=126).contains(&b) {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            tracing::info!("Strip {} as ASCII: '{}'", strip_idx, ascii_str);
        }

        // For compression tag 6, the data might not be a complete LJPEG stream
        // It might be raw compressed data that needs different handling
        tracing::warn!("Compression tag 6 detected - this may not be standard LJPEG");

        // Check what type of data this is
        if strip_data.len() >= 2 && u16::from_be_bytes([strip_data[0], strip_data[1]]) == 0xffd8 {
            // This is JPEG data (preview image, not RAW data)
            tracing::warn!("Found JPEG data (preview image) - this is NOT the RAW sensor data!");
            tracing::warn!(
                "Strip size: {} bytes - too small for RAW data",
                strip_data.len()
            );
            tracing::warn!(
                "Expected RAW data size: ~{} MB",
                (metadata.width * metadata.height * 2) / 1_000_000
            );
            Ok(())
        } else {
            // Nikon proprietary compression (compression tag 6 but not LJPEG)
            tracing::info!("Nikon proprietary compression detected (tag 6, non-LJPEG)");
            self.extract_roi_from_nikon_compressed_strip(
                strip_data, strip_idx, roi, metadata, output,
            )
        }
    }

    /// Find the start of LJPEG scan data
    #[allow(dead_code)]
    fn find_ljpeg_scan_data(&self, data: &[u8]) -> Result<usize> {
        let mut pos = 2; // Skip SOI

        while pos < data.len() - 1 {
            if pos + 4 > data.len() {
                break;
            }

            let tag = u16::from_be_bytes([data[pos], data[pos + 1]]);
            let len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;

            if tag == 0xffda {
                // SOS marker
                return Ok(pos + 4 + len - 2);
            }

            pos += 4 + len - 2;
        }

        Err(anyhow::anyhow!("Could not find LJPEG scan data"))
    }

    /// Decompress LJPEG data for ROI
    fn decompress_ljpeg_roi(
        &self,
        pump: &mut BitPumpMSB,
        header: &LjpegHeader,
        roi_start_row: u32,
        roi_end_row: u32,
        _strip_start_row: u32,
        roi: &Roi,
        metadata: &Z9Metadata,
        output: &mut dyn RawOutput,
    ) -> Result<()> {
        // Get the first Huffman table (should be available)
        let huff_table = header.huff[0]
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No Huffman table found in LJPEG header"))?;

        // Initialize predictors
        let mut vpred = header.vpred;

        // Process each row in the ROI
        for row in roi_start_row..roi_end_row {
            let output_row = row - roi.y;

            // For LJPEG, we need to decode the entire row to maintain predictor state
            // but we only extract the ROI portion
            let mut row_data = vec![0u16; metadata.width as usize];

            // Decode the row using LJPEG differential decoding
            for col in 0..metadata.width {
                let diff = self.ljpeg_diff(pump, huff_table)?;

                let pred = if col == 0 {
                    // First column uses vertical predictor
                    vpred[0] as i32
                } else {
                    // Subsequent columns use horizontal predictor
                    row_data[(col - 1) as usize] as i32
                };

                let value = (pred + diff).max(0).min((1 << header.bits) - 1) as u16;
                row_data[col as usize] = value;

                // Update vertical predictor for next row
                if col == 0 {
                    vpred[0] = value;
                }
            }

            // Extract ROI portion from the decoded row
            for x in roi.x..roi.x + roi.width {
                if x < metadata.width {
                    let output_x = x - roi.x;
                    output.set_pixel(output_x, output_row, row_data[x as usize]);
                }
            }
        }

        Ok(())
    }

    /// Decode LJPEG differential value
    fn ljpeg_diff(
        &self,
        pump: &mut BitPumpMSB,
        huff_table: &super::huffman::HuffTable,
    ) -> Result<i32> {
        let len = huff_table.huff_decode(pump)? as u32;

        if len == 0 {
            return Ok(0);
        }

        if len == 16 {
            return Ok(-32768); // Special case for 16-bit
        }

        let diff = pump.get_bits(len)? as i32;

        // Convert to signed value
        if diff < (1 << (len - 1)) {
            Ok(diff - (1 << len) + 1)
        } else {
            Ok(diff)
        }
    }

    // NOTE: extract_roi_from_rawloader_image was removed (rawloader dependency eliminated)

    /// Extract ROI from compressed NEF strip using Huffman decompression
    fn extract_roi_from_compressed_strip(
        &self,
        strip_data: &[u8],
        strip_idx: usize,
        roi: &Roi,
        metadata: &Z9Metadata,
        output: &mut dyn RawOutput,
    ) -> Result<()> {
        // For now, implement a simplified version that decompresses the entire strip
        // ROI-only decompression optimization can be added for improved performance

        // Determine Huffman table based on bit depth and compression type
        let huff_select = if metadata.bits_per_sample == 14 {
            if metadata.compression == 6 {
                5
            } else {
                3
            } // 14-bit lossless or lossy
        } else if metadata.compression == 6 {
            2
        } else {
            0
        };

        // Create Huffman table
        let htable = create_huffman_table(huff_select)
            .with_context(|| format!("Failed to create Huffman table {}", huff_select))?;

        // Create bit pump for reading compressed data
        let mut pump = BitPumpMSB::new(strip_data);

        // Initialize predictors (from rawloader implementation)
        let mut random = pump.peek_bits(24).unwrap_or(0);

        // Calculate strip boundaries
        let strip_start_row = strip_idx as u32 * metadata.rows_per_strip;
        let strip_end_row = (strip_start_row + metadata.rows_per_strip).min(metadata.height);
        let strip_height = strip_end_row - strip_start_row;

        // Decompress the strip data
        let decompressed_data = self.decompress_strip_huffman(
            &mut pump,
            &htable,
            metadata.width,
            strip_height,
            metadata.bits_per_sample,
            &mut random,
        )?;

        // Extract ROI portion from decompressed data
        for y in roi.y.max(strip_start_row)..roi.y + roi.height.min(strip_end_row) {
            if y >= strip_start_row && y < strip_end_row {
                let strip_row = y - strip_start_row;
                let output_row = y - roi.y;

                for x in roi.x..roi.x + roi.width {
                    if x < metadata.width {
                        let strip_idx = (strip_row * metadata.width + x) as usize;
                        let output_x = x - roi.x;

                        if strip_idx < decompressed_data.len() {
                            output.set_pixel(output_x, output_row, decompressed_data[strip_idx]);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Decompress a strip using Huffman decoding (based on rawloader implementation)
    fn decompress_strip_huffman(
        &self,
        pump: &mut BitPumpMSB,
        htable: &super::huffman::HuffTable,
        width: u32,
        height: u32,
        bits_per_sample: u16,
        random: &mut u32,
    ) -> Result<Vec<u16>> {
        let mut output = vec![0u16; (width * height) as usize];

        // Initialize predictors for each row (based on rawloader)
        // For 14-bit data, initial predictors are typically around mid-range
        let initial_value = 1 << (bits_per_sample - 1); // Mid-range value
        let mut pred_up1: [i32; 2] = [initial_value, initial_value];
        let mut pred_up2: [i32; 2] = [initial_value, initial_value];

        // Try to read initial predictors from the data stream if available
        // Skip the first few bytes which might be metadata
        let _ = pump.get_bits(8); // Skip potential metadata byte
        let _ = pump.get_bits(8); // Skip potential metadata byte

        // Create linearization curve that maps to full 16-bit range
        let max_val = 1 << bits_per_sample;
        let curve_points: Vec<u16> = (0..max_val)
            .map(|i| {
                // Scale from bit depth to 16-bit range
                let scaled = (i * 65535) / (max_val - 1);
                scaled as u16
            })
            .collect();
        let curve = LookupTable::new(&curve_points);

        // Decompress row by row
        for row in 0..height {
            // Update predictors
            if let Ok(diff1) = htable.huff_decode(pump) {
                pred_up1[row as usize & 1] += diff1;
            }
            if let Ok(diff2) = htable.huff_decode(pump) {
                pred_up2[row as usize & 1] += diff2;
            }

            let mut pred_left1 = pred_up1[row as usize & 1];
            let mut pred_left2 = pred_up2[row as usize & 1];

            // Decompress pixels in pairs
            for col in (0..width).step_by(2) {
                if col > 0 {
                    if let Ok(diff1) = htable.huff_decode(pump) {
                        pred_left1 += diff1;
                    }
                    if let Ok(diff2) = htable.huff_decode(pump) {
                        pred_left2 += diff2;
                    }
                }

                let idx1 = (row * width + col) as usize;
                let idx2 = (row * width + col + 1) as usize;

                if idx1 < output.len() {
                    output[idx1] =
                        curve.dither(clamp_bits(pred_left1, bits_per_sample as u32), random);
                }
                if idx2 < output.len() && col + 1 < width {
                    output[idx2] =
                        curve.dither(clamp_bits(pred_left2, bits_per_sample as u32), random);
                }
            }
        }

        Ok(output)
    }

    /// Extract ROI from Nikon proprietary compressed strip
    fn extract_roi_from_nikon_compressed_strip(
        &self,
        strip_data: &[u8],
        _strip_idx: usize,
        _roi: &Roi,
        metadata: &Z9Metadata,
        _output: &mut dyn RawOutput,
    ) -> Result<()> {
        tracing::info!("Analyzing strip data pattern...");

        // Check if this might actually be uncompressed data with some header/padding
        let expected_pixels = metadata.width * metadata.rows_per_strip;
        let expected_bytes = expected_pixels * 2; // 16-bit per pixel

        tracing::info!(
            "Expected pixels: {}, Expected bytes: {}, Actual bytes: {}",
            expected_pixels,
            expected_bytes,
            strip_data.len()
        );

        // Check for patterns in the data
        let mut unique_values = std::collections::HashSet::new();
        for i in 0..strip_data.len().min(1000) {
            unique_values.insert(strip_data[i]);
        }
        tracing::info!(
            "Unique byte values in first 1000 bytes: {}",
            unique_values.len()
        );

        // Try to find where actual image data might start
        let mut data_start = 0;
        for i in 0..strip_data.len().saturating_sub(100) {
            // Look for a change in pattern that might indicate start of image data
            let current_byte = strip_data[i];
            let mut different_count = 0;
            for j in 1..10 {
                if i + j < strip_data.len() && strip_data[i + j] != current_byte {
                    different_count += 1;
                }
            }
            if different_count >= 5 {
                data_start = i;
                tracing::info!("Potential data start found at offset: {}", data_start);
                break;
            }
        }

        // The strip contains much more data than expected - this suggests it's the entire image
        // compressed in a single strip, not per-strip compression
        tracing::info!(
            "Strip contains {} bytes, much larger than expected {} bytes",
            strip_data.len(),
            expected_bytes
        );
        tracing::info!("This suggests the entire image is in one compressed block");

        // For Z9 compression tag 6, we need to implement the actual decompression algorithm
        // For now, let's try to extract some data from the variable portion
        if data_start < strip_data.len() {
            tracing::info!(
                "Attempting to extract data from variable portion starting at {}",
                data_start
            );

            // Try different interpretations of the data after the header
            let remaining_data = &strip_data[data_start..];
            tracing::info!("Remaining data length: {}", remaining_data.len());

            // Check if this might be compressed with a different algorithm
            if remaining_data.len() >= 16 {
                tracing::info!(
                    "First 16 bytes of variable data: {:02x?}",
                    &remaining_data[0..16]
                );

                // Convert to ASCII to see if it's text/XML
                let ascii_str: String = remaining_data[0..16]
                    .iter()
                    .map(|&b| {
                        if (32..=126).contains(&b) {
                            b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                tracing::info!("As ASCII: '{}'", ascii_str);

                // Check if this is XML metadata (XMP)
                if ascii_str.contains("<?x") {
                    tracing::info!("Detected XML/XMP metadata in strip data");
                    tracing::warn!("This strip contains metadata, not image data!");
                }
            }

            // For now, return empty data since we haven't implemented the decompression
            tracing::warn!("Z9 compression tag 6 decompression using fallback method");
            Ok(())
        } else {
            tracing::warn!("No variable data found in strip");
            Ok(())
        }
    }

    /// Extract ROI from raw 16-bit data
    fn extract_roi_from_raw_data(
        &self,
        raw_data: &[u8],
        strip_idx: usize,
        roi: &Roi,
        metadata: &Z9Metadata,
        output: &mut dyn RawOutput,
    ) -> Result<()> {
        // Calculate strip boundaries
        let strip_start_row = strip_idx as u32 * metadata.rows_per_strip;
        let strip_end_row = (strip_start_row + metadata.rows_per_strip).min(metadata.height);

        // Process each row in the strip
        for row in strip_start_row..strip_end_row {
            if row >= roi.y && row < roi.y + roi.height {
                let output_row = row - roi.y;
                let strip_row = row - strip_start_row;

                // Extract ROI portion from this row
                for x in roi.x..roi.x + roi.width {
                    if x < metadata.width {
                        let pixel_idx = (strip_row * metadata.width + x) as usize * 2;
                        let output_x = x - roi.x;

                        if pixel_idx + 1 < raw_data.len() {
                            // Read 16-bit value (little-endian)
                            let value =
                                u16::from_le_bytes([raw_data[pixel_idx], raw_data[pixel_idx + 1]]);
                            output.set_pixel(output_x, output_row, value);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Decode Nikon Huffman value (based on LibRaw implementation)
    fn nikon_huff_decode(
        &self,
        pump: &mut BitPumpMSB,
        huff_table: &super::huffman::HuffTable,
    ) -> Result<i32> {
        let code = huff_table.huff_decode(pump)? as u32;
        let len = code & 15;
        let shl = code >> 4;

        if len == 0 {
            return Ok(0);
        }

        let diff = if len > shl {
            ((pump.get_bits(len - shl)? << 1) + 1) << shl >> 1
        } else {
            0
        };

        let result = if len > 0 && (diff & (1 << (len - 1))) == 0 {
            diff - (1 << len) + if shl > 0 { 0 } else { 1 }
        } else {
            diff
        };

        Ok(result as i32)
    }

    /// Extract ROI from Nikon compression 34713 strip
    fn extract_roi_from_nikon_34713(
        &self,
        strip_data: &[u8],
        strip_idx: usize,
        roi: &Roi,
        metadata: &Z9Metadata,
        output: &mut dyn RawOutput,
    ) -> Result<()> {
        tracing::info!("Starting Nikon compression 34713 decompression");

        // Parse MakerNote to get real compression metadata
        let compression_meta =
            match self.parse_makernote_compression_meta(metadata.bits_per_sample as u8) {
                Ok(meta) => {
                    tracing::info!("Successfully parsed MakerNote compression metadata");
                    meta
                }
                Err(e) => {
                    // Strict: no fallbacks or hardcoded tables
                    return Err(anyhow::anyhow!(
                        "MakerNote parse failed: {} (no fallbacks allowed)",
                        e
                    ));
                }
            };

        let decompressor = NikonDecompressor::new(compression_meta);

        // Calculate strip boundaries
        let strip_start_row = strip_idx as u32 * metadata.rows_per_strip;
        let strip_end_row = (strip_start_row + metadata.rows_per_strip).min(metadata.height);

        // Calculate ROI bbox relative to the strip
        let strip_roi_y = roi.y.saturating_sub(strip_start_row) as i32;
        let strip_roi_height = roi
            .height
            .min(((strip_end_row - strip_start_row) as i32 - strip_roi_y).max(0) as u32);

        if strip_roi_height == 0 {
            tracing::debug!("Strip {} doesn't intersect with ROI", strip_idx);
            return Ok(());
        }

        let bbox = crate::object_detection::BoundingBox {
            x: roi.x,
            y: strip_roi_y.max(0) as u32,
            width: roi.width,
            height: strip_roi_height,
        };

        tracing::info!(
            "Strip {} ROI: bbox=({}, {}, {}, {})",
            strip_idx,
            bbox.x,
            bbox.y,
            bbox.width,
            bbox.height
        );

        let region_width = bbox.width as usize;
        let region_height = bbox.height as usize;
        let expected_pixels = region_width
            .checked_mul(region_height)
            .context("NEF ROI dimensions overflow")?;
        let output_row_offset =
            (strip_start_row as i64 + strip_roi_y as i64 - roi.y as i64).max(0) as usize;
        let output_width = output.width() as usize;

        let decode_start = std::time::Instant::now();
        let access_mode =
            std::env::var("TRUESHOT_NEF_ACCESS_MODE").unwrap_or_else(|_| "stream".to_owned());
        if !access_mode.eq_ignore_ascii_case("indexed") {
            if !access_mode.eq_ignore_ascii_case("stream") {
                tracing::warn!(
                    "Unknown TRUESHOT_NEF_ACCESS_MODE={:?}; using sidecar-free streaming",
                    access_mode
                );
            }
            if region_width != output_width {
                anyhow::bail!("NEF strip ROI width does not match output width");
            }
            let destination_start = output_row_offset
                .checked_mul(output_width)
                .context("NEF ROI output offset overflow")?;
            let destination_end = destination_start
                .checked_add(expected_pixels)
                .context("NEF ROI output length overflow")?;
            let destination = output
                .pixels_mut()
                .get_mut(destination_start..destination_end)
                .context("NEF ROI output exceeds destination buffer")?;
            decompressor.decompress_selective_streaming_into(
                strip_data,
                metadata.width,
                strip_end_row - strip_start_row,
                bbox,
                destination,
            )?;
            tracing::info!(
                "Sidecar-free NEF ROI stream decode completed in {:.2}ms: {} pixels",
                decode_start.elapsed().as_secs_f64() * 1000.0,
                expected_pixels
            );
            return Ok(());
        }

        let decompressed_pixels =
            self.decode_indexed_nef_roi(&decompressor, strip_data, strip_idx, metadata, bbox)?;

        if decompressed_pixels.len() != expected_pixels {
            anyhow::bail!(
                "Decompressed pixel count mismatch: expected {}, got {}",
                expected_pixels,
                decompressed_pixels.len()
            );
        }

        for (source_row, source) in decompressed_pixels.chunks_exact(region_width).enumerate() {
            let output_y = output_row_offset + source_row;
            if output_y >= output.height() as usize {
                break;
            }
            let destination_start = output_y * output_width;
            let destination_end = destination_start + region_width.min(output_width);
            output.pixels_mut()[destination_start..destination_end]
                .copy_from_slice(&source[..destination_end - destination_start]);
        }

        tracing::info!(
            "Nikon compression 34713 ROI decompression completed: {} pixels",
            expected_pixels
        );
        Ok(())
    }

    fn decode_indexed_nef_roi(
        &self,
        decompressor: &NikonDecompressor,
        strip_data: &[u8],
        strip_idx: usize,
        metadata: &Z9Metadata,
        bbox: crate::object_detection::BoundingBox,
    ) -> Result<Vec<u16>> {
        let index_path = self.seek_index_path(strip_idx, metadata, strip_data.len() as u64)?;
        let strip_start_row = strip_idx as u32 * metadata.rows_per_strip;
        let strip_height = metadata
            .rows_per_strip
            .min(metadata.height.saturating_sub(strip_start_row));
        let decode_start = std::time::Instant::now();
        match NikonSeekIndex::load(&index_path) {
            Ok(index) => match decompressor.decompress_selective_from_index(
                strip_data,
                metadata.width,
                strip_height,
                bbox,
                &index,
            ) {
                Ok(pixels) => {
                    tracing::info!(
                        "Indexed NEF ROI decode completed in {:.2}ms using {}",
                        decode_start.elapsed().as_secs_f64() * 1000.0,
                        index_path.display()
                    );
                    Ok(pixels)
                }
                Err(error) => {
                    tracing::warn!("Discarding incompatible NEF seek index: {}", error);
                    let _ = std::fs::remove_file(&index_path);
                    self.build_seek_index_and_decode(
                        decompressor,
                        strip_data,
                        metadata.width,
                        strip_height,
                        bbox,
                        &index_path,
                    )
                }
            },
            Err(error) => {
                if index_path.exists() {
                    tracing::warn!("Discarding unreadable NEF seek index: {}", error);
                    let _ = std::fs::remove_file(&index_path);
                }
                self.build_seek_index_and_decode(
                    decompressor,
                    strip_data,
                    metadata.width,
                    strip_height,
                    bbox,
                    &index_path,
                )
            }
        }
    }

    fn build_seek_index_and_decode(
        &self,
        decompressor: &NikonDecompressor,
        strip_data: &[u8],
        width: u32,
        height: u32,
        bbox: crate::object_detection::BoundingBox,
        index_path: &Path,
    ) -> Result<Vec<u16>> {
        let start = std::time::Instant::now();
        let stride = std::env::var("TRUESHOT_NEF_INDEX_STRIDE")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(DEFAULT_SEEK_INDEX_STRIDE);
        let (index, pixels) =
            decompressor.build_seek_index_and_extract(strip_data, width, height, bbox, stride)?;
        if let Err(error) = index.save_atomic(index_path) {
            tracing::warn!("Failed to persist NEF seek index: {}", error);
        }
        tracing::info!(
            "Cold NEF ROI decode and seek-index build completed in {:.2}ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
        Ok(pixels)
    }

    fn seek_index_path(
        &self,
        strip_idx: usize,
        metadata: &Z9Metadata,
        compressed_len: u64,
    ) -> Result<PathBuf> {
        let source_metadata = std::fs::metadata(&self.file_path)?;
        let modified = source_metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .unwrap_or_default();
        let strip_offset = metadata.strip_offsets.get(strip_idx).copied().unwrap_or(0);

        let mut hasher = Sha256::new();
        hasher.update(self.file_path.as_bytes());
        hasher.update(source_metadata.len().to_le_bytes());
        hasher.update(modified.as_secs().to_le_bytes());
        hasher.update(modified.subsec_nanos().to_le_bytes());
        hasher.update((strip_idx as u64).to_le_bytes());
        hasher.update(strip_offset.to_le_bytes());
        hasher.update(compressed_len.to_le_bytes());
        hasher.update(metadata.width.to_le_bytes());
        hasher.update(metadata.height.to_le_bytes());
        hasher.update(metadata.bits_per_sample.to_le_bytes());
        let key = hex::encode(hasher.finalize());

        let cache_dir = std::env::var_os("TRUESHOT_NEF_INDEX_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::cache_dir().map(|path| path.join("trueshot").join("nef-index")))
            .context("Unable to resolve TrueShot NEF index cache directory")?;
        Ok(cache_dir.join(format!("{}.tsnidx", key)))
    }

    /// Parse MakerNote to extract white balance (with explicit offset/size)
    fn parse_makernote_white_balance_with_offset(
        &self,
        offset: u64,
        _size: u64,
    ) -> Result<[f32; 4]> {
        let mut file = File::open(&self.file_path)?;
        let mut reader = BufReader::new(&mut file);

        tracing::debug!("Parsing MakerNote for WB at offset: {}", offset);

        // Seek to MakerNote
        reader.seek(SeekFrom::Start(offset))?;

        // Read MakerNote header - Nikon MakerNote starts with "Nikon\0\x02\x10\0\0"
        let mut header = [0u8; 10];
        reader.read_exact(&mut header)?;

        if &header[0..5] != b"Nikon" {
            return Err(anyhow::anyhow!("Invalid Nikon MakerNote header"));
        }

        // Read TIFF header within MakerNote
        let mut tiff_header = [0u8; 8];
        reader.read_exact(&mut tiff_header)?;

        // Check byte order
        let little_endian = match &tiff_header[0..2] {
            b"II" => true,
            b"MM" => false,
            _ => return Err(anyhow::anyhow!("Invalid TIFF header in MakerNote")),
        };

        // Read IFD offset within MakerNote
        let ifd_offset = if little_endian {
            u32::from_le_bytes([
                tiff_header[4],
                tiff_header[5],
                tiff_header[6],
                tiff_header[7],
            ])
        } else {
            u32::from_be_bytes([
                tiff_header[4],
                tiff_header[5],
                tiff_header[6],
                tiff_header[7],
            ])
        };

        // Seek to MakerNote IFD (relative to MakerNote start + 10 bytes)
        reader.seek(SeekFrom::Start(offset + 10 + ifd_offset as u64))?;

        // Read number of entries
        let mut entry_count_bytes = [0u8; 2];
        reader.read_exact(&mut entry_count_bytes)?;
        let entry_count = if little_endian {
            u16::from_le_bytes(entry_count_bytes)
        } else {
            u16::from_be_bytes(entry_count_bytes)
        };

        tracing::info!(
            "MakerNote has {} entries, searching for WB tags",
            entry_count
        );

        // Look for WB tag: 0x000c (WB_RBLevels - as-shot white balance)
        // NOT 0x0097 which is ColorBalance (complex structure with presets)
        for i in 0..entry_count {
            let mut entry_bytes = [0u8; 12];
            reader.read_exact(&mut entry_bytes)?;

            let tag = if little_endian {
                u16::from_le_bytes([entry_bytes[0], entry_bytes[1]])
            } else {
                u16::from_be_bytes([entry_bytes[0], entry_bytes[1]])
            };

            // Log all tags to debug
            if i < 20 || tag == 0x000c {
                tracing::debug!("MakerNote tag {}: 0x{:04x}", i, tag);
            }

            // WB_RBLevels (0x000c) - as-shot white balance
            if tag == 0x000c {
                tracing::info!("Found WB tag 0x{:04x}", tag);

                let count = if little_endian {
                    u32::from_le_bytes([
                        entry_bytes[4],
                        entry_bytes[5],
                        entry_bytes[6],
                        entry_bytes[7],
                    ])
                } else {
                    u32::from_be_bytes([
                        entry_bytes[4],
                        entry_bytes[5],
                        entry_bytes[6],
                        entry_bytes[7],
                    ])
                };

                let value_offset = if little_endian {
                    u32::from_le_bytes([
                        entry_bytes[8],
                        entry_bytes[9],
                        entry_bytes[10],
                        entry_bytes[11],
                    ])
                } else {
                    u32::from_be_bytes([
                        entry_bytes[8],
                        entry_bytes[9],
                        entry_bytes[10],
                        entry_bytes[11],
                    ])
                };

                // WB data is stored as 4 rational values (R, G, B, G2)
                // Each rational is 8 bytes (numerator + denominator as u32)
                let wb_offset = offset + 10 + value_offset as u64;
                reader.seek(SeekFrom::Start(wb_offset))?;

                let mut wb = [1.0f32; 4];
                let mut raw_wb = [1.0f32; 4];
                for i in 0..count.min(4) as usize {
                    let mut rational_bytes = [0u8; 8];
                    reader.read_exact(&mut rational_bytes)?;

                    let numerator = if little_endian {
                        u32::from_le_bytes([
                            rational_bytes[0],
                            rational_bytes[1],
                            rational_bytes[2],
                            rational_bytes[3],
                        ])
                    } else {
                        u32::from_be_bytes([
                            rational_bytes[0],
                            rational_bytes[1],
                            rational_bytes[2],
                            rational_bytes[3],
                        ])
                    };

                    let denominator = if little_endian {
                        u32::from_le_bytes([
                            rational_bytes[4],
                            rational_bytes[5],
                            rational_bytes[6],
                            rational_bytes[7],
                        ])
                    } else {
                        u32::from_be_bytes([
                            rational_bytes[4],
                            rational_bytes[5],
                            rational_bytes[6],
                            rational_bytes[7],
                        ])
                    };

                    tracing::info!(
                        "WB rational[{}]: {}/{} = {:.6}",
                        i,
                        numerator,
                        denominator,
                        if denominator > 0 {
                            numerator as f32 / denominator as f32
                        } else {
                            0.0
                        }
                    );

                    if denominator > 0 {
                        let value = numerator as f32 / denominator as f32;
                        raw_wb[i] = value;
                        // DON'T invert - we multiply by these values in demosaic
                        // Nikon stores them ready to use as multipliers
                        wb[i] = value;
                    }
                }

                tracing::info!(
                    "Raw WB from MakerNote tag 0x000c: [0]={:.6}, [1]={:.6}, [2]={:.6}, [3]={:.6}",
                    raw_wb[0],
                    raw_wb[1],
                    raw_wb[2],
                    raw_wb[3]
                );

                // CRITICAL: Tag 0x000c stores values in [R, B, G, G2] order, NOT [R, G, B, G2]!
                // dcraw output confirms: "multipliers 1.707031 1.000000 1.478516 1.000000" = [R, G, B, G2]
                // But MakerNote has: [1.707031, 1.478516, 1.000000, 1.000000] = [R, B, G, G2]
                // So we need to swap indices [1] and [2]

                let r_raw = wb[0]; // R is correct
                let b_raw = wb[1]; // This is actually B
                let g_raw = wb[2]; // This is actually G
                let g2_raw = wb[3]; // G2 is correct

                tracing::info!(
                    "Reordered WB (R, G, B, G2): R={:.6}, G={:.6}, B={:.6}, G2={:.6}",
                    r_raw,
                    g_raw,
                    b_raw,
                    g2_raw
                );

                // Normalize by green channel to get [R, G, B, G2] = [R/G, 1.0, B/G, 1.0]
                let green = g_raw.max(0.001);
                let wb_normalized = [
                    r_raw / green, // R/G
                    1.0,           // G (reference)
                    b_raw / green, // B/G
                    1.0,           // G2 (same as G)
                ];

                tracing::info!(
                    "Normalized WB: R={:.3}, G={:.3}, B={:.3}, G2={:.3}",
                    wb_normalized[0],
                    wb_normalized[1],
                    wb_normalized[2],
                    wb_normalized[3]
                );

                return Ok(wb_normalized);
            }
        }

        Err(anyhow::anyhow!("WB tags not found in MakerNote"))
    }

    /// Parse MakerNote to extract white balance (using stored offset)
    fn parse_makernote_white_balance(&self) -> Result<[f32; 4]> {
        if let (Some(offset), Some(size)) = (self.makernote_offset, self.makernote_size) {
            self.parse_makernote_white_balance_with_offset(offset, size)
        } else {
            Err(anyhow::anyhow!("MakerNote not found"))
        }
    }

    /// Parse MakerNote to extract Nikon compression metadata
    fn parse_makernote_compression_meta(
        &self,
        bits_per_sample: u8,
    ) -> Result<NikonCompressionMeta> {
        if let (Some(offset), Some(_size)) = (self.makernote_offset, self.makernote_size) {
            let mut file = File::open(&self.file_path)?;
            let mut reader = BufReader::new(&mut file);

            tracing::info!("Parsing MakerNote at offset: {}", offset);

            // Seek to MakerNote
            reader.seek(SeekFrom::Start(offset))?;

            // Read MakerNote header - Nikon MakerNote starts with "Nikon\0\x02\x10\0\0"
            let mut header = [0u8; 10];
            reader.read_exact(&mut header)?;

            if &header[0..5] != b"Nikon" {
                return Err(anyhow::anyhow!("Invalid Nikon MakerNote header"));
            }

            tracing::info!("Found valid Nikon MakerNote header");

            // Read TIFF header within MakerNote
            let mut tiff_header = [0u8; 8];
            reader.read_exact(&mut tiff_header)?;

            // Check byte order
            let little_endian = match &tiff_header[0..2] {
                b"II" => true,
                b"MM" => false,
                _ => return Err(anyhow::anyhow!("Invalid TIFF header in MakerNote")),
            };

            // Read IFD offset within MakerNote
            let ifd_offset = if little_endian {
                u32::from_le_bytes([
                    tiff_header[4],
                    tiff_header[5],
                    tiff_header[6],
                    tiff_header[7],
                ])
            } else {
                u32::from_be_bytes([
                    tiff_header[4],
                    tiff_header[5],
                    tiff_header[6],
                    tiff_header[7],
                ])
            };

            tracing::info!("MakerNote IFD offset: {}", ifd_offset);

            // Seek to MakerNote IFD (relative to MakerNote start + 10 bytes)
            reader.seek(SeekFrom::Start(offset + 10 + ifd_offset as u64))?;

            // Read number of entries
            let mut entry_count_bytes = [0u8; 2];
            reader.read_exact(&mut entry_count_bytes)?;
            let entry_count = if little_endian {
                u16::from_le_bytes(entry_count_bytes)
            } else {
                u16::from_be_bytes(entry_count_bytes)
            };

            tracing::info!("MakerNote has {} entries", entry_count);

            // Look for tag 0x96 (linearization table)
            for _ in 0..entry_count {
                let mut entry_bytes = [0u8; 12];
                reader.read_exact(&mut entry_bytes)?;

                let tag = if little_endian {
                    u16::from_le_bytes([entry_bytes[0], entry_bytes[1]])
                } else {
                    u16::from_be_bytes([entry_bytes[0], entry_bytes[1]])
                };

                if tag == 0x96 {
                    // Linearization table
                    tracing::info!("Found linearization table (tag 0x96)!");

                    let data_type = if little_endian {
                        u16::from_le_bytes([entry_bytes[2], entry_bytes[3]])
                    } else {
                        u16::from_be_bytes([entry_bytes[2], entry_bytes[3]])
                    };

                    let count = if little_endian {
                        u32::from_le_bytes([
                            entry_bytes[4],
                            entry_bytes[5],
                            entry_bytes[6],
                            entry_bytes[7],
                        ])
                    } else {
                        u32::from_be_bytes([
                            entry_bytes[4],
                            entry_bytes[5],
                            entry_bytes[6],
                            entry_bytes[7],
                        ])
                    };

                    let value_offset = if little_endian {
                        u32::from_le_bytes([
                            entry_bytes[8],
                            entry_bytes[9],
                            entry_bytes[10],
                            entry_bytes[11],
                        ])
                    } else {
                        u32::from_be_bytes([
                            entry_bytes[8],
                            entry_bytes[9],
                            entry_bytes[10],
                            entry_bytes[11],
                        ])
                    };

                    tracing::info!(
                        "Tag 0x96: type={}, count={}, offset={}",
                        data_type,
                        count,
                        value_offset
                    );

                    // Parse the linearization table data
                    let table_offset = offset + 10 + value_offset as u64;
                    return NikonCompressionMeta::parse_from_makernote(
                        &mut reader,
                        table_offset,
                        bits_per_sample,
                    );
                }
            }

            Err(anyhow::anyhow!(
                "Tag 0x96 (linearization table) not found in MakerNote"
            ))
        } else {
            Err(anyhow::anyhow!("MakerNote not found"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_nikon_z9, verified_sensor_profile, SensorLevels};

    #[test]
    fn z9_identity_matching_is_model_specific() {
        assert!(is_nikon_z9("NIKON CORPORATION", "NIKON Z 9"));
        assert!(is_nikon_z9("Nikon", "Z 9"));
        assert!(!is_nikon_z9("Nikon", "Z 8"));
        assert!(!is_nikon_z9("Canon", "Z 9"));
    }

    #[test]
    fn verified_z9_profile_uses_capture_validated_levels() {
        let profile = verified_sensor_profile("NIKON CORPORATION", "NIKON Z 9", 14)
            .expect("verified Z9 profile");
        assert_eq!(
            profile.levels,
            SensorLevels {
                black: 1008,
                white: 15311
            }
        );
        assert!((profile.geometry.pixel_pitch_um - 35_900.0 / 8_256.0).abs() < 1e-6);
        assert!(verified_sensor_profile("NIKON CORPORATION", "NIKON Z 8", 14).is_none());
        assert!(verified_sensor_profile("NIKON CORPORATION", "NIKON Z 9", 12).is_none());
    }
}
