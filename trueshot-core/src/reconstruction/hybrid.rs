//! Hybrid 3DGS + Photogrammetry Pipeline
//!
//! State-of-the-art pipeline that combines the best of both worlds:
//! - Real-time processing DURING scanning (low-res webcam)
//! - High-quality refinement AFTER scanning (DSLR SD card)
//!
//! This is TrueShot's flagship reconstruction method.

use anyhow::Result;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use exif::{In, Tag, Value};
use nalgebra as na;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::gaussian_splatting::{
    Camera as GSCamera, GaussianCloud, GaussianSplatTrainer, TrainingConfig,
};

/// Hybrid pipeline phases
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PipelinePhase {
    /// Phase 1: Live scanning with real-time processing
    LiveScanning,
    /// Phase 2: SD card ingestion (high-res images)
    HighResIngestion,
    /// Phase 3: Quality refinement
    Refinement,
    /// Phase 4: Final export
    Export,
}

/// Hybrid reconstruction configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HybridConfig {
    /// Workspace directory
    pub workspace_path: PathBuf,
    /// Quality preset
    pub quality: HybridQuality,
    /// Enable real-time preview during scanning
    pub enable_live_preview: bool,
    /// Number of cameras in rig (multi-view support)
    pub num_cameras: usize,
    /// Use Mip-Splatting for anti-aliasing
    pub use_mip_splatting: bool,
    /// Use ASG for specular surfaces
    pub use_spec_gaussian: bool,
    /// Maximum Gaussians (memory limit)
    pub max_gaussians: usize,
}

/// Quality presets for hybrid pipeline
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HybridQuality {
    /// Fast preview during scanning
    Preview,
    /// Balanced quality/speed for standard use
    Standard,
    /// Maximum quality for production
    Production,
}

impl Default for HybridConfig {
    fn default() -> Self {
        Self {
            workspace_path: PathBuf::from("./workspace"),
            quality: HybridQuality::Standard,
            enable_live_preview: true,
            num_cameras: 1,
            use_mip_splatting: true,
            use_spec_gaussian: true,
            max_gaussians: 500_000,
        }
    }
}

/// Real-time frame data from scanning
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LiveFrame {
    /// Camera pose (world-to-camera) as flattened 4x4 matrix
    pub pose: [[f32; 4]; 4],
    /// Camera intrinsics as flattened 3x3 matrix
    pub intrinsics: [[f32; 3]; 3],
    /// Image dimensions
    pub width: u32,
    pub height: u32,
    /// Turntable angle
    pub turntable_angle: f32,
    /// Camera index (for multi-view rigs)
    pub camera_index: usize,
    /// Path to image file
    pub image_path: PathBuf,
    /// Timestamp
    pub timestamp: f64,
}

impl LiveFrame {
    pub fn pose_matrix(&self) -> na::Matrix4<f32> {
        na::Matrix4::from_fn(|r, c| self.pose[r][c])
    }

    pub fn intrinsics_matrix(&self) -> na::Matrix3<f32> {
        na::Matrix3::from_fn(|r, c| self.intrinsics[r][c])
    }
}

/// High-resolution image from SD card
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HighResImage {
    /// Matched to a LiveFrame via timestamp
    pub matched_live_frame_idx: Option<usize>,
    /// Camera index in rig
    pub camera_index: usize,
    /// Path to high-res image
    pub image_path: PathBuf,
    /// EXIF timestamp
    pub timestamp: f64,
    /// Turntable angle (from session log)
    pub turntable_angle: f32,
}

/// Sparse 3D point from triangulation
#[derive(Debug, Clone)]
pub struct SparsePoint3D {
    pub position: na::Point3<f32>,
    pub color: [u8; 3],
    pub observations: usize,
}

/// Real-time reconstruction state (updated during scanning)
pub struct LiveReconState {
    /// Accumulated 3D points from feature triangulation
    pub sparse_points: Vec<SparsePoint3D>,
    /// Camera poses
    pub poses: Vec<na::Matrix4<f32>>,
    /// Current Gaussian cloud (preview quality)
    pub preview_gaussians: Option<GaussianCloud>,
    /// Frame count processed
    pub frames_processed: usize,
}

impl LiveReconState {
    pub fn new() -> Self {
        Self {
            sparse_points: Vec::new(),
            poses: Vec::new(),
            preview_gaussians: None,
            frames_processed: 0,
        }
    }
}

