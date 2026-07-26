//! Unified Multi-Camera SfM Integration
//!
//! Bridges trueshot-sfm with the HybridPipeline for production-quality reconstruction.
//! Handles:
//! - Multi-camera livescan (10+ webcams simultaneously)
//! - Sequential SD card ingestion (multiple DSLRs)
//! - Efficient matching with pre-computed livescan poses
//! - Focus-stacked and HDR image handling

use image::{GrayImage, ImageEncoder, RgbImage};
use nalgebra as na;
use ndarray::{Array2, Array3};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::align_raw::align_phasecorr_gray_with_scale;
use crate::hierarchical_collapse::{collapse_foreground_single_pass, HierarchicalParams};
use crate::hierarchical_grading::{compute_sharpness_map_from_rgb, grade_pixels, GradingParams};
use crate::types::AlignmentInfo;

// Re-export trueshot-sfm types for convenience
pub use trueshot_sfm::{
    CameraIntrinsics, CameraPose, FeatureType, ImageData, Point3D, ReprojectionStats, SfmConfig,
    SfmPipeline, SparseReconstruction,
};

// Dense reconstruction
pub use trueshot_sfm::dense::MvsInput;
pub use trueshot_sfm::dense::{fuse_depth_maps, patchmatch_stereo, DepthMap, PatchMatchConfig};

// Mesh generation
pub use trueshot_sfm::mesh::{marching_cubes_reconstruction, Mesh, OrientedPoint};

/// Camera identifier for multi-camera setups
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CameraId(pub String);

impl CameraId {
    pub fn webcam(idx: usize) -> Self {
        Self(format!("webcam_{}", idx))
    }

    pub fn dslr(name: &str) -> Self {
        Self(format!("dslr_{}", name))
    }
}

/// Livescan frame data from a webcam
#[derive(Debug, Clone)]
pub struct LivescanFrame {
    pub camera_id: CameraId,
    pub timestamp_ms: u64,
    pub pose: CameraPose,
    pub intrinsics: CameraIntrinsics,
    /// Low-res image data (640x480 typical)
    pub image_data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Turntable angle at capture time
    pub turntable_angle: f32,
    /// Detected features (pre-computed during livescan)
    pub features: Vec<LivescanFeature>,
}

/// Pre-computed feature for fast matching
#[derive(Debug, Clone)]
pub struct LivescanFeature {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub angle: f32,
    pub descriptor: Vec<u8>,
}

/// High-res image from SD card
#[derive(Debug, Clone)]
pub struct HighResImage {
    pub camera_id: CameraId,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub intrinsics: CameraIntrinsics,
    /// EXIF timestamp for matching to livescan
    pub timestamp_ms: Option<u64>,
    /// Focus distance (for focus stacking)
    pub focus_distance: Option<f32>,
    /// Exposure value (for HDR)
    pub exposure_value: Option<f32>,
    /// Bracketing group ID
    pub bracket_group: Option<u32>,
    /// Decoded/fused pixels retained across feature extraction and dense MVS.
    pub pixels: Option<Arc<RgbImage>>,
}

/// Reconstruction configuration for multi-camera setup
#[derive(Debug, Clone)]
pub struct MultiCamConfig {
    /// Number of webcams in the ring
    pub num_webcams: usize,
    /// Feature type for livescan (fast)
    pub livescan_feature_type: FeatureType,
    /// Feature type for high-res (quality)
    pub highres_feature_type: FeatureType,
    /// Max features per livescan frame
    pub livescan_max_features: usize,
    /// Max features per high-res image
    pub highres_max_features: usize,
    /// Bundle adjustment iterations during livescan
    pub livescan_ba_iterations: usize,
    /// Bundle adjustment iterations for final
    pub final_ba_iterations: usize,
    /// Enable dense reconstruction
    pub enable_dense: bool,
    /// Dense MVS config
    pub mvs_config: PatchMatchConfig,
    /// Merge tolerance for duplicate points (mm)
    pub point_merge_tolerance: f32,
    /// Max source views per reference for dense MVS
    pub dense_max_sources: usize,
    /// Minimum consistent views for depth fusion
    pub dense_min_views: usize,
    /// Depth consistency threshold (world units)
    pub dense_consistency_threshold: f32,
    /// Marching cubes resolution for mesh extraction
    pub mesh_resolution: u32,
    /// Persist fused per-view PNGs for project reopen/resume.
    pub persist_focus_stacks: bool,
}

impl Default for MultiCamConfig {
    fn default() -> Self {
        Self {
            num_webcams: 1,
            livescan_feature_type: FeatureType::Orb, // Fast
            highres_feature_type: FeatureType::Sift, // Quality
            livescan_max_features: 500,
            highres_max_features: 8000,
            livescan_ba_iterations: 10,
            final_ba_iterations: 100,
            enable_dense: true,
            mvs_config: PatchMatchConfig::default(),
            point_merge_tolerance: 1.0,
            dense_max_sources: 4,
            dense_min_views: 2,
            dense_consistency_threshold: 0.01,
            mesh_resolution: 256,
            persist_focus_stacks: true,
        }
    }
}

