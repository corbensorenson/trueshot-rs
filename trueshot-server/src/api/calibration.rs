use actix_web::{delete, get, post, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use utoipa::{IntoParams, ToSchema};

use crate::audit::AuditEvent;
use crate::auth::require_admin;
use crate::config::AppConfig;
use crate::fs_safety::{
    ensure_project_directory, open_project_file_read, remove_project_file_if_exists,
    stage_project_file, write_project_file_atomic,
};
use crate::state::{AppState, CalibrationSession};
use image::ImageReader;
use ndarray::Array3;
use std::io::BufReader;
use trueshot_core::color_chart::ColorChartDetector;
use trueshot_core::inventory::{
    CameraCalibration as InventoryCalibration, CameraColorCalibration as InventoryColorCalibration,
};
use trueshot_device_manager::{CalibrationData, CameraConfig, ColorCalibrationData};

const CALIBRATION_PROJECT_ID: &str = "_calibration";
const MAX_CALIBRATION_FRAMES: usize = 64;
const MAX_CALIBRATION_FRAME_BYTES: u64 = 128 * 1024 * 1024;
const MAX_CALIBRATION_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Deserialize, IntoParams)]
struct CaptureParams {
    camera_id: Option<String>,
    camera_index: Option<usize>,
}

#[derive(Deserialize, IntoParams)]
struct ComputeParams {
    rows: Option<i32>,
    cols: Option<i32>,
    square_size_mm: Option<f32>,
    camera_id: Option<String>,
}

#[derive(Serialize, ToSchema)]
struct CaptureResponse {
    frame_id: usize,
    path: String,
    total_frames: usize,
    camera_id: String,
}

#[derive(Serialize, ToSchema)]
struct CalibrationResult {
    success: bool,
    rms_error: Option<f64>,
    camera_id: Option<String>,
    message: String,
    calibration_path: Option<String>,
}

#[derive(Deserialize, IntoParams)]
struct ColorComputeParams {
    camera_id: Option<String>,
    frame_index: Option<usize>,
}

#[derive(Serialize, ToSchema)]
struct ColorCalibrationResult {
    success: bool,
    camera_id: Option<String>,
    delta_e: Option<f32>,
    message: String,
    calibration_path: Option<String>,
    ccm: Option<[[f32; 3]; 3]>,
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(capture_calibration_frame)
        .service(compute_calibration)
        .service(compute_color_calibration)
        .service(clear_calibration_session)
        .service(clear_calibration_session_delete)
        .service(list_calibrations)
        .service(get_calibration);
}

#[utoipa::path(
    post,
    path = "/api/calibration/capture",
    tag = "calibration",
    params(CaptureParams),
    responses(
        (status = 200, description = "Calibration frame captured", body = CaptureResponse),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/calibration/capture")]
async fn capture_calibration_frame(
    req: HttpRequest,
    params: web::Query<CaptureParams>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }

    let (camera, camera_id) = match select_camera(&state, &params).await {
        Ok(v) => v,
        Err(err) => {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": err.to_string()
            }));
        }
    };

    let capture_path =
        match tokio::task::spawn_blocking(move || camera.capture(&CameraConfig::default())).await {
            Ok(Ok(path)) => path,
            Ok(Err(err)) => {
                return HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": format!("Capture failed: {}", err)
                }));
            }
            Err(err) => {
                return HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": format!("Capture task failed: {}", err)
                }));
            }
        };

    let mut session = state.calibration_session.lock().await;
    reset_session_if_needed(&mut session, &camera_id, &state.config.paths.projects_dir);
    if session.captured_frames.len() >= MAX_CALIBRATION_FRAMES {
        return HttpResponse::PayloadTooLarge().json(serde_json::json!({
            "error": format!(
                "Calibration session is limited to {} frames",
                MAX_CALIBRATION_FRAMES
            )
        }));
    }

    let frame_id = session.captured_frames.len();
    let calib_dir = calibration_dir(&state.config.paths.projects_dir, &camera_id);
    if let Err(response) = ensure_project_directory(
        &state.config.paths.projects_dir,
        CALIBRATION_PROJECT_ID,
        &calib_dir,
    ) {
        return HttpResponse::InternalServerError().json(serde_json::json!({
            "error": format!("Failed to create calibration dir: {}", response.status())
        }));
    }

    let target_path = calib_dir.join(format!(
        "calib_{}_{}.jpg",
        sanitize_id(&camera_id),
        frame_id
    ));
    if let Err(err) = move_capture_file(
        &state.config.paths.projects_dir,
        &capture_path,
        &target_path,
    ) {
        return HttpResponse::InternalServerError().json(serde_json::json!({
            "error": format!("Failed to store calibration frame: {}", err)
        }));
    }

    session.captured_frames.push(target_path.clone());
    session.camera_id = Some(camera_id.clone());

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "calibration.capture",
            camera_id.clone(),
            "success",
            audit_actor(&req).2,
            serde_json::json!({ "path": target_path.to_string_lossy() }),
        ),
    );

    HttpResponse::Ok().json(CaptureResponse {
        frame_id,
        path: target_path.to_string_lossy().to_string(),
        total_frames: session.captured_frames.len(),
        camera_id,
    })
}