impl Default for LiveReconState {
    fn default() -> Self {
        Self::new()
    }
}

/// The main Hybrid Pipeline
///
/// Architecture:
/// ```text
/// ┌─────────────────────────────────────────────────────────────────────┐
/// │                        SCANNING PHASE                               │
/// │  ┌──────────┐    ┌─────────────┐    ┌─────────────────────────────┐ │
/// │  │ Webcam   │───▶│ Feature     │───▶│ Incremental SfM             │ │
/// │  │ Frames   │    │ Detection   │    │ (FAST + BRIEF + MAGSAC++)   │ │
/// │  └──────────┘    └─────────────┘    └─────────────────────────────┘ │
/// │                         │                       │                   │
/// │                         ▼                       ▼                   │
/// │              ┌──────────────────────────────────────────────┐       │
/// │              │ Preview 3DGS (low-res, fast training)        │       │
/// │              └──────────────────────────────────────────────┘       │
/// └─────────────────────────────────────────────────────────────────────┘
///                              │
///                              ▼
/// ┌─────────────────────────────────────────────────────────────────────┐
/// │                      SD CARD INGESTION                              │
/// │  ┌──────────┐    ┌─────────────┐    ┌─────────────────────────────┐ │
/// │  │ DSLR     │───▶│ Timestamp   │───▶│ Pose Refinement             │ │
/// │  │ Images   │    │ Matching    │    │ (Bundle Adjustment)         │ │
/// │  └──────────┘    └─────────────┘    └─────────────────────────────┘ │
/// └─────────────────────────────────────────────────────────────────────┘
///                              │
///                              ▼
/// ┌─────────────────────────────────────────────────────────────────────┐
/// │                      REFINEMENT PHASE                               │
/// │  ┌─────────────────────────────────────────────────────────────────┐│
/// │  │ High-Quality 3DGS Training                                      ││
/// │  │ - Initialize from preview Gaussians                             ││
/// │  │ - Add Mip-Splatting for anti-aliasing                           ││
/// │  │ - Add Spec-Gaussian for reflections                             ││
/// │  │ - Full 30K iterations with densification                        ││
/// │  └─────────────────────────────────────────────────────────────────┘│
/// │                              │                                      │
/// │                              ▼                                      │
/// │  ┌─────────────────────────────────────────────────────────────────┐│
/// │  │ Mesh Extraction (GS2Mesh approach)                              ││
/// │  │ - Render stereo pairs from trained 3DGS                         ││
/// │  │ - Dense depth estimation via stereo matching                    ││
/// │  │ - Poisson surface reconstruction                                ││
/// │  └─────────────────────────────────────────────────────────────────┘│
/// └─────────────────────────────────────────────────────────────────────┘
/// ```
pub struct HybridPipeline {
    config: HybridConfig,
    phase: PipelinePhase,
    state: Arc<RwLock<LiveReconState>>,
    live_frames: Vec<LiveFrame>,
    high_res_images: Vec<HighResImage>,
}

impl HybridPipeline {
    /// Create new hybrid pipeline
    pub fn new(config: HybridConfig) -> Self {
        Self {
            config,
            phase: PipelinePhase::LiveScanning,
            state: Arc::new(RwLock::new(LiveReconState::new())),
            live_frames: Vec::new(),
            high_res_images: Vec::new(),
        }
    }

    /// Get current phase
    pub fn phase(&self) -> PipelinePhase {
        self.phase
    }

    /// Get shared state for real-time preview
    pub fn state(&self) -> Arc<RwLock<LiveReconState>> {
        self.state.clone()
    }

    /// Get number of live frames
    pub fn live_frame_count(&self) -> usize {
        self.live_frames.len()
    }

    /// Get number of high-res images
    pub fn high_res_image_count(&self) -> usize {
        self.high_res_images.len()
    }

    // =========================================================================
    // PHASE 1: LIVE SCANNING
    // =========================================================================