/// Multi-camera SfM pipeline state
pub struct MultiCamSfm {
    pub config: MultiCamConfig,

    /// Livescan poses indexed by (camera_id, turntable_angle)
    pub livescan_poses: HashMap<(CameraId, i32), CameraPose>,

    /// Livescan intrinsics per camera
    pub camera_intrinsics: HashMap<CameraId, CameraIntrinsics>,

    /// Sparse point cloud from livescan
    pub livescan_points: Vec<Point3D>,

    /// High-res images pending processing
    pub pending_highres: Vec<HighResImage>,

    /// Focus stacking groups: bracket_group -> images
    pub focus_groups: HashMap<u32, Vec<HighResImage>>,

    /// Final sparse reconstruction
    pub sparse_recon: Option<SparseReconstruction>,

    /// Dense depth maps per view
    pub depth_maps: Vec<DepthMap>,

    /// Final mesh
    pub mesh: Option<Mesh>,

    /// Progress callback
    progress_callback: Option<Arc<dyn Fn(&str, f32) + Send + Sync>>,
}

impl MultiCamSfm {
    pub fn new(config: MultiCamConfig) -> Self {
        Self {
            config,
            livescan_poses: HashMap::new(),
            camera_intrinsics: HashMap::new(),
            livescan_points: Vec::new(),
            pending_highres: Vec::new(),
            focus_groups: HashMap::new(),
            sparse_recon: None,
            depth_maps: Vec::new(),
            mesh: None,
            progress_callback: None,
        }
    }

    pub fn set_progress_callback<F>(&mut self, callback: F)
    where
        F: Fn(&str, f32) + Send + Sync + 'static,
    {
        self.progress_callback = Some(Arc::new(callback));
    }

    fn report_progress(&self, stage: &str, progress: f32) {
        if let Some(cb) = &self.progress_callback {
            cb(stage, progress);
        }
    }

    // =========================================================================
    // Phase 1: Livescan Processing (Real-time during scanning)
    // =========================================================================

    /// Register a webcam with its intrinsics
    pub fn register_camera(&mut self, camera_id: CameraId, intrinsics: CameraIntrinsics) {
        self.camera_intrinsics.insert(camera_id, intrinsics);
    }

    /// Process a livescan frame from a webcam
    /// Called in real-time during scanning
    pub fn process_livescan_frame(&mut self, frame: LivescanFrame) -> anyhow::Result<()> {
        // Quantize turntable angle to nearest degree for indexing
        let angle_key = (frame.turntable_angle + 0.5) as i32 % 360;

        // Store pose for later high-res matching
        self.livescan_poses
            .insert((frame.camera_id.clone(), angle_key), frame.pose.clone());

        // Store intrinsics if not already present
        self.camera_intrinsics
            .entry(frame.camera_id)
            .or_insert(frame.intrinsics);

        // For now, just store features for later matching
        // In production, we'd also triangulate points incrementally

        Ok(())
    }

    /// Get livescan pose nearest to a turntable angle
    pub fn get_livescan_pose(&self, camera_id: &CameraId, angle: f32) -> Option<&CameraPose> {
        let angle_key = (angle + 0.5) as i32 % 360;
        self.livescan_poses.get(&(camera_id.clone(), angle_key))
    }

    /// Get number of registered livescan poses
    pub fn livescan_pose_count(&self) -> usize {
        self.livescan_poses.len()
    }

    // =========================================================================
    // Phase 2: SD Card Ingestion (After scanning)
    // =========================================================================

    /// Add high-res images from an SD card
    pub fn ingest_sd_card(&mut self, images: Vec<HighResImage>) -> anyhow::Result<()> {
        self.report_progress("Ingesting SD card", 0.0);

        let total = images.len();
        for (i, image) in images.into_iter().enumerate() {
            // Group focus-stacked images
            if let Some(group_id) = image.bracket_group {
                self.focus_groups
                    .entry(group_id)
                    .or_default()
                    .push(image.clone());
            }

            self.pending_highres.push(image);

            self.report_progress("Ingesting SD card", (i + 1) as f32 / total as f32);
        }

        tracing::info!(
            "Ingested {} high-res images, {} focus groups",
            self.pending_highres.len(),
            self.focus_groups.len()
        );

        Ok(())
    }

    // =========================================================================
    // Phase 3: High-Res Reconstruction (After all SD cards ingested)
    // =========================================================================

