use crate::export::{export_point_cloud_ply, write_provenance_for_export};
use crate::gaussian_splatting::{Camera as GsCamera, GaussianSplatTrainer, TrainingConfig};
use crate::intrinsics::estimate_intrinsics;
use crate::photogrammetry::heatmap::{apply_heatmap_to_points, CoverageVoxelGrid};
use crate::reconstruction::livescan::PosePriors;
use crate::reconstruction::multicam_sfm::{
    patchmatch_stereo, CameraIntrinsics, CameraPose, DepthMap, FeatureType, MvsInput,
    PatchMatchConfig, SfmConfig, SfmPipeline, SparseReconstruction,
};
use crate::reconstruction::ColoredPoint;
use crate::reconstruction::QualityLevel;
use anyhow::{Context, Result};
use image::{imageops::FilterType, DynamicImage, GrayImage, RgbImage};
use nalgebra as na;
use std::path::{Path, PathBuf};

#[derive(Copy, Clone)]
pub enum ReconstructionType {
    PhotogrammetryHighQuality,
    PhotogrammetryFast,
    GaussianSplatting,
}

pub struct ReconstructionConfig {
    pub reconstruction_type: ReconstructionType,
    pub workspace_path: PathBuf,
}

pub struct ReconstructionPipeline {
    config: ReconstructionConfig,
}

impl ReconstructionPipeline {
    pub fn new(config: ReconstructionConfig) -> Self {
        Self { config }
    }

    /// Run the native reconstruction pipeline (SfM + optional MVS + optional 3DGS).
    pub fn run(&self, priors: Option<PosePriors>) -> Result<()> {
        let images_dir = self.config.workspace_path.join("raw/images");
        let image_paths = collect_image_paths(&images_dir)?;
        if image_paths.len() < 2 {
            anyhow::bail!("Need at least 2 images under {}", images_dir.display());
        }

        let quality = quality_for_reconstruction_type(self.config.reconstruction_type);
        let sfm_config = sfm_config_for_quality(quality);

        let mut pipeline = SfmPipeline::new(sfm_config);
        let mut prior_poses: Vec<CameraPose> = Vec::new();
        let mut have_all_priors = true;

        if let Some(priors) = priors.as_ref() {
            for path in &image_paths {
                let intrinsics = match priors.intrinsics_for_path(path) {
                    Some(intr) => intr,
                    None => estimate_intrinsics(path)?,
                };
                let prior_pose = priors.pose_for_path(path);
                let camera_motion = priors.motion_for_path(path);
                let rolling_shutter = priors.rolling_shutter_for_path(path);
                if prior_pose.is_none() {
                    have_all_priors = false;
                }
                if let Some(pose) = &prior_pose {
                    prior_poses.push(pose.clone());
                }
                pipeline.add_image_with_context(
                    path,
                    intrinsics,
                    prior_pose,
                    rolling_shutter,
                    camera_motion,
                )?;
            }
        } else {
            pipeline.add_images(&image_paths)?;
        }

        if priors.is_some() && !have_all_priors {
            tracing::warn!("Pose priors missing for some images; falling back to pose estimation.");
        }

        let mut reconstruction = if priors.is_some() && have_all_priors {
            pipeline.run_with_priors(&prior_poses)?
        } else {
            pipeline.run()?
        };

        let processed_dir = self.config.workspace_path.join("processed/sfm");
        std::fs::create_dir_all(&processed_dir)?;
        if let Some(priors) = priors.as_ref() {
            if !priors.imu_samples.is_empty() {
                let imu_path = processed_dir.join("imu_timeline.json");
                if let Ok(payload) = serde_json::to_string_pretty(&priors.imu_samples) {
                    let _ = std::fs::write(&imu_path, payload);
                }
            }
        }

        let mut color_images =
            load_color_images(&image_paths, sparse_color_scale_for_quality(quality))?;
        let poses = reconstruction.poses.clone();
        let cameras = reconstruction.cameras.clone();
        colorize_sparse_points(
            &mut reconstruction,
            &mut color_images,
            &poses,
            &cameras,
            sparse_color_views_for_quality(quality),
        );
        reconstruction.export_ply(processed_dir.join("sparse.ply"))?;

        if !reconstruction.points.is_empty() {
            let voxel_size = quality.voxel_size();
            let colored_points: Vec<ColoredPoint> = reconstruction
                .points
                .iter()
                .map(|p| ColoredPoint {
                    position: na::Point3::new(
                        p.position.x as f32,
                        p.position.y as f32,
                        p.position.z as f32,
                    ),
                    color: p.color,
                    confidence: 1.0,
                })
                .collect();
            let heatmap_points = apply_heatmap_to_points(&colored_points, voxel_size);
            let mut positions: Vec<na::Point3<f32>> = Vec::with_capacity(heatmap_points.len());
            let mut colors: Vec<[u8; 3]> = Vec::with_capacity(heatmap_points.len());
            for point in &heatmap_points {
                positions.push(point.position);
                colors.push(point.color);
            }
            let heatmap_path = processed_dir.join("coverage_heatmap.ply");
            export_point_cloud_ply(&positions, Some(&colors), None, &heatmap_path)?;

            let mut grid = CoverageVoxelGrid::new(voxel_size);
            grid.add_points(&colored_points);
            let stats = grid.get_stats();
            let stats_payload = serde_json::json!({
                "voxel_size": voxel_size,
                "total_voxels": stats.total_voxels,
                "none_count": stats.none_count,
                "very_low_count": stats.very_low_count,
                "low_count": stats.low_count,
                "medium_count": stats.medium_count,
                "good_count": stats.good_count,
                "excellent_count": stats.excellent_count,
                "max_density": stats.max_density,
                "good_coverage_percent": stats.good_coverage_percent(),
                "poor_coverage_percent": stats.poor_coverage_percent(),
            });
            let stats_path = processed_dir.join("coverage_stats.json");
            std::fs::write(&stats_path, serde_json::to_string_pretty(&stats_payload)?)?;
            write_provenance_for_export(&stats_path)?;
        }

        let mut dense_points: Vec<(na::Point3<f64>, [u8; 3])> = Vec::new();
        if should_run_dense(quality) {
            dense_points = run_dense_mvs(
                &image_paths,
                &reconstruction.poses,
                &reconstruction.cameras,
                quality,
            )?;
            if !dense_points.is_empty() {
                export_dense_point_cloud(&dense_points, &processed_dir.join("dense.ply"))?;
            }
        }

        if matches!(
            self.config.reconstruction_type,
            ReconstructionType::GaussianSplatting
        ) {
            let output_dir = self.config.workspace_path.join("output");
            std::fs::create_dir_all(&output_dir)?;
            run_gaussian_splatting(
                &output_dir.join("gaussians.ply"),
                &image_paths,
                &reconstruction.poses,
                &reconstruction.cameras,
                quality,
                &dense_points,
                &reconstruction.points,
            )?;
        }

        Ok(())
    }
}

