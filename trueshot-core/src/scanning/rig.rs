use crate::intrinsics::estimate_intrinsics;
use crate::reconstruction::multicam_sfm::{CameraPose, FeatureType, SfmConfig, SfmPipeline};
use anyhow::Result;
use nalgebra as na;
use std::collections::HashMap;
use std::path::PathBuf;

/// Represents the solved configuration of the scanner rig at a specific time (Angle 0).
/// Camera poses are stored as camera-to-world transforms in the rig coordinate frame.
#[derive(Debug, Clone)]
pub struct ScannerRig {
    /// Stable camera ordering for propagation and matching.
    pub camera_order: Vec<String>,
    /// Map of Camera ID/path -> Pose (camera-to-world).
    pub camera_poses: HashMap<String, CameraPose>,
}

#[derive(Debug, Clone)]
pub struct RigCameraPose {
    pub name: String,
    pub pose: CameraPose,
}

impl ScannerRig {
    /// Propagate rig poses to a new turntable angle.
    /// We rotate the camera poses around the rig's Y axis by `angle_deg`.
    pub fn propagate(&self, angle_deg: f32) -> Vec<RigCameraPose> {
        let angle_rad = angle_deg.to_radians() as f64;
        let rot_y = na::Rotation3::from_axis_angle(&na::Vector3::y_axis(), angle_rad);

        let mut poses = Vec::with_capacity(self.camera_order.len());
        for name in &self.camera_order {
            if let Some(base_pose) = self.camera_poses.get(name) {
                let rotation =
                    na::UnitQuaternion::from_rotation_matrix(&na::Rotation3::from_matrix(
                        &(rot_y * base_pose.rotation.to_rotation_matrix()).into_inner(),
                    ));
                let translation = rot_y * base_pose.translation;
                poses.push(RigCameraPose {
                    name: name.clone(),
                    pose: CameraPose {
                        rotation,
                        translation,
                    },
                });
            }
        }

        poses
    }
}

pub struct RigSolver {
    pub workspace_path: PathBuf,
}

impl RigSolver {
    pub fn new(workspace_path: PathBuf) -> Self {
        Self { workspace_path }
    }

    /// Solves the rig configuration from a set of images taken at the same time (Angle 0).
    /// Uses native SfM (no external dependencies) to recover camera-to-world poses.
    pub fn solve_for_sequence(&self, images: &[PathBuf]) -> Result<ScannerRig> {
        tracing::info!("🧩 Solving Rig for {} images...", images.len());

        if images.len() < 2 {
            anyhow::bail!("Need at least 2 images to solve rig");
        }

        let mut images_sorted: Vec<PathBuf> = images.to_vec();
        images_sorted.sort();

        let sfm_config = SfmConfig {
            feature_type: FeatureType::Sift,
            max_features: 12000,
            match_ratio: 0.7,
            min_matches: 50,
            ba_iterations: 120,
            local_ba_window: 7,
            local_ba_stride: 1,
            local_ba_iterations: 36,
            local_ba_min_points: 260,
            local_ba_min_rmse: 0.7,
            enable_dense: false,
            num_threads: num_cpus::get(),
        };

        let mut pipeline = SfmPipeline::new(sfm_config);
        for path in &images_sorted {
            let intrinsics = estimate_intrinsics(path)?;
            pipeline.add_image_with_intrinsics(path, intrinsics, None)?;
        }

        let reconstruction = pipeline.run()?;

        let mut camera_poses = HashMap::new();
        let mut camera_order = Vec::with_capacity(reconstruction.poses.len());

        for (idx, pose) in reconstruction.poses.iter().enumerate() {
            let name = reconstruction
                .image_names
                .get(idx)
                .cloned()
                .unwrap_or_else(|| images_sorted[idx].to_string_lossy().to_string());
            camera_order.push(name.clone());
            camera_poses.insert(name, pose.clone());
        }

        tracing::info!("✅ Rig Solved with {} cameras", camera_poses.len());

        Ok(ScannerRig {
            camera_order,
            camera_poses,
        })
    }
}
