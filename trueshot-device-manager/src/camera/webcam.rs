use anyhow::{Result, anyhow};
use nokhwa::{
    pixel_format::RgbFormat,
    utils::{CameraIndex, RequestedFormat, RequestedFormatType, Resolution, FrameFormat, CameraFormat},
    Camera,
};
use std::sync::{Arc, Mutex};
use std::path::PathBuf;
use super::{Camera as TetherCamera, CameraConfig};

pub struct GenericWebcam {
    camera: Arc<Mutex<Camera>>,
    id: String,
}

unsafe impl Send for GenericWebcam {}
unsafe impl Sync for GenericWebcam {}

impl GenericWebcam {
    pub fn new(index: u32, name: &str) -> Result<Self> {
        let index_val = CameraIndex::Index(index);
        
        let mut formats_to_try = vec![
            RequestedFormatType::Closest(CameraFormat::new(Resolution::new(1920, 1080), FrameFormat::MJPEG, 30)),
            RequestedFormatType::Closest(CameraFormat::new(Resolution::new(1280, 720), FrameFormat::MJPEG, 30)),
            RequestedFormatType::Closest(CameraFormat::new(Resolution::new(640, 480), FrameFormat::MJPEG, 30)),
        ];

        // Legacy/Buggy Camera Overrides
        if name.to_lowercase().contains("lifecam") {
            tracing::info!("Applying Safe Mode for Legacy Camera: {}", name);
            formats_to_try = vec![
                RequestedFormatType::Closest(CameraFormat::new(Resolution::new(640, 480), FrameFormat::MJPEG, 30)),
                RequestedFormatType::Closest(CameraFormat::new(Resolution::new(640, 480), FrameFormat::YUYV, 30)),
            ];
        }

        let mut camera = None;

        for format_type in formats_to_try {
            let idx_clone = index_val.clone();
            // Wrap strictly the new call which might panic on bad drivers
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let format = RequestedFormat::new::<RgbFormat>(format_type);
                Camera::new(idx_clone, format)
            }));

            match res {
                Ok(Ok(mut cam)) => {
                    // Try to open stream to verify it actually works
                    if let Err(e) = cam.open_stream() {
                         tracing::warn!("Webcam {} stream open failed with {:?}: {}", index, format_type, e);
                         continue;
                    }
                    // It works!
                    camera = Some(Arc::new(Mutex::new(cam)));
                    break;
                },
                Ok(Err(e)) => {
                    tracing::debug!("Webcam {} init failed with {:?}: {}", index, format_type, e);
                },
                Err(_) => {
                    tracing::warn!("Webcam {} panicked (SIGSEGV/Safe) with format {:?}", index, format_type);
                },
            }
        }

        if let Some(cam) = camera {
             Ok(Self {
                camera: cam, // Changed from `inner` to `camera` to match struct field
                id: format!("Webcam_{}", index), // Changed from `webcam-` to `Webcam_` and removed `index` field
            })
        } else {
            Err(anyhow::anyhow!("Failed to initialize webcam {} with any format", index))
        }
    }
}

impl TetherCamera for GenericWebcam {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn capture(&self, _config: &CameraConfig) -> Result<PathBuf> {
        let mut cam = self.camera.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
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
    }

    fn capture_preview(&self) -> Result<Vec<u8>> {
        let mut cam = self.camera.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
        let frame = cam.frame()?;
        
        if frame.source_frame_format() == FrameFormat::MJPEG {
            return Ok(frame.buffer().to_vec());
        }

        // Encode to JPEG if raw
        let image_buffer = frame.decode_image::<nokhwa::pixel_format::RgbFormat>()?;
        let mut bytes: Vec<u8> = Vec::new();
        image_buffer.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Jpeg)?;
        Ok(bytes)
    }

    fn set_config(&self, _config: &CameraConfig) -> Result<()> {
        Ok(())
    }

    fn battery_level(&self) -> Result<u8> {
        Ok(100)
    }
}
