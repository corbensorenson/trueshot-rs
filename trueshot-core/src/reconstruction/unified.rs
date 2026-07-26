use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use crate::reconstruction::pipeline::{ReconstructionPipeline, ReconstructionConfig, ReconstructionType};
use crate::reconstruction::livescan::{LiveScanData, PosePriors, PosePriorFrame, TransformConvention};
use crate::reconstruction::multicam_sfm::CameraPose;
use crate::intrinsics::estimate_intrinsics;
use crate::scanning::rig::{RigSolver, ScannerRig};
use chrono::NaiveDateTime;
use exif::{In, Reader, Tag, Value};
use nalgebra as na;
use serde::Serialize;
use walkdir::WalkDir;

/// Unified interface for all reconstruction tasks
pub struct UnifiedReconstruction {
    workspace_path: PathBuf,
}

impl UnifiedReconstruction {
    pub fn new(workspace_path: PathBuf) -> Self {
        Self { workspace_path }
    }

    /// Run photogrammetry pipeline (SFM -> MVS -> Mesh)
    pub fn process_photogrammetry(
        &self,
        quality: String,
        livescan_path: Option<PathBuf>,
        priors: Option<PosePriors>,
    ) -> Result<()> {
        let priors = self.resolve_priors(livescan_path.clone(), priors)?;
        let config = ReconstructionConfig {
            reconstruction_type: if quality == "fast" { 
                ReconstructionType::PhotogrammetryFast 
            } else { 
                ReconstructionType::PhotogrammetryHighQuality 
            },
            workspace_path: self.workspace_path.clone(),
        };

        self.run_pipeline(config, priors)?;

        Ok(())
    }

    /// Run Gaussian Splatting pipeline
    pub fn process_gaussian_splatting(
        &self,
        livescan_path: Option<PathBuf>,
        priors: Option<PosePriors>,
    ) -> Result<()> {
        let priors = self.resolve_priors(livescan_path.clone(), priors)?;
        let config = ReconstructionConfig {
            reconstruction_type: ReconstructionType::GaussianSplatting,
            workspace_path: self.workspace_path.clone(),
        };

        self.run_pipeline(config, priors)?;

        Ok(())
    }