    /// Run full reconstruction pipeline
    pub fn run_reconstruction(&mut self) -> anyhow::Result<SparseReconstruction> {
        self.report_progress("Starting reconstruction", 0.0);

        // Step 1: Process focus stacks
        self.report_progress("Processing focus stacks", 0.1);
        let fused_images = self.process_focus_stacks()?;

        // Step 2: Match high-res to livescan poses
        self.report_progress("Matching to livescan", 0.2);
        let matched = self.match_highres_to_livescan(&fused_images)?;

        // Step 3: Feature extraction and matching
        self.report_progress("Extracting features", 0.3);
        let image_data = self.extract_highres_features(&matched)?;

        // Step 4: Run SfM with livescan prior
        self.report_progress("Running SfM", 0.5);
        let sfm_config = SfmConfig {
            feature_type: self.config.highres_feature_type,
            max_features: self.config.highres_max_features,
            ba_iterations: self.config.final_ba_iterations,
            ..Default::default()
        };

        let mut pipeline = SfmPipeline::new(sfm_config);

        // Add images with livescan pose priors
        for (img_data, prior_pose) in image_data.iter().zip(matched.iter()) {
            pipeline.add_image_with_prior(img_data.clone(), prior_pose.1.clone())?;
        }

        // Run reconstruction
        let recon = pipeline.run()?;

        self.report_progress("SfM complete", 0.7);

        // Step 5: Dense reconstruction (optional)
        if self.config.enable_dense {
            self.report_progress("Running dense MVS", 0.8);
            self.run_dense_reconstruction(&recon, &matched)?;
        }

        self.sparse_recon = Some(recon.clone());
        self.report_progress("Reconstruction complete", 1.0);

        Ok(recon)
    }

    fn process_focus_stacks(&self) -> anyhow::Result<Vec<HighResImage>> {
        // Group images by bracket_group for focus stacking
        if self.focus_groups.is_empty() {
            // No focus stacking needed - return all images directly
            return Ok(self.pending_highres.clone());
        }

        tracing::info!(
            "Processing {} focus stacking groups",
            self.focus_groups.len()
        );

        let mut processed_images = Vec::new();

        // Images not in any focus group go through directly
        for image in &self.pending_highres {
            if image.bracket_group.is_none() {
                processed_images.push(image.clone());
            }
        }

        // Process each focus group using hierarchical collapse
        for (group_id, group_images) in &self.focus_groups {
            if group_images.is_empty() {
                continue;
            }
            tracing::debug!("Focus group {} has {} images", group_id, group_images.len());

            let mut grouped: BTreeMap<i32, Vec<HighResImage>> = BTreeMap::new();
            let mut fallback_idx = 0;
            for image in group_images {
                let key = image
                    .focus_distance
                    .map(|d| (d * 1000.0).round() as i32)
                    .unwrap_or_else(|| {
                        fallback_idx += 1;
                        fallback_idx
                    });
                grouped.entry(key).or_default().push(image.clone());
            }

            let mut focus_planes: Vec<FocusPlane> = Vec::new();
            for (_key, plane_images) in grouped {
                let fused = fuse_exposures(&plane_images)?;
                focus_planes.push(FocusPlane {
                    images: plane_images,
                    rgb: fused.rgb,
                    rgb_array: fused.rgb_array,
                    luma_array: fused.luma_array,
                    sharpness: Array2::zeros((1, 1)),
                });
            }

            if focus_planes.is_empty() {
                continue;
            }

            let ref_idx = focus_planes.len() / 2;
            let ref_luma = luma_channel_to_array2(&focus_planes[ref_idx].luma_array);

            let mut alignments: Vec<AlignmentInfo> = Vec::with_capacity(focus_planes.len());
            for (idx, plane) in focus_planes.iter().enumerate() {
                let plane_luma = luma_channel_to_array2(&plane.luma_array);
                let (dx, dy, scale) = if idx == ref_idx {
                    (0.0, 0.0, 1.0)
                } else {
                    align_phasecorr_gray_with_scale(&ref_luma, &plane_luma, 3)
                };
                alignments.push(AlignmentInfo { dx, dy, scale });
            }

            for (plane, align) in focus_planes.iter_mut().zip(alignments.iter()) {
                if align.dx.abs() < 1e-6
                    && align.dy.abs() < 1e-6
                    && (align.scale - 1.0).abs() < 1e-6
                {
                    plane.sharpness = compute_sharpness_map_from_rgb(
                        &plane.rgb_array,
                        &GradingParams::default(),
                    )?;
                    continue;
                }
                let aligned = apply_alignment_rgb(&plane.rgb, align);
                plane.rgb = aligned;
                plane.rgb_array = rgb_to_array(&plane.rgb);
                plane.luma_array = rgb_to_luma_array(&plane.rgb);
                plane.sharpness =
                    compute_sharpness_map_from_rgb(&plane.rgb_array, &GradingParams::default())?;
            }

            let (height, width, _) = focus_planes[0].rgb_array.dim();
            let mut max_sharpness = Array2::<f64>::zeros((height, width));
            let mut best_plane = Array2::<usize>::zeros((height, width));

            for y in 0..height {
                for x in 0..width {
                    let mut best = 0usize;
                    let mut best_val = -1.0f64;
                    for (idx, plane) in focus_planes.iter().enumerate() {
                        let val = plane.sharpness[[y, x]];
                        if val > best_val {
                            best_val = val;
                            best = idx;
                        }
                    }
                    max_sharpness[[y, x]] = best_val.max(0.0);
                    best_plane[[y, x]] = best;
                }
            }

            let foreground_mask = Array2::from_elem((height, width), true);
            let grades = grade_pixels(&max_sharpness, &foreground_mask, &GradingParams::default())?;

            let frames: Vec<Array3<f64>> = focus_planes
                .iter()
                .map(|plane| plane.luma_array.clone())
                .collect();

            let collapsed_luma = collapse_foreground_single_pass(
                &frames,
                &grades,
                &vec![1.0; frames.len()],
                &HierarchicalParams::default(),
                Some(&best_plane),
                1,
                &[1.0, 1.0, 1.0, 1.0],
                None,
            )?;

            let mut output = RgbImage::new(width as u32, height as u32);
            for y in 0..height {
                for x in 0..width {
                    let plane_idx = best_plane[[y, x]].min(focus_planes.len() - 1);
                    let pixel = focus_planes[plane_idx].rgb.get_pixel(x as u32, y as u32);
                    let mut r = pixel[0] as f64 / 255.0;
                    let mut g = pixel[1] as f64 / 255.0;
                    let mut b = pixel[2] as f64 / 255.0;

                    let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b).max(1e-6);
                    let target = collapsed_luma[[y, x]].clamp(0.0, 1.0);
                    let scale = (target / luma).clamp(0.0, 2.0);
                    r = (r * scale).clamp(0.0, 1.0);
                    g = (g * scale).clamp(0.0, 1.0);
                    b = (b * scale).clamp(0.0, 1.0);

                    output.put_pixel(
                        x as u32,
                        y as u32,
                        image::Rgb([(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]),
                    );
                }
            }

            let output_path = focus_stack_output_path(group_id, &focus_planes[0].images[0].path);
            if self.config.persist_focus_stacks {
                save_focus_stack(&output_path, &output)?;
            }
            let output = Arc::new(output);

            let representative = &focus_planes[0].images[0];
            processed_images.push(HighResImage {
                camera_id: representative.camera_id.clone(),
                path: output_path,
                width: output.width(),
                height: output.height(),
                intrinsics: representative.intrinsics.clone(),
                timestamp_ms: representative.timestamp_ms,
                focus_distance: representative.focus_distance,
                exposure_value: representative.exposure_value,
                bracket_group: None,
                pixels: Some(output),
            });
        }

        tracing::info!(
            "Processed focus stacks: {} output images from {} input images",
            processed_images.len(),
            self.pending_highres.len()
        );

        Ok(processed_images)
    }

