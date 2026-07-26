use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

const TIFF_TAG_JPEG_INTERCHANGE_FORMAT: u16 = 513;
const TIFF_TAG_JPEG_INTERCHANGE_FORMAT_LENGTH: u16 = 514;
const NIKON_TAG_PREVIEW_IMAGE_START: u16 = 0x0201;
const NIKON_TAG_PREVIEW_IMAGE_LENGTH: u16 = 0x0202;

#[derive(Debug, Clone)]
enum ByteOrder {
    LittleEndian,
    BigEndian,
}

#[derive(Debug)]
struct TiffHeader {
    ifd_offset: u64,
}

#[derive(Debug, Clone)]
struct IfdEntry {
    data_type: u16,
    count: u32,
    value_offset: u32,
}

struct TiffParser {
    byte_order: ByteOrder,
}

impl TiffParser {
    fn new() -> Self {
        Self {
            byte_order: ByteOrder::LittleEndian,
        }
    }

    fn read_header(&mut self, reader: &mut BufReader<&mut File>) -> Result<TiffHeader> {
        let mut header_bytes = [0u8; 8];
        reader
            .read_exact(&mut header_bytes)
            .context("Failed to read TIFF header")?;

        let byte_order = match &header_bytes[0..2] {
            b"II" => ByteOrder::LittleEndian,
            b"MM" => ByteOrder::BigEndian,
            _ => return Err(anyhow::anyhow!("Invalid TIFF byte order marker")),
        };
        self.byte_order = byte_order.clone();

        let magic = self.read_u16(&header_bytes[2..4])?;
        if magic != 42 {
            return Err(anyhow::anyhow!("Invalid TIFF magic number: {}", magic));
        }

        let ifd_offset = self.read_u32(&header_bytes[4..8])? as u64;
        Ok(TiffHeader { ifd_offset })
    }

    fn read_ifd(
        &self,
        reader: &mut BufReader<&mut File>,
        offset: u64,
    ) -> Result<HashMap<u16, IfdEntry>> {
        reader
            .seek(SeekFrom::Start(offset))
            .context("Failed to seek to IFD")?;

        let mut count_bytes = [0u8; 2];
        reader
            .read_exact(&mut count_bytes)
            .context("Failed to read IFD entry count")?;
        let entry_count = self.read_u16(&count_bytes)?;

        let mut ifd = HashMap::new();
        for _ in 0..entry_count {
            let mut entry_bytes = [0u8; 12];
            reader
                .read_exact(&mut entry_bytes)
                .context("Failed to read IFD entry")?;

            let tag = self.read_u16(&entry_bytes[0..2])?;
            let data_type = self.read_u16(&entry_bytes[2..4])?;
            let count = self.read_u32(&entry_bytes[4..8])?;
            let value_offset = self.read_u32(&entry_bytes[8..12])?;

            ifd.insert(
                tag,
                IfdEntry {
                    data_type,
                    count,
                    value_offset,
                },
            );
        }

        Ok(ifd)
    }

    fn read_next_ifd_offset(&self, reader: &mut BufReader<&mut File>) -> Result<u64> {
        let mut offset_bytes = [0u8; 4];
        reader
            .read_exact(&mut offset_bytes)
            .context("Failed to read next IFD offset")?;
        Ok(self.read_u32(&offset_bytes)? as u64)
    }

    fn read_tag_data(
        &self,
        reader: &mut BufReader<&mut File>,
        entry: &IfdEntry,
    ) -> Result<Vec<u8>> {
        let data_size = self.get_data_type_size(entry.data_type)? * entry.count as usize;
        if data_size <= 4 {
            let mut data = Vec::new();
            let bytes = entry.value_offset.to_le_bytes();
            data.extend_from_slice(&bytes[0..data_size]);
            Ok(data)
        } else {
            reader.seek(SeekFrom::Start(entry.value_offset as u64))?;
            let mut data = vec![0u8; data_size];
            reader.read_exact(&mut data)?;
            Ok(data)
        }
    }

