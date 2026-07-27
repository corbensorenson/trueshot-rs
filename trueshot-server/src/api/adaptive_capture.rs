use crate::at_rest::{
    encrypt_file_in_place, policy_for_project, require_master_key, ProjectKeyStore,
};
use crate::auth::require_admin;
use crate::fs_safety::ensure_project_id;
use crate::licensing::require_license_feature;
use crate::state::AppState;
use actix_web::{get, post, web, HttpRequest, HttpResponse, Responder};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;
use trueshot_core::capture::{
    build_camera_candidates, observe_nef_reference, observe_nef_roi,
    resolve_camera_candidate_options, AdaptiveCaptureTermination, AdaptivePlannerConfig,
    AdaptiveSessionStatus, CameraOptionSelection, CaptureCandidate, CaptureRuntimeTelemetry,
    MeasuredAdaptiveSession, MeasuredAdaptiveSessionSnapshot, RawAssimilationReport,
    RawObservationConfig,
};
use trueshot_core::licensing::Feature;
use trueshot_core::nef::raw_data::Roi;
use trueshot_core::sensor_noise::SensorNoiseProfile;
use trueshot_device_manager::CameraConfig;
use uuid::Uuid;

const MAX_ADAPTIVE_SESSIONS: usize = 32;
const MAX_FOCUS_CANDIDATES: usize = 256;
const MAX_API_CANDIDATES: usize = 4_096;
const MAX_SESSION_ARTIFACTS: usize = 256;
const MAX_SESSION_SNAPSHOT_BYTES: u64 = 16 * 1024 * 1024;
const RETAINED_SESSION_GENERATIONS: usize = 2;
const SERVER_SESSION_SCHEMA: &str = "trueshot.server-adaptive-session.v1";
const SERVER_SESSION_ENVELOPE_SCHEMA: &str = "trueshot.server-adaptive-envelope.v1";

pub struct AdaptiveCaptureSessions {
    sessions: HashMap<Uuid, ServerAdaptiveSession>,
    storage_dir: PathBuf,
}

