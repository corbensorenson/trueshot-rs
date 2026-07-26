use serde::{Deserialize, Serialize};
use nalgebra as na;
use std::path::Path;
use anyhow::Result;
use crate::reconstruction::multicam_sfm::{CameraIntrinsics as SfmIntrinsics, CameraPose};
use trueshot_sfm::{CameraMotion, RollingShutterModel};

/// Data format captured during a "Live Scan" session.
/// This typically comes from a webcam recording + tracking data (e.g. ARKit, MediaPipe).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveScanData {
    pub session_id: String,
    pub frames: Vec<LiveScanFrame>,
    pub camera_intrinsics: CameraIntrinsics,
    #[serde(default)]
    pub transform_convention: TransformConvention,
    #[serde(default)]
    pub imu_samples: Vec<ImuSample>,
    #[serde(default)]
    pub rolling_shutter: Option<RollingShutterModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveScanFrame {
    pub image_path: String, // Relative to session root
    pub timestamp: f64,
    pub transform_matrix: [[f64; 4]; 4], // 4x4 Row-major ModelView matrix (or World-to-Camera)
    pub confidence: f32,
    pub turntable_angle: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraIntrinsics {
    pub width: u32,
    pub height: u32,
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformConvention {
    CameraToWorld,
    WorldToCamera,
}

impl Default for TransformConvention {
    fn default() -> Self {
        Self::CameraToWorld
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PosePriors {
    pub frames: Vec<PosePriorFrame>,
    #[serde(default)]
    pub imu_samples: Vec<ImuSample>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PosePriorFrame {
    pub image_path: String,
    pub pose: CameraPose,
    pub intrinsics: SfmIntrinsics,
    #[serde(default)]
    pub camera_motion: Option<CameraMotion>,
    #[serde(default)]
    pub rolling_shutter: Option<RollingShutterModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImuSample {
    pub timestamp: f64,
    pub accel: [f64; 3],
    pub gyro: [f64; 3],
}

impl PosePriors {
    pub fn pose_for_path(&self, path: &Path) -> Option<CameraPose> {
        let path_str = path.to_string_lossy();
        let file_name = path.file_name().map(|p| p.to_string_lossy());
        for frame in &self.frames {
            if frame.image_path == path_str {
                return Some(frame.pose.clone());
            }
            if let Some(name) = &file_name {
                if frame.image_path.ends_with(name.as_ref()) {
                    return Some(frame.pose.clone());
                }
            }
        }
        None
    }

    pub fn intrinsics_for_path(&self, path: &Path) -> Option<SfmIntrinsics> {
        let path_str = path.to_string_lossy();
        let file_name = path.file_name().map(|p| p.to_string_lossy());
        for frame in &self.frames {
            if frame.image_path == path_str {
                return Some(frame.intrinsics.clone());
            }
            if let Some(name) = &file_name {
                if frame.image_path.ends_with(name.as_ref()) {
                    return Some(frame.intrinsics.clone());
                }
            }
        }
        None
    }

    pub fn motion_for_path(&self, path: &Path) -> Option<CameraMotion> {
        let path_str = path.to_string_lossy();
        let file_name = path.file_name().map(|p| p.to_string_lossy());
        for frame in &self.frames {
            if frame.image_path == path_str {
                return frame.camera_motion.clone();
            }
            if let Some(name) = &file_name {
                if frame.image_path.ends_with(name.as_ref()) {
                    return frame.camera_motion.clone();
                }
            }
        }
        None
    }

    pub fn rolling_shutter_for_path(&self, path: &Path) -> Option<RollingShutterModel> {
        let path_str = path.to_string_lossy();
        let file_name = path.file_name().map(|p| p.to_string_lossy());
        for frame in &self.frames {
            if frame.image_path == path_str {
                return frame.rolling_shutter.clone();
            }
            if let Some(name) = &file_name {
                if frame.image_path.ends_with(name.as_ref()) {
                    return frame.rolling_shutter.clone();
                }
            }
        }
        None
    }
}

impl LiveScanData {
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let data: Self = serde_json::from_str(&content)?;
        Ok(data)
    }

    /// Convert LiveScan data to native pose priors (camera-to-world).
    /// `transform_matrix` is interpreted according to `transform_convention`.
    pub fn to_native_priors(&self, _root_path: &Path) -> PosePriors {
        let mut frames = Vec::with_capacity(self.frames.len());

        for (i, frame) in self.frames.iter().enumerate() {
            let mat = na::Matrix4::from_fn(|r, c| frame.transform_matrix[r][c] as f64);
            
            let pose_mat = match self.transform_convention {
                TransformConvention::CameraToWorld => mat,
                TransformConvention::WorldToCamera => match mat.try_inverse() {
                    Some(m) => m,
                    None => {
                        tracing::warn!("Failed to invert matrix for frame {}", i);
                        continue;
                    }
                },
            };

            let rotation = pose_mat.fixed_view::<3, 3>(0, 0);
            let translation = pose_mat.fixed_view::<3, 1>(0, 3);
            let rotation_64 = na::Matrix3::from_fn(|r, c| rotation[(r, c)] as f64);
            let translation_64 = na::Vector3::from_fn(|r, c| translation[(r, c)] as f64);
            let q = na::UnitQuaternion::from_rotation_matrix(&na::Rotation3::from_matrix(&rotation_64));

            let camera_motion = self.imu_motion_for_timestamp(frame.timestamp);
            frames.push(PosePriorFrame {
                image_path: frame.image_path.clone(),
                pose: CameraPose {
                    rotation: q,
                    translation: translation_64,
                },
                intrinsics: SfmIntrinsics {
                    fx: self.camera_intrinsics.fx,
                    fy: self.camera_intrinsics.fy,
                    cx: self.camera_intrinsics.cx,
                    cy: self.camera_intrinsics.cy,
                    width: self.camera_intrinsics.width,
                    height: self.camera_intrinsics.height,
                    distortion: Vec::new(),
                    distortion_model: trueshot_sfm::DistortionModel::None,
                },
                camera_motion,
                rolling_shutter: self.rolling_shutter.clone(),
            });
        }

        PosePriors {
            frames,
            imu_samples: self.imu_samples.clone(),
        }
    }

    fn imu_motion_for_timestamp(&self, timestamp: f64) -> Option<CameraMotion> {
        if self.imu_samples.is_empty() {
            return None;
        }
        let window = 0.01;
        let mut sum_gyro = na::Vector3::zeros();
        let mut count = 0usize;
        for sample in &self.imu_samples {
            let dt = (sample.timestamp - timestamp).abs();
            if dt <= window {
                sum_gyro += na::Vector3::new(sample.gyro[0], sample.gyro[1], sample.gyro[2]);
                count += 1;
            }
        }
        let gyro = if count > 0 {
            sum_gyro / count as f64
        } else {
            let nearest = self.imu_samples.iter().min_by(|a, b| {
                (a.timestamp - timestamp).abs().partial_cmp(&(b.timestamp - timestamp).abs()).unwrap()
            })?;
            na::Vector3::new(nearest.gyro[0], nearest.gyro[1], nearest.gyro[2])
        };
        Some(CameraMotion {
            angular_velocity: gyro,
            linear_velocity: na::Vector3::zeros(),
            timestamp_ms: Some((timestamp * 1000.0) as u64),
        })
    }
}
