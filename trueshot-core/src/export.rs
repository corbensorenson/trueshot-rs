//! Export functionality for TrueShot
//!
//! Provides export to various formats:
//! - Image formats: PNG, JPEG, TIFF (16-bit)
//! - 3D formats: glTF, USD, PLY
//! - Metadata: XMP sidecar

pub mod digital_twin;
pub mod fbx;
pub mod gltf;
pub mod obj;
pub mod ply;
pub mod stl;
pub mod usd;
pub mod usdz;

// Re-export for convenience
pub use fbx::export_fbx;
pub use gltf::{export_glb, export_gltf};
pub use obj::export_obj;
pub use ply::{export_ply, export_point_cloud_ply, PlyExportOptions};
pub use stl::export_stl;
pub use usd::export_usd;
pub use usdz::export_usdz;

use crate::security::provenance::{write_provenance_sidecar, ProvenanceOptions};
use anyhow::{Context, Result};
use image::ImageEncoder;
use ndarray::{Array2, Array3};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
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
pub fn save_xmp_sidecar(
    path: &Path,
    meta_map: &std::collections::HashMap<String, String>,
) -> Result<()> {
    // Basic XMP template
    let mut xmp = String::from(
        r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
        xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/"
        xmlns:dc="http://purl.org/dc/elements/1.1/">
"#,
    );

    for (key, val) in meta_map {
        xmp.push_str(&format!("      <{}>{}</{}>\n", key, val, key));
    }

    xmp.push_str(
        r#"    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#,
    );

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

struct PartialExport {
    path: PathBuf,
    published: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportDigest {
    pub size_bytes: u64,
    pub sha256: String,
}

/// Atomically publish an arbitrary diagnostic/report artifact while retaining
/// the same digest and provenance contract as image exports.
pub fn save_bytes_with_digest(bytes: &[u8], path: &Path) -> Result<ExportDigest> {
    let (partial, file) = PartialExport::create(path)?;
    let mut writer = DigestWriter::new(BufWriter::with_capacity(
        bytes.len().clamp(4096, 1024 * 1024),
        file,
    ));
    writer.write_all(bytes)?;
    writer.flush()?;
    let digest = writer.export_digest();
    drop(writer);
    partial.publish(path)?;
    write_provenance_for_export(path)?;
    Ok(digest)
}

struct DigestWriter<W> {
    inner: W,
    digest: Sha256,
    size_bytes: u64,
}

impl<W> DigestWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            digest: Sha256::new(),
            size_bytes: 0,
        }
    }

    fn export_digest(&self) -> ExportDigest {
        ExportDigest {
            size_bytes: self.size_bytes,
            sha256: hex::encode(self.digest.clone().finalize()),
        }
    }
}

impl<W: Write> Write for DigestWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.digest.update(&buffer[..written]);
        self.size_bytes = self.size_bytes.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

struct Tiff16Layout {
    header: Vec<u8>,
    rows_per_strip: usize,
}