    /// Match DSLR images to LiveScan data via timestamp synchronization
    /// Uses Dynamic Rig Estimation:
    /// 1. Detects sequences (turntable rotations).
    /// 2. Solves Rig Configuration at the start of each sequence.
    /// 3. Propagates poses for the rest of the sequence using turntable angles.
    pub fn synchronize_dslr_images(&self, dslr_dir: &PathBuf, livescan_path: &PathBuf) -> Result<PosePriors> {
        let livescan = LiveScanData::load_from_file(livescan_path)?;
        
        // 1. Gather DSLR images (EXIF timestamps preferred)
        let dslr_images = collect_dslr_images(dslr_dir)?;
        if dslr_images.is_empty() {
            anyhow::bail!("No DSLR images found under {}", dslr_dir.display());
        }

        // 2. Group LiveScan data into Sequences (Angle resets or time gaps)
        // A sequence is a continuous set of captures where cameras are static.
        // We assume a sequence starts when angle ~= 0 or angle < prev_angle
        
        let sequences = group_livescan_sequences(&livescan.frames);
        tracing::info!("Found {} capture sequences.", sequences.len());

        // 3. Estimate global time offset between DSLR and LiveScan clocks
        let livescan_times: Vec<f64> = livescan.frames.iter().map(|f| f.timestamp).collect();
        let time_offset = estimate_time_offset(&dslr_images, &livescan_times);

        // 4. Match DSLR images to nearest LiveScan frame timestamps
        let match_window_sec = 1.2;
        let (frame_matches, unmatched, mean_error) =
            match_dslr_to_livescan(&dslr_images, &livescan_times, time_offset, match_window_sec);

        // 5. Solve rigs per sequence and propagate poses per frame
        let rig_solver = RigSolver::new(self.workspace_path.clone());
        let mut priors = PosePriors { frames: Vec::new(), imu_samples: Vec::new() };
        let mut used_paths: HashSet<PathBuf> = HashSet::new();
        let mut rig_solved = 0usize;
        let mut rig_fallback = 0usize;

        for (seq_idx, frame_indices) in sequences.iter().enumerate() {
            if frame_indices.is_empty() {
                continue;
            }

            let (_rig_frame_idx, rig_images) =
                select_rig_frame(frame_indices, &frame_matches);

            let rig = if rig_images.len() >= 2 {
                match rig_solver.solve_for_sequence(&rig_images) {
                    Ok(rig) => {
                        rig_solved += 1;
                        Some(rig)
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Sequence {}: rig solve failed ({}). Falling back to single-camera priors.",
                            seq_idx,
                            e
                        );
                        rig_fallback += 1;
                        None
                    }
                }
            } else {
                rig_fallback += 1;
                None
            };

            for &frame_idx in frame_indices {
                let frame = &livescan.frames[frame_idx];
                let angle = frame.turntable_angle.unwrap_or(0.0);
                let Some(images) = frame_matches.get(&frame_idx) else {
                    continue;
                };

                if let Some(rig) = &rig {
                    let pose_map = rig_pose_map_by_key(rig, angle);
                    for img in images {
                        if used_paths.contains(&img.path) {
                            continue;
                        }
                        let key = img.camera_key.clone();
                        let pose = pose_map
                            .get(&key)
                            .cloned()
                            .unwrap_or_else(|| pose_map.values().next().cloned().unwrap_or_else(CameraPose::identity));
                        let intrinsics = estimate_intrinsics(&img.path)?;
                        priors.frames.push(PosePriorFrame {
                            image_path: img.path.to_string_lossy().to_string(),
                            pose,
                            intrinsics,
                            camera_motion: None,
                            rolling_shutter: None,
                        });
                        used_paths.insert(img.path.clone());
                    }
                } else {
                    let pose = pose_from_livescan_frame(frame, livescan.transform_convention)
                        .unwrap_or_else(CameraPose::identity);
                    for img in images {
                        if used_paths.contains(&img.path) {
                            continue;
                        }
                        let intrinsics = estimate_intrinsics(&img.path)?;
                        priors.frames.push(PosePriorFrame {
                            image_path: img.path.to_string_lossy().to_string(),
                            pose: pose.clone(),
                            intrinsics,
                            camera_motion: None,
                            rolling_shutter: None,
                        });
                        used_paths.insert(img.path.clone());
                    }
                }
            }
        }

        let report = SyncReport {
            dslr_images_total: dslr_images.len(),
            matched_images: priors.frames.len(),
            unmatched_images: unmatched.len(),
            time_offset_seconds: time_offset,
            mean_match_error_seconds: mean_error,
            sequences: sequences.len(),
            rig_solved_sequences: rig_solved,
            fallback_sequences: rig_fallback,
            camera_keys: collect_camera_keys(&dslr_images),
        };
        write_sync_report(&self.workspace_path, &report)?;

        Ok(priors)
    }

    fn resolve_priors(
        &self,
        livescan_path: Option<PathBuf>,
        priors: Option<PosePriors>,
    ) -> Result<Option<PosePriors>> {
        if priors.is_some() {
            return Ok(priors);
        }
        let Some(path) = livescan_path else {
            return Ok(None);
        };
        tracing::info!("Using LiveScan priors from {:?}", path);
        let data = LiveScanData::load_from_file(&path)?;
        Ok(Some(data.to_native_priors(&self.workspace_path)))
    }

    fn run_pipeline(&self, config: ReconstructionConfig, priors: Option<PosePriors>) -> Result<()> {
        let pipeline = ReconstructionPipeline::new(config);
        pipeline.run(priors)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct DslrImageInfo {
    path: PathBuf,
    timestamp: f64,
    camera_key: String,
}

#[derive(Debug, Serialize)]
struct SyncReport {
    dslr_images_total: usize,
    matched_images: usize,
    unmatched_images: usize,
    time_offset_seconds: f64,
    mean_match_error_seconds: f64,
    sequences: usize,
    rig_solved_sequences: usize,
    fallback_sequences: usize,
    camera_keys: Vec<String>,
}

fn collect_dslr_images(root: &Path) -> Result<Vec<DslrImageInfo>> {
    let mut images = Vec::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path().to_path_buf();
        if !is_image_path(&path) {
            continue;
        }
        let timestamp = read_exif_timestamp(&path)
            .or_else(|| file_mtime_seconds(&path))
            .unwrap_or(0.0);
        let camera_key = camera_key_from_path(&path);
        images.push(DslrImageInfo {
            path,
            timestamp,
            camera_key,
        });
    }
    images.sort_by(|a, b| a.timestamp.partial_cmp(&b.timestamp).unwrap_or(std::cmp::Ordering::Equal));
    Ok(images)
}