    fn match_highres_to_livescan(
        &self,
        images: &[HighResImage],
    ) -> anyhow::Result<Vec<(HighResImage, CameraPose)>> {
        let mut matched = Vec::new();

        // Build a sorted index of livescan timestamps for binary search
        let mut livescan_timestamps: Vec<(u64, CameraId, i32)> = Vec::new();
        for (camera_id, angle) in self.livescan_poses.keys() {
            // Estimate timestamp from angle (assuming constant rotation speed)
            // In production, we'd store actual timestamps with each pose
            let estimated_ts = (*angle as u64) * 100; // Placeholder
            livescan_timestamps.push((estimated_ts, camera_id.clone(), *angle));
        }
        livescan_timestamps.sort_by_key(|x| x.0);

        for image in images {
            let pose = if let Some(ts) = image.timestamp_ms {
                // Find closest livescan pose by timestamp
                match self.find_closest_pose_by_timestamp(ts, &livescan_timestamps) {
                    Some((camera_id, angle)) => self
                        .livescan_poses
                        .get(&(camera_id, angle))
                        .cloned()
                        .unwrap_or_else(CameraPose::identity),
                    None => CameraPose::identity(),
                }
            } else {
                // No timestamp - try to match by focus distance if available
                // This helps when DSLR EXIF doesn't have accurate timestamps
                CameraPose::identity()
            };

            matched.push((image.clone(), pose));
        }

        tracing::info!(
            "Matched {} high-res images to livescan poses",
            matched.len()
        );
        Ok(matched)
    }

    /// Find the closest livescan pose by timestamp using binary search
    fn find_closest_pose_by_timestamp(
        &self,
        target_ts: u64,
        sorted_timestamps: &[(u64, CameraId, i32)],
    ) -> Option<(CameraId, i32)> {
        if sorted_timestamps.is_empty() {
            return None;
        }

        // Binary search for closest
        let idx = sorted_timestamps
            .binary_search_by_key(&target_ts, |x| x.0)
            .unwrap_or_else(|x| x.min(sorted_timestamps.len() - 1));

        // Check neighbors to find actual closest
        let mut best_idx = idx;
        let mut best_diff = (sorted_timestamps[idx].0 as i64 - target_ts as i64).unsigned_abs();

        if idx > 0 {
            let diff = (sorted_timestamps[idx - 1].0 as i64 - target_ts as i64).unsigned_abs();
            if diff < best_diff {
                best_idx = idx - 1;
                best_diff = diff;
            }
        }

        if idx + 1 < sorted_timestamps.len() {
            let diff = (sorted_timestamps[idx + 1].0 as i64 - target_ts as i64).unsigned_abs();
            if diff < best_diff {
                best_idx = idx + 1;
            }
        }

        let (_, camera_id, angle) = &sorted_timestamps[best_idx];
        Some((camera_id.clone(), *angle))
    }