    /// Add a live frame with pre-extracted 3D points
    /// This is called by the scanning system which handles feature extraction
    pub async fn add_live_frame_with_points(
        &mut self,
        frame: LiveFrame,
        new_points: Vec<SparsePoint3D>,
    ) -> Result<()> {
        let mut state = self.state.write().await;

        // Store pose
        state.poses.push(frame.pose_matrix());

        // Add new 3D points
        state.sparse_points.extend(new_points);
        state.frames_processed += 1;

        drop(state);

        // Store frame
        self.live_frames.push(frame);

        // Update preview 3DGS every N frames
        if self.live_frames.len() % 20 == 0 && self.config.enable_live_preview {
            self.update_preview_gaussians().await?;
        }

        Ok(())
    }

    /// Update preview Gaussian cloud from sparse points
    async fn update_preview_gaussians(&mut self) -> Result<()> {
        let mut state = self.state.write().await;

        if state.sparse_points.is_empty() {
            return Ok(());
        }

        // Create preview Gaussians from sparse points
        let points: Vec<(na::Point3<f32>, [u8; 3])> = state
            .sparse_points
            .iter()
            .map(|p| (p.position, p.color))
            .collect();

        let cloud = GaussianCloud::from_points(&points);

        tracing::info!(
            "📊 Preview: {} Gaussians from {} points",
            cloud.num_gaussians(),
            state.sparse_points.len()
        );

        state.preview_gaussians = Some(cloud);

        Ok(())
    }

    /// Notify scanning is complete
    pub fn complete_scanning(&mut self) {
        self.phase = PipelinePhase::HighResIngestion;
        tracing::info!(
            "✅ Scanning complete: {} frames, moving to SD card ingestion",
            self.live_frames.len()
        );
    }

    // =========================================================================
    // PHASE 2: SD CARD INGESTION
    // =========================================================================

