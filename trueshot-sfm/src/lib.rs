//! TrueShot SfM - Native Rust Structure from Motion
//!
//! Complete 3D reconstruction pipeline without external dependencies.
//! Replaces COLMAP with pure Rust implementation.
//!
//! # Architecture
//!
//! ```text
//! Images → Features → Matching → Geometry → Bundle Adjustment → Dense → Mesh
//! ```
//!
//! # Features
//!
//! - **Native SIFT/ORB**: Feature detection without OpenCV
//! - **Bundle Adjustment**: Levenberg-Marquardt optimization via argmin
//! - **Dense Reconstruction**: Multi-view stereo depth estimation
//! - **Mesh Generation**: Poisson surface reconstruction
//!
//! # Example
//!
//! ```rust,ignore
//! use trueshot_sfm::{SfmPipeline, SfmConfig};
//!
//! let mut pipeline = SfmPipeline::new(SfmConfig::default());
//! pipeline.add_images(&["img1.jpg", "img2.jpg", "img3.jpg"])?;
//! 
//! let reconstruction = pipeline.run()?;
//! reconstruction.export_ply("output.ply")?;
//! ```

pub mod features;
pub mod geometry;
pub mod optimization;
pub mod dense;
pub mod mesh;
pub mod distortion;

use std::path::{Path, PathBuf};
use nalgebra as na;
use serde::{Deserialize, Serialize};

// ============================================================================
// Core Types
// ============================================================================

/// 3D point with color
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Point3D {
    pub position: na::Point3<f64>,
    pub color: [u8; 3],
    pub error: f64,
    pub track: Vec<(usize, usize)>, // (image_id, keypoint_id)
}

/// Camera intrinsics
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CameraIntrinsics {
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub width: u32,
    pub height: u32,
    pub distortion: Vec<f64>,
    #[serde(default)]
    pub distortion_model: DistortionModel,
}

impl CameraIntrinsics {
    pub fn to_matrix(&self) -> na::Matrix3<f64> {
        na::Matrix3::new(
            self.fx, 0.0, self.cx,
            0.0, self.fy, self.cy,
            0.0, 0.0, 1.0,
        )
    }
}

/// Distortion model for camera intrinsics
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistortionModel {
    None,
    BrownConrady,
    Fisheye,
}

impl Default for DistortionModel {
    fn default() -> Self {
        DistortionModel::None
    }
}

/// Rolling shutter readout direction.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollingShutterDirection {
    TopToBottom,
    BottomToTop,
    LeftToRight,
    RightToLeft,
}

impl Default for RollingShutterDirection {
    fn default() -> Self {
        RollingShutterDirection::TopToBottom
    }
}

/// Rolling shutter model with line readout time in milliseconds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RollingShutterModel {
    pub readout_time_ms: f64,
    #[serde(default)]
    pub direction: RollingShutterDirection,
}

impl RollingShutterModel {
    /// Compute time offset (seconds) relative to mid-exposure for a given pixel coordinate.
    pub fn time_offset_seconds(&self, x: f64, y: f64, width: u32, height: u32) -> f64 {
        let readout = (self.readout_time_ms / 1000.0).max(0.0);
        if readout <= 0.0 || width == 0 || height == 0 {
            return 0.0;
        }
        let norm = match self.direction {
            RollingShutterDirection::TopToBottom | RollingShutterDirection::BottomToTop => {
                if height > 1 { y / (height as f64 - 1.0) } else { 0.0 }
            }
            RollingShutterDirection::LeftToRight | RollingShutterDirection::RightToLeft => {
                if width > 1 { x / (width as f64 - 1.0) } else { 0.0 }
            }
        };
        let centered = norm - 0.5;
        let signed = match self.direction {
            RollingShutterDirection::BottomToTop | RollingShutterDirection::RightToLeft => -centered,
            _ => centered,
        };
        signed * readout
    }
}

/// Camera motion prior for rolling-shutter compensation (world frame).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CameraMotion {
    /// Angular velocity (rad/s) in world frame.
    pub angular_velocity: na::Vector3<f64>,
    /// Linear velocity (units/s) in world frame.
    pub linear_velocity: na::Vector3<f64>,
    /// Optional timestamp (ms).
    #[serde(default)]
    pub timestamp_ms: Option<u64>,
}

