use super::{Camera, CameraCapabilities, CameraConfig};
use anyhow::{anyhow, Context, Result};
use futures::executor::block_on;
use gphoto2::{
    widget::{GroupWidget, Widget},
    Camera as GpCamera, Context as GpContext,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const ISO_WIDGETS: &[&str] = &["ISO Speed", "ISO", "iso"];
const SHUTTER_WIDGETS: &[&str] = &["Shutter Speed", "shutterspeed", "Shutter Speed 2"];
const APERTURE_WIDGETS: &[&str] = &["F-Number", "Aperture", "f-number"];
const WHITE_BALANCE_WIDGETS: &[&str] = &[
    "White Balance",
    "White balance",
    "whitebalance",
    "WhiteBalance",
    "WB",
];
const CAPTURE_TARGET_WIDGETS: &[&str] = &["Capture Target", "capturetarget"];

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
        tracing::debug!("Starting gPhoto camera discovery");
        #[cfg(target_os = "macos")]
        release_macos_ptp_helper();

        let context = GpContext::new()?;
        let mut cameras = Vec::new();

        let detection_result = block_on(context.autodetect_camera());
        match detection_result {
            Ok(cam) => {
                tracing::info!("gPhoto camera detected");
                if let Ok(c) = Self::new(cam, GpContext::new()?) {
                    cameras.push(c);
                }
            }
            Err(e) => {
                tracing::debug!("gPhoto autodetect found no available camera: {}", e);
            }
        }

        tracing::debug!("gPhoto discovery returned {} cameras", cameras.len());
        Ok(cameras)
    }
}

