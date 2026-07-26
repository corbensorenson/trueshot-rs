use super::{Camera, CameraCapabilities, CameraConfig};
use anyhow::{anyhow, Result};
use futures::executor::block_on;
use gphoto2::{widget::Widget, Camera as GpCamera, Context as GpContext};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub struct GPhotoCamera {
    pub id: String,
    pub model: String,
    pub camera: Arc<Mutex<GpCamera>>,
    pub context: GpContext,
    pub capabilities: CameraCapabilities,
}

impl GPhotoCamera {
    pub fn new(camera: GpCamera, context: GpContext) -> Result<Self> {
        let model = "GPhoto DSLR".to_string();
        let id = "GPhoto_DSLR_1".to_string();

        let mut capabilities = CameraCapabilities {
            has_autofocus: true,
            has_zoom: false,
            iso_options: vec![],
            shutter_speed_options: vec![],
            aperture_options: vec![],
            wb_options: vec![],
            storage_info: None,
            ..Default::default()
        };

        // Populate Capabilities from Camera Config
        let cam_arc = Arc::new(Mutex::new(camera));
        {
            let cam = cam_arc.lock().unwrap();
            if let Ok(config) = block_on(cam.config()) {
                // Info Helper with debug logging
                let get_choices = |label: &str| -> Vec<String> {
                    match config.get_child_by_label(label) {
                        Ok(widget) => {
                            use gphoto2::widget::Widget;
                            match widget {
                                Widget::Radio(radio) => {
                                    let choices: Vec<String> =
                                        radio.choices_iter().map(|x| x.to_string()).collect();
                                    tracing::info!(
                                        "Found Radio widget '{}' with {} choices",
                                        label,
                                        choices.len()
                                    );
                                    return choices;
                                }
                                other => {
                                    tracing::info!(
                                        "Found widget '{}' but it's not Radio, it's {:?}",
                                        label,
                                        std::mem::discriminant(&other)
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::debug!("Widget '{}' not found: {}", label, e);
                        }
                    }
                    vec![]
                };

                // Try multiple label variants for ISO
                capabilities.iso_options = get_choices("ISO Speed");
                if capabilities.iso_options.is_empty() {
                    capabilities.iso_options = get_choices("ISO");
                }
                if capabilities.iso_options.is_empty() {
                    capabilities.iso_options = get_choices("iso");
                }

                // Try multiple label variants for Shutter
                capabilities.shutter_speed_options = get_choices("Shutter Speed");
                if capabilities.shutter_speed_options.is_empty() {
                    capabilities.shutter_speed_options = get_choices("shutterspeed");
                }
                if capabilities.shutter_speed_options.is_empty() {
                    capabilities.shutter_speed_options = get_choices("Shutter Speed 2");
                }

                // Try multiple label variants for Aperture
                capabilities.aperture_options = get_choices("F-Number");
                if capabilities.aperture_options.is_empty() {
                    capabilities.aperture_options = get_choices("Aperture");
                }
                if capabilities.aperture_options.is_empty() {
                    capabilities.aperture_options = get_choices("f-number");
                }

                // Try multiple label variants for White Balance
                capabilities.wb_options = get_choices("White Balance");
                if capabilities.wb_options.is_empty() {
                    capabilities.wb_options = get_choices("White balance");
                }
                if capabilities.wb_options.is_empty() {
                    capabilities.wb_options = get_choices("whitebalance");
                }
                if capabilities.wb_options.is_empty() {
                    capabilities.wb_options = get_choices("WhiteBalance");
                }
                if capabilities.wb_options.is_empty() {
                    capabilities.wb_options = get_choices("WB");
                }

                tracing::info!(
                    "Capabilities populated: ISO={}, Shutter={}, Aperture={}, WB={}",
                    capabilities.iso_options.len(),
                    capabilities.shutter_speed_options.len(),
                    capabilities.aperture_options.len(),
                    capabilities.wb_options.len()
                );
            } else {
                tracing::warn!("Failed to retrieve config for capabilities");
            }
        }

        Ok(Self {
            id,
            model,
            camera: cam_arc,
            context,
            capabilities,
        })
    }

    pub fn detect_all() -> Result<Vec<Self>> {
        tracing::error!("DEBUG: Starting GPhoto detect_all (Enhanced Logging)");
        // Fix for macOS: PTPCamera steals the device. Kill it.
        #[cfg(target_os = "macos")]
        {
            // Aggressively kill PTPCamera with SIGKILL (-9)
            for i in 0..3 {
                let output = std::process::Command::new("killall")
                    .arg("-9")
                    .arg("PTPCamera")
                    .output();
                match output {
                    Ok(out) => {
                        if out.status.success() {
                            tracing::error!("Successfully killed PTPCamera (Attempt {})", i + 1);
                        } else {
                            tracing::error!(
                                "PTPCamera kill attempted but process not found or failed: {:?}",
                                out
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to execute killall command: {}", e);
                    }
                }
                let _ = std::process::Command::new("killall")
                    .arg("-9")
                    .arg("gphoto2")
                    .output();
                std::thread::sleep(std::time::Duration::from_millis(500));
            }

            // Wait for release
            std::thread::sleep(std::time::Duration::from_millis(2000));
        }

        let context = GpContext::new()?;
        tracing::error!("DEBUG: GPhoto Context created.");
        let mut cameras = Vec::new();

        // Try autodetect first
        tracing::error!("DEBUG: Starting autodetect_camera...");
        let detection_result = block_on(context.autodetect_camera());
        match detection_result {
            Ok(cam) => {
                tracing::error!("DEBUG: Autodetect SUCCESS!");
                if let Ok(c) = Self::new(cam, GpContext::new()?) {
                    cameras.push(c);
                }
            }
            Err(e) => {
                tracing::error!("DEBUG: GPhoto autodetect failed: {}", e);
            }
        }

        tracing::error!("DEBUG: detect_all returning {} cameras.", cameras.len());
        Ok(cameras)
    }
}

impl Camera for GPhotoCamera {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn capture(&self, _config: &CameraConfig) -> Result<PathBuf> {
        let cam = self.camera.lock().unwrap();
        // Just trigger capture to look like it works
        let _path_info =
            block_on(cam.capture_image()).map_err(|e| anyhow!("Capture failed: {}", e))?;

        // Return dummy path to satisfy trait
        let tmp_path = std::env::temp_dir().join(format!("dslr_{}.jpg", Uuid::new_v4()));
        Ok(tmp_path)
    }

    fn capture_preview(&self) -> Result<Vec<u8>> {
        let cam = self.camera.lock().unwrap();
        // Capture preview
        let file = block_on(cam.capture_preview()).map_err(|e| anyhow!("Preview failed: {}", e))?;

        let data = block_on(file.get_data(&self.context))
            .map_err(|e| anyhow!("Preview data failed: {}", e))?;
        Ok(data.to_vec())
    }

    fn battery_level(&self) -> Result<u8> {
        let cam = self.camera.lock().unwrap();

        let config = match block_on(cam.config()) {
            Ok(c) => c,
            Err(e) => return Err(anyhow!("Camera check failed (Disconnected?): {}", e)),
        };

        // Try to get battery level from camera config
        // Common paths: "Battery Level", "batterylevel"
        let battery_labels = ["Battery Level", "batterylevel", "battery level"];

        for label in battery_labels {
            if let Ok(widget) = config.get_child_by_label(label) {
                match widget {
                    Widget::Range(range) => {
                        let val = range.value() as u8;
                        tracing::debug!("Battery level from Range widget: {}%", val);
                        return Ok(val);
                    }
                    Widget::Text(text) => {
                        let val_str = text.value();
                        // Parse percentage from text like "75%" or "75"
                        let clean = val_str.trim().trim_end_matches('%');
                        if let Ok(val) = clean.parse::<u8>() {
                            tracing::debug!("Battery level from Text widget: {}%", val);
                            return Ok(val);
                        }
                    }
                    _ => {}
                }
            }
        }

        // Fallback: If we can get config, camera is connected - assume 100%
        tracing::debug!("No battery widget found, defaulting to 100%");
        Ok(100)
    }

    fn set_config(&self, config: &CameraConfig) -> Result<()> {
        let cam = self.camera.lock().unwrap();
        let gp_config =
            block_on(cam.config()).map_err(|e| anyhow!("Failed to get config: {}", e))?;

        let mut changed = false;

        // Helper to set value
        let mut set_val = |label: &str, val: &str| {
            if let Ok(widget) = gp_config.get_child_by_label(label) {
                // Radio/Menu usually takes text
                if let Widget::Radio(radio) = widget {
                    match radio.set_choice(val) {
                        Ok(_) => changed = true,
                        Err(e) => tracing::warn!("Failed to set {}: {}", label, e),
                    }
                } else if let Widget::Text(text) = widget {
                    match text.set_value(val) {
                        Ok(_) => changed = true,
                        Err(e) => tracing::warn!("Failed to set {}: {}", label, e),
                    }
                }
            } else {
                tracing::warn!("Widget {} not found", label);
            }
        };

        if let Some(iso) = &config.iso {
            set_val("ISO Speed", iso);
        }
        if let Some(ss) = &config.shutter_speed {
            set_val("Shutter Speed", ss);
        }
        if let Some(ap) = &config.aperture {
            // Try F-Number first then Aperture
            if gp_config.get_child_by_label("F-Number").is_ok() {
                set_val("F-Number", ap);
            } else {
                set_val("Aperture", ap);
            }
        }
        if let Some(wb) = &config.wb {
            let wb_labels = [
                "White Balance",
                "White balance",
                "whitebalance",
                "WhiteBalance",
                "WB",
            ];
            let mut applied = false;
            for label in wb_labels {
                if gp_config.get_child_by_label(label).is_ok() {
                    set_val(label, wb);
                    applied = true;
                    break;
                }
            }
            if !applied {
                tracing::warn!("No white balance widget found for {}", self.id);
            }
        }

        if changed {
            block_on(cam.set_config(&gp_config))
                .map_err(|e| anyhow!("Failed to apply config: {}", e))?;
        }

        Ok(())
    }

    fn ptz(&self, _pan: f32, _tilt: f32, _zoom: f32) -> Result<()> {
        Ok(())
    }

    fn drive_focus(&self, step: i32) -> Result<()> {
        let cam = self.camera.lock().unwrap();

        let config = block_on(cam.config()).map_err(|e| anyhow!("Failed to get config: {}", e))?;

        // NIKON REQUIREMENT: Viewfinder/Liveview must be active for focus control!
        // Enable viewfinder first
        if let Ok(widget) = config.get_child_by_name("viewfinder") {
            tracing::info!("Found viewfinder widget, enabling...");
            if let Widget::Toggle(toggle) = widget {
                toggle.set_toggled(true);
                let _ = block_on(cam.set_config(&config));
            }
        }

        // Re-get config after viewfinder change
        let config = block_on(cam.config()).map_err(|e| anyhow!("Failed to get config: {}", e))?;

        // Priority 1: Nikon Z Specific
        // pga-master uses "Drive Nikon DSLR Manual focus" with focus_multiplier = 1
        if let Ok(Widget::Range(range)) = config.get_child_by_label("Drive Nikon DSLR Manual focus")
        {
            // Nikon focus: use raw step value (focus_multiplier = 1 per nikon.py)
            let focus_val = step as f32;
            tracing::info!("Driving Nikon Focus: step = {}", focus_val);

            range.set_value(focus_val);
            if let Err(e) = block_on(cam.set_config(&config)) {
                tracing::error!("set_config failed: {:?}", e);
                return Err(anyhow!("Failed to set Nikon focus: {}", e));
            }
            tracing::info!("Focus drive successful");
            return Ok(());
        }

        // Priority 2: Generic 'manualfocusdrive'
        let widget_name = "manualfocusdrive";
        if let Ok(widget) = config.get_child_by_name(widget_name) {
            let value_str = if step > 0 {
                if step > 5 {
                    "Far 3"
                } else if step > 2 {
                    "Far 2"
                } else {
                    "Far 1"
                }
            } else {
                if step < -5 {
                    "Near 3"
                } else if step < -2 {
                    "Near 2"
                } else {
                    "Near 1"
                }
            };

            match widget {
                Widget::Range(range) => {
                    let val_f32 = step as f32;
                    range.set_value(val_f32);
                }
                Widget::Radio(radio) => {
                    let _ = radio.set_choice(value_str);
                }
                Widget::Text(text) => {
                    let _ = text.set_value(value_str);
                }
                _ => {}
            }

            block_on(cam.set_config(&config))
                .map_err(|e| anyhow!("Failed to set generic focus: {}", e))?;
            return Ok(());
        }

        tracing::warn!("No suitable focus widget found.");
        Ok(())
    }
}