/// Camera pose (extrinsics), stored as camera-to-world transform.
/// `rotation` maps camera coordinates into world coordinates.
/// `translation` is the camera origin in world coordinates.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CameraPose {
    pub rotation: na::UnitQuaternion<f64>,
    pub translation: na::Vector3<f64>,
}

impl CameraPose {
    pub fn identity() -> Self {
        Self {
            rotation: na::UnitQuaternion::identity(),
            translation: na::Vector3::zeros(),
        }
    }
    
    pub fn to_matrix(&self) -> na::Matrix4<f64> {
        let r = self.rotation.to_rotation_matrix();
        let mut m = na::Matrix4::identity();
        m.fixed_view_mut::<3, 3>(0, 0).copy_from(r.matrix());
        m.fixed_view_mut::<3, 1>(0, 3).copy_from(&self.translation);
        m
    }

    pub fn camera_to_world(&self, point_cam: &na::Point3<f64>) -> na::Point3<f64> {
        na::Point3::from(self.rotation * point_cam.coords + self.translation)
    }

    pub fn world_to_camera(&self, point_world: &na::Point3<f64>) -> na::Point3<f64> {
        na::Point3::from(self.rotation.inverse() * (point_world.coords - self.translation))
    }

    pub fn world_to_camera_rotation(&self) -> na::Rotation3<f64> {
        self.rotation.inverse().to_rotation_matrix()
    }

    pub fn world_to_camera_translation(&self) -> na::Vector3<f64> {
        -(self.rotation.inverse() * self.translation)
    }
}

/// Image with pose and features
#[derive(Clone, Debug)]
pub struct ImageData {
    pub id: usize,
    pub path: PathBuf,
    pub intrinsics: CameraIntrinsics,
    pub pose: Option<CameraPose>,
    pub prior_pose: Option<CameraPose>,  // Prior from livescan
    pub keypoints: Vec<features::Keypoint>,
    pub descriptors: Vec<features::Descriptor>,
    pub rolling_shutter: Option<RollingShutterModel>,
    pub camera_motion: Option<CameraMotion>,
}

impl ImageData {
    /// Create ImageData from path with intrinsics
    pub fn from_path(path: &Path, intrinsics: &CameraIntrinsics) -> anyhow::Result<Self> {
        Self::from_path_with_limit(path, intrinsics, 8000)
    }

    pub fn from_path_with_limit(
        path: &Path,
        intrinsics: &CameraIntrinsics,
        max_features: usize,
    ) -> anyhow::Result<Self> {
        let img = image::open(path)?;
        let (keypoints, descriptors) = features::detect_sift(&img, max_features);
        
        Ok(Self {
            id: 0,
            path: path.to_path_buf(),
            intrinsics: intrinsics.clone(),
            pose: None,
            prior_pose: None,
            keypoints,
            descriptors,
            rolling_shutter: None,
            camera_motion: None,
        })
    }

    /// Create feature data from an in-memory RGB view while preserving the
    /// intended durable path as the view identifier.
    pub fn from_rgb_image(
        path: &Path,
        image: &image::RgbImage,
        intrinsics: &CameraIntrinsics,
        max_features: usize,
    ) -> anyhow::Result<Self> {
        let gray = image::imageops::grayscale(image);
        let (keypoints, descriptors) = features::detect_sift_gray(&gray, max_features);
        Ok(Self {
            id: 0,
            path: path.to_path_buf(),
            intrinsics: intrinsics.clone(),
            pose: None,
            prior_pose: None,
            keypoints,
            descriptors,
            rolling_shutter: None,
            camera_motion: None,
        })
    }
}

/// Sparse reconstruction result
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SparseReconstruction {
    pub points: Vec<Point3D>,
    pub cameras: Vec<CameraIntrinsics>,
    pub poses: Vec<CameraPose>,
    pub image_names: Vec<String>,
}

