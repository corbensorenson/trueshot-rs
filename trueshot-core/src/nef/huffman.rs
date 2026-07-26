/// LJPEG (Lossless JPEG) decoder for compression tag 6
/// Based on LibRaw implementation
use anyhow::{anyhow, Result};

/// LJPEG header structure
#[derive(Debug)]
pub struct LjpegHeader {
    pub bits: u8,
    pub high: u16,
    pub wide: u16,
    pub clrs: u8,
    pub sraw: u8,
    pub psv: u8,
    pub restart: u16,
    pub vpred: [u16; 6],
    pub huff: [Option<HuffTable>; 20],
    pub quant: [u16; 64],
}

impl Default for LjpegHeader {
    fn default() -> Self {
        Self {
            bits: 0,
            high: 0,
            wide: 0,
            clrs: 0,
            sraw: 0,
            psv: 0,
            restart: 0,
            vpred: [0; 6],
            huff: [
                None, None, None, None, None, None, None, None, None, None, None, None, None, None,
                None, None, None, None, None, None,
            ],
            quant: [0; 64],
        }
    }
}

// LJPEG marker constants
const SOI: u16 = 0xffd8; // Start of Image
const SOF3: u16 = 0xffc3; // Start of Frame (lossless)
const DHT: u16 = 0xffc4; // Define Huffman Table
const SOS: u16 = 0xffda; // Start of Scan
const DQT: u16 = 0xffdb; // Define Quantization Table
const DRI: u16 = 0xffdd; // Define Restart Interval

