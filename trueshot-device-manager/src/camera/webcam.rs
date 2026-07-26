use super::thread_actor::ThreadOwnedCamera;
use super::{Camera as TetherCamera, CameraConfig};
use anyhow::Result;
use nokhwa::utils::{CameraFormat, FrameFormat, RequestedFormatType, Resolution};
use std::path::PathBuf;

pub struct GenericWebcam {
    camera: ThreadOwnedCamera,
    id: String,
}

impl GenericWebcam {
    pub fn new(index: u32, name: &str) -> Result<Self> {
        let mut formats_to_try = vec![
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

        // Legacy/Buggy Camera Overrides
        if name.to_lowercase().contains("lifecam") {
            tracing::info!("Applying Safe Mode for Legacy Camera: {}", name);
            formats_to_try = vec![
                RequestedFormatType::Closest(CameraFormat::new(
                    Resolution::new(640, 480),
                    FrameFormat::MJPEG,
                    30,
                )),
                RequestedFormatType::Closest(CameraFormat::new(
                    Resolution::new(640, 480),
                    FrameFormat::YUYV,
                    30,
                )),
            ];
        }

        let id = format!("Webcam_{index}");
        let camera = ThreadOwnedCamera::open(index, id.clone(), formats_to_try, false)?;
        Ok(Self { camera, id })
    }
}

impl TetherCamera for GenericWebcam {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn capture(&self, _config: &CameraConfig) -> Result<PathBuf> {
        self.camera.capture()
    }

    fn capture_preview(&self) -> Result<Vec<u8>> {
        self.camera.preview()
    }

    fn set_config(&self, config: &CameraConfig) -> Result<()> {
        self.camera.configure(config.clone())
    }

    fn battery_level(&self) -> Result<u8> {
        Ok(100)
    }
}