fn quality_for_reconstruction_type(recon_type: ReconstructionType) -> QualityLevel {
    match recon_type {
        ReconstructionType::PhotogrammetryFast => QualityLevel::Low,
        ReconstructionType::PhotogrammetryHighQuality => QualityLevel::High,
        ReconstructionType::GaussianSplatting => QualityLevel::High,
    }
}

fn sfm_config_for_quality(quality: QualityLevel) -> SfmConfig {
    match quality {
        QualityLevel::Low => SfmConfig {
            feature_type: FeatureType::Orb,
            max_features: 4000,
            match_ratio: 0.8,
            min_matches: 40,
            ba_iterations: 40,
            local_ba_window: 4,
            local_ba_stride: 2,
            local_ba_iterations: 16,
            local_ba_min_points: 140,
            local_ba_min_rmse: 1.1,
            enable_dense: should_run_dense(quality),
            num_threads: num_cpus::get(),
        },
        QualityLevel::Medium => SfmConfig {
            feature_type: FeatureType::Akaze,
            max_features: 6000,
            match_ratio: 0.75,
            min_matches: 50,
            ba_iterations: 60,
            local_ba_window: 5,
            local_ba_stride: 2,
            local_ba_iterations: 22,
            local_ba_min_points: 180,
            local_ba_min_rmse: 0.9,
            enable_dense: should_run_dense(quality),
            num_threads: num_cpus::get(),
        },
        QualityLevel::High => SfmConfig {
            feature_type: FeatureType::Sift,
            max_features: 10000,
            match_ratio: 0.7,
            min_matches: 60,
            ba_iterations: 120,
            local_ba_window: 6,
            local_ba_stride: 2,
            local_ba_iterations: 32,
            local_ba_min_points: 220,
            local_ba_min_rmse: 0.8,
            enable_dense: should_run_dense(quality),
            num_threads: num_cpus::get(),
        },
        QualityLevel::Ultra => SfmConfig {
            feature_type: FeatureType::Sift,
            max_features: 14000,
            match_ratio: 0.7,
            min_matches: 80,
            ba_iterations: 180,
            local_ba_window: 7,
            local_ba_stride: 1,
            local_ba_iterations: 40,
            local_ba_min_points: 260,
            local_ba_min_rmse: 0.7,
            enable_dense: should_run_dense(quality),
            num_threads: num_cpus::get(),
        },
    }
}

