use crate::auth::require_admin;
use crate::licensing::require_license_feature;
use crate::state::AppState;
use actix_web::{get, post, web, HttpRequest, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use trueshot_core::capture::{
    build_camera_candidates, observe_nef_reference, observe_nef_roi, AdaptiveCaptureTermination,
    AdaptivePlannerConfig, AdaptiveSessionStatus, CaptureRuntimeTelemetry, MeasuredAdaptiveSession,
    RawAssimilationReport, RawObservationConfig,
};
use trueshot_core::licensing::Feature;
use trueshot_core::nef::raw_data::Roi;
use trueshot_core::sensor_noise::SensorNoiseProfile;
use uuid::Uuid;

const MAX_ADAPTIVE_SESSIONS: usize = 32;
const MAX_FOCUS_CANDIDATES: usize = 256;
const MAX_API_CANDIDATES: usize = 4_096;

#[derive(Default)]
pub struct AdaptiveCaptureSessions {
    sessions: HashMap<Uuid, ServerAdaptiveSession>,
}

struct ServerAdaptiveSession {
    camera_id: String,
    core: MeasuredAdaptiveSession,
    sensor_profile: SensorNoiseProfile,
    roi: Roi,
    observation_config: RawObservationConfig,
    generation: u64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct RoiRequest {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl From<RoiRequest> for Roi {
    fn from(value: RoiRequest) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct StartAdaptiveCaptureRequest {
    pub camera_id: String,
    pub reference_path: PathBuf,
    pub sensor_profile: SensorNoiseProfile,
    pub roi: RoiRequest,
    pub focus_diopters: Vec<f32>,
    pub readout_ms: f32,
    pub settle_ms: f32,
    #[serde(default)]
    pub planner: AdaptivePlannerConfig,
    #[serde(default)]
    pub observation: RawObservationConfig,
}

#[derive(Debug, Serialize)]
pub struct AdaptiveCaptureResponse {
    pub session_id: Uuid,
    pub camera_id: String,
    pub generation: u64,
    pub status: AdaptiveSessionStatus,
}

#[derive(Debug, Deserialize)]
pub struct AssimilateAdaptiveCaptureRequest {
    pub capture_path: PathBuf,
    pub telemetry: CaptureRuntimeTelemetry,
}

#[derive(Debug, Serialize)]
pub struct AdaptiveAssimilationResponse {
    pub session_id: Uuid,
    pub generation: u64,
    pub report: RawAssimilationReport,
    pub status: AdaptiveSessionStatus,
}

#[derive(Debug, Deserialize)]
pub struct TerminateAdaptiveCaptureRequest {
    pub reason: AdaptiveCaptureTermination,
}

#[utoipa::path(
    post,
    path = "/api/cameras/adaptive",
    tag = "hardware",
    request_body = serde_json::Value,
    responses(
        (status = 201, description = "Adaptive session created", body = serde_json::Value),
        (status = 400, description = "Invalid camera, calibration, reference, or candidates"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Feature not licensed"),
        (status = 429, description = "Active session limit reached")
    )
)]
#[post("/api/cameras/adaptive")]
pub async fn start_adaptive_capture(
    req: HttpRequest,
    json: web::Json<StartAdaptiveCaptureRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(response) = require_admin(&req) {
        return response;
    }
    if let Err(response) = require_license_feature(
        &state,
        Feature::AdvancedCaptureAutomation,
        "advanced_capture_automation",
    ) {
        return response;
    }
    if json.focus_diopters.is_empty() || json.focus_diopters.len() > MAX_FOCUS_CANDIDATES {
        return HttpResponse::BadRequest().body("focus_diopters must contain 1..=256 values");
    }

    let capabilities = {
        let manager = state.camera_manager.lock().await;
        if manager.get_camera_by_id(&json.camera_id).is_none() {
            return HttpResponse::NotFound().body("Camera not connected");
        }
        let Some(profile) = manager.registry.get_profile(&json.camera_id) else {
            return HttpResponse::BadRequest().body("Camera has no registered capabilities");
        };
        profile.capabilities.clone()
    };
    let candidates = match build_camera_candidates(
        &capabilities.shutter_speed_options,
        &capabilities.iso_options,
        &json.focus_diopters,
        json.readout_ms,
        json.settle_ms,
    ) {
        Ok(report) => report.candidates,
        Err(error) => return HttpResponse::BadRequest().body(error.to_string()),
    };
    if candidates.len() > MAX_API_CANDIDATES {
        return HttpResponse::BadRequest().body(format!(
            "Adaptive API candidate grid has {} entries; limit is {}",
            candidates.len(),
            MAX_API_CANDIDATES
        ));
    }
    let reference_path =
        match project_local_file(&state.config.paths.projects_dir, &json.reference_path) {
            Ok(path) => path,
            Err(error) => return HttpResponse::BadRequest().body(error.to_string()),
        };
    let sensor_profile = json.sensor_profile.clone();
    let roi = Roi::from(json.roi);
    let decode_roi = roi.clone();
    let observation_config = json.observation;
    let reference = match tokio::task::spawn_blocking({
        let sensor_profile = sensor_profile.clone();
        move || {
            observe_nef_reference(
                &reference_path,
                decode_roi,
                &sensor_profile,
                observation_config,
            )
        }
    })
    .await
    {
        Ok(Ok(observation)) => observation,
        Ok(Err(error)) => return HttpResponse::BadRequest().body(error.to_string()),
        Err(error) => {
            return HttpResponse::InternalServerError()
                .body(format!("Adaptive reference task failed: {error}"))
        }
    };
    let core = match MeasuredAdaptiveSession::start(
        reference,
        sensor_profile.clone(),
        candidates,
        json.planner,
    ) {
        Ok(session) => session,
        Err(error) => return HttpResponse::BadRequest().body(error.to_string()),
    };
    let session_id = Uuid::new_v4();
    let camera_id = json.camera_id.clone();
    let status = core.status();
    let mut sessions = state.adaptive_capture.lock().await;
    if sessions.sessions.len() >= MAX_ADAPTIVE_SESSIONS {
        sessions
            .sessions
            .retain(|_, session| !session.core.is_complete());
    }
    if sessions.sessions.len() >= MAX_ADAPTIVE_SESSIONS {
        return HttpResponse::TooManyRequests().body("Too many active adaptive capture sessions");
    }
    sessions.sessions.insert(
        session_id,
        ServerAdaptiveSession {
            camera_id: camera_id.clone(),
            core,
            sensor_profile,
            roi,
            observation_config,
            generation: 0,
        },
    );
    HttpResponse::Created().json(AdaptiveCaptureResponse {
        session_id,
        camera_id,
        generation: 0,
        status,
    })
}

#[utoipa::path(
    post,
    path = "/api/cameras/adaptive/{session_id}/observe",
    tag = "hardware",
    params(("session_id" = String, Path, description = "Adaptive session UUID")),
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "RAW verified and assimilated", body = serde_json::Value),
        (status = 400, description = "RAW or telemetry rejected"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Feature not licensed"),
        (status = 404, description = "Session not found"),
        (status = 409, description = "Concurrent session advance")
    )
)]
#[post("/api/cameras/adaptive/{session_id}/observe")]
pub async fn assimilate_adaptive_capture(
    req: HttpRequest,
    path: web::Path<Uuid>,
    json: web::Json<AssimilateAdaptiveCaptureRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(response) = require_admin(&req) {
        return response;
    }
    if let Err(response) = require_license_feature(
        &state,
        Feature::AdvancedCaptureAutomation,
        "advanced_capture_automation",
    ) {
        return response;
    }
    let session_id = path.into_inner();
    let (sensor_profile, anchor, roi, observation_config, expected_generation) = {
        let sessions = state.adaptive_capture.lock().await;
        let Some(session) = sessions.sessions.get(&session_id) else {
            return HttpResponse::NotFound().body("Adaptive capture session not found");
        };
        (
            session.sensor_profile.clone(),
            session.core.status().posterior.radiance_anchor_exposure,
            session.roi.clone(),
            session.observation_config,
            session.generation,
        )
    };
    let capture_path =
        match project_local_file(&state.config.paths.projects_dir, &json.capture_path) {
            Ok(path) => path,
            Err(error) => return HttpResponse::BadRequest().body(error.to_string()),
        };
    let observation = match tokio::task::spawn_blocking(move || {
        observe_nef_roi(
            &capture_path,
            roi,
            &sensor_profile,
            anchor,
            observation_config,
        )
    })
    .await
    {
        Ok(Ok(observation)) => observation,
        Ok(Err(error)) => return HttpResponse::BadRequest().body(error.to_string()),
        Err(error) => {
            return HttpResponse::InternalServerError()
                .body(format!("Adaptive observation task failed: {error}"))
        }
    };

    let mut sessions = state.adaptive_capture.lock().await;
    let Some(session) = sessions.sessions.get_mut(&session_id) else {
        return HttpResponse::NotFound().body("Adaptive capture session not found");
    };
    if session.generation != expected_generation {
        return HttpResponse::Conflict().body("Adaptive capture session advanced concurrently");
    }
    let report = match session
        .core
        .assimilate_selected(observation, json.telemetry)
    {
        Ok(report) => report,
        Err(error) => return HttpResponse::BadRequest().body(error.to_string()),
    };
    session.generation += 1;
    HttpResponse::Ok().json(AdaptiveAssimilationResponse {
        session_id,
        generation: session.generation,
        report,
        status: session.core.status(),
    })
}

