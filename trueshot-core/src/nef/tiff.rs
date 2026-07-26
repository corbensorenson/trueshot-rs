/// TIFF format parsing for NEF files
///
/// This module handles the low-level TIFF structure parsing.
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};

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

    pub fn read_header(&mut self, reader: &mut BufReader<&mut File>) -> Result<TiffHeader> {
        let mut header_bytes = [0u8; 8];
        reader.read_exact(&mut header_bytes)
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

    pub fn read_ifd(&self, reader: &mut BufReader<&mut File>, offset: u64) -> Result<HashMap<u16, IfdEntry>> {
        // Seek to IFD offset
        reader.seek(SeekFrom::Start(offset))
            .context("Failed to seek to IFD")?;

        // Read number of directory entries
        let mut count_bytes = [0u8; 2];
        reader.read_exact(&mut count_bytes)
            .context("Failed to read IFD entry count")?;

        let entry_count = self.read_u16(&count_bytes)?;
        let mut ifd = HashMap::new();

        // Read each IFD entry (12 bytes each)
        for _ in 0..entry_count {
            let mut entry_bytes = [0u8; 12];
            reader.read_exact(&mut entry_bytes)
                .context("Failed to read IFD entry")?;

            let tag = self.read_u16(&entry_bytes[0..2])?;
            let data_type = self.read_u16(&entry_bytes[2..4])?;
            let count = self.read_u32(&entry_bytes[4..8])?;
            let value_offset = self.read_u32(&entry_bytes[8..12])?;

            ifd.insert(tag, IfdEntry {
                tag,
                data_type,
                count,
                value_offset,
            });
        }

        Ok(ifd)
    }

    pub fn read_next_ifd_offset(&self, reader: &mut BufReader<&mut File>) -> Result<u64> {
        let mut offset_bytes = [0u8; 4];
        reader.read_exact(&mut offset_bytes)
            .context("Failed to read next IFD offset")?;
        Ok(self.read_u32(&offset_bytes)? as u64)
    }

    pub fn read_tag_data(&self, reader: &mut BufReader<&mut File>, entry: &IfdEntry) -> Result<Vec<u8>> {
        let data_size = self.get_data_type_size(entry.data_type)? * entry.count as usize;
        
        if data_size <= 4 {
            // Data is stored in the value_offset field itself
            let mut data = Vec::new();
            let bytes = entry.value_offset.to_le_bytes();
            data.extend_from_slice(&bytes[0..data_size]);
            Ok(data)
        } else {
            // Data is stored at the offset
            reader.seek(SeekFrom::Start(entry.value_offset as u64))?;
            let mut data = vec![0u8; data_size];
            reader.read_exact(&mut data)?;
            Ok(data)
        }
    }

    pub fn read_u32_array(&self, reader: &mut BufReader<&mut File>, entry: &IfdEntry) -> Result<Vec<u32>> {
        let data = self.read_tag_data(reader, entry)?;
        let mut result = Vec::new();
        
        for chunk in data.chunks_exact(4) {
            result.push(self.read_u32(chunk)?);
        }
        
        Ok(result)
    }

    pub fn read_u16_array(&self, reader: &mut BufReader<&mut File>, entry: &IfdEntry) -> Result<Vec<u16>> {
        let data = self.read_tag_data(reader, entry)?;
        let mut result = Vec::new();
        
        for chunk in data.chunks_exact(2) {
            result.push(self.read_u16(chunk)?);
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
            1 | 2 | 6 | 7 => Ok(1), // BYTE, ASCII, SBYTE, UNDEFINED
            3 | 8 => Ok(2),          // SHORT, SSHORT
            4 | 9 | 11 => Ok(4),     // LONG, SLONG, FLOAT
            5 | 10 | 12 => Ok(8),    // RATIONAL, SRATIONAL, DOUBLE
            _ => Err(anyhow::anyhow!("Unknown TIFF data type: {}", data_type)),
        }
    }
}

impl Default for TiffParser {
    fn default() -> Self {
        Self::new()
    }
}