    /// Ingest high-resolution images from SD card
    pub async fn ingest_sd_card(&mut self, sd_path: PathBuf) -> Result<()> {
        tracing::info!("📸 Ingesting SD card from: {:?}", sd_path);

        // Find all images
        let image_extensions = ["jpg", "jpeg", "png", "tif", "tiff", "nef", "cr2", "arw"];
        let mut images = Vec::new();

        for entry in walkdir::WalkDir::new(&sd_path)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if image_extensions.contains(&ext.to_str().unwrap_or("").to_lowercase().as_str()) {
                    images.push(path.to_path_buf());
                }
            }
        }

        tracing::info!("Found {} images on SD card", images.len());

        // Match to live frames by timestamp
        for image_path in images {
            // Extract EXIF timestamp (simplified)
            let timestamp = self.extract_timestamp(&image_path)?;

            // Find closest live frame
            let matched_idx = self.find_closest_live_frame(timestamp);

            // Determine camera index from filename pattern
            let camera_index = self.detect_camera_index(&image_path);

            let high_res = HighResImage {
                matched_live_frame_idx: matched_idx,
                camera_index,
                image_path: image_path.clone(),
                timestamp,
                turntable_angle: matched_idx
                    .map(|i| self.live_frames[i].turntable_angle)
                    .unwrap_or(0.0),
            };

            self.high_res_images.push(high_res);
        }

        tracing::info!(
            "📸 Matched {} high-res images to live frames",
            self.high_res_images
                .iter()
                .filter(|i| i.matched_live_frame_idx.is_some())
                .count()
        );

        self.phase = PipelinePhase::Refinement;
        Ok(())
    }

    fn extract_timestamp(&self, path: &PathBuf) -> Result<f64> {
        if let Some(ts) = Self::read_exif_timestamp(path) {
            return Ok(ts);
        }
        if let Some(ts) = Self::read_filesystem_timestamp(path) {
            return Ok(ts);
        }
        if let Some(ts) = Self::read_filename_timestamp(path) {
            return Ok(ts);
        }
        anyhow::bail!("No timestamp available for {}", path.display())
    }

    fn find_closest_live_frame(&self, timestamp: f64) -> Option<usize> {
        let mut best_idx = None;
        let mut best_diff = f64::MAX;

        for (idx, frame) in self.live_frames.iter().enumerate() {
            let diff = (frame.timestamp - timestamp).abs();
            if diff < best_diff && diff < 5.0 {
                // Within 5 seconds
                best_diff = diff;
                best_idx = Some(idx);
            }
        }

        best_idx
    }

    fn detect_camera_index(&self, path: &PathBuf) -> usize {
        // Detect camera index from filename (e.g., CAM1_0001.jpg -> 0)
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if filename.contains("CAM2") || filename.contains("cam2") || filename.contains("_2_") {
            1
        } else if filename.contains("CAM3") || filename.contains("cam3") || filename.contains("_3_")
        {
            2
        } else {
            0 // Default to first camera
        }
    }

    fn read_exif_timestamp(path: &PathBuf) -> Option<f64> {
        let file = File::open(path).ok()?;
        let mut bufreader = BufReader::new(file);
        let exif = exif::Reader::new()
            .read_from_container(&mut bufreader)
            .ok()?;
        let candidates = [Tag::DateTimeOriginal, Tag::DateTimeDigitized, Tag::DateTime];
        for tag in candidates {
            if let Some(field) = exif.get_field(tag, In::PRIMARY) {
                if let Value::Ascii(ref vec) = field.value {
                    if let Some(raw) = vec.first().and_then(|b| std::str::from_utf8(b).ok()) {
                        if let Some(dt) = Self::parse_exif_datetime(raw.trim()) {
                            return Some(
                                dt.timestamp() as f64
                                    + dt.timestamp_subsec_millis() as f64 / 1000.0,
                            );
                        }
                    }
                }
            }
        }
        None
    }

    fn read_filesystem_timestamp(path: &PathBuf) -> Option<f64> {
        let meta = std::fs::metadata(path).ok()?;
        let time = meta.created().or_else(|_| meta.modified()).ok()?;
        let dt: DateTime<Utc> = time.into();
        Some(dt.timestamp() as f64 + dt.timestamp_subsec_millis() as f64 / 1000.0)
    }

    fn read_filename_timestamp(path: &PathBuf) -> Option<f64> {
        let name = path.file_stem().and_then(|n| n.to_str())?;
        // Look for 14 consecutive digits: YYYYMMDDHHMMSS
        let digits: String = name.chars().filter(|c| c.is_ascii_digit()).collect();
        for i in 0..digits.len().saturating_sub(13) {
            let window = &digits[i..i + 14];
            if let Some(dt) = Self::parse_compact_timestamp(window) {
                return Some(dt.timestamp() as f64 + dt.timestamp_subsec_millis() as f64 / 1000.0);
            }
        }
        None
    }

    fn parse_exif_datetime(raw: &str) -> Option<DateTime<Utc>> {
        let normalized = raw.replace(':', "-");
        let normalized = normalized.replace('T', " ");
        let fmt = "%Y-%m-%d %H-%M-%S";
        NaiveDateTime::parse_from_str(&normalized, fmt)
            .ok()
            .map(|naive| DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
    }

    fn parse_compact_timestamp(raw: &str) -> Option<DateTime<Utc>> {
        if raw.len() != 14 || !raw.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let year: i32 = raw[0..4].parse().ok()?;
        let month: u32 = raw[4..6].parse().ok()?;
        let day: u32 = raw[6..8].parse().ok()?;
        let hour: u32 = raw[8..10].parse().ok()?;
        let minute: u32 = raw[10..12].parse().ok()?;
        let second: u32 = raw[12..14].parse().ok()?;
        let date = NaiveDate::from_ymd_opt(year, month, day)?;
        let time = NaiveTime::from_hms_opt(hour, minute, second)?;
        let naive = NaiveDateTime::new(date, time);
        Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
    }

    // =========================================================================
    // PHASE 3: REFINEMENT
    // =========================================================================

    /// Run full quality refinement
    pub async fn run_refinement(&mut self) -> Result<()> {
        tracing::info!("🔥 Starting high-quality refinement...");

        // 1. Build cameras from high-res images
        let cameras = self.build_cameras_from_high_res()?;

        // 2. Get initial points from live scan
        let state = self.state.read().await;
        let initial_points: Vec<(na::Point3<f32>, [u8; 3])> = state
            .sparse_points
            .iter()
            .map(|p| (p.position, p.color))
            .collect();
        drop(state);

        // 3. Create training config based on quality
        let config = match self.config.quality {
            HybridQuality::Preview => TrainingConfig {
                iterations: 3000,
                use_gpu_gradients: true,
                gpu_parity_check: true,
                ..Default::default()
            },
            HybridQuality::Standard => TrainingConfig {
                iterations: 15000,
                use_gpu_gradients: true,
                gpu_parity_check: true,
                ..Default::default()
            },
            HybridQuality::Production => TrainingConfig {
                iterations: 30000,
                densify_interval: 50,
                use_gpu_gradients: true,
                gpu_parity_check: true,
                ..Default::default()
            },
        };

        // 4. Train 3DGS
        tracing::info!(
            "🎯 Training 3DGS with {} initial points, {} cameras",
            initial_points.len(),
            cameras.len()
        );

        let mut trainer = GaussianSplatTrainer::new(&initial_points, cameras, config.clone());

        for i in 0..config.iterations {
            let loss = trainer.step()?;

            if i % 1000 == 0 {
                tracing::info!(
                    "  Iteration {}/{}: loss={:.4}, gaussians={}",
                    i,
                    config.iterations,
                    loss,
                    trainer.num_gaussians()
                );
            }
        }

        // 5. Export results
        let output_dir = self.config.workspace_path.join("output");
        std::fs::create_dir_all(&output_dir)?;

        let ply_path = output_dir.join("point_cloud.ply");
        trainer.export_ply(&ply_path)?;
        let splat_path = output_dir.join("model.splat");
        trainer.export_splat(&splat_path)?;
        let spz_path = output_dir.join("model.spz");
        trainer.export_spz(&spz_path)?;

        tracing::info!(
            "✅ 3DGS training complete: {} Gaussians",
            trainer.num_gaussians()
        );
        tracing::info!("📁 Output saved to: {:?}", output_dir);

        self.phase = PipelinePhase::Export;
        Ok(())
    }

    fn build_cameras_from_high_res(&self) -> Result<Vec<GSCamera>> {
        let mut cameras = Vec::new();

        for high_res in &self.high_res_images {
            if let Some(live_idx) = high_res.matched_live_frame_idx {
                let live_frame = &self.live_frames[live_idx];

                cameras.push(GSCamera {
                    transform: live_frame.pose_matrix(),
                    intrinsics: live_frame.intrinsics_matrix(),
                    width: 4000, // Assume high-res
                    height: 3000,
                    image_path: high_res.image_path.clone(),
                });
            }
        }

        Ok(cameras)
    }

    // =========================================================================
    // PHASE 4: EXPORT
    // =========================================================================

    /// Export final results
    pub async fn export(&self) -> Result<ExportResult> {
        let output_dir = self.config.workspace_path.join("output");

        Ok(ExportResult {
            ply_path: output_dir.join("point_cloud.ply"),
            splat_path: output_dir.join("model.splat"),
            spz_path: output_dir.join("model.spz"),
            mesh_path: output_dir.join("mesh.obj"),
            texture_path: output_dir.join("texture.png"),
        })
    }

    /// Get pipeline progress (0.0 - 1.0)
    pub async fn progress(&self) -> f32 {
        let state = self.state.read().await;

        match self.phase {
            PipelinePhase::LiveScanning => {
                // Estimate based on expected frame count
                (state.frames_processed as f32 / 100.0).min(0.25)
            }
            PipelinePhase::HighResIngestion => 0.30,
            PipelinePhase::Refinement => 0.50, // Would need iteration tracking
            PipelinePhase::Export => 1.0,
        }
    }
}

