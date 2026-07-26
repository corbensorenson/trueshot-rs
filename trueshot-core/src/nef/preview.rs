/// Preview JPEG extraction from NEF files
///
/// This module handles extracting embedded preview JPEGs from NEF files
/// for fast bbox/mask detection without loading the full RAW data.
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};

use super::tiff::TiffParser;
use super::{
    TIFF_TAG_JPEG_INTERCHANGE_FORMAT,
    TIFF_TAG_JPEG_INTERCHANGE_FORMAT_LENGTH,
    NIKON_TAG_PREVIEW_IMAGE_START,
    NIKON_TAG_PREVIEW_IMAGE_LENGTH,
};

pub struct PreviewExtractor {
    parser: TiffParser,
}

impl PreviewExtractor {
    pub fn new() -> Self {
        Self {
            parser: TiffParser::new(),
        }
    }

    /// Extract preview JPEG from NEF file
    ///
    /// Z9 NEF files store preview JPEGs in:
    /// 1. IFD0 JPEG Interchange Format (tags 513/514) - main preview
    /// 2. SubIFD0 - another preview/thumbnail
    /// 3. IFD1 (if exists) - thumbnail directory
    pub fn extract_preview_jpeg(&mut self, file_path: &str) -> Result<Vec<u8>> {
        let mut file = File::open(file_path)
            .with_context(|| format!("Failed to open NEF file: {}", file_path))?;

        let mut reader = BufReader::new(&mut file);

        // Read TIFF header
        let header = self.parser.read_header(&mut reader)?;

        // Try different methods to find preview JPEG

        // Method 1: Look in IFD0 for JPEG Interchange Format (Z9 primary method)
        if let Ok(jpeg_data) = self.extract_from_ifd0(&mut reader, &header) {
            if !jpeg_data.is_empty() {
                tracing::info!("Found preview JPEG in IFD0 (size: {} bytes)", jpeg_data.len());
                return Ok(jpeg_data);
            }
        }

        // Method 2: Look in SubIFDs for additional previews
        if let Ok(jpeg_data) = self.extract_from_subifd(&mut reader, &header) {
            if !jpeg_data.is_empty() {
                tracing::info!("Found preview JPEG in SubIFD (size: {} bytes)", jpeg_data.len());
                return Ok(jpeg_data);
            }
        }

        // Method 3: Look in IFD1 (thumbnail directory)
        if let Ok(jpeg_data) = self.extract_from_ifd1(&mut reader, &header) {
            if !jpeg_data.is_empty() {
                tracing::info!("Found preview JPEG in IFD1 (size: {} bytes)", jpeg_data.len());
                return Ok(jpeg_data);
            }
        }

        // Method 4: Scan for JPEG markers in the file (fallback)
        if let Ok(jpeg_data) = self.scan_for_jpeg_markers(&mut reader) {
            if !jpeg_data.is_empty() {
                tracing::info!("Found preview JPEG by scanning (size: {} bytes)", jpeg_data.len());
                return Ok(jpeg_data);
            }
        }

        Err(anyhow::anyhow!("No preview JPEG found in NEF file"))
    }

    fn extract_from_ifd1(&mut self, reader: &mut BufReader<&mut File>, header: &super::tiff::TiffHeader) -> Result<Vec<u8>> {
        // Read IFD0 first
        let _ifd0 = self.parser.read_ifd(reader, header.ifd_offset)?;
        
        // Get next IFD offset (should point to IFD1)
        let ifd1_offset = self.parser.read_next_ifd_offset(reader)?;
        
        if ifd1_offset == 0 {
            return Err(anyhow::anyhow!("No IFD1 found"));
        }
        
        // Read IFD1
        let ifd1 = self.parser.read_ifd(reader, ifd1_offset)?;
        
        // Look for JPEG preview tags
        if let (Some(offset_entry), Some(length_entry)) = (
            ifd1.get(&TIFF_TAG_JPEG_INTERCHANGE_FORMAT),
            ifd1.get(&TIFF_TAG_JPEG_INTERCHANGE_FORMAT_LENGTH)
        ) {
            let jpeg_offset = offset_entry.value_offset as u64;
            let jpeg_length = length_entry.value_offset as usize;
            
            // Read the JPEG data
            reader.seek(SeekFrom::Start(jpeg_offset))?;
            let mut jpeg_data = vec![0u8; jpeg_length];
            reader.read_exact(&mut jpeg_data)?;
            
            // Verify it's a valid JPEG
            if jpeg_data.starts_with(&[0xFF, 0xD8]) {
                return Ok(jpeg_data);
            }
        }
        
        Err(anyhow::anyhow!("No JPEG in IFD1"))
    }

