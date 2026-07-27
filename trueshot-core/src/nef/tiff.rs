/// TIFF format parsing for NEF files
///
/// This module handles the low-level TIFF structure parsing.
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

#[derive(Debug, Clone)]
pub enum ByteOrder {
    LittleEndian,
    BigEndian,
}

#[derive(Debug)]
pub struct TiffHeader {
    pub byte_order: ByteOrder,
    pub magic: u16,
    pub ifd_offset: u64,
}

#[derive(Debug, Clone)]
pub struct IfdEntry {
    pub tag: u16,
    pub data_type: u16,
    pub count: u32,
    pub value_offset: u32,
}

#[derive(Debug)]
pub enum TiffDataType {
    Byte = 1,
    Ascii = 2,
    Short = 3,
    Long = 4,
    Rational = 5,
    SByte = 6,
    Undefined = 7,
    SShort = 8,
    SLong = 9,
    SRational = 10,
    Float = 11,
    Double = 12,
}

pub struct TiffParser {
    byte_order: ByteOrder,
}

impl TiffParser {
    pub fn new() -> Self {
        Self {
            byte_order: ByteOrder::LittleEndian,
        }
    }

    pub fn read_header<R: Read + Seek>(&mut self, reader: &mut R) -> Result<TiffHeader> {
        let mut header_bytes = [0u8; 8];
        reader
            .read_exact(&mut header_bytes)
            .context("Failed to read TIFF header")?;

        // Check byte order (first 2 bytes)
        let byte_order = match &header_bytes[0..2] {
            b"II" => ByteOrder::LittleEndian,
            b"MM" => ByteOrder::BigEndian,
            _ => return Err(anyhow::anyhow!("Invalid TIFF byte order marker")),
        };

        self.byte_order = byte_order.clone();

        // Read magic number (should be 42)
        let magic = self.read_u16(&header_bytes[2..4])?;
        if magic != 42 {
            return Err(anyhow::anyhow!("Invalid TIFF magic number: {}", magic));
        }

        // Read IFD offset
        let ifd_offset = self.read_u32(&header_bytes[4..8])? as u64;

        Ok(TiffHeader {
            byte_order,
            magic,
            ifd_offset,
        })
    }