#[utoipa::path(
    post,
    path = "/api/calibration/compute",
    tag = "calibration",
    params(ComputeParams),
    responses(
        (status = 200, description = "Calibration result", body = CalibrationResult),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/calibration/compute")]
async fn compute_calibration(
    req: HttpRequest,
    params: web::Query<ComputeParams>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }

    let (paths, camera_id) = {
        let session = state.calibration_session.lock().await;
        let camera_id = params
            .camera_id
            .clone()
            .or_else(|| session.camera_id.clone());
        if session.captured_frames.len() < 5 {
            return HttpResponse::BadRequest().json(CalibrationResult {
                success: false,
                rms_error: None,
                camera_id,
                message: format!(
                    "Need at least 5 frames, have {}",
                    session.captured_frames.len()
                ),
                calibration_path: None,
            });
        }
        (session.captured_frames.clone(), camera_id)
    };

    let camera_id = match camera_id {
        Some(id) => id,
        None => {
            return HttpResponse::BadRequest().json(CalibrationResult {
                success: false,
                rms_error: None,
                camera_id: None,
                message: "Missing camera_id for calibration".to_string(),
                calibration_path: None,
            });
        }
    };

    let rows = params.rows.unwrap_or(9);
    let cols = params.cols.unwrap_or(6);
    let square_size = params.square_size_mm.unwrap_or(25.0);

    let mut opened_frames = Vec::with_capacity(paths.len());
    let mut total_bytes = 0_u64;
    for path in &paths {
        let file = match open_project_file_read(
            &state.config.paths.projects_dir,
            CALIBRATION_PROJECT_ID,
            path,
        ) {
            Ok(file) => file,
            Err(response) => return response,
        };
        let bytes = match file.metadata() {
            Ok(metadata) => metadata.len(),
            Err(_) => {
                return HttpResponse::InternalServerError().json(CalibrationResult {
                    success: false,
                    rms_error: None,
                    camera_id: Some(camera_id),
                    message: "Failed to inspect calibration frame".to_string(),
                    calibration_path: None,
                });
            }
        };
        if bytes == 0 || bytes > MAX_CALIBRATION_FRAME_BYTES {
            return HttpResponse::PayloadTooLarge().json(CalibrationResult {
                success: false,
                rms_error: None,
                camera_id: Some(camera_id),
                message: format!(
                    "Calibration frame must be 1..={} bytes",
                    MAX_CALIBRATION_FRAME_BYTES
                ),
                calibration_path: None,
            });
        }
        total_bytes = match total_bytes.checked_add(bytes) {
            Some(total) if total <= MAX_CALIBRATION_TOTAL_BYTES => total,
            _ => {
                return HttpResponse::PayloadTooLarge().json(CalibrationResult {
                    success: false,
                    rms_error: None,
                    camera_id: Some(camera_id),
                    message: format!(
                        "Calibration session exceeds {} bytes",
                        MAX_CALIBRATION_TOTAL_BYTES
                    ),
                    calibration_path: None,
                });
            }
        };
        opened_frames.push(file);
    }

    let calibration = match tokio::task::spawn_blocking(move || {
        let mut encoded = Vec::with_capacity(opened_frames.len());
        for file in opened_frames {
            let mut bytes = Vec::new();
            file.take(MAX_CALIBRATION_FRAME_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_CALIBRATION_FRAME_BYTES {
                anyhow::bail!("Calibration frame changed beyond its size limit");
            }
            encoded.push(bytes);
        }
        trueshot_core::calibration::lens::calibrate_checkerboard_encoded(
            &encoded,
            rows,
            cols,
            square_size,
        )
    })
    .await
    {
        Ok(Ok(intrinsics)) => intrinsics,
        Ok(Err(err)) => {
            return HttpResponse::InternalServerError().json(CalibrationResult {
                success: false,
                rms_error: None,
                camera_id: Some(camera_id),
                message: format!("Calibration failed: {}", err),
                calibration_path: None,
            });
        }
        Err(err) => {
            return HttpResponse::InternalServerError().json(CalibrationResult {
                success: false,
                rms_error: None,
                camera_id: Some(camera_id),
                message: format!("Calibration task failed: {}", err),
                calibration_path: None,
            });
        }
    };

    let calibration_path = match write_calibration_file(
        &state.config.paths.projects_dir,
        &camera_id,
        &calibration,
        rows,
        cols,
        square_size,
    ) {
        Ok(path) => Some(path.to_string_lossy().to_string()),
        Err(err) => {
            return HttpResponse::InternalServerError().json(CalibrationResult {
                success: false,
                rms_error: Some(calibration.rms_error),
                camera_id: Some(camera_id),
                message: format!("Failed to persist calibration: {}", err),
                calibration_path: None,
            });
        }
    };

    let calibration_data = CalibrationData {
        intrinsics: Some(calibration.camera_matrix.clone()),
        distortion: Some(calibration.dist_coeffs.clone()),
        rms_error: Some(calibration.rms_error),
        image_width: Some(calibration.width),
        image_height: Some(calibration.height),
        last_calibrated: Utc::now().to_rfc3339(),
    };

    if let Ok(mut manager) = state.camera_manager.try_lock() {
        let _ = manager
            .registry
            .update_calibration(&camera_id, calibration_data);
    } else {
        let mut manager = state.camera_manager.lock().await;
        let _ = manager
            .registry
            .update_calibration(&camera_id, calibration_data);
    }

    let _ = state.inventory.upsert_camera_calibration(
        &camera_id,
        calibration.camera_matrix.clone(),
        calibration.dist_coeffs.clone(),
        calibration.rms_error,
        calibration.width,
        calibration.height,
    );
    let inventory_calibration = InventoryCalibration {
        camera_id: camera_id.clone(),
        camera_matrix: calibration.camera_matrix.clone(),
        distortion: calibration.dist_coeffs.clone(),
        rms_error: calibration.rms_error,
        width: calibration.width,
        height: calibration.height,
        updated_at: Utc::now(),
    };
    cache_calibration(&state, &inventory_calibration).await;

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "calibration.compute",
            camera_id.clone(),
            "success",
            audit_actor(&req).2,
            serde_json::json!({ "rms_error": calibration.rms_error }),
        ),
    );

    HttpResponse::Ok().json(CalibrationResult {
        success: true,
        rms_error: Some(calibration.rms_error),
        camera_id: Some(camera_id),
        message: format!(
            "Calibration complete. RMS error: {:.4}px",
            calibration.rms_error
        ),
        calibration_path,
    })
}