fn is_image_path(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase()) {
        Some(ext) => matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "tif" | "tiff" | "nef" | "arw"),
        None => false,
    }
}

fn file_mtime_seconds(path: &Path) -> Option<f64> {
    let meta = std::fs::metadata(path).ok()?;
    let time = meta.modified().ok()?;
    let duration = time.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(duration.as_secs_f64())
}

fn read_exif_timestamp(path: &Path) -> Option<f64> {
    let file = std::fs::File::open(path).ok()?;
    let mut bufreader = std::io::BufReader::new(file);
    let exif = Reader::new().read_from_container(&mut bufreader).ok()?;
    let field = exif
        .get_field(Tag::DateTimeOriginal, In::PRIMARY)
        .or_else(|| exif.get_field(Tag::DateTimeDigitized, In::PRIMARY))
        .or_else(|| exif.get_field(Tag::DateTime, In::PRIMARY))?;
    let dt = parse_exif_datetime(&field.value)?;
    let mut timestamp = dt.and_utc().timestamp() as f64;
    if let Some(subsec) = exif.get_field(Tag::SubSecTimeOriginal, In::PRIMARY) {
        if let Some(ms) = parse_exif_subsec(&subsec.value) {
            timestamp += ms;
        }
    }
    Some(timestamp)
}

fn parse_exif_datetime(value: &Value) -> Option<NaiveDateTime> {
    let Value::Ascii(values) = value else { return None };
    let raw = values.first()?;
    let text = String::from_utf8_lossy(raw).trim().to_string();
    NaiveDateTime::parse_from_str(&text, "%Y:%m:%d %H:%M:%S").ok()
}

fn parse_exif_subsec(value: &Value) -> Option<f64> {
    let Value::Ascii(values) = value else { return None };
    let raw = values.first()?;
    let text = String::from_utf8_lossy(raw).trim().to_string();
    if text.is_empty() {
        return None;
    }
    let digits = text.chars().take(6).collect::<String>();
    let frac = format!("0.{}", digits);
    frac.parse::<f64>().ok()
}

fn camera_key_from_path(path: &Path) -> String {
    let file = match path.file_name().and_then(|s| s.to_str()) {
        Some(name) => name,
        None => return "camera".to_string(),
    };
    let stem = Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or(file);
    let key = if let Some((prefix, _)) = stem.split_once("__") {
        prefix
    } else if let Some((prefix, _)) = stem.split_once('_') {
        prefix
    } else if let Some((prefix, _)) = stem.split_once('-') {
        prefix
    } else {
        stem
    };
    key.to_string()
}

fn group_livescan_sequences(frames: &[crate::reconstruction::livescan::LiveScanFrame]) -> Vec<Vec<usize>> {
    let mut sequences: Vec<Vec<usize>> = Vec::new();
    let mut current = Vec::new();
    let mut last_angle = 360.0f32;
    let mut last_time = frames.first().map(|f| f.timestamp).unwrap_or(0.0);
    for (idx, frame) in frames.iter().enumerate() {
        let angle = frame.turntable_angle.unwrap_or(0.0);
        let time_gap = frame.timestamp - last_time;
        if angle < last_angle - 10.0 || time_gap > 3.0 {
            if !current.is_empty() {
                sequences.push(current);
            }
            current = Vec::new();
        }
        current.push(idx);
        last_angle = angle;
        last_time = frame.timestamp;
    }
    if !current.is_empty() {
        sequences.push(current);
    }
    sequences
}

fn estimate_time_offset(dslr_images: &[DslrImageInfo], livescan_times: &[f64]) -> f64 {
    if dslr_images.is_empty() || livescan_times.is_empty() {
        return 0.0;
    }
    let mut offsets = Vec::new();
    let max_gap = 8.0;
    for img in dslr_images {
        if let Some((idx, diff)) = nearest_index(img.timestamp, livescan_times) {
            if diff <= max_gap {
                offsets.push(img.timestamp - livescan_times[idx]);
            }
        }
    }
    if offsets.is_empty() {
        return 0.0;
    }
    offsets.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    offsets[offsets.len() / 2]
}

