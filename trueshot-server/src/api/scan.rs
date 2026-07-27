//! ScanWizard API - Intelligent scanning endpoints
//!
//! Provides endpoints for the smart scanning wizard:
//! - Background calibration
//! - Object detection & analysis
//! - Scan plan computation
//! - Guided capture execution

use actix_web::http::StatusCode;
use actix_web::{get, post, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use anyhow::{Context, Result};
use image::{DynamicImage, GrayImage, Rgb, RgbImage};
use ndarray::{Array2, Array3};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::info;

use crate::audit::AuditEvent;
use crate::auth::require_admin;
use crate::fs_safety::{
    available_space_bytes, ensure_project_directory, open_project_file_read,
    remove_project_file_if_exists, stage_project_file, write_project_file_atomic,
};
use crate::licensing::{enforce_scan_limit, require_license_feature};
use crate::scan_types::{
    BackgroundStatus, BoundingBox, ComplexityInfo, ComputePlanRequest, CoverageStatus,
    ExecuteStepRequest, ObjectAnalysis, ObjectDetection, QualityAssessment, QualityDefectScore,
    QualityHistoryEntry, SDCardStatus, ScaleAnchor, ScaleAnchorRequest, ScaleAnchorStatus,
    ScanPlan, ScanProgress, ScanStep, SizeInfo, StepIntegrity, SurfaceInfo,
};
use crate::scan_wizard::{
    DetectionState, QualityHistoryEntry as WizardQualityHistoryEntry, ScanRuntime, ScanWizardState,
};
use crate::state::AppState;

use sha2::{Digest, Sha256};
use std::io::Cursor;
use trueshot_core::ai::material::MaterialEstimator;
use trueshot_core::inventory::CameraCalibration;
use trueshot_core::quality_analyzer::{Analyzer, Defect, ProcessingParams};
use trueshot_core::vision::features::NativeFeatureExtractor;
use trueshot_device_manager::{CameraConfig, CameraRole};
use uuid::Uuid;

const WIZARD_PROJECT_ID: &str = "_wizard";

// ============================================================================
// Background Calibration Endpoints
// ============================================================================

/// Check if background has been captured
#[utoipa::path(
    get,
    path = "/api/wizard/background/status",
    tag = "wizard",
    responses(
        (status = 200, description = "Background status", body = BackgroundStatus),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/wizard/background/status")]
pub async fn get_background_status(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let wizard = state.scan_wizard.lock().await;

    let status = BackgroundStatus {
        captured: wizard.background.is_some(),
        timestamp: wizard.background_captured_at.map(|t| t.to_rfc3339()),
        frame_count: wizard.background_frames,
    };

    HttpResponse::Ok().json(status)
}

/// Capture background (360° rotation of empty turntable)
#[utoipa::path(
    post,
    path = "/api/wizard/background/capture",
    tag = "wizard",
    responses(
        (status = 200, description = "Background capture complete", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/wizard/background/capture")]
pub async fn capture_background(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    info!("📸 Starting background calibration capture...");

    let capture_result = capture_background_sequence(&state).await;
    match capture_result {
        Ok((frame_count, timestamp)) => {
            info!("✅ Background capture complete");
            HttpResponse::Ok().json(serde_json::json!({
                "success": true,
                "frame_count": frame_count,
                "timestamp": timestamp.to_rfc3339()
            }))
        }
        Err(err) => HttpResponse::InternalServerError().body(err.to_string()),
    }
}

// ============================================================================
// Object Detection Endpoints
// ============================================================================

/// Get current object detection status (polling endpoint)
#[utoipa::path(
    get,
    path = "/api/wizard/detection/status",
    tag = "wizard",
    responses(
        (status = 200, description = "Detection status", body = ObjectDetection),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/wizard/detection/status")]
pub async fn get_detection_status(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let frame = match capture_preview_rgb(&state).await {
        Ok(img) => img,
        Err(e) => return HttpResponse::InternalServerError().body(e.to_string()),
    };

    let result = {
        let wizard = state.scan_wizard.lock().await;
        let background = wizard.background.as_ref();
        let previous = wizard.last_detection.as_ref();
        compute_detection(&frame, background, previous)
    };

    {
        let mut wizard = state.scan_wizard.lock().await;
        wizard.last_detection = Some(result.state.clone());
        if result.detection.detected {
            if let Ok((assessment, uncertainty)) =
                compute_quality_assessment(&frame, result.width, result.height, &result.mask)
            {
                record_quality_assessment(&mut wizard, assessment, uncertainty);
            }
        }
    }

    HttpResponse::Ok().json(result.detection)
}

/// Get current quality assessment (polling endpoint)
#[utoipa::path(
    get,
    path = "/api/wizard/quality",
    tag = "wizard",
    responses(
        (status = 200, description = "Quality assessment", body = QualityAssessment),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/wizard/quality")]
pub async fn get_quality_status(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Some(quality) = state.scan_wizard.lock().await.last_quality.clone() {
        return HttpResponse::Ok().json(quality);
    }
    match update_quality_from_preview(&state).await {
        Ok(quality) => HttpResponse::Ok().json(quality),
        Err(err) => HttpResponse::InternalServerError().body(err.to_string()),
    }
}

/// Get recent quality history (polling endpoint)
#[utoipa::path(
    get,
    path = "/api/wizard/quality/history",
    tag = "wizard",
    responses(
        (status = 200, description = "Quality history", body = [QualityHistoryEntry]),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/wizard/quality/history")]
pub async fn get_quality_history(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let wizard = state.scan_wizard.lock().await;
    let mut history: Vec<QualityHistoryEntry> = wizard
        .quality_history
        .iter()
        .rev()
        .take(50)
        .map(|entry| QualityHistoryEntry {
            captured_at: entry.captured_at.to_rfc3339(),
            score: entry.score,
            pass: entry.pass,
            issues: entry.issues.clone(),
            actions: entry.actions.clone(),
        })
        .collect();
    history.reverse();
    HttpResponse::Ok().json(history)
}

/// Get current scale anchor (meters-per-unit + geo origin)
#[utoipa::path(
    get,
    path = "/api/wizard/scale-anchor",
    tag = "wizard",
    responses(
        (status = 200, description = "Scale anchor status", body = ScaleAnchorStatus),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/wizard/scale-anchor")]
pub async fn get_scale_anchor(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let wizard = state.scan_wizard.lock().await;
    let anchor = wizard.scale_anchor.clone();
    let status = ScaleAnchorStatus {
        configured: anchor.is_some(),
        anchor,
    };
    HttpResponse::Ok().json(status)
}

/// Set scale anchor (known distance / measured units)
#[utoipa::path(
    post,
    path = "/api/wizard/scale-anchor",
    tag = "wizard",
    request_body = ScaleAnchorRequest,
    responses(
        (status = 200, description = "Scale anchor set", body = ScaleAnchor),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/wizard/scale-anchor")]
pub async fn set_scale_anchor(
    req: HttpRequest,
    state: web::Data<AppState>,
    request: web::Json<ScaleAnchorRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if request.known_distance_m <= 0.0 || request.measured_units <= 0.0 {
        return HttpResponse::BadRequest().body("known_distance_m and measured_units must be > 0");
    }
    let meters_per_unit = request.known_distance_m / request.measured_units;
    let anchor = ScaleAnchor {
        known_distance_m: request.known_distance_m,
        measured_units: request.measured_units,
        meters_per_unit,
        label: request.label.clone(),
        origin_lat: request.origin_lat,
        origin_lon: request.origin_lon,
        origin_alt: request.origin_alt,
        crs: request.crs.clone(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    let mut wizard = state.scan_wizard.lock().await;
    wizard.scale_anchor = Some(anchor.clone());
    HttpResponse::Ok().json(anchor)
}

/// Get latest uncertainty map (PNG)
#[utoipa::path(
    get,
    path = "/api/wizard/quality/uncertainty",
    tag = "wizard",
    responses(
        (status = 200, description = "Uncertainty map (PNG)"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/wizard/quality/uncertainty")]
pub async fn get_uncertainty_map(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if state.scan_wizard.lock().await.last_uncertainty.is_none() {
        let _ = update_quality_from_preview(&state).await;
    }
    let wizard = state.scan_wizard.lock().await;
    let Some(map) = wizard.last_uncertainty.as_ref() else {
        return HttpResponse::NotFound().body("No uncertainty map available");
    };
    let mut buf = Vec::new();
    let img = DynamicImage::ImageLuma8(map.clone());
    if img
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .is_err()
    {
        return HttpResponse::InternalServerError().body("Failed to encode uncertainty map");
    }
    HttpResponse::Ok().content_type("image/png").body(buf)
}

// ============================================================================
// AI Analysis Endpoints
// ============================================================================

/// Trigger AI analysis of detected object
#[utoipa::path(
    post,
    path = "/api/wizard/analyze",
    tag = "wizard",
    responses(
        (status = 200, description = "Object analysis", body = ObjectAnalysis),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/wizard/analyze")]
pub async fn analyze_object(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    info!("🔬 Starting object analysis...");

    let frame = match capture_preview_rgb(&state).await {
        Ok(img) => img,
        Err(e) => return HttpResponse::InternalServerError().body(e.to_string()),
    };

    let background = {
        let wizard = state.scan_wizard.lock().await;
        wizard.background.clone()
    };

    let analysis = match analyze_frame(&state, &frame, background.as_ref()) {
        Ok(analysis) => analysis,
        Err(e) => return HttpResponse::InternalServerError().body(e.to_string()),
    };

    {
        let mut wizard = state.scan_wizard.lock().await;
        wizard.last_analysis = Some(analysis.clone());
    }

    info!("✅ Analysis complete: {:?}", analysis.size.category);
    HttpResponse::Ok().json(analysis)
}

// ============================================================================
// Scan Plan Computation
// ============================================================================

/// Compute optimal scan plan from analysis + quality level
#[utoipa::path(
    post,
    path = "/api/wizard/plan/compute",
    tag = "wizard",
    request_body = ComputePlanRequest,
    responses(
        (status = 200, description = "Scan plan", body = ScanPlan),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/wizard/plan/compute")]
pub async fn compute_scan_plan(
    req: HttpRequest,
    request: web::Json<ComputePlanRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    info!(
        "📐 Computing scan plan for quality: {}",
        request.quality_level
    );

    if let Some(preset) = request.preset.as_ref() {
        let preset_norm = preset.to_lowercase();
        if preset_norm == "room" {
            if let Err(resp) = require_license_feature(
                &state,
                trueshot_core::licensing::Feature::RoomReconstruction,
                "room_reconstruction",
            ) {
                return resp;
            }
        } else if preset_norm == "human" {
            if let Err(resp) = require_license_feature(
                &state,
                trueshot_core::licensing::Feature::AvatarReconstruction,
                "avatar_reconstruction",
            ) {
                return resp;
            }
        } else if preset_norm == "dynamic" || preset_norm == "4dgs" {
            if let Err(resp) = require_license_feature(
                &state,
                trueshot_core::licensing::Feature::FourDGS,
                "dynamic_4dgs",
            ) {
                return resp;
            }
        }
    }

    let (uncertainty, quality) = {
        let wizard = state.scan_wizard.lock().await;
        (wizard.last_uncertainty.clone(), wizard.last_quality.clone())
    };
    let plan = compute_optimal_plan(
        &request.quality_level,
        &request.analysis,
        uncertainty.as_ref(),
        quality.as_ref(),
    );

    {
        let mut wizard = state.scan_wizard.lock().await;
        wizard.plan = Some(plan.clone());
    }

    info!(
        "✅ Plan: {} photos, {} orientations, {} positions",
        plan.total_photos, plan.object_orientations, plan.camera_positions_per_orientation
    );

    HttpResponse::Ok().json(plan)
}

fn compute_optimal_plan(
    quality: &str,
    analysis: &ObjectAnalysis,
    uncertainty: Option<&GrayImage>,
    quality_state: Option<&QualityAssessment>,
) -> ScanPlan {
    // Quality configuration
    let (mut angular_resolution, mut camera_elevations, min_orientations): (f32, Vec<i32>, u32) =
        match quality {
            "preview" => (30.0, vec![0, 45], 1),
            "standard" => (15.0, vec![0, 30, 60], 2),
            "high" => (10.0, vec![-15, 0, 30, 60], 2),
            "ultra" => (7.5, vec![-15, 0, 15, 30, 45, 60], 2),
            _ => (15.0, vec![0, 30, 60], 2),
        };

    if analysis.complexity.score > 0.7 {
        angular_resolution *= 0.85;
    }
    if analysis.surface.specular_ratio > 0.35 {
        angular_resolution *= 0.9;
        if !camera_elevations.contains(&15) {
            camera_elevations.push(15);
            camera_elevations.sort();
        }
    }
    if let Some(quality_state) = quality_state {
        if quality_state.score < 0.5 {
            angular_resolution *= 0.85;
        }
    }

    // Adjust for object properties
    let orientations = if analysis.has_underside_detail {
        min_orientations.max(2)
    } else {
        min_orientations
    };

    let camera_positions =
        camera_elevations.len() as u32 + if analysis.aspect_ratio > 1.5 { 1 } else { 0 };

    let photos_per_rotation = (360.0 / angular_resolution).ceil() as u32;
    // Time estimation: 3 sec/photo + setup
    let setup_time = (orientations * 30) + (camera_positions * orientations * 15);

    let extra_angles = uncertainty
        .map(|map| select_uncertainty_angles(map, photos_per_rotation as usize, 2))
        .unwrap_or_default();

    // Generate steps
    let mut steps = Vec::new();
    let mut photo_index = 0u32;

    for orient in 0..orientations {
        if orient > 0 {
            steps.push(ScanStep {
                step_type: "object_orientation".to_string(),
                instruction: if orient == 1 {
                    "Flip object upside down to capture underside".to_string()
                } else {
                    format!("Reposition object to orientation {}", orient + 1)
                },
                camera_position: None,
                object_orientation: Some(orient),
                rotation_angle: None,
                photo_index: None,
            });
        }

        for cam in 0..camera_positions {
            let elevation = camera_elevations.get(cam as usize).copied().unwrap_or(0);
            steps.push(ScanStep {
                step_type: "camera_position".to_string(),
                instruction: get_camera_instruction(elevation),
                camera_position: Some(cam),
                object_orientation: Some(orient),
                rotation_angle: None,
                photo_index: None,
            });

            let mut angle = 0.0f32;
            while angle < 360.0 {
                steps.push(ScanStep {
                    step_type: "capture".to_string(),
                    instruction: format!("Capture photo {}", photo_index + 1),
                    camera_position: Some(cam),
                    object_orientation: Some(orient),
                    rotation_angle: Some(angle),
                    photo_index: Some(photo_index),
                });
                photo_index += 1;
                angle += angular_resolution;
            }

            for extra in &extra_angles {
                if angle_exists(&steps, *extra, Some(cam), Some(orient)) {
                    continue;
                }
                steps.push(ScanStep {
                    step_type: "capture".to_string(),
                    instruction: format!("Capture photo {}", photo_index + 1),
                    camera_position: Some(cam),
                    object_orientation: Some(orient),
                    rotation_angle: Some(*extra),
                    photo_index: Some(photo_index),
                });
                photo_index += 1;
            }
        }
    }

    let total_photos = photo_index;
    let total_time = total_photos * 3 + setup_time;

    ScanPlan {
        quality_level: quality.to_string(),
        object_orientations: orientations,
        camera_positions_per_orientation: camera_positions,
        photos_per_rotation,
        total_photos,
        estimated_time_seconds: total_time,
        steps,
    }
}

fn select_uncertainty_angles(map: &GrayImage, bins: usize, max_angles: usize) -> Vec<f32> {
    let bins = bins.max(1);
    let mut hist = vec![0f32; bins];
    let (width, height) = map.dimensions();
    let width_f = width.max(1) as f32;
    for y in 0..height {
        for x in 0..width {
            let val = map.get_pixel(x, y)[0] as f32 / 255.0;
            if val < 0.05 {
                continue;
            }
            let bin = ((x as f32 / width_f) * bins as f32).floor() as usize;
            let idx = bin.min(bins - 1);
            hist[idx] += val;
        }
    }
    let mut ranked: Vec<(usize, f32)> = hist.iter().copied().enumerate().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked
        .into_iter()
        .take(max_angles)
        .map(|(bin, _)| ((bin as f32 + 0.5) * (360.0 / bins as f32)) % 360.0)
        .collect()
}

fn angle_exists(steps: &[ScanStep], angle: f32, cam: Option<u32>, orient: Option<u32>) -> bool {
    let tol = 0.5;
    steps.iter().any(|step| {
        if step.step_type != "capture" {
            return false;
        }
        if step.camera_position != cam || step.object_orientation != orient {
            return false;
        }
        if let Some(a) = step.rotation_angle {
            (a - angle).abs() <= tol
        } else {
            false
        }
    })
}

fn get_camera_instruction(elevation: i32) -> String {
    if elevation < 0 {
        format!("Move camera LOW ({}° below eye level)", elevation.abs())
    } else if elevation == 0 {
        "Position camera at EYE LEVEL with object".to_string()
    } else if elevation <= 30 {
        format!("Move camera MEDIUM height ({}° above)", elevation)
    } else if elevation <= 45 {
        format!("Move camera HIGH ({}° above, looking down)", elevation)
    } else {
        format!("Position camera OVERHEAD ({}° from above)", elevation)
    }
}

async fn capture_preview_rgb(state: &AppState) -> Result<RgbImage> {
    let camera = select_camera_by_role(state, CameraRole::LiveFeedback)
        .await
        .or_else(|| select_any_camera(state));

    let camera = camera.context("No camera available for preview")?;
    let jpeg = camera.capture_preview().context("Preview capture failed")?;
    let img = image::load_from_memory(&jpeg).context("Failed to decode preview JPEG")?;
    Ok(img.to_rgb8())
}

async fn update_quality_from_preview(state: &AppState) -> Result<QualityAssessment> {
    let frame = capture_preview_rgb(state).await?;
    let background = {
        let wizard = state.scan_wizard.lock().await;
        wizard.background.clone()
    };
    let (mask, _bbox, _confidence) = compute_mask_and_bbox(&frame, background.as_ref());
    let (assessment, uncertainty) =
        compute_quality_assessment(&frame, frame.width(), frame.height(), &mask)?;
    let mut wizard = state.scan_wizard.lock().await;
    record_quality_assessment(&mut wizard, assessment.clone(), uncertainty);
    Ok(assessment)
}

struct CaptureGateDecision {
    assessment: QualityAssessment,
    passed: bool,
    parallax_score: Option<f32>,
    motion_score: Option<f32>,
}

async fn preflight_capture_gate(state: &AppState, retries: usize) -> Result<CaptureGateDecision> {
    let mut attempt = 0usize;
    loop {
        let decision = preflight_capture_quality(state).await?;
        if decision.passed || attempt >= retries {
            return Ok(decision);
        }
        attempt += 1;
        tokio::time::sleep(Duration::from_millis(220)).await;
    }
}

async fn preflight_capture_quality(state: &AppState) -> Result<CaptureGateDecision> {
    let frame = capture_preview_rgb(state).await?;
    let (background, last_preview) = {
        let wizard = state.scan_wizard.lock().await;
        (wizard.background.clone(), wizard.last_preview.clone())
    };
    let (mask, _bbox, _confidence) = compute_mask_and_bbox(&frame, background.as_ref());
    let (mut assessment, uncertainty) =
        compute_quality_assessment(&frame, frame.width(), frame.height(), &mask)?;

    let parallax_score = last_preview
        .as_ref()
        .map(|prev| compute_parallax_score(prev, &frame, &mask, frame.width(), frame.height()));
    let motion_score = last_preview
        .as_ref()
        .map(|prev| compute_motion_score(prev, &frame, &mask, frame.width(), frame.height()));
    let parallax_ok = parallax_score.map(|score| score >= 0.035).unwrap_or(true);
    if !parallax_ok {
        let issue = "Insufficient viewpoint change detected";
        if !assessment.issues.iter().any(|entry| entry == issue) {
            assessment.issues.push(issue.to_string());
        }
        let action = "Increase parallax: move the camera to a new angle before capturing.";
        if !assessment.actions.iter().any(|entry| entry == action) {
            assessment.actions.push(action.to_string());
        }
        assessment.score = (assessment.score - 0.15).max(0.0);
        assessment.defects.push(QualityDefectScore {
            defect: "Parallax".to_string(),
            score: parallax_score.unwrap_or(0.0) as f64,
            threshold: 0.035,
            status: "warn".to_string(),
        });
    } else if let Some(score) = parallax_score {
        assessment.defects.push(QualityDefectScore {
            defect: "Parallax".to_string(),
            score: score as f64,
            threshold: 0.035,
            status: "ok".to_string(),
        });
    }

    let motion_warn = motion_score.map(|score| score >= 0.25).unwrap_or(false);
    if motion_warn {
        let issue = "Excessive motion during preview";
        if !assessment.issues.iter().any(|entry| entry == issue) {
            assessment.issues.push(issue.to_string());
        }
        let action = "Stabilize camera/turntable or slow motion before capturing.";
        if !assessment.actions.iter().any(|entry| entry == action) {
            assessment.actions.push(action.to_string());
        }
        assessment.score = (assessment.score - 0.1).max(0.0);
        assessment.defects.push(QualityDefectScore {
            defect: "Motion".to_string(),
            score: motion_score.unwrap_or(0.0) as f64,
            threshold: 0.25,
            status: "warn".to_string(),
        });
    } else if let Some(score) = motion_score {
        assessment.defects.push(QualityDefectScore {
            defect: "Motion".to_string(),
            score: score as f64,
            threshold: 0.25,
            status: "ok".to_string(),
        });
    }

    let passed = assessment.pass && parallax_ok;
    assessment.pass = passed;

    let mut wizard = state.scan_wizard.lock().await;
    record_quality_assessment(&mut wizard, assessment.clone(), uncertainty);
    wizard.last_preview = Some(frame);

    Ok(CaptureGateDecision {
        assessment,
        passed,
        parallax_score,
        motion_score,
    })
}

fn record_quality_assessment(
    wizard: &mut ScanWizardState,
    assessment: QualityAssessment,
    uncertainty: GrayImage,
) {
    wizard.last_quality = Some(assessment.clone());
    wizard.last_uncertainty = Some(uncertainty);
    let now = chrono::Utc::now();
    wizard.last_quality_at = Some(now);
    wizard.quality_history.push(WizardQualityHistoryEntry {
        captured_at: now,
        score: assessment.score,
        pass: assessment.pass,
        issues: assessment.issues.clone(),
        actions: assessment.actions.clone(),
    });
    if wizard.quality_history.len() > 100 {
        let drain = wizard.quality_history.len() - 100;
        wizard.quality_history.drain(0..drain);
    }
    if let Some(runtime) = wizard.runtime.as_mut() {
        runtime.quality = Some(assessment);
    }
}

async fn select_camera_by_role(
    state: &AppState,
    role: CameraRole,
) -> Option<Arc<dyn trueshot_device_manager::Camera>> {
    let cm = state.camera_manager.lock().await;
    for cam in &cm.cameras {
        if let Some(profile) = cm.registry.get_profile(&cam.id()) {
            if profile.role == role {
                return Some(cam.clone());
            }
        }
    }
    None
}

fn select_any_camera(state: &AppState) -> Option<Arc<dyn trueshot_device_manager::Camera>> {
    if let Ok(cm) = state.camera_manager.try_lock() {
        return cm.cameras.first().cloned();
    }
    None
}

async fn capture_background_sequence(
    state: &AppState,
) -> Result<(u32, chrono::DateTime<chrono::Utc>)> {
    let steps = 24u32;
    let mut frames: Vec<RgbImage> = Vec::new();

    for step in 0..steps {
        if let Some(mut tt) = state.turntable.lock().await.take() {
            let angle = (step as f32) * (360.0 / steps as f32);
            let _ = tt.rotate_to(angle).await;
            *state.turntable.lock().await = Some(tt);
        }

        let frame = capture_preview_rgb(state).await?;
        frames.push(frame);
        tokio::time::sleep(Duration::from_millis(80)).await;
    }

    if frames.is_empty() {
        anyhow::bail!("No frames captured for background");
    }

    let background = median_stack(&frames)?;
    let timestamp = chrono::Utc::now();

    {
        let mut wizard = state.scan_wizard.lock().await;
        wizard.background = Some(background.clone());
        wizard.background_captured_at = Some(timestamp);
        wizard.background_frames = frames.len() as u32;
    }

    let base = state.config.paths.projects_dir.join(WIZARD_PROJECT_ID);
    ensure_project_directory(
        &state.config.paths.projects_dir,
        WIZARD_PROJECT_ID,
        &base.join("state"),
    )
    .map_err(|response| anyhow::anyhow!("Create wizard state: {}", response.status()))?;
    let path = base.join("background.png");
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(background).write_to(&mut encoded, image::ImageFormat::Png)?;
    write_project_file_atomic(
        &state.config.paths.projects_dir,
        WIZARD_PROJECT_ID,
        &path,
        encoded.get_ref(),
    )
    .map_err(|response| anyhow::anyhow!("Write wizard background: {}", response.status()))?;

    Ok((frames.len() as u32, timestamp))
}

fn median_stack(frames: &[RgbImage]) -> Result<RgbImage> {
    let first = frames.first().context("No frames for median stack")?;
    let (w, h) = first.dimensions();
    let mut normalized: Vec<RgbImage> = Vec::with_capacity(frames.len());
    for frame in frames {
        if frame.dimensions() != (w, h) {
            let resized =
                image::imageops::resize(frame, w, h, image::imageops::FilterType::Lanczos3);
            normalized.push(resized);
        } else {
            normalized.push(frame.clone());
        }
    }

    let mut output = RgbImage::new(w, h);
    let mut rbuf: Vec<u8> = Vec::with_capacity(normalized.len());
    let mut gbuf: Vec<u8> = Vec::with_capacity(normalized.len());
    let mut bbuf: Vec<u8> = Vec::with_capacity(normalized.len());

    for y in 0..h {
        for x in 0..w {
            rbuf.clear();
            gbuf.clear();
            bbuf.clear();
            for frame in &normalized {
                let p = frame.get_pixel(x, y);
                rbuf.push(p[0]);
                gbuf.push(p[1]);
                bbuf.push(p[2]);
            }
            rbuf.sort_unstable();
            gbuf.sort_unstable();
            bbuf.sort_unstable();
            let mid = rbuf.len() / 2;
            output.put_pixel(x, y, Rgb([rbuf[mid], gbuf[mid], bbuf[mid]]));
        }
    }
    Ok(output)
}

#[derive(Debug, Clone)]
struct DetectionResult {
    detection: ObjectDetection,
    state: DetectionState,
    mask: Vec<u8>,
    width: u32,
    height: u32,
}

fn compute_detection(
    frame: &RgbImage,
    background: Option<&RgbImage>,
    previous: Option<&DetectionState>,
) -> DetectionResult {
    let (mask, bbox, confidence) = compute_mask_and_bbox(frame, background);
    let detected = bbox.is_some();

    let now = Instant::now();
    let previous = previous.filter(|prev| now.duration_since(prev.last_seen).as_millis() < 1500);
    let confidence = if let Some(prev) = previous {
        (confidence * 0.7 + prev.confidence * 0.3).clamp(0.0, 1.0)
    } else {
        confidence
    };

    let (stable, stable_since) = match (previous, bbox.as_ref()) {
        (Some(prev), Some(curr)) => {
            let prev_bbox = prev.bbox.as_ref();
            if let Some(prev_bbox) = prev_bbox {
                if bbox_similar(prev_bbox, curr, frame.width(), frame.height()) {
                    let since = prev.stable_since.unwrap_or(now);
                    (now.duration_since(since).as_millis() >= 800, Some(since))
                } else {
                    (false, Some(now))
                }
            } else {
                (false, Some(now))
            }
        }
        (_, Some(_)) => (false, Some(now)),
        _ => (false, None),
    };

    let stable_duration_ms = stable_since
        .map(|t| now.duration_since(t).as_millis() as u64)
        .unwrap_or(0);

    let detection = ObjectDetection {
        detected,
        confidence,
        bounding_box: bbox.clone(),
        stable,
        stable_duration_ms,
    };

    let detection_state = DetectionState {
        bbox,
        stable_since,
        last_seen: now,
        confidence,
    };

    DetectionResult {
        detection,
        state: detection_state,
        mask,
        width: frame.width(),
        height: frame.height(),
    }
}

fn compute_mask_and_bbox(
    frame: &RgbImage,
    background: Option<&RgbImage>,
) -> (Vec<u8>, Option<BoundingBox>, f32) {
    let (w, h) = frame.dimensions();
    let mut mask = vec![0u8; (w * h) as usize];
    let mut foreground = 0usize;

    if let Some(bg) = background {
        let bg = if bg.dimensions() != (w, h) {
            image::imageops::resize(bg, w, h, image::imageops::FilterType::Lanczos3)
        } else {
            bg.clone()
        };
        for (i, (p, b)) in frame.pixels().zip(bg.pixels()).enumerate() {
            let diff = (p[0] as i16 - b[0] as i16).abs()
                + (p[1] as i16 - b[1] as i16).abs()
                + (p[2] as i16 - b[2] as i16).abs();
            if diff > 60 {
                mask[i] = 255;
                foreground += 1;
            }
        }
    } else {
        let gray = DynamicImage::ImageRgb8(frame.clone()).to_luma8();
        let threshold = otsu_threshold(&gray);
        for (i, p) in gray.pixels().enumerate() {
            if p[0] > threshold {
                mask[i] = 255;
                foreground += 1;
            }
        }
    }

    let bbox = bbox_from_mask(&mask, w, h);
    let ratio = foreground as f32 / (w * h) as f32;
    let confidence = (ratio / 0.35).clamp(0.0, 1.0);
    (mask, bbox, confidence)
}

fn compute_quality_assessment(
    frame: &RgbImage,
    width: u32,
    height: u32,
    mask: &[u8],
) -> Result<(QualityAssessment, GrayImage)> {
    let rgb = rgb_to_array(frame);
    let mask = Array2::from_shape_vec((height as usize, width as usize), mask.to_vec())
        .context("Invalid mask dimensions")?;
    let depth = Array2::<f32>::zeros((height as usize, width as usize));
    let params = Arc::new(Mutex::new(ProcessingParams::default()));
    let analyzer = Analyzer::new(params);
    let assessment = analyzer.assess(&rgb, &depth, &mask)?;
    let thresholds = analyzer.thresholds();

    let mut defects = Vec::new();
    let mut actions: Vec<String> = Vec::new();
    let mut penalty = 0.0f32;

    for (defect, score) in assessment.scores.iter() {
        let threshold = thresholds.get(defect).copied().unwrap_or(0.0);
        let is_bad = if defect.is_low_bad() {
            *score < threshold
        } else {
            *score > threshold
        };
        let badness = if defect.is_low_bad() {
            if threshold > 0.0 {
                ((threshold - score) / threshold).max(0.0)
            } else {
                0.0
            }
        } else if threshold > 0.0 {
            ((score - threshold) / threshold).max(0.0)
        } else {
            0.0
        };
        let defect_penalty = (badness as f32).clamp(0.0, 1.0) * 0.2;
        if is_bad {
            penalty += defect_penalty;
            let action = defect_action(*defect);
            if !actions.iter().any(|a| a == action) {
                actions.push(action.to_string());
            }
        }
        defects.push(QualityDefectScore {
            defect: format!("{:?}", defect),
            score: *score,
            threshold,
            status: if is_bad {
                "warn".to_string()
            } else {
                "ok".to_string()
            },
        });
    }

    let score = (1.0 - penalty).clamp(0.0, 1.0);
    let quality = QualityAssessment {
        score,
        pass: assessment.pass,
        issues: assessment.reasons.clone(),
        actions,
        defects,
    };
    let uncertainty = compute_uncertainty_map(frame, width, height, mask.as_slice().unwrap_or(&[]));
    Ok((quality, uncertainty))
}

fn rgb_to_array(frame: &RgbImage) -> Array3<u8> {
    let (width, height) = frame.dimensions();
    let mut data = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            let p = frame.get_pixel(x, y);
            data.push(p[0]);
            data.push(p[1]);
            data.push(p[2]);
        }
    }
    Array3::from_shape_vec((height as usize, width as usize, 3), data)
        .unwrap_or_else(|_| Array3::zeros((height as usize, width as usize, 3)))
}

fn compute_uncertainty_map(frame: &RgbImage, width: u32, height: u32, mask: &[u8]) -> GrayImage {
    let mut out = GrayImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) as usize;
            let mut value = 255u8;
            if mask.get(idx).copied().unwrap_or(0) > 0 {
                let p = frame.get_pixel(x, y);
                let right = if x + 1 < width {
                    frame.get_pixel(x + 1, y)
                } else {
                    p
                };
                let down = if y + 1 < height {
                    frame.get_pixel(x, y + 1)
                } else {
                    p
                };
                let grad = (p[0] as i16 - right[0] as i16).unsigned_abs()
                    + (p[1] as i16 - right[1] as i16).unsigned_abs()
                    + (p[2] as i16 - right[2] as i16).unsigned_abs();
                let grad2 = (p[0] as i16 - down[0] as i16).unsigned_abs()
                    + (p[1] as i16 - down[1] as i16).unsigned_abs()
                    + (p[2] as i16 - down[2] as i16).unsigned_abs();
                let total = (grad + grad2) as f32;
                let norm = (total / (3.0 * 255.0 * 2.0)).clamp(0.0, 1.0);
                value = ((1.0 - norm) * 255.0) as u8;
            }
            out.put_pixel(x, y, image::Luma([value]));
        }
    }
    out
}

fn compute_parallax_score(
    prev: &RgbImage,
    current: &RgbImage,
    mask: &[u8],
    width: u32,
    height: u32,
) -> f32 {
    if prev.dimensions() != current.dimensions() {
        return 0.0;
    }
    let stride = 6u32;
    let mut sum = 0.0f32;
    let mut count = 0u32;
    for y in (0..height).step_by(stride as usize) {
        for x in (0..width).step_by(stride as usize) {
            let idx = (y * width + x) as usize;
            if mask.get(idx).copied().unwrap_or(0) == 0 {
                continue;
            }
            let p = current.get_pixel(x, y);
            let q = prev.get_pixel(x, y);
            let g0 = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
            let g1 = 0.299 * q[0] as f32 + 0.587 * q[1] as f32 + 0.114 * q[2] as f32;
            sum += ((g0 - g1).abs() / 255.0).min(1.0);
            count += 1;
        }
    }
    if count == 0 {
        return 0.0;
    }
    sum / count as f32
}

fn compute_motion_score(
    prev: &RgbImage,
    current: &RgbImage,
    mask: &[u8],
    width: u32,
    height: u32,
) -> f32 {
    if prev.dimensions() != current.dimensions() {
        return 0.0;
    }
    let stride = 4u32;
    let mut sum = 0.0f32;
    let mut count = 0u32;
    for y in (0..height).step_by(stride as usize) {
        for x in (0..width).step_by(stride as usize) {
            let idx = (y * width + x) as usize;
            if mask.get(idx).copied().unwrap_or(0) == 0 {
                continue;
            }
            let p = current.get_pixel(x, y);
            let q = prev.get_pixel(x, y);
            let diff = (p[0] as i16 - q[0] as i16).abs() as f32
                + (p[1] as i16 - q[1] as i16).abs() as f32
                + (p[2] as i16 - q[2] as i16).abs() as f32;
            sum += (diff / (3.0 * 255.0)).min(1.0);
            count += 1;
        }
    }
    if count == 0 {
        return 0.0;
    }
    (sum / count as f32).clamp(0.0, 1.0)
}

fn defect_action(defect: Defect) -> &'static str {
    match defect {
        Defect::EdgeBanding => "Reduce edge sharpening or increase sampling coverage.",
        Defect::BackgroundLeak => "Improve background separation or recapture background.",
        Defect::ObjectErosion => "Increase foreground contrast or adjust masking.",
        Defect::Blur => "Stabilize camera and increase shutter speed.",
        Defect::Overexposure => "Lower exposure or reduce lighting intensity.",
        Defect::ColorCast => "Rebalance white balance or adjust lighting.",
        Defect::BlackDots => "Clean lens/sensor and reduce noise.",
        Defect::RawUnderexposed => "Increase exposure or ISO to lift shadows.",
    }
}

fn bbox_from_mask(mask: &[u8], width: u32, height: u32) -> Option<BoundingBox> {
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut found = false;

    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) as usize;
            if mask[idx] > 0 {
                found = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    if !found {
        return None;
    }

    let bbox_w = max_x.saturating_sub(min_x).max(1);
    let bbox_h = max_y.saturating_sub(min_y).max(1);
    Some(BoundingBox {
        x: min_x as f32 / width as f32,
        y: min_y as f32 / height as f32,
        width: bbox_w as f32 / width as f32,
        height: bbox_h as f32 / height as f32,
    })
}

fn bbox_similar(a: &BoundingBox, b: &BoundingBox, width: u32, height: u32) -> bool {
    let ax = a.x * width as f32;
    let ay = a.y * height as f32;
    let bx = b.x * width as f32;
    let by = b.y * height as f32;
    let aw = a.width * width as f32;
    let bw = b.width * width as f32;
    let ah = a.height * height as f32;
    let bh = b.height * height as f32;

    let center_dx = ((ax + aw * 0.5) - (bx + bw * 0.5)).abs() / width as f32;
    let center_dy = ((ay + ah * 0.5) - (by + bh * 0.5)).abs() / height as f32;
    let size_dw = (aw - bw).abs() / width as f32;
    let size_dh = (ah - bh).abs() / height as f32;

    center_dx < 0.02 && center_dy < 0.02 && size_dw < 0.08 && size_dh < 0.08
}

fn otsu_threshold(gray: &GrayImage) -> u8 {
    let mut hist = [0u32; 256];
    for p in gray.pixels() {
        hist[p[0] as usize] += 1;
    }
    let total = (gray.width() * gray.height()) as f64;
    let mut sum = 0f64;
    for i in 0..256 {
        sum += (i as f64) * (hist[i] as f64);
    }

    let mut sum_b = 0f64;
    let mut w_b = 0f64;
    let mut max_var = 0f64;
    let mut threshold = 0u8;

    for i in 0..256 {
        w_b += hist[i] as f64;
        if w_b == 0.0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0.0 {
            break;
        }
        sum_b += (i as f64) * (hist[i] as f64);
        let m_b = sum_b / w_b;
        let m_f = (sum - sum_b) / w_f;
        let var_between = w_b * w_f * (m_b - m_f).powi(2);
        if var_between > max_var {
            max_var = var_between;
            threshold = i as u8;
        }
    }
    threshold
}

fn analyze_frame(
    state: &AppState,
    frame: &RgbImage,
    background: Option<&RgbImage>,
) -> Result<ObjectAnalysis> {
    let (mask, bbox, _) = compute_mask_and_bbox(frame, background);
    let bbox = bbox.context("No object detected")?;

    let (w, h) = frame.dimensions();
    let rect = bbox_to_rect(&bbox, w, h);
    let cropped = image::imageops::crop_imm(frame, rect.0, rect.1, rect.2, rect.3).to_image();
    let aspect_ratio = rect.2 as f32 / rect.3.max(1) as f32;

    let grayscale = DynamicImage::ImageRgb8(cropped.clone()).to_luma8();
    let extractor = NativeFeatureExtractor::new(3000);
    let features = extractor.detect_multiscale(&grayscale, 3);
    let feature_count = features.len() as u32;
    let score = (feature_count as f32 / 2000.0).clamp(0.0, 1.0);
    let complexity_category = if score < 0.2 {
        "simple"
    } else if score < 0.5 {
        "moderate"
    } else if score < 0.8 {
        "complex"
    } else {
        "intricate"
    };

    let model_path = std::env::var("TRUESHOT_MATERIAL_MODEL").unwrap_or_default();
    let mut estimator = MaterialEstimator::new(&model_path).or_else(|e| {
        tracing::warn!("Material model load failed: {e}. Falling back to heuristic");
        MaterialEstimator::new("")
    })?;
    let (rough_img, metal_img) = estimator.estimate(&DynamicImage::ImageRgb8(cropped.clone()))?;
    let rough = rough_img.to_luma8();
    let metal = metal_img.to_luma8();
    let (avg_rough, avg_metal) = mean_maps(&rough, &metal);

    let surface_type = if avg_metal > 0.4 {
        "metallic"
    } else if avg_rough < 0.35 {
        "glossy"
    } else if avg_rough > 0.7 {
        "matte"
    } else {
        "mixed"
    };

    let specular_ratio = (1.0 - avg_rough).clamp(0.0, 1.0);

    let turntable_diameter = state.config.hardware.turntable_diameter_cm.unwrap_or(30.0);
    let cm_per_px = turntable_diameter / w.max(1) as f32;
    let width_cm = rect.2 as f32 * cm_per_px;
    let height_cm = rect.3 as f32 * cm_per_px;
    let depth_cm = (width_cm.min(height_cm) * 0.6).max(1.0);
    let max_dim = width_cm.max(height_cm).max(depth_cm);
    let size_category = if max_dim < 5.0 {
        "tiny"
    } else if max_dim < 10.0 {
        "small"
    } else if max_dim < 20.0 {
        "medium"
    } else if max_dim < 40.0 {
        "large"
    } else {
        "xlarge"
    };

    let underside_detail = underside_detail_from_mask(&mask, w, h, &bbox);

    Ok(ObjectAnalysis {
        size: SizeInfo {
            category: size_category.to_string(),
            dimensions: [width_cm, height_cm, depth_cm],
        },
        complexity: ComplexityInfo {
            category: complexity_category.to_string(),
            feature_count,
            score,
        },
        surface: SurfaceInfo {
            surface_type: surface_type.to_string(),
            specular_ratio,
        },
        has_underside_detail: underside_detail,
        aspect_ratio,
    })
}

fn bbox_to_rect(bbox: &BoundingBox, width: u32, height: u32) -> (u32, u32, u32, u32) {
    let x = (bbox.x * width as f32).clamp(0.0, width as f32 - 1.0) as u32;
    let y = (bbox.y * height as f32).clamp(0.0, height as f32 - 1.0) as u32;
    let w = (bbox.width * width as f32).clamp(1.0, width as f32 - x as f32) as u32;
    let h = (bbox.height * height as f32).clamp(1.0, height as f32 - y as f32) as u32;
    (x, y, w, h)
}

fn mean_maps(rough: &GrayImage, metal: &GrayImage) -> (f32, f32) {
    let mut sum_r = 0f32;
    let mut sum_m = 0f32;
    let mut count = 0f32;
    for (r, m) in rough.pixels().zip(metal.pixels()) {
        sum_r += r[0] as f32 / 255.0;
        sum_m += m[0] as f32 / 255.0;
        count += 1.0;
    }
    if count == 0.0 {
        return (0.5, 0.0);
    }
    (sum_r / count, sum_m / count)
}

fn underside_detail_from_mask(mask: &[u8], width: u32, height: u32, bbox: &BoundingBox) -> bool {
    let rect = bbox_to_rect(bbox, width, height);
    let bottom_start = rect.1 + (rect.3 as f32 * 0.75) as u32;
    let mut bottom_fg = 0u32;
    let mut bottom_total = 0u32;
    for y in bottom_start..(rect.1 + rect.3) {
        for x in rect.0..(rect.0 + rect.2) {
            let idx = (y * width + x) as usize;
            bottom_total += 1;
            if mask.get(idx).copied().unwrap_or(0) > 0 {
                bottom_fg += 1;
            }
        }
    }
    if bottom_total == 0 {
        return false;
    }
    (bottom_fg as f32 / bottom_total as f32) > 0.15
}

async fn ensure_scan_session_dir(state: &AppState, session_id: &str) -> Result<PathBuf> {
    let base = state
        .config
        .paths
        .projects_dir
        .join("_wizard")
        .join(session_id);
    let raw_dir = base.join("raw");
    ensure_project_directory(
        &state.config.paths.projects_dir,
        WIZARD_PROJECT_ID,
        &raw_dir,
    )
    .map_err(|response| anyhow::anyhow!("Create scan session: {}", response.status()))?;
    Ok(raw_dir)
}

async fn ensure_manual_capture_dir(state: &AppState) -> PathBuf {
    let base = state
        .config
        .paths
        .projects_dir
        .join("_wizard")
        .join("manual");
    let _ = ensure_project_directory(&state.config.paths.projects_dir, WIZARD_PROJECT_ID, &base);
    base
}

async fn run_scan(state: &AppState, start_index: usize) -> Result<()> {
    let session_id = {
        let wizard = state.scan_wizard.lock().await;
        let runtime = wizard.runtime.as_ref().context("No active scan")?;
        runtime.session_id.clone()
    };

    let session_dir = ensure_scan_session_dir(state, &session_id).await?;
    persist_plan_history(state, &session_id).await?;

    let mut idx = start_index;
    loop {
        let step = {
            let wizard = state.scan_wizard.lock().await;
            let runtime = wizard.runtime.as_ref().context("No active scan")?;
            if idx >= runtime.plan.steps.len() {
                break;
            }
            runtime.plan.steps[idx].clone()
        };

        {
            let mut wizard = state.scan_wizard.lock().await;
            let runtime = wizard.runtime.as_mut().context("No active scan")?;
            if runtime.is_cancelled() {
                runtime.status = "stopped".to_string();
                return Ok(());
            }
            runtime.current_step = idx;
            runtime.total_steps = runtime.plan.steps.len();
            runtime.current_instruction = step.instruction.clone();
        }

        match step.step_type.as_str() {
            "camera_position" | "object_orientation" => {
                let mut wizard = state.scan_wizard.lock().await;
                if let Some(runtime) = wizard.runtime.as_mut() {
                    runtime.status = "paused".to_string();
                    runtime.waiting_step = Some(idx);
                }
                return Ok(());
            }
            "capture" => {
                {
                    let mut wizard = state.scan_wizard.lock().await;
                    let runtime = wizard.runtime.as_mut().context("No active scan")?;
                    if !runtime.auto_capture && runtime.manual_capture_step != Some(idx) {
                        runtime.status = "paused".to_string();
                        runtime.waiting_step = Some(idx);
                        return Ok(());
                    }
                    if runtime.manual_capture_step == Some(idx) {
                        runtime.manual_capture_step = None;
                    }
                }
                let gate = preflight_capture_gate(state, 2).await?;
                if !gate.passed {
                    let mut wizard = state.scan_wizard.lock().await;
                    if let Some(runtime) = wizard.runtime.as_mut() {
                        runtime.status = "paused".to_string();
                        runtime.waiting_step = Some(idx);
                        runtime.current_instruction =
                            "Adjust capture quality before continuing".to_string();
                        let mut warnings = gate.assessment.issues.clone();
                        warnings.extend(gate.assessment.actions.clone());
                        runtime.set_warnings(warnings);
                        runtime.quality = Some(gate.assessment);
                    }
                    return Ok(());
                }
                {
                    let mut wizard = state.scan_wizard.lock().await;
                    if let Some(runtime) = wizard.runtime.as_mut() {
                        runtime.set_warnings(Vec::new());
                    }
                }
                let angle = step.rotation_angle;
                let verification = perform_capture(state, angle, &session_dir).await?;
                let mut plan_updated = false;
                let mut wizard = state.scan_wizard.lock().await;
                let uncertainty = wizard.last_uncertainty.clone();
                if let Some(runtime) = wizard.runtime.as_mut() {
                    runtime.photos_captured = runtime
                        .photos_captured
                        .saturating_add(verification.verified as u32);
                    runtime.record_integrity(StepIntegrity {
                        step_index: idx as u32,
                        expected_files: verification.expected as u32,
                        verified_files: verification.verified as u32,
                        ok: verification.ok,
                        hashes: verification.hashes,
                        message: verification.message,
                    });
                    update_coverage(runtime, &step);
                    if let Some(uncertainty) = uncertainty.as_ref() {
                        let added = maybe_adapt_plan(runtime, &step, uncertainty, idx);
                        if added > 0 {
                            runtime.total_steps = runtime.plan.steps.len();
                            runtime.record_plan_revision("uncertainty_adapt", added);
                            plan_updated = true;
                        }
                    }
                }
                drop(wizard);
                if plan_updated {
                    persist_plan_history(state, &session_id).await?;
                }
            }
            _ => {}
        }
        idx += 1;
    }

    let mut wizard = state.scan_wizard.lock().await;
    if let Some(runtime) = wizard.runtime.as_mut() {
        runtime.status = "complete".to_string();
        runtime.waiting_step = None;
    }
    Ok(())
}

fn update_coverage(runtime: &mut ScanRuntime, step: &ScanStep) {
    let Some(angle) = step.rotation_angle else {
        return;
    };
    let Some(orientation) = step.object_orientation else {
        return;
    };
    let Some(camera_position) = step.camera_position else {
        return;
    };
    let orient_idx = orientation as usize;
    if orient_idx >= runtime.coverage.len() {
        return;
    }
    let grid = &mut runtime.coverage[orient_idx];
    let bin = azimuth_bin(angle, grid.azimuth_bins);
    let elevation = camera_position as usize;
    grid.update(bin, elevation, 1.0);
    let neighbor = (bin + grid.azimuth_bins - 1) % grid.azimuth_bins;
    grid.update(neighbor, elevation, 0.3);
    let neighbor = (bin + 1) % grid.azimuth_bins;
    grid.update(neighbor, elevation, 0.3);
}

fn azimuth_bin(angle: f32, bins: usize) -> usize {
    let bins = bins.max(1) as f32;
    let angle = angle.rem_euclid(360.0);
    ((angle / 360.0) * bins).floor() as usize % bins as usize
}

fn maybe_adapt_plan(
    runtime: &mut ScanRuntime,
    step: &ScanStep,
    uncertainty: &GrayImage,
    current_step: usize,
) -> usize {
    let stride = 12usize;
    if let Some(last) = runtime.last_adapt_step {
        if current_step.saturating_sub(last) < stride {
            return 0;
        }
    }
    let Some(orientation) = step.object_orientation else {
        return 0;
    };
    let Some(camera_position) = step.camera_position else {
        return 0;
    };
    let orient_idx = orientation as usize;
    if orient_idx >= runtime.coverage.len() {
        return 0;
    }
    let grid = &runtime.coverage[orient_idx];
    let uncertainty_bins = select_uncertainty_bins(uncertainty, grid.azimuth_bins);
    let candidates = rank_next_best_bins(grid, &uncertainty_bins, camera_position, 2);
    if candidates.is_empty() {
        return 0;
    }

    let insertion_idx = find_insertion_index(
        &runtime.plan.steps,
        current_step,
        camera_position,
        orientation,
    );
    let mut added = 0usize;
    let mut photo_index = runtime.plan.total_photos;
    for bin in candidates {
        let key = format!("{}:{}:{}", orientation, camera_position, bin);
        if runtime.added_view_keys.contains(&key) {
            continue;
        }
        let angle = (bin as f32 + 0.5) * (360.0 / grid.azimuth_bins as f32);
        if angle_exists(
            &runtime.plan.steps,
            angle,
            Some(camera_position),
            Some(orientation),
        ) {
            continue;
        }
        runtime.plan.steps.insert(
            insertion_idx + added,
            ScanStep {
                step_type: "capture".to_string(),
                instruction: format!("Capture photo {}", photo_index + 1),
                camera_position: Some(camera_position),
                object_orientation: Some(orientation),
                rotation_angle: Some(angle),
                photo_index: Some(photo_index),
            },
        );
        runtime.added_view_keys.insert(key);
        added += 1;
        photo_index += 1;
    }
    if added > 0 {
        runtime.plan.total_photos = photo_index;
        runtime.plan.estimated_time_seconds =
            runtime.plan.total_photos * 3 + estimate_setup_time(&runtime.plan);
        runtime.last_adapt_step = Some(current_step);
    }
    added
}

fn find_insertion_index(
    steps: &[ScanStep],
    current_idx: usize,
    camera_position: u32,
    orientation: u32,
) -> usize {
    let mut idx = current_idx + 1;
    while idx < steps.len() {
        let step = &steps[idx];
        if step.step_type != "capture" {
            break;
        }
        if step.camera_position != Some(camera_position)
            || step.object_orientation != Some(orientation)
        {
            break;
        }
        idx += 1;
    }
    idx
}

fn select_uncertainty_bins(map: &GrayImage, bins: usize) -> Vec<f32> {
    let bins = bins.max(1);
    let mut hist = vec![0f32; bins];
    let (width, height) = map.dimensions();
    let width_f = width.max(1) as f32;
    for y in 0..height {
        for x in 0..width {
            let val = map.get_pixel(x, y)[0] as f32 / 255.0;
            if val < 0.05 {
                continue;
            }
            let bin = ((x as f32 / width_f) * bins as f32).floor() as usize;
            let idx = bin.min(bins - 1);
            hist[idx] += val;
        }
    }
    hist
}

fn rank_next_best_bins(
    grid: &crate::scan_wizard::CoverageGrid,
    uncertainty_bins: &[f32],
    camera_position: u32,
    max_bins: usize,
) -> Vec<usize> {
    let mut scored = Vec::new();
    for bin in 0..grid.azimuth_bins {
        let coverage = grid.get(bin, camera_position as usize).max(0.0);
        let uncertainty = uncertainty_bins.get(bin).copied().unwrap_or(0.0);
        let score = (uncertainty + 0.15) / (coverage + 1.0);
        scored.push((bin, score));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(max_bins)
        .map(|(bin, _)| bin)
        .collect()
}

fn estimate_setup_time(plan: &ScanPlan) -> u32 {
    let orientations = plan.object_orientations.max(1);
    let camera_positions = plan.camera_positions_per_orientation.max(1);
    (orientations * 30) + (camera_positions * orientations * 15)
}

#[derive(serde::Serialize)]
struct PlanHistoryFile {
    plan: ScanPlan,
    revisions: Vec<crate::scan_wizard::PlanRevision>,
}

async fn persist_plan_history(state: &AppState, session_id: &str) -> Result<()> {
    let (plan, revisions) = {
        let wizard = state.scan_wizard.lock().await;
        let runtime = wizard.runtime.as_ref().context("No active scan")?;
        (runtime.plan.clone(), runtime.plan_history.clone())
    };
    let history = PlanHistoryFile { plan, revisions };
    let base = state
        .config
        .paths
        .projects_dir
        .join("_wizard")
        .join(session_id);
    ensure_project_directory(&state.config.paths.projects_dir, WIZARD_PROJECT_ID, &base).map_err(
        |response| anyhow::anyhow!("Create plan history directory: {}", response.status()),
    )?;
    let path = base.join("plan_history.json");
    let payload = serde_json::to_vec_pretty(&history)?;
    write_project_file_atomic(
        &state.config.paths.projects_dir,
        WIZARD_PROJECT_ID,
        &path,
        &payload,
    )
    .map_err(|response| anyhow::anyhow!("Write plan history: {}", response.status()))?;
    Ok(())
}

struct CaptureVerification {
    expected: usize,
    verified: usize,
    hashes: Vec<String>,
    ok: bool,
    message: Option<String>,
    burst_total: usize,
    burst_kept: usize,
    best_scores: Vec<f32>,
}

async fn perform_capture(
    state: &AppState,
    angle: Option<f32>,
    output_dir: &Path,
) -> Result<CaptureVerification> {
    if let Some(angle) = angle {
        if let Some(mut tt) = state.turntable.lock().await.take() {
            let _ = tt.rotate_to(angle).await;
            *state.turntable.lock().await = Some(tt);
        }
    }

    let cameras = list_cameras_by_role(state, CameraRole::HighResCapture).await;
    let fallback = list_cameras_by_role(state, CameraRole::LiveFeedback).await;
    let active = if cameras.is_empty() {
        fallback
    } else {
        cameras
    };

    if active.is_empty() {
        anyhow::bail!("No cameras available for capture");
    }

    let config = CameraConfig {
        iso: None,
        shutter_speed: None,
        aperture: None,
        wb: None,
        capture_target: Some("Memory Card".to_string()),
        resolution: None,
        fps: None,
    };

    if active.len() == 1 {
        if let Some(cal) = state
            .inventory
            .get_camera_calibration(&active[0].0)
            .ok()
            .flatten()
        {
            let _ = write_camera_calibration(state, output_dir, "calibration.json", &cal).await;
        }
    }
    for (id, _) in &active {
        if let Some(cal) = state.inventory.get_camera_calibration(id).ok().flatten() {
            let filename = format!("calibration_{}.json", sanitize_camera_id(id));
            let _ = write_camera_calibration(state, output_dir, &filename, &cal).await;
        }
    }

    let expected = active.len();
    let mut captured = 0usize;
    let mut captured_paths: Vec<PathBuf> = Vec::new();
    let mut best_scores: Vec<f32> = Vec::new();
    let mut burst_total = 0usize;
    let mut burst_kept = 0usize;
    let burst_count = capture_burst_count();
    let keep_all = capture_burst_keep_all();

    for (id, cam) in active {
        let id_safe = sanitize_camera_id(&id);
        let mut burst_paths: Vec<(PathBuf, f32)> = Vec::new();

        for burst_index in 0..burst_count {
            match cam.capture(&config) {
                Ok(path) => {
                    if path.exists() {
                        let _ =
                            wait_for_stable_file(&path, 5, std::time::Duration::from_millis(200))
                                .await;
                        let filename = format!(
                            "{}__{}_b{}__{}",
                            id_safe,
                            chrono::Utc::now().timestamp_millis(),
                            burst_index,
                            path.file_name().unwrap_or_default().to_string_lossy()
                        );
                        let dest = output_dir.join(filename);
                        if copy_into_wizard_project(state, &path, &dest, false).is_ok() {
                            let score = score_capture_image(state, &dest).await.unwrap_or(0.0);
                            burst_paths.push((dest, score));
                            burst_total += 1;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Capture failed for {} (burst {}): {}", id, burst_index, e);
                }
            }
        }

        if burst_paths.is_empty() {
            continue;
        }

        burst_paths.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let (best_path, best_score) = burst_paths[0].clone();
        best_scores.push(best_score);
        captured_paths.push(best_path.clone());
        captured += 1;
        burst_kept += 1;

        if !keep_all {
            for (path, _) in burst_paths.iter().skip(1) {
                let _ = remove_project_file_if_exists(
                    &state.config.paths.projects_dir,
                    WIZARD_PROJECT_ID,
                    path,
                );
            }
        }
    }

    if captured == 0 {
        anyhow::bail!("All camera captures failed");
    }
    let (verified, hashes) = verify_capture_files(state, &captured_paths).await?;
    if verified < expected {
        anyhow::bail!(
            "Capture verification incomplete: expected {}, verified {}",
            expected,
            verified
        );
    }
    Ok(CaptureVerification {
        expected,
        verified,
        hashes,
        ok: verified >= expected,
        message: None,
        burst_total,
        burst_kept,
        best_scores,
    })
}

async fn verify_capture_files(state: &AppState, paths: &[PathBuf]) -> Result<(usize, Vec<String>)> {
    let mut hashes = Vec::new();
    let mut verified = 0usize;
    for path in paths {
        wait_for_stable_project_file(state, path, 5, std::time::Duration::from_millis(200)).await?;
        let file =
            open_project_file_read(&state.config.paths.projects_dir, WIZARD_PROJECT_ID, path)
                .map_err(|response| anyhow::anyhow!("Open capture: {}", response.status()))?;
        let mut file = tokio::fs::File::from_std(file);
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = tokio::io::AsyncReadExt::read(&mut file, &mut buf).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        hashes.push(hex::encode(hasher.finalize()));
        verified += 1;
    }
    Ok((verified, hashes))
}

async fn wait_for_stable_file(
    path: &Path,
    attempts: usize,
    delay: std::time::Duration,
) -> Result<()> {
    let mut last_size = None;
    for _ in 0..attempts {
        let meta = tokio::fs::metadata(path).await?;
        let size = meta.len();
        if last_size == Some(size) && size > 0 {
            return Ok(());
        }
        last_size = Some(size);
        tokio::time::sleep(delay).await;
    }
    Ok(())
}

async fn wait_for_stable_project_file(
    state: &AppState,
    path: &Path,
    attempts: usize,
    delay: std::time::Duration,
) -> Result<()> {
    let mut last_size = None;
    for _ in 0..attempts {
        let file =
            open_project_file_read(&state.config.paths.projects_dir, WIZARD_PROJECT_ID, path)
                .map_err(|response| anyhow::anyhow!("Open capture: {}", response.status()))?;
        let size = file.metadata()?.len();
        if last_size == Some(size) && size > 0 {
            return Ok(());
        }
        last_size = Some(size);
        tokio::time::sleep(delay).await;
    }
    Ok(())
}

fn copy_into_wizard_project(
    state: &AppState,
    source_path: &Path,
    destination_path: &Path,
    replace: bool,
) -> Result<()> {
    let metadata = std::fs::symlink_metadata(source_path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("Capture source is not a regular file");
    }
    let mut source = std::fs::File::open(source_path)?;
    let mut staged = stage_project_file(
        &state.config.paths.projects_dir,
        WIZARD_PROJECT_ID,
        destination_path,
        replace,
    )
    .map_err(|response| anyhow::anyhow!("Stage capture: {}", response.status()))?;
    std::io::copy(&mut source, staged.file_mut())?;
    staged
        .commit()
        .map_err(|response| anyhow::anyhow!("Commit capture: {}", response.status()))
}

fn capture_burst_count() -> usize {
    let count = std::env::var("TRUESHOT_CAPTURE_BURST")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(3);
    count.clamp(1, 10)
}

fn capture_burst_keep_all() -> bool {
    std::env::var("TRUESHOT_CAPTURE_BURST_KEEP_ALL")
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

async fn score_capture_image(state: &AppState, path: &Path) -> Result<f32> {
    let mut file =
        open_project_file_read(&state.config.paths.projects_dir, WIZARD_PROJECT_ID, path)
            .map_err(|response| anyhow::anyhow!("Open burst capture: {}", response.status()))?;
    let bytes = tokio::task::spawn_blocking(move || {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes)?;
        Ok::<_, std::io::Error>(bytes)
    })
    .await??;
    let image = image::load_from_memory(&bytes)?;
    Ok(compute_burst_score(&image))
}

fn compute_burst_score(image: &DynamicImage) -> f32 {
    let sharpness = compute_laplacian_sharpness(image);
    let brightness = compute_mean_brightness(image);
    let min_brightness = std::env::var("TRUESHOT_CAPTURE_BURST_MIN_BRIGHTNESS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(20.0);
    let max_brightness = std::env::var("TRUESHOT_CAPTURE_BURST_MAX_BRIGHTNESS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(240.0);
    let mut penalty = 0.0;
    if brightness < min_brightness {
        penalty += (min_brightness - brightness) * 0.5;
    }
    if brightness > max_brightness {
        penalty += (brightness - max_brightness) * 0.5;
    }
    (sharpness - penalty).max(0.0)
}

fn compute_mean_brightness(image: &DynamicImage) -> f32 {
    let gray = image.to_luma8();
    let mut sum = 0u64;
    for p in gray.pixels() {
        sum += p[0] as u64;
    }
    let count = (gray.width() * gray.height()).max(1) as f32;
    sum as f32 / count
}

fn compute_laplacian_sharpness(image: &DynamicImage) -> f32 {
    let gray = image.to_luma8();
    let (width, height) = gray.dimensions();
    if width < 3 || height < 3 {
        return 0.0;
    }

    let mut lap_sum = 0.0f32;
    let mut lap_sq_sum = 0.0f32;
    let count = ((width - 2) * (height - 2)) as f32;

    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let center = gray.get_pixel(x, y)[0] as i32;
            let top = gray.get_pixel(x, y - 1)[0] as i32;
            let bottom = gray.get_pixel(x, y + 1)[0] as i32;
            let left = gray.get_pixel(x - 1, y)[0] as i32;
            let right = gray.get_pixel(x + 1, y)[0] as i32;

            let val = top + bottom + left + right - (4 * center);
            let val_f = val as f32;
            lap_sum += val_f;
            lap_sq_sum += val_f * val_f;
        }
    }

    let mean = lap_sum / count;
    (lap_sq_sum / count) - (mean * mean)
}

async fn write_camera_calibration(
    state: &AppState,
    output_dir: &Path,
    filename: &str,
    cal: &CameraCalibration,
) -> Result<()> {
    ensure_project_directory(
        &state.config.paths.projects_dir,
        WIZARD_PROJECT_ID,
        output_dir,
    )
    .map_err(|response| anyhow::anyhow!("Create capture directory: {}", response.status()))?;
    let payload = serde_json::json!({
        "camera_id": cal.camera_id,
        "camera_matrix": cal.camera_matrix,
        "dist_coeffs": cal.distortion,
        "rms_error": cal.rms_error,
        "width": cal.width,
        "height": cal.height,
        "updated_at": cal.updated_at.to_rfc3339(),
    });
    let path = output_dir.join(filename);
    let bytes = serde_json::to_vec_pretty(&payload)?;
    write_project_file_atomic(
        &state.config.paths.projects_dir,
        WIZARD_PROJECT_ID,
        &path,
        &bytes,
    )
    .map_err(|response| anyhow::anyhow!("Write camera calibration: {}", response.status()))?;
    Ok(())
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

async fn evaluate_calibration_freshness(state: &AppState) -> Vec<String> {
    let max_rms = state.config.server.calibration_max_rms.unwrap_or(0.8);
    let max_age_days = state.config.server.calibration_max_age_days.unwrap_or(30);
    let cm = state.camera_manager.lock().await;
    let mut warnings = Vec::new();

    for cam in &cm.cameras {
        let id = cam.id();
        let profile = match cm.registry.get_profile(&id) {
            Some(p) => p,
            None => {
                warnings.push(format!("Camera {} is not registered", id));
                continue;
            }
        };
        let cal = match &profile.calibration {
            Some(c) => c,
            None => {
                warnings.push(format!("Camera {} has no calibration profile", id));
                continue;
            }
        };

        if let Some(rms) = cal.rms_error {
            if rms > max_rms {
                warnings.push(format!(
                    "Camera {} calibration RMS {:.3} exceeds {:.3}. Recalibrate recommended.",
                    id, rms, max_rms
                ));
            }
        } else {
            warnings.push(format!("Camera {} calibration RMS missing", id));
        }

        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&cal.last_calibrated) {
            let age_days = (chrono::Utc::now() - ts.with_timezone(&chrono::Utc)).num_days();
            if age_days > max_age_days {
                warnings.push(format!(
                    "Camera {} calibration is {} days old (>{}). Recalibrate recommended.",
                    id, age_days, max_age_days
                ));
            }
        } else {
            warnings.push(format!("Camera {} calibration timestamp invalid", id));
        }
    }

    warnings
}

async fn list_cameras_by_role(
    state: &AppState,
    role: CameraRole,
) -> Vec<(String, Arc<dyn trueshot_device_manager::Camera>)> {
    let cm = state.camera_manager.lock().await;
    let mut cams = Vec::new();
    for cam in &cm.cameras {
        if let Some(profile) = cm.registry.get_profile(&cam.id()) {
            if profile.role == role {
                cams.push((cam.id(), cam.clone()));
            }
        }
    }
    cams
}

#[derive(Debug, Clone)]
struct SdCardInfo {
    root: PathBuf,
    name: String,
    image_count: u32,
    total_size_bytes: u64,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct StartScanRequest {
    #[serde(default)]
    pub auto_capture: Option<bool>,
}

fn sdcard_mount_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        vec![PathBuf::from("/Volumes")]
    }
    #[cfg(target_os = "linux")]
    {
        vec![
            PathBuf::from("/media"),
            PathBuf::from("/run/media"),
            PathBuf::from("/mnt"),
        ]
    }
    #[cfg(target_os = "windows")]
    {
        let mut roots = Vec::new();
        for drive in b'D'..=b'Z' {
            let path = format!("{}:\\", drive as char);
            roots.push(PathBuf::from(path));
        }
        roots
    }
}

fn is_media_extension(ext: &str) -> bool {
    matches!(
        ext,
        "jpg"
            | "jpeg"
            | "png"
            | "tif"
            | "tiff"
            | "heic"
            | "heif"
            | "bmp"
            | "raw"
            | "dng"
            | "cr2"
            | "cr3"
            | "nef"
            | "arw"
            | "raf"
            | "orf"
            | "rw2"
            | "srw"
    )
}

fn count_media_files(dir: &Path) -> (u32, u64) {
    let mut count = 0u32;
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        if entry.file_type().is_file() {
            let ext = entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_ascii_lowercase());
            if let Some(ext) = ext {
                if is_media_extension(&ext) {
                    count = count.saturating_add(1);
                    if let Ok(meta) = entry.metadata() {
                        total = total.saturating_add(meta.len());
                    }
                }
            }
        }
    }
    (count, total)
}

fn find_sdcard_info() -> Option<SdCardInfo> {
    for root in sdcard_mount_roots() {
        if !root.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let Ok(ft) = entry.file_type() else { continue };
                if !ft.is_dir() {
                    continue;
                }
                let path = entry.path();
                let dcim = path.join("DCIM");
                if dcim.is_dir() {
                    let (count, total) = count_media_files(&dcim);
                    return Some(SdCardInfo {
                        root: path,
                        name: entry.file_name().to_string_lossy().to_string(),
                        image_count: count,
                        total_size_bytes: total,
                    });
                }
            }
        }
    }
    None
}

fn list_media_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        if entry.file_type().is_file() {
            if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                if is_media_extension(&ext.to_ascii_lowercase()) {
                    files.push(entry.path().to_path_buf());
                }
            }
        }
    }
    files
}

async fn ensure_sdcard_import_dir(state: &AppState, session_id: &str) -> Result<PathBuf> {
    let base = state
        .config
        .paths
        .projects_dir
        .join("_wizard")
        .join("sdcard")
        .join(session_id);
    ensure_project_directory(&state.config.paths.projects_dir, WIZARD_PROJECT_ID, &base)
        .map_err(|response| anyhow::anyhow!("Create SD import directory: {}", response.status()))?;
    Ok(base)
}

// ============================================================================
// Scan Execution
// ============================================================================

/// Start scan execution
#[utoipa::path(
    post,
    path = "/api/scan/start",
    tag = "scan",
    request_body = StartScanRequest,
    responses(
        (status = 200, description = "Scan started", body = serde_json::Value),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 409, description = "Conflict")
    )
)]
#[post("/api/scan/start")]
pub async fn start_scan(
    req: HttpRequest,
    request: Option<web::Json<StartScanRequest>>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let plan = {
        let wizard = state.scan_wizard.lock().await;
        wizard.plan.clone()
    };
    let plan = match plan {
        Some(plan) => plan,
        None => return HttpResponse::BadRequest().body("No scan plan computed"),
    };
    if let Err(resp) = enforce_scan_limit(&state).await {
        return resp;
    }
    if let Err(resp) = enforce_capture_resolution(&state).await {
        return resp;
    }

    let auto_capture = request
        .as_ref()
        .and_then(|payload| payload.auto_capture)
        .unwrap_or(true);

    let step_count = plan.steps.len();
    let session_id = Uuid::new_v4().to_string();
    let mut runtime = ScanRuntime::new(plan, session_id.clone(), auto_capture);
    runtime.status = "capturing".to_string();
    let warnings = evaluate_calibration_freshness(&state).await;
    runtime.set_warnings(warnings);

    {
        let mut wizard = state.scan_wizard.lock().await;
        if let Some(existing) = wizard.runtime.as_ref() {
            if existing.status == "capturing" || existing.status == "paused" {
                return HttpResponse::Conflict().body("Scan already running");
            }
        }
        wizard.runtime = Some(runtime);
    }

    let state_clone = state.clone();
    tokio::spawn(async move {
        if let Err(e) = run_scan(&state_clone, 0).await {
            let mut wizard = state_clone.scan_wizard.lock().await;
            if let Some(runtime) = wizard.runtime.as_mut() {
                runtime.status = "error".to_string();
                runtime.error_message = Some(e.to_string());
            }
        }
    });

    info!("🚀 Scan Started");
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "scan.start",
            session_id.clone(),
            "success",
            audit_actor(&req).2,
            serde_json::json!({ "steps": step_count }),
        ),
    );
    HttpResponse::Ok().json(serde_json::json!({
        "status": "started",
        "session_id": session_id
    }))
}