// Nikon Huffman tables for fallback (if needed)
const NIKON_TREE: [[[u8; 16]; 3]; 6] = [
    [
        // 12-bit lossy
        [0, 0, 1, 5, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0],
        [5, 4, 3, 6, 2, 7, 1, 0, 8, 9, 11, 10, 12, 0, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    ],
    [
        // 12-bit lossy after split
        [0, 0, 1, 5, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0],
        [6, 5, 5, 5, 5, 5, 4, 3, 2, 1, 0, 11, 12, 12, 0, 0],
        [3, 5, 3, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    ],
    [
        // 12-bit lossless
        [0, 0, 1, 4, 2, 3, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
        [5, 4, 6, 3, 7, 2, 8, 1, 9, 0, 10, 11, 12, 0, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    ],
    [
        // 14-bit lossy
        [0, 0, 1, 4, 3, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0],
        [5, 6, 4, 7, 8, 3, 9, 2, 1, 0, 10, 11, 12, 13, 14, 0],
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    ],
    [
        // 14-bit lossy after split
        [0, 0, 1, 5, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0],
        [8, 7, 7, 7, 7, 7, 6, 5, 4, 3, 2, 1, 0, 13, 14, 0],
        [0, 5, 4, 3, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    ],
    [
        // 14-bit lossless
        [0, 0, 1, 4, 2, 2, 3, 1, 2, 0, 0, 0, 0, 0, 0, 0],
        [7, 6, 8, 5, 9, 4, 10, 3, 11, 12, 2, 0, 1, 13, 14, 0],
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    ],
];

#[derive(Debug)]
pub struct HuffTable {
    pub bits: [u32; 16],
    pub huffval: [u32; 16],
    pub shiftval: [u32; 16],
    // Lookup tables for fast decoding
    pub maxcode: [i32; 17],
    pub mincode: [i32; 17],
    pub valptr: [i32; 17],
}

impl HuffTable {
    pub fn empty() -> Self {
        Self {
            bits: [0; 16],
            huffval: [0; 16],
            shiftval: [0; 16],
            maxcode: [-1; 17],
            mincode: [-1; 17],
            valptr: [-1; 17],
        }
    }

    pub fn initialize(&mut self) -> Result<()> {
        // Build the Huffman lookup tables
        let mut huffsize = [0u8; 257];
        let mut huffcode = [0u16; 257];

        // Generate size table
        let mut k = 0;
        for i in 1..=16 {
            for _ in 0..self.bits[i - 1] {
                if k >= 256 {
                    return Err(anyhow!("Huffman table too large"));
                }
                huffsize[k] = i as u8;
                k += 1;
            }
        }
        huffsize[k] = 0;

        // Generate code table
        let mut code = 0u16;
        let mut si = huffsize[0];
        let mut p = 0;

        while huffsize[p] != 0 {
            while huffsize[p] == si {
                huffcode[p] = code;
                code += 1;
                p += 1;
            }
            code <<= 1;
            si += 1;
        }

        // Generate lookup tables
        for l in 1..=16 {
            if self.bits[l - 1] != 0 {
                if p >= huffcode.len() {
                    return Err(anyhow!("Huffman table index out of bounds"));
                }
                self.valptr[l] = p as i32;
                self.mincode[l] = huffcode[p] as i32;
                p += self.bits[l - 1] as usize - 1;
                if p >= huffcode.len() {
                    return Err(anyhow!("Huffman table index out of bounds"));
                }
                self.maxcode[l] = huffcode[p] as i32;
                p += 1;
            } else {
                self.maxcode[l] = -1;
            }
        }

        Ok(())
    }

    /// Build Huffman table from bit counts and values (for Nikon compression)
    pub fn build_from_counts_and_values(&mut self, bit_counts: &[u8], values: &[u8]) -> Result<()> {
        // Copy bit counts
        for (i, &count) in bit_counts.iter().enumerate().take(16) {
            self.bits[i] = count as u32;
        }

        // Copy values (Nikon uses direct symbol values, not differential)
        for (i, &value) in values.iter().enumerate().take(values.len().min(16)) {
            self.huffval[i] = value as u32;
            self.shiftval[i] = 0; // No shift for Nikon
        }

        // Initialize lookup tables
        self.initialize()
    }

    pub fn huff_decode(&self, pump: &mut BitPumpMSB) -> Result<i32> {
        let mut code = 0i32;

        for l in 1..=16 {
            code = (code << 1) | pump.get_bits(1)? as i32;

            if code <= self.maxcode[l] {
                let index = (self.valptr[l] + code - self.mincode[l]) as usize;
                if index < 16 {
                    let val = self.huffval[index] as i32;
                    let shift = self.shiftval[index] as i32;

                    if val == 0 {
                        return Ok(0);
                    }

                    let diff = pump.get_bits(val as u32)? as i32;
                    let result = if diff < (1 << (val - 1)) {
                        diff - (1 << val) + 1
                    } else {
                        diff
                    };

                    return Ok(result << shift);
                }
            }
        }

        Err(anyhow!("Invalid Huffman code"))
    }

    /// Decode Huffman symbol for Nikon compression (returns raw symbol, not differential)
    pub fn nikon_huff_decode(&self, pump: &mut BitPumpMSB) -> Result<u32> {
        let mut code = 0i32;

        for l in 1..=16 {
            code = (code << 1) | pump.get_bits(1)? as i32;

            if code <= self.maxcode[l] {
                let index = (self.valptr[l] + code - self.mincode[l]) as usize;
                if index < 16 {
                    return Ok(self.huffval[index]);
                }
            }
        }

        Err(anyhow!("Invalid Nikon Huffman code"))
    }
}

pub struct BitPumpMSB {
    data: Vec<u8>,
    pos: usize,
    bits: u32,
    nbits: u32,
}

impl BitPumpMSB {
    pub fn new(data: &[u8]) -> Self {
        Self {
            data: data.to_vec(),
            pos: 0,
            bits: 0,
            nbits: 0,
        }
    }

    pub fn get_bits(&mut self, n: u32) -> Result<u32> {
        if n > 32 {
            return Err(anyhow!("Cannot get more than 32 bits"));
        }

        while self.nbits < n {
            if self.pos >= self.data.len() {
                return Err(anyhow!("End of data reached"));
            }

            self.bits = (self.bits << 8) | (self.data[self.pos] as u32);
            self.pos += 1;
            self.nbits += 8;
        }

        let result = (self.bits >> (self.nbits - n)) & ((1 << n) - 1);
        self.nbits -= n;

        Ok(result)
    }

    pub fn peek_bits(&mut self, n: u32) -> Result<u32> {
        if n > 32 {
            return Err(anyhow!("Cannot peek more than 32 bits"));
        }

        // Ensure we have enough bits
        while self.nbits < n {
            if self.pos >= self.data.len() {
                return Err(anyhow!("End of data reached"));
            }

            self.bits = (self.bits << 8) | (self.data[self.pos] as u32);
            self.pos += 1;
            self.nbits += 8;
        }

        // Return bits without consuming them
        let result = (self.bits >> (self.nbits - n)) & ((1 << n) - 1);
        Ok(result)
    }

    pub fn consume_bits(&mut self, n: u32) -> Result<()> {
        if n > self.nbits {
            return Err(anyhow!("Cannot consume more bits than available"));
        }

        self.nbits -= n;
        Ok(())
    }
}

pub fn create_huffman_table(table_index: usize) -> Result<HuffTable> {
    if table_index >= NIKON_TREE.len() {
        return Err(anyhow!("Invalid Huffman table index: {}", table_index));
    }

    let mut htable = HuffTable::empty();

    for i in 0..15 {
        htable.bits[i] = NIKON_TREE[table_index][0][i] as u32;
        htable.huffval[i] = NIKON_TREE[table_index][1][i] as u32;
        htable.shiftval[i] = NIKON_TREE[table_index][2][i] as u32;
    }

    htable.initialize()?;
    Ok(htable)
}

pub fn clamp_bits(value: i32, bits: u32) -> u16 {
    let max_val = (1 << bits) - 1;
    if value < 0 {
        0
    } else if value > max_val {
        max_val as u16
    } else {
        value as u16
    }
}

/// Parse LJPEG header from data stream
pub fn parse_ljpeg_header(data: &[u8]) -> Result<LjpegHeader> {
    let mut header = LjpegHeader::default();
    let mut pos = 0;

    // Check for SOI marker
    if data.len() < 2 || u16::from_be_bytes([data[0], data[1]]) != SOI {
        return Err(anyhow!("Invalid LJPEG: missing SOI marker"));
    }
    pos += 2;

    let mut tag_count = 0;
    while pos < data.len() - 1 && tag_count < 1024 {
        tag_count += 1;

        // Read marker
        if pos + 4 > data.len() {
            break;
        }

        let tag = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;

        if len < 2 || pos + len - 2 > data.len() {
            break;
        }

        let segment_data = &data[pos..pos + len - 2];

        match tag {
            SOF3 => {
                // Start of Frame - lossless
                if segment_data.len() >= 6 {
                    header.bits = segment_data[0];
                    header.high = u16::from_be_bytes([segment_data[1], segment_data[2]]);
                    header.wide = u16::from_be_bytes([segment_data[3], segment_data[4]]);
                    header.clrs = segment_data[5];

                    if segment_data.len() >= 8 {
                        header.sraw = ((segment_data[7] >> 4) * (segment_data[7] & 15) - 1) & 3;
                    }
                }
            }
            DHT => {
                // Define Huffman Table
                parse_huffman_tables(&mut header, segment_data)?;
            }
            SOS => {
                // Start of Scan
                if segment_data.len() >= 4 {
                    header.psv = segment_data[1 + segment_data[0] as usize * 2];
                    header.bits = header
                        .bits
                        .saturating_sub(segment_data[3 + segment_data[0] as usize * 2] & 15);
                }
                break; // End of header
            }
            DQT => {
                // Define Quantization Table
                for i in 0..64.min(segment_data.len() / 2) {
                    header.quant[i] =
                        u16::from_be_bytes([segment_data[i * 2], segment_data[i * 2 + 1]]);
                }
            }
            DRI => {
                // Define Restart Interval
                if segment_data.len() >= 2 {
                    header.restart = u16::from_be_bytes([segment_data[0], segment_data[1]]);
                }
            }
            _ => {
                // Skip unknown markers
            }
        }

        pos += len - 2;
    }

    // Validate header
    if header.bits == 0 || header.high == 0 || header.wide == 0 || header.clrs == 0 {
        return Err(anyhow!("Invalid LJPEG header: missing required fields"));
    }

    // Initialize predictors
    for i in 0..6 {
        header.vpred[i] = 1 << (header.bits - 1);
    }

    Ok(header)
}

fn parse_huffman_tables(header: &mut LjpegHeader, data: &[u8]) -> Result<()> {
    let mut pos = 0;

    while pos < data.len() {
        if pos + 17 > data.len() {
            break;
        }

        let table_id = data[pos] & 0x0f;
        if table_id >= 20 {
            return Err(anyhow!("Invalid Huffman table ID: {}", table_id));
        }

        pos += 1;

        // Read bit counts
        let mut htable = HuffTable::empty();
        for i in 0..16 {
            htable.bits[i] = data[pos + i] as u32;
        }
        pos += 16;

        // Read values
        let mut value_count = 0;
        for i in 0..16 {
            value_count += htable.bits[i];
        }

        if pos + value_count as usize > data.len() {
            return Err(anyhow!("Huffman table data truncated"));
        }

        for i in 0..(value_count as usize).min(16) {
            htable.huffval[i] = data[pos + i] as u32;
        }
        pos += value_count as usize;

        // Initialize the table
        htable.initialize()?;
        header.huff[table_id as usize] = Some(htable);
    }

    Ok(())
}

/// Lookup table for linearization curves
pub struct LookupTable {
    table: Vec<u16>,
}

impl LookupTable {
    pub fn new(points: &[u16]) -> Self {
        Self {
            table: points.to_vec(),
        }
    }

    pub fn dither(&self, value: u16, random: &mut u32) -> u16 {
        // Simple dithering implementation
        *random = random.wrapping_mul(1103515245).wrapping_add(12345);
        let dither = (*random >> 16) & 0xff;

        let index = value as usize;
        if index < self.table.len() {
            let base = self.table[index];
            // Add small random dither
            base.saturating_add((dither & 1) as u16)
        } else {
            value
        }
    }
}