#[utoipa::path(
    post,
    path = "/api/calibration/color/compute",
    tag = "calibration",
    params(ColorComputeParams),
    responses(
        (status = 200, description = "Color calibration result", body = ColorCalibrationResult),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/calibration/color/compute")]
async fn compute_color_calibration(
    req: HttpRequest,
    params: web::Query<ColorComputeParams>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }

    let (frame_path, camera_id) = {
        let session = state.calibration_session.lock().await;
        let camera_id = params
            .camera_id
            .clone()
            .or_else(|| session.camera_id.clone());
        if session.captured_frames.is_empty() {
            return HttpResponse::BadRequest().json(ColorCalibrationResult {
                success: false,
                delta_e: None,
                camera_id,
                message: "No calibration frames captured".to_string(),
                calibration_path: None,
                ccm: None,
            });
        }
        let idx = params
            .frame_index
            .unwrap_or_else(|| session.captured_frames.len().saturating_sub(1));
        let frame = session
            .captured_frames
            .get(idx)
            .cloned()
            .unwrap_or_else(|| session.captured_frames.last().cloned().unwrap());
        (frame, camera_id)
    };

    let camera_id = match camera_id {
        Some(id) => id,
        None => {
            return HttpResponse::BadRequest().json(ColorCalibrationResult {
                success: false,
                delta_e: None,
                camera_id: None,
                message: "Missing camera_id for color calibration".to_string(),
                calibration_path: None,
                ccm: None,
            });
        }
    };

    let frame_path_clone = frame_path.clone();
    let projects_dir = state.config.paths.projects_dir.clone();
    let detection = match tokio::task::spawn_blocking(move || {
        let rgb = load_rgb_array(&projects_dir, &frame_path_clone)?;
        ColorChartDetector::detect_and_calibrate(&rgb).map_err(|e| anyhow::anyhow!(e))
    })
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => {
            return HttpResponse::InternalServerError().json(ColorCalibrationResult {
                success: false,
                delta_e: None,
                camera_id: Some(camera_id),
                message: format!("Color calibration failed: {}", err),
                calibration_path: None,
                ccm: None,
            });
        }
        Err(err) => {
            return HttpResponse::InternalServerError().json(ColorCalibrationResult {
                success: false,
                delta_e: None,
                camera_id: Some(camera_id),
                message: format!("Color calibration task failed: {}", err),
                calibration_path: None,
                ccm: None,
            });
        }
    };

    let Some(result) = detection else {
        return HttpResponse::BadRequest().json(ColorCalibrationResult {
            success: false,
            delta_e: None,
            camera_id: Some(camera_id),
            message: "Color chart not detected or DeltaE too high".to_string(),
            calibration_path: None,
            ccm: None,
        });
    };

    let color_calibration = ColorCalibrationData {
        ccm: result.matrix,
        delta_e: result.error,
        last_calibrated: Utc::now().to_rfc3339(),
    };

    if let Ok(mut manager) = state.camera_manager.try_lock() {
        let _ = manager
            .registry
            .update_color_calibration(&camera_id, color_calibration.clone());
    } else {
        let mut manager = state.camera_manager.lock().await;
        let _ = manager
            .registry
            .update_color_calibration(&camera_id, color_calibration.clone());
    }

    let _ = state.inventory.upsert_camera_color_calibration(
        &camera_id,
        color_calibration.ccm,
        color_calibration.delta_e,
    );
    let inventory_color = InventoryColorCalibration {
        camera_id: camera_id.clone(),
        ccm: color_calibration.ccm,
        delta_e: color_calibration.delta_e,
        updated_at: Utc::now(),
    };
    cache_color_calibration(&state, &inventory_color).await;

    let color_path = match write_color_calibration_file(
        &state.config.paths.projects_dir,
        &camera_id,
        &color_calibration,
    ) {
        Ok(path) => Some(path.to_string_lossy().to_string()),
        Err(err) => {
            return HttpResponse::InternalServerError().json(ColorCalibrationResult {
                success: false,
                delta_e: Some(result.error),
                camera_id: Some(camera_id),
                message: format!("Failed to persist color calibration: {}", err),
                calibration_path: None,
                ccm: Some(result.matrix),
            });
        }
    };

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "calibration.color",
            camera_id.clone(),
            "success",
            audit_actor(&req).2,
            serde_json::json!({ "delta_e": result.error }),
        ),
    );

    HttpResponse::Ok().json(ColorCalibrationResult {
        success: true,
        delta_e: Some(result.error),
        camera_id: Some(camera_id),
        message: format!("Color calibration complete. DeltaE: {:.2}", result.error),
        calibration_path: color_path,
        ccm: Some(result.matrix),
    })
}