impl SparseReconstruction {
    /// Export to PLY format
    pub fn export_ply(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        use std::io::Write;
        
        let path = path.as_ref();
        let mut file = std::fs::File::create(path)?;
        
        // PLY header
        writeln!(file, "ply")?;
        writeln!(file, "format ascii 1.0")?;
        writeln!(file, "element vertex {}", self.points.len())?;
        writeln!(file, "property float x")?;
        writeln!(file, "property float y")?;
        writeln!(file, "property float z")?;
        writeln!(file, "property uchar red")?;
        writeln!(file, "property uchar green")?;
        writeln!(file, "property uchar blue")?;
        writeln!(file, "end_header")?;
        
        // Points
        for p in &self.points {
            writeln!(file, "{} {} {} {} {} {}",
                p.position.x, p.position.y, p.position.z,
                p.color[0], p.color[1], p.color[2])?;
        }
        
        Ok(())
    }
}

// ============================================================================
// Pipeline Configuration
// ============================================================================

/// SfM pipeline configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SfmConfig {
    /// Feature detector type
    pub feature_type: FeatureType,
    /// Maximum features per image
    pub max_features: usize,
    /// Matching ratio threshold (Lowe's ratio test)
    pub match_ratio: f32,
    /// Minimum matches for valid pair
    pub min_matches: usize,
    /// Bundle adjustment iterations
    pub ba_iterations: usize,
    /// Local bundle adjustment window size
    #[serde(default)]
    pub local_ba_window: usize,
    /// Local bundle adjustment stride
    #[serde(default)]
    pub local_ba_stride: usize,
    /// Local bundle adjustment iterations
    #[serde(default)]
    pub local_ba_iterations: usize,
    /// Minimum points to run local BA
    #[serde(default)]
    pub local_ba_min_points: usize,
    /// Minimum RMSE to trigger local BA
    #[serde(default)]
    pub local_ba_min_rmse: f64,
    /// Enable dense reconstruction
    pub enable_dense: bool,
    /// Number of parallel threads
    pub num_threads: usize,
}