#[derive(Clone)]
struct ServerAdaptiveSession {
    camera_id: String,
    project_id: Option<String>,
    aperture_option: Option<String>,
    core: MeasuredAdaptiveSession,
    roi: Roi,
    observation_config: RawObservationConfig,
    generation: u64,
    capture_active: Arc<AtomicBool>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableSessionSnapshot {
    schema: String,
    session_id: String,
    camera_id: String,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    aperture_option: Option<String>,
    generation: u64,
    roi: [u32; 4],
    observation_config: RawObservationConfig,
    core: MeasuredAdaptiveSessionSnapshot,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableSessionEnvelope {
    schema: String,
    payload_sha256: String,
    payload: DurableSessionSnapshot,
}

impl AdaptiveCaptureSessions {
    pub fn load(projects_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(projects_dir)?;
        let projects_root = std::fs::canonicalize(projects_dir)?;
        let storage_dir = projects_dir.join("_adaptive_sessions");
        std::fs::create_dir_all(&storage_dir)?;
        if std::fs::symlink_metadata(&storage_dir)?
            .file_type()
            .is_symlink()
        {
            anyhow::bail!("Adaptive session storage directory cannot be a symlink");
        }
        let storage_dir = std::fs::canonicalize(storage_dir)?;
        if !storage_dir.starts_with(&projects_root) {
            anyhow::bail!("Adaptive session storage escaped the projects directory");
        }
        let mut artifacts = HashMap::<Uuid, Vec<(u64, PathBuf)>>::new();
        let mut artifact_count = 0usize;
        let mut removed_partial = false;
        for entry in std::fs::read_dir(&storage_dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".part") {
                std::fs::remove_file(entry.path())?;
                removed_partial = true;
                continue;
            }
            let Some((session_id, generation)) = parse_snapshot_filename(&name) else {
                if name.ends_with(".json") {
                    anyhow::bail!("Invalid adaptive session artifact name {name}");
                }
                continue;
            };
            artifact_count = artifact_count
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("Adaptive session artifact count overflow"))?;
            if artifact_count > MAX_SESSION_ARTIFACTS {
                anyhow::bail!(
                    "Adaptive session artifact count exceeds {}",
                    MAX_SESSION_ARTIFACTS
                );
            }
            artifacts
                .entry(session_id)
                .or_default()
                .push((generation, entry.path()));
        }
        if removed_partial {
            File::open(&storage_dir)?.sync_all()?;
        }
        if artifacts.len() > MAX_ADAPTIVE_SESSIONS {
            anyhow::bail!("Adaptive session count exceeds {}", MAX_ADAPTIVE_SESSIONS);
        }

        let mut sessions = HashMap::with_capacity(artifacts.len());
        for (session_id, mut generations) in artifacts {
            generations.sort_unstable_by(|left, right| right.0.cmp(&left.0));
            let newest_generation = generations[0].0;
            let mut restored = None;
            let mut failures = Vec::new();
            for (generation, path) in generations {
                match read_durable_session(&path, session_id, generation) {
                    Ok(session) => {
                        if generation != newest_generation {
                            tracing::warn!(
                                %session_id,
                                newest_generation,
                                recovered_generation = generation,
                                "Recovered adaptive session from prior valid generation"
                            );
                            for (failed_path, _) in &failures {
                                std::fs::remove_file(failed_path)?;
                            }
                            File::open(&storage_dir)?.sync_all()?;
                        }
                        restored = Some(session);
                        break;
                    }
                    Err(error) => failures.push((path, format!("{error:#}"))),
                }
            }
            let session = restored.ok_or_else(|| {
                anyhow::anyhow!(
                    "No valid checkpoint remains for adaptive session {session_id}: {}",
                    failures
                        .iter()
                        .map(|(path, error)| format!("{}: {error}", path.display()))
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            })?;
            sessions.insert(session_id, session);
        }
        Ok(Self {
            sessions,
            storage_dir,
        })
    }

    fn make_room(&mut self) -> anyhow::Result<bool> {
        if self.sessions.len() < MAX_ADAPTIVE_SESSIONS {
            return Ok(true);
        }
        let mut completed = self
            .sessions
            .iter()
            .filter_map(|(id, session)| session.core.is_complete().then_some(*id))
            .collect::<Vec<_>>();
        completed.sort_unstable();
        for session_id in completed {
            self.remove_durable_session(session_id)?;
            self.sessions.remove(&session_id);
            if self.sessions.len() < MAX_ADAPTIVE_SESSIONS {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn insert_new(
        &mut self,
        session_id: Uuid,
        session: ServerAdaptiveSession,
    ) -> anyhow::Result<()> {
        if session.generation != 0 || self.sessions.contains_key(&session_id) {
            anyhow::bail!("Adaptive session identity or initial generation is invalid");
        }
        persist_durable_session(&self.storage_dir, session_id, &session)?;
        self.sessions.insert(session_id, session);
        Ok(())
    }

    fn assimilate(
        &mut self,
        session_id: Uuid,
        expected_generation: u64,
        observation: trueshot_core::capture::RawCaptureObservation,
        telemetry: CaptureRuntimeTelemetry,
    ) -> anyhow::Result<(RawAssimilationReport, ServerAdaptiveSession)> {
        let current = self
            .sessions
            .get(&session_id)
            .ok_or_else(|| anyhow::anyhow!("Adaptive capture session not found"))?;
        if current.generation != expected_generation {
            anyhow::bail!("Adaptive capture session advanced concurrently");
        }
        let mut next = current.clone();
        let report = next.core.assimilate_selected(observation, telemetry)?;
        next.generation = next
            .generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Adaptive session generation overflow"))?;
        persist_durable_session(&self.storage_dir, session_id, &next)?;
        self.sessions.insert(session_id, next.clone());
        Ok((report, next))
    }

    fn terminate(
        &mut self,
        session_id: Uuid,
        reason: AdaptiveCaptureTermination,
    ) -> anyhow::Result<ServerAdaptiveSession> {
        let current = self
            .sessions
            .get(&session_id)
            .ok_or_else(|| anyhow::anyhow!("Adaptive capture session not found"))?;
        let mut next = current.clone();
        next.core.terminate(reason)?;
        next.generation = next
            .generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Adaptive session generation overflow"))?;
        persist_durable_session(&self.storage_dir, session_id, &next)?;
        self.sessions.insert(session_id, next.clone());
        Ok(next)
    }

    fn remove_durable_session(&self, session_id: Uuid) -> anyhow::Result<()> {
        let prefix = format!("{session_id}.");
        let mut removed = false;
        for entry in std::fs::read_dir(&self.storage_dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if file_type.is_file()
                && name.starts_with(&prefix)
                && (name.ends_with(".json") || name.ends_with(".part"))
            {
                std::fs::remove_file(entry.path())?;
                removed = true;
            }
        }
        if removed {
            File::open(&self.storage_dir)?.sync_all()?;
        }
        Ok(())
    }
}

fn snapshot_filename(session_id: Uuid, generation: u64) -> String {
    format!("{session_id}.{generation:020}.json")
}

fn parse_snapshot_filename(name: &str) -> Option<(Uuid, u64)> {
    let stem = name.strip_suffix(".json")?;
    let (session, generation) = stem.rsplit_once('.')?;
    if generation.len() != 20 || !generation.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((Uuid::parse_str(session).ok()?, generation.parse().ok()?))
}

fn durable_payload(session_id: Uuid, session: &ServerAdaptiveSession) -> DurableSessionSnapshot {
    DurableSessionSnapshot {
        schema: SERVER_SESSION_SCHEMA.to_string(),
        session_id: session_id.to_string(),
        camera_id: session.camera_id.clone(),
        project_id: session.project_id.clone(),
        aperture_option: session.aperture_option.clone(),
        generation: session.generation,
        roi: [
            session.roi.x,
            session.roi.y,
            session.roi.width,
            session.roi.height,
        ],
        observation_config: session.observation_config,
        core: session.core.snapshot(),
    }
}

fn persist_durable_session(
    storage_dir: &Path,
    session_id: Uuid,
    session: &ServerAdaptiveSession,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(storage_dir)?;
    let payload = durable_payload(session_id, session);
    let payload_bytes = serde_json::to_vec(&payload)?;
    let envelope = DurableSessionEnvelope {
        schema: SERVER_SESSION_ENVELOPE_SCHEMA.to_string(),
        payload_sha256: hex::encode(Sha256::digest(&payload_bytes)),
        payload,
    };
    let mut bytes = serde_json::to_vec_pretty(&envelope)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_SESSION_SNAPSHOT_BYTES {
        anyhow::bail!("Adaptive session checkpoint exceeds the size limit");
    }

    let final_path = storage_dir.join(snapshot_filename(session_id, session.generation));
    let partial_path = storage_dir.join(format!(
        ".{session_id}.{:020}.{}.part",
        session.generation,
        Uuid::new_v4()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial_path)?;
    let publish_result = (|| -> anyhow::Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::hard_link(&partial_path, &final_path)?;
        std::fs::remove_file(&partial_path)?;
        File::open(storage_dir)?.sync_all()?;
        if let Err(error) = prune_old_generations(storage_dir, session_id) {
            tracing::warn!(
                %session_id,
                generation = session.generation,
                %error,
                "Adaptive session checkpoint published but old-generation cleanup failed"
            );
        }
        Ok(())
    })();
    if publish_result.is_err() {
        let _ = std::fs::remove_file(&partial_path);
    }
    publish_result
}

fn prune_old_generations(storage_dir: &Path, session_id: Uuid) -> anyhow::Result<()> {
    let mut generations = Vec::new();
    for entry in std::fs::read_dir(storage_dir)? {
        let entry = entry?;
        let Some((artifact_id, generation)) =
            parse_snapshot_filename(&entry.file_name().to_string_lossy())
        else {
            continue;
        };
        if artifact_id == session_id && entry.file_type()?.is_file() {
            generations.push((generation, entry.path()));
        }
    }
    generations.sort_unstable_by(|left, right| right.0.cmp(&left.0));
    let mut removed = false;
    for (_, path) in generations.into_iter().skip(RETAINED_SESSION_GENERATIONS) {
        std::fs::remove_file(path)?;
        removed = true;
    }
    if removed {
        File::open(storage_dir)?.sync_all()?;
    }
    Ok(())
}

fn read_durable_session(
    path: &Path,
    expected_session_id: Uuid,
    expected_generation: u64,
) -> anyhow::Result<ServerAdaptiveSession> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_SESSION_SNAPSHOT_BYTES {
        anyhow::bail!("Adaptive session checkpoint is not a bounded regular file");
    }
    let bytes = std::fs::read(path)?;
    let envelope: DurableSessionEnvelope = serde_json::from_slice(&bytes)?;
    if envelope.schema != SERVER_SESSION_ENVELOPE_SCHEMA {
        anyhow::bail!("Unsupported adaptive session envelope schema");
    }
    let payload_bytes = serde_json::to_vec(&envelope.payload)?;
    let expected_digest = hex::encode(Sha256::digest(&payload_bytes));
    if envelope.payload_sha256 != expected_digest {
        anyhow::bail!("Adaptive session checkpoint digest mismatch");
    }
    let payload = envelope.payload;
    if payload.schema != SERVER_SESSION_SCHEMA
        || payload.session_id != expected_session_id.to_string()
        || payload.generation != expected_generation
        || payload.camera_id.trim().is_empty()
        || payload.roi[2] == 0
        || payload.roi[3] == 0
    {
        anyhow::bail!("Adaptive session checkpoint identity or bounds are invalid");
    }
    payload.observation_config.validate()?;
    Ok(ServerAdaptiveSession {
        camera_id: payload.camera_id,
        project_id: payload.project_id,
        aperture_option: payload.aperture_option,
        core: MeasuredAdaptiveSession::restore(payload.core)?,
        roi: Roi {
            x: payload.roi[0],
            y: payload.roi[1],
            width: payload.roi[2],
            height: payload.roi[3],
        },
        observation_config: payload.observation_config,
        generation: payload.generation,
        capture_active: Arc::new(AtomicBool::new(false)),
    })
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
    pub project_id: String,
    /// Project-relative path such as `raw/reference.NEF`.
    pub reference_raw_path: String,
    /// Project-relative calibrated profile JSON path.
    pub sensor_profile_path: String,
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
    pub project_id: Option<String>,
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
pub struct CaptureNextAdaptiveRequest {
    /// The operator has moved an uncalibrated lens to the recommended physical
    /// distance. The completed NEF remains the authority and is rejected on a
    /// metadata mismatch.
    pub confirmed_focus_diopters: f32,
    #[serde(default)]
    pub motion_pixels_per_second: f32,
    #[serde(default)]
    pub thermal_load: f32,
}

#[derive(Debug, Serialize)]
pub struct CaptureNextAdaptiveResponse {
    pub session_id: Uuid,
    pub generation: u64,
    pub capture_path: String,
    pub selected: CaptureCandidate,
    pub camera_options: CameraOptionSelection,
    pub measured_capture_elapsed_ms: f32,
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
    let reference_path = match project_asset_file(
        &state.config.paths.projects_dir,
        &json.project_id,
        &json.reference_raw_path,
        &["raw"],
    ) {
        Ok(path) => path,
        Err(error) => return HttpResponse::BadRequest().body(error.to_string()),
    };
    let sensor_profile_path = match project_asset_file(
        &state.config.paths.projects_dir,
        &json.project_id,
        &json.sensor_profile_path,
        &["raw", "processed"],
    ) {
        Ok(path) => path,
        Err(error) => return HttpResponse::BadRequest().body(error.to_string()),
    };
    if reference_path.extension().and_then(|value| value.to_str()) == Some("enc")
        || sensor_profile_path
            .extension()
            .and_then(|value| value.to_str())
            == Some("enc")
    {
        return HttpResponse::BadRequest().body(
            "Adaptive capture currently requires seekable plaintext project assets; decrypt this project scope before starting",
        );
    }
    let roi = Roi::from(json.roi);
    let decode_roi = roi.clone();
    let observation_config = json.observation;
    let (sensor_profile, reference) = match tokio::task::spawn_blocking({
        move || {
            let sensor_profile = SensorNoiseProfile::load_json(&sensor_profile_path)?;
            let reference = observe_nef_reference(
                &reference_path,
                decode_roi,
                &sensor_profile,
                observation_config,
            )?;
            anyhow::Ok((sensor_profile, reference))
        }
    })
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => return HttpResponse::BadRequest().body(error.to_string()),
        Err(error) => {
            return HttpResponse::InternalServerError()
                .body(format!("Adaptive reference task failed: {error}"))
        }
    };
    let aperture_option =
        match declared_aperture_option(&capabilities.aperture_options, reference.aperture) {
            Ok(option) => option,
            Err(error) => return HttpResponse::BadRequest().body(error.to_string()),
        };
    let mut candidates = match build_camera_candidates(
        &capabilities.shutter_speed_options,
        &capabilities.iso_options,
        &json.focus_diopters,
        json.readout_ms,
        json.settle_ms,
    ) {
        Ok(report) => report.candidates,
        Err(error) => return HttpResponse::BadRequest().body(error.to_string()),
    };
    candidates.retain(|candidate| sensor_profile.model_for_iso(candidate.iso).is_some());
    if candidates.is_empty() {
        return HttpResponse::BadRequest()
            .body("Camera exposes no candidate ISO with exact sensor calibration");
    }
    if candidates.len() > MAX_API_CANDIDATES {
        return HttpResponse::BadRequest().body(format!(
            "Exact-calibration adaptive grid has {} entries; limit is {}. Reduce focus planes or camera shutter options",
            candidates.len(),
            MAX_API_CANDIDATES
        ));
    }
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
    match sessions.make_room() {
        Ok(true) => {}
        Ok(false) => {
            return HttpResponse::TooManyRequests()
                .body("Too many active adaptive capture sessions")
        }
        Err(error) => {
            return HttpResponse::InternalServerError()
                .body(format!("Adaptive session retention failed: {error}"))
        }
    }
    if let Err(error) = sessions.insert_new(
        session_id,
        ServerAdaptiveSession {
            camera_id: camera_id.clone(),
            project_id: Some(json.project_id.clone()),
            aperture_option: Some(aperture_option),
            core,
            roi,
            observation_config,
            generation: 0,
            capture_active: Arc::new(AtomicBool::new(false)),
        },
    ) {
        return HttpResponse::InternalServerError()
            .body(format!("Adaptive session checkpoint failed: {error}"));
    }
    HttpResponse::Created().json(AdaptiveCaptureResponse {
        session_id,
        camera_id,
        project_id: Some(json.project_id.clone()),
        generation: 0,
        status,
    })
}

#[utoipa::path(
    post,
    path = "/api/cameras/adaptive/{session_id}/capture-next",
    tag = "hardware",
    params(("session_id" = String, Path, description = "Adaptive session UUID")),
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Recommended RAW captured, verified, and assimilated", body = serde_json::Value),
        (status = 400, description = "Focus confirmation, camera options, RAW, or telemetry rejected"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Feature not licensed"),
        (status = 404, description = "Session or camera not found"),
        (status = 409, description = "Capture already active or session advanced"),
        (status = 500, description = "Camera, storage, encryption, or checkpoint failure")
    )
)]
#[post("/api/cameras/adaptive/{session_id}/capture-next")]
pub async fn capture_next_adaptive(
    req: HttpRequest,
    path: web::Path<Uuid>,
    json: web::Json<CaptureNextAdaptiveRequest>,
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
    if !json.confirmed_focus_diopters.is_finite()
        || json.confirmed_focus_diopters < 0.0
        || !json.motion_pixels_per_second.is_finite()
        || json.motion_pixels_per_second < 0.0
        || !json.thermal_load.is_finite()
        || json.thermal_load < 0.0
    {
        return HttpResponse::BadRequest().body("Adaptive capture confirmation is invalid");
    }

    let session = {
        let sessions = state.adaptive_capture.lock().await;
        let Some(session) = sessions.sessions.get(&session_id).cloned() else {
            return HttpResponse::NotFound().body("Adaptive capture session not found");
        };
        let Some(selected) = session.core.next_candidate() else {
            return HttpResponse::BadRequest().body("Adaptive capture session has no next action");
        };
        let focus_tolerance = (selected.focus_diopters.abs() * 0.01).max(0.01);
        if (json.confirmed_focus_diopters - selected.focus_diopters).abs() > focus_tolerance {
            return HttpResponse::BadRequest().body(format!(
                "Confirmed focus {:.6} D does not match recommended {:.6} D",
                json.confirmed_focus_diopters, selected.focus_diopters
            ));
        }
        if session.project_id.is_none() || session.aperture_option.is_none() {
            return HttpResponse::BadRequest().body(
                "Legacy adaptive session cannot automate capture; create a project-bound session",
            );
        }
        session
    };
    let capture_lease = match AdaptiveCaptureLease::acquire(&session.capture_active) {
        Ok(lease) => lease,
        Err(message) => return HttpResponse::Conflict().body(message),
    };

    let result = execute_capture_next(session_id, &session, &json, &state, capture_lease).await;
    let mut sessions = state.adaptive_capture.lock().await;
    let completed = match result {
        Ok(completed) => completed,
        Err((status, message)) => return HttpResponse::build(status).body(message),
    };
    let (report, advanced) = match sessions.assimilate(
        session_id,
        session.generation,
        completed.observation,
        CaptureRuntimeTelemetry {
            capture_elapsed_ms: completed.elapsed_ms,
            motion_pixels_per_second: json.motion_pixels_per_second,
            thermal_load: json.thermal_load,
        },
    ) {
        Ok(transition) => transition,
        Err(error) if error.to_string().contains("advanced concurrently") => {
            return HttpResponse::Conflict().body(error.to_string())
        }
        Err(error) => return persistence_error_response(&error),
    };
    HttpResponse::Ok().json(CaptureNextAdaptiveResponse {
        session_id,
        generation: advanced.generation,
        capture_path: completed.project_path,
        selected: completed.selected,
        camera_options: completed.camera_options,
        measured_capture_elapsed_ms: completed.elapsed_ms,
        report,
        status: advanced.core.status(),
    })
}

struct CompletedAdaptiveCapture {
    project_path: String,
    selected: CaptureCandidate,
    camera_options: CameraOptionSelection,
    elapsed_ms: f32,
    observation: trueshot_core::capture::RawCaptureObservation,
    _capture_lease: AdaptiveCaptureLease,
}

struct AdaptiveCaptureLease {
    active: Arc<AtomicBool>,
}

impl AdaptiveCaptureLease {
    fn acquire(active: &Arc<AtomicBool>) -> Result<Self, &'static str> {
        active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "An adaptive camera transaction is already active for this session")?;
        Ok(Self {
            active: Arc::clone(active),
        })
    }
}