fn collect_image_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    if !dir.exists() {
        anyhow::bail!("Image directory {} does not exist", dir.display());
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && is_image_path(&path) {
            paths.push(path);
        }
    }

    paths.sort();
    Ok(paths)
}

fn is_image_path(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => matches!(
            ext.to_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "tif" | "tiff" | "nef" | "arw"
        ),
        None => false,
    }
}

fn sparse_color_scale_for_quality(quality: QualityLevel) -> f32 {
    match quality {
        QualityLevel::Low => 0.5,
        QualityLevel::Medium => 0.75,
        QualityLevel::High => 1.0,
        QualityLevel::Ultra => 1.0,
    }
}

fn sparse_color_views_for_quality(quality: QualityLevel) -> usize {
    match quality {
        QualityLevel::Low => 2,
        QualityLevel::Medium => 4,
        QualityLevel::High => 6,
        QualityLevel::Ultra => 8,
    }
}

fn should_run_dense(quality: QualityLevel) -> bool {
    matches!(
        quality,
        QualityLevel::Medium | QualityLevel::High | QualityLevel::Ultra
    )
}

fn load_color_images(paths: &[PathBuf], scale: f32) -> Result<Vec<RgbImage>> {
    let mut images = Vec::with_capacity(paths.len());
    for path in paths {
        let img = image::open(path)
            .with_context(|| format!("Failed to open image {}", path.display()))?
            .to_rgb8();
        if (scale - 1.0).abs() > f32::EPSILON {
            let width = (img.width() as f32 * scale).round().max(1.0) as u32;
            let height = (img.height() as f32 * scale).round().max(1.0) as u32;
            let resized = image::imageops::resize(&img, width, height, FilterType::Lanczos3);
            images.push(resized);
        } else {
            images.push(img);
        }
    }
    Ok(images)
}

fn colorize_sparse_points(
    reconstruction: &mut SparseReconstruction,
    images: &mut [RgbImage],
    poses: &[CameraPose],
    intrinsics: &[CameraIntrinsics],
    max_views: usize,
) {
    if images.is_empty() || poses.is_empty() || intrinsics.is_empty() {
        return;
    }

    let view_limit = max_views.min(images.len());
    for point in &mut reconstruction.points {
        let mut colored = false;
        for view_idx in 0..view_limit {
            if let Some((x, y)) =
                project_point(&point.position, &poses[view_idx], &intrinsics[view_idx])
            {
                let pixel = images[view_idx].get_pixel(x, y);
                point.color = [pixel[0], pixel[1], pixel[2]];
                colored = true;
                break;
            }
        }
        if !colored {
            point.color = [180, 180, 180];
        }
    }
}

fn project_point(
    point_world: &na::Point3<f64>,
    pose: &CameraPose,
    intrinsics: &CameraIntrinsics,
) -> Option<(u32, u32)> {
    let rotation = pose.rotation.to_rotation_matrix();
    let cam = rotation.inverse() * (point_world.coords - pose.translation);
    if cam.z <= 0.0 {
        return None;
    }
    let x = (intrinsics.fx * cam.x / cam.z + intrinsics.cx).round();
    let y = (intrinsics.fy * cam.y / cam.z + intrinsics.cy).round();
    if x < 0.0 || y < 0.0 {
        return None;
    }
    let (x, y) = (x as u32, y as u32);
    if x < intrinsics.width && y < intrinsics.height {
        Some((x, y))
    } else {
        None
    }
}

struct MvsImage {
    rgb: RgbImage,
    gray: GrayImage,
    intrinsics: CameraIntrinsics,
}