    fn extract_highres_features(
        &self,
        images: &[(HighResImage, CameraPose)],
    ) -> anyhow::Result<Vec<ImageData>> {
        images
            .par_iter()
            .map(|(img, _pose)| {
                if let Some(pixels) = &img.pixels {
                    ImageData::from_rgb_image(
                        &img.path,
                        pixels,
                        &img.intrinsics,
                        self.config.highres_max_features,
                    )
                } else {
                    ImageData::from_path_with_limit(
                        &img.path,
                        &img.intrinsics,
                        self.config.highres_max_features,
                    )
                }
            })
            .collect()
    }

    fn run_dense_reconstruction(
        &mut self,
        sparse: &SparseReconstruction,
        images: &[(HighResImage, CameraPose)],
    ) -> anyhow::Result<()> {
        let view_count = sparse
            .image_names
            .len()
            .min(sparse.poses.len())
            .min(sparse.cameras.len());
        if view_count < 2 {
            tracing::warn!("Dense reconstruction skipped: need at least 2 views");
            return Ok(());
        }

        let in_memory: HashMap<String, Arc<RgbImage>> = images
            .iter()
            .filter_map(|(image, _)| {
                image.pixels.as_ref().map(|pixels| {
                    (
                        image.path.to_string_lossy().into_owned(),
                        Arc::clone(pixels),
                    )
                })
            })
            .collect();
        let mut views = Vec::with_capacity(view_count);
        for idx in 0..view_count {
            let path = PathBuf::from(&sparse.image_names[idx]);
            let rgb = if let Some(pixels) = in_memory.get(&sparse.image_names[idx]) {
                Arc::clone(pixels)
            } else {
                Arc::new(
                    image::open(&path)
                        .map_err(|e| {
                            anyhow::anyhow!("Failed to load image {}: {}", path.display(), e)
                        })?
                        .to_rgb8(),
                )
            };
            let gray = image::imageops::grayscale(rgb.as_ref());
            views.push(ViewData {
                rgb,
                gray,
                intrinsics: sparse.cameras[idx].clone(),
                pose: sparse.poses[idx].clone(),
            });
        }

        let mut depth_maps = Vec::with_capacity(view_count);
        for ref_idx in 0..view_count {
            let ref_view = &views[ref_idx];
            let src_indices = select_mvs_sources(ref_idx, &views, self.config.dense_max_sources);

            let src_images: Vec<&GrayImage> = src_indices.iter().map(|&i| &views[i].gray).collect();
            let src_poses: Vec<&CameraPose> = src_indices.iter().map(|&i| &views[i].pose).collect();
            let src_intrinsics: Vec<&CameraIntrinsics> =
                src_indices.iter().map(|&i| &views[i].intrinsics).collect();

            let input = MvsInput {
                ref_image: &ref_view.gray,
                ref_pose: &ref_view.pose,
                ref_intrinsics: &ref_view.intrinsics,
                src_images,
                src_poses,
                src_intrinsics,
            };

            let depth_map = patchmatch_stereo(&input, &self.config.mvs_config);
            depth_maps.push(depth_map);
        }

        if depth_maps.is_empty() {
            tracing::warn!("Dense reconstruction skipped: no depth maps generated");
            return Ok(());
        }

        self.depth_maps = depth_maps.clone();

        let oriented_points = collect_oriented_points(
            &depth_maps,
            &views,
            self.config.dense_consistency_threshold,
            self.config.dense_min_views,
        );

        if oriented_points.is_empty() {
            tracing::warn!("Dense reconstruction produced no oriented points");
            return Ok(());
        }

        let mut mesh =
            marching_cubes_reconstruction(&oriented_points, self.config.mesh_resolution)?;
        mesh.compute_normals();
        self.mesh = Some(mesh);
        Ok(())
    }

    // =========================================================================
    // Accessors
    // =========================================================================

    pub fn sparse_reconstruction(&self) -> Option<&SparseReconstruction> {
        self.sparse_recon.as_ref()
    }

    pub fn mesh(&self) -> Option<&Mesh> {
        self.mesh.as_ref()
    }

    pub fn depth_maps(&self) -> &[DepthMap] {
        &self.depth_maps
    }
}

struct FocusPlane {
    images: Vec<HighResImage>,
    rgb: RgbImage,
    rgb_array: Array3<f64>,
    luma_array: Array3<f64>,
    sharpness: Array2<f64>,
}

struct ViewData {
    rgb: Arc<RgbImage>,
    gray: GrayImage,
    intrinsics: CameraIntrinsics,
    pose: CameraPose,
}