impl Drop for AdaptiveCaptureLease {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

async fn execute_capture_next(
    _session_id: Uuid,
    session: &ServerAdaptiveSession,
    _request: &CaptureNextAdaptiveRequest,
    state: &web::Data<AppState>,
    capture_lease: AdaptiveCaptureLease,
) -> Result<CompletedAdaptiveCapture, (actix_web::http::StatusCode, String)> {
    use actix_web::http::StatusCode;

    let selected = session
        .core
        .next_candidate()
        .ok_or((StatusCode::BAD_REQUEST, "No next action".to_string()))?;
    let project_id = session
        .project_id
        .as_deref()
        .ok_or((
            StatusCode::BAD_REQUEST,
            "Adaptive session is not project-bound".to_string(),
        ))?
        .to_string();
    let raw_dir = project_raw_directory(&state.config.paths.projects_dir, &project_id)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    let (camera, camera_options, aperture_option) = {
        let manager = state.camera_manager.lock().await;
        let camera = manager.get_camera_by_id(&session.camera_id).ok_or((
            StatusCode::NOT_FOUND,
            "Adaptive session camera is not connected".to_string(),
        ))?;
        let profile = manager.registry.get_profile(&session.camera_id).ok_or((
            StatusCode::BAD_REQUEST,
            "Camera has no registered capabilities".to_string(),
        ))?;
        let options = resolve_camera_candidate_options(
            &profile.capabilities.shutter_speed_options,
            &profile.capabilities.iso_options,
            selected,
        )
        .map_err(|error| (StatusCode::CONFLICT, error.to_string()))?;
        (
            camera,
            options,
            session.aperture_option.clone().ok_or((
                StatusCode::BAD_REQUEST,
                "Adaptive session has no verified aperture option".to_string(),
            ))?,
        )
    };
    let config = CameraConfig {
        iso: Some(camera_options.iso.clone()),
        shutter_speed: Some(camera_options.shutter_speed.clone()),
        aperture: Some(aperture_option),
        wb: None,
        capture_target: None,
        resolution: None,
        fps: None,
    };
    let started = Instant::now();
    let (capture_result, capture_lease) =
        tokio::task::spawn_blocking(move || (camera.capture_to(&config, &raw_dir), capture_lease))
            .await
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Adaptive camera task failed: {error}"),
                )
            })?;
    let capture_path = capture_result.map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Adaptive camera capture failed: {error}"),
        )
    })?;
    let elapsed_ms = started.elapsed().as_secs_f32() * 1_000.0;
    let sensor_profile = session.core.sensor_profile().clone();
    let anchor = session.core.status().posterior.radiance_anchor_exposure;
    let roi = session.roi.clone();
    let observation_config = session.observation_config;
    let observation_path = capture_path.clone();
    let observation = tokio::task::spawn_blocking(move || {
        observe_nef_roi(
            &observation_path,
            roi,
            &sensor_profile,
            anchor,
            observation_config,
        )
    })
    .await
    .map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Adaptive observation task failed: {error}"),
        )
    })?
    .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;

    let mut stored_path = capture_path.clone();
    if policy_for_project(
        &state.config.paths.projects_dir,
        &project_id,
        &state.config.privacy,
    )
    .is_some_and(|policy| policy.scopes.iter().any(|scope| scope == "raw"))
    {
        let master = require_master_key(&state.config.privacy, &state.config.paths.projects_dir)
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Adaptive RAW encryption key unavailable: {error}"),
                )
            })?;
        let key = ProjectKeyStore::new(&state.config.paths.projects_dir, master)
            .load_or_create(&project_id)
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Adaptive project key unavailable: {error}"),
                )
            })?;
        stored_path = encrypt_file_in_place(&capture_path, &key, 0)
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Adaptive RAW encryption failed: {error}"),
                )
            })?
            .ok_or((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Adaptive RAW was not encrypted under the active project policy".to_string(),
            ))?;
    }
    let project_root = std::fs::canonicalize(state.config.paths.projects_dir.join(&project_id))
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Adaptive project path disappeared: {error}"),
            )
        })?;
    let project_path = stored_path
        .strip_prefix(&project_root)
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Adaptive capture escaped the project".to_string(),
            )
        })?
        .to_string_lossy()
        .replace('\\', "/");
    Ok(CompletedAdaptiveCapture {
        project_path,
        selected,
        camera_options,
        elapsed_ms,
        observation,
        _capture_lease: capture_lease,
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
        if sessions
            .sessions
            .get(&session_id)
            .is_some_and(|session| session.capture_active.load(Ordering::Acquire))
        {
            return HttpResponse::Conflict()
                .body("An adaptive camera transaction is already active for this session");
        }
        let Some(session) = sessions.sessions.get(&session_id) else {
            return HttpResponse::NotFound().body("Adaptive capture session not found");
        };
        (
            session.core.sensor_profile().clone(),
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
    if !sessions.sessions.contains_key(&session_id) {
        return HttpResponse::NotFound().body("Adaptive capture session not found");
    }
    if sessions
        .sessions
        .get(&session_id)
        .is_some_and(|session| session.capture_active.load(Ordering::Acquire))
    {
        return HttpResponse::Conflict()
            .body("An adaptive camera transaction is already active for this session");
    }
    let (report, session) =
        match sessions.assimilate(session_id, expected_generation, observation, json.telemetry) {
            Ok(transition) => transition,
            Err(error) if error.to_string().contains("advanced concurrently") => {
                return HttpResponse::Conflict().body(error.to_string())
            }
            Err(error) => return persistence_error_response(&error),
        };
    HttpResponse::Ok().json(AdaptiveAssimilationResponse {
        session_id,
        generation: session.generation,
        report,
        status: session.core.status(),
    })
}

fn persistence_error_response(error: &anyhow::Error) -> HttpResponse {
    if error
        .chain()
        .any(|cause| cause.downcast_ref::<std::io::Error>().is_some())
    {
        HttpResponse::InternalServerError()
            .body(format!("Adaptive session checkpoint failed: {error}"))
    } else {
        HttpResponse::BadRequest().body(error.to_string())
    }
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
        project_id: session.project_id.clone(),
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
    if !sessions.sessions.contains_key(&session_id) {
        return HttpResponse::NotFound().body("Adaptive capture session not found");
    }
    if sessions
        .sessions
        .get(&session_id)
        .is_some_and(|session| session.capture_active.load(Ordering::Acquire))
    {
        return HttpResponse::Conflict()
            .body("Cannot terminate while an adaptive camera transaction is active");
    }
    let session = match sessions.terminate(session_id, json.reason) {
        Ok(session) => session,
        Err(error) => return persistence_error_response(&error),
    };
    HttpResponse::Ok().json(AdaptiveCaptureResponse {
        session_id,
        camera_id: session.camera_id.clone(),
        project_id: session.project_id.clone(),
        generation: session.generation,
        status: session.core.status(),
    })
}

fn project_raw_directory(projects_root: &Path, project_id: &str) -> anyhow::Result<PathBuf> {
    if ensure_project_id(project_id).is_err() {
        anyhow::bail!("Invalid adaptive project id");
    }
    let root = std::fs::canonicalize(projects_root)?;
    let project = std::fs::canonicalize(root.join(project_id))?;
    if !project.starts_with(&root) {
        anyhow::bail!("Adaptive project escaped the projects directory");
    }
    let raw = project.join("raw");
    std::fs::create_dir_all(&raw)?;
    let metadata = std::fs::symlink_metadata(&raw)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!("Adaptive project RAW destination must be a real directory");
    }
    let raw = std::fs::canonicalize(raw)?;
    if !raw.starts_with(&project) {
        anyhow::bail!("Adaptive project RAW destination escaped the project");
    }
    Ok(raw)
}

fn project_asset_file(
    projects_root: &Path,
    project_id: &str,
    relative: &str,
    allowed_scopes: &[&str],
) -> anyhow::Result<PathBuf> {
    if ensure_project_id(project_id).is_err() {
        anyhow::bail!("Invalid adaptive project id");
    }
    let relative = Path::new(relative);
    if relative.is_absolute() || relative.as_os_str().is_empty() {
        anyhow::bail!("Adaptive project asset path must be relative");
    }
    if relative.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        anyhow::bail!("Adaptive project asset path contains traversal");
    }
    let scope = relative
        .components()
        .next()
        .and_then(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .context("Adaptive project asset has no scope")?;
    if !allowed_scopes.contains(&scope) {
        anyhow::bail!(
            "Adaptive project asset must be under one of: {}",
            allowed_scopes.join(", ")
        );
    }
    let root = std::fs::canonicalize(projects_root)?;
    let project = std::fs::canonicalize(root.join(project_id))?;
    let scope_root = std::fs::canonicalize(project.join(scope))?;
    let requested = project.join(relative);
    let metadata = std::fs::symlink_metadata(&requested)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        anyhow::bail!("Adaptive project asset must be a regular non-symlink file");
    }
    let requested = std::fs::canonicalize(requested)?;
    if !project.starts_with(&root)
        || !scope_root.starts_with(&project)
        || !requested.starts_with(&scope_root)
    {
        anyhow::bail!("Adaptive project asset escaped its authorized scope");
    }
    Ok(requested)
}