fn run_dense_mvs(
    image_paths: &[PathBuf],
    poses: &[CameraPose],
    intrinsics: &[CameraIntrinsics],
    quality: QualityLevel,
) -> Result<Vec<(na::Point3<f64>, [u8; 3])>> {
    let scale = dense_scale_for_quality(quality);
    let images = load_mvs_images(image_paths, intrinsics, scale)?;
    let adjusted_intrinsics: Vec<CameraIntrinsics> =
        images.iter().map(|img| img.intrinsics.clone()).collect();

    let patch_config = patchmatch_config_from_quality(quality);
    let mut depth_maps: Vec<DepthMap> = Vec::with_capacity(images.len());

    for (i, image) in images.iter().enumerate() {
        let src_indices = select_source_indices(i, images.len(), max_source_views(quality));
        let src_images: Vec<&GrayImage> =
            src_indices.iter().map(|&idx| &images[idx].gray).collect();
        let src_poses: Vec<&CameraPose> = src_indices.iter().map(|&idx| &poses[idx]).collect();
        let src_intrinsics: Vec<&CameraIntrinsics> = src_indices
            .iter()
            .map(|&idx| &adjusted_intrinsics[idx])
            .collect();

        let input = MvsInput {
            ref_image: &image.gray,
            ref_pose: &poses[i],
            ref_intrinsics: &adjusted_intrinsics[i],
            src_images,
            src_poses,
            src_intrinsics,
        };

        let depth_map = patchmatch_stereo(&input, &patch_config);
        depth_maps.push(depth_map);
    }

    let consistency = consistency_threshold_for_quality(quality);
    let min_views = min_views_for_quality(quality);
    Ok(fuse_depth_maps_colored(
        &depth_maps,
        &images,
        poses,
        &adjusted_intrinsics,
        consistency,
        min_views,
    ))
}

fn load_mvs_images(
    image_paths: &[PathBuf],
    intrinsics: &[CameraIntrinsics],
    scale: f32,
) -> Result<Vec<MvsImage>> {
    let mut images = Vec::with_capacity(image_paths.len());
    for (idx, path) in image_paths.iter().enumerate() {
        let img = image::open(path)
            .with_context(|| format!("Failed to open image {}", path.display()))?;
        let (rgb, gray) = downscale_image_pair(&img, scale);
        let intr = scale_intrinsics(&intrinsics[idx], scale, rgb.width(), rgb.height());
        images.push(MvsImage {
            rgb,
            gray,
            intrinsics: intr,
        });
    }
    Ok(images)
}

fn downscale_image_pair(img: &DynamicImage, scale: f32) -> (RgbImage, GrayImage) {
    let rgb = img.to_rgb8();
    let gray = img.to_luma8();
    if (scale - 1.0).abs() <= f32::EPSILON {
        return (rgb, gray);
    }
    let width = (rgb.width() as f32 * scale).round().max(1.0) as u32;
    let height = (rgb.height() as f32 * scale).round().max(1.0) as u32;
    let rgb_resized = image::imageops::resize(&rgb, width, height, FilterType::Lanczos3);
    let gray_resized = image::imageops::resize(&gray, width, height, FilterType::Lanczos3);
    (rgb_resized, gray_resized)
}

fn scale_intrinsics(
    intrinsics: &CameraIntrinsics,
    scale: f32,
    width: u32,
    height: u32,
) -> CameraIntrinsics {
    CameraIntrinsics {
        fx: intrinsics.fx * scale as f64,
        fy: intrinsics.fy * scale as f64,
        cx: intrinsics.cx * scale as f64,
        cy: intrinsics.cy * scale as f64,
        width,
        height,
        distortion: intrinsics.distortion.clone(),
        distortion_model: intrinsics.distortion_model,
    }
}

fn dense_scale_for_quality(quality: QualityLevel) -> f32 {
    match quality {
        QualityLevel::Low => 0.5,
        QualityLevel::Medium => 0.75,
        QualityLevel::High => 1.0,
        QualityLevel::Ultra => 1.0,
    }
}

fn patchmatch_config_from_quality(quality: QualityLevel) -> PatchMatchConfig {
    match quality {
        QualityLevel::Low => PatchMatchConfig {
            patch_radius: 3,
            num_iterations: 2,
            num_samples: 6,
            ncc_threshold: 0.55,
            ..Default::default()
        },
        QualityLevel::Medium => PatchMatchConfig {
            patch_radius: 5,
            num_iterations: 3,
            num_samples: 8,
            ncc_threshold: 0.6,
            ..Default::default()
        },
        QualityLevel::High => PatchMatchConfig {
            patch_radius: 7,
            num_iterations: 4,
            num_samples: 10,
            ncc_threshold: 0.65,
            ..Default::default()
        },
        QualityLevel::Ultra => PatchMatchConfig {
            patch_radius: 7,
            num_iterations: 5,
            num_samples: 12,
            ncc_threshold: 0.7,
            ..Default::default()
        },
    }
}

