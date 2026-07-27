use anyhow::{Context, Result};
use memmap2::MmapOptions;
use std::fs::File;
use std::path::PathBuf;
use std::time::Instant;
use trueshot_core::nef::parser::Z9NefParser;
use trueshot_core::nef::raw_data::{RawBuffer, Roi};
use trueshot_core::object_detection::detect_object_bbox_with_parser;

fn checksum(pixels: &[u16]) -> u64 {
    pixels.iter().fold(0u64, |hash, &pixel| {
        hash.wrapping_mul(1_000_003).wrapping_add(pixel as u64)
    })
}

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let path = arguments.next().map(PathBuf::from).context(
        "usage: nef_roi_benchmark <image.nef> [--verify-full] [--reference-pgm <raw.pgm>]",
    )?;
    let mut verify_full = false;
    let mut reference_pgm = None;
    while let Some(argument) = arguments.next() {
        if argument == "--verify-full" {
            verify_full = true;
        } else if argument == "--reference-pgm" {
            reference_pgm = Some(
                arguments
                    .next()
                    .map(PathBuf::from)
                    .context("--reference-pgm requires a path")?,
            );
        } else {
            anyhow::bail!("Unknown argument: {}", argument.to_string_lossy());
        }
    }

    let mut parser = Z9NefParser::new(&path);
    let parse_start = Instant::now();
    parser.parse()?;
    let parse_ms = parse_start.elapsed().as_secs_f64() * 1000.0;
    let metadata = parser.get_metadata()?;
    println!(
        "camera=\"{} {}\" raw={}x{} bits={} compression={} sensor_levels={:?} \
         sensor_geometry={:?} lens={:?} focal_mm={:?} aperture={:?} focus_m={:?}",
        metadata.camera_make,
        metadata.camera_model,
        metadata.width,
        metadata.height,
        metadata.bits_per_sample,
        metadata.compression,
        metadata.sensor_levels,
        metadata.sensor_geometry,
        metadata.lens_model,
        metadata.focal_length,
        metadata.aperture,
        metadata.focus_distance
    );

    let detection_start = Instant::now();
    let rect = detect_object_bbox_with_parser(&mut parser)?;
    let detection_ms = detection_start.elapsed().as_secs_f64() * 1000.0;
    let (x0, y0, x1, y1) = rect.to_bounds();
    let roi = Roi::new(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);

    let mut crop = vec![0u16; roi.area() as usize];
    let roi_start = Instant::now();
    parser.load_roi_into(&roi, &mut crop)?;
    let roi_ms = roi_start.elapsed().as_secs_f64() * 1000.0;
    println!(
        "parse_ms={parse_ms:.2} detection_ms={detection_ms:.2} roi_into_ms={roi_ms:.2} \
         roi={}x{} bytes={} checksum={}",
        roi.width,
        roi.height,
        crop.len() * std::mem::size_of::<u16>(),
        checksum(&crop)
    );

    if verify_full || reference_pgm.is_some() {
        let full_start = Instant::now();
        let full = parser.load_full()?;
        let full_ms = full_start.elapsed().as_secs_f64() * 1000.0;
        let exact = (0..roi.height).all(|y| {
            (0..roi.width).all(|x| {
                crop[(y * roi.width + x) as usize] == full.get_pixel(roi.x + x, roi.y + y).unwrap()
            })
        });
        println!(
            "full_ms={full_ms:.2} full_checksum={} roi_matches_full={exact}",
            checksum(&full.data)
        );
        if !exact {
            anyhow::bail!("selective NEF pixels differ from full decode");
        }
        if let Some(reference_path) = reference_pgm {
            compare_pgm_reference(&full, &reference_path)?;
        }
    }

    Ok(())
}

fn compare_pgm_reference(raw: &RawBuffer, path: &std::path::Path) -> Result<()> {
    let file = File::open(path)
        .with_context(|| format!("Unable to open raw PGM reference {}", path.display()))?;
    // SAFETY: The map is read-only and the file remains open throughout comparison.
    let mapped = unsafe { MmapOptions::new().map(&file)? };
    let mut cursor = 0usize;
    let magic = next_pgm_token(&mapped, &mut cursor)?;
    let width = next_pgm_token(&mapped, &mut cursor)?.parse::<u32>()?;
    let height = next_pgm_token(&mapped, &mut cursor)?.parse::<u32>()?;
    let maximum = next_pgm_token(&mapped, &mut cursor)?.parse::<u32>()?;
    if magic != "P5" || maximum != 65_535 {
        anyhow::bail!("Reference must be binary 16-bit PGM (P5, max=65535)");
    }
    if width != raw.width || height != raw.height {
        anyhow::bail!(
            "Reference dimensions {}x{} differ from TrueShot {}x{}",
            width,
            height,
            raw.width,
            raw.height
        );
    }
    if mapped.get(cursor) == Some(&b'\r') && mapped.get(cursor + 1) == Some(&b'\n') {
        cursor += 2;
    } else if mapped.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    } else {
        anyhow::bail!("Reference PGM header has no payload separator");
    }
    let expected_bytes = raw
        .data
        .len()
        .checked_mul(2)
        .context("Reference PGM dimensions overflow")?;
    let payload = mapped
        .get(cursor..)
        .context("Reference PGM has no pixel payload")?;
    if payload.len() != expected_bytes {
        anyhow::bail!(
            "Reference PGM has {} payload bytes; expected {}",
            payload.len(),
            expected_bytes
        );
    }

    let mut mismatches = 0usize;
    let mut maximum_error = 0u16;
    let mut first_mismatch = None;
    for (index, (bytes, &actual)) in payload.chunks_exact(2).zip(&raw.data).enumerate() {
        let expected = u16::from_be_bytes([bytes[0], bytes[1]]);
        let error = expected.abs_diff(actual);
        if error != 0 {
            mismatches += 1;
            maximum_error = maximum_error.max(error);
            first_mismatch.get_or_insert((index, expected, actual));
        }
    }
    println!(
        "reference_pgm={} mismatches={} max_abs_error={} first_mismatch={:?}",
        path.display(),
        mismatches,
        maximum_error,
        first_mismatch
    );
    if mismatches != 0 {
        anyhow::bail!("TrueShot full decode differs from independent raw PGM reference");
    }
    Ok(())
}

fn next_pgm_token<'a>(bytes: &'a [u8], cursor: &mut usize) -> Result<&'a str> {
    loop {
        while *cursor < bytes.len() && bytes[*cursor].is_ascii_whitespace() {
            *cursor += 1;
        }
        if bytes.get(*cursor) != Some(&b'#') {
            break;
        }
        while *cursor < bytes.len() && bytes[*cursor] != b'\n' {
            *cursor += 1;
        }
    }
    let start = *cursor;
    while *cursor < bytes.len() && !bytes[*cursor].is_ascii_whitespace() && bytes[*cursor] != b'#' {
        *cursor += 1;
    }
    if start == *cursor {
        anyhow::bail!("Reference PGM header is truncated");
    }
    std::str::from_utf8(&bytes[start..*cursor]).context("Reference PGM header is not ASCII")
}