/// Stop/cancel scan
#[utoipa::path(
    post,
    path = "/api/scan/stop",
    tag = "scan",
    responses(
        (status = 200, description = "Scan stopped", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/scan/stop")]
pub async fn stop_scan(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let mut wizard = state.scan_wizard.lock().await;
    if let Some(runtime) = wizard.runtime.as_mut() {
        runtime.cancel();
        runtime.status = "stopped".to_string();
    }
    info!("🛑 Scan Stopped");
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "scan.stop",
            "scan",
            "success",
            audit_actor(&req).2,
            serde_json::json!({}),
        ),
    );
    HttpResponse::Ok().json(serde_json::json!({"status": "stopped"}))
}

async fn enforce_capture_resolution(state: &web::Data<AppState>) -> Result<(), HttpResponse> {
    let max_resolution = {
        let mut gate = crate::licensing::lock_license_gate(state)?;
        gate.max_resolution()
    };
    let Some(max_resolution) = max_resolution else {
        return Ok(());
    };

    let camera_manager = state.camera_manager.lock().await;
    for profile in camera_manager.registry.profiles.values() {
        if !profile.enabled {
            continue;
        }
        if let Some(settings) = profile.last_settings.as_ref() {
            if let Some((width, height)) = settings.resolution {
                let max_dim = width.max(height);
                if max_dim > max_resolution {
                    return Err(HttpResponse::PaymentRequired().json(serde_json::json!({
                        "error": "resolution_limit_exceeded",
                        "capability": "max_resolution",
                        "message": "Camera resolution exceeds licensed maximum.",
                        "limit": max_resolution,
                        "current": max_dim,
                        "camera_id": profile.id,
                        "camera_name": profile.name,
                    })));
                }
            }
        }
    }

    Ok(())
}

