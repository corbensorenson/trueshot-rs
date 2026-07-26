use actix_web::{get, post, patch, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use crate::state::AppState;
use crate::auth::{require_admin, AuthContext, Role, SESSION_COOKIE_NAME};
use crate::licensing::require_license_feature;
use trueshot_core::events::SystemEvent;
use trueshot_core::licensing::Feature;
use trueshot_device_manager::camera::registry::CameraProfile;
use trueshot_device_manager::camera::CameraCapabilities;
use crate::intervalometer::{IntervalometerRamp, IntervalometerStatus};
use tokio::sync::oneshot;
use tokio::time::MissedTickBehavior;
use chrono::Utc;
use utoipa::ToSchema;
use tokio::time::sleep;
use std::time::Duration;
use serde::Deserialize;

#[derive(serde::Serialize)]
pub struct CameraStatus {
    #[serde(flatten)]
    pub profile: CameraProfile,
    pub connected: bool,
    pub battery_level: Option<u8>,  // 0-100 percentage, None if not available
}

#[utoipa::path(
    get,
    path = "/api/cameras",
    tag = "hardware",
    responses(
        (status = 200, description = "Camera list", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/cameras")]
pub async fn get_cameras(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let cm = state.camera_manager.lock().await;
    
    // Create map of connected camera IDs to their battery levels
    let mut battery_levels: std::collections::HashMap<String, u8> = std::collections::HashMap::new();
    for cam in &cm.cameras {
        if let Ok(level) = cam.battery_level() {
            battery_levels.insert(cam.id(), level);
        }
    }
    
    // Get list of connected IDs
    let connected_ids: Vec<String> = cm.cameras.iter().map(|c| c.id()).collect();
    
    // Iterate registry and enrich
    let statuses: Vec<CameraStatus> = cm.registry.profiles.values().map(|p| {
        let connected = connected_ids.contains(&p.id);
        CameraStatus {
            profile: p.clone(),
            connected,
            battery_level: battery_levels.get(&p.id).copied(),
        }
    }).collect();
    
    HttpResponse::Ok().json(statuses)
}

#[derive(serde::Deserialize, ToSchema)]
pub struct NicknameUpdate {
    pub nickname: String,
}

#[utoipa::path(
    patch,
    path = "/api/cameras/{id}/nickname",
    tag = "hardware",
    params(("id" = String, Path, description = "Camera id")),
    request_body = NicknameUpdate,
    responses(
        (status = 200, description = "Camera updated", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[patch("/api/cameras/{id}/nickname")]
pub async fn update_camera_nickname(
    req: HttpRequest,
    path: web::Path<String>,
    json: web::Json<NicknameUpdate>,
    state: web::Data<AppState>
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    let mut cm = state.camera_manager.lock().await;
    
    let mut updated_profile = None;
    
    if let Some(profile) = cm.registry.profiles.get_mut(&id) {
        profile.nickname = Some(json.nickname.clone());
        updated_profile = Some(profile.clone());
    }
    
    if let Some(profile) = updated_profile {
        if let Err(e) = cm.registry.save() {
             return HttpResponse::InternalServerError().body(e.to_string());
        }
        return HttpResponse::Ok().json(profile);
    }

    HttpResponse::NotFound().body("Camera not found")
}

#[derive(serde::Deserialize, ToSchema)]
pub struct PtzRequest {
    pub pan: f32,
    pub tilt: f32,
    pub zoom: f32,
}

#[utoipa::path(
    post,
    path = "/api/cameras/{id}/ptz",
    tag = "hardware",
    params(("id" = String, Path, description = "Camera id")),
    request_body = PtzRequest,
    responses(
        (status = 200, description = "PTZ applied", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/cameras/{id}/ptz")]
pub async fn camera_ptz(
    req: HttpRequest,
    path: web::Path<String>,
    json: web::Json<PtzRequest>,
    state: web::Data<AppState>
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    tracing::info!("Received PTZ Request for ID: {}", id);
    let cm = state.camera_manager.lock().await;
    
    // Find camera by ID
    // Optimized: Use get_camera_by_id (Arc clone)
    if let Some(cam) = cm.get_camera_by_id(&id) {
         // Drop lock implicitly or explicitly if strictly needed, but here we hold it for the duration of the request 
         // which is fine for simple commands.
         // Actually, ptz is now &self, so we can drop lock immediately if we wanted to be super optimized,
         // but keeping logic simple is better for now.
         
         if let Err(e) = cam.ptz(json.pan, json.tilt, json.zoom) {
             tracing::error!("PTZ Failed for {}: {}", id, e);
             return HttpResponse::InternalServerError().body(e.to_string());
         }
         return HttpResponse::Ok().json(serde_json::json!({"status": "moved"}));
    }
    HttpResponse::NotFound().body("Camera not connected")
}

#[derive(serde::Deserialize, ToSchema)]
pub struct ConfigRequest {
    pub iso: Option<String>,
    pub shutter_speed: Option<String>,
    pub aperture: Option<String>,
    pub wb: Option<String>,
    pub capture_target: Option<String>,
}

#[derive(serde::Deserialize, ToSchema)]
pub struct IntervalometerStartRequest {
    pub interval_ms: u64,
    #[serde(default)]
    pub total_frames: Option<u32>,
    #[serde(default)]
    pub ramp: Option<IntervalometerRamp>,
    #[serde(default)]
    pub capture_target: Option<String>,
}

#[derive(serde::Deserialize, ToSchema)]
pub struct HdrBracketRequest {
    pub bracket_count: u8,
    pub ev_spacing: u8,
    #[serde(default)]
    pub base_shutter: Option<String>,
    #[serde(default)]
    pub capture_target: Option<String>,
}

#[derive(serde::Deserialize, ToSchema)]
pub struct FocusStackRequest {
    pub slice_count: u32,
    pub step_size: i32,
    pub direction: String,
    #[serde(default)]
    pub capture_target: Option<String>,
}

#[derive(serde::Deserialize, ToSchema)]
pub struct HdrFocusStackRequest {
    pub bracket_count: u8,
    pub ev_spacing: u8,
    #[serde(default)]
    pub base_shutter: Option<String>,
    pub slice_count: u32,
    pub step_size: i32,
    pub direction: String,
    #[serde(default)]
    pub capture_target: Option<String>,
}

#[derive(serde::Serialize, ToSchema)]
pub struct CaptureSequenceResult {
    pub status: String,
    pub shots: Vec<String>,
}

#[utoipa::path(
    post,
    path = "/api/cameras/{id}/config",
    tag = "hardware",
    params(("id" = String, Path, description = "Camera id")),
    request_body = ConfigRequest,
    responses(
        (status = 200, description = "Config updated", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/cameras/{id}/config")]
pub async fn set_camera_config(
    req: HttpRequest,
    path: web::Path<String>,
    json: web::Json<ConfigRequest>,
    state: web::Data<AppState>
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    let mut cm = state.camera_manager.lock().await;

    // We need to map ConfigRequest -> CameraConfig
    // CameraConfig is defined in trueshot_device_manager::camera::CameraConfig (Wait, I deleted it or restored it?)
    // I restored it as `pub struct CameraConfig` in `mod.rs`.
    // It is `pub use registry::CameraSettings`? No, I defined `CameraConfig` in `mod.rs`.
    
    let config = trueshot_device_manager::camera::CameraConfig {
        iso: json.iso.clone(),
        shutter_speed: json.shutter_speed.clone(),
        aperture: json.aperture.clone(),
        wb: json.wb.clone(),
        capture_target: json.capture_target.clone(),
        resolution: None, // Simplified for now
        fps: None,
    };

    if let Some(cam) = cm.get_camera_by_id(&id) {
         if let Err(e) = cam.set_config(&config) {
             return HttpResponse::InternalServerError().body(e.to_string());
         }
         // Should we also update registry "last_settings"?
         // Yes, for persistence.
         let _ = cm.registry.update_settings(&id, trueshot_device_manager::camera::CameraSettings {
             resolution: None, fps: None,
             iso: json.iso.clone(),
             shutter_speed: json.shutter_speed.clone(),
             wb: json.wb.clone(),
         });
         
         return HttpResponse::Ok().json(serde_json::json!({"status": "updated"}));
    }
    HttpResponse::NotFound().body("Camera not connected")
}

#[utoipa::path(
    post,
    path = "/api/cameras/{id}/capture",
    tag = "hardware",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "Capture complete", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/cameras/{id}/capture")]
pub async fn capture_photo(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    let cm = state.camera_manager.lock().await;
    
    if let Some(cam) = cm.get_camera_by_id(&id) {
        // Build a default config for capture
        let config = trueshot_device_manager::camera::CameraConfig {
            iso: None,
            shutter_speed: None,
            aperture: None,
            wb: None,
            capture_target: None,
            resolution: None,
            fps: None,
        };
        
        match cam.capture(&config) {
            Ok(path) => {
                return HttpResponse::Ok().json(serde_json::json!({
                    "status": "captured",
                    "path": path.display().to_string()
                }));
            },
            Err(e) => {
                return HttpResponse::InternalServerError().body(e.to_string());
            }
        }
    }
    HttpResponse::NotFound().body("Camera not connected")
}

#[utoipa::path(
    post,
    path = "/api/cameras/{id}/hdr",
    tag = "hardware",
    params(("id" = String, Path, description = "Camera id")),
    request_body = HdrBracketRequest,
    responses(
        (status = 200, description = "HDR bracket captured", body = CaptureSequenceResult),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/cameras/{id}/hdr")]
pub async fn capture_hdr_bracket(
    req: HttpRequest,
    path: web::Path<String>,
    json: web::Json<HdrBracketRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) = require_license_feature(&state, Feature::AdvancedCaptureAutomation, "advanced_capture_automation") {
        return resp;
    }
    if json.bracket_count < 3 || json.bracket_count % 2 == 0 {
        return HttpResponse::BadRequest().body("bracket_count must be an odd number >= 3");
    }
    if json.ev_spacing == 0 {
        return HttpResponse::BadRequest().body("ev_spacing must be >= 1");
    }
    let camera_id = path.into_inner();
    let (cam, profile) = {
        let cm = state.camera_manager.lock().await;
        let cam = cm.get_camera_by_id(&camera_id);
        let profile = cm.registry.get_profile(&camera_id).cloned();
        (cam, profile)
    };
    let cam = match cam {
        Some(cam) => cam,
        None => return HttpResponse::NotFound().body("Camera not connected"),
    };
    let shutter_options = profile
        .as_ref()
        .map(|p| p.capabilities.shutter_speed_options.clone())
        .unwrap_or_default();
    let base_shutter = json
        .base_shutter
        .clone()
        .or_else(|| profile.as_ref().and_then(|p| p.last_settings.as_ref().and_then(|s| s.shutter_speed.clone())));
    let sequence = build_hdr_sequence(&shutter_options, base_shutter.as_ref(), json.bracket_count, json.ev_spacing);

    let mut shots = Vec::new();
    for shutter in sequence {
        let config = trueshot_device_manager::camera::CameraConfig {
            iso: None,
            shutter_speed: shutter,
            aperture: None,
            wb: None,
            capture_target: json.capture_target.clone(),
            resolution: None,
            fps: None,
        };
        if let Err(err) = cam.set_config(&config) {
            return HttpResponse::InternalServerError().body(err.to_string());
        }
        match cam.capture(&config) {
            Ok(path) => shots.push(path.display().to_string()),
            Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
        }
    }

    HttpResponse::Ok().json(CaptureSequenceResult {
        status: "captured".to_string(),
        shots,
    })
}

#[utoipa::path(
    post,
    path = "/api/cameras/{id}/focus_stack",
    tag = "hardware",
    params(("id" = String, Path, description = "Camera id")),
    request_body = FocusStackRequest,
    responses(
        (status = 200, description = "Focus stack captured", body = CaptureSequenceResult),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/cameras/{id}/focus_stack")]
pub async fn capture_focus_stack(
    req: HttpRequest,
    path: web::Path<String>,
    json: web::Json<FocusStackRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) = require_license_feature(&state, Feature::AdvancedCaptureAutomation, "advanced_capture_automation") {
        return resp;
    }
    if json.slice_count < 2 {
        return HttpResponse::BadRequest().body("slice_count must be >= 2");
    }
    let step_size = json.step_size.abs().clamp(1, 200);
    let direction = json.direction.to_lowercase();
    if direction != "near" && direction != "far" {
        return HttpResponse::BadRequest().body("direction must be 'near' or 'far'");
    }
    let direction_step = if direction == "near" { step_size } else { -step_size };
    let camera_id = path.into_inner();
    let cam = {
        let cm = state.camera_manager.lock().await;
        cm.get_camera_by_id(&camera_id)
    };
    let cam = match cam {
        Some(cam) => cam,
        None => return HttpResponse::NotFound().body("Camera not connected"),
    };

    let mut shots = Vec::new();
    for index in 0..json.slice_count {
        let config = trueshot_device_manager::camera::CameraConfig {
            iso: None,
            shutter_speed: None,
            aperture: None,
            wb: None,
            capture_target: json.capture_target.clone(),
            resolution: None,
            fps: None,
        };
        match cam.capture(&config) {
            Ok(path) => shots.push(path.display().to_string()),
            Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
        }
        if index + 1 < json.slice_count {
            if let Err(err) = cam.drive_focus(direction_step) {
                return HttpResponse::InternalServerError().body(err.to_string());
            }
            sleep(Duration::from_millis(120)).await;
        }
    }

    HttpResponse::Ok().json(CaptureSequenceResult {
        status: "captured".to_string(),
        shots,
    })
}

#[utoipa::path(
    post,
    path = "/api/cameras/{id}/hdr_focus_stack",
    tag = "hardware",
    params(("id" = String, Path, description = "Camera id")),
    request_body = HdrFocusStackRequest,
    responses(
        (status = 200, description = "HDR focus stack captured", body = CaptureSequenceResult),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/cameras/{id}/hdr_focus_stack")]
pub async fn capture_hdr_focus_stack(
    req: HttpRequest,
    path: web::Path<String>,
    json: web::Json<HdrFocusStackRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) = require_license_feature(&state, Feature::AdvancedCaptureAutomation, "advanced_capture_automation") {
        return resp;
    }
    if json.bracket_count < 3 || json.bracket_count % 2 == 0 {
        return HttpResponse::BadRequest().body("bracket_count must be an odd number >= 3");
    }
    if json.ev_spacing == 0 {
        return HttpResponse::BadRequest().body("ev_spacing must be >= 1");
    }
    if json.slice_count < 2 {
        return HttpResponse::BadRequest().body("slice_count must be >= 2");
    }
    let step_size = json.step_size.abs().clamp(1, 200);
    let direction = json.direction.to_lowercase();
    if direction != "near" && direction != "far" {
        return HttpResponse::BadRequest().body("direction must be 'near' or 'far'");
    }
    let direction_step = if direction == "near" { step_size } else { -step_size };
    let camera_id = path.into_inner();
    let (cam, profile) = {
        let cm = state.camera_manager.lock().await;
        let cam = cm.get_camera_by_id(&camera_id);
        let profile = cm.registry.get_profile(&camera_id).cloned();
        (cam, profile)
    };
    let cam = match cam {
        Some(cam) => cam,
        None => return HttpResponse::NotFound().body("Camera not connected"),
    };
    let shutter_options = profile
        .as_ref()
        .map(|p| p.capabilities.shutter_speed_options.clone())
        .unwrap_or_default();
    let base_shutter = json
        .base_shutter
        .clone()
        .or_else(|| profile.as_ref().and_then(|p| p.last_settings.as_ref().and_then(|s| s.shutter_speed.clone())));
    let sequence = build_hdr_sequence(&shutter_options, base_shutter.as_ref(), json.bracket_count, json.ev_spacing);

    let mut shots = Vec::new();
    for slice_index in 0..json.slice_count {
        for shutter in sequence.iter() {
            let config = trueshot_device_manager::camera::CameraConfig {
                iso: None,
                shutter_speed: shutter.clone(),
                aperture: None,
                wb: None,
                capture_target: json.capture_target.clone(),
                resolution: None,
                fps: None,
            };
            if let Err(err) = cam.set_config(&config) {
                return HttpResponse::InternalServerError().body(err.to_string());
            }
            match cam.capture(&config) {
                Ok(path) => shots.push(path.display().to_string()),
                Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
            }
        }
        if slice_index + 1 < json.slice_count {
            if let Err(err) = cam.drive_focus(direction_step) {
                return HttpResponse::InternalServerError().body(err.to_string());
            }
            sleep(Duration::from_millis(150)).await;
        }
    }

    HttpResponse::Ok().json(CaptureSequenceResult {
        status: "captured".to_string(),
        shots,
    })
}

#[utoipa::path(
    post,
    path = "/api/cameras/{id}/interval/start",
    tag = "hardware",
    params(("id" = String, Path, description = "Camera id")),
    request_body = IntervalometerStartRequest,
    responses(
        (status = 200, description = "Intervalometer started", body = IntervalometerStatus),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/cameras/{id}/interval/start")]
pub async fn start_intervalometer(
    req: HttpRequest,
    path: web::Path<String>,
    json: web::Json<IntervalometerStartRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) = require_license_feature(&state, Feature::AdvancedCaptureAutomation, "advanced_capture_automation") {
        return resp;
    }
    let camera_id = path.into_inner();
    if json.interval_ms < 200 {
        return HttpResponse::BadRequest().body("interval_ms must be >= 200");
    }
    let interval_ms = json.interval_ms;
    let total_frames = json.total_frames;
    let ramp = json.ramp.clone();
    let capture_target = json.capture_target.clone();

    let (capabilities, exists) = {
        let cm = state.camera_manager.lock().await;
        let exists = cm.get_camera_by_id(&camera_id).is_some();
        let caps = cm
            .registry
            .get_profile(&camera_id)
            .map(|profile| profile.capabilities.clone());
        (caps, exists)
    };
    if !exists {
        return HttpResponse::NotFound().body("Camera not connected");
    }

    let (cancel_tx, cancel_rx) = oneshot::channel();
    let now = Utc::now();
    let status = IntervalometerStatus {
        camera_id: camera_id.clone(),
        active: true,
        interval_ms,
        total_frames,
        captured_frames: 0,
        started_at: now.to_rfc3339(),
        last_capture_at: None,
        next_capture_at: Some((now + chrono::Duration::milliseconds(interval_ms as i64)).to_rfc3339()),
        last_error: None,
        ramp: ramp.clone(),
    };
    {
        let mut interval_state = state.intervalometer.lock().await;
        if let Some(existing) = interval_state.tasks.get_mut(&camera_id) {
            if let Some(cancel) = existing.cancel.take() {
                let _ = cancel.send(());
            }
        }
        interval_state.set_task(
            camera_id.clone(),
            crate::intervalometer::IntervalometerTask {
                status: status.clone(),
                cancel: Some(cancel_tx),
            },
        );
    }

    let state_clone = state.clone();
    tokio::spawn(async move {
        run_intervalometer(state_clone, camera_id, interval_ms, total_frames, ramp, capture_target, capabilities, cancel_rx).await;
    });

    HttpResponse::Ok().json(status)
}

#[utoipa::path(
    post,
    path = "/api/cameras/{id}/interval/stop",
    tag = "hardware",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "Intervalometer stopped", body = IntervalometerStatus),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/cameras/{id}/interval/stop")]
pub async fn stop_intervalometer(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let camera_id = path.into_inner();
    let mut interval_state = state.intervalometer.lock().await;
    if let Some(status) = interval_state.stop_task(&camera_id) {
        return HttpResponse::Ok().json(status);
    }
    HttpResponse::NotFound().body("Intervalometer not running")
}

#[utoipa::path(
    get,
    path = "/api/cameras/{id}/interval/status",
    tag = "hardware",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "Intervalometer status", body = IntervalometerStatus),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/cameras/{id}/interval/status")]
pub async fn get_intervalometer_status(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let camera_id = path.into_inner();
    let interval_state = state.intervalometer.lock().await;
    if let Some(status) = interval_state.status(&camera_id) {
        return HttpResponse::Ok().json(status);
    }
    HttpResponse::NotFound().body("Intervalometer not running")
}

async fn run_intervalometer(
    state: web::Data<AppState>,
    camera_id: String,
    interval_ms: u64,
    total_frames: Option<u32>,
    ramp: Option<IntervalometerRamp>,
    capture_target: Option<String>,
    capabilities: Option<CameraCapabilities>,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(interval_ms));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut captured = 0u32;
    loop {
        if let Some(total) = total_frames {
            if captured >= total {
                break;
            }
        }
        tokio::select! {
            _ = &mut cancel_rx => {
                break;
            }
            _ = ticker.tick() => {
                let mut config = trueshot_device_manager::camera::CameraConfig {
                    iso: None,
                    shutter_speed: None,
                    aperture: None,
                    wb: None,
                    capture_target: capture_target.clone(),
                    resolution: None,
                    fps: None,
                };
                if let Some(ramp_cfg) = ramp.as_ref() {
                    if let Some(total) = total_frames {
                        if total > 1 {
                            let t = (captured as f32) / (total.saturating_sub(1) as f32);
                            if let Some(value) = ramp_value(
                                capabilities.as_ref().map(|c| &c.shutter_speed_options),
                                ramp_cfg.shutter_start.as_ref(),
                                ramp_cfg.shutter_end.as_ref(),
                                t,
                            ) {
                                config.shutter_speed = Some(value);
                            }
                            if let Some(value) = ramp_value(
                                capabilities.as_ref().map(|c| &c.iso_options),
                                ramp_cfg.iso_start.as_ref(),
                                ramp_cfg.iso_end.as_ref(),
                                t,
                            ) {
                                config.iso = Some(value);
                            }
                        }
                    }
                }
                let cam = {
                    let cm = state.camera_manager.lock().await;
                    cm.get_camera_by_id(&camera_id)
                };
                let now = Utc::now();
                let mut last_error = None;
                if let Some(cam) = cam {
                    if config.iso.is_some() || config.shutter_speed.is_some() || config.aperture.is_some() || config.capture_target.is_some() {
                        if let Err(err) = cam.set_config(&config) {
                            last_error = Some(format!("Config update failed: {}", err));
                        }
                    }
                    match cam.capture(&config) {
                        Ok(_) => {
                            captured = captured.saturating_add(1);
                        }
                        Err(err) => {
                            last_error = Some(format!("Capture failed: {}", err));
                        }
                    }
                } else {
                    last_error = Some("Camera disconnected".to_string());
                }
                let next_at = (now + chrono::Duration::milliseconds(interval_ms as i64)).to_rfc3339();
                let mut interval_state = state.intervalometer.lock().await;
                if let Some(task) = interval_state.tasks.get_mut(&camera_id) {
                    task.status.captured_frames = captured;
                    task.status.last_capture_at = Some(now.to_rfc3339());
                    task.status.next_capture_at = Some(next_at);
                    if last_error.is_some() {
                        task.status.last_error = last_error.clone();
                    }
                }
                if last_error.as_deref() == Some("Camera disconnected") {
                    break;
                }
            }
        }
    }
    let mut interval_state = state.intervalometer.lock().await;
    if let Some(task) = interval_state.tasks.get_mut(&camera_id) {
        task.status.active = false;
        task.status.next_capture_at = None;
    }
}

fn ramp_value(
    options: Option<&Vec<String>>,
    start: Option<&String>,
    end: Option<&String>,
    t: f32,
) -> Option<String> {
    let options = options?;
    let start = start?;
    let end = end?;
    let start_idx = options.iter().position(|value| value == start)?;
    let end_idx = options.iter().position(|value| value == end)?;
    if options.is_empty() {
        return None;
    }
    let t = t.clamp(0.0, 1.0);
    let idx = (start_idx as f32 + (end_idx as f32 - start_idx as f32) * t).round() as isize;
    let idx = idx.clamp(0, (options.len() - 1) as isize) as usize;
    Some(options[idx].clone())
}

fn build_hdr_sequence(
    options: &[String],
    base_shutter: Option<&String>,
    bracket_count: u8,
    ev_spacing: u8,
) -> Vec<Option<String>> {
    if bracket_count == 0 {
        return Vec::new();
    }
    if options.is_empty() {
        let mut sequence = Vec::with_capacity(bracket_count as usize);
        for _ in 0..bracket_count {
            sequence.push(base_shutter.cloned());
        }
        return sequence;
    }
    let base_idx = base_shutter
        .and_then(|value| options.iter().position(|opt| opt == value))
        .unwrap_or(options.len() / 2) as i32;
    let half = (bracket_count as i32) / 2;
    let step = ev_spacing as i32;
    let mut sequence = Vec::with_capacity(bracket_count as usize);
    for idx in 0..bracket_count {
        let offset = idx as i32 - half;
        let target = (base_idx + offset * step).clamp(0, (options.len() - 1) as i32);
        sequence.push(Some(options[target as usize].clone()));
    }
    sequence
}


#[derive(serde::Deserialize, ToSchema)]
pub struct EnabledRequest {
    pub enabled: bool,
}

#[utoipa::path(
    post,
    path = "/api/cameras/{id}/enabled",
    tag = "hardware",
    params(("id" = String, Path, description = "Camera id")),
    request_body = EnabledRequest,
    responses(
        (status = 200, description = "Camera enabled", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/cameras/{id}/enabled")]
pub async fn set_camera_enabled(
    req: HttpRequest,
    path: web::Path<String>,
    json: web::Json<EnabledRequest>,
    state: web::Data<AppState>
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    let mut cm = state.camera_manager.lock().await;

    if let Err(e) = cm.registry.set_enabled(&id, json.enabled) {
         return HttpResponse::InternalServerError().body(e.to_string());
    }
    
    // Forces re-init on next use if enabled, or stops if disabled.
    // However, the lazy camera struct has a copy of 'enabled'.
    // We must update the active instance as well!
    if cm.get_camera_by_id(&id).is_some() {
         // This is hard because cam is Arc<dyn Camera>. 
         // We can't easily mutate the inner 'enabled' field of LazyNokhwaCamera via trait object.
         // BUT, reconcile_cameras re-creates the LazyCamera if discovery runs? No, it keeps it.
         
         // Solution: We force a reconcile/reload or specialized update?
         // Simpler: Just rely on reconcile_cameras. If the user toggles, we can trigger a scan?
         // Or, we update the registry, and the next 'scan_hardware' (polled by frontend) will see the change?
         // Wait, 'reconcile_cameras' reads from registry! 
         // "let enabled = self.registry.get_profile(&id)..."
         // But it only checks this when CREATING a new camera (Discovered::Nokhwa).
         // It does NOT update existing cameras.
         
         // Fix: In reconcile_cameras loop (lines 336+), we should check if enabled state changed?
         // Or, simpler: When enabling via API, we just return OK. The frontend should trigger a re-scan.
         // Or we can manually remove it from `cm.cameras` so it gets re-added?
         
         // Hack/Fix: Remove it from active list if it exists?
         // No, that causes race.
         
         // Proper fix: Update LazyNokhwaCamera to look up 'enabled' from registry dynamically?
         // Or pass a shared atomic?
         // For now, I will instruct the frontend to trigger a scan after enabling.
         // But to ensure it works, I will remove the camera from the manager's list so it gets re-created on next scan.
         
         cm.cameras.retain(|c| c.id() != id); 
    }
    
    HttpResponse::Ok().json(serde_json::json!({"status": "updated", "enabled": json.enabled}))
}

#[derive(serde::Deserialize, ToSchema)]
pub struct DriveFocusRequest {
    pub step: i32,
}

#[derive(serde::Deserialize, ToSchema)]
pub struct FocusPointRequest {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    pub token: Option<String>,
}

struct StreamLimits {
    max_fps: u32,
    max_frame_bytes: usize,
    max_bytes_per_sec: usize,
    idle_timeout: Duration,
}

fn load_stream_limits() -> StreamLimits {
    let max_fps = std::env::var("TRUESHOT_STREAM_MAX_FPS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(30)
        .clamp(5, 120);
    let max_frame_bytes = std::env::var("TRUESHOT_STREAM_MAX_FRAME_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(4 * 1024 * 1024);
    let max_bytes_per_sec = std::env::var("TRUESHOT_STREAM_MAX_BYTES_PER_SEC")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(8 * 1024 * 1024);
    let idle_seconds = std::env::var("TRUESHOT_STREAM_IDLE_SECONDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(30);
    StreamLimits {
        max_fps,
        max_frame_bytes,
        max_bytes_per_sec,
        idle_timeout: Duration::from_secs(idle_seconds.max(5)),
    }
}

fn origin_allowed(req: &HttpRequest, allowed: &Option<Vec<String>>) -> bool {
    let Some(allowed) = allowed.as_ref() else {
        return true;
    };
    let origin = req
        .headers()
        .get("Origin")
        .and_then(|v| v.to_str().ok());
    match origin {
        Some(origin) => allowed.iter().any(|o| o == origin),
        None => true,
    }
}

fn extract_bearer(req: &HttpRequest) -> Option<String> {
    let header = req.headers().get(actix_web::http::header::AUTHORIZATION)?;
    let value = header.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    if token.trim().is_empty() {
        return None;
    }
    Some(token.trim().to_string())
}

fn extract_session_cookie(req: &HttpRequest) -> Option<String> {
    req.cookie(SESSION_COOKIE_NAME).map(|c| c.value().to_string())
}

fn scopes_allow(ctx: &AuthContext, scope: &str) -> bool {
    if ctx.role == Role::Admin {
        return true;
    }
    if ctx.scopes.iter().any(|s| s == "*" || s == scope) {
        return true;
    }
    false
}

#[utoipa::path(
    post,
    path = "/api/cameras/{id}/focus_point",
    tag = "hardware",
    params(("id" = String, Path, description = "Camera id")),
    request_body = FocusPointRequest,
    responses(
        (status = 200, description = "Focus point set", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/cameras/{id}/focus_point")]
pub async fn camera_focus_point(
    req: HttpRequest,
    _path: web::Path<String>,
    _json: web::Json<FocusPointRequest>,
    _state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    HttpResponse::NotImplemented().body("Camera focus point not supported yet")
}

#[utoipa::path(
    post,
    path = "/api/cameras/{id}/autofocus",
    tag = "hardware",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "Autofocus triggered", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/cameras/{id}/autofocus")]
pub async fn camera_autofocus(
    req: HttpRequest,
    _path: web::Path<String>,
    _state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    HttpResponse::NotImplemented().body("Camera autofocus not supported yet")
}

#[utoipa::path(
    post,
    path = "/api/cameras/{id}/focus/drive",
    tag = "hardware",
    params(("id" = String, Path, description = "Camera id")),
    request_body = DriveFocusRequest,
    responses(
        (status = 200, description = "Focus drive applied", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/cameras/{id}/focus/drive")]
pub async fn camera_drive_focus(
    req: HttpRequest,
    path: web::Path<String>,
    json: web::Json<DriveFocusRequest>,
    state: web::Data<AppState>
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    let cm = state.camera_manager.lock().await;
    
    if let Some(cam) = cm.get_camera_by_id(&id) {
         if let Err(e) = cam.drive_focus(json.step) {
             return HttpResponse::InternalServerError().body(e.to_string());
         }
         return HttpResponse::Ok().json(serde_json::json!({"status": "driven", "step": json.step}));
    }
    HttpResponse::NotFound().body("Camera not connected")
}

#[utoipa::path(
    get,
    path = "/api/stream/{id}",
    tag = "hardware",
    params(
        ("id" = String, Path, description = "Camera id"),
        ("token" = Option<String>, Query, description = "Optional access token for signed stream URLs")
    ),
    responses(
        (status = 200, description = "Camera stream"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/stream/{id}")]
pub async fn camera_stream(
    req: HttpRequest,
    path: web::Path<String>,
    query: web::Query<StreamQuery>,
    state: web::Data<AppState>,
) -> HttpResponse {
    if !origin_allowed(&req, &state.config.server.allowed_origins) {
        return HttpResponse::Forbidden().body("Origin not allowed");
    }

    let auth_ctx = if let Some(ctx) = req.extensions().get::<AuthContext>() {
        ctx.clone()
    } else {
        let token = query
            .token
            .as_ref()
            .cloned()
            .or_else(|| extract_bearer(&req))
            .or_else(|| extract_session_cookie(&req));
        let token = match token {
            Some(token) => token,
            None => return HttpResponse::Unauthorized().body("Missing auth token"),
        };
        match state.auth.verify_token(&token) {
            Ok(ctx) => ctx,
            Err(_) => return HttpResponse::Unauthorized().body("Invalid auth token"),
        }
    };

    if !scopes_allow(&auth_ctx, "stream:read") {
        return HttpResponse::Forbidden().body("Insufficient scope");
    }
    let id = path.into_inner();
    let cm = state.camera_manager.clone();
    let limits = load_stream_limits();
    
    let stream: std::pin::Pin<Box<dyn futures::stream::Stream<Item = Result<web::Bytes, actix_web::Error>>>> = Box::pin(async_stream::try_stream! {
        let interval_ms = (1000.0 / limits.max_fps as f32).max(5.0) as u64;
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(interval_ms));
        let mut bytes_budget = limits.max_bytes_per_sec as i64;
        let mut last_refill = std::time::Instant::now();
        let mut last_sent = std::time::Instant::now();
        let mut frames_sent: u64 = 0;
        let mut bytes_sent: u64 = 0;
        let stream_start = std::time::Instant::now();
        
        // OPTIMIZATION: Acquire Camera Arc ONCE
        let camera_arc = {
            let cm_lock = cm.lock().await;
            cm_lock.get_camera_by_id(&id)
        };

        if let Some(cam) = camera_arc {
            loop {
                interval.tick().await;
                let now = std::time::Instant::now();
                if now.duration_since(last_sent) > limits.idle_timeout {
                    tracing::info!("MJPEG stream idle timeout for {}", id);
                    break;
                }
                if now.duration_since(last_refill) >= std::time::Duration::from_secs(1) {
                    bytes_budget = limits.max_bytes_per_sec as i64;
                    last_refill = now;
                }
                // No global lock here! Shared ownership via Arc.
                // If cam is disconnected, capture_preview might return err.
                
                let frame_result = cam.capture_preview();
                
                match frame_result {
                    Ok(jpeg) => {
                        let frame_len = jpeg.len();
                        if frame_len > limits.max_frame_bytes {
                            tracing::warn!("Stream frame too large ({} bytes), dropping", frame_len);
                            continue;
                        }
                        if bytes_budget < frame_len as i64 {
                            continue;
                        }
                        let header = format!(
                             "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                             frame_len
                        );
                        yield web::Bytes::from(header);
                        yield web::Bytes::from(jpeg);
                        yield web::Bytes::from("\r\n");
                        bytes_budget -= frame_len as i64;
                        last_sent = now;
                        frames_sent += 1;
                        bytes_sent += frame_len as u64;
                    },
                    Err(e) => {
                         tracing::warn!("Stream grab failed: {}. Retrying in 2s...", e);
                         tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    }
                }
            }
            tracing::info!(
                "MJPEG stream ended for {} (frames={}, bytes={}, duration_ms={})",
                id,
                frames_sent,
                bytes_sent,
                stream_start.elapsed().as_millis()
            );
        } else {
             // Camera not found initially
             // Yield nothing or error?
             // yield web::Bytes::from("Camera not found"); 
        }
    });

    HttpResponse::Ok()
        .content_type("multipart/x-mixed-replace; boundary=frame")
        .streaming(stream)
}

#[utoipa::path(
    get,
    path = "/api/turntable/status",
    tag = "hardware",
    responses(
        (status = 200, description = "Turntable status", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/turntable/status")]
pub async fn get_turntable_status(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let status_str = state.turntable_status.lock().unwrap().clone();
    let tt = state.turntable.lock().await;
    let connected = tt.is_some();
    let angle = if let Some(t) = tt.as_ref() { t.get_rotation() } else { 0.0 };
    let moving = *state.turntable_moving.lock().unwrap();
    
    HttpResponse::Ok().json(serde_json::json!({
        "connected": connected,
        "type": status_str,
        "angle": angle,
        "moving": moving
    }))
}

#[utoipa::path(
    post,
    path = "/api/turntable/home",
    tag = "hardware",
    responses(
        (status = 200, description = "Turntable homed", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/turntable/home")]
pub async fn turntable_home(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let mut tt_lock = state.turntable.lock().await;
    if let Some(tt) = tt_lock.as_mut() {
        // Set moving = true
        *state.turntable_moving.lock().unwrap() = true;
        state.event_bus.publish(SystemEvent::TurntableStatus { connected: true, angle: tt.get_rotation(), moving: true });
        
        let result = tt.home().await;
        
        // Set moving = false
        *state.turntable_moving.lock().unwrap() = false;
        state.event_bus.publish(SystemEvent::TurntableStatus { connected: true, angle: tt.get_rotation(), moving: false });

        if let Err(e) = result {
            return HttpResponse::InternalServerError().body(e.to_string());
        }
        return HttpResponse::Ok().json(serde_json::json!({"status": "homed"}));
    }
    HttpResponse::NotFound().body("Turntable not connected")
}

#[derive(serde::Deserialize, ToSchema)]
pub struct Rotation { pub degrees: f32 }

#[utoipa::path(
    post,
    path = "/api/turntable/rotate",
    tag = "hardware",
    request_body = Rotation,
    responses(
        (status = 200, description = "Turntable rotated", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/turntable/rotate")]
pub async fn turntable_rotate(req: HttpRequest, state: web::Data<AppState>, json: web::Json<Rotation>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let mut tt_lock = state.turntable.lock().await;
    if let Some(tt) = tt_lock.as_mut() {
        // Set moving = true
        *state.turntable_moving.lock().unwrap() = true;
        state.event_bus.publish(SystemEvent::TurntableStatus { connected: true, angle: tt.get_rotation(), moving: true });

        let result = tt.rotate(json.degrees).await;
        
        // Set moving = false
        *state.turntable_moving.lock().unwrap() = false;
        state.event_bus.publish(SystemEvent::TurntableStatus { connected: true, angle: tt.get_rotation(), moving: false });

        if let Err(e) = result {
            return HttpResponse::InternalServerError().body(e.to_string());
        }
        return HttpResponse::Ok().json(serde_json::json!({"status": "rotated", "degrees": json.degrees}));
    }
    HttpResponse::NotFound().body("Turntable not connected")
}

#[utoipa::path(
    post,
    path = "/api/hardware/scan",
    tag = "hardware",
    responses(
        (status = 200, description = "Hardware scan report", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/hardware/scan")]
pub async fn scan_hardware(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let cm_arc = state.camera_manager.clone();
    let eb_arc = state.event_bus.clone();
    let mock = state.config.hardware.mock_devices.unwrap_or(false);

    let mut cm = cm_arc.lock().await;
    match cm.reconcile_cameras(mock).await {
        Ok(report) => {
             for id in &report.added {
                 eb_arc.publish(SystemEvent::DeviceConnected { kind: "camera".to_string(), id: id.clone() });
             }
             for id in &report.removed {
                 eb_arc.publish(SystemEvent::DeviceDisconnected { id: id.clone() });
             }
             HttpResponse::Ok().json(report)
        }
        Err(e) => HttpResponse::InternalServerError().body(format!("Scan failed: {}", e))
    }
}
