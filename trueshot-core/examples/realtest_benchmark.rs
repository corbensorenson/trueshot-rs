use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;
use image::{DynamicImage, GrayImage, RgbImage};

use trueshot_core::object_detection::detect_object_bbox;
use trueshot_core::smart_loader::SmartLoader;
use trueshot_core::timing::{HierarchicalTimer, TimingStats};
use trueshot_core::types::ProcessingOptions;
use trueshot_core::ai::segmentation::SegmentationEngine;
use trueshot_core::nef::parser::Z9NefParser;
use trueshot_core::metrics::image_metrics::{psnr_rgb_u8, ssim_luma_u8};
use trueshot_core::metrics::geometry_metrics::{
    compute_geometry_metrics,
    load_point_cloud_with_normals,
    GeometryMetrics,
    GeometryMetricsOptions,
};

#[derive(Serialize)]
struct SequenceMetrics {
    id: String,
    frame_count: usize,
    load_total_ms: f64,
    timings: HashMap<String, TimingStats>,
    psnr_db: Option<f64>,
    ssim: Option<f64>,
    chamfer: Option<f64>,
    hausdorff: Option<f64>,
    fscore: Option<f64>,
    precision: Option<f64>,
    recall: Option<f64>,
    normal_consistency: Option<f64>,
    seg_iou: Option<f64>,
    seg_dice: Option<f64>,
    gt_matches: usize,
    mesh_matches: usize,
    seg_matches: usize,
}

#[derive(Serialize)]
struct DatasetMetrics {
    dataset_path: String,
    timestamp_utc: String,
    nef_count: usize,
    sequence_count: usize,
    total_frames_loaded: usize,
    bbox_coverage_pct: Option<f64>,
    full_width: Option<u32>,
    full_height: Option<u32>,
    psnr_db: Option<f64>,
    ssim: Option<f64>,
    chamfer: Option<f64>,
    hausdorff: Option<f64>,
    fscore: Option<f64>,
    precision: Option<f64>,
    recall: Option<f64>,
    normal_consistency: Option<f64>,
    seg_iou: Option<f64>,
    seg_dice: Option<f64>,
    gt_matches: usize,
    mesh_matches: usize,
    seg_matches: usize,
    sequences: Vec<SequenceMetrics>,
}

fn parse_args() -> Result<(PathBuf, PathBuf, Option<PathBuf>, Option<PathBuf>, Option<PathBuf>, Option<PathBuf>, Option<PathBuf>)> {
    let mut args = env::args().skip(1);
    let input = args.next().context("usage: realtest_benchmark <nef_dir> [--out <path>] [--gt <dir>] [--gt-mesh <dir>] [--pred-mesh <dir>] [--gt-mask <dir>] [--seg-model <path>]")?;
    let mut out_path: Option<PathBuf> = None;
    let mut gt_dir: Option<PathBuf> = None;
    let mut gt_mesh_dir: Option<PathBuf> = None;
    let mut pred_mesh_dir: Option<PathBuf> = None;
    let mut gt_mask_dir: Option<PathBuf> = None;
    let mut seg_model_path: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        if arg == "--out" {
            let value = args.next().context("--out requires a path")?;
            out_path = Some(PathBuf::from(value));
        } else if arg == "--gt" {
            let value = args.next().context("--gt requires a path")?;
            gt_dir = Some(PathBuf::from(value));
        } else if arg == "--gt-mesh" {
            let value = args.next().context("--gt-mesh requires a path")?;
            gt_mesh_dir = Some(PathBuf::from(value));
        } else if arg == "--pred-mesh" {
            let value = args.next().context("--pred-mesh requires a path")?;
            pred_mesh_dir = Some(PathBuf::from(value));
        } else if arg == "--gt-mask" {
            let value = args.next().context("--gt-mask requires a path")?;
            gt_mask_dir = Some(PathBuf::from(value));
        } else if arg == "--seg-model" {
            let value = args.next().context("--seg-model requires a path")?;
            seg_model_path = Some(PathBuf::from(value));
        }
    }

    let default_out = PathBuf::from(format!(
        "benchmarks/results/realtest_{}.json",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    ));

    Ok((PathBuf::from(input), out_path.unwrap_or(default_out), gt_dir, gt_mesh_dir, pred_mesh_dir, gt_mask_dir, seg_model_path))
}