/// Get current scan progress
#[utoipa::path(
    get,
    path = "/api/scan/progress",
    tag = "scan",
    responses(
        (status = 200, description = "Scan progress", body = ScanProgress),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/scan/progress")]
pub async fn get_scan_progress(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let wizard = state.scan_wizard.lock().await;
    let progress = if let Some(runtime) = wizard.runtime.as_ref() {
        runtime.progress()
    } else {
        ScanProgress {
            status: "idle".to_string(),
            current_step: 0,
            total_steps: 0,
            photos_captured: 0,
            elapsed_seconds: 0,
            current_instruction: String::new(),
            error_message: None,
            step_integrity: Vec::new(),
            warnings: Vec::new(),
            quality: None,
        }
    };

    HttpResponse::Ok().json(progress)
}

/// Get current scan coverage grid + confidence score
#[utoipa::path(
    get,
    path = "/api/scan/coverage",
    tag = "scan",
    responses(
        (status = 200, description = "Coverage status", body = CoverageStatus),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/scan/coverage")]
pub async fn get_scan_coverage(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let wizard = state.scan_wizard.lock().await;
    let runtime = match wizard.runtime.as_ref() {
        Some(runtime) => runtime,
        None => return HttpResponse::NotFound().body("No active scan"),
    };
    let mut orientation_index = 0usize;
    if let Some(step) = runtime.plan.steps.get(runtime.current_step) {
        if let Some(orientation) = step.object_orientation {
            orientation_index = orientation as usize;
        }
    }
    if orientation_index >= runtime.coverage.len() {
        orientation_index = 0;
    }
    let grid = &runtime.coverage[orientation_index];
    let total_bins = grid.azimuth_bins.max(1) * grid.elevation_bins.max(1);
    let mut coverage_hits = 0usize;
    let mut density = 0.0f32;
    for value in &grid.counts {
        if *value >= 0.9 {
            coverage_hits += 1;
        }
        density += value.min(1.0);
    }
    let total_bins_f = total_bins as f32;
    let coverage_score = if total_bins == 0 {
        0.0
    } else {
        coverage_hits as f32 / total_bins_f
    };
    let coverage_density = if total_bins == 0 {
        0.0
    } else {
        density / total_bins_f
    };
    HttpResponse::Ok().json(CoverageStatus {
        orientation_index: orientation_index as u32,
        azimuth_bins: grid.azimuth_bins as u32,
        elevation_bins: grid.elevation_bins as u32,
        counts: grid.counts.clone(),
        coverage_score,
        coverage_density,
    })
}

/// Execute a specific step (for manual progression)
#[utoipa::path(
    post,
    path = "/api/scan/execute-step",
    tag = "scan",
    request_body = ExecuteStepRequest,
    responses(
        (status = 200, description = "Step executed", body = serde_json::Value),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/scan/execute-step")]
pub async fn execute_step(
    req: HttpRequest,
    request: web::Json<ExecuteStepRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let mut next_step = request.step_index as usize + 1;
    {
        let mut wizard = state.scan_wizard.lock().await;
        let runtime = match wizard.runtime.as_mut() {
            Some(runtime) => runtime,
            None => return HttpResponse::BadRequest().body("No active scan"),
        };
        if let Some(step) = runtime.plan.steps.get(request.step_index as usize) {
            if step.step_type == "capture" {
                next_step = request.step_index as usize;
                runtime.manual_capture_step = Some(request.step_index as usize);
            }
        }
        if runtime.waiting_step != Some(request.step_index as usize) {
            return HttpResponse::BadRequest().body("Step is not awaiting confirmation");
        }
        runtime.waiting_step = None;
        runtime.status = "capturing".to_string();
        runtime.current_step = next_step;
    }

    let state_clone = state.clone();
    tokio::spawn(async move {
        if let Err(e) = run_scan(&state_clone, next_step).await {
            let mut wizard = state_clone.scan_wizard.lock().await;
            if let Some(runtime) = wizard.runtime.as_mut() {
                runtime.status = "error".to_string();
                runtime.error_message = Some(e.to_string());
            }
        }
    });

    info!("▶️ Executing step {}", request.step_index);
    HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "step_index": request.step_index
    }))
}

