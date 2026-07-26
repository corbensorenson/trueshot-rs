//! Export functionality for TrueShot
//!
//! Provides export to various formats:
//! - Image formats: PNG, JPEG, TIFF (16-bit)
//! - 3D formats: glTF, USD, PLY
//! - Metadata: XMP sidecar

pub mod gltf;
pub mod usd;
pub mod ply;
pub mod obj;
pub mod stl;
pub mod digital_twin;
pub mod usdz;
pub mod fbx;

// Re-export for convenience
pub use gltf::{export_gltf, export_glb};
pub use usd::export_usd;
pub use ply::{export_ply, export_point_cloud_ply, PlyExportOptions};
pub use obj::export_obj;
pub use stl::export_stl;
pub use usdz::export_usdz;
pub use fbx::export_fbx;

use anyhow::{Context, Result};
use image::ImageEncoder;
use ndarray::{Array2, Array3};
use std::path::Path;
use crate::security::provenance::{ProvenanceOptions, write_provenance_sidecar};
// New import for Feature 6

// Existing functions (save_png, save_jpeg, save_tiff) omitted for brevity if unmodified, 
// BUT `save_tiff16_from_f64` needs to inject metadata.
// I will rewrite the *whole file* to include the new metadata injection logic.
// Or wait, `image` crate support for writing EXIF is very limited/non-existent for custom tags in `save_buffer`.
// We need to use `img-parts` or similar to inject EXIF after saving, or use a custom Tiff writer.
// Since `image` 0.24 is the dependency, let's see if we can use a helper. 
// Writing metadata to TIFF is complex.
// For "Production Ready", we should probably write a sidecar XMP file, which is robust and standard.
// Injecting into TIFF header requires a TIFF library that supports read/write of tags. 
// `tiff` crate (dependency added) has `TiffEncoder`. It allows custom tags? Not easily.
// I will implement XMP Sidecar writing which is standard for archivals.

/// Save XMP Sidecar (Feature 6)
pub fn save_xmp_sidecar(path: &Path, meta_map: &std::collections::HashMap<String, String>) -> Result<()> {
    // Basic XMP template
    let mut xmp = String::from(r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
        xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/"
        xmlns:dc="http://purl.org/dc/elements/1.1/">
"#);
    
    for (key, val) in meta_map {
        xmp.push_str(&format!("      <{}>{}</{}>\n", key, val, key));
    }

    xmp.push_str(r#"    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#);

    let xmp_path = path.with_extension("xmp");
    std::fs::write(&xmp_path, xmp)
        .with_context(|| format!("Failed to write XMP sidecar: {:?}", xmp_path))?;
        
    Ok(())
}

pub fn write_provenance_for_export(path: &Path) -> Result<()> {
    let options = ProvenanceOptions::from_env();
    let _ = write_provenance_sidecar(path, options)?;
    Ok(())
}

/// Helper to save depth tiff (public now)
pub fn save_depth_tiff(depth: &Array2<f32>, path: &Path) -> Result<()> {
    let (height, width) = depth.dim();
    let mut depth_data = vec![0f32; height * width];
    for y in 0..height {
        for x in 0..width {
            depth_data[y*width+x] = depth[[y,x]];
        }
    }
    let depth_u16: Vec<u16> = depth_data.iter().map(|&f| (f * 65535.0).round().clamp(0.0, 65535.0) as u16).collect();
    let file = std::fs::File::create(path).context("Create depth file")?;
    let encoder = image::codecs::tiff::TiffEncoder::new(file);
    let bytes: Vec<u8> = depth_u16.iter().flat_map(|v| v.to_le_bytes()).collect();
    encoder.write_image(&bytes, width as u32, height as u32, image::ColorType::L16.into())?;
    write_provenance_for_export(path)?;
    Ok(())
}

// ... Re-implement save_png, save_jpeg, save_tiff16_from_f64 ...
pub fn save_png(rgb: &Array3<u8>, mask: &Array2<u8>, path: &Path) -> Result<()> {
    let (h, w, _) = rgb.dim();
    let mut rgba = vec![0u8; h * w * 4];
    for ((y, x), &m) in mask.indexed_iter() {
        let idx = (y * w + x) * 4;
        rgba[idx] = rgb[[y,x,0]];
        rgba[idx+1] = rgb[[y,x,1]];
        rgba[idx+2] = rgb[[y,x,2]];
        rgba[idx+3] = m;
    }
    image::save_buffer(path, &rgba, w as u32, h as u32, image::ColorType::Rgba8)?;
    write_provenance_for_export(path)?;
    Ok(())
}

pub fn save_jpeg(rgb: &Array3<u8>, mask: &Array2<u8>, path: &Path) -> Result<()> {
    let (h, w, _) = rgb.dim();
    let mut data = vec![0u8; h * w * 3];
    for ((y, x), &m) in mask.indexed_iter() {
        let idx = (y * w + x) * 3;
        if m > 0 {
            data[idx] = rgb[[y,x,0]];
            data[idx+1] = rgb[[y,x,1]];
            data[idx+2] = rgb[[y,x,2]];
        }
    }
    image::save_buffer(path, &data, w as u32, h as u32, image::ColorType::Rgb8)?;
    write_provenance_for_export(path)?;
    Ok(())
}

pub fn save_tiff16_from_f64(rgb: &Array3<f64>, mask: &Array2<bool>, path: &Path) -> Result<()> {
    let (h, w, _) = rgb.dim();
    let mut buf = vec![0u16; h * w * 4];
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) * 4;
            buf[idx] = (rgb[[y,x,0]] * 65535.0).clamp(0.0, 65535.0) as u16;
            buf[idx+1] = (rgb[[y,x,1]] * 65535.0).clamp(0.0, 65535.0) as u16;
            buf[idx+2] = (rgb[[y,x,2]] * 65535.0).clamp(0.0, 65535.0) as u16;
            buf[idx+3] = if mask[[y,x]] { 65535 } else { 0 };
        }
    }
    let bytes: Vec<u8> = buf.iter().flat_map(|v| v.to_le_bytes()).collect();
    let file = std::fs::File::create(path)?;
    let encoder = image::codecs::tiff::TiffEncoder::new(file);
    encoder.write_image(&bytes, w as u32, h as u32, image::ColorType::Rgba16.into())?;
    write_provenance_for_export(path)?;
    Ok(())
}

pub fn generate_output_path(dir: &Path, bone: &str, vantage: &str, rot: f32) -> std::path::PathBuf {
    dir.join(format!("{}_{}_{:03}deg.tiff", bone, vantage, rot as u32))
}