impl Camera for GPhotoCamera {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn capture(&self, config: &CameraConfig) -> Result<PathBuf> {
        if config.has_requested_settings() {
            self.set_config(config)?;
        }
        let cam = self.camera.lock().unwrap();
        let path_info =
            block_on(cam.capture_image()).map_err(|e| anyhow!("Capture failed: {}", e))?;
        let camera_name = path_info.name();
        let destination = unique_capture_path(&camera_name)?;
        let partial = partial_path(&destination);
        let downloaded = block_on(cam.fs().download_to(
            &path_info.folder(),
            &camera_name,
            &partial,
        ))
        .map_err(|error| {
            let _ = fs::remove_file(&partial);
            anyhow!("Captured {camera_name} but failed to download it: {error}")
        })?;
        drop(downloaded);
        let publish_result = (|| -> Result<()> {
            let size = fs::metadata(&partial)
                .with_context(|| format!("Inspect downloaded capture {}", partial.display()))?
                .len();
            if size == 0 {
                return Err(anyhow!(
                    "Camera returned an empty capture for {camera_name}"
                ));
            }
            fs::File::open(&partial)
                .with_context(|| format!("Open downloaded capture {}", partial.display()))?
                .sync_all()
                .with_context(|| format!("Sync downloaded capture {}", partial.display()))?;
            fs::rename(&partial, &destination).with_context(|| {
                format!(
                    "Publish downloaded capture {} as {}",
                    partial.display(),
                    destination.display()
                )
            })
        })();
        if let Err(error) = publish_result {
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
        if let Some(parent) = destination.parent() {
            if let Err(error) = fs::File::open(parent).and_then(|directory| directory.sync_all()) {
                tracing::warn!(
                    "Capture {} is published but directory sync failed: {}",
                    destination.display(),
                    error
                );
            }
        }
        Ok(destination)
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
        if config.resolution.is_some() || config.fps.is_some() {
            return Err(anyhow!(
                "gPhoto still-capture adapter does not support video resolution/FPS controls"
            ));
        }
        let cam = self.camera.lock().unwrap();
        let gp_config =
            block_on(cam.config()).map_err(|e| anyhow!("Failed to get config: {}", e))?;
        let mut requested = Vec::new();
        if let Some(iso) = &config.iso {
            set_required_setting(&gp_config, "ISO", ISO_WIDGETS, iso)?;
            requested.push(("ISO", ISO_WIDGETS, iso.as_str()));
        }
        if let Some(ss) = &config.shutter_speed {
            set_required_setting(&gp_config, "shutter speed", SHUTTER_WIDGETS, ss)?;
            requested.push(("shutter speed", SHUTTER_WIDGETS, ss.as_str()));
        }
        if let Some(ap) = &config.aperture {
            set_required_setting(&gp_config, "aperture", APERTURE_WIDGETS, ap)?;
            requested.push(("aperture", APERTURE_WIDGETS, ap.as_str()));
        }
        if let Some(wb) = &config.wb {
            set_required_setting(&gp_config, "white balance", WHITE_BALANCE_WIDGETS, wb)?;
            requested.push(("white balance", WHITE_BALANCE_WIDGETS, wb.as_str()));
        }
        if let Some(target) = &config.capture_target {
            set_required_setting(&gp_config, "capture target", CAPTURE_TARGET_WIDGETS, target)?;
            requested.push(("capture target", CAPTURE_TARGET_WIDGETS, target.as_str()));
        }
        if !requested.is_empty() {
            block_on(cam.set_config(&gp_config))
                .map_err(|e| anyhow!("Failed to apply config: {}", e))?;
            let mut last_error = None;
            for attempt in 0..3 {
                let verified = block_on(cam.config())
                    .map_err(|e| anyhow!("Failed to verify config: {}", e))?;
                let result = requested
                    .iter()
                    .try_for_each(|(setting, aliases, expected)| {
                        verify_setting(&verified, setting, aliases, expected)
                    });
                match result {
                    Ok(()) => return Ok(()),
                    Err(error) => last_error = Some(error),
                }
                if attempt < 2 {
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
            }
            return Err(last_error
                .unwrap_or_else(|| anyhow!("Camera setting readback failed unexpectedly")));
        }
        Ok(())
    }

    fn ptz(&self, _pan: f32, _tilt: f32, _zoom: f32) -> Result<()> {
        Err(anyhow!(
            "PTZ control is not supported by the gPhoto adapter"
        ))
    }

    fn drive_focus(&self, step: i32) -> Result<()> {
        if step == 0 {
            return Err(anyhow!("Manual focus drive step cannot be zero"));
        }
        let cam = self.camera.lock().unwrap();

        let config = block_on(cam.config()).map_err(|e| anyhow!("Failed to get config: {}", e))?;

        // NIKON REQUIREMENT: Viewfinder/Liveview must be active for focus control!
        // Enable viewfinder first
        if let Ok(widget) = config.get_child_by_name("viewfinder") {
            tracing::info!("Found viewfinder widget, enabling...");
            if let Widget::Toggle(toggle) = widget {
                toggle.set_toggled(true);
                block_on(cam.set_config(&config))
                    .map_err(|e| anyhow!("Failed to enable live view for focus drive: {}", e))?;
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
                    radio
                        .set_choice(value_str)
                        .map_err(|e| anyhow!("Failed to select focus drive command: {}", e))?;
                }
                Widget::Text(text) => {
                    text.set_value(value_str)
                        .map_err(|e| anyhow!("Failed to set text focus drive command: {}", e))?;
                }
                _ => return Err(anyhow!("Unsupported manual focus widget type")),
            }

            block_on(cam.set_config(&config))
                .map_err(|e| anyhow!("Failed to set generic focus: {}", e))?;
            return Ok(());
        }

        Err(anyhow!("Camera exposes no supported manual focus widget"))
    }
}

fn find_widget(config: &GroupWidget, aliases: &[&str]) -> Option<Widget> {
    aliases
        .iter()
        .find_map(|alias| config.get_child_by_label(alias).ok())
        .or_else(|| {
            aliases
                .iter()
                .find_map(|alias| config.get_child_by_name(alias).ok())
        })
}

fn set_required_setting(
    config: &GroupWidget,
    setting: &str,
    aliases: &[&str],
    value: &str,
) -> Result<()> {
    let widget = find_widget(config, aliases)
        .with_context(|| format!("Camera exposes no writable {setting} control"))?;
    if widget.readonly() {
        return Err(anyhow!("Camera {setting} control is read-only"));
    }
    match widget {
        Widget::Radio(radio) => {
            if !radio.choices_iter().any(|choice| choice == value) {
                return Err(anyhow!(
                    "Camera rejected unsupported {setting} value {value:?}"
                ));
            }
            radio
                .set_choice(value)
                .with_context(|| format!("Set camera {setting} to {value:?}"))
        }
        Widget::Text(text) => text
            .set_value(value)
            .with_context(|| format!("Set camera {setting} to {value:?}")),
        _ => Err(anyhow!(
            "Camera {setting} control has an unsupported widget type"
        )),
    }
}

fn verify_setting(
    config: &GroupWidget,
    setting: &str,
    aliases: &[&str],
    expected: &str,
) -> Result<()> {
    let widget = find_widget(config, aliases)
        .with_context(|| format!("Camera {setting} control disappeared during verification"))?;
    let actual = match widget {
        Widget::Radio(radio) => radio.choice(),
        Widget::Text(text) => text.value(),
        _ => {
            return Err(anyhow!(
                "Camera {setting} control cannot be read back exactly"
            ))
        }
    };
    if actual != expected {
        return Err(anyhow!(
            "Camera {setting} readback mismatch: requested {expected:?}, got {actual:?}"
        ));
    }
    Ok(())
}

fn unique_capture_path(camera_name: &str) -> Result<PathBuf> {
    let root = std::env::var_os("TRUESHOT_CAPTURE_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|path| path.join("TrueShot").join("captures")))
        .context("Cannot determine local TrueShot capture directory")?;
    fs::create_dir_all(&root)
        .with_context(|| format!("Create capture directory {}", root.display()))?;
    let safe_name = safe_camera_filename(camera_name);
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
    Ok(root.join(format!("{timestamp}_{}_{}", Uuid::new_v4(), safe_name)))
}

fn safe_camera_filename(camera_name: &str) -> String {
    let safe = Path::new(camera_name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("capture.bin")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let safe = safe.trim_matches('.').to_string();
    if safe.is_empty() {
        "capture.bin".to_string()
    } else {
        safe
    }
}

fn partial_path(destination: &Path) -> PathBuf {
    let mut name = destination.as_os_str().to_os_string();
    name.push(".part");
    PathBuf::from(name)
}

#[cfg(target_os = "macos")]
fn release_macos_ptp_helper() {
    let result = std::process::Command::new("killall")
        .arg("PTPCamera")
        .output();
    match result {
        Ok(output) if output.status.success() => {
            tracing::debug!("Released Apple's PTPCamera helper for tethered capture");
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        Ok(_) => {}
        Err(error) => tracing::debug!("Could not request PTPCamera release: {}", error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_filename_cannot_escape_capture_directory() {
        assert_eq!(
            safe_camera_filename("../../DCIM/DSC_0001.NEF"),
            "DSC_0001.NEF"
        );
        assert_eq!(safe_camera_filename("a b?.nef"), "a_b_.nef");
        assert_eq!(safe_camera_filename(".."), "capture.bin");
    }
}
