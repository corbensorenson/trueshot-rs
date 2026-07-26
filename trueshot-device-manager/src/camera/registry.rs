use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum CameraRole {
    LiveFeedback,   // Webcam for real-time 3D
    HighResCapture, // DSLR for texture/details
    TextureReference,
    DepthCamera, // Kinect/RealSense for depth sensing
    None,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CameraProfile {
    pub id: String,
    pub name: String,
    pub nickname: Option<String>,
    pub role: CameraRole,
    pub capabilities: CameraCapabilities,
    pub calibration: Option<CalibrationData>,
    #[serde(default)]
    pub color_calibration: Option<ColorCalibrationData>,
    pub last_settings: Option<CameraSettings>,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CameraCapabilities {
    pub resolutions: Vec<(u32, u32)>,
    pub frame_rates: Vec<u32>,
    pub has_gimbal: bool,
    pub has_zoom: bool,
    pub has_autofocus: bool,

    // Advanced DSLR Capabilities
    #[serde(default)]
    pub iso_options: Vec<String>,
    #[serde(default)]
    pub shutter_speed_options: Vec<String>,
    #[serde(default)]
    pub aperture_options: Vec<String>,
    #[serde(default)]
    pub wb_options: Vec<String>,
    #[serde(default)]
    pub storage_info: Option<StorageInfo>,

    // Depth Camera Capabilities (Kinect, RealSense, etc.)
    #[serde(default)]
    pub has_depth: bool,
    #[serde(default)]
    pub has_infrared: bool,
    #[serde(default)]
    pub has_motor_tilt: bool,
    #[serde(default)]
    pub has_accelerometer: bool,
    #[serde(default)]
    pub has_audio_array: bool,
    #[serde(default)]
    pub depth_resolution: Option<(u32, u32)>,
    #[serde(default)]
    pub tilt_range_degrees: Option<(i8, i8)>, // (min, max) e.g. (-27, 27)
    #[serde(default)]
    pub audio_channels: Option<u8>,
    #[serde(default)]
    pub depth_range_meters: Option<(f32, f32)>, // (min, max) e.g. (0.4, 4.0)
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StorageInfo {
    pub capacity_gb: f32,
    pub free_gb: f32,
    pub remaining_shots: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CalibrationData {
    #[serde(default)]
    pub intrinsics: Option<Vec<f64>>,
    #[serde(default)]
    pub distortion: Option<Vec<f64>>,
    #[serde(default)]
    pub rms_error: Option<f64>,
    #[serde(default)]
    pub image_width: Option<i32>,
    #[serde(default)]
    pub image_height: Option<i32>,
    pub last_calibrated: String, // ISO8601 Date
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ColorCalibrationData {
    pub ccm: [[f32; 3]; 3],
    pub delta_e: f32,
    pub last_calibrated: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CameraSettings {
    pub resolution: Option<(u32, u32)>,
    pub fps: Option<u32>,
    pub iso: Option<String>,
    pub shutter_speed: Option<String>,
    pub wb: Option<String>,
}

pub struct CameraRegistry {
    pub profiles: HashMap<String, CameraProfile>,
    store_path: PathBuf,
}

impl Default for CameraRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CameraRegistry {
    pub fn new() -> Self {
        let store_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".trueshot/cameras.json");

        let mut registry = Self {
            profiles: HashMap::new(),
            store_path,
        };

        if let Err(e) = registry.load() {
            tracing::warn!("Failed to load camera registry: {}", e);
        }

        registry
    }

    pub fn load(&mut self) -> Result<()> {
        if !self.store_path.exists() {
            return Ok(());
        }

        let content = fs::read_to_string(&self.store_path)?;
        let profiles: HashMap<String, CameraProfile> = serde_json::from_str(&content)?;
        self.profiles = profiles;
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.store_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(&self.profiles)?;
        fs::write(&self.store_path, content)?;
        Ok(())
    }

    pub fn get_profile(&self, id: &str) -> Option<&CameraProfile> {
        self.profiles.get(id)
    }

    pub fn get_profile_mut(&mut self, id: &str) -> Option<&mut CameraProfile> {
        self.profiles.get_mut(id)
    }

    pub fn register_camera(
        &mut self,
        id: String,
        name: String,
        role: CameraRole,
        caps: CameraCapabilities,
    ) {
        if let std::collections::hash_map::Entry::Vacant(entry) = self.profiles.entry(id.clone()) {
            let profile = CameraProfile {
                id,
                name,
                nickname: None,
                role,
                capabilities: caps,
                calibration: None,
                color_calibration: None,
                last_settings: None,
                enabled: false,
            };
            entry.insert(profile);
            let _ = self.save();
        }
    }

    pub fn update_calibration(&mut self, id: &str, data: CalibrationData) -> Result<()> {
        if let Some(profile) = self.profiles.get_mut(id) {
            profile.calibration = Some(data);
            self.save()?;
            Ok(())
        } else {
            Err(anyhow!("Camera {} not found in registry", id))
        }
    }

    pub fn update_settings(&mut self, id: &str, settings: CameraSettings) -> Result<()> {
        if let Some(profile) = self.profiles.get_mut(id) {
            profile.last_settings = Some(settings);
            self.save()?;
            Ok(())
        } else {
            Err(anyhow!("Camera {} not found in registry", id))
        }
    }

    pub fn update_color_calibration(&mut self, id: &str, data: ColorCalibrationData) -> Result<()> {
        if let Some(profile) = self.profiles.get_mut(id) {
            profile.color_calibration = Some(data);
            self.save()?;
            Ok(())
        } else {
            Err(anyhow!("Camera {} not found in registry", id))
        }
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<()> {
        if let Some(profile) = self.profiles.get_mut(id) {
            profile.enabled = enabled;
            self.save()?;
            Ok(())
        } else {
            Err(anyhow!("Camera {} not found in registry", id))
        }
    }
}