    fn read_u32_array(
        &self,
        reader: &mut BufReader<&mut File>,
        entry: &IfdEntry,
    ) -> Result<Vec<u32>> {
        let data = self.read_tag_data(reader, entry)?;
        let mut result = Vec::new();
        for chunk in data.chunks_exact(4) {
            result.push(self.read_u32(chunk)?);
        }
        Ok(result)
    }

    fn read_u16(&self, bytes: &[u8]) -> Result<u16> {
        if bytes.len() < 2 {
            return Err(anyhow::anyhow!("Not enough bytes for u16"));
        }
        Ok(match self.byte_order {
            ByteOrder::LittleEndian => u16::from_le_bytes([bytes[0], bytes[1]]),
            ByteOrder::BigEndian => u16::from_be_bytes([bytes[0], bytes[1]]),
        })
    }

    fn read_u32(&self, bytes: &[u8]) -> Result<u32> {
        if bytes.len() < 4 {
            return Err(anyhow::anyhow!("Not enough bytes for u32"));
        }
        Ok(match self.byte_order {
            ByteOrder::LittleEndian => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            ByteOrder::BigEndian => u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        })
    }

    fn get_data_type_size(&self, data_type: u16) -> Result<usize> {
        match data_type {
            1 | 2 | 6 | 7 => Ok(1),
            3 | 8 => Ok(2),
            4 | 9 | 11 => Ok(4),
            5 | 10 | 12 => Ok(8),
            _ => Err(anyhow::anyhow!("Unknown TIFF data type: {}", data_type)),
        }
    }
}

/// Preview JPEG extraction for fast bbox/mask detection.
pub struct VisionPreviewExtractor {
    parser: TiffParser,
}

impl VisionPreviewExtractor {
    pub fn new() -> Self {
        Self {
            parser: TiffParser::new(),
        }
    }

    pub fn extract_preview_jpeg(path: &Path) -> Result<Vec<u8>> {
        let mut extractor = VisionPreviewExtractor::new();
        extractor.extract_from_path(path)
    }

    fn extract_from_path(&mut self, path: &Path) -> Result<Vec<u8>> {
        let mut file =
            File::open(path).with_context(|| format!("Failed to open file: {}", path.display()))?;
        let mut reader = BufReader::new(&mut file);

        let header = self.parser.read_header(&mut reader)?;

        if let Ok(jpeg) = self.extract_from_ifd0(&mut reader, &header) {
            if !jpeg.is_empty() {
                return Ok(jpeg);
            }
        }

        if let Ok(jpeg) = self.extract_from_subifd(&mut reader, &header) {
            if !jpeg.is_empty() {
                return Ok(jpeg);
            }
        }

        if let Ok(jpeg) = self.extract_from_ifd1(&mut reader, &header) {
            if !jpeg.is_empty() {
                return Ok(jpeg);
            }
        }

        if let Ok(jpeg) = self.scan_for_jpeg_markers(&mut reader) {
            if !jpeg.is_empty() {
                return Ok(jpeg);
            }
        }

        Err(anyhow::anyhow!("No preview JPEG found"))
    }

    fn extract_from_ifd0(
        &mut self,
        reader: &mut BufReader<&mut File>,
        header: &TiffHeader,
    ) -> Result<Vec<u8>> {
        let ifd0 = self.parser.read_ifd(reader, header.ifd_offset)?;

        if let (Some(offset_entry), Some(length_entry)) = (
            ifd0.get(&TIFF_TAG_JPEG_INTERCHANGE_FORMAT),
            ifd0.get(&TIFF_TAG_JPEG_INTERCHANGE_FORMAT_LENGTH),
        ) {
            return self.read_jpeg(reader, offset_entry.value_offset, length_entry.value_offset);
        }

        if let (Some(offset_entry), Some(length_entry)) = (
            ifd0.get(&NIKON_TAG_PREVIEW_IMAGE_START),
            ifd0.get(&NIKON_TAG_PREVIEW_IMAGE_LENGTH),
        ) {
            return self.read_jpeg(reader, offset_entry.value_offset, length_entry.value_offset);
        }

        Err(anyhow::anyhow!("No JPEG in IFD0"))
    }