fn select_mvs_sources(ref_idx: usize, views: &[ViewData], max_sources: usize) -> Vec<usize> {
    if views.len() <= 1 {
        return Vec::new();
    }
    let max_sources = max_sources.max(1).min(views.len() - 1);
    let ref_pos = views[ref_idx].pose.translation;
    let mut candidates: Vec<(usize, f64)> = views
        .iter()
        .enumerate()
        .filter(|(idx, _)| *idx != ref_idx)
        .map(|(idx, view)| {
            let dist = (view.pose.translation - ref_pos).norm();
            (idx, dist)
        })
        .collect();
    candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    candidates
        .into_iter()
        .take(max_sources)
        .map(|(idx, _)| idx)
        .collect()
}

fn backproject_point(intrinsics: &CameraIntrinsics, x: u32, y: u32, depth: f32) -> na::Point3<f64> {
    let x3d = (x as f64 - intrinsics.cx) * depth as f64 / intrinsics.fx;
    let y3d = (y as f64 - intrinsics.cy) * depth as f64 / intrinsics.fy;
    na::Point3::new(x3d, y3d, depth as f64)
}

fn compute_depth_normals(
    depth_map: &DepthMap,
    intrinsics: &CameraIntrinsics,
) -> Vec<na::Vector3<f64>> {
    let width = depth_map.width as usize;
    let height = depth_map.height as usize;
    let mut normals = vec![na::Vector3::new(0.0, 0.0, 1.0); width * height];

    if width < 2 || height < 2 {
        return normals;
    }

    for y in 0..(height - 1) {
        for x in 0..(width - 1) {
            let idx = y * width + x;
            let d = depth_map.depths[idx];
            let dx = depth_map.depths[idx + 1];
            let dy = depth_map.depths[idx + width];
            if d <= 0.0 || dx <= 0.0 || dy <= 0.0 {
                continue;
            }
            let p = backproject_point(intrinsics, x as u32, y as u32, d);
            let px = backproject_point(intrinsics, (x + 1) as u32, y as u32, dx);
            let py = backproject_point(intrinsics, x as u32, (y + 1) as u32, dy);
            let v1 = px - p;
            let v2 = py - p;
            let n = v1.cross(&v2);
            let norm = n.norm();
            if norm > 1e-9 {
                normals[idx] = n / norm;
            }
        }
    }

    normals
}

fn collect_oriented_points(
    depth_maps: &[DepthMap],
    views: &[ViewData],
    consistency_threshold: f32,
    min_views: usize,
) -> Vec<OrientedPoint> {
    let min_views = min_views.max(1);
    let mut oriented = Vec::new();

    for (ref_idx, depth_map) in depth_maps.iter().enumerate() {
        let view = &views[ref_idx];
        let width = depth_map.width;
        let height = depth_map.height;
        let normals_cam = compute_depth_normals(depth_map, &view.intrinsics);

        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) as usize;
                let depth = depth_map.depths[idx];
                let confidence = depth_map.confidences[idx];
                if depth <= 0.0 || confidence < 0.5 {
                    continue;
                }

                let point_cam = backproject_point(&view.intrinsics, x, y, depth);
                let point_world = view.pose.camera_to_world(&point_cam);

                let mut consistent_views = 1;
                for (src_idx, src_map) in depth_maps.iter().enumerate() {
                    if src_idx == ref_idx {
                        continue;
                    }

                    let src_view = &views[src_idx];
                    let point_src = src_view.pose.world_to_camera(&point_world);
                    if point_src.z <= 0.0 {
                        continue;
                    }

                    let src_x = (src_view.intrinsics.fx * point_src.x / point_src.z
                        + src_view.intrinsics.cx) as i32;
                    let src_y = (src_view.intrinsics.fy * point_src.y / point_src.z
                        + src_view.intrinsics.cy) as i32;
                    if src_x < 0
                        || src_y < 0
                        || src_x >= src_map.width as i32
                        || src_y >= src_map.height as i32
                    {
                        continue;
                    }

                    let (src_depth, src_conf, _) = src_map.get(src_x as u32, src_y as u32).unwrap();
                    if src_conf > 0.5 {
                        let depth_diff = (src_depth as f64 - point_src.z).abs();
                        if depth_diff < consistency_threshold as f64 {
                            consistent_views += 1;
                        }
                    }
                }

                if consistent_views < min_views {
                    continue;
                }

                let normal_cam = normals_cam[idx];
                let normal_world = view.pose.rotation.transform_vector(&normal_cam);
                let color = view.rgb.get_pixel(x, y);

                oriented.push(OrientedPoint {
                    position: point_world,
                    normal: normal_world,
                    color: [color[0], color[1], color[2]],
                });
            }
        }
    }

    oriented
}

struct FusedExposure {
    rgb: RgbImage,
    rgb_array: Array3<f64>,
    luma_array: Array3<f64>,
}

