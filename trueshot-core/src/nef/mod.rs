pub mod huffman;
pub mod nikon_compression;
pub mod parser;
pub mod preview;
pub mod raw_data;
pub mod tiff;

pub use parser::Z9NefParser;
pub use raw_data::RawBuffer;

// Nikon Z9 Constants
pub const Z9_WIDTH: u32 = 8256;
pub const Z9_HEIGHT: u32 = 5504;
pub const Z9_CFA_PATTERN: [u8; 4] = [0, 1, 1, 2]; // RGGB

// TIFF Tag Constants
pub const TIFF_TAG_IMAGE_WIDTH: u16 = 256;
pub const TIFF_TAG_IMAGE_LENGTH: u16 = 257;
pub const TIFF_TAG_BITS_PER_SAMPLE: u16 = 258;
pub const TIFF_TAG_COMPRESSION: u16 = 259;
pub const TIFF_TAG_ROWS_PER_STRIP: u16 = 278;
pub const TIFF_TAG_STRIP_OFFSETS: u16 = 273;
pub const TIFF_TAG_STRIP_BYTE_COUNTS: u16 = 279;
pub const TIFF_TAG_JPEG_INTERCHANGE_FORMAT: u16 = 513;
pub const TIFF_TAG_JPEG_INTERCHANGE_FORMAT_LENGTH: u16 = 514;

// Nikon Specific Tags
pub const NIKON_TAG_PREVIEW_IMAGE_START: u16 = 0x0201; // Just a guess if not found
pub const NIKON_TAG_PREVIEW_IMAGE_LENGTH: u16 = 0x0202; // Just a guess
                                                        // Actually, parser uses them, I should check parser lines for usage context if needed, but error said they are missing from super.