async fn clear_calibration_session_impl(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> HttpResponse {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let mut session = state.calibration_session.lock().await;
    for path in &session.captured_frames {
        let _ = remove_project_file_if_exists(
            &state.config.paths.projects_dir,
            CALIBRATION_PROJECT_ID,
            path,
        );
    }
    session.captured_frames.clear();
    session.camera_id = None;
    HttpResponse::Ok().json(serde_json::json!({ "message": "Calibration session cleared" }))
}

#[utoipa::path(
    post,
    path = "/api/calibration/session/clear",
    tag = "calibration",
    responses(
        (status = 200, description = "Calibration session cleared", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/calibration/session/clear")]
async fn clear_calibration_session(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    clear_calibration_session_impl(req, state).await
}

#[utoipa::path(
    delete,
    path = "/api/calibration/session",
    tag = "calibration",
    responses(
        (status = 200, description = "Calibration session cleared", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[delete("/api/calibration/session")]
async fn clear_calibration_session_delete(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> impl Responder {
    clear_calibration_session_impl(req, state).await
}

#[utoipa::path(
    get,
    path = "/api/calibration",
    tag = "calibration",
    responses(
        (status = 200, description = "Calibration list", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/calibration")]
async fn list_calibrations(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let cm = state.camera_manager.lock().await;
    let mut list = Vec::new();
    for profile in cm.registry.profiles.values() {
        let mut calibration = state
            .inventory
            .get_camera_calibration(&profile.id)
            .ok()
            .flatten();
        if calibration.is_none() {
            calibration = fetch_cached_calibration(&state, &profile.id).await;
        }
        let mut color_calibration = state
            .inventory
            .get_camera_color_calibration(&profile.id)
            .ok()
            .flatten();
        if color_calibration.is_none() {
            color_calibration = fetch_cached_color_calibration(&state, &profile.id).await;
        }
        let warnings = calibration_warnings(
            &state.config,
            profile.calibration.as_ref(),
            calibration.as_ref(),
            profile.color_calibration.as_ref(),
            color_calibration.as_ref(),
            &profile.id,
        );
        list.push(serde_json::json!({
            "camera_id": profile.id,
            "camera_name": profile.name,
            "nickname": profile.nickname,
            "has_profile_calibration": profile.calibration.is_some(),
            "inventory_calibration": calibration,
            "profile_color_calibration": profile.color_calibration,
            "inventory_color_calibration": color_calibration,
            "warnings": warnings,
        }));
    }
    HttpResponse::Ok().json(list)
}

#[utoipa::path(
    get,
    path = "/api/calibration/{camera_id}",
    tag = "calibration",
    params(("camera_id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "Calibration data", body = serde_json::Value),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/calibration/{camera_id}")]
async fn get_calibration(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let camera_id = path.into_inner();
    let profile_calibration = {
        let cm = state.camera_manager.lock().await;
        cm.registry
            .get_profile(&camera_id)
            .and_then(|p| p.calibration.clone())
    };
    let profile_color_calibration = {
        let cm = state.camera_manager.lock().await;
        cm.registry
            .get_profile(&camera_id)
            .and_then(|p| p.color_calibration.clone())
    };
    let inventory_calibration = state
        .inventory
        .get_camera_calibration(&camera_id)
        .ok()
        .flatten();
    let inventory_color_calibration = state
        .inventory
        .get_camera_color_calibration(&camera_id)
        .ok()
        .flatten();
    let inventory_calibration = if inventory_calibration.is_some() {
        inventory_calibration
    } else {
        fetch_cached_calibration(&state, &camera_id).await
    };
    let inventory_color_calibration = if inventory_color_calibration.is_some() {
        inventory_color_calibration
    } else {
        fetch_cached_color_calibration(&state, &camera_id).await
    };

    if profile_calibration.is_none()
        && inventory_calibration.is_none()
        && profile_color_calibration.is_none()
        && inventory_color_calibration.is_none()
    {
        return HttpResponse::NotFound().body("Calibration not found");
    }

    HttpResponse::Ok().json(serde_json::json!({
        "camera_id": camera_id,
        "profile_calibration": profile_calibration,
        "inventory_calibration": inventory_calibration,
        "profile_color_calibration": profile_color_calibration,
        "inventory_color_calibration": inventory_color_calibration,
        "warnings": calibration_warnings(
            &state.config,
            profile_calibration.as_ref(),
            inventory_calibration.as_ref(),
            profile_color_calibration.as_ref(),
            inventory_color_calibration.as_ref(),
            &camera_id,
        ),
    }))
}

fn calibration_warnings(
    config: &AppConfig,
    profile: Option<&CalibrationData>,
    inventory: Option<&trueshot_core::inventory::CameraCalibration>,
    profile_color: Option<&ColorCalibrationData>,
    inventory_color: Option<&trueshot_core::inventory::CameraColorCalibration>,
    camera_id: &str,
) -> Vec<String> {
    let max_rms = config.server.calibration_max_rms.unwrap_or(0.8);
    let max_age_days = config.server.calibration_max_age_days.unwrap_or(30);
    let max_deltae = config.server.calibration_max_deltae.unwrap_or(6.0);
    let mut warnings = Vec::new();

    let (rms, timestamp) = if let Some(cal) = profile {
        (cal.rms_error, Some(cal.last_calibrated.clone()))
    } else if let Some(cal) = inventory {
        (Some(cal.rms_error), Some(cal.updated_at.to_rfc3339()))
    } else {
        (None, None)
    };

    if let Some(rms) = rms {
        if rms > max_rms {
            warnings.push(format!(
                "Camera {} calibration RMS {:.3} exceeds {:.3}",
                camera_id, rms, max_rms
            ));
        }
    } else {
        warnings.push(format!("Camera {} calibration RMS missing", camera_id));
    }

    if let Some(ts) = timestamp {
        if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&ts) {
            let age_days = (chrono::Utc::now() - parsed.with_timezone(&chrono::Utc)).num_days();
            if age_days > max_age_days {
                warnings.push(format!(
                    "Camera {} calibration is {} days old (>{})",
                    camera_id, age_days, max_age_days
                ));
            }
        } else {
            warnings.push(format!(
                "Camera {} calibration timestamp invalid",
                camera_id
            ));
        }
    } else {
        warnings.push(format!(
            "Camera {} calibration timestamp missing",
            camera_id
        ));
    }

    let (delta_e, color_ts) = if let Some(cal) = profile_color {
        (Some(cal.delta_e), Some(cal.last_calibrated.clone()))
    } else if let Some(cal) = inventory_color {
        (Some(cal.delta_e), Some(cal.updated_at.to_rfc3339()))
    } else {
        (None, None)
    };

    if let Some(delta_e) = delta_e {
        if delta_e > max_deltae {
            warnings.push(format!(
                "Camera {} color DeltaE {:.2} exceeds {:.2}",
                camera_id, delta_e, max_deltae
            ));
        }
    } else {
        warnings.push(format!("Camera {} color DeltaE missing", camera_id));
    }

    if let Some(ts) = color_ts {
        if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&ts) {
            let age_days = (chrono::Utc::now() - parsed.with_timezone(&chrono::Utc)).num_days();
            if age_days > max_age_days {
                warnings.push(format!(
                    "Camera {} color calibration is {} days old (>{})",
                    camera_id, age_days, max_age_days
                ));
            }
        } else {
            warnings.push(format!(
                "Camera {} color calibration timestamp invalid",
                camera_id
            ));
        }
    } else {
        warnings.push(format!(
            "Camera {} color calibration timestamp missing",
            camera_id
        ));
    }

    warnings
}

async fn cache_calibration(state: &AppState, calibration: &InventoryCalibration) {
    let Some(pool) = state.redis_pool.as_ref() else {
        return;
    };
    let key = format!("calib:{}", calibration.camera_id);
    if let Err(error) = pool.set_json(&key, calibration).await {
        tracing::debug!("Redis calibration cache write skipped: {}", error);
    }
}

async fn cache_color_calibration(state: &AppState, calibration: &InventoryColorCalibration) {
    let Some(pool) = state.redis_pool.as_ref() else {
        return;
    };
    let key = format!("calib_color:{}", calibration.camera_id);
    if let Err(error) = pool.set_json(&key, calibration).await {
        tracing::debug!("Redis color-calibration cache write skipped: {}", error);
    }
}

async fn fetch_cached_calibration(
    state: &AppState,
    camera_id: &str,
) -> Option<InventoryCalibration> {
    let pool = state.redis_pool.as_ref()?;
    let key = format!("calib:{camera_id}");
    pool.get_json(&key).await.ok().flatten()
}

async fn fetch_cached_color_calibration(
    state: &AppState,
    camera_id: &str,
) -> Option<InventoryColorCalibration> {
    let pool = state.redis_pool.as_ref()?;
    let key = format!("calib_color:{camera_id}");
    pool.get_json(&key).await.ok().flatten()
}

async fn select_camera(
    state: &web::Data<AppState>,
    params: &CaptureParams,
) -> Result<(std::sync::Arc<dyn trueshot_device_manager::Camera>, String)> {
    let manager = state.camera_manager.lock().await;
    if manager.cameras.is_empty() {
        anyhow::bail!("No cameras available");
    }

    if let Some(camera_id) = params.camera_id.as_ref() {
        for cam in &manager.cameras {
            if cam.id() == *camera_id {
                return Ok((cam.clone(), camera_id.clone()));
            }
        }
        anyhow::bail!("Camera {} not found", camera_id);
    }

    if let Some(index) = params.camera_index {
        if let Some(cam) = manager.cameras.get(index) {
            let id = cam.id();
            return Ok((cam.clone(), id));
        }
        anyhow::bail!("Camera index {} out of range", index);
    }

    let cam = manager.cameras.first().context("No cameras available")?;
    let id = cam.id();
    Ok((cam.clone(), id))
}

fn reset_session_if_needed(session: &mut CalibrationSession, camera_id: &str, projects_dir: &Path) {
    if session.camera_id.as_deref() != Some(camera_id) {
        for path in session.captured_frames.drain(..) {
            let _ = remove_project_file_if_exists(projects_dir, CALIBRATION_PROJECT_ID, &path);
        }
        session.camera_id = Some(camera_id.to_string());
    }
}

fn calibration_dir(projects_dir: &Path, camera_id: &str) -> PathBuf {
    projects_dir
        .join("_calibration")
        .join(sanitize_id(camera_id))
}

fn sanitize_id(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

fn move_capture_file(projects_dir: &Path, src: &Path, dst: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(src)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("Camera capture is not a regular file");
    }
    let mut source = std::fs::File::open(src)?;
    let mut staged = stage_project_file(projects_dir, CALIBRATION_PROJECT_ID, dst, false)
        .map_err(|response| anyhow::anyhow!("Stage calibration frame: {}", response.status()))?;
    std::io::copy(&mut source, staged.file_mut())?;
    staged
        .commit()
        .map_err(|response| anyhow::anyhow!("Commit calibration frame: {}", response.status()))?;
    std::fs::remove_file(src)?;
    Ok(())
}

fn write_calibration_file(
    projects_dir: &Path,
    camera_id: &str,
    intrinsics: &trueshot_core::calibration::lens::CameraIntrinsics,
    rows: i32,
    cols: i32,
    square_size_mm: f32,
) -> Result<PathBuf> {
    let calib_dir = calibration_dir(projects_dir, camera_id);
    ensure_project_directory(projects_dir, CALIBRATION_PROJECT_ID, &calib_dir).map_err(
        |response| anyhow::anyhow!("Create calibration directory: {}", response.status()),
    )?;
    let path = calib_dir.join("calibration.json");
    let payload = serde_json::json!({
        "camera_id": camera_id,
        "camera_matrix": intrinsics.camera_matrix,
        "dist_coeffs": intrinsics.dist_coeffs,
        "rms_error": intrinsics.rms_error,
        "width": intrinsics.width,
        "height": intrinsics.height,
        "rows": rows,
        "cols": cols,
        "square_size_mm": square_size_mm,
        "updated_at": Utc::now().to_rfc3339(),
    });
    write_project_file_atomic(
        projects_dir,
        CALIBRATION_PROJECT_ID,
        &path,
        serde_json::to_string_pretty(&payload)?.as_bytes(),
    )
    .map_err(|response| anyhow::anyhow!("Write calibration profile: {}", response.status()))?;
    Ok(path)
}

fn write_color_calibration_file(
    projects_dir: &Path,
    camera_id: &str,
    calibration: &ColorCalibrationData,
) -> Result<PathBuf> {
    let calib_dir = calibration_dir(projects_dir, camera_id);
    ensure_project_directory(projects_dir, CALIBRATION_PROJECT_ID, &calib_dir).map_err(
        |response| anyhow::anyhow!("Create calibration directory: {}", response.status()),
    )?;
    let path = calib_dir.join(format!("color_calibration_{}.json", sanitize_id(camera_id)));
    let payload = serde_json::to_string_pretty(calibration)?;
    write_project_file_atomic(
        projects_dir,
        CALIBRATION_PROJECT_ID,
        &path,
        payload.as_bytes(),
    )
    .map_err(|response| anyhow::anyhow!("Write color calibration: {}", response.status()))?;
    Ok(path)
}

fn load_rgb_array(projects_dir: &Path, path: &Path) -> Result<Array3<f64>> {
    let file = open_project_file_read(projects_dir, CALIBRATION_PROJECT_ID, path)
        .map_err(|response| anyhow::anyhow!("Open calibration frame: {}", response.status()))?;
    let image = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .with_context(|| format!("Failed to identify image: {}", path.display()))?
        .decode()
        .with_context(|| format!("Failed to decode image: {}", path.display()))?;
    let rgb = image.to_rgb32f();
    let (width, height) = rgb.dimensions();
    let mut array = Array3::<f64>::zeros((height as usize, width as usize, 3));
    for (x, y, pixel) in rgb.enumerate_pixels() {
        let yi = y as usize;
        let xi = x as usize;
        array[(yi, xi, 0)] = pixel[0] as f64;
        array[(yi, xi, 1)] = pixel[1] as f64;
        array[(yi, xi, 2)] = pixel[2] as f64;
    }
    Ok(array)
}

fn audit_actor(req: &HttpRequest) -> (String, String, Option<String>) {
    let (actor, role) = match req.extensions().get::<crate::auth::AuthContext>() {
        Some(ctx) => (ctx.sub.clone(), format!("{:?}", ctx.role)),
        None => ("unknown".to_string(), "unknown".to_string()),
    };
    let ip = req.peer_addr().map(|p| p.ip().to_string());
    (actor, role, ip)
}

fn log_audit(req: &HttpRequest, state: &web::Data<AppState>, event: AuditEvent) {
    if state
        .audit
        .append_with_redaction(event, &state.config.privacy)
        .is_err()
    {
        crate::public_error::log_redacted_failure(req, "audit.append");
    }
}
