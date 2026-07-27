use super::thread_actor::ThreadOwnedCamera;
use super::{Camera as TetherCamera, CameraConfig};
use anyhow::{anyhow, Result};
use nokhwa::utils::{CameraFormat, FrameFormat, RequestedFormatType, Resolution};
use std::path::PathBuf;

pub trait Gimbal: Send + Sync {
    fn set_pan_tilt(&self, pan_deg: f32, tilt_deg: f32) -> Result<()>;
    fn get_pan_tilt(&self) -> Result<(f32, f32)>;
    fn reset(&self) -> Result<()>;
}

pub struct Insta360Link {
    camera: ThreadOwnedCamera,
    id: String,
}

impl Insta360Link {
    pub fn new(index: u32) -> Result<Self> {
        let formats_to_try = vec![
            RequestedFormatType::AbsoluteHighestFrameRate,
            RequestedFormatType::AbsoluteHighestResolution,
            RequestedFormatType::Closest(CameraFormat::new(
                Resolution::new(1920, 1080),
                FrameFormat::MJPEG,
                30,
            )),
            RequestedFormatType::Closest(CameraFormat::new(
                Resolution::new(1280, 720),
                FrameFormat::MJPEG,
                30,
            )),
            RequestedFormatType::Closest(CameraFormat::new(
                Resolution::new(640, 480),
                FrameFormat::MJPEG,
                30,
            )),
        ];

        let id = format!("Insta360_{index}");
        let camera = ThreadOwnedCamera::open(index, id.clone(), formats_to_try, true)?;
        if !camera.is_available() {
            tracing::error!(
                "Failed to initialize Insta360 camera stream. Proceeding in Gimbal-Only mode."
            );
        }

        Ok(Self { camera, id })
    }
}

impl TetherCamera for Insta360Link {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn capture(&self, config: &CameraConfig) -> Result<PathBuf> {
        if config.has_requested_settings() {
            self.set_config(config)?;
        }
        self.camera.capture()
    }

    fn capture_preview(&self) -> Result<Vec<u8>> {
        self.camera.preview()
    }

    fn set_config(&self, config: &CameraConfig) -> Result<()> {
        self.camera.configure(config.clone())
    }

    fn ptz(&self, pan: f32, tilt: f32, zoom: f32) -> Result<()> {
        self.set_pan_tilt(pan, tilt)?;

        // Handle Zoom (Zoom-Abs)
        // Check local or parent uvc-util
        let possible_paths = [
            PathBuf::from("./uvc-util"),
            std::env::current_exe()?.parent().unwrap().join("uvc-util"),
            PathBuf::from("../uvc-util"), // Added check for parent in case
        ];
        let tool_path = possible_paths.iter().find(|p| p.exists());

        if let Some(tool) = tool_path {
            let zoom_val = (zoom * 100.0) as u16;
            if zoom > 0.0 {
                let _ = std::process::Command::new(tool)
                    .arg("-N")
                    .arg("Insta360 Link")
                    .arg("-s")
                    .arg(format!("zoom-abs={}", zoom_val))
                    .output();
            }
        }
        Ok(())
    }

    fn battery_level(&self) -> Result<u8> {
        Ok(100)
    }

    fn as_gimbal(&self) -> Option<&dyn Gimbal> {
        Some(self)
    }
}

impl Gimbal for Insta360Link {
    fn set_pan_tilt(&self, pan_deg: f32, tilt_deg: f32) -> Result<()> {
        let pan_val = (pan_deg * 3600.0) as i32;
        let tilt_val = (tilt_deg * 3600.0) as i32;

        tracing::info!(
            "Setting PT: P={} ({}), T={} ({})",
            pan_deg,
            pan_val,
            tilt_deg,
            tilt_val
        );

        let possible_paths = [
            PathBuf::from("./uvc-util"),
            std::env::current_exe()?.parent().unwrap().join("uvc-util"),
            PathBuf::from("../uvc-util"),
        ];

        let tool_command = possible_paths
            .iter()
            .find(|p| p.exists())
            .map(|p| p.as_os_str().to_owned())
            .unwrap_or_else(|| std::ffi::OsString::from("uvc-util"));

        let args = format!("{{pan={},tilt={}}}", pan_val, tilt_val);
        let set_arg = format!("pan-tilt-abs={}", args);

        tracing::info!("Executing PTZ: {:?} {}", tool_command, set_arg);

        let output = std::process::Command::new(&tool_command)
            .arg("-N")
            .arg("Insta360 Link")
            .arg("-s")
            .arg(&set_arg)
            .output()
            .map_err(|e| anyhow!("Failed to spawn uvc-util: {}", e))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            let out = String::from_utf8_lossy(&output.stdout);
            tracing::error!("uvc-util failed. STDERR: {}, STDOUT: {}", err, out);
            return Err(anyhow!("uvc-util failed: {}", err));
        }

        Ok(())
    }

    fn get_pan_tilt(&self) -> Result<(f32, f32)> {
        let possible_paths = [
            PathBuf::from("./uvc-util"),
            std::env::current_exe()?.parent().unwrap().join("uvc-util"),
            PathBuf::from("../uvc-util"),
        ];
        let tool_path = possible_paths
            .iter()
            .find(|p| p.exists())
            .ok_or_else(|| anyhow!("uvc-util binary not found"))?;

        let output = std::process::Command::new(tool_path)
            .arg("-N")
            .arg("Insta360 Link")
            .arg("-o")
            .arg("pan-tilt-abs")
            .output()?;

        if !output.status.success() {
            return Err(anyhow!("uvc-util failed to read"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.trim();
        if line.starts_with('{') && line.contains("pan=") {
            let p_start = line.find("pan=").ok_or(anyhow!("fmt err"))? + 4;
            let p_end = line[p_start..].find(',').ok_or(anyhow!("fmt err"))? + p_start;
            let pan_str = &line[p_start..p_end];

            let t_start = line.find("tilt=").ok_or(anyhow!("fmt err"))? + 5;
            let t_end = line[t_start..].find('}').ok_or(anyhow!("fmt err"))? + t_start;
            let tilt_str = &line[t_start..t_end];

            let pan_val: i32 = pan_str.parse()?;
            let tilt_val: i32 = tilt_str.parse()?;

            return Ok((pan_val as f32 / 3600.0, tilt_val as f32 / 3600.0));
        }

        Err(anyhow!("Could not parse value: {}", line))
    }

    fn reset(&self) -> Result<()> {
        self.set_pan_tilt(0.0, 0.0)
    }
}