    fn extract_from_ifd0(&mut self, reader: &mut BufReader<&mut File>, header: &super::tiff::TiffHeader) -> Result<Vec<u8>> {
        // Read IFD0
        let ifd0 = self.parser.read_ifd(reader, header.ifd_offset)?;
        
        // Look for direct JPEG tags in IFD0
        if let (Some(offset_entry), Some(length_entry)) = (
            ifd0.get(&TIFF_TAG_JPEG_INTERCHANGE_FORMAT),
            ifd0.get(&TIFF_TAG_JPEG_INTERCHANGE_FORMAT_LENGTH)
        ) {
            let jpeg_offset = offset_entry.value_offset as u64;
            let jpeg_length = length_entry.value_offset as usize;
            
            // Read the JPEG data
            reader.seek(SeekFrom::Start(jpeg_offset))?;
            let mut jpeg_data = vec![0u8; jpeg_length];
            reader.read_exact(&mut jpeg_data)?;
            
            // Verify it's a valid JPEG
            if jpeg_data.starts_with(&[0xFF, 0xD8]) {
                return Ok(jpeg_data);
            }
        }
        
        // Look for Nikon-specific preview tags
        if let (Some(offset_entry), Some(length_entry)) = (
            ifd0.get(&NIKON_TAG_PREVIEW_IMAGE_START),
            ifd0.get(&NIKON_TAG_PREVIEW_IMAGE_LENGTH)
        ) {
            let jpeg_offset = offset_entry.value_offset as u64;
            let jpeg_length = length_entry.value_offset as usize;
            
            // Read the JPEG data
            reader.seek(SeekFrom::Start(jpeg_offset))?;
            let mut jpeg_data = vec![0u8; jpeg_length];
            reader.read_exact(&mut jpeg_data)?;
            
            // Verify it's a valid JPEG
            if jpeg_data.starts_with(&[0xFF, 0xD8]) {
                return Ok(jpeg_data);
            }
        }
        
        Err(anyhow::anyhow!("No JPEG in IFD0"))
    }

    fn extract_from_subifd(&mut self, reader: &mut BufReader<&mut File>, header: &super::tiff::TiffHeader) -> Result<Vec<u8>> {
        // Read IFD0 to find SubIFDs tag
        let ifd0 = self.parser.read_ifd(reader, header.ifd_offset)?;

        // Look for SubIFDs tag (330)
        if let Some(subifd_entry) = ifd0.get(&330) {
            // Read SubIFD offsets
            if let Ok(subifd_offsets) = self.parser.read_u32_array(reader, subifd_entry) {
                // Check each SubIFD for JPEG data
                for &subifd_offset in &subifd_offsets {
                    if let Ok(subifd) = self.parser.read_ifd(reader, subifd_offset as u64) {
                        // Look for JPEG in this SubIFD
                        if let (Some(offset_entry), Some(length_entry)) = (
                            subifd.get(&TIFF_TAG_JPEG_INTERCHANGE_FORMAT),
                            subifd.get(&TIFF_TAG_JPEG_INTERCHANGE_FORMAT_LENGTH)
                        ) {
                            let jpeg_offset = offset_entry.value_offset as u64;
                            let jpeg_length = length_entry.value_offset as usize;

                            // Read the JPEG data
                            reader.seek(SeekFrom::Start(jpeg_offset))?;
                            let mut jpeg_data = vec![0u8; jpeg_length];
                            reader.read_exact(&mut jpeg_data)?;

                            // Verify it's a valid JPEG
                            if jpeg_data.starts_with(&[0xFF, 0xD8]) {
                                return Ok(jpeg_data);
                            }
                        }
                    }
                }
            }
        }

        Err(anyhow::anyhow!("No JPEG in SubIFDs"))
    }

    fn scan_for_jpeg_markers(&mut self, reader: &mut BufReader<&mut File>) -> Result<Vec<u8>> {
        // Scan the file for JPEG SOI (Start of Image) markers
        reader.seek(SeekFrom::Start(0))?;
        
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;
        
        // Look for JPEG SOI marker (0xFF 0xD8)
        for i in 0..buffer.len().saturating_sub(1) {
            if buffer[i] == 0xFF && buffer[i + 1] == 0xD8 {
                // Found potential JPEG start
                // Look for EOI marker (0xFF 0xD9) to find the end
                for j in (i + 2)..buffer.len().saturating_sub(1) {
                    if buffer[j] == 0xFF && buffer[j + 1] == 0xD9 {
                        // Found JPEG end
                        let jpeg_data = buffer[i..=j + 1].to_vec();
                        
                        // Basic validation - should be reasonable size for a preview
                        if jpeg_data.len() > 1024 && jpeg_data.len() < 10_000_000 {
                            return Ok(jpeg_data);
                        }
                    }
                }
            }
        }
        
        Err(anyhow::anyhow!("No JPEG markers found"))
    }
}

impl Default for PreviewExtractor {
    fn default() -> Self {
        Self::new()
    }
}