impl Default for SfmConfig {
    fn default() -> Self {
        Self {
            feature_type: FeatureType::Orb,
            max_features: 8000,
            match_ratio: 0.75,
            min_matches: 30,
            ba_iterations: 50,
            local_ba_window: 6,
            local_ba_stride: 2,
            local_ba_iterations: 25,
            local_ba_min_points: 200,
            local_ba_min_rmse: 0.8,
            enable_dense: true,
            num_threads: num_cpus::get(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum FeatureType {
    /// ORB features (fast, good for real-time)
    Orb,
    /// SIFT-like features (slower, more accurate)
    Sift,
    /// AKAZE features (good balance)
    Akaze,
}

// ============================================================================
// Pipeline
// ============================================================================

/// Main SfM pipeline
pub struct SfmPipeline {
    config: SfmConfig,
    images: Vec<ImageData>,
    reconstruction: Option<SparseReconstruction>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReprojectionStats {
    pub points: usize,
    pub observations: usize,
    pub invalid_observations: usize,
    pub mean_error_px: f64,
    pub median_error_px: f64,
    pub p90_error_px: f64,
    pub max_error_px: f64,
    pub mean_track_len: f64,
    pub median_track_len: f64,
    pub min_track_len: usize,
    pub max_track_len: usize,
    pub points_with_2plus: usize,
    pub points_with_3plus: usize,
}

impl SfmPipeline {
    pub fn new(config: SfmConfig) -> Self {
        Self {
            config,
            images: Vec::new(),
            reconstruction: None,
        }
    }
    
    /// Add images to the pipeline
    pub fn add_images<P: AsRef<Path>>(&mut self, paths: &[P]) -> anyhow::Result<()> {
        for (id, path) in paths.iter().enumerate() {
            let path = path.as_ref();
            let img = image::open(path)?;
            
            // Extract features
            let (keypoints, descriptors) = match self.config.feature_type {
                FeatureType::Orb => features::detect_orb(&img, self.config.max_features),
                FeatureType::Sift => features::detect_sift(&img, self.config.max_features),
                FeatureType::Akaze => features::detect_orb(&img, self.config.max_features), // Fallback
            };
            
            // Estimate intrinsics from image size
            let w = img.width();
            let h = img.height();
            let focal = (w.max(h) as f64) * 1.2; // Heuristic
            
            let intrinsics = CameraIntrinsics {
                fx: focal,
                fy: focal,
                cx: w as f64 / 2.0,
                cy: h as f64 / 2.0,
                width: w,
                height: h,
                distortion: vec![],
                distortion_model: DistortionModel::None,
            };
            
            self.images.push(ImageData {
                id,
                path: path.to_path_buf(),
                intrinsics,
                pose: None,
                prior_pose: None,
                keypoints,
                descriptors,
                rolling_shutter: None,
                camera_motion: None,
            });
            
            tracing::info!("Added image {} with {} features", path.display(), self.images.last().unwrap().keypoints.len());
        }
        
        Ok(())
    }

    /// Add a single image with explicit intrinsics and optional pose prior
    pub fn add_image_with_intrinsics<P: AsRef<Path>>(
        &mut self,
        path: P,
        intrinsics: CameraIntrinsics,
        prior_pose: Option<CameraPose>,
    ) -> anyhow::Result<()> {
        let path = path.as_ref();
        let img = image::open(path)?;
        
        let (keypoints, descriptors) = match self.config.feature_type {
            FeatureType::Orb => features::detect_orb(&img, self.config.max_features),
            FeatureType::Sift => features::detect_sift(&img, self.config.max_features),
            FeatureType::Akaze => features::detect_orb(&img, self.config.max_features),
        };
        
        let id = self.images.len();
        self.images.push(ImageData {
            id,
            path: path.to_path_buf(),
            intrinsics,
            pose: None,
            prior_pose,
            keypoints,
            descriptors,
            rolling_shutter: None,
            camera_motion: None,
        });
        
        tracing::info!("Added image {} with {} features", path.display(), self.images.last().unwrap().keypoints.len());
        Ok(())
    }

    /// Add a single image with explicit intrinsics, optional pose prior, and temporal metadata.
    pub fn add_image_with_context<P: AsRef<Path>>(
        &mut self,
        path: P,
        intrinsics: CameraIntrinsics,
        prior_pose: Option<CameraPose>,
        rolling_shutter: Option<RollingShutterModel>,
        camera_motion: Option<CameraMotion>,
    ) -> anyhow::Result<()> {
        let path = path.as_ref();
        let img = image::open(path)?;

        let (keypoints, descriptors) = match self.config.feature_type {
            FeatureType::Orb => features::detect_orb(&img, self.config.max_features),
            FeatureType::Sift => features::detect_sift(&img, self.config.max_features),
            FeatureType::Akaze => features::detect_orb(&img, self.config.max_features),
        };

        let id = self.images.len();
        self.images.push(ImageData {
            id,
            path: path.to_path_buf(),
            intrinsics,
            pose: None,
            prior_pose,
            keypoints,
            descriptors,
            rolling_shutter,
            camera_motion,
        });

        tracing::info!("Added image {} with {} features", path.display(), self.images.last().unwrap().keypoints.len());
        Ok(())
    }
    
    /// Add a single pre-processed image
    pub fn add_image(&mut self, mut image: ImageData) -> anyhow::Result<()> {
        image.id = self.images.len();
        self.images.push(image);
        Ok(())
    }
    
    /// Run the full SfM pipeline
    pub fn run(&mut self) -> anyhow::Result<SparseReconstruction> {
        tracing::info!("🚀 Starting SfM pipeline with {} images", self.images.len());
        
        if self.images.len() < 2 {
            anyhow::bail!("Need at least 2 images for reconstruction");
        }
        
        // 1. Match features between all image pairs
        tracing::info!("🔗 Matching features...");
        let matches = geometry::match_all_pairs(&self.images, self.config.match_ratio, self.config.min_matches);
        tracing::info!("Found {} valid image pairs", matches.len());
        
        // 2. Geometric verification and essential matrix estimation
        tracing::info!("📐 Estimating geometry...");
        let mut poses = geometry::estimate_poses(&self.images, &matches)?;
        
        // 3. Triangulate points
        tracing::info!("📍 Triangulating points...");
        let mut points = geometry::triangulate_points(&self.images, &matches, &poses)?;

        // 4. Local bundle adjustment to reduce drift before global BA
        tracing::info!("🧭 Running local bundle adjustment...");
        optimization::local_bundle_adjust(
            &mut points,
            &mut poses,
            &self.images,
            self.config.local_ba_window,
            self.config.local_ba_stride,
            self.config.local_ba_iterations,
            self.config.local_ba_min_points,
            self.config.local_ba_min_rmse,
        )?;

        // 4. Bundle adjustment
        tracing::info!("⚙️  Running bundle adjustment...");
        optimization::bundle_adjust(&mut points, &mut poses, &self.images, self.config.ba_iterations)?;
        
        // 5. Build result
        let reconstruction = SparseReconstruction {
            points,
            cameras: self.images.iter().map(|i| i.intrinsics.clone()).collect(),
            poses,
            image_names: self.images.iter().map(|i| i.path.to_string_lossy().to_string()).collect(),
        };
        
        tracing::info!("✅ Reconstruction complete: {} points, {} cameras",
            reconstruction.points.len(), reconstruction.poses.len());
        
        self.reconstruction = Some(reconstruction.clone());
        Ok(reconstruction)
    }

    /// Run the SfM pipeline using provided camera pose priors (camera-to-world).
    /// Priors must be supplied for all images in the same order as added.
    pub fn run_with_priors(&mut self, priors: &[CameraPose]) -> anyhow::Result<SparseReconstruction> {
        tracing::info!("🚀 Starting SfM pipeline with {} images and pose priors", self.images.len());
        
        if self.images.len() < 2 {
            anyhow::bail!("Need at least 2 images for reconstruction");
        }
        if priors.len() != self.images.len() {
            anyhow::bail!(
                "Pose priors count ({}) does not match images ({})",
                priors.len(),
                self.images.len()
            );
        }
        
        // 1. Match features between all image pairs
        tracing::info!("🔗 Matching features...");
        let matches = geometry::match_all_pairs(&self.images, self.config.match_ratio, self.config.min_matches);
        tracing::info!("Found {} valid image pairs", matches.len());
        
        // 2. Use priors as initial poses
        let mut poses = priors.to_vec();
        for (image, prior) in self.images.iter_mut().zip(priors.iter()) {
            image.prior_pose = Some(prior.clone());
        }
        
        // 3. Triangulate points
        tracing::info!("📍 Triangulating points...");
        let mut points = geometry::triangulate_points(&self.images, &matches, &poses)?;

        // 4. Local bundle adjustment to reduce drift before global BA
        tracing::info!("🧭 Running local bundle adjustment...");
        optimization::local_bundle_adjust(
            &mut points,
            &mut poses,
            &self.images,
            self.config.local_ba_window,
            self.config.local_ba_stride,
            self.config.local_ba_iterations,
            self.config.local_ba_min_points,
            self.config.local_ba_min_rmse,
        )?;

        // 4. Bundle adjustment
        tracing::info!("⚙️  Running bundle adjustment...");
        optimization::bundle_adjust(&mut points, &mut poses, &self.images, self.config.ba_iterations)?;
        
        let reconstruction = SparseReconstruction {
            points,
            cameras: self.images.iter().map(|i| i.intrinsics.clone()).collect(),
            poses,
            image_names: self.images.iter().map(|i| i.path.to_string_lossy().to_string()).collect(),
        };
        
        tracing::info!("✅ Reconstruction complete: {} points, {} cameras",
            reconstruction.points.len(), reconstruction.poses.len());
        
        self.reconstruction = Some(reconstruction.clone());
        Ok(reconstruction)
    }
    
    /// Get the reconstruction result
    pub fn get_reconstruction(&self) -> Option<&SparseReconstruction> {
        self.reconstruction.as_ref()
    }

    pub fn reprojection_stats(&self) -> Option<ReprojectionStats> {
        let reconstruction = self.reconstruction.as_ref()?;
        if reconstruction.points.is_empty() || reconstruction.poses.is_empty() || self.images.is_empty() {
            return None;
        }

        let mut errors: Vec<f64> = Vec::new();
        let mut invalid_observations = 0usize;

        let mut track_lengths: Vec<usize> = Vec::with_capacity(reconstruction.points.len());
        let mut points_with_2plus = 0usize;
        let mut points_with_3plus = 0usize;

        for point in &reconstruction.points {
            let track_len = point.track.len();
            track_lengths.push(track_len);
            if track_len >= 2 {
                points_with_2plus += 1;
            }
            if track_len >= 3 {
                points_with_3plus += 1;
            }

            for (image_id, keypoint_id) in &point.track {
                if *image_id >= self.images.len() || *image_id >= reconstruction.poses.len() {
                    invalid_observations += 1;
                    continue;
                }
                let image = &self.images[*image_id];
                if *keypoint_id >= image.keypoints.len() {
                    invalid_observations += 1;
                    continue;
                }
                let pose = &reconstruction.poses[*image_id];
                let intr = &image.intrinsics;

                let point_cam = pose.world_to_camera(&point.position);
                if point_cam.z <= 0.0 {
                    invalid_observations += 1;
                    continue;
                }

                let u = intr.fx * point_cam.x / point_cam.z + intr.cx;
                let v = intr.fy * point_cam.y / point_cam.z + intr.cy;

                let kp = &image.keypoints[*keypoint_id];
                let dx = u - kp.x as f64;
                let dy = v - kp.y as f64;
                let err = (dx * dx + dy * dy).sqrt();
                if err.is_finite() {
                    errors.push(err);
                } else {
                    invalid_observations += 1;
                }
            }
        }

        let observations = errors.len();
        let mean_error_px = mean(&errors).unwrap_or(0.0);
        let median_error_px = median(&errors).unwrap_or(0.0);
        let p90_error_px = percentile(&mut errors.clone(), 0.90).unwrap_or(0.0);
        let max_error_px = errors.iter().cloned().fold(0.0, f64::max);

        let mean_track_len = mean_usize(&track_lengths).unwrap_or(0.0);
        let median_track_len = median_usize(&mut track_lengths.clone()).unwrap_or(0.0);
        let min_track_len = track_lengths.iter().cloned().min().unwrap_or(0);
        let max_track_len = track_lengths.iter().cloned().max().unwrap_or(0);

        Some(ReprojectionStats {
            points: reconstruction.points.len(),
            observations,
            invalid_observations,
            mean_error_px,
            median_error_px,
            p90_error_px,
            max_error_px,
            mean_track_len,
            median_track_len,
            min_track_len,
            max_track_len,
            points_with_2plus,
            points_with_3plus,
        })
    }
}

fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        Some((sorted[mid - 1] + sorted[mid]) * 0.5)
    } else {
        Some(sorted[mid])
    }
}

fn percentile(values: &mut Vec<f64>, p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((values.len() - 1) as f64 * p).round() as usize;
    values.get(idx).cloned()
}

fn mean_usize(values: &[usize]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let sum: usize = values.iter().sum();
    Some(sum as f64 / values.len() as f64)
}

fn median_usize(values: &mut Vec<usize>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        Some((values[mid - 1] as f64 + values[mid] as f64) * 0.5)
    } else {
        Some(values[mid] as f64)
    }
}

// Re-export for convenience
pub use features::{Keypoint, Descriptor};
pub use geometry::FeatureMatch;

// Add num_cpus for default thread count
mod num_cpus {
    pub fn get() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    }
}

#[cfg(test)]
mod in_memory_image_tests {
    use super::*;

    #[test]
    fn image_data_accepts_pixels_without_reopening_path() {
        let path = Path::new("/definitely/not/persisted/fused-view.png");
        let mut image = image::RgbImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let value = if (x / 8 + y / 8) % 2 == 0 { 16 } else { 240 };
                image.put_pixel(x, y, image::Rgb([value, 255 - value, value]));
            }
        }
        let intrinsics = CameraIntrinsics {
            fx: 50.0,
            fy: 50.0,
            cx: 32.0,
            cy: 32.0,
            width: 64,
            height: 64,
            distortion: Vec::new(),
            distortion_model: DistortionModel::None,
        };
        let data = ImageData::from_rgb_image(path, &image, &intrinsics, 128).unwrap();
        assert_eq!(data.path, path);
        assert_eq!(data.keypoints.len(), data.descriptors.len());
        assert!(data.keypoints.len() <= 128);
    }
}