    fn extract_from_subifd(
        &mut self,
        reader: &mut BufReader<&mut File>,
        header: &TiffHeader,
    ) -> Result<Vec<u8>> {
        let ifd0 = self.parser.read_ifd(reader, header.ifd_offset)?;

        if let Some(subifd_entry) = ifd0.get(&330) {
            if let Ok(subifd_offsets) = self.parser.read_u32_array(reader, subifd_entry) {
                for &offset in &subifd_offsets {
                    if let Ok(subifd) = self.parser.read_ifd(reader, offset as u64) {
                        if let (Some(offset_entry), Some(length_entry)) = (
                            subifd.get(&TIFF_TAG_JPEG_INTERCHANGE_FORMAT),
                            subifd.get(&TIFF_TAG_JPEG_INTERCHANGE_FORMAT_LENGTH),
                        ) {
                            if let Ok(jpeg) = self.read_jpeg(
                                reader,
                                offset_entry.value_offset,
                                length_entry.value_offset,
                            ) {
                                if !jpeg.is_empty() {
                                    return Ok(jpeg);
                                }
                            }
                        }
                    }
                }
            }
        }

        Err(anyhow::anyhow!("No JPEG in SubIFDs"))
    }

    fn extract_from_ifd1(
        &mut self,
        reader: &mut BufReader<&mut File>,
        header: &TiffHeader,
    ) -> Result<Vec<u8>> {
        let _ifd0 = self.parser.read_ifd(reader, header.ifd_offset)?;
        let ifd1_offset = self.parser.read_next_ifd_offset(reader)?;
        if ifd1_offset == 0 {
            return Err(anyhow::anyhow!("No IFD1 found"));
        }

        let ifd1 = self.parser.read_ifd(reader, ifd1_offset)?;
        if let (Some(offset_entry), Some(length_entry)) = (
            ifd1.get(&TIFF_TAG_JPEG_INTERCHANGE_FORMAT),
            ifd1.get(&TIFF_TAG_JPEG_INTERCHANGE_FORMAT_LENGTH),
        ) {
            return self.read_jpeg(reader, offset_entry.value_offset, length_entry.value_offset);
        }

        Err(anyhow::anyhow!("No JPEG in IFD1"))
    }

    fn read_jpeg(
        &self,
        reader: &mut BufReader<&mut File>,
        offset: u32,
        length: u32,
    ) -> Result<Vec<u8>> {
        reader.seek(SeekFrom::Start(offset as u64))?;
        let mut data = vec![0u8; length as usize];
        reader.read_exact(&mut data)?;
        if data.starts_with(&[0xFF, 0xD8]) {
            Ok(data)
        } else {
            Err(anyhow::anyhow!("Invalid JPEG header"))
        }
    }

    fn scan_for_jpeg_markers(&mut self, reader: &mut BufReader<&mut File>) -> Result<Vec<u8>> {
        reader.seek(SeekFrom::Start(0))?;
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;

        for i in 0..buffer.len().saturating_sub(1) {
            if buffer[i] == 0xFF && buffer[i + 1] == 0xD8 {
                for j in (i + 2)..buffer.len().saturating_sub(1) {
                    if buffer[j] == 0xFF && buffer[j + 1] == 0xD9 {
                        let jpeg = buffer[i..=j + 1].to_vec();
                        if jpeg.len() > 1024 && jpeg.len() < 10_000_000 {
                            return Ok(jpeg);
                        }
                    }
                }
            }
        }

        Err(anyhow::anyhow!("No JPEG markers found"))
    }
}

impl Default for VisionPreviewExtractor {
    fn default() -> Self {
        Self::new()
    }
}