fn max_source_views(quality: QualityLevel) -> usize {
    match quality {
        QualityLevel::Low => 4,
        QualityLevel::Medium => 6,
        QualityLevel::High => 10,
        QualityLevel::Ultra => 12,
    }
}

fn select_source_indices(index: usize, total: usize, max_sources: usize) -> Vec<usize> {
    let mut indices = Vec::new();
    let mut offset = 1;
    while indices.len() < max_sources && offset < total {
        if index >= offset {
            indices.push(index - offset);
        }
        if indices.len() >= max_sources {
            break;
        }
        if index + offset < total {
            indices.push(index + offset);
        }
        offset += 1;
    }
    indices
}

fn consistency_threshold_for_quality(quality: QualityLevel) -> f32 {
    match quality {
        QualityLevel::Low => 0.02,
        QualityLevel::Medium => 0.015,
        QualityLevel::High => 0.01,
        QualityLevel::Ultra => 0.008,
    }
}

fn min_views_for_quality(quality: QualityLevel) -> usize {
    match quality {
        QualityLevel::Low => 2,
        QualityLevel::Medium => 2,
        QualityLevel::High => 3,
        QualityLevel::Ultra => 3,
    }
}

fn fuse_depth_maps_colored(
    depth_maps: &[DepthMap],
    images: &[MvsImage],
    poses: &[CameraPose],
    intrinsics: &[CameraIntrinsics],
    consistency_threshold: f32,
    min_views: usize,
) -> Vec<(na::Point3<f64>, [u8; 3])> {
    let mut points = Vec::new();
    if depth_maps.is_empty() {
        return points;
    }

    for (ref_idx, depth_map) in depth_maps.iter().enumerate() {
        let intr = &intrinsics[ref_idx];
        let pose = &poses[ref_idx];
        let rgb = &images[ref_idx].rgb;

        for y in 0..depth_map.height {
            for x in 0..depth_map.width {
                let (depth, confidence, _normal) = depth_map.get(x, y).unwrap();
                if depth <= 0.0 || confidence < 0.5 {
                    continue;
                }

                let point_world = unproject_to_world(x, y, depth, intr, pose);

                let mut consistent_views = 1;
                for (src_idx, src_depth_map) in depth_maps.iter().enumerate() {
                    if src_idx == ref_idx {
                        continue;
                    }
                    let src_pose = &poses[src_idx];
                    let src_intr = &intrinsics[src_idx];
                    if let Some((sx, sy, sz)) = project_to_camera(&point_world, src_pose, src_intr)
                    {
                        if let Some((src_depth, src_conf, _)) = src_depth_map.get(sx, sy) {
                            if src_conf > 0.5 {
                                let depth_diff = (src_depth as f64 - sz).abs();
                                if depth_diff < consistency_threshold as f64 {
                                    consistent_views += 1;
                                }
                            }
                        }
                    }
                }

                if consistent_views >= min_views {
                    let pixel = rgb.get_pixel(x.min(rgb.width() - 1), y.min(rgb.height() - 1));
                    points.push((point_world, [pixel[0], pixel[1], pixel[2]]));
                }
            }
        }
    }

    points
}

fn unproject_to_world(
    x: u32,
    y: u32,
    depth: f32,
    intrinsics: &CameraIntrinsics,
    pose: &CameraPose,
) -> na::Point3<f64> {
    let x3d = (x as f64 - intrinsics.cx) * depth as f64 / intrinsics.fx;
    let y3d = (y as f64 - intrinsics.cy) * depth as f64 / intrinsics.fy;
    let point_cam = na::Point3::new(x3d, y3d, depth as f64);
    let rotation = pose.rotation.to_rotation_matrix();
    na::Point3::from(rotation * point_cam.coords + pose.translation)
}