fn fuse_exposures(images: &[HighResImage]) -> anyhow::Result<FusedExposure> {
    let mut rgbs = Vec::new();
    let mut weights = Vec::new();
    for img in images {
        let rgb = load_rgb_image(&img.path)?;
        let weight = exposure_weight(&rgb, img.exposure_value);
        rgbs.push(rgb);
        weights.push(weight);
    }

    let weight_sum: f64 = weights.iter().sum();
    let norm_weights: Vec<f64> = if weight_sum > 1e-6 {
        weights.iter().map(|w| w / weight_sum).collect()
    } else {
        vec![1.0 / weights.len() as f64; weights.len()]
    };

    let (width, height) = rgbs[0].dimensions();
    let mut fused = RgbImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let mut r = 0.0f64;
            let mut g = 0.0f64;
            let mut b = 0.0f64;
            for (idx, rgb) in rgbs.iter().enumerate() {
                let p = rgb.get_pixel(x, y);
                let w = norm_weights[idx];
                r += p[0] as f64 * w;
                g += p[1] as f64 * w;
                b += p[2] as f64 * w;
            }
            fused.put_pixel(
                x,
                y,
                image::Rgb([
                    r.round().clamp(0.0, 255.0) as u8,
                    g.round().clamp(0.0, 255.0) as u8,
                    b.round().clamp(0.0, 255.0) as u8,
                ]),
            );
        }
    }

    let rgb_array = rgb_to_array(&fused);
    let luma_array = rgb_to_luma_array(&fused);

    Ok(FusedExposure {
        rgb: fused,
        rgb_array,
        luma_array,
    })
}

fn exposure_weight(rgb: &RgbImage, exposure_value: Option<f32>) -> f64 {
    let (width, height) = rgb.dimensions();
    let step = 8u32.max(width / 512).max(height / 512);
    let mut sum = 0.0f64;
    let mut count = 0u64;
    let mut y = 0u32;
    while y < height {
        let mut x = 0u32;
        while x < width {
            let p = rgb.get_pixel(x, y);
            let r = p[0] as f64 / 255.0;
            let g = p[1] as f64 / 255.0;
            let b = p[2] as f64 / 255.0;
            let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            sum += luma;
            count += 1;
            x += step;
        }
        y += step;
    }
    let mean = if count > 0 { sum / count as f64 } else { 0.5 };
    let sigma = 0.25;
    let base = (-((mean - 0.5) * (mean - 0.5)) / (2.0 * sigma * sigma)).exp();
    let ev_boost = exposure_value
        .map(|ev| (-((ev as f64) * (ev as f64)) / 2.0).exp())
        .unwrap_or(1.0);
    (base * ev_boost).max(1e-4)
}

fn load_rgb_image(path: &PathBuf) -> anyhow::Result<RgbImage> {
    let img = image::open(path)
        .map_err(|e| anyhow::anyhow!("Failed to load image {}: {}", path.display(), e))?;
    Ok(img.to_rgb8())
}

fn rgb_to_array(rgb: &RgbImage) -> Array3<f64> {
    let (width, height) = rgb.dimensions();
    let mut out = Array3::<f64>::zeros((height as usize, width as usize, 3));
    for y in 0..height as usize {
        for x in 0..width as usize {
            let p = rgb.get_pixel(x as u32, y as u32);
            out[[y, x, 0]] = p[0] as f64 / 255.0;
            out[[y, x, 1]] = p[1] as f64 / 255.0;
            out[[y, x, 2]] = p[2] as f64 / 255.0;
        }
    }
    out
}

fn rgb_to_luma_array(rgb: &RgbImage) -> Array3<f64> {
    let (width, height) = rgb.dimensions();
    let mut out = Array3::<f64>::zeros((height as usize, width as usize, 1));
    for y in 0..height as usize {
        for x in 0..width as usize {
            let p = rgb.get_pixel(x as u32, y as u32);
            let r = p[0] as f64 / 255.0;
            let g = p[1] as f64 / 255.0;
            let b = p[2] as f64 / 255.0;
            out[[y, x, 0]] = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        }
    }
    out
}

fn luma_channel_to_array2(luma: &Array3<f64>) -> Array2<f64> {
    let (height, width, channels) = luma.dim();
    let channel = channels.min(1);
    let mut out = Array2::<f64>::zeros((height, width));
    for y in 0..height {
        for x in 0..width {
            out[[y, x]] = luma[[y, x, channel - 1]];
        }
    }
    out
}

fn apply_alignment_rgb(rgb: &RgbImage, alignment: &AlignmentInfo) -> RgbImage {
    let (width, height) = rgb.dimensions();
    let mut out = RgbImage::new(width, height);
    let cx = width as f64 / 2.0;
    let cy = height as f64 / 2.0;

    for y in 0..height {
        for x in 0..width {
            let src_x = ((x as f64 - cx) * alignment.scale) + cx + alignment.dx;
            let src_y = ((y as f64 - cy) * alignment.scale) + cy + alignment.dy;
            let pixel = sample_bilinear_rgb(rgb, src_x, src_y);
            out.put_pixel(x, y, pixel);
        }
    }

    out
}

