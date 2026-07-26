use super::{DirectorContext, ScanTask};
use crate::director::DirectorState;
use crate::events::{LogLevel, SystemEvent};
use crate::scanning::workflow::{CaptureConfig, ScanAction};
use crate::scanning::{QualityLevel, SmartScanStrategy};
use anyhow::Result;
use chrono::Utc;
use image::{ImageBuffer, Rgb};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use trueshot_device_manager::{CameraConfig, CameraRole};

pub struct PromptUserTask {
    pub message: String,
}

#[async_trait::async_trait]
impl ScanTask for PromptUserTask {
    fn name(&self) -> &'static str {
        "PromptUser"
    }

    async fn on_enter(&self, ctx: &DirectorContext) -> Result<bool> {
        ctx.bus.publish(SystemEvent::SystemMessage(
            self.message.clone(),
            LogLevel::Warning,
        ));

        // Lock state to WaitingForUser
        let mut state = ctx.state.lock().await;
        *state = DirectorState::WaitingForUser(self.message.clone());

        let mut detector = ctx.detector.lock().await;
        detector.reset();

        Ok(false) // Wait for user/stabilization
    }

    async fn on_frame(
        &self,
        ctx: &DirectorContext,
        frame: &ImageBuffer<Rgb<u8>, Vec<u8>>,
    ) -> Result<bool> {
        let mut detector = ctx.detector.lock().await;
        if detector.update(frame) {
            ctx.bus.publish(SystemEvent::SystemMessage(
                "Scene Stabilized. Proceeding.".into(),
                LogLevel::Success,
            ));

            // Clear Waiting state
            let state = ctx.state.lock().await;
            // Only clear if we are still in waiting?
            // Actually execute_step will update state to ExecutingStep(next) or Idle.
            // But for correctness we could set it to internal generic state.
            // However, returning `true` here causes Director to call `advance_step`, which overwrites state.
            drop(state);
            return Ok(true);
        }
        Ok(false)
    }
}

pub struct CalibrateTask {
    pub quality: QualityLevel,
}

#[async_trait::async_trait]
impl ScanTask for CalibrateTask {
    fn name(&self) -> &'static str {
        "Calibrate"
    }

    async fn on_enter(&self, ctx: &DirectorContext) -> Result<bool> {
        ctx.bus.publish(SystemEvent::SystemMessage(
            "Auto-Calibrating...".into(),
            LogLevel::Info,
        ));

        let calibrator = crate::scanning::calibration::AutoCalibrator::new(Default::default());
        let result = calibrator.calculate(self.quality, 100.0, 500.0);

        ctx.bus.publish(SystemEvent::SystemMessage(
            format!(
                "Optimized: {:.1} deg step, {} focus slices.",
                result.turntable_step_deg, result.focus_steps
            ),
            LogLevel::Success,
        ));

        // Logic copied from Director::execute_step for Calibration
        let mut brackets = vec![0.0];
        if let Some(img) = ctx.background_reference.lock().await.as_ref() {
            brackets = calibrator.calculate_exposure_brackets(img);
        }

        let views = (360.0 / result.turntable_step_deg).ceil() as usize;
        let shots = views * result.focus_steps * brackets.len();
        let total_sec = shots as f32 * 0.5 + 20.0;

        let bracket_str = if brackets.len() > 1 {
            format!(" | Auto-HDR: {:?}", brackets)
        } else {
            "".into()
        };
        ctx.bus.publish(SystemEvent::SystemMessage(
            format!(
                "Est. Time: {:.1} min ({} shots){}",
                total_sec / 60.0,
                shots,
                bracket_str
            ),
            LogLevel::Info,
        ));

        // Update Workflow Steps
        let mut wf_guard = ctx.workflow.lock().await;
        if let Some(wf) = wf_guard.as_mut() {
            for step in &mut wf.steps {
                if let ScanAction::SmartScan {
                    capture,
                    quality: q,
                } = step
                {
                    if matches!(capture, CaptureConfig::Auto { .. }) {
                        *capture = CaptureConfig::ComplexStack {
                            focus_count: result.focus_steps,
                            focus_step_size: None,
                            hdr_stops: brackets.clone(),
                        };
                        *q = self.quality;
                    }
                }
            }
        }

        ctx.bus.publish(SystemEvent::SystemMessage(
            "Switched to SD Card Storage.".into(),
            LogLevel::Info,
        ));

        Ok(true)
    }
}