#[utoipa::path(
    get,
    path = "/api/cameras/adaptive/{session_id}",
    tag = "hardware",
    params(("session_id" = String, Path, description = "Adaptive session UUID")),
    responses(
        (status = 200, description = "Adaptive session status", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Feature not licensed"),
        (status = 404, description = "Session not found")
    )
)]
#[get("/api/cameras/adaptive/{session_id}")]
pub async fn get_adaptive_capture(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(response) = require_admin(&req) {
        return response;
    }
    if let Err(response) = require_license_feature(
        &state,
        Feature::AdvancedCaptureAutomation,
        "advanced_capture_automation",
    ) {
        return response;
    }
    let session_id = path.into_inner();
    let sessions = state.adaptive_capture.lock().await;
    let Some(session) = sessions.sessions.get(&session_id) else {
        return HttpResponse::NotFound().body("Adaptive capture session not found");
    };
    HttpResponse::Ok().json(AdaptiveCaptureResponse {
        session_id,
        camera_id: session.camera_id.clone(),
        generation: session.generation,
        status: session.core.status(),
    })
}

#[utoipa::path(
    get,
    path = "/api/cameras/adaptive/{session_id}/provenance",
    tag = "hardware",
    params(("session_id" = String, Path, description = "Adaptive session UUID")),
    responses(
        (status = 200, description = "Adaptive provenance trace", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Feature not licensed"),
        (status = 404, description = "Session not found")
    )
)]
#[get("/api/cameras/adaptive/{session_id}/provenance")]
pub async fn get_adaptive_capture_provenance(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(response) = require_admin(&req) {
        return response;
    }
    if let Err(response) = require_license_feature(
        &state,
        Feature::AdvancedCaptureAutomation,
        "advanced_capture_automation",
    ) {
        return response;
    }
    let sessions = state.adaptive_capture.lock().await;
    let Some(session) = sessions.sessions.get(&path.into_inner()) else {
        return HttpResponse::NotFound().body("Adaptive capture session not found");
    };
    HttpResponse::Ok().json(session.core.provenance())
}