fn declared_aperture_option(
    options: &[String],
    measured_aperture: Option<f32>,
) -> anyhow::Result<String> {
    let measured = measured_aperture.context("Adaptive reference RAW has no physical aperture")?;
    if !measured.is_finite() || measured <= 0.0 {
        anyhow::bail!("Adaptive reference aperture is invalid");
    }
    options
        .iter()
        .filter_map(|option| {
            let normalized = option
                .trim()
                .trim_start_matches(['f', 'F'])
                .trim_start_matches('/')
                .trim();
            let parsed = normalized.parse::<f32>().ok()?;
            let relative_error = (parsed - measured).abs() / measured;
            (parsed.is_finite() && parsed > 0.0 && relative_error <= 0.001).then_some(option)
        })
        .min()
        .cloned()
        .context("Reference aperture is not an exact camera-declared option")
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
    use trueshot_core::capture::{
        CaptureCandidate, FocusResponseObservation, RadianceObservation, RawCaptureObservation,
    };
    use trueshot_core::sensor_noise::{
        IsoNoiseModel, SensorNoiseModel, SENSOR_NOISE_PROFILE_SCHEMA,
    };

    fn profile() -> SensorNoiseProfile {
        SensorNoiseProfile {
            schema: SENSOR_NOISE_PROFILE_SCHEMA.to_string(),
            camera_make: "Nikon".to_string(),
            camera_model: "Z9".to_string(),
            bits_per_sample: 14,
            calibration_id: "sha256:server-adaptive-test".to_string(),
            iso_models: vec![IsoNoiseModel {
                iso: 100,
                model: SensorNoiseModel {
                    read_noise_dn: [2.0; 4],
                    electrons_per_dn: [0.8; 4],
                    black_drift_dn: [0.25; 4],
                    saturation_margin_dn: 16.0,
                    calibrated: true,
                },
            }],
        }
    }

    fn observation(shutter: f32, focus: f32, variance: f32) -> RawCaptureObservation {
        RawCaptureObservation {
            camera_make: "Nikon".to_string(),
            camera_model: "Z9".to_string(),
            bits_per_sample: 14,
            sensor_calibration_id: profile().calibration_id,
            sensor_range_dn: 14_303.0,
            iso: 100,
            exposure_seconds: shutter,
            sensor_exposure: shutter / 64.0,
            focus_diopters: Some(focus),
            focal_length_mm: Some(105.0),
            aperture: Some(8.0),
            roi: [0, 0, 64, 64],
            radiance_anchor_exposure: 0.01 / 64.0,
            radiance: vec![RadianceObservation {
                probe_id: 0,
                cfa_site: 0,
                weight: 1.0,
                mean: Some(0.2),
                variance: Some(variance),
                lower_bound: None,
                valid_samples: 64,
                censored_samples: 0,
            }],
            focus: vec![FocusResponseObservation {
                probe_id: 0,
                weight: 1.0,
                score: 0.5,
                variance: 0.01,
                sample_count: 64,
            }],
        }
    }

    fn active_server_session() -> ServerAdaptiveSession {
        let candidates = [1.0, 2.0]
            .into_iter()
            .map(|focus_diopters| CaptureCandidate {
                shutter_seconds: 0.01,
                iso: 100,
                focus_diopters,
                readout_ms: 20.0,
                settle_ms: 5.0,
            })
            .collect();
        let core = MeasuredAdaptiveSession::start(
            observation(0.01, 1.0, 0.05),
            profile(),
            candidates,
            AdaptivePlannerConfig {
                target_radiance_variance: 1e-6,
                target_focus_variance_diopters2: 1e-6,
                minimum_hdr_information_nats: 0.0,
                minimum_focus_information_nats: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        ServerAdaptiveSession {
            camera_id: "camera-z9".to_string(),
            project_id: None,
            aperture_option: None,
            core,
            roi: Roi {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            },
            observation_config: RawObservationConfig::default(),
            generation: 0,
            capture_active: Arc::new(AtomicBool::new(false)),
        }
    }

    fn telemetry() -> CaptureRuntimeTelemetry {
        CaptureRuntimeTelemetry {
            capture_elapsed_ms: 31.0,
            motion_pixels_per_second: 0.1,
            thermal_load: 0.02,
        }
    }

    fn temporary_projects() -> PathBuf {
        std::env::temp_dir().join(format!("trueshot-adaptive-store-{}", Uuid::new_v4()))
    }

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

    #[test]
    fn project_asset_gate_is_scope_bound_and_rejects_symlinks() {
        let base = temporary_projects();
        let project = base.join("product");
        fs::create_dir_all(project.join("raw")).unwrap();
        fs::create_dir_all(project.join("processed")).unwrap();
        let raw = project.join("raw/reference.NEF");
        let profile = project.join("processed/noise.json");
        fs::write(&raw, b"raw").unwrap();
        fs::write(&profile, b"{}").unwrap();

        assert_eq!(
            project_asset_file(&base, "product", "raw/reference.NEF", &["raw"]).unwrap(),
            fs::canonicalize(&raw).unwrap()
        );
        assert!(project_asset_file(&base, "product", "processed/noise.json", &["raw"]).is_err());
        assert!(project_asset_file(&base, "product", "../escape", &["raw"]).is_err());

        #[cfg(unix)]
        {
            let link = project.join("raw/profile-link.json");
            std::os::unix::fs::symlink(&profile, &link).unwrap();
            assert!(
                project_asset_file(&base, "product", "raw/profile-link.json", &["raw"]).is_err()
            );
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn aperture_must_round_trip_to_a_declared_camera_option() {
        let options = vec!["Auto".to_string(), "f/8".to_string(), "11".to_string()];
        assert_eq!(
            declared_aperture_option(&options, Some(8.0)).unwrap(),
            "f/8"
        );
        assert!(declared_aperture_option(&options, Some(9.5)).is_err());
        assert!(declared_aperture_option(&options, None).is_err());
    }

    #[test]
    fn capture_lease_is_exclusive_and_releases_on_drop() {
        let active = Arc::new(AtomicBool::new(false));
        let first = AdaptiveCaptureLease::acquire(&active).unwrap();
        assert!(AdaptiveCaptureLease::acquire(&active).is_err());
        drop(first);
        assert!(AdaptiveCaptureLease::acquire(&active).is_ok());
    }

    #[test]
    fn capture_worker_owns_lease_until_hardware_action_exits() {
        let active = Arc::new(AtomicBool::new(false));
        let lease = AdaptiveCaptureLease::acquire(&active).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            finish_rx.recv().unwrap();
            drop(lease);
        });
        started_rx.recv().unwrap();
        assert!(active.load(Ordering::Acquire));
        assert!(AdaptiveCaptureLease::acquire(&active).is_err());
        finish_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(!active.load(Ordering::Acquire));
    }

    #[cfg(unix)]
    #[test]
    fn adaptive_storage_rejects_symlink_redirection() {
        let projects = temporary_projects();
        let outside = temporary_projects();
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, projects.join("_adaptive_sessions")).unwrap();
        assert!(AdaptiveCaptureSessions::load(&projects).is_err());
        fs::remove_dir_all(projects).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn durable_session_round_trip_restores_exact_active_state() {
        let projects = temporary_projects();
        let mut sessions = AdaptiveCaptureSessions::load(&projects).unwrap();
        let session_id = Uuid::new_v4();
        sessions
            .insert_new(session_id, active_server_session())
            .unwrap();
        let expected = sessions.sessions[&session_id].core.snapshot();
        drop(sessions);

        let restored = AdaptiveCaptureSessions::load(&projects).unwrap();
        let session = &restored.sessions[&session_id];
        assert_eq!(session.generation, 0);
        assert_eq!(session.core.snapshot(), expected);
        assert_eq!(session.camera_id, "camera-z9");
        fs::remove_dir_all(projects).unwrap();
    }

    #[test]
    fn corrupt_newest_generation_recovers_and_can_advance() {
        let projects = temporary_projects();
        let mut sessions = AdaptiveCaptureSessions::load(&projects).unwrap();
        let session_id = Uuid::new_v4();
        sessions
            .insert_new(session_id, active_server_session())
            .unwrap();
        let selected = sessions.sessions[&session_id]
            .core
            .next_candidate()
            .unwrap();
        let captured = observation(selected.shutter_seconds, selected.focus_diopters, 0.01);
        sessions
            .assimilate(session_id, 0, captured.clone(), telemetry())
            .unwrap();
        let corrupt = sessions.storage_dir.join(snapshot_filename(session_id, 1));
        fs::write(&corrupt, b"{truncated").unwrap();
        drop(sessions);

        let mut restored = AdaptiveCaptureSessions::load(&projects).unwrap();
        assert_eq!(restored.sessions[&session_id].generation, 0);
        assert!(!corrupt.exists());
        let (_, advanced) = restored
            .assimilate(session_id, 0, captured, telemetry())
            .unwrap();
        assert_eq!(advanced.generation, 1);
        fs::remove_dir_all(projects).unwrap();
    }

    #[test]
    fn digest_tampering_without_fallback_is_rejected() {
        let projects = temporary_projects();
        let mut sessions = AdaptiveCaptureSessions::load(&projects).unwrap();
        let session_id = Uuid::new_v4();
        sessions
            .insert_new(session_id, active_server_session())
            .unwrap();
        let path = sessions.storage_dir.join(snapshot_filename(session_id, 0));
        let mut envelope: DurableSessionEnvelope =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        envelope.payload.camera_id.push_str("-tampered");
        fs::write(&path, serde_json::to_vec_pretty(&envelope).unwrap()).unwrap();
        drop(sessions);

        assert!(AdaptiveCaptureSessions::load(&projects).is_err());
        fs::remove_dir_all(projects).unwrap();
    }

    #[test]
    fn interrupted_partial_is_cleaned_without_losing_checkpoint() {
        let projects = temporary_projects();
        let mut sessions = AdaptiveCaptureSessions::load(&projects).unwrap();
        let session_id = Uuid::new_v4();
        sessions
            .insert_new(session_id, active_server_session())
            .unwrap();
        let partial = sessions.storage_dir.join(".interrupted.part");
        fs::write(&partial, b"partial").unwrap();
        drop(sessions);

        let restored = AdaptiveCaptureSessions::load(&projects).unwrap();
        assert!(restored.sessions.contains_key(&session_id));
        assert!(!partial.exists());
        fs::remove_dir_all(projects).unwrap();
    }

    #[test]
    fn terminated_session_is_durable_and_restores_complete() {
        let projects = temporary_projects();
        let mut sessions = AdaptiveCaptureSessions::load(&projects).unwrap();
        let session_id = Uuid::new_v4();
        sessions
            .insert_new(session_id, active_server_session())
            .unwrap();
        let terminated = sessions
            .terminate(session_id, AdaptiveCaptureTermination::OperatorStopped)
            .unwrap();
        assert_eq!(terminated.generation, 1);
        assert!(terminated.core.is_complete());
        drop(sessions);

        let restored = AdaptiveCaptureSessions::load(&projects).unwrap();
        assert_eq!(restored.sessions[&session_id].generation, 1);
        assert!(restored.sessions[&session_id].core.is_complete());
        fs::remove_dir_all(projects).unwrap();
    }

    #[test]
    fn failed_publication_does_not_advance_live_state() {
        let projects = temporary_projects();
        let mut sessions = AdaptiveCaptureSessions::load(&projects).unwrap();
        let session_id = Uuid::new_v4();
        sessions
            .insert_new(session_id, active_server_session())
            .unwrap();
        let before = sessions.sessions[&session_id].core.status();
        let selected = sessions.sessions[&session_id]
            .core
            .next_candidate()
            .unwrap();
        fs::write(
            sessions.storage_dir.join(snapshot_filename(session_id, 1)),
            b"occupied",
        )
        .unwrap();

        assert!(sessions
            .assimilate(
                session_id,
                0,
                observation(selected.shutter_seconds, selected.focus_diopters, 0.01,),
                telemetry(),
            )
            .is_err());
        assert_eq!(sessions.sessions[&session_id].generation, 0);
        assert_eq!(sessions.sessions[&session_id].core.status(), before);
        fs::remove_dir_all(projects).unwrap();
    }
}