pub struct StartProcessingTask;

#[async_trait::async_trait]
impl ScanTask for StartProcessingTask {
    fn name(&self) -> &'static str {
        "StartProcessing"
    }

    async fn on_enter(&self, ctx: &DirectorContext) -> Result<bool> {
        ctx.bus.publish(SystemEvent::SystemMessage(
            "Starting Unified Reconstruction...".into(),
            LogLevel::Info,
        ));

        if let Some(scheduler) = &ctx.scheduler {
            let proj = ctx.project.lock().await;
            if let Some(p) = proj.as_ref() {
                use crate::reconstruction::job::{UnifiedJob, UnifiedJobType};

                let job = UnifiedJob::new(p.root_path.clone(), UnifiedJobType::GaussianSplatting)
                    .with_livescan(p.root_path.join("livescan.json"))
                    .with_dslr(p.root_path.join("raw/images"));

                let _ = scheduler.submit(job).await;
                ctx.bus.publish(SystemEvent::SystemMessage(
                    "Unified processing started.".into(),
                    LogLevel::Info,
                ));
            }
        }
        Ok(true)
    }
}

pub struct SmartScanTask {
    pub quality: QualityLevel,
    pub capture: CaptureConfig,
    pub step_idx: usize,
}

#[async_trait::async_trait]
impl ScanTask for SmartScanTask {
    fn name(&self) -> &'static str {
        "SmartScan"
    }

    async fn on_enter(&self, ctx: &DirectorContext) -> Result<bool> {
        let mut strat = ctx.strategy.lock().await;
        *strat = SmartScanStrategy::new(self.quality);
        ctx.bus.publish(SystemEvent::SystemMessage(
            format!("Starting Smart Scan ({:?})", self.quality),
            LogLevel::Info,
        ));
        ctx.bus
            .publish(SystemEvent::CaptureStarted(self.step_idx as u32));
        Ok(false) // Wait for frames to drive the scan
    }

    async fn on_frame(
        &self,
        ctx: &DirectorContext,
        _frame: &ImageBuffer<Rgb<u8>, Vec<u8>>,
    ) -> Result<bool> {
        let mut strat = ctx.strategy.lock().await;

        if let Some(next_angle) = strat.next_angle() {
            // Rotate
            {
                let mut tt = ctx.turntable.lock().await;
                if let Err(e) = tt.rotate_to(next_angle).await {
                    ctx.bus.publish(SystemEvent::SystemMessage(
                        format!("Turntable error: {}", e),
                        LogLevel::Error,
                    ));
                    return Ok(false); // Check if we should error hard?
                }
            }

            // Capture
            self.do_capture_sequence(ctx, &self.capture, next_angle)
                .await?;

            strat.visit(next_angle);
            ctx.bus.publish(SystemEvent::CaptureProgress(
                self.step_idx as u32,
                next_angle / 360.0,
            ));
            Ok(false) // Continue
        } else {
            Ok(true) // Done with this scan
        }
    }
}

pub struct BackgroundScanTask {
    pub step_idx: usize,
}

