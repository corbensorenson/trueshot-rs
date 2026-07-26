use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::{DynamicImage, GrayImage};

use trueshot_core::ai::segmentation::SegmentationEngine;
use trueshot_core::nef::parser::Z9NefParser;

fn load_preview_rgb(path: &Path) -> Result<DynamicImage> {
    let mut parser = Z9NefParser::new(path);
    parser.parse()?;
    let jpeg = parser.extract_preview_jpeg()?;
    let img = image::load_from_memory(&jpeg)?;
    Ok(DynamicImage::ImageRgb8(img.to_rgb8()))
}

fn write_mask(mask: &GrayImage, out_path: &Path) -> Result<()> {
    mask.save(out_path)?;
    Ok(())
}

fn parse_args() -> Result<(PathBuf, PathBuf, Option<PathBuf>, Option<usize>, bool)> {
    let mut args = env::args().skip(1);
    let nef_dir = args
        .next()
        .context("usage: bootstrap_gt_masks <nef_dir> <out_dir> [--seg-model <path>] [--limit <n>] [--overwrite]")?;
    let out_dir = args
        .next()
        .context("usage: bootstrap_gt_masks <nef_dir> <out_dir> [--seg-model <path>] [--limit <n>] [--overwrite]")?;

    let mut seg_model = None;
    let mut limit = None;
    let mut overwrite = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seg-model" => {
                let value = args.next().context("--seg-model requires a path")?;
                seg_model = Some(PathBuf::from(value));
            }
            "--limit" => {
                let value = args.next().context("--limit requires a number")?;
                limit = Some(value.parse::<usize>().context("invalid --limit value")?);
            }
            "--overwrite" => overwrite = true,
            other => anyhow::bail!("Unknown argument: {other}"),
        }
    }

    Ok((PathBuf::from(nef_dir), PathBuf::from(out_dir), seg_model, limit, overwrite))
}

fn is_nef(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("nef"))
        .unwrap_or(false)
}

fn main() -> Result<()> {
    let (nef_dir, out_dir, seg_model, limit, overwrite) = parse_args()?;
    if !nef_dir.is_dir() {
        anyhow::bail!("NEF directory not found: {}", nef_dir.display());
    }
    fs::create_dir_all(&out_dir)?;

    let mut entries: Vec<PathBuf> = fs::read_dir(&nef_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| is_nef(path))
        .collect();
    entries.sort();

    let model_path = seg_model
        .as_ref()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut engine = SegmentationEngine::new(&model_path)?;

    let mut processed = 0usize;
    for path in entries {
        if let Some(max) = limit {
            if processed >= max {
                break;
            }
        }

        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(stem) => stem,
            None => {
                eprintln!("Skipping unreadable filename: {}", path.display());
                continue;
            }
        };
        let out_path = out_dir.join(format!("{stem}.png"));
        if out_path.exists() && !overwrite {
            continue;
        }

        let preview = match load_preview_rgb(&path) {
            Ok(img) => img,
            Err(err) => {
                eprintln!("Failed to load preview for {}: {err}", path.display());
                continue;
            }
        };

        let mask = match engine.segment(&preview) {
            Ok(mask) => mask.to_luma8(),
            Err(err) => {
                eprintln!("Segmentation failed for {}: {err}", path.display());
                continue;
            }
        };

        if let Err(err) = write_mask(&mask, &out_path) {
            eprintln!("Failed to write mask {}: {err}", out_path.display());
            continue;
        }

        processed += 1;
        println!("Wrote mask: {}", out_path.display());
    }

    println!("Generated {processed} masks in {}", out_dir.display());
    Ok(())
}