fn tiff16_layout(
    width: usize,
    height: usize,
    samples_per_pixel: usize,
    rows_per_strip: usize,
    rgb: bool,
    alpha: bool,
) -> Result<Tiff16Layout> {
    if width == 0 || height == 0 || !matches!(samples_per_pixel, 1 | 4) {
        anyhow::bail!("Unsupported TIFF dimensions or sample count");
    }
    let width_u32 = u32::try_from(width).context("TIFF width exceeds u32")?;
    let height_u32 = u32::try_from(height).context("TIFF height exceeds u32")?;
    let rows_per_strip = rows_per_strip.max(1).min(height);
    let strip_count = height.div_ceil(rows_per_strip);
    let row_bytes = width
        .checked_mul(samples_per_pixel)
        .and_then(|value| value.checked_mul(2))
        .context("TIFF row size overflow")?;
    let entry_count = if alpha { 12usize } else { 11usize };
    let ifd_bytes = 2usize
        .checked_add(entry_count.checked_mul(12).context("TIFF IFD overflow")?)
        .and_then(|value| value.checked_add(4))
        .context("TIFF IFD overflow")?;
    let mut cursor = 8usize
        .checked_add(ifd_bytes)
        .context("TIFF header overflow")?;

    let bits_offset = if samples_per_pixel > 1 {
        let offset = cursor;
        cursor = cursor
            .checked_add(samples_per_pixel * 2)
            .context("TIFF bits array overflow")?;
        Some(offset)
    } else {
        None
    };
    let strip_offsets_array = if strip_count > 1 {
        let offset = cursor;
        cursor = cursor
            .checked_add(strip_count.checked_mul(4).context("TIFF strip overflow")?)
            .context("TIFF strip overflow")?;
        Some(offset)
    } else {
        None
    };
    let strip_counts_array = if strip_count > 1 {
        let offset = cursor;
        cursor = cursor
            .checked_add(strip_count.checked_mul(4).context("TIFF strip overflow")?)
            .context("TIFF strip overflow")?;
        Some(offset)
    } else {
        None
    };
    let sample_format_offset = if samples_per_pixel > 1 {
        let offset = cursor;
        cursor = cursor
            .checked_add(samples_per_pixel * 2)
            .context("TIFF sample-format overflow")?;
        Some(offset)
    } else {
        None
    };
    if cursor % 2 != 0 {
        cursor += 1;
    }
    let pixel_data_offset = cursor;

    let mut strip_offsets = Vec::with_capacity(strip_count);
    let mut strip_counts = Vec::with_capacity(strip_count);
    let mut data_cursor = pixel_data_offset;
    for strip_index in 0..strip_count {
        let start_row = strip_index * rows_per_strip;
        let rows = (height - start_row).min(rows_per_strip);
        let byte_count = rows.checked_mul(row_bytes).context("TIFF strip overflow")?;
        strip_offsets.push(u32::try_from(data_cursor).context("Classic TIFF exceeds 4 GiB")?);
        strip_counts.push(u32::try_from(byte_count).context("Classic TIFF strip exceeds 4 GiB")?);
        data_cursor = data_cursor
            .checked_add(byte_count)
            .context("TIFF file size overflow")?;
    }
    u32::try_from(data_cursor).context("Classic TIFF output exceeds 4 GiB")?;

    let mut header = vec![0u8; pixel_data_offset];
    header[0..2].copy_from_slice(b"II");
    put_u16(&mut header, 2, 42);
    put_u32(&mut header, 4, 8);
    put_u16(
        &mut header,
        8,
        u16::try_from(entry_count).context("Too many TIFF entries")?,
    );
    let mut entry_index = 0usize;
    let mut entry = |tag: u16, field_type: u16, count: u32, value: u32| {
        let offset = 10 + entry_index * 12;
        put_u16(&mut header, offset, tag);
        put_u16(&mut header, offset + 2, field_type);
        put_u32(&mut header, offset + 4, count);
        put_u32(&mut header, offset + 8, value);
        entry_index += 1;
    };
    entry(256, 4, 1, width_u32);
    entry(257, 4, 1, height_u32);
    entry(
        258,
        3,
        samples_per_pixel as u32,
        bits_offset.map(|offset| offset as u32).unwrap_or(16),
    );
    entry(259, 3, 1, 1);
    entry(262, 3, 1, if rgb { 2 } else { 1 });
    entry(
        273,
        4,
        strip_count as u32,
        strip_offsets_array
            .map(|offset| offset as u32)
            .unwrap_or(strip_offsets[0]),
    );
    entry(277, 3, 1, samples_per_pixel as u32);
    entry(278, 4, 1, rows_per_strip as u32);
    entry(
        279,
        4,
        strip_count as u32,
        strip_counts_array
            .map(|offset| offset as u32)
            .unwrap_or(strip_counts[0]),
    );
    entry(284, 3, 1, 1);
    if alpha {
        entry(338, 3, 1, 2);
    }
    entry(
        339,
        3,
        samples_per_pixel as u32,
        sample_format_offset
            .map(|offset| offset as u32)
            .unwrap_or(1),
    );
    put_u32(&mut header, 10 + entry_count * 12, 0);

    if let Some(offset) = bits_offset {
        for index in 0..samples_per_pixel {
            put_u16(&mut header, offset + index * 2, 16);
        }
    }
    if let Some(offset) = strip_offsets_array {
        for (index, value) in strip_offsets.iter().enumerate() {
            put_u32(&mut header, offset + index * 4, *value);
        }
    }
    if let Some(offset) = strip_counts_array {
        for (index, value) in strip_counts.iter().enumerate() {
            put_u32(&mut header, offset + index * 4, *value);
        }
    }
    if let Some(offset) = sample_format_offset {
        for index in 0..samples_per_pixel {
            put_u16(&mut header, offset + index * 2, 1);
        }
    }
    Ok(Tiff16Layout {
        header,
        rows_per_strip,
    })
}