#[async_trait::async_trait]
impl ScanTask for BackgroundScanTask {
    fn name(&self) -> &'static str {
        "BackgroundScan"
    }

    async fn on_enter(&self, ctx: &DirectorContext) -> Result<bool> {
        let mut strat = ctx.strategy.lock().await;
        *strat = SmartScanStrategy::new(QualityLevel::Preview);
        ctx.bus.publish(SystemEvent::SystemMessage(
            "Capturing Background...".into(),
            LogLevel::Info,
        ));
        Ok(false)
    }

    async fn on_frame(
        &self,
        ctx: &DirectorContext,
        frame: &ImageBuffer<Rgb<u8>, Vec<u8>>,
    ) -> Result<bool> {
        // Save reference on first frame if missing
        {
            let mut bg_ref = ctx.background_reference.lock().await;
            if bg_ref.is_none() {
                *bg_ref = Some(frame.clone());
            }
        }

        let mut strat = ctx.strategy.lock().await;
        if let Some(next_angle) = strat.next_angle() {
            {
                let mut tt = ctx.turntable.lock().await;
                if let Err(e) = tt.rotate_to(next_angle).await {
                    ctx.bus.publish(SystemEvent::SystemMessage(
                        format!("Turntable error: {}", e),
                        LogLevel::Error,
                    ));
                    return Ok(false);
                }
            }

            let config = CaptureConfig::Single;
            // We reuse the capture logic from SmartScanTask or helper?
            // We need to move `do_capture_sequence` to shared helper or impl here.
            // Impl here for now.

            // ... Capture Logic ...
            // (Simplified for this task)
            perform_capture(ctx, &config, next_angle).await?;

            strat.visit(next_angle);
            ctx.bus.publish(SystemEvent::CaptureProgress(
                self.step_idx as u32,
                next_angle / 360.0,
            ));
            Ok(false)
        } else {
            Ok(true)
        }
    }
}

impl SmartScanTask {
    async fn do_capture_sequence(
        &self,
        ctx: &DirectorContext,
        config: &CaptureConfig,
        angle: f32,
    ) -> Result<()> {
        perform_capture(ctx, config, angle).await
    }
}

// Shared helper
async fn perform_capture(ctx: &DirectorContext, config: &CaptureConfig, angle: f32) -> Result<()> {
    // 1. Live Feedback
    let _live_res = {
        let mgr = ctx.cameras.lock().await;
        mgr.trigger_group(CameraRole::LiveFeedback, &CameraConfig::default())
    };

    // 2. High Res
    // ... Logic from Director::do_capture_sequence ...
    // Note: Simplified for brevity, assume similar structure to original code

    let output_dir = {
        let project = ctx.project.lock().await;
        if let Some(p) = project.as_ref() {
            p.root_path.join("raw/images")
        } else {
            anyhow::bail!("No active project loaded for capture");
        }
    };
    tokio::fs::create_dir_all(&output_dir).await?;

    let mgr = ctx.cameras.lock().await;
    let mut active = Vec::new();
    for cam in &mgr.cameras {
        if let Some(profile) = mgr.registry.get_profile(&cam.id()) {
            if profile.role == CameraRole::HighResCapture {
                active.push(cam.clone());
            }
        }
    }
    if active.is_empty() {
        for cam in &mgr.cameras {
            if let Some(profile) = mgr.registry.get_profile(&cam.id()) {
                if profile.role == CameraRole::LiveFeedback {
                    active.push(cam.clone());
                }
            }
        }
    }
    if active.is_empty() {
        anyhow::bail!("No active cameras available for capture");
    }

    let active_ids: Vec<String> = active.iter().map(|c| c.id()).collect();
    let _ = write_calibration_overrides(&output_dir, &mgr, &active_ids).await;

    let mut captured_ids = Vec::new();
    let mut captured_count = 0usize;
    for cam in &active {
        let capture_config = CameraConfig {
            capture_target: Some("Memory Card".to_string()),
            ..Default::default()
        };
        match cam.capture(&capture_config) {
            Ok(path) => {
                if path.exists() {
                    let id_safe = sanitize_camera_id(&cam.id());
                    let filename = format!(
                        "{}__{}_{}",
                        id_safe,
                        Utc::now().timestamp_millis(),
                        path.file_name().unwrap_or_default().to_string_lossy()
                    );
                    let dest = output_dir.join(filename);
                    if move_capture_file(&path, &dest).await.is_ok() {
                        captured_ids.push(cam.id());
                        captured_count += 1;
                    }
                }
            }
            Err(e) => {
                ctx.bus.publish(SystemEvent::SystemMessage(
                    format!("Capture failed for {}: {}", cam.id(), e),
                    LogLevel::Warning,
                ));
            }
        }
    }
    drop(mgr);

    if captured_count == 0 {
        anyhow::bail!("All camera captures failed");
    }

    // 3. Log
    let mut session = ctx.session.lock().await;
    if let Some(s) = session.as_mut() {
        s.log_capture(
            angle,
            captured_ids.clone(),
            &format!("{:?}", config),
            captured_count,
        );
        let event_idx = s.events.len().saturating_sub(1);
        drop(session);
        let (verified_count, hashes) = verify_capture(&output_dir, &captured_ids).await?;
        let mut session = ctx.session.lock().await;
        if let Some(s) = session.as_mut() {
            s.record_verification(event_idx, verified_count, hashes);
        }
    }

    Ok(())
}