fn nearest_index(target: f64, times: &[f64]) -> Option<(usize, f64)> {
    if times.is_empty() {
        return None;
    }
    let mut lo = 0usize;
    let mut hi = times.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        if times[mid] < target {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let mut best_idx = lo.saturating_sub(1);
    let mut best_diff = (target - times[best_idx]).abs();
    if lo < times.len() {
        let diff = (target - times[lo]).abs();
        if diff < best_diff {
            best_idx = lo;
            best_diff = diff;
        }
    }
    Some((best_idx, best_diff))
}

fn match_dslr_to_livescan(
    dslr_images: &[DslrImageInfo],
    livescan_times: &[f64],
    offset: f64,
    max_gap: f64,
) -> (HashMap<usize, Vec<DslrImageInfo>>, Vec<DslrImageInfo>, f64) {
    let mut mapping: HashMap<usize, Vec<DslrImageInfo>> = HashMap::new();
    let mut unmatched = Vec::new();
    let mut total_error = 0.0;
    let mut matched = 0usize;
    for img in dslr_images {
        let adjusted = img.timestamp - offset;
        if let Some((frame_idx, diff)) = nearest_index(adjusted, livescan_times) {
            if diff <= max_gap {
                mapping.entry(frame_idx).or_default().push(img.clone());
                total_error += diff;
                matched += 1;
                continue;
            }
        }
        unmatched.push(img.clone());
    }
    let mean_error = if matched > 0 {
        total_error / matched as f64
    } else {
        0.0
    };
    for images in mapping.values_mut() {
        images.sort_by(|a, b| a.camera_key.cmp(&b.camera_key));
    }
    (mapping, unmatched, mean_error)
}

fn select_rig_frame(
    frame_indices: &[usize],
    matches: &HashMap<usize, Vec<DslrImageInfo>>,
) -> (usize, Vec<PathBuf>) {
    let mut best_frame = *frame_indices.first().unwrap_or(&0);
    let mut best_count = 0usize;
    let mut best_paths: Vec<PathBuf> = Vec::new();
    for &idx in frame_indices {
        let Some(images) = matches.get(&idx) else {
            continue;
        };
        let mut unique = HashSet::new();
        for img in images {
            unique.insert(img.camera_key.clone());
        }
        if unique.len() > best_count {
            best_count = unique.len();
            best_frame = idx;
            best_paths = images.iter().map(|img| img.path.clone()).collect();
        }
    }
    (best_frame, best_paths)
}

fn rig_pose_map_by_key(rig: &ScannerRig, angle: f32) -> HashMap<String, CameraPose> {
    let propagated = rig.propagate(angle);
    let mut map = HashMap::new();
    for pose in propagated {
        let key = camera_key_from_path(Path::new(&pose.name));
        map.insert(key, pose.pose);
    }
    map
}

fn pose_from_livescan_frame(
    frame: &crate::reconstruction::livescan::LiveScanFrame,
    convention: TransformConvention,
) -> Option<CameraPose> {
    let mat = na::Matrix4::from_fn(|r, c| frame.transform_matrix[r][c]);
    let pose_mat = match convention {
        TransformConvention::CameraToWorld => mat,
        TransformConvention::WorldToCamera => mat.try_inverse()?,
    };
    let rotation = pose_mat.fixed_view::<3, 3>(0, 0);
    let translation = pose_mat.fixed_view::<3, 1>(0, 3);
    let rotation_64 = na::Matrix3::from_fn(|r, c| rotation[(r, c)]);
    let translation_64 = na::Vector3::from_fn(|r, c| translation[(r, c)]);
    let q = na::UnitQuaternion::from_rotation_matrix(&na::Rotation3::from_matrix(&rotation_64));
    Some(CameraPose {
        rotation: q,
        translation: translation_64,
    })
}

fn collect_camera_keys(dslr_images: &[DslrImageInfo]) -> Vec<String> {
    let mut keys: Vec<String> = dslr_images.iter().map(|img| img.camera_key.clone()).collect();
    keys.sort();
    keys.dedup();
    keys
}

fn write_sync_report(workspace: &Path, report: &SyncReport) -> Result<()> {
    let dir = workspace.join("processed").join("sfm");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("sync_report.json");
    let payload = serde_json::to_string_pretty(report)?;
    std::fs::write(&path, payload)
        .with_context(|| format!("Failed to write sync report to {}", path.display()))?;
    Ok(())
}