/// Export result paths
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExportResult {
    pub ply_path: PathBuf,
    pub splat_path: PathBuf,
    pub spz_path: PathBuf,
    pub mesh_path: PathBuf,
    pub texture_path: PathBuf,
}

/// Multi-camera rig configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MultiCameraRig {
    /// Number of cameras in the rig
    pub num_cameras: usize,
    /// Relative transforms from primary camera to each secondary camera
    pub camera_transforms: Vec<[[f32; 4]; 4]>,
    /// Camera intrinsics for each camera
    pub camera_intrinsics: Vec<[[f32; 3]; 3]>,
}

impl MultiCameraRig {
    /// Create a single-camera "rig"
    pub fn single_camera() -> Self {
        Self {
            num_cameras: 1,
            camera_transforms: vec![[
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ]],
            camera_intrinsics: vec![[[500.0, 0.0, 320.0], [0.0, 500.0, 240.0], [0.0, 0.0, 1.0]]],
        }
    }

    /// Get transform for camera at index
    pub fn camera_transform(&self, index: usize) -> na::Matrix4<f32> {
        if index < self.camera_transforms.len() {
            na::Matrix4::from_fn(|r, c| self.camera_transforms[index][r][c])
        } else {
            na::Matrix4::identity()
        }
    }
}