fn sample_bilinear_rgb(img: &RgbImage, x: f64, y: f64) -> image::Rgb<u8> {
    let (width, height) = img.dimensions();
    if x < 0.0 || y < 0.0 || x > (width - 1) as f64 || y > (height - 1) as f64 {
        return image::Rgb([0, 0, 0]);
    }

    let x0 = x.floor() as i64;
    let y0 = y.floor() as i64;
    let x1 = (x0 + 1).min(width as i64 - 1);
    let y1 = (y0 + 1).min(height as i64 - 1);
    let wx = (x - x0 as f64).clamp(0.0, 1.0);
    let wy = (y - y0 as f64).clamp(0.0, 1.0);

    let p00 = img.get_pixel(x0 as u32, y0 as u32);
    let p10 = img.get_pixel(x1 as u32, y0 as u32);
    let p01 = img.get_pixel(x0 as u32, y1 as u32);
    let p11 = img.get_pixel(x1 as u32, y1 as u32);

    let mut out = [0u8; 3];
    for c in 0..3 {
        let v00 = p00[c] as f64;
        let v10 = p10[c] as f64;
        let v01 = p01[c] as f64;
        let v11 = p11[c] as f64;
        let v0 = v00 * (1.0 - wx) + v10 * wx;
        let v1 = v01 * (1.0 - wx) + v11 * wx;
        let v = v0 * (1.0 - wy) + v1 * wy;
        out[c] = v.round().clamp(0.0, 255.0) as u8;
    }

    image::Rgb(out)
}

fn focus_stack_output_path(group_id: &u32, source_path: &Path) -> PathBuf {
    let base_dir = source_path
        .parent()
        .map(|p| p.join("focus_stacks"))
        .unwrap_or_else(|| PathBuf::from("focus_stacks"));
    base_dir.join(format!("focus_stack_{group_id}.png"))
}

fn save_focus_stack(output_path: &Path, output: &RgbImage) -> anyhow::Result<()> {
    let base_dir = output_path
        .parent()
        .unwrap_or_else(|| Path::new("focus_stacks"));
    std::fs::create_dir_all(base_dir)?;
    let file_name = output_path
        .file_name()
        .map(|value| value.to_string_lossy())
        .unwrap_or_else(|| "focus-stack.png".into());
    let temporary =
        output_path.with_file_name(format!(".{file_name}.{}.part", uuid::Uuid::new_v4()));
    let result = (|| -> anyhow::Result<()> {
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        let mut writer = std::io::BufWriter::with_capacity(1024 * 1024, file);
        image::codecs::png::PngEncoder::new(&mut writer).write_image(
            output.as_raw(),
            output.width(),
            output.height(),
            image::ExtendedColorType::Rgb8,
        )?;
        use std::io::Write;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        std::fs::rename(&temporary, output_path)?;
        #[cfg(unix)]
        std::fs::File::open(base_dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Extension trait to add images with pose priors
trait SfmPipelineExt {
    fn add_image_with_prior(&mut self, image: ImageData, prior: CameraPose) -> anyhow::Result<()>;
}

impl SfmPipelineExt for SfmPipeline {
    fn add_image_with_prior(
        &mut self,
        mut image: ImageData,
        prior: CameraPose,
    ) -> anyhow::Result<()> {
        image.prior_pose = Some(prior);
        self.add_image(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_camera_id() {
        let webcam = CameraId::webcam(0);
        assert_eq!(webcam.0, "webcam_0");

        let dslr = CameraId::dslr("nikon_d850");
        assert_eq!(dslr.0, "dslr_nikon_d850");
    }

    #[test]
    fn test_multicam_config_default() {
        let config = MultiCamConfig::default();
        assert_eq!(config.num_webcams, 1);
        assert!(config.enable_dense);
        assert!(config.persist_focus_stacks);
    }

    #[test]
    fn test_multicam_sfm_creation() {
        let config = MultiCamConfig {
            num_webcams: 10,
            ..Default::default()
        };

        let sfm = MultiCamSfm::new(config);
        assert_eq!(sfm.livescan_pose_count(), 0);
    }

    #[test]
    fn test_register_camera() {
        let mut sfm = MultiCamSfm::new(MultiCamConfig::default());

        let cam_id = CameraId::webcam(0);
        let intrinsics = CameraIntrinsics {
            fx: 500.0,
            fy: 500.0,
            cx: 320.0,
            cy: 240.0,
            width: 640,
            height: 480,
            distortion: vec![],
            distortion_model: Default::default(),
        };

        sfm.register_camera(cam_id.clone(), intrinsics);
        assert!(sfm.camera_intrinsics.contains_key(&cam_id));
    }

    #[test]
    fn focus_stack_paths_are_deterministic() {
        let source = PathBuf::from("/capture/angle/frame.nef");
        assert_eq!(
            focus_stack_output_path(&7, &source),
            PathBuf::from("/capture/angle/focus_stacks/focus_stack_7.png")
        );
    }
}