fn project_to_camera(
    point_world: &na::Point3<f64>,
    pose: &CameraPose,
    intrinsics: &CameraIntrinsics,
) -> Option<(u32, u32, f64)> {
    let rotation = pose.rotation.to_rotation_matrix();
    let point_cam = rotation.inverse() * (point_world.coords - pose.translation);
    if point_cam.z <= 0.0 {
        return None;
    }
    let px = (intrinsics.fx * point_cam.x / point_cam.z + intrinsics.cx).round();
    let py = (intrinsics.fy * point_cam.y / point_cam.z + intrinsics.cy).round();
    if px < 0.0 || py < 0.0 {
        return None;
    }
    let (px, py) = (px as u32, py as u32);
    if px < intrinsics.width && py < intrinsics.height {
        Some((px, py, point_cam.z))
    } else {
        None
    }
}

fn export_dense_point_cloud(points: &[(na::Point3<f64>, [u8; 3])], path: &Path) -> Result<()> {
    let mut positions = Vec::with_capacity(points.len());
    let mut colors = Vec::with_capacity(points.len());
    for (p, c) in points {
        positions.push(na::Point3::new(p.x as f32, p.y as f32, p.z as f32));
        colors.push(*c);
    }
    export_point_cloud_ply(&positions, Some(&colors), None, path)?;
    Ok(())
}

fn training_config_from_quality(quality: QualityLevel) -> TrainingConfig {
    match quality {
        QualityLevel::Low => TrainingConfig {
            iterations: 800,
            use_gpu_gradients: true,
            gpu_parity_check: true,
            ..TrainingConfig::default()
        },
        QualityLevel::Medium => TrainingConfig {
            iterations: 1500,
            use_gpu_gradients: true,
            gpu_parity_check: true,
            ..TrainingConfig::default()
        },
        QualityLevel::High => TrainingConfig {
            iterations: 2500,
            use_gpu_gradients: true,
            gpu_parity_check: true,
            ..TrainingConfig::default()
        },
        QualityLevel::Ultra => TrainingConfig {
            iterations: 3500,
            use_gpu_gradients: true,
            gpu_parity_check: true,
            ..TrainingConfig::default()
        },
    }
}

fn run_gaussian_splatting(
    output: &Path,
    image_paths: &[PathBuf],
    poses: &[CameraPose],
    intrinsics: &[CameraIntrinsics],
    quality: QualityLevel,
    dense_points: &[(na::Point3<f64>, [u8; 3])],
    sparse_points: &[trueshot_sfm::Point3D],
) -> Result<()> {
    let mut initial_points: Vec<(na::Point3<f32>, [u8; 3])> = Vec::new();
    if !dense_points.is_empty() {
        for (p, c) in dense_points {
            initial_points.push((na::Point3::new(p.x as f32, p.y as f32, p.z as f32), *c));
        }
    } else {
        for p in sparse_points {
            initial_points.push((
                na::Point3::new(
                    p.position.x as f32,
                    p.position.y as f32,
                    p.position.z as f32,
                ),
                p.color,
            ));
        }
    }

    if initial_points.is_empty() {
        anyhow::bail!("No points available to initialize Gaussian splatting");
    }

    let mut cameras = Vec::with_capacity(image_paths.len());
    for (idx, path) in image_paths.iter().enumerate() {
        let pose = &poses[idx];
        let intr = &intrinsics[idx];
        let rotation = pose.rotation.to_rotation_matrix();
        let mut transform = na::Matrix4::<f32>::identity();
        transform
            .fixed_view_mut::<3, 3>(0, 0)
            .copy_from(&rotation.matrix().map(|v| v as f32));
        transform[(0, 3)] = pose.translation.x as f32;
        transform[(1, 3)] = pose.translation.y as f32;
        transform[(2, 3)] = pose.translation.z as f32;

        let intr_matrix = na::Matrix3::<f32>::new(
            intr.fx as f32,
            0.0,
            intr.cx as f32,
            0.0,
            intr.fy as f32,
            intr.cy as f32,
            0.0,
            0.0,
            1.0,
        );

        cameras.push(GsCamera {
            transform,
            intrinsics: intr_matrix,
            width: intr.width,
            height: intr.height,
            image_path: path.clone(),
        });
    }

    let config = training_config_from_quality(quality);
    let mut trainer = GaussianSplatTrainer::new(&initial_points, cameras, config.clone());

    for _ in 0..config.iterations {
        trainer.step()?;
    }

    let output_path = output.to_path_buf();
    trainer.export_ply(&output_path)?;
    Ok(())
}
