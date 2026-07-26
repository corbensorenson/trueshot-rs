use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Instant;
use trueshot_core::nef::parser::Z9NefParser;
use trueshot_core::nef::raw_data::Roi;
use trueshot_core::object_detection::detect_object_bbox_with_parser;

fn checksum(pixels: &[u16]) -> u64 {
    pixels.iter().fold(0u64, |hash, &pixel| {
        hash.wrapping_mul(1_000_003).wrapping_add(pixel as u64)
    })
}

fn main() -> Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: nef_roi_benchmark <image.nef> [--verify-full]")?;
    let verify_full = std::env::args().any(|argument| argument == "--verify-full");

    let mut parser = Z9NefParser::new(&path);
    let parse_start = Instant::now();
    parser.parse()?;
    let parse_ms = parse_start.elapsed().as_secs_f64() * 1000.0;

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

    if verify_full {
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
    }

    Ok(())
}