#[utoipa::path(
    post,
    path = "/api/cameras/adaptive/{session_id}/terminate",
    tag = "hardware",
    params(("session_id" = String, Path, description = "Adaptive session UUID")),
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Adaptive session terminated", body = serde_json::Value),
        (status = 400, description = "Invalid termination"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Feature not licensed"),
        (status = 404, description = "Session not found")
    )
)]
#[post("/api/cameras/adaptive/{session_id}/terminate")]
pub async fn terminate_adaptive_capture(
    req: HttpRequest,
    path: web::Path<Uuid>,
    json: web::Json<TerminateAdaptiveCaptureRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(response) = require_admin(&req) {
        return response;
    }
    if let Err(response) = require_license_feature(
        &state,
        Feature::AdvancedCaptureAutomation,
        "advanced_capture_automation",
    ) {
        return response;
    }
    let session_id = path.into_inner();
    let mut sessions = state.adaptive_capture.lock().await;
    let Some(session) = sessions.sessions.get_mut(&session_id) else {
        return HttpResponse::NotFound().body("Adaptive capture session not found");
    };
    if let Err(error) = session.core.terminate(json.reason) {
        return HttpResponse::BadRequest().body(error.to_string());
    }
    session.generation += 1;
    HttpResponse::Ok().json(AdaptiveCaptureResponse {
        session_id,
        camera_id: session.camera_id.clone(),
        generation: session.generation,
        status: session.core.status(),
    })
}

fn project_local_file(projects_root: &Path, requested: &Path) -> anyhow::Result<PathBuf> {
    let root = std::fs::canonicalize(projects_root)?;
    let path = std::fs::canonicalize(requested)?;
    if !path.starts_with(&root) || !path.is_file() {
        anyhow::bail!("Adaptive capture files must be regular files under the projects directory");
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn project_file_gate_rejects_escape_and_symlink_escape() {
        let base = std::env::temp_dir().join(format!("trueshot-adaptive-{}", Uuid::new_v4()));
        let root = base.join("projects");
        fs::create_dir_all(&root).unwrap();
        let inside = root.join("inside.nef");
        let outside = base.join("outside.nef");
        fs::write(&inside, b"inside").unwrap();
        fs::write(&outside, b"outside").unwrap();
        assert_eq!(
            project_local_file(&root, &inside).unwrap(),
            fs::canonicalize(&inside).unwrap()
        );
        assert!(project_local_file(&root, &outside).is_err());

        #[cfg(unix)]
        {
            let link = root.join("escape.nef");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            assert!(project_local_file(&root, &link).is_err());
        }
        fs::remove_dir_all(base).unwrap();
    }
}