async fn verify_capture(output_dir: &Path, camera_ids: &[String]) -> Result<(usize, Vec<String>)> {
    let mut hashes = Vec::new();
    let mut verified = 0usize;
    let mut pending: Vec<PathBuf> = Vec::new();

    for id in camera_ids {
        let prefix = format!("{}__", sanitize_camera_id(id));
        let mut newest: Option<(PathBuf, std::time::SystemTime)> = None;
        let mut entries = tokio::fs::read_dir(output_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                if name.starts_with(&prefix) {
                    let meta = entry.metadata().await?;
                    let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    if newest.as_ref().map(|(_, t)| &modified > t).unwrap_or(true) {
                        newest = Some((path.clone(), modified));
                    }
                }
            }
        }
        if let Some((path, _)) = newest {
            pending.push(path);
        }
    }

    if pending.is_empty() {
        anyhow::bail!("No captured files found for verification");
    }

    for path in pending {
        let mut file = tokio::fs::File::open(&path).await?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = tokio::io::AsyncReadExt::read(&mut file, &mut buf).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let digest = hasher.finalize();
        hashes.push(hex::encode(digest));
        verified += 1;
    }

    if verified < camera_ids.len() {
        anyhow::bail!(
            "Capture verification incomplete: expected {}, verified {}",
            camera_ids.len(),
            verified
        );
    }
    Ok((verified, hashes))
}

async fn write_calibration_overrides(
    output_dir: &Path,
    mgr: &trueshot_device_manager::CameraManager,
    active_ids: &[String],
) -> Result<()> {
    let mut wrote_any = false;
    for id in active_ids {
        let profile = match mgr.registry.get_profile(id) {
            Some(p) => p,
            None => continue,
        };
        let cal = match &profile.calibration {
            Some(c) => c,
            None => continue,
        };
        let (intrinsics, distortion) = match (&cal.intrinsics, &cal.distortion) {
            (Some(i), Some(d)) if i.len() >= 9 => (i, d),
            _ => continue,
        };
        let payload = serde_json::json!({
            "camera_id": id,
            "camera_matrix": intrinsics,
            "dist_coeffs": distortion,
            "rms_error": cal.rms_error,
            "width": cal.image_width,
            "height": cal.image_height,
            "updated_at": cal.last_calibrated,
        });
        let filename = format!("calibration_{}.json", sanitize_camera_id(id));
        let path = output_dir.join(filename);
        tokio::fs::write(&path, serde_json::to_string_pretty(&payload)?).await?;
        wrote_any = true;
    }

    if wrote_any {
        let candidates = active_ids
            .iter()
            .filter_map(|id| {
                mgr.registry
                    .get_profile(id)
                    .and_then(|p| p.calibration.as_ref())
            })
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            let cal = candidates[0];
            let (intrinsics, distortion) = match (&cal.intrinsics, &cal.distortion) {
                (Some(i), Some(d)) if i.len() >= 9 => (i, d),
                _ => return Ok(()),
            };
            let payload = serde_json::json!({
                "camera_matrix": intrinsics,
                "dist_coeffs": distortion,
                "rms_error": cal.rms_error,
                "width": cal.image_width,
                "height": cal.image_height,
                "updated_at": cal.last_calibrated,
            });
            let path = output_dir.join("calibration.json");
            tokio::fs::write(&path, serde_json::to_string_pretty(&payload)?).await?;
        }
    }

    Ok(())
}

async fn move_capture_file(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    match tokio::fs::rename(src, dst).await {
        Ok(_) => Ok(()),
        Err(_) => {
            tokio::fs::copy(src, dst).await?;
            tokio::fs::remove_file(src).await?;
            Ok(())
        }
    }
}

fn sanitize_camera_id(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