fn put_u16(buffer: &mut [u8], offset: usize, value: u16) {
    buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(buffer: &mut [u8], offset: usize, value: u32) {
    buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

impl PartialExport {
    fn create(target: &Path) -> Result<(Self, File)> {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file_name = target
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| "trueshot-export".into());
        let path = target.with_file_name(format!(".{file_name}.{}.part", uuid::Uuid::new_v4()));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("Create temporary export {}", path.display()))?;
        Ok((
            Self {
                path,
                published: false,
            },
            file,
        ))
    }

    fn publish(mut self, target: &Path) -> Result<()> {
        OpenOptions::new()
            .read(true)
            .open(&self.path)?
            .sync_all()
            .with_context(|| format!("Sync temporary export {}", self.path.display()))?;
        std::fs::rename(&self.path, target)
            .with_context(|| format!("Atomically publish export {}", target.display()))?;
        sync_parent_directory(target)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for PartialExport {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    File::open(parent)?
        .sync_all()
        .with_context(|| format!("Sync export directory {}", parent.display()))
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<()> {
    Ok(())
}

/// Save a normalized depth map as a crash-safe, strip-streamed 16-bit TIFF.
pub fn save_depth_tiff(depth: &Array2<f32>, path: &Path) -> Result<()> {
    save_depth_tiff_with_digest(depth, path).map(|_| ())
}

pub fn save_depth_tiff_with_digest(depth: &Array2<f32>, path: &Path) -> Result<ExportDigest> {
    let (height, width) = depth.dim();
    if width == 0 || height == 0 {
        anyhow::bail!("Cannot export an empty depth map");
    }
    let (partial, file) = PartialExport::create(path)?;
    let mut writer = DigestWriter::new(BufWriter::with_capacity(1024 * 1024, file));
    let layout = tiff16_layout(width, height, 1, 64, false, false)?;
    writer.write_all(&layout.header)?;
    for start_row in (0..height).step_by(layout.rows_per_strip) {
        let end_row = (start_row + layout.rows_per_strip).min(height);
        let mut strip = Vec::with_capacity((end_row - start_row) * width * 2);
        for y in start_row..end_row {
            for x in 0..width {
                let value = depth[[y, x]];
                let value = (value.clamp(0.0, 1.0) * 65_535.0)
                    .round()
                    .clamp(0.0, 65_535.0) as u16;
                strip.extend_from_slice(&value.to_le_bytes());
            }
        }
        writer.write_all(&strip)?;
    }
    writer.flush()?;
    let digest = writer.export_digest();
    drop(writer);
    partial.publish(path)?;
    write_provenance_for_export(path)?;
    Ok(digest)
}

/// Save an exact one-channel `u16` diagnostic map as a compressed, crash-safe
/// PNG. Values are not normalized, so frame IDs and `u16::MAX` sentinels round
/// trip exactly.
pub fn save_u16_map_png_with_digest(values: &Array2<u16>, path: &Path) -> Result<ExportDigest> {
    let (height, width) = values.dim();
    let contiguous = values
        .as_slice()
        .context("u16 diagnostic map must be contiguous")?;
    if width == 0 || height == 0 {
        anyhow::bail!("Cannot export an empty u16 diagnostic map");
    }
    let (partial, file) = PartialExport::create(path)?;
    let mut writer = DigestWriter::new(BufWriter::with_capacity(1024 * 1024, file));
    image::codecs::png::PngEncoder::new_with_quality(
        &mut writer,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        bytemuck::cast_slice(contiguous),
        u32::try_from(width).context("PNG width exceeds u32")?,
        u32::try_from(height).context("PNG height exceeds u32")?,
        image::ExtendedColorType::L16,
    )?;
    writer.flush()?;
    let digest = writer.export_digest();
    drop(writer);
    partial.publish(path)?;
    write_provenance_for_export(path)?;
    Ok(digest)
}

/// Save an exact one-channel `u8` diagnostic map as a compressed, crash-safe
/// PNG. Bitwise fusion flags therefore remain machine-readable.
pub fn save_u8_map_png_with_digest(values: &Array2<u8>, path: &Path) -> Result<ExportDigest> {
    let (height, width) = values.dim();
    let contiguous = values
        .as_slice()
        .context("u8 diagnostic map must be contiguous")?;
    if width == 0 || height == 0 {
        anyhow::bail!("Cannot export an empty u8 diagnostic map");
    }
    let (partial, file) = PartialExport::create(path)?;
    let mut writer = DigestWriter::new(BufWriter::with_capacity(1024 * 1024, file));
    image::codecs::png::PngEncoder::new_with_quality(
        &mut writer,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        contiguous,
        u32::try_from(width).context("PNG width exceeds u32")?,
        u32::try_from(height).context("PNG height exceeds u32")?,
        image::ExtendedColorType::L8,
    )?;
    writer.flush()?;
    let digest = writer.export_digest();
    drop(writer);
    partial.publish(path)?;
    write_provenance_for_export(path)?;
    Ok(digest)
}

/// Save metric depth without quantization as a crash-safe little-endian PFM.
///
/// PFM stores rows bottom-to-top and uses a negative scale to declare
/// little-endian IEEE-754 samples.
pub fn save_metric_depth_pfm_with_digest(
    depth_m: &Array2<f32>,
    path: &Path,
) -> Result<ExportDigest> {
    let (height, width) = depth_m.dim();
    if width == 0 || height == 0 {
        anyhow::bail!("Cannot export an empty metric depth map");
    }
    if depth_m
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
    {
        anyhow::bail!("Metric depth map contains invalid distances");
    }
    let (partial, file) = PartialExport::create(path)?;
    let mut writer = DigestWriter::new(BufWriter::with_capacity(1024 * 1024, file));
    write!(writer, "Pf\n{width} {height}\n-1.0\n")?;
    for y in (0..height).rev() {
        for x in 0..width {
            writer.write_all(&depth_m[[y, x]].to_le_bytes())?;
        }
    }
    writer.flush()?;
    let digest = writer.export_digest();
    drop(writer);
    partial.publish(path)?;
    write_provenance_for_export(path)?;
    Ok(digest)
}

/// Save an RGBA preview through a same-directory atomic rename.
pub fn save_png(rgb: &Array3<u8>, mask: &Array2<u8>, path: &Path) -> Result<()> {
    save_png_with_digest(rgb, mask, path).map(|_| ())
}

pub fn save_png_with_digest(
    rgb: &Array3<u8>,
    mask: &Array2<u8>,
    path: &Path,
) -> Result<ExportDigest> {
    save_png_preview_with_digest(rgb, mask, path, usize::MAX)
}

pub fn save_png_preview_with_digest(
    rgb: &Array3<u8>,
    mask: &Array2<u8>,
    path: &Path,
    max_dimension: usize,
) -> Result<ExportDigest> {
    let (h, w, channels) = rgb.dim();
    if channels != 3 || mask.dim() != (h, w) {
        anyhow::bail!(
            "PNG export shape mismatch: RGB {:?}, mask {:?}",
            rgb.dim(),
            mask.dim()
        );
    }
    if w == 0 || h == 0 || max_dimension == 0 {
        anyhow::bail!("Cannot export an empty or zero-sized PNG preview");
    }
    let scale = (max_dimension as f64 / w.max(h) as f64).min(1.0);
    let output_width = ((w as f64 * scale).round() as usize).max(1);
    let output_height = ((h as f64 * scale).round() as usize).max(1);
    let mut rgba = vec![0u8; output_height * output_width * 4];
    for output_y in 0..output_height {
        let source_y = ((output_y as f64 + 0.5) / scale - 0.5).clamp(0.0, h as f64 - 1.0);
        let y0 = source_y.floor() as usize;
        let y1 = (y0 + 1).min(h - 1);
        let wy = source_y - y0 as f64;
        for output_x in 0..output_width {
            let source_x = ((output_x as f64 + 0.5) / scale - 0.5).clamp(0.0, w as f64 - 1.0);
            let x0 = source_x.floor() as usize;
            let x1 = (x0 + 1).min(w - 1);
            let wx = source_x - x0 as f64;
            let idx = (output_y * output_width + output_x) * 4;
            for channel in 0..3 {
                rgba[idx + channel] = bilinear_u8(
                    rgb[[y0, x0, channel]],
                    rgb[[y0, x1, channel]],
                    rgb[[y1, x0, channel]],
                    rgb[[y1, x1, channel]],
                    wx,
                    wy,
                );
            }
            rgba[idx + 3] = bilinear_u8(
                mask[[y0, x0]],
                mask[[y0, x1]],
                mask[[y1, x0]],
                mask[[y1, x1]],
                wx,
                wy,
            );
        }
    }
    let (partial, file) = PartialExport::create(path)?;
    let mut writer = DigestWriter::new(BufWriter::with_capacity(1024 * 1024, file));
    image::codecs::png::PngEncoder::new(&mut writer).write_image(
        &rgba,
        u32::try_from(output_width).context("PNG width exceeds u32")?,
        u32::try_from(output_height).context("PNG height exceeds u32")?,
        image::ExtendedColorType::Rgba8,
    )?;
    writer.flush()?;
    let digest = writer.export_digest();
    drop(writer);
    partial.publish(path)?;
    write_provenance_for_export(path)?;
    Ok(digest)
}

fn bilinear_u8(
    top_left: u8,
    top_right: u8,
    bottom_left: u8,
    bottom_right: u8,
    wx: f64,
    wy: f64,
) -> u8 {
    let top = top_left as f64 * (1.0 - wx) + top_right as f64 * wx;
    let bottom = bottom_left as f64 * (1.0 - wx) + bottom_right as f64 * wx;
    (top * (1.0 - wy) + bottom * wy).round().clamp(0.0, 255.0) as u8
}

pub fn save_jpeg(rgb: &Array3<u8>, mask: &Array2<u8>, path: &Path) -> Result<()> {
    let (h, w, _) = rgb.dim();
    let mut data = vec![0u8; h * w * 3];
    for ((y, x), &m) in mask.indexed_iter() {
        let idx = (y * w + x) * 3;
        if m > 0 {
            data[idx] = rgb[[y, x, 0]];
            data[idx + 1] = rgb[[y, x, 1]];
            data[idx + 2] = rgb[[y, x, 2]];
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
            buf[idx] = (rgb[[y, x, 0]] * 65535.0).clamp(0.0, 65535.0) as u16;
            buf[idx + 1] = (rgb[[y, x, 1]] * 65535.0).clamp(0.0, 65535.0) as u16;
            buf[idx + 2] = (rgb[[y, x, 2]] * 65535.0).clamp(0.0, 65535.0) as u16;
            buf[idx + 3] = if mask[[y, x]] { 65535 } else { 0 };
        }
    }
    let bytes: Vec<u8> = buf.iter().flat_map(|v| v.to_le_bytes()).collect();
    let file = std::fs::File::create(path)?;
    let encoder = image::codecs::tiff::TiffEncoder::new(file);
    encoder.write_image(&bytes, w as u32, h as u32, image::ColorType::Rgba16.into())?;
    write_provenance_for_export(path)?;
    Ok(())
}

/// Stream a native linear RGB image into a crash-safe 16-bit TIFF.
///
/// Only one small strip is materialized. The completed file is renamed from a
/// same-directory temporary path so interrupted exports never appear valid.
pub fn save_tiff16_from_f32(rgb: &Array3<f32>, mask: &Array2<u8>, path: &Path) -> Result<()> {
    save_tiff16_from_f32_with_digest(rgb, mask, path).map(|_| ())
}

pub fn save_tiff16_from_f32_with_digest(
    rgb: &Array3<f32>,
    mask: &Array2<u8>,
    path: &Path,
) -> Result<ExportDigest> {
    let (height, width, channels) = rgb.dim();
    if channels != 3 || mask.dim() != (height, width) {
        anyhow::bail!(
            "TIFF export shape mismatch: RGB {:?}, mask {:?}",
            rgb.dim(),
            mask.dim()
        );
    }
    let (partial, file) = PartialExport::create(path)?;
    let mut writer = DigestWriter::new(BufWriter::with_capacity(1024 * 1024, file));
    let layout = tiff16_layout(width, height, 4, 32, true, true)?;
    writer.write_all(&layout.header)?;
    for start_row in (0..height).step_by(layout.rows_per_strip) {
        let end_row = (start_row + layout.rows_per_strip).min(height);
        let mut strip = Vec::with_capacity((end_row - start_row) * width * 8);
        for y in start_row..end_row {
            for x in 0..width {
                for channel in 0..3 {
                    let value = (rgb[[y, x, channel]] * 65_535.0).clamp(0.0, 65_535.0) as u16;
                    strip.extend_from_slice(&value.to_le_bytes());
                }
                let alpha = if mask[[y, x]] != 0 { u16::MAX } else { 0 };
                strip.extend_from_slice(&alpha.to_le_bytes());
            }
        }
        writer.write_all(&strip)?;
    }
    writer.flush()?;
    let digest = writer.export_digest();
    drop(writer);
    partial.publish(path)?;
    write_provenance_for_export(path)?;
    Ok(digest)
}

pub fn generate_output_path(dir: &Path, bone: &str, vantage: &str, rot: f32) -> std::path::PathBuf {
    dir.join(format!("{}_{}_{:03}deg.tiff", bone, vantage, rot as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiff::decoder::{Decoder, DecodingResult};

    #[test]
    fn f32_tiff_stream_is_readable_and_leaves_no_partial_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.tiff");
        let rgb = Array3::from_shape_vec(
            (2, 2, 3),
            vec![
                0.0f32, 0.25, 0.5, 0.5, 0.75, 1.0, 1.0, 0.5, 0.0, 0.1, 0.2, 0.3,
            ],
        )
        .unwrap();
        let mask = Array2::from_shape_vec((2, 2), vec![255u8, 0, 255, 255]).unwrap();
        let digest = save_tiff16_from_f32_with_digest(&rgb, &mask, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(digest.size_bytes, bytes.len() as u64);
        assert_eq!(digest.sha256, hex::encode(Sha256::digest(&bytes)));

        let mut decoder = Decoder::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(decoder.dimensions().unwrap(), (2, 2));
        let DecodingResult::U16(pixels) = decoder.read_image().unwrap() else {
            panic!("Expected 16-bit TIFF samples");
        };
        assert_eq!(pixels.len(), 16);
        assert_eq!(pixels[3], u16::MAX);
        assert_eq!(pixels[7], 0);
        assert!(directory.path().read_dir().unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".part")
        }));
    }

    #[test]
    fn png_and_depth_exports_are_atomic_and_readable() {
        let directory = tempfile::tempdir().unwrap();
        let rgb = Array3::from_shape_vec(
            (2, 2, 3),
            vec![0, 32, 64, 96, 128, 160, 192, 224, 255, 1, 2, 3],
        )
        .unwrap();
        let mask = Array2::from_shape_vec((2, 2), vec![255, 0, 128, 255]).unwrap();
        let png = directory.path().join("preview.png");
        let png_digest = save_png_with_digest(&rgb, &mask, &png).unwrap();
        let png_bytes = std::fs::read(&png).unwrap();
        assert_eq!(png_digest.size_bytes, png_bytes.len() as u64);
        assert_eq!(png_digest.sha256, hex::encode(Sha256::digest(&png_bytes)));
        let decoded = image::open(&png).unwrap().into_rgba8();
        assert_eq!(decoded.get_pixel(1, 0).0[3], 0);
        let thumbnail = directory.path().join("thumbnail.png");
        save_png_preview_with_digest(&rgb, &mask, &thumbnail, 1).unwrap();
        assert_eq!(
            image::open(&thumbnail).unwrap().into_rgba8().dimensions(),
            (1, 1)
        );

        let depth = Array2::from_shape_vec((2, 2), vec![0.0f32, 0.25, 0.5, 1.0]).unwrap();
        let depth_path = directory.path().join("depth.tiff");
        let depth_digest = save_depth_tiff_with_digest(&depth, &depth_path).unwrap();
        let depth_bytes = std::fs::read(&depth_path).unwrap();
        assert_eq!(depth_digest.size_bytes, depth_bytes.len() as u64);
        assert_eq!(
            depth_digest.sha256,
            hex::encode(Sha256::digest(&depth_bytes))
        );
        let mut decoder = Decoder::new(std::fs::File::open(&depth_path).unwrap()).unwrap();
        let DecodingResult::U16(values) = decoder.read_image().unwrap() else {
            panic!("Expected 16-bit depth samples");
        };
        assert_eq!(values, vec![0, 16_384, 32_768, 65_535]);

        let metric = Array2::from_shape_vec((2, 2), vec![0.25f32, 0.5, 1.0, 2.0]).unwrap();
        let metric_path = directory.path().join("depth_m.pfm");
        let metric_digest = save_metric_depth_pfm_with_digest(&metric, &metric_path).unwrap();
        let metric_bytes = std::fs::read(&metric_path).unwrap();
        let header = b"Pf\n2 2\n-1.0\n";
        assert!(metric_bytes.starts_with(header));
        assert_eq!(metric_digest.size_bytes, metric_bytes.len() as u64);
        let first = f32::from_le_bytes(
            metric_bytes[header.len()..header.len() + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(first, 1.0, "PFM rows must be stored bottom-to-top");
        assert!(directory.path().read_dir().unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".part")
        }));
    }

    #[test]
    fn exact_diagnostic_png_maps_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("source.png");
        let flags_path = directory.path().join("flags.png");
        let source =
            Array2::from_shape_vec((2, 3), vec![0u16, 1, 257, 4096, 65_534, u16::MAX]).unwrap();
        let flags = Array2::from_shape_vec((2, 3), vec![0u8, 1, 2, 64, 128, 255]).unwrap();
        save_u16_map_png_with_digest(&source, &source_path).unwrap();
        save_u8_map_png_with_digest(&flags, &flags_path).unwrap();

        let decoded_source = image::open(&source_path).unwrap().into_luma16();
        let decoded_flags = image::open(&flags_path).unwrap().into_luma8();
        assert_eq!(decoded_source.into_raw(), source.into_raw_vec());
        assert_eq!(decoded_flags.into_raw(), flags.into_raw_vec());
    }
}
