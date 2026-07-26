use super::{Camera as TetherCamera, CameraConfig};
use anyhow::{anyhow, Result};
use nokhwa::{
    pixel_format::RgbFormat,
    utils::{
        CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
    },
    Camera,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub trait Gimbal: Send + Sync {
    fn set_pan_tilt(&self, pan_deg: f32, tilt_deg: f32) -> Result<()>;
    fn get_pan_tilt(&self) -> Result<(f32, f32)>;
    fn reset(&self) -> Result<()>;
}

pub struct Insta360Link {
    camera: Option<Arc<Mutex<Camera>>>,
    id: String,
}

unsafe impl Send for Insta360Link {}
unsafe impl Sync for Insta360Link {}

impl Insta360Link {
    pub fn new(index: u32) -> Result<Self> {
        let index_val = CameraIndex::Index(index);

        let formats_to_try = [
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

        let mut camera = None;

        for format_type in formats_to_try {
            let format = RequestedFormat::new::<RgbFormat>(format_type);
            match Camera::new(index_val.clone(), format) {
                Ok(mut cam) => {
                    if let Err(e) = cam.open_stream() {
                        tracing::warn!("Failed to open stream with {:?}: {}", format_type, e);
                        continue;
                    }
                    camera = Some(Arc::new(Mutex::new(cam)));
                    break;
                }
                Err(e) => {
                    tracing::debug!("Format {:?} failed: {}", format_type, e);
                }
            }
        }

        if camera.is_none() {
            tracing::error!(
                "Failed to initialize Insta360 camera stream. Proceeding in Gimbal-Only mode."
            );
        }

        Ok(Self {
            camera,
            id: format!("Insta360_{}", index),
        })
    }
}

impl TetherCamera for Insta360Link {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn capture(&self, _config: &CameraConfig) -> Result<PathBuf> {
        if let Some(cam_mutex) = &self.camera {
            let mut cam = cam_mutex.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
            let frame = cam.frame()?;

            let filename = format!("capture_{}_{}.jpg", self.id, chrono::Utc::now().timestamp());
            let path = std::env::temp_dir().join(filename);

            match frame.source_frame_format() {
                FrameFormat::MJPEG => {
                    std::fs::write(&path, frame.buffer())?;
                }
                _ => {
                    let image_buffer = frame.decode_image::<nokhwa::pixel_format::RgbFormat>()?;
                    image_buffer.save(&path)?;
                }
            }
            Ok(path)
        } else {
            Err(anyhow!("Camera stream not available (Gimbal-only mode)"))
        }
    }

    fn capture_preview(&self) -> Result<Vec<u8>> {
        if let Some(cam_mutex) = &self.camera {
            let mut cam = cam_mutex.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
            let frame = cam.frame()?;

            if frame.source_frame_format() == FrameFormat::MJPEG {
                return Ok(frame.buffer().to_vec());
            }

            let image_buffer = frame.decode_image::<nokhwa::pixel_format::RgbFormat>()?;
            let mut bytes: Vec<u8> = Vec::new();
            image_buffer.write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Jpeg,
            )?;
            Ok(bytes)
        } else {
            Err(anyhow!("Camera stream not available"))
        }
    }

    fn set_config(&self, config: &CameraConfig) -> Result<()> {
        if let Some(cam_mutex) = &self.camera {
            if let Some((w, h)) = config.resolution {
                let mut cam = cam_mutex.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
                let current = cam.camera_format();
                if current.resolution() != Resolution::new(w, h) {
                    tracing::info!("Changing resolution to {}x{}", w, h);
                    cam.stop_stream()?;
                    let new_fmt = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(
                        CameraFormat::new(Resolution::new(w, h), FrameFormat::MJPEG, 30),
                    ));
                    cam.set_camera_requset(new_fmt)?;
                    cam.open_stream()?;
                }
            }
        }
        Ok(())
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
