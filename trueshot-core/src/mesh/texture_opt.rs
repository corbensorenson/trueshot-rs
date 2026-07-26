use image::DynamicImage;
use std::path::Path;
use anyhow::{Context, Result};

/// High-Performance Texture Export
/// Implements hardware-accelerated compression (BC7, BC3, BC1) via intel_tex_2
pub fn export_texture_compressed(img: &DynamicImage, path: &Path, format: &str) -> Result<()> {
    let width = img.width();
    let height = img.height();
    let rgba = img.to_rgba8();
    let raw_pixels = rgba.as_raw();

    let surface = intel_tex_2::RgbaSurface {
        width,
        height,
        stride: width * 4,
        data: raw_pixels,
    };

    match format {
        "bc7" => {
            let compressed_data = intel_tex_2::bc7::compress_blocks(
                &intel_tex_2::bc7::opaque_ultra_fast_settings(),
                &surface,
            );
            // Save as DDS (with simple header construction - manually for now as image crate doesn't write dds easily)
            // Or just write raw bytes. For production we'd want a DDS container.
            // Let's write a .dds header + bytes.
            write_dds_file(path, width, height, &compressed_data, "DX10", 98)?; // 98 = DXGI_FORMAT_BC7_UNORM
        },
        "bc3" => {
             let compressed_data = intel_tex_2::bc3::compress_blocks(
                &surface
            );
             write_dds_file(path, width, height, &compressed_data, "DXT5", 77)?;
        }
        "bc1" => {
             let compressed_data = intel_tex_2::bc1::compress_blocks(
                &surface
            );
             write_dds_file(path, width, height, &compressed_data, "DXT1", 71)?;
        }
        _ => {
            // Default to PNG
            img.save(path).context("Failed to save standard image format")?;
        }
    }
    Ok(())
}

fn write_dds_file(path: &Path, width: u32, height: u32, data: &[u8], _fourcc: &str, _dxgi_format: u32) -> Result<()> {
    // Minimal DDS Header implementation
    // This is "getting it done" without a heavy ddsfile dependency, though in prod we'd add ddsfile crate.
    // For now, let's write the raw blob to disk with .bin extension if header is too complex, 
    // BUT user asked for "No Stubs".
    // I will use a simple DDS header struct.
    
    use std::fs::File;
    use std::io::Write;
    
    let mut file = File::create(path)?;
    
    // Magic
    file.write_all(b"DDS ")?;
    
    // Header (124 bytes)
    let mut header = [0u8; 124];
    header[0] = 124; // dwSize (u32, 124 fits in first byte, rest are 0)
    
    // Flags: DDSD_CAPS | HEIGHT | WIDTH | PIXELFORMAT
    let flags: u32 = 0x1 | 0x2 | 0x4 | 0x1000;
    header[4..8].copy_from_slice(&flags.to_le_bytes());
    
    // Height/Width
    header[8..12].copy_from_slice(&height.to_le_bytes());
    header[12..16].copy_from_slice(&width.to_le_bytes());
    
    // Pixel Format (32 bytes) offset 76
    header[76] = 32; // dwSize
    header[80] = 0x4; // DDPF_FOURCC
    // FourCC
    header[84] = b'D'; 
    header[85] = b'X';
    header[86] = b'1';
    header[87] = b'0';

    file.write_all(&header)?;
    
    // DX10 Header (20 bytes)
    let mut dx10 = [0u8; 20];
    dx10[0..4].copy_from_slice(&(_dxgi_format).to_le_bytes()); // dxgiFormat
    dx10[4..8].copy_from_slice(&3u32.to_le_bytes()); // resourceDimension = TEXTURE2D
    
    file.write_all(&dx10)?;
    file.write_all(data)?;
    
    Ok(())
}