    pub fn read_ifd<R: Read + Seek>(
        &self,
        reader: &mut R,
        offset: u64,
    ) -> Result<HashMap<u16, IfdEntry>> {
        // Seek to IFD offset
        reader
            .seek(SeekFrom::Start(offset))
            .context("Failed to seek to IFD")?;

        // Read number of directory entries
        let mut count_bytes = [0u8; 2];
        reader
            .read_exact(&mut count_bytes)
            .context("Failed to read IFD entry count")?;

        let entry_count = self.read_u16(&count_bytes)?;
        let mut ifd = HashMap::new();

        // Read each IFD entry (12 bytes each)
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
                    tag,
                    data_type,
                    count,
                    value_offset,
                },
            );
        }

        Ok(ifd)
    }

    pub fn read_next_ifd_offset<R: Read + Seek>(&self, reader: &mut R) -> Result<u64> {
        let mut offset_bytes = [0u8; 4];
        reader
            .read_exact(&mut offset_bytes)
            .context("Failed to read next IFD offset")?;
        Ok(self.read_u32(&offset_bytes)? as u64)
    }

    pub fn read_tag_data<R: Read + Seek>(
        &self,
        reader: &mut R,
        entry: &IfdEntry,
    ) -> Result<Vec<u8>> {
        let data_size = self
            .get_data_type_size(entry.data_type)?
            .checked_mul(entry.count as usize)
            .context("TIFF tag data size overflow")?;

        if data_size <= 4 {
            // Data is stored in the value_offset field itself
            let bytes = match self.byte_order {
                ByteOrder::LittleEndian => entry.value_offset.to_le_bytes(),
                ByteOrder::BigEndian => entry.value_offset.to_be_bytes(),
            };
            Ok(bytes[..data_size].to_vec())
        } else {
            // Data is stored at the offset
            reader.seek(SeekFrom::Start(entry.value_offset as u64))?;
            let mut data = vec![0u8; data_size];
            reader.read_exact(&mut data)?;
            Ok(data)
        }
    }

    pub fn read_ascii<R: Read + Seek>(&self, reader: &mut R, entry: &IfdEntry) -> Result<String> {
        if entry.data_type != TiffDataType::Ascii as u16 {
            anyhow::bail!(
                "TIFF tag {} is type {}, expected ASCII",
                entry.tag,
                entry.data_type
            );
        }
        if entry.count == 0 || entry.count > 4096 {
            anyhow::bail!(
                "TIFF ASCII tag {} has invalid length {}",
                entry.tag,
                entry.count
            );
        }
        let bytes = self.read_tag_data(reader, entry)?;
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        let value = std::str::from_utf8(&bytes[..end])
            .context("TIFF ASCII tag is not valid UTF-8")?
            .trim();
        if value.is_empty() {
            anyhow::bail!("TIFF ASCII tag {} is empty", entry.tag);
        }
        Ok(value.to_owned())
    }

    pub fn read_u32_array<R: Read + Seek>(
        &self,
        reader: &mut R,
        entry: &IfdEntry,
    ) -> Result<Vec<u32>> {
        let data = self.read_tag_data(reader, entry)?;
        let mut result = Vec::new();

        for chunk in data.chunks_exact(4) {
            result.push(self.read_u32(chunk)?);
        }

        Ok(result)
    }

    pub fn read_u16_array<R: Read + Seek>(
        &self,
        reader: &mut R,
        entry: &IfdEntry,
    ) -> Result<Vec<u16>> {
        let data = self.read_tag_data(reader, entry)?;
        let mut result = Vec::new();

        for chunk in data.chunks_exact(2) {
            result.push(self.read_u16(chunk)?);
        }

        Ok(result)
    }

    pub fn read_unsigned_scalar<R: Read + Seek>(
        &self,
        reader: &mut R,
        entry: &IfdEntry,
    ) -> Result<u32> {
        if entry.count == 0 {
            anyhow::bail!("TIFF tag {} has no values", entry.tag);
        }
        let data = self.read_tag_data(reader, entry)?;
        match entry.data_type {
            1 => data
                .first()
                .copied()
                .map(u32::from)
                .context("TIFF BYTE tag has no data"),
            3 => self.read_u16(&data).map(u32::from),
            4 => self.read_u32(&data),
            other => anyhow::bail!(
                "TIFF tag {} has unsupported scalar type {}",
                entry.tag,
                other
            ),
        }
    }

    pub fn read_rational<R: Read + Seek>(&self, reader: &mut R, entry: &IfdEntry) -> Result<f64> {
        if entry.data_type != TiffDataType::Rational as u16 || entry.count == 0 || entry.count > 16
        {
            anyhow::bail!("TIFF tag {} is not a non-empty rational", entry.tag);
        }
        let data = self.read_tag_data(reader, entry)?;
        if data.len() < 8 {
            anyhow::bail!("TIFF rational tag {} is truncated", entry.tag);
        }
        let numerator = self.read_u32(&data[..4])?;
        let denominator = self.read_u32(&data[4..8])?;
        if denominator == 0 {
            anyhow::bail!("TIFF rational tag {} has a zero denominator", entry.tag);
        }
        Ok(f64::from(numerator) / f64::from(denominator))
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
            1 | 2 | 6 | 7 => Ok(1), // BYTE, ASCII, SBYTE, UNDEFINED
            3 | 8 => Ok(2),         // SHORT, SSHORT
            4 | 9 | 11 => Ok(4),    // LONG, SLONG, FLOAT
            5 | 10 | 12 => Ok(8),   // RATIONAL, SRATIONAL, DOUBLE
            _ => Err(anyhow::anyhow!("Unknown TIFF data type: {}", data_type)),
        }
    }
}

impl Default for TiffParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{ByteOrder, IfdEntry, TiffParser};
    use std::io::{BufReader, Seek, SeekFrom, Write};

    #[test]
    fn inline_scalars_respect_tiff_byte_order() {
        let mut file = tempfile::tempfile().expect("temporary TIFF");
        let mut reader = BufReader::new(&mut file);
        let entry = IfdEntry {
            tag: 258,
            data_type: 3,
            count: 1,
            value_offset: 0x1234,
        };
        let little = TiffParser {
            byte_order: ByteOrder::LittleEndian,
        };
        assert_eq!(
            little
                .read_unsigned_scalar(&mut reader, &entry)
                .expect("little-endian SHORT"),
            0x1234
        );

        let entry = IfdEntry {
            value_offset: 0x1234_0000,
            ..entry
        };
        let big = TiffParser {
            byte_order: ByteOrder::BigEndian,
        };
        assert_eq!(
            big.read_unsigned_scalar(&mut reader, &entry)
                .expect("big-endian SHORT"),
            0x1234
        );
    }

    #[test]
    fn ascii_reader_trims_nul_and_rejects_unbounded_lengths() {
        let mut file = tempfile::tempfile().expect("temporary TIFF");
        file.seek(SeekFrom::Start(32)).expect("seek");
        file.write_all(b"NIKON CORPORATION\0").expect("write ASCII");
        let parser = TiffParser::new();
        let mut reader = BufReader::new(&mut file);
        let entry = IfdEntry {
            tag: 271,
            data_type: 2,
            count: 18,
            value_offset: 32,
        };
        assert_eq!(
            parser.read_ascii(&mut reader, &entry).expect("camera make"),
            "NIKON CORPORATION"
        );

        let oversized = IfdEntry {
            count: 4097,
            ..entry
        };
        assert!(parser.read_ascii(&mut reader, &oversized).is_err());
    }
}