/// Trigger single capture at current position
#[utoipa::path(
    post,
    path = "/api/scan/capture",
    tag = "scan",
    responses(
        (status = 200, description = "Capture triggered", body = serde_json::Value),
        (status = 409, description = "Quality gate failed", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/scan/capture")]
pub async fn trigger_capture(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    info!("📸 Triggering capture");
    let session_dir = ensure_manual_capture_dir(&state).await;
    match preflight_capture_gate(&state, 2).await {
        Ok(gate) => {
            if !gate.passed {
                return HttpResponse::Conflict().json(serde_json::json!({
                    "success": false,
                    "message": "Capture quality gate failed. Adjust camera position or lighting.",
                    "quality": gate.assessment,
                    "parallax_score": gate.parallax_score,
                    "motion_score": gate.motion_score
                }));
            }
        }
        Err(err) => {
            return HttpResponse::InternalServerError().body(err.to_string());
        }
    }
    match perform_capture(&state, None, &session_dir).await {
        Ok(verification) => {
            let hashes = verification.hashes.clone();
            log_audit(
                &req,
                &state,
                AuditEvent::new(
                    audit_actor(&req).0,
                    audit_actor(&req).1,
                    "scan.capture",
                    "manual",
                    "success",
                    audit_actor(&req).2,
                    serde_json::json!({
                        "verified": verification.verified,
                        "hashes": hashes,
                        "burst_total": verification.burst_total,
                        "burst_kept": verification.burst_kept
                    }),
                ),
            );
            HttpResponse::Ok().json(serde_json::json!({
                "success": true,
                "captured": verification.verified,
                "hashes": verification.hashes,
                "burst_total": verification.burst_total,
                "burst_kept": verification.burst_kept,
                "best_scores": verification.best_scores,
                "timestamp": chrono::Utc::now().to_rfc3339()
            }))
        }
        Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
    }
}

/// Check for SD card
#[utoipa::path(
    get,
    path = "/api/scan/sdcard/status",
    tag = "scan",
    responses(
        (status = 200, description = "SD card status", body = SDCardStatus),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/scan/sdcard/status")]
pub async fn get_sdcard_status(req: HttpRequest) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let info = match tokio::task::spawn_blocking(find_sdcard_info).await {
        Ok(info) => info,
        Err(e) => return HttpResponse::InternalServerError().body(e.to_string()),
    };
    let status = if let Some(info) = info {
        SDCardStatus {
            detected: true,
            volume_name: Some(info.name),
            image_count: info.image_count,
            total_size_mb: info.total_size_bytes / (1024 * 1024),
        }
    } else {
        SDCardStatus {
            detected: false,
            volume_name: None,
            image_count: 0,
            total_size_mb: 0,
        }
    };

    HttpResponse::Ok().json(status)
}

/// Import images from SD card
#[utoipa::path(
    post,
    path = "/api/scan/sdcard/import",
    tag = "scan",
    responses(
        (status = 200, description = "SD card import complete", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/scan/sdcard/import")]
pub async fn import_from_sdcard(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    info!("📥 Starting SD card import");

    let info = match tokio::task::spawn_blocking(find_sdcard_info).await {
        Ok(info) => info,
        Err(e) => return HttpResponse::InternalServerError().body(e.to_string()),
    };
    let info = match info {
        Some(info) => info,
        None => return HttpResponse::NotFound().body("No SD card detected"),
    };

    let files = match tokio::task::spawn_blocking({
        let root = info.root.clone();
        move || list_media_files(&root.join("DCIM"))
    })
    .await
    {
        Ok(list) => list,
        Err(e) => return HttpResponse::InternalServerError().body(e.to_string()),
    };
    if files.is_empty() {
        return HttpResponse::BadRequest().body("No media files found on SD card");
    }

    let total_bytes = total_files_bytes(&files);
    let max_upload_bytes = state
        .config
        .server
        .max_upload_bytes
        .unwrap_or(10 * 1024 * 1024 * 1024);
    let max_project_bytes = state
        .config
        .server
        .max_project_bytes
        .unwrap_or(100 * 1024 * 1024 * 1024);

    let verification = match verify_sdcard_files(&files).await {
        Ok(v) => v,
        Err(e) => return HttpResponse::InternalServerError().body(e.to_string()),
    };

    let session_id = {
        let wizard = state.scan_wizard.lock().await;
        wizard
            .runtime
            .as_ref()
            .map(|r| r.session_id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string())
    };
    let dest_root = match ensure_sdcard_import_dir(&state, &session_id).await {
        Ok(p) => p,
        Err(e) => return HttpResponse::InternalServerError().body(e.to_string()),
    };
    let existing_size = dir_size_bytes(&dest_root);
    let min_free_bytes = state
        .config
        .server
        .min_free_bytes
        .unwrap_or(2 * 1024 * 1024 * 1024);
    if min_free_bytes > 0 {
        if let Some(available) = available_space_bytes(&dest_root) {
            if available < min_free_bytes.saturating_add(total_bytes) {
                return HttpResponse::build(StatusCode::INSUFFICIENT_STORAGE)
                    .body("Insufficient disk space for SD card import");
            }
        }
    }
    if total_bytes > max_upload_bytes {
        return HttpResponse::PayloadTooLarge().body("SD card import exceeds max upload size");
    }
    if existing_size.saturating_add(total_bytes) > max_project_bytes {
        return HttpResponse::PayloadTooLarge().body("Project quota exceeded");
    }
    let manifest_path = dest_root.join("sdcard_manifest.json");
    let manifest_payload = serde_json::to_vec_pretty(&serde_json::json!({
        "verified_at": chrono::Utc::now().to_rfc3339(),
        "file_count": verification.count,
        "manifest_hash": verification.manifest_hash,
        "hashes": verification.hashes,
    }))
    .unwrap_or_default();
    if let Err(response) = write_project_file_atomic(
        &state.config.paths.projects_dir,
        WIZARD_PROJECT_ID,
        &manifest_path,
        &manifest_payload,
    ) {
        return response;
    }

    let mut imported = 0u32;
    for file in files {
        let rel = file.strip_prefix(&info.root).unwrap_or(&file);
        let dest = dest_root.join(rel);
        if let Some(parent) = dest.parent() {
            if ensure_project_directory(&state.config.paths.projects_dir, WIZARD_PROJECT_ID, parent)
                .is_err()
            {
                continue;
            }
        }
        if copy_into_wizard_project(&state, &file, &dest, false).is_ok() {
            imported = imported.saturating_add(1);
        }
    }

    let manifest_hash = verification.manifest_hash.clone();
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "scan.sdcard.import",
            session_id.clone(),
            "success",
            audit_actor(&req).2,
            serde_json::json!({
                "imported_count": imported,
                "file_count": verification.count,
                "manifest_hash": manifest_hash,
            }),
        ),
    );

    HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "imported_count": imported,
        "session_id": session_id,
        "volume_name": info.name,
        "verification": {
            "file_count": verification.count,
            "manifest_hash": verification.manifest_hash,
            "manifest_path": manifest_path.to_string_lossy()
        }
    }))
}

struct SdcardVerification {
    count: usize,
    hashes: Vec<String>,
    manifest_hash: String,
}

async fn verify_sdcard_files(files: &[PathBuf]) -> Result<SdcardVerification> {
    let mut hashes = Vec::with_capacity(files.len());
    for path in files {
        wait_for_stable_file(path, 5, std::time::Duration::from_millis(200)).await?;
        let mut file = tokio::fs::File::open(path).await?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = tokio::io::AsyncReadExt::read(&mut file, &mut buf).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        hashes.push(hex::encode(hasher.finalize()));
    }

    let mut manifest_hasher = Sha256::new();
    for hash in &hashes {
        manifest_hasher.update(hash.as_bytes());
    }
    let manifest_hash = hex::encode(manifest_hasher.finalize());

    Ok(SdcardVerification {
        count: hashes.len(),
        hashes,
        manifest_hash,
    })
}

fn total_files_bytes(files: &[PathBuf]) -> u64 {
    let mut total = 0u64;
    for file in files {
        if let Ok(meta) = std::fs::metadata(file) {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

fn dir_size_bytes(root: &Path) -> u64 {
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        if entry.file_type().is_file() {
            if let Ok(meta) = entry.metadata() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
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