fn compute_bbox_coverage(path: &Path) -> Result<(f64, u32, u32)> {
    let bbox = detect_object_bbox(path)?;

    let mut parser = Z9NefParser::new(path);
    parser.parse()?;
    let metadata = parser.get_metadata()?;

    let full_width = metadata.width;
    let full_height = metadata.height;
    let total_pixels = (full_width as f64) * (full_height as f64);
    let bbox_pixels = bbox.width * bbox.height;
    let coverage_pct = if total_pixels > 0.0 {
        (bbox_pixels / total_pixels) * 100.0
    } else {
        0.0
    };

    Ok((coverage_pct, full_width, full_height))
}

fn load_preview_rgb(path: &Path) -> Result<RgbImage> {
    let mut parser = Z9NefParser::new(path);
    parser.parse()?;
    let jpeg = parser.extract_preview_jpeg()?;
    let img = image::load_from_memory(&jpeg)?;
    Ok(img.to_rgb8())
}

fn load_gt_image(path: &Path) -> Result<RgbImage> {
    let img = image::open(path)?;
    Ok(img.to_rgb8())
}

fn find_gt_image(gt_dir: &Path, nef_path: &Path) -> Option<PathBuf> {
    let stem = nef_path.file_stem()?.to_string_lossy().to_string();
    for ext in ["jpg", "jpeg", "png"] {
        let candidate = gt_dir.join(format!("{}.{}", stem, ext));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn load_gt_mask(path: &Path) -> Result<GrayImage> {
    let img = image::open(path)?;
    Ok(img.to_luma8())
}

fn find_gt_mask(gt_dir: &Path, nef_path: &Path) -> Option<PathBuf> {
    let stem = nef_path.file_stem()?.to_string_lossy().to_string();
    for ext in ["png", "jpg", "jpeg", "tif", "tiff"] {
        let candidate = gt_dir.join(format!("{}.{}", stem, ext));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn compute_preview_metrics(seq: &trueshot_core::types::Sequence, gt_dir: &Path) -> Result<(Option<f64>, Option<f64>, usize)> {
    if seq.paths.is_empty() {
        return Ok((None, None, 0));
    }
    let ref_path = &seq.paths[seq.ref_index()];
    let gt_path = match find_gt_image(gt_dir, ref_path) {
        Some(path) => path,
        None => return Ok((None, None, 0)),
    };
    let preview = load_preview_rgb(ref_path)?;
    let mut gt = load_gt_image(&gt_path)?;

    if preview.width() != gt.width() || preview.height() != gt.height() {
        let resized = image::imageops::resize(&gt, preview.width(), preview.height(), image::imageops::FilterType::Lanczos3);
        gt = resized;
    }

    let psnr = psnr_rgb_u8(&preview, &gt);
    let ssim = ssim_luma_u8(&preview, &gt);
    Ok((psnr, ssim, 1))
}

fn compute_segmentation_metrics(
    seq: &trueshot_core::types::Sequence,
    gt_mask_dir: &Path,
    engine: &mut SegmentationEngine,
) -> Result<(Option<f64>, Option<f64>, usize)> {
    if seq.paths.is_empty() {
        return Ok((None, None, 0));
    }
    let ref_path = &seq.paths[seq.ref_index()];
    let gt_path = match find_gt_mask(gt_mask_dir, ref_path) {
        Some(path) => path,
        None => return Ok((None, None, 0)),
    };

    let preview = load_preview_rgb(ref_path)?;
    let pred_mask = engine.segment(&DynamicImage::ImageRgb8(preview))?;
    let pred_mask = pred_mask.to_luma8();
    let mut gt_mask = load_gt_mask(&gt_path)?;

    if pred_mask.dimensions() != gt_mask.dimensions() {
        gt_mask = image::imageops::resize(&gt_mask, pred_mask.width(), pred_mask.height(), image::imageops::FilterType::Nearest);
    }

    let (iou, dice) = match compute_iou_dice(&pred_mask, &gt_mask) {
        Some(values) => values,
        None => return Ok((None, None, 0)),
    };

    Ok((Some(iou), Some(dice), 1))
}

fn compute_iou_dice(pred: &GrayImage, gt: &GrayImage) -> Option<(f64, f64)> {
    if pred.dimensions() != gt.dimensions() {
        return None;
    }
    let mut intersection = 0usize;
    let mut union = 0usize;
    let mut pred_count = 0usize;
    let mut gt_count = 0usize;

    for (p, g) in pred.pixels().zip(gt.pixels()) {
        let pv = p[0] > 0;
        let gv = g[0] > 0;
        if pv {
            pred_count += 1;
        }
        if gv {
            gt_count += 1;
        }
        if pv && gv {
            intersection += 1;
        }
        if pv || gv {
            union += 1;
        }
    }

    if union == 0 || (pred_count + gt_count) == 0 {
        return None;
    }

    let iou = intersection as f64 / union as f64;
    let dice = (2.0 * intersection as f64) / (pred_count + gt_count) as f64;
    Some((iou, dice))
}

fn find_mesh_file(mesh_dir: &Path, stem: &str) -> Option<PathBuf> {
    for ext in ["ply", "obj"] {
        let candidate = mesh_dir.join(format!("{}.{}", stem, ext));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn compute_geometry_for_sequence(
    seq: &trueshot_core::types::Sequence,
    gt_mesh_dir: &Path,
    pred_mesh_dir: &Path,
    options: &GeometryMetricsOptions,
) -> Result<(GeometryMetrics, usize)> {
    if seq.paths.is_empty() {
        return Ok((
            GeometryMetrics {
                chamfer: None,
                hausdorff: None,
                fscore: None,
                precision: None,
                recall: None,
                normal_consistency: None,
            },
            0,
        ));
    }

    let ref_path = &seq.paths[seq.ref_index()];
    let stem = ref_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let gt_path = match find_mesh_file(gt_mesh_dir, stem) {
        Some(path) => path,
        None => return Ok((None, 0)),
    };
    let pred_path = match find_mesh_file(pred_mesh_dir, stem) {
        Some(path) => path,
        None => return Ok((
            GeometryMetrics {
                chamfer: None,
                hausdorff: None,
                fscore: None,
                precision: None,
                recall: None,
                normal_consistency: None,
            },
            0,
        )),
    };

    let gt_cloud = load_point_cloud_with_normals(&gt_path)?;
    let pred_cloud = load_point_cloud_with_normals(&pred_path)?;
    let metrics = compute_geometry_metrics(&pred_cloud, &gt_cloud, options);
    Ok((metrics, if metrics.chamfer.is_some() { 1 } else { 0 }))
}

fn geometry_options_from_env() -> GeometryMetricsOptions {
    let mut options = GeometryMetricsOptions::default();
    if let Ok(value) = env::var("TRUESHOT_FSCORE_THRESHOLD") {
        if let Ok(parsed) = value.parse::<f64>() {
            options.fscore_threshold = parsed;
        }
    }
    if let Ok(value) = env::var("TRUESHOT_GEOM_SAMPLE_LIMIT") {
        if let Ok(parsed) = value.parse::<usize>() {
            if parsed > 0 {
                options.sample_limit = parsed;
            }
        }
    }
    if let Ok(value) = env::var("TRUESHOT_GEOM_NORMAL_K") {
        if let Ok(parsed) = value.parse::<usize>() {
            if parsed >= 3 {
                options.normal_k = parsed;
            }
        }
    }
    options
}

fn main() -> Result<()> {
    let (input_dir, out_path, gt_dir, gt_mesh_dir, pred_mesh_dir, gt_mask_dir, seg_model_path) = parse_args()?;
    let input_dir = input_dir.canonicalize().context("failed to resolve input dir")?;
    let gt_dir = match gt_dir {
        Some(path) => Some(path.canonicalize().context("failed to resolve gt dir")?),
        None => None,
    };
    let gt_mesh_dir = match gt_mesh_dir {
        Some(path) => Some(path.canonicalize().context("failed to resolve gt mesh dir")?),
        None => None,
    };
    let pred_mesh_dir = match pred_mesh_dir {
        Some(path) => Some(path.canonicalize().context("failed to resolve pred mesh dir")?),
        None => None,
    };
    let gt_mask_dir = match gt_mask_dir {
        Some(path) => Some(path.canonicalize().context("failed to resolve gt mask dir")?),
        None => None,
    };
    let seg_model_path = match seg_model_path {
        Some(path) => Some(path.canonicalize().context("failed to resolve segmentation model path")?),
        None => None,
    };

    let mut seg_engine = match seg_model_path.as_ref() {
        Some(path) => Some(SegmentationEngine::new(path.to_string_lossy().as_ref())?),
        None => None,
    };

    let mut options = ProcessingOptions::default();
    options.verbose_timing = false;
    let geometry_options = geometry_options_from_env();

    let loader = SmartLoader::new(options);
    let sequences = loader.scan_and_group(&input_dir)?;

    let nef_count: usize = sequences.iter().map(|s| s.len()).sum();
    let mut total_frames_loaded = 0usize;

    let mut metrics = Vec::new();
    let mut psnr_values = Vec::new();
    let mut ssim_values = Vec::new();
    let mut gt_matches = 0usize;
    let mut chamfer_values = Vec::new();
    let mut hausdorff_values = Vec::new();
    let mut fscore_values = Vec::new();
    let mut precision_values = Vec::new();
    let mut recall_values = Vec::new();
    let mut normal_values = Vec::new();
    let mut mesh_matches = 0usize;
    let mut seg_iou_values = Vec::new();
    let mut seg_dice_values = Vec::new();
    let mut seg_matches = 0usize;
    for seq in &sequences {
        let mut timer = HierarchicalTimer::new(&seq.meta.bone_id);
        let frames = loader.load_sequence(seq, &mut timer)?;
        total_frames_loaded += frames.len();

        let report = timer.aggregate();
        let (psnr_db, ssim, matches) = if let Some(gt_dir) = gt_dir.as_ref() {
            compute_preview_metrics(seq, gt_dir)?
        } else {
            (None, None, 0)
        };
        let (geometry_metrics, mesh_match) = if let (Some(gt_mesh), Some(pred_mesh)) = (gt_mesh_dir.as_ref(), pred_mesh_dir.as_ref()) {
            compute_geometry_for_sequence(seq, gt_mesh, pred_mesh, &geometry_options)?
        } else {
            (
                GeometryMetrics {
                    chamfer: None,
                    hausdorff: None,
                    fscore: None,
                    precision: None,
                    recall: None,
                    normal_consistency: None,
                },
                0,
            )
        };
        let chamfer = geometry_metrics.chamfer;
        let hausdorff = geometry_metrics.hausdorff;
        let fscore = geometry_metrics.fscore;
        let precision = geometry_metrics.precision;
        let recall = geometry_metrics.recall;
        let normal_consistency = geometry_metrics.normal_consistency;
        let (seg_iou, seg_dice, seg_match) = if let (Some(mask_dir), Some(engine)) = (gt_mask_dir.as_ref(), seg_engine.as_mut()) {
            compute_segmentation_metrics(seq, mask_dir, engine)?
        } else {
            (None, None, 0)
        };
        if let Some(value) = psnr_db {
            psnr_values.push(value);
        }
        if let Some(value) = ssim {
            ssim_values.push(value);
        }
        if let Some(value) = chamfer {
            chamfer_values.push(value);
        }
        if let Some(value) = hausdorff {
            hausdorff_values.push(value);
        }
        if let Some(value) = fscore {
            fscore_values.push(value);
        }
        if let Some(value) = precision {
            precision_values.push(value);
        }
        if let Some(value) = recall {
            recall_values.push(value);
        }
        if let Some(value) = normal_consistency {
            normal_values.push(value);
        }
        if let Some(value) = seg_iou {
            seg_iou_values.push(value);
        }
        if let Some(value) = seg_dice {
            seg_dice_values.push(value);
        }
        gt_matches += matches;
        mesh_matches += mesh_match;
        seg_matches += seg_match;
        metrics.push(SequenceMetrics {
            id: seq.meta.bone_id.clone(),
            frame_count: frames.len(),
            load_total_ms: report.total_ms,
            timings: report.timings,
            psnr_db,
            ssim,
            chamfer,
            hausdorff,
            fscore,
            precision,
            recall,
            normal_consistency,
            seg_iou,
            seg_dice,
            gt_matches: matches,
            mesh_matches: mesh_match,
            seg_matches: seg_match,
        });
    }

    let (coverage_pct, full_width, full_height) = if let Some(first_seq) = sequences.first() {
        let ref_path = &first_seq.paths[first_seq.ref_index()];
        match compute_bbox_coverage(ref_path) {
            Ok((pct, w, h)) => (Some(pct), Some(w), Some(h)),
            Err(err) => {
                eprintln!("bbox coverage failed: {err}");
                (None, None, None)
            }
        }
    } else {
        (None, None, None)
    };

    let psnr_db = if psnr_values.is_empty() {
        None
    } else {
        Some(psnr_values.iter().sum::<f64>() / psnr_values.len() as f64)
    };
    let ssim = if ssim_values.is_empty() {
        None
    } else {
        Some(ssim_values.iter().sum::<f64>() / ssim_values.len() as f64)
    };
    let chamfer = if chamfer_values.is_empty() {
        None
    } else {
        Some(chamfer_values.iter().sum::<f64>() / chamfer_values.len() as f64)
    };
    let hausdorff = if hausdorff_values.is_empty() {
        None
    } else {
        Some(hausdorff_values.iter().sum::<f64>() / hausdorff_values.len() as f64)
    };
    let fscore = if fscore_values.is_empty() {
        None
    } else {
        Some(fscore_values.iter().sum::<f64>() / fscore_values.len() as f64)
    };
    let precision = if precision_values.is_empty() {
        None
    } else {
        Some(precision_values.iter().sum::<f64>() / precision_values.len() as f64)
    };
    let recall = if recall_values.is_empty() {
        None
    } else {
        Some(recall_values.iter().sum::<f64>() / recall_values.len() as f64)
    };
    let normal_consistency = if normal_values.is_empty() {
        None
    } else {
        Some(normal_values.iter().sum::<f64>() / normal_values.len() as f64)
    };
    let seg_iou = if seg_iou_values.is_empty() {
        None
    } else {
        Some(seg_iou_values.iter().sum::<f64>() / seg_iou_values.len() as f64)
    };
    let seg_dice = if seg_dice_values.is_empty() {
        None
    } else {
        Some(seg_dice_values.iter().sum::<f64>() / seg_dice_values.len() as f64)
    };

    let output = DatasetMetrics {
        dataset_path: input_dir.to_string_lossy().to_string(),
        timestamp_utc: Utc::now().to_rfc3339(),
        nef_count,
        sequence_count: sequences.len(),
        total_frames_loaded,
        bbox_coverage_pct: coverage_pct,
        full_width,
        full_height,
        psnr_db,
        ssim,
        chamfer,
        hausdorff,
        fscore,
        precision,
        recall,
        normal_consistency,
        seg_iou,
        seg_dice,
        gt_matches,
        mesh_matches,
        seg_matches,
        sequences: metrics,
    };

    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).ok();
    }

    let payload = serde_json::to_string_pretty(&output)?;
    fs::write(&out_path, payload)?;
    println!("Wrote benchmark results to {}", out_path.display());

    Ok(())
}
