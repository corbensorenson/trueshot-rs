use crate::at_rest::{
    clear_project_encrypted, decrypt_file_in_place, decrypt_file_to_bytes, decrypt_project_scopes,
    encrypt_file_in_place, encrypt_project_scopes, mark_project_encrypted, policy_for_project,
    require_master_key, write_encrypted_bytes_atomic, ProjectKeyStore,
};
use crate::audit::AuditEvent;
use crate::auth::require_admin;
use crate::fs_safety::{
    project_size_bytes, resolve_project_child, resolve_project_child_file, resolve_project_dir,
    resolve_project_file,
};
use crate::fusion_revision::{preflight as preflight_fusion_revision, FusionRevisionJobPayload};
use crate::licensing::require_license_feature;
use crate::state::AppState;
use actix_files::NamedFile;
use actix_web::{delete, get, post, put, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use trueshot_core::fusion_edit::{
    FusionEditDocument, FusionEditOperation, FUSION_EDIT_SCHEMA, MAX_FUSION_EDIT_BYTES,
};
use trueshot_core::fusion_replay::FusionReplayCapsule;
use trueshot_core::licensing::Feature;
use utoipa::ToSchema;
use uuid::Uuid;
use walkdir::WalkDir;

const MAX_FUSION_REPORT_BYTES: usize = 2 * 1024 * 1024;
const MAX_FUSION_ARTIFACT_BYTES: usize = 128 * 1024 * 1024;
const MAX_FUSION_REPORTS: usize = 128;

// Struct for create request
#[derive(serde::Deserialize, ToSchema)]
pub struct CreateProjectRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ProjectAssetsQuery {
    pub scope: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectAsset {
    pub path: String,
    pub bytes: u64,
    pub modified_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FusionReportInventory {
    pub reports: Vec<FusionReportSummary>,
    pub rejected_reports: usize,
    pub truncated: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FusionReportSummary {
    pub report_path: String,
    pub report_sha256: String,
    pub label: String,
    pub modified_at: Option<String>,
    pub schema: String,
    pub width: u32,
    pub height: u32,
    pub integrity_complete: bool,
    pub warnings: Vec<String>,
    pub artifacts: BTreeMap<String, FusionArtifactRef>,
    pub flags: BTreeMap<String, FusionFlagSummary>,
    pub frequency_flags: BTreeMap<String, FusionFlagSummary>,
    pub boundary_trimap_legend: BTreeMap<String, u8>,
    pub sensor_correction_legend: BTreeMap<String, u8>,
    pub metrics: BTreeMap<String, u64>,
    pub policy: FusionPolicySummary,
    pub calibration: FusionCalibrationSummary,
    pub demosaic: FusionDemosaicSummary,
    pub performance: FusionPerformanceSummary,
    pub capture_group_id: Option<String>,
    pub revision_group_id: Option<String>,
    pub frame_count: Option<u16>,
    pub crop_origin_x: Option<u32>,
    pub crop_origin_y: Option<u32>,
    pub fusion_edit_digest: Option<String>,
    pub editable_base: bool,
    pub revision_executable: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ExecuteFusionRevisionRequest {
    pub request_id: Option<Uuid>,
    pub report_path: String,
    pub report_sha256: String,
    pub edit_path: String,
    pub edit_digest: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FusionArtifactRef {
    pub path: String,
    pub present: bool,
    pub bytes: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FusionFlagSummary {
    pub bit: u8,
    pub pixels: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FusionPolicySummary {
    pub archival: String,
    pub focus: String,
    pub boundary: String,
    pub glare: String,
    pub frequency: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FusionCalibrationSummary {
    pub noise_model_calibrated: bool,
    pub lens_psf_calibrated: bool,
    pub sensor_correction_id: Option<String>,
    pub lens_psf_calibration_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FusionDemosaicSummary {
    pub backend: Option<String>,
    pub adapter: Option<String>,
    pub fallback: bool,
    pub generative_reconstruction: bool,
}

#[derive(Debug, Serialize, ToSchema, Default)]
pub struct FusionPerformanceSummary {
    pub decode_seconds: Option<f64>,
    pub fusion_seconds: Option<f64>,
    pub demosaic_and_postprocess_seconds: Option<f64>,
    pub processing_before_export_seconds: Option<f64>,
    pub decoded_megapixels: Option<f64>,
    pub admitted_peak_memory_bytes: Option<u64>,
    pub major_page_faults: Option<u64>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct FusionReportQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct CreateFusionEditRequest {
    pub report_path: String,
    pub operations: Vec<FusionEditOperation>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Default)]
pub struct ProjectLicense {
    pub title: Option<String>,
    pub url: Option<String>,
    pub data_ownership: Option<String>,
    pub export_rights: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ImuDiagnostics {
    pub status: String,
    pub samples: usize,
    pub duration_seconds: f64,
    pub sample_rate_hz: f64,
    pub accel_mean: f64,
    pub accel_rms: f64,
    pub accel_peak: f64,
    pub gyro_mean: f64,
    pub gyro_rms: f64,
    pub gyro_peak: f64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ImuSample {
    pub timestamp: f64,
    pub accel: [f64; 3],
    pub gyro: [f64; 3],
}

#[utoipa::path(
    post,
    path = "/api/projects",
    tag = "project",
    request_body = CreateProjectRequest,
    responses(
        (status = 200, description = "Project created", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 409, description = "Conflict")
    )
)]
#[post("/api/projects")]
pub async fn create_project(
    req: HttpRequest,
    json: web::Json<CreateProjectRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let projects_dir = &state.config.paths.projects_dir;
    let project_path = match resolve_project_dir(projects_dir, &json.name) {
        Ok(path) => path,
        Err(resp) => return resp,
    };

    if project_path.exists() {
        return HttpResponse::Conflict().body("Project already exists");
    }

    if let Err(e) = fs::create_dir_all(&project_path).await {
        return HttpResponse::InternalServerError().body(e.to_string());
    }

    // Create raw/processed dirs
    let _ = fs::create_dir_all(project_path.join("raw")).await;
    let _ = fs::create_dir_all(project_path.join("processed")).await;

    // Persist project metadata
    let metadata_path = project_path.join("project.json");
    let license = license_from_config(&state.config);
    let metadata = serde_json::json!({
        "name": json.name,
        "description": json.description,
        "created_at": Utc::now().to_rfc3339(),
        "license": license
    });
    if let Ok(payload) = serde_json::to_vec_pretty(&metadata) {
        if let Err(e) = fs::write(&metadata_path, payload).await {
            tracing::warn!(
                "Failed to write project metadata {:?}: {}",
                metadata_path,
                e
            );
        }
    }

    if let Some(policy) = policy_for_project(
        &state.config.paths.projects_dir,
        &json.name,
        &state.config.privacy,
    ) {
        if let Err(err) =
            mark_project_encrypted(&state.config.paths.projects_dir, &json.name, &policy.scopes)
        {
            tracing::warn!("Failed to mark project encryption: {}", err);
        }
    }

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "project.create",
            json.name.clone(),
            "success",
            audit_actor(&req).2,
            serde_json::json!({ "description": json.description.clone() }),
        ),
    );

    HttpResponse::Ok().json(serde_json::json!({
        "status": "created",
        "name": json.name,
        "description": json.description,
        "path": project_path
    }))
}

#[utoipa::path(
    delete,
    path = "/api/projects/{id}/raw",
    tag = "project",
    params(("id" = String, Path, description = "Project id")),
    responses(
        (status = 200, description = "Raw data purged", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[delete("/api/projects/{id}/raw")]
pub async fn purge_project_raw(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    let project_path = match resolve_project_child(&state.config.paths.projects_dir, &id, "raw") {
        Ok(path) => path,
        Err(resp) => return resp,
    };

    if project_path.exists() {
        if let Err(e) = fs::remove_dir_all(project_path).await {
            return HttpResponse::InternalServerError().body(e.to_string());
        }
    }
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "project.raw.purge",
            id,
            "success",
            audit_actor(&req).2,
            serde_json::json!({}),
        ),
    );
    HttpResponse::Ok().json(serde_json::json!({"status": "purged"}))
}

async fn get_projects_impl(req: HttpRequest, state: web::Data<AppState>) -> HttpResponse {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let projects_dir = &state.config.paths.projects_dir;

    // std::fs::read_dir is blocking, but iterating directories is complex in async without a stream.
    // For now, we wrap it in spawn_blocking to be safe, or just accept it's fast on FS cache.
    // Using tokio::task::spawn_blocking is the "correct" way for blocking std calls.

    let dir = projects_dir.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut list = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_dir() {
                        let created = meta
                            .created()
                            .or_else(|_| meta.modified())
                            .ok()
                            .map(|time| DateTime::<Utc>::from(time).to_rfc3339());
                        list.push(serde_json::json!({
                            "name": entry.file_name().to_string_lossy(),
                            "created": created
                        }));
                    }
                }
            }
        }
        list
    })
    .await;

    match result {
        Ok(list) => HttpResponse::Ok().json(list),
        Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
    }
}

#[utoipa::path(
    get,
    path = "/api/projects",
    tag = "project",
    responses(
        (status = 200, description = "Project list", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/projects")]
pub async fn get_projects(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    get_projects_impl(req, state).await
}

#[utoipa::path(
    get,
    path = "/api/models",
    tag = "project",
    responses(
        (status = 200, description = "Project list (legacy)", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/models")]
pub async fn get_projects_legacy(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    get_projects_impl(req, state).await
}

#[utoipa::path(
    post,
    path = "/api/projects/{id}/open",
    tag = "project",
    params(("id" = String, Path, description = "Project id")),
    responses(
        (status = 200, description = "Open project result", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden")
    )
)]
#[post("/api/projects/{id}/open")]
pub async fn open_project_fs(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let env = std::env::var("TRUESHOT_ENV").unwrap_or_else(|_| "development".to_string());
    if env == "production" {
        return HttpResponse::Forbidden().body("Filesystem open disabled in production");
    }
    let id = path.into_inner();
    let project_path = match resolve_project_dir(&state.config.paths.projects_dir, &id) {
        Ok(path) => path,
        Err(resp) => return resp,
    };

    if !project_path.exists() {
        return HttpResponse::NotFound().body("Project not found");
    }

    // MacOS "open", Windows "explorer", Linux "xdg-open"
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";

    if let Err(e) = std::process::Command::new(cmd).arg(&project_path).spawn() {
        return HttpResponse::InternalServerError()
            .body(format!("Failed to open filesystem: {}", e));
    }

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "project.open_fs",
            id,
            "success",
            audit_actor(&req).2,
            serde_json::json!({ "path": project_path.to_string_lossy() }),
        ),
    );

    HttpResponse::Ok().json(serde_json::json!({"status": "opened"}))
}

use actix_multipart::Multipart;
use futures::{StreamExt, TryStreamExt};

#[utoipa::path(
    post,
    path = "/api/projects/{id}/import",
    tag = "project",
    params(("id" = String, Path, description = "Project id")),
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Import result", body = serde_json::Value),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/projects/{id}/import")]
pub async fn import_model(
    req: HttpRequest,
    path: web::Path<String>,
    mut payload: Multipart,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    let project_path = match resolve_project_dir(&state.config.paths.projects_dir, &id) {
        Ok(path) => path,
        Err(resp) => return resp,
    };

    if !project_path.exists() {
        return HttpResponse::NotFound().body("Project not found");
    }

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

    let existing_size = match project_size_bytes(&project_path) {
        Ok(size) => size,
        Err(resp) => return resp,
    };
    let mut total_written: u64 = 0;
    let mut imported_files: Vec<serde_json::Value> = Vec::new();

    // Iterate over multipart stream
    while let Ok(Some(mut field)) = payload.try_next().await {
        let content_disposition = field.content_disposition();
        let filename = content_disposition
            .and_then(|cd| cd.get_filename())
            .map(|f| f.to_string());

        if let Some(fname) = filename {
            if !is_allowed_model_extension(&fname) {
                return HttpResponse::BadRequest().body("Unsupported file type");
            }
            let filepath = match resolve_project_file(&state.config.paths.projects_dir, &id, &fname)
            {
                Ok(path) => path,
                Err(resp) => return resp,
            };
            let mut f = match tokio::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&filepath)
                .await
            {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    return HttpResponse::Conflict().body("File already exists")
                }
                Err(e) => return HttpResponse::InternalServerError().body(e.to_string()),
            };

            let mut hasher = Sha256::new();
            let mut sniff_buf: Vec<u8> = Vec::new();
            let mut sniffed_mime: Option<String> = None;
            let mut file_written: u64 = 0;

            // Field streaming
            while let Some(chunk) = field.next().await {
                match chunk {
                    Ok(data) => {
                        total_written = total_written.saturating_add(data.len() as u64);
                        file_written = file_written.saturating_add(data.len() as u64);
                        if total_written > max_upload_bytes {
                            let _ = tokio::fs::remove_file(&filepath).await;
                            return HttpResponse::PayloadTooLarge()
                                .body("Upload exceeded max size");
                        }
                        if existing_size.saturating_add(total_written) > max_project_bytes {
                            let _ = tokio::fs::remove_file(&filepath).await;
                            return HttpResponse::PayloadTooLarge().body("Project quota exceeded");
                        }
                        if sniff_buf.len() < 8192 {
                            let remaining = 8192 - sniff_buf.len();
                            let take = remaining.min(data.len());
                            sniff_buf.extend_from_slice(&data[..take]);
                            if sniffed_mime.is_none() && sniff_buf.len() >= 16 {
                                if let Some(kind) = infer::get(&sniff_buf) {
                                    sniffed_mime = Some(kind.mime_type().to_string());
                                }
                            }
                        }
                        hasher.update(&data);
                        if let Err(e) = f.write_all(&data).await {
                            let _ = tokio::fs::remove_file(&filepath).await;
                            return HttpResponse::InternalServerError().body(e.to_string());
                        }
                    }
                    Err(e) => {
                        let _ = tokio::fs::remove_file(&filepath).await;
                        return HttpResponse::InternalServerError().body(e.to_string());
                    }
                }
            }

            if let Err(e) = f.flush().await {
                let _ = tokio::fs::remove_file(&filepath).await;
                return HttpResponse::InternalServerError().body(e.to_string());
            }

            if sniffed_mime.is_none() {
                if let Some(kind) = infer::get(&sniff_buf) {
                    sniffed_mime = Some(kind.mime_type().to_string());
                }
            }

            if let Some(mime) = sniffed_mime.as_ref() {
                let ext = std::path::Path::new(&fname)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !is_allowed_mime_for_extension(&ext, mime) {
                    let _ = tokio::fs::remove_file(&filepath).await;
                    return HttpResponse::BadRequest().body("Uploaded file MIME type not allowed");
                }
            }

            if let Err(resp) = run_antivirus_scan(&state, &filepath).await {
                let _ = tokio::fs::remove_file(&filepath).await;
                return resp;
            }

            let sha256 = hex::encode(hasher.finalize());
            let mut stored_path = filepath.clone();
            if policy_for_project(&state.config.paths.projects_dir, &id, &state.config.privacy)
                .is_some()
            {
                let key_store = match project_key_store(&state) {
                    Ok(store) => store,
                    Err(resp) => return resp,
                };
                let key = match key_store.load_or_create(&id) {
                    Ok(key) => key,
                    Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
                };
                if let Ok(Some(enc_path)) = encrypt_file_in_place(&filepath, &key, 0) {
                    stored_path = enc_path;
                }
            }
            imported_files.push(serde_json::json!({
                "filename": fname,
                "bytes": file_written,
                "sha256": sha256,
                "mime": sniffed_mime,
                "path": stored_path.to_string_lossy(),
            }));
        }
    }

    let files_for_log = imported_files.clone();
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "project.import",
            id,
            "success",
            audit_actor(&req).2,
            serde_json::json!({ "files": files_for_log }),
        ),
    );

    HttpResponse::Ok().json(serde_json::json!({
        "status": "imported",
        "files": imported_files
    }))
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/output/{tail}",
    tag = "project",
    params(
        ("id" = String, Path, description = "Project id"),
        ("tail" = String, Path, description = "Output path")
    ),
    responses(
        (status = 200, description = "Output asset"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/projects/{id}/output/{tail:.*}")]
pub async fn download_output_file(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let (id, tail) = path.into_inner();
    let file_path =
        match resolve_project_child_file(&state.config.paths.projects_dir, &id, "output", &tail) {
            Ok(path) => path,
            Err(resp) => return resp,
        };

    match open_project_file(&state, &id, &file_path).await {
        Ok(file) => {
            log_audit(
                &req,
                &state,
                AuditEvent::new(
                    audit_actor(&req).0,
                    audit_actor(&req).1,
                    "project.output.read",
                    id,
                    "success",
                    audit_actor(&req).2,
                    serde_json::json!({ "path": file_path.to_string_lossy() }),
                ),
            );
            file.into_response(&req)
        }
        Err(resp) => resp,
    }
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/assets",
    tag = "project",
    params(
        ("id" = String, Path, description = "Project id"),
        ("scope" = Option<String>, Query, description = "raw|output|processed|all (all excludes raw; default output)"),
        ("limit" = Option<usize>, Query, description = "Maximum number of assets to return")
    ),
    responses(
        (status = 200, description = "Project asset list", body = [ProjectAsset]),
        (status = 401, description = "Unauthorized"),
        (status = 400, description = "Invalid request")
    )
)]
#[get("/api/projects/{id}/assets")]
pub async fn list_project_assets(
    req: HttpRequest,
    path: web::Path<String>,
    query: web::Query<ProjectAssetsQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    let scope = query.scope.clone().unwrap_or_else(|| "output".to_string());
    let limit = query.limit.unwrap_or(500).clamp(1, 5000);

    let mut assets: Vec<(i64, ProjectAsset)> = Vec::new();

    let add_scope = |root: std::path::PathBuf,
                     prefix: &str,
                     traversal_limit: Option<usize>,
                     assets: &mut Vec<(i64, ProjectAsset)>| {
        let mut added = 0usize;
        for entry in WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if traversal_limit.is_some_and(|limit| added >= limit) {
                break;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let rel = match path.strip_prefix(&root) {
                Ok(rel) => rel,
                Err(_) => continue,
            };
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            let meta = entry.metadata().ok();
            let bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified_at = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64);
            let modified_label = modified_at.map(|ts| {
                DateTime::<Utc>::from(UNIX_EPOCH + std::time::Duration::from_secs(ts as u64))
                    .to_rfc3339()
            });
            let path = format!("{}/{}", prefix, rel_str);
            assets.push((
                modified_at.unwrap_or(0),
                ProjectAsset {
                    path,
                    bytes,
                    modified_at: modified_label,
                },
            ));
            added = added.saturating_add(1);
        }
    };

    match scope.as_str() {
        "raw" => {
            let dir = match resolve_project_child(&state.config.paths.projects_dir, &id, "raw") {
                Ok(path) => path,
                Err(resp) => return resp,
            };
            add_scope(dir, "raw", Some(limit), &mut assets);
        }
        "output" => {
            let dir = match resolve_project_child(&state.config.paths.projects_dir, &id, "output") {
                Ok(path) => path,
                Err(resp) => return resp,
            };
            add_scope(dir, "output", None, &mut assets);
        }
        "processed" => {
            let dir =
                match resolve_project_child(&state.config.paths.projects_dir, &id, "processed") {
                    Ok(path) => path,
                    Err(resp) => return resp,
                };
            add_scope(dir, "processed", None, &mut assets);
        }
        "all" => {
            let dir = match resolve_project_child(&state.config.paths.projects_dir, &id, "output") {
                Ok(path) => path,
                Err(resp) => return resp,
            };
            add_scope(dir, "output", None, &mut assets);
            let dir =
                match resolve_project_child(&state.config.paths.projects_dir, &id, "processed") {
                    Ok(path) => path,
                    Err(resp) => return resp,
                };
            add_scope(dir, "processed", None, &mut assets);
        }
        _ => {
            return HttpResponse::BadRequest().body("scope must be raw, output, processed, or all");
        }
    }

    assets.sort_by(|a, b| b.0.cmp(&a.0));
    let result: Vec<ProjectAsset> = assets
        .into_iter()
        .take(limit)
        .map(|(_, asset)| asset)
        .collect();

    HttpResponse::Ok().json(result)
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/fusion-reports",
    tag = "project",
    params(
        ("id" = String, Path, description = "Project id"),
        ("limit" = Option<usize>, Query, description = "Maximum number of reports")
    ),
    responses(
        (status = 200, description = "Validated fusion provenance manifests", body = FusionReportInventory),
        (status = 401, description = "Unauthorized"),
        (status = 402, description = "Advanced Capture add-on required"),
        (status = 400, description = "Invalid request")
    )
)]
#[get("/api/projects/{id}/fusion-reports")]
pub async fn list_fusion_reports(
    req: HttpRequest,
    path: web::Path<String>,
    query: web::Query<FusionReportQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) = require_license_feature(
        &state,
        Feature::AdvancedCaptureAutomation,
        "fusion_inspector",
    ) {
        return resp;
    }

    let id = path.into_inner();
    let output_root = match resolve_project_child(&state.config.paths.projects_dir, &id, "output") {
        Ok(path) => path,
        Err(resp) => return resp,
    };
    let requested_limit = query.limit.unwrap_or(32).clamp(1, MAX_FUSION_REPORTS);
    let mut candidates: HashMap<String, (PathBuf, i64, Option<String>)> = HashMap::new();
    let mut visited_files = 0usize;

    for entry in WalkDir::new(&output_root)
        .max_depth(8)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        visited_files = visited_files.saturating_add(1);
        if visited_files > 20_000 {
            break;
        }
        let relative = match entry.path().strip_prefix(&output_root) {
            Ok(path) => path.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        let logical = relative.strip_suffix(".enc").unwrap_or(&relative);
        if !logical.ends_with("_fusion_report.json") {
            continue;
        }
        let metadata = entry.metadata().ok();
        let modified_at = metadata
            .as_ref()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        let modified_label = (modified_at > 0).then(|| {
            DateTime::<Utc>::from(UNIX_EPOCH + std::time::Duration::from_secs(modified_at as u64))
                .to_rfc3339()
        });
        let is_encrypted = relative.ends_with(".enc");
        candidates
            .entry(logical.to_string())
            .and_modify(|current| {
                let current_encrypted =
                    current.0.extension().and_then(|value| value.to_str()) == Some("enc");
                if (current_encrypted && !is_encrypted) || modified_at > current.1 {
                    *current = (
                        entry.path().to_path_buf(),
                        modified_at,
                        modified_label.clone(),
                    );
                }
            })
            .or_insert_with(|| {
                (
                    entry.path().to_path_buf(),
                    modified_at,
                    modified_label.clone(),
                )
            });
    }

    let mut candidates = candidates.into_iter().collect::<Vec<_>>();
    candidates.sort_by(|a, b| b.1 .1.cmp(&a.1 .1).then_with(|| a.0.cmp(&b.0)));
    let truncated = candidates.len() > requested_limit || visited_files > 20_000;
    candidates.truncate(requested_limit);

    let mut reports = Vec::with_capacity(candidates.len());
    let mut rejected_reports = 0usize;
    for (logical_path, (_, _, modified_at)) in candidates {
        let logical_file = output_root.join(&logical_path);
        let bytes = match read_project_file_bytes_bounded(
            &state,
            &id,
            &logical_file,
            MAX_FUSION_REPORT_BYTES,
        )
        .await
        {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                rejected_reports = rejected_reports.saturating_add(1);
                continue;
            }
            Err(_) => {
                rejected_reports = rejected_reports.saturating_add(1);
                continue;
            }
        };
        let value: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => {
                rejected_reports = rejected_reports.saturating_add(1);
                continue;
            }
        };
        let report_sha256 = hex::encode(Sha256::digest(&bytes));
        match parse_fusion_report_summary(
            &value,
            &logical_path,
            &report_sha256,
            modified_at,
            &output_root,
        ) {
            Ok(report) => reports.push(report),
            Err(_) => rejected_reports = rejected_reports.saturating_add(1),
        }
    }

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "project.fusion_reports.read",
            id,
            "success",
            audit_actor(&req).2,
            serde_json::json!({
                "reports": reports.len(),
                "rejected_reports": rejected_reports,
                "truncated": truncated
            }),
        ),
    );

    HttpResponse::Ok().json(FusionReportInventory {
        reports,
        rejected_reports,
        truncated,
    })
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/fusion-artifact/{tail}",
    tag = "project",
    params(
        ("id" = String, Path, description = "Project id"),
        ("tail" = String, Path, description = "Validated fusion PNG path")
    ),
    responses(
        (status = 200, description = "Fusion provenance PNG"),
        (status = 401, description = "Unauthorized"),
        (status = 402, description = "Advanced Capture add-on required"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/projects/{id}/fusion-artifact/{tail:.*}")]
pub async fn download_fusion_artifact(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) = require_license_feature(
        &state,
        Feature::AdvancedCaptureAutomation,
        "fusion_inspector",
    ) {
        return resp;
    }

    let (id, tail) = path.into_inner();
    if !is_allowed_fusion_artifact(&tail) {
        return HttpResponse::BadRequest().body("Unsupported fusion artifact");
    }
    let file_path =
        match resolve_project_child_file(&state.config.paths.projects_dir, &id, "output", &tail) {
            Ok(path) => path,
            Err(resp) => return resp,
        };
    let bytes =
        match read_project_file_bytes_bounded(&state, &id, &file_path, MAX_FUSION_ARTIFACT_BYTES)
            .await
        {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return HttpResponse::NotFound().body("Fusion artifact not found"),
            Err(resp) => return resp,
        };

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "project.fusion_artifact.read",
            id,
            "success",
            audit_actor(&req).2,
            serde_json::json!({ "path": tail, "bytes": bytes.len() }),
        ),
    );
    HttpResponse::Ok()
        .insert_header(("Content-Type", "image/png"))
        .insert_header(("Cache-Control", "private, no-store"))
        .insert_header(("X-Content-Type-Options", "nosniff"))
        .body(bytes)
}

#[utoipa::path(
    post,
    path = "/api/projects/{id}/fusion-edits",
    tag = "project",
    params(("id" = String, Path, description = "Project id")),
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Immutable measured-source edit document", body = serde_json::Value),
        (status = 400, description = "Invalid or unsafe edit"),
        (status = 401, description = "Unauthorized"),
        (status = 402, description = "Advanced Capture add-on required"),
        (status = 404, description = "Base report not found"),
        (status = 409, description = "Base report lacks revision binding")
    )
)]
#[post("/api/projects/{id}/fusion-edits")]
pub async fn create_fusion_edit(
    req: HttpRequest,
    path: web::Path<String>,
    json: web::Json<CreateFusionEditRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) = require_license_feature(
        &state,
        Feature::AdvancedCaptureAutomation,
        "fusion_inspector",
    ) {
        return resp;
    }

    let id = path.into_inner();
    if !json.report_path.ends_with("_fusion_report.json") {
        return HttpResponse::BadRequest().body("Invalid fusion report path");
    }
    let output_root = match resolve_project_child(&state.config.paths.projects_dir, &id, "output") {
        Ok(path) => path,
        Err(resp) => return resp,
    };
    let report_path = match resolve_project_child_file(
        &state.config.paths.projects_dir,
        &id,
        "output",
        &json.report_path,
    ) {
        Ok(path) => path,
        Err(resp) => return resp,
    };
    let report_bytes =
        match read_project_file_bytes_bounded(&state, &id, &report_path, MAX_FUSION_REPORT_BYTES)
            .await
        {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return HttpResponse::NotFound().body("Base fusion report not found"),
            Err(resp) => return resp,
        };
    let report_value: serde_json::Value = match serde_json::from_slice(&report_bytes) {
        Ok(value) => value,
        Err(_) => return HttpResponse::BadRequest().body("Base fusion report is malformed"),
    };
    let report_sha256 = hex::encode(Sha256::digest(&report_bytes));
    let report_summary = match parse_fusion_report_summary(
        &report_value,
        &json.report_path,
        &report_sha256,
        None,
        &output_root,
    ) {
        Ok(summary) => summary,
        Err(_) => {
            return HttpResponse::BadRequest().body("Base fusion report is not archival-safe")
        }
    };
    if report_value.get("fusion_edit").is_some() {
        return HttpResponse::Conflict()
            .body("Edit chaining is disabled; create revisions from the immutable base report");
    }
    if !report_summary.editable_base {
        return HttpResponse::Conflict().body("Selected report is not an immutable base revision");
    }

    let Some((capture_group_id, width, height, crop_origin_x, crop_origin_y, frame_count)) =
        fusion_edit_binding_from_report(&report_value)
    else {
        return HttpResponse::Conflict().body(
            "Base report predates revision binding; rerun native fusion before creating edits",
        );
    };
    let document = FusionEditDocument {
        schema: FUSION_EDIT_SCHEMA.to_string(),
        capture_group_id,
        base_report_sha256: report_sha256,
        width,
        height,
        crop_origin_x,
        crop_origin_y,
        frame_count,
        operations: json.operations.clone(),
    };
    if let Err(error) = document.validate() {
        return HttpResponse::BadRequest().body(error.to_string());
    }
    let digest = match document.digest() {
        Ok(digest) => digest,
        Err(error) => return HttpResponse::BadRequest().body(error.to_string()),
    };
    let payload = match serde_json::to_vec_pretty(&document) {
        Ok(payload) if payload.len() as u64 <= MAX_FUSION_EDIT_BYTES => payload,
        Ok(_) => return HttpResponse::PayloadTooLarge().body("Fusion edit document is too large"),
        Err(error) => return HttpResponse::BadRequest().body(error.to_string()),
    };

    let edit_directory =
        match ensure_bounded_subdirectory(&output_root, &[".trueshot", "fusion_edits"]) {
            Ok(path) => path,
            Err(error) => return HttpResponse::BadRequest().body(error.to_string()),
        };
    let edit_filename = format!("{}_{}.json", document.capture_group_id, digest);
    let logical_path = edit_directory.join(&edit_filename);
    if !logical_path.starts_with(&edit_directory) {
        return HttpResponse::BadRequest().body("Invalid fusion edit path");
    };
    let relative_path = match logical_path.strip_prefix(&output_root) {
        Ok(path) => path.to_string_lossy().replace('\\', "/"),
        Err(_) => return HttpResponse::BadRequest().body("Invalid fusion edit path"),
    };
    let encrypt_output =
        policy_for_project(&state.config.paths.projects_dir, &id, &state.config.privacy)
            .is_some_and(|policy| policy.scopes.iter().any(|scope| scope == "output"));
    let existing = match read_project_file_bytes_bounded(
        &state,
        &id,
        &logical_path,
        MAX_FUSION_EDIT_BYTES as usize,
    )
    .await
    {
        Ok(value) => value,
        Err(resp) => return resp,
    };
    if let Some(existing) = existing {
        if existing != payload {
            return HttpResponse::Conflict().body("Fusion edit digest collision");
        }
    } else {
        let write_payload = payload.clone();
        let write_result = if encrypt_output {
            let key_store = match project_key_store(&state) {
                Ok(store) => store,
                Err(resp) => return resp,
            };
            let key = match key_store.load_or_create(&id) {
                Ok(key) => key,
                Err(error) => {
                    return HttpResponse::InternalServerError().body(error.to_string());
                }
            };
            let encrypted_path = PathBuf::from(format!("{}.enc", logical_path.display()));
            tokio::task::spawn_blocking(move || {
                write_encrypted_bytes_atomic(&encrypted_path, &key, &write_payload)
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result.map_err(|error| error.to_string()))
        } else {
            let clear_path = logical_path.clone();
            tokio::task::spawn_blocking(move || {
                write_bytes_atomic_no_replace(&clear_path, &write_payload)
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result.map_err(|error| error.to_string()))
        };
        if let Err(error) = write_result {
            match read_project_file_bytes_bounded(
                &state,
                &id,
                &logical_path,
                MAX_FUSION_EDIT_BYTES as usize,
            )
            .await
            {
                Ok(Some(existing)) if existing == payload => {}
                Ok(Some(_)) => {
                    return HttpResponse::Conflict().body("Fusion edit digest collision")
                }
                _ => return HttpResponse::InternalServerError().body(error),
            }
        }
    }

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "project.fusion_edit.create",
            id,
            "success",
            audit_actor(&req).2,
            serde_json::json!({
                "report_path": json.report_path,
                "edit_path": relative_path,
                "digest": digest,
                "operations": document.operations.len(),
                "edited_pixels": document.edited_pixel_count(),
                "encrypted": encrypt_output
            }),
        ),
    );
    let cli_argument =
        (!encrypt_output).then(|| format!("--fusion-edits {}", logical_path.display()));
    HttpResponse::Ok().json(serde_json::json!({
        "schema": document.schema,
        "capture_group_id": document.capture_group_id,
        "base_report_sha256": document.base_report_sha256,
        "digest": digest,
        "path": relative_path,
        "operations": document.operations.len(),
        "edited_pixels": document.edited_pixel_count(),
        "encrypted": encrypt_output,
        "download_filename": edit_filename,
        "cli_argument": cli_argument,
        "document": document
    }))
}

#[utoipa::path(
    post,
    path = "/api/projects/{id}/fusion-revisions",
    tag = "project",
    params(("id" = String, Path, description = "Project id")),
    request_body = ExecuteFusionRevisionRequest,
    responses(
        (status = 202, description = "Measured revision queued", body = serde_json::Value),
        (status = 400, description = "Invalid or non-replayable revision"),
        (status = 401, description = "Unauthorized"),
        (status = 402, description = "Advanced Capture add-on required"),
        (status = 409, description = "Revision identity changed")
    )
)]
#[post("/api/projects/{id}/fusion-revisions")]
pub async fn execute_fusion_revision(
    req: HttpRequest,
    path: web::Path<String>,
    json: web::Json<ExecuteFusionRevisionRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) = require_license_feature(
        &state,
        Feature::AdvancedCaptureAutomation,
        "fusion_inspector",
    ) {
        return resp;
    }
    let id = path.into_inner();
    let payload = FusionRevisionJobPayload {
        schema: "trueshot.fusion.revision-job.v1".to_string(),
        project_id: id.clone(),
        report_path: json.report_path.clone(),
        report_sha256: json.report_sha256.clone(),
        edit_path: json.edit_path.clone(),
        edit_digest: json.edit_digest.clone(),
    };
    if let Err(error) = payload.validate() {
        return HttpResponse::BadRequest().body(error.to_string());
    }
    let config = state.config.clone();
    let preflight_payload = payload.clone();
    match tokio::task::spawn_blocking(move || {
        preflight_fusion_revision(&config, &preflight_payload)
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return HttpResponse::Conflict().body(error.to_string()),
        Err(error) => return HttpResponse::InternalServerError().body(error.to_string()),
    }

    let request_id = json.request_id.unwrap_or_else(Uuid::new_v4);
    let payload_value = match serde_json::to_value(&payload) {
        Ok(value) => value,
        Err(error) => return HttpResponse::BadRequest().body(error.to_string()),
    };
    let (record, created) = match state
        .job_queue
        .enqueue(
            request_id,
            crate::fusion_revision::FUSION_REVISION_JOB_KIND,
            "Measured HDR/focus revision",
            &payload_value,
            1,
        )
        .await
    {
        Ok(result) => result,
        Err(error) => return HttpResponse::InternalServerError().body(error.to_string()),
    };
    if !created {
        let matches = state
            .job_queue
            .get_job_detail(record.id)
            .await
            .ok()
            .flatten()
            .is_some_and(|(existing, existing_payload)| {
                existing.kind == crate::fusion_revision::FUSION_REVISION_JOB_KIND
                    && existing_payload == payload_value
            });
        if !matches {
            return HttpResponse::Conflict().body("Request id is already bound to another job");
        }
    }
    if created {
        let job = match state.fusion_revision_executor.build_job(
            record.id,
            &state.config,
            payload.clone(),
        ) {
            Ok(job) => job,
            Err(error) => {
                let _ = state
                    .job_queue
                    .sync_job_info(
                        record.id,
                        "failed",
                        0.0,
                        None,
                        Some(Utc::now()),
                        Some(error.to_string()),
                    )
                    .await;
                return HttpResponse::InternalServerError().body(error.to_string());
            }
        };
        if let Err(error) = state.scheduler.submit_with_id(record.id, job).await {
            let _ = state
                .job_queue
                .sync_job_info(
                    record.id,
                    "failed",
                    0.0,
                    None,
                    Some(Utc::now()),
                    Some(error.to_string()),
                )
                .await;
            return HttpResponse::InternalServerError().body(error.to_string());
        }
    }
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "project.fusion_revision.execute",
            id,
            if created { "queued" } else { "idempotent" },
            audit_actor(&req).2,
            serde_json::json!({
                "job_id": record.id,
                "request_id": request_id,
                "report_sha256": payload.report_sha256,
                "edit_digest": payload.edit_digest
            }),
        ),
    );
    HttpResponse::Accepted().json(record)
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/fusion-revisions/{job_id}",
    tag = "project",
    params(
        ("id" = String, Path, description = "Project id"),
        ("job_id" = Uuid, Path, description = "Revision job id")
    ),
    responses(
        (status = 200, description = "Measured revision job"),
        (status = 401, description = "Unauthorized"),
        (status = 402, description = "Advanced Capture add-on required"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/projects/{id}/fusion-revisions/{job_id}")]
pub async fn get_fusion_revision(
    req: HttpRequest,
    path: web::Path<(String, Uuid)>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) = require_license_feature(
        &state,
        Feature::AdvancedCaptureAutomation,
        "fusion_inspector",
    ) {
        return resp;
    }
    let (project_id, job_id) = path.into_inner();
    match state.job_queue.get_job_detail(job_id).await {
        Ok(Some((record, payload)))
            if record.kind == crate::fusion_revision::FUSION_REVISION_JOB_KIND
                && payload
                    .get("project_id")
                    .and_then(serde_json::Value::as_str)
                    == Some(project_id.as_str()) =>
        {
            HttpResponse::Ok().json(record)
        }
        Ok(_) => HttpResponse::NotFound().body("Fusion revision job not found"),
        Err(error) => HttpResponse::InternalServerError().body(error.to_string()),
    }
}

#[utoipa::path(
    post,
    path = "/api/projects/{id}/fusion-revisions/{job_id}/cancel",
    tag = "project",
    params(
        ("id" = String, Path, description = "Project id"),
        ("job_id" = Uuid, Path, description = "Revision job id")
    ),
    responses(
        (status = 202, description = "Cancellation requested"),
        (status = 401, description = "Unauthorized"),
        (status = 402, description = "Advanced Capture add-on required"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/projects/{id}/fusion-revisions/{job_id}/cancel")]
pub async fn cancel_fusion_revision(
    req: HttpRequest,
    path: web::Path<(String, Uuid)>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) = require_license_feature(
        &state,
        Feature::AdvancedCaptureAutomation,
        "fusion_inspector",
    ) {
        return resp;
    }
    let (project_id, job_id) = path.into_inner();
    let belongs = state
        .job_queue
        .get_job_detail(job_id)
        .await
        .ok()
        .flatten()
        .is_some_and(|(record, payload)| {
            record.kind == crate::fusion_revision::FUSION_REVISION_JOB_KIND
                && payload
                    .get("project_id")
                    .and_then(serde_json::Value::as_str)
                    == Some(project_id.as_str())
        });
    if !belongs || !state.fusion_revision_executor.cancel(job_id) {
        return HttpResponse::NotFound().body("Active fusion revision job not found");
    }
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "project.fusion_revision.cancel",
            project_id,
            "requested",
            audit_actor(&req).2,
            serde_json::json!({ "job_id": job_id }),
        ),
    );
    HttpResponse::Accepted().json(serde_json::json!({
        "job_id": job_id,
        "status": "cancellation_requested"
    }))
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/license",
    tag = "project",
    params(("id" = String, Path, description = "Project id")),
    responses(
        (status = 200, description = "Project license terms", body = ProjectLicense),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/projects/{id}/license")]
pub async fn get_project_license(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    let project_path = match resolve_project_dir(&state.config.paths.projects_dir, &id) {
        Ok(path) => path,
        Err(resp) => return resp,
    };
    if !project_path.exists() {
        return HttpResponse::NotFound().body("Project not found");
    }

    let metadata_path = project_path.join("project.json");
    let mut license = None;
    if let Ok(payload) = fs::read(&metadata_path).await {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&payload) {
            if let Some(license_value) = value.get("license") {
                license = serde_json::from_value::<ProjectLicense>(license_value.clone()).ok();
            }
        }
    }

    let response = license
        .or_else(|| license_from_config(&state.config))
        .unwrap_or_default();
    HttpResponse::Ok().json(response)
}

#[utoipa::path(
    put,
    path = "/api/projects/{id}/license",
    tag = "project",
    params(("id" = String, Path, description = "Project id")),
    request_body = ProjectLicense,
    responses(
        (status = 200, description = "Project license updated", body = ProjectLicense),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[put("/api/projects/{id}/license")]
pub async fn update_project_license(
    req: HttpRequest,
    path: web::Path<String>,
    payload: web::Json<ProjectLicense>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    let project_path = match resolve_project_dir(&state.config.paths.projects_dir, &id) {
        Ok(path) => path,
        Err(resp) => return resp,
    };
    if !project_path.exists() {
        return HttpResponse::NotFound().body("Project not found");
    }

    let metadata_path = project_path.join("project.json");
    let mut metadata = if let Ok(payload) = fs::read(&metadata_path).await {
        serde_json::from_slice::<serde_json::Value>(&payload)
            .unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let mut updated = payload.into_inner();
    updated.updated_at = Some(Utc::now().to_rfc3339());
    metadata["license"] = serde_json::to_value(&updated).unwrap_or_else(|_| serde_json::json!({}));

    if let Ok(serialized) = serde_json::to_vec_pretty(&metadata) {
        if let Err(err) = fs::write(&metadata_path, serialized).await {
            return HttpResponse::InternalServerError().body(err.to_string());
        }
    }

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "project.license.update",
            id,
            "success",
            audit_actor(&req).2,
            serde_json::json!({ "license": updated }),
        ),
    );

    HttpResponse::Ok().json(updated)
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/imu/diagnostics",
    tag = "project",
    params(("id" = String, Path, description = "Project id")),
    responses(
        (status = 200, description = "IMU diagnostics", body = ImuDiagnostics),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/projects/{id}/imu/diagnostics")]
pub async fn get_imu_diagnostics(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    let imu_path = match resolve_project_child_file(
        &state.config.paths.projects_dir,
        &id,
        "processed",
        "sfm/imu_timeline.json",
    ) {
        Ok(path) => path,
        Err(resp) => return resp,
    };

    let payload = match read_project_file_bytes(&state, &id, &imu_path).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            return HttpResponse::Ok().json(ImuDiagnostics {
                status: "missing".to_string(),
                samples: 0,
                duration_seconds: 0.0,
                sample_rate_hz: 0.0,
                accel_mean: 0.0,
                accel_rms: 0.0,
                accel_peak: 0.0,
                gyro_mean: 0.0,
                gyro_rms: 0.0,
                gyro_peak: 0.0,
                warnings: vec!["IMU timeline not found".to_string()],
            });
        }
        Err(resp) => return resp,
    };

    let samples: Vec<ImuSample> = match serde_json::from_slice(&payload) {
        Ok(samples) => samples,
        Err(err) => {
            return HttpResponse::Ok().json(ImuDiagnostics {
                status: "error".to_string(),
                samples: 0,
                duration_seconds: 0.0,
                sample_rate_hz: 0.0,
                accel_mean: 0.0,
                accel_rms: 0.0,
                accel_peak: 0.0,
                gyro_mean: 0.0,
                gyro_rms: 0.0,
                gyro_peak: 0.0,
                warnings: vec![format!("Failed to parse IMU timeline: {err}")],
            });
        }
    };

    if samples.is_empty() {
        return HttpResponse::Ok().json(ImuDiagnostics {
            status: "empty".to_string(),
            samples: 0,
            duration_seconds: 0.0,
            sample_rate_hz: 0.0,
            accel_mean: 0.0,
            accel_rms: 0.0,
            accel_peak: 0.0,
            gyro_mean: 0.0,
            gyro_rms: 0.0,
            gyro_peak: 0.0,
            warnings: vec!["IMU timeline contains no samples".to_string()],
        });
    }

    let mut accel_sum = 0.0f64;
    let mut accel_sq_sum = 0.0f64;
    let mut accel_peak = 0.0f64;
    let mut gyro_sum = 0.0f64;
    let mut gyro_sq_sum = 0.0f64;
    let mut gyro_peak = 0.0f64;
    let mut first_ts = f64::INFINITY;
    let mut last_ts = f64::NEG_INFINITY;

    for sample in &samples {
        let accel = vector_norm(sample.accel);
        let gyro = vector_norm(sample.gyro);
        accel_sum += accel;
        accel_sq_sum += accel * accel;
        accel_peak = accel_peak.max(accel);
        gyro_sum += gyro;
        gyro_sq_sum += gyro * gyro;
        gyro_peak = gyro_peak.max(gyro);
        first_ts = first_ts.min(sample.timestamp);
        last_ts = last_ts.max(sample.timestamp);
    }

    let count = samples.len() as f64;
    let duration_seconds = (last_ts - first_ts).max(0.0);
    let sample_rate_hz = if duration_seconds > 0.0 {
        (samples.len().saturating_sub(1) as f64) / duration_seconds
    } else {
        0.0
    };
    let accel_mean = accel_sum / count;
    let accel_rms = (accel_sq_sum / count).sqrt();
    let gyro_mean = gyro_sum / count;
    let gyro_rms = (gyro_sq_sum / count).sqrt();

    let mut warnings = Vec::new();
    if samples.len() < 10 {
        warnings.push("IMU sample count is low".to_string());
    }
    if sample_rate_hz > 0.0 && sample_rate_hz < 30.0 {
        warnings.push(format!("IMU sample rate is low ({:.1} Hz)", sample_rate_hz));
    }
    if duration_seconds <= 0.0 {
        warnings.push("IMU timeline duration is zero".to_string());
    }

    HttpResponse::Ok().json(ImuDiagnostics {
        status: "ok".to_string(),
        samples: samples.len(),
        duration_seconds,
        sample_rate_hz,
        accel_mean,
        accel_rms,
        accel_peak,
        gyro_mean,
        gyro_rms,
        gyro_peak,
        warnings,
    })
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/processed/{tail}",
    tag = "project",
    params(
        ("id" = String, Path, description = "Project id"),
        ("tail" = String, Path, description = "Processed path")
    ),
    responses(
        (status = 200, description = "Processed asset"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/projects/{id}/processed/{tail:.*}")]
pub async fn download_processed_file(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let (id, tail) = path.into_inner();
    let file_path =
        match resolve_project_child_file(&state.config.paths.projects_dir, &id, "processed", &tail)
        {
            Ok(path) => path,
            Err(resp) => return resp,
        };

    match open_project_file(&state, &id, &file_path).await {
        Ok(file) => {
            log_audit(
                &req,
                &state,
                AuditEvent::new(
                    audit_actor(&req).0,
                    audit_actor(&req).1,
                    "project.processed.read",
                    id,
                    "success",
                    audit_actor(&req).2,
                    serde_json::json!({ "path": file_path.to_string_lossy() }),
                ),
            );
            file.into_response(&req)
        }
        Err(resp) => resp,
    }
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/raw/{tail}",
    tag = "project",
    params(
        ("id" = String, Path, description = "Project id"),
        ("tail" = String, Path, description = "Raw path")
    ),
    responses(
        (status = 200, description = "Raw asset"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/projects/{id}/raw/{tail:.*}")]
pub async fn download_raw_file(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let (id, tail) = path.into_inner();
    let file_path =
        match resolve_project_child_file(&state.config.paths.projects_dir, &id, "raw", &tail) {
            Ok(path) => path,
            Err(resp) => return resp,
        };

    match open_project_file(&state, &id, &file_path).await {
        Ok(file) => {
            log_audit(
                &req,
                &state,
                AuditEvent::new(
                    audit_actor(&req).0,
                    audit_actor(&req).1,
                    "project.raw.read",
                    id,
                    "success",
                    audit_actor(&req).2,
                    serde_json::json!({ "path": file_path.to_string_lossy() }),
                ),
            );
            file.into_response(&req)
        }
        Err(resp) => resp,
    }
}

#[derive(serde::Deserialize)]
pub struct EncryptionRequest {
    pub scopes: Option<Vec<String>>,
}

#[utoipa::path(
    post,
    path = "/api/projects/{id}/encrypt",
    tag = "project",
    params(("id" = String, Path, description = "Project id")),
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Project encrypted", body = serde_json::Value),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/projects/{id}/encrypt")]
pub async fn encrypt_project(
    req: HttpRequest,
    path: web::Path<String>,
    payload: Option<web::Json<EncryptionRequest>>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    let project_path = match resolve_project_dir(&state.config.paths.projects_dir, &id) {
        Ok(path) => path,
        Err(resp) => return resp,
    };
    if !project_path.exists() {
        return HttpResponse::NotFound().body("Project not found");
    }
    let scopes = payload
        .as_ref()
        .and_then(|p| p.scopes.clone())
        .unwrap_or_else(|| {
            vec![
                "raw".to_string(),
                "processed".to_string(),
                "output".to_string(),
            ]
        });
    let scopes_for_job = scopes.clone();

    let key_store = match project_key_store(&state) {
        Ok(store) => store,
        Err(resp) => return resp,
    };
    let report = match tokio::task::spawn_blocking({
        let project_path = project_path.clone();
        let id = id.clone();
        move || encrypt_project_scopes(&project_path, &id, &scopes_for_job, &key_store)
    })
    .await
    {
        Ok(Ok(report)) => report,
        Ok(Err(err)) => return HttpResponse::InternalServerError().body(err.to_string()),
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "project.encrypt",
            id.clone(),
            "success",
            audit_actor(&req).2,
            serde_json::json!({ "scopes": scopes }),
        ),
    );

    if let Err(err) = mark_project_encrypted(&state.config.paths.projects_dir, &id, &scopes) {
        tracing::warn!("Failed to mark project encryption: {}", err);
    }

    HttpResponse::Ok().json(report)
}

#[utoipa::path(
    post,
    path = "/api/projects/{id}/decrypt",
    tag = "project",
    params(("id" = String, Path, description = "Project id")),
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Project decrypted", body = serde_json::Value),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/projects/{id}/decrypt")]
pub async fn decrypt_project(
    req: HttpRequest,
    path: web::Path<String>,
    payload: Option<web::Json<EncryptionRequest>>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    let project_path = match resolve_project_dir(&state.config.paths.projects_dir, &id) {
        Ok(path) => path,
        Err(resp) => return resp,
    };
    if !project_path.exists() {
        return HttpResponse::NotFound().body("Project not found");
    }
    let scopes = payload
        .as_ref()
        .and_then(|p| p.scopes.clone())
        .unwrap_or_else(|| {
            vec![
                "raw".to_string(),
                "processed".to_string(),
                "output".to_string(),
            ]
        });
    let scopes_for_job = scopes.clone();

    let key_store = match project_key_store(&state) {
        Ok(store) => store,
        Err(resp) => return resp,
    };
    let report = match tokio::task::spawn_blocking({
        let project_path = project_path.clone();
        let id = id.clone();
        move || decrypt_project_scopes(&project_path, &id, &scopes_for_job, &key_store)
    })
    .await
    {
        Ok(Ok(report)) => report,
        Ok(Err(err)) => return HttpResponse::InternalServerError().body(err.to_string()),
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };

    log_audit(
        &req,
        &state,
        AuditEvent::new(
            audit_actor(&req).0,
            audit_actor(&req).1,
            "project.decrypt",
            id.clone(),
            "success",
            audit_actor(&req).2,
            serde_json::json!({ "scopes": scopes }),
        ),
    );

    if let Err(err) = clear_project_encrypted(&state.config.paths.projects_dir, &id) {
        tracing::warn!("Failed to clear project encryption marker: {}", err);
    }

    HttpResponse::Ok().json(report)
}

fn parse_fusion_report_summary(
    value: &serde_json::Value,
    logical_path: &str,
    report_sha256: &str,
    modified_at: Option<String>,
    output_root: &Path,
) -> Result<FusionReportSummary, &'static str> {
    let object = value.as_object().ok_or("Fusion report must be an object")?;
    let schema = bounded_string(object.get("schema"), 80).ok_or("Missing fusion schema")?;
    if schema != "trueshot.fusion.provenance.v2" {
        return Err("Unsupported fusion schema");
    }
    let width = bounded_dimension(object.get("width")).ok_or("Invalid fusion width")?;
    let height = bounded_dimension(object.get("height")).ok_or("Invalid fusion height")?;
    if u64::from(width).saturating_mul(u64::from(height)) > u64::from(u32::MAX) {
        return Err("Fusion dimensions exceed supported pixel count");
    }

    let archival =
        bounded_string(object.get("archival_policy"), 160).ok_or("Missing archival policy")?;
    if archival != "measured_sources_only_no_generative_reconstruction" {
        return Err("Fusion report is not measurement-only");
    }
    let demosaic_value = object
        .get("demosaic")
        .and_then(serde_json::Value::as_object);
    let generative_reconstruction = demosaic_value
        .and_then(|entry| entry.get("generative_reconstruction"))
        .and_then(serde_json::Value::as_bool)
        .ok_or("Missing generative reconstruction policy")?;
    if generative_reconstruction {
        return Err("Generative fusion reports are excluded from archival inspection");
    }

    let report_relative = Path::new(logical_path);
    if report_relative.is_absolute()
        || report_relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("Invalid fusion report path");
    }
    let report_directory = report_relative.parent().unwrap_or_else(|| Path::new(""));
    let mut artifacts = BTreeMap::new();
    let artifact_fields = [
        ("source", "source_map"),
        ("detail_source", "detail_source_map"),
        ("flags", "fusion_flags"),
        ("frequency_flags", "frequency_flags"),
        ("sensor_correction", "sensor_correction_map"),
        ("glare", "glare_map"),
        ("boundary", "boundary_trimap"),
        ("overlay", "overlay"),
    ];
    let mut warnings = Vec::new();
    for (key, field) in artifact_fields {
        let filename = bounded_string(object.get(field), 255).ok_or("Missing fusion artifact")?;
        if !is_portable_png_filename(&filename) {
            return Err("Fusion artifact must be a portable PNG filename");
        }
        let relative = report_directory.join(filename);
        let absolute = output_root.join(&relative);
        let encrypted = PathBuf::from(format!("{}.enc", absolute.display()));
        let present_path = safe_existing_file(output_root, &absolute)
            .or_else(|| safe_existing_file(output_root, &encrypted));
        let present = present_path.is_some();
        let bytes = present_path
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|metadata| metadata.len());
        let portable_path = relative.to_string_lossy().replace('\\', "/");
        if !present {
            warnings.push(format!("Missing {key} artifact: {portable_path}"));
        }
        artifacts.insert(
            key.to_string(),
            FusionArtifactRef {
                path: portable_path,
                present,
                bytes,
            },
        );
    }
    if let Some(edit_filename) = object
        .get("fusion_edit")
        .and_then(serde_json::Value::as_object)
        .and_then(|edit| bounded_string(edit.get("map"), 255))
    {
        if !is_portable_png_filename(&edit_filename) {
            return Err("Fusion edit artifact must be a portable PNG filename");
        }
        let relative = report_directory.join(edit_filename);
        let absolute = output_root.join(&relative);
        let encrypted = PathBuf::from(format!("{}.enc", absolute.display()));
        let present_path = safe_existing_file(output_root, &absolute)
            .or_else(|| safe_existing_file(output_root, &encrypted));
        let present = present_path.is_some();
        let bytes = present_path
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|metadata| metadata.len());
        let portable_path = relative.to_string_lossy().replace('\\', "/");
        if !present {
            warnings.push(format!("Missing edit artifact: {portable_path}"));
        }
        artifacts.insert(
            "edit".to_string(),
            FusionArtifactRef {
                path: portable_path,
                present,
                bytes,
            },
        );
    }
    let integrity_complete = artifacts.values().all(|artifact| artifact.present);

    let noise_model_calibrated = bool_field(object, "noise_model_calibrated");
    let lens_psf_calibrated = bool_field(object, "lens_psf_calibrated");
    let fallback = demosaic_value
        .and_then(|entry| entry.get("fallback"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !noise_model_calibrated {
        warnings
            .push("Noise model is uncalibrated; uncertainty is fallback-qualified.".to_string());
    }
    if !lens_psf_calibrated {
        warnings.push("Lens PSF is uncalibrated; physical focus uses ideal fallback.".to_string());
    }
    if fallback {
        warnings.push("Demosaic backend used a reported fallback.".to_string());
    }

    let mut metrics = BTreeMap::new();
    for field in [
        "defect_repaired_pixels",
        "depth_refusion_pixels",
        "visibility_adjusted_pixels",
        "mixed_boundary_pixels",
        "boundary_source_fallback_pixels",
        "glare_affected_pixels",
        "local_aligned_cells",
        "disoccluded_cells",
        "frequency_separated_pixels",
        "detail_single_source_pixels",
        "detail_reference_pixels",
    ] {
        if let Some(metric) = object.get(field).and_then(serde_json::Value::as_u64) {
            metrics.insert(field.to_string(), metric);
        }
    }

    let performance_value = object
        .get("performance")
        .and_then(serde_json::Value::as_object);
    let performance = FusionPerformanceSummary {
        decode_seconds: finite_nonnegative(performance_value, "decode_seconds"),
        fusion_seconds: finite_nonnegative(performance_value, "fusion_seconds"),
        demosaic_and_postprocess_seconds: finite_nonnegative(
            performance_value,
            "demosaic_and_postprocess_seconds",
        ),
        processing_before_export_seconds: finite_nonnegative(
            performance_value,
            "processing_before_export_seconds",
        ),
        decoded_megapixels: finite_nonnegative(performance_value, "decoded_megapixels"),
        admitted_peak_memory_bytes: u64_field(performance_value, "admitted_peak_memory_bytes"),
        major_page_faults: u64_field(performance_value, "major_page_faults"),
    };

    let label = report_relative
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix("_fusion_report.json"))
        .unwrap_or("Fusion result")
        .to_string();
    let capture_group_id = bounded_string(object.get("capture_group_id"), 64)
        .filter(|value| valid_lower_sha256(value));
    let revision_group_id = bounded_string(object.get("revision_group_id"), 64)
        .filter(|value| valid_lower_sha256(value));
    let frame_count = object
        .get("frame_count")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value > 0);
    let crop_origin = object
        .get("crop_origin")
        .and_then(serde_json::Value::as_object);
    let crop_origin_x = crop_origin
        .and_then(|origin| origin.get("x"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let crop_origin_y = crop_origin
        .and_then(|origin| origin.get("y"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let fusion_edit_digest = object
        .get("fusion_edit")
        .and_then(serde_json::Value::as_object)
        .and_then(|edit| bounded_string(edit.get("digest"), 64))
        .filter(|value| valid_lower_sha256(value));
    let editable_base = capture_group_id.is_some()
        && capture_group_id == revision_group_id
        && frame_count.is_some()
        && crop_origin_x.is_some()
        && crop_origin_y.is_some()
        && object.get("fusion_edit").is_none();
    let revision_executable = editable_base
        && object
            .get("replay")
            .cloned()
            .and_then(|value| serde_json::from_value::<FusionReplayCapsule>(value).ok())
            .is_some_and(|replay| replay.validate().is_ok());
    if editable_base && !revision_executable {
        warnings.push(
            "This base report predates deterministic one-click replay; rerun native fusion."
                .to_string(),
        );
    }

    Ok(FusionReportSummary {
        report_path: logical_path.to_string(),
        report_sha256: report_sha256.to_string(),
        label,
        modified_at,
        schema,
        width,
        height,
        integrity_complete,
        warnings,
        artifacts,
        flags: parse_flag_legend(object.get("flag_legend")),
        frequency_flags: parse_flag_legend(object.get("frequency_flag_legend")),
        boundary_trimap_legend: parse_u8_legend(object.get("boundary_trimap_legend")),
        sensor_correction_legend: parse_u8_legend(object.get("sensor_correction_legend")),
        metrics,
        policy: FusionPolicySummary {
            archival,
            focus: bounded_string(object.get("physical_focus_policy"), 160)
                .unwrap_or_else(|| "unreported".to_string()),
            boundary: bounded_string(object.get("boundary_policy"), 160)
                .unwrap_or_else(|| "unreported".to_string()),
            glare: bounded_string(object.get("glare_policy"), 160)
                .unwrap_or_else(|| "unreported".to_string()),
            frequency: bounded_string(object.get("frequency_policy"), 160)
                .unwrap_or_else(|| "unreported".to_string()),
        },
        calibration: FusionCalibrationSummary {
            noise_model_calibrated,
            lens_psf_calibrated,
            sensor_correction_id: bounded_string(object.get("sensor_correction_id"), 256),
            lens_psf_calibration_id: bounded_string(object.get("lens_psf_calibration_id"), 256),
        },
        demosaic: FusionDemosaicSummary {
            backend: demosaic_value.and_then(|entry| bounded_string(entry.get("backend"), 80)),
            adapter: demosaic_value.and_then(|entry| bounded_string(entry.get("adapter"), 160)),
            fallback,
            generative_reconstruction,
        },
        performance,
        capture_group_id,
        revision_group_id,
        frame_count,
        crop_origin_x,
        crop_origin_y,
        fusion_edit_digest,
        editable_base,
        revision_executable,
    })
}

fn fusion_edit_binding_from_report(
    value: &serde_json::Value,
) -> Option<(String, u32, u32, u32, u32, u16)> {
    let object = value.as_object()?;
    let capture_group_id = object.get("capture_group_id")?.as_str()?.to_string();
    if !valid_lower_sha256(&capture_group_id) {
        return None;
    }
    let width = bounded_dimension(object.get("width"))?;
    let height = bounded_dimension(object.get("height"))?;
    let frame_count = u16::try_from(object.get("frame_count")?.as_u64()?).ok()?;
    if frame_count == 0 {
        return None;
    }
    let crop = object.get("crop_origin")?.as_object()?;
    let crop_origin_x = u32::try_from(crop.get("x")?.as_u64()?).ok()?;
    let crop_origin_y = u32::try_from(crop.get("y")?.as_u64()?).ok()?;
    Some((
        capture_group_id,
        width,
        height,
        crop_origin_x,
        crop_origin_y,
        frame_count,
    ))
}

fn valid_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn bounded_string(value: Option<&serde_json::Value>, max_len: usize) -> Option<String> {
    let value = value?.as_str()?;
    (value.len() <= max_len).then(|| value.to_string())
}

fn bounded_dimension(value: Option<&serde_json::Value>) -> Option<u32> {
    let value = value?.as_u64()?;
    (value > 0 && value <= 200_000)
        .then(|| u32::try_from(value).ok())
        .flatten()
}

fn bool_field(object: &serde_json::Map<String, serde_json::Value>, field: &str) -> bool {
    object
        .get(field)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn finite_nonnegative(
    object: Option<&serde_json::Map<String, serde_json::Value>>,
    field: &str,
) -> Option<f64> {
    let value = object?.get(field)?.as_f64()?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn u64_field(
    object: Option<&serde_json::Map<String, serde_json::Value>>,
    field: &str,
) -> Option<u64> {
    object?.get(field)?.as_u64()
}

fn parse_flag_legend(value: Option<&serde_json::Value>) -> BTreeMap<String, FusionFlagSummary> {
    value
        .and_then(serde_json::Value::as_object)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(name, value)| {
                    let entry = value.as_object()?;
                    let bit = u8::try_from(entry.get("bit")?.as_u64()?).ok()?;
                    let pixels = entry.get("pixels")?.as_u64()?;
                    Some((name.clone(), FusionFlagSummary { bit, pixels }))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_u8_legend(value: Option<&serde_json::Value>) -> BTreeMap<String, u8> {
    value
        .and_then(serde_json::Value::as_object)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(name, value)| {
                    u8::try_from(value.as_u64()?)
                        .ok()
                        .map(|value| (name.clone(), value))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn is_portable_png_filename(filename: &str) -> bool {
    !filename.is_empty()
        && filename.len() <= 255
        && !filename.contains(['/', '\\'])
        && !filename.starts_with('.')
        && filename.to_ascii_lowercase().ends_with(".png")
        && Path::new(filename)
            .file_name()
            .and_then(|name| name.to_str())
            == Some(filename)
}

fn is_allowed_fusion_artifact(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        "_source_map.png",
        "_detail_source_map.png",
        "_fusion_flags.png",
        "_frequency_flags.png",
        "_sensor_correction_map.png",
        "_sensor_correction.png",
        "_glare_map.png",
        "_boundary_trimap.png",
        "_fusion_overlay.png",
        "_fusion_edit_map.png",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

fn is_allowed_model_extension(name: &str) -> bool {
    let allowed = ["ply", "obj", "gltf", "glb", "usdz", "usd"];
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|ext| allowed.iter().any(|a| a.eq_ignore_ascii_case(ext)))
        .unwrap_or(false)
}

fn is_allowed_mime_for_extension(ext: &str, mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    if mime == "application/octet-stream" {
        return true;
    }

    match ext {
        "glb" => matches!(
            mime.as_str(),
            "model/gltf-binary" | "application/octet-stream"
        ),
        "gltf" => matches!(
            mime.as_str(),
            "model/gltf+json" | "application/json" | "text/plain"
        ),
        "obj" => matches!(mime.as_str(), "text/plain" | "application/octet-stream"),
        "ply" => matches!(mime.as_str(), "application/octet-stream" | "text/plain"),
        "usdz" => matches!(
            mime.as_str(),
            "model/vnd.usdz+zip" | "application/zip" | "application/octet-stream"
        ),
        "usd" => matches!(
            mime.as_str(),
            "model/vnd.usd" | "text/plain" | "application/octet-stream"
        ),
        _ => false,
    }
}

async fn open_project_file(
    state: &AppState,
    project_id: &str,
    file_path: &std::path::Path,
) -> Result<NamedFile, HttpResponse> {
    let mut resolved = file_path.to_path_buf();
    if !resolved.exists() {
        let enc_path = std::path::PathBuf::from(format!("{}.enc", resolved.display()));
        if enc_path.exists() {
            let key_store = project_key_store(state)?;
            let key = key_store
                .load_or_create(project_id)
                .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))?;
            if let Ok(restored) = decrypt_file_in_place(&enc_path, &key) {
                resolved = restored;
            }
        }
    }

    if !resolved.exists() {
        return Err(HttpResponse::NotFound().body("File not found"));
    }

    NamedFile::open_async(resolved)
        .await
        .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))
}

async fn read_project_file_bytes(
    state: &AppState,
    project_id: &str,
    file_path: &std::path::Path,
) -> Result<Option<Vec<u8>>, HttpResponse> {
    let mut resolved = file_path.to_path_buf();
    if !resolved.exists() {
        let enc_path = std::path::PathBuf::from(format!("{}.enc", resolved.display()));
        if enc_path.exists() {
            let key_store = project_key_store(state)?;
            let key = key_store
                .load_or_create(project_id)
                .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))?;
            match decrypt_file_in_place(&enc_path, &key) {
                Ok(restored) => resolved = restored,
                Err(err) => {
                    return Err(HttpResponse::InternalServerError().body(err.to_string()));
                }
            }
        } else {
            return Ok(None);
        }
    }

    if !resolved.exists() {
        return Ok(None);
    }

    let bytes = fs::read(resolved)
        .await
        .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))?;
    Ok(Some(bytes))
}

async fn read_project_file_bytes_bounded(
    state: &AppState,
    project_id: &str,
    file_path: &Path,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, HttpResponse> {
    let allowed_root =
        resolve_project_child(&state.config.paths.projects_dir, project_id, "output")?;
    if let Some(safe_path) = safe_existing_file(&allowed_root, file_path) {
        let metadata = fs::metadata(&safe_path)
            .await
            .map_err(|error| HttpResponse::InternalServerError().body(error.to_string()))?;
        if metadata.len() > max_bytes as u64 {
            return Err(HttpResponse::PayloadTooLarge().body(format!(
                "Project file exceeds {} byte read limit",
                max_bytes
            )));
        }
        let bytes = fs::read(safe_path)
            .await
            .map_err(|error| HttpResponse::InternalServerError().body(error.to_string()))?;
        return Ok(Some(bytes));
    }

    let encrypted_path = PathBuf::from(format!("{}.enc", file_path.display()));
    let Some(encrypted_path) = safe_existing_file(&allowed_root, &encrypted_path) else {
        return Ok(None);
    };
    let key_store = project_key_store(state)?;
    let key = key_store
        .load_or_create(project_id)
        .map_err(|error| HttpResponse::InternalServerError().body(error.to_string()))?;
    let decrypted = tokio::task::spawn_blocking(move || {
        decrypt_file_to_bytes(&encrypted_path, &key, max_bytes)
    })
    .await
    .map_err(|error| HttpResponse::InternalServerError().body(error.to_string()))?
    .map_err(|error| {
        if error.to_string().contains("read limit") {
            HttpResponse::PayloadTooLarge().body(error.to_string())
        } else {
            HttpResponse::InternalServerError().body(error.to_string())
        }
    })?;
    Ok(Some(decrypted))
}

fn safe_existing_file(allowed_root: &Path, candidate: &Path) -> Option<PathBuf> {
    let root = allowed_root.canonicalize().ok()?;
    let candidate = candidate.canonicalize().ok()?;
    (candidate.starts_with(root) && candidate.is_file()).then_some(candidate)
}

fn write_bytes_atomic_no_replace(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;

    let parent = path.parent().context("Target has no parent directory")?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("Target filename is not UTF-8")?;
    let temporary = parent.join(format!(
        ".{file_name}.{}.part",
        uuid::Uuid::new_v4().as_simple()
    ));
    let result = (|| -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::hard_link(&temporary, path)?;
        std::fs::remove_file(&temporary)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn ensure_bounded_subdirectory(root: &Path, components: &[&str]) -> anyhow::Result<PathBuf> {
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("Resolve bounded directory root {}", root.display()))?;
    let mut current = canonical_root.clone();
    for component in components {
        if component.is_empty()
            || component.contains(['/', '\\'])
            || matches!(*component, "." | "..")
        {
            anyhow::bail!("Invalid bounded directory component");
        }
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                anyhow::bail!("Bounded directory component is not a real directory")
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)
                    .with_context(|| format!("Create bounded directory {}", current.display()))?;
            }
            Err(error) => return Err(error.into()),
        }
        let canonical = current
            .canonicalize()
            .with_context(|| format!("Resolve bounded directory {}", current.display()))?;
        if !canonical.starts_with(&canonical_root) {
            anyhow::bail!("Bounded directory escaped project output");
        }
        current = canonical;
    }
    Ok(current)
}

fn vector_norm(values: [f64; 3]) -> f64 {
    (values[0] * values[0] + values[1] * values[1] + values[2] * values[2]).sqrt()
}

fn project_key_store(state: &AppState) -> Result<ProjectKeyStore, HttpResponse> {
    let master_key = require_master_key(&state.config.privacy, &state.config.paths.projects_dir)
        .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))?;
    Ok(ProjectKeyStore::new(
        &state.config.paths.projects_dir,
        master_key,
    ))
}

fn license_from_config(config: &crate::config::AppConfig) -> Option<ProjectLicense> {
    let title = config.legal.license_title.clone();
    let url = config.legal.license_url.clone();
    let data_ownership = config.legal.data_ownership.clone();
    let export_rights = config.legal.export_rights.clone();
    if title.is_none() && url.is_none() && data_ownership.is_none() && export_rights.is_none() {
        return None;
    }
    Some(ProjectLicense {
        title,
        url,
        data_ownership,
        export_rights,
        updated_at: Some(Utc::now().to_rfc3339()),
    })
}

async fn run_antivirus_scan(state: &AppState, path: &std::path::Path) -> Result<(), HttpResponse> {
    let Some(cmd) = state.config.server.antivirus_command.as_ref() else {
        return Ok(());
    };

    let mut command = tokio::process::Command::new(cmd);
    let mut used_path = false;
    if let Some(args) = state.config.server.antivirus_args.as_ref() {
        for arg in args {
            if arg.contains("{path}") {
                command.arg(arg.replace("{path}", &path.to_string_lossy()));
                used_path = true;
            } else {
                command.arg(arg);
            }
        }
    }
    if !used_path {
        command.arg(path);
    }

    match command.output().await {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let msg = if !stderr.is_empty() { stderr } else { stdout };
            Err(HttpResponse::BadRequest().body(format!("Antivirus scan failed: {}", msg.trim())))
        }
        Err(e) => {
            Err(HttpResponse::InternalServerError().body(format!("Antivirus scan error: {e}")))
        }
    }
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
    if let Err(err) = state
        .audit
        .append_with_redaction(event, &state.config.privacy)
    {
        tracing::warn!("audit log failed for {}: {}", req.path(), err);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_fusion_report() -> serde_json::Value {
        serde_json::json!({
            "schema": "trueshot.fusion.provenance.v2",
            "capture_group_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "revision_group_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "width": 64,
            "height": 48,
            "frame_count": 6,
            "crop_origin": {"x": 12, "y": 20},
            "source_map": "capture_source_map.png",
            "detail_source_map": "capture_detail_source_map.png",
            "fusion_flags": "capture_fusion_flags.png",
            "frequency_flags": "capture_frequency_flags.png",
            "sensor_correction_map": "capture_sensor_correction_map.png",
            "glare_map": "capture_glare_map.png",
            "boundary_trimap": "capture_boundary_trimap.png",
            "overlay": "capture_fusion_overlay.png",
            "archival_policy": "measured_sources_only_no_generative_reconstruction",
            "replay": {
                "schema": "trueshot.fusion.replay.v1",
                "project_layout": "raw_output_siblings",
                "quality": "ultra",
                "jobs": 2,
                "full_frame": false,
                "gpu_enabled": true,
                "export_depth": false,
                "full_resolution_preview": false,
                "preview_max_dimension": 1600,
                "deghost_strength": 1.0,
                "frequency_separated_deghosting": true,
                "glare_spread_um": 80.0,
                "glare_aware_focus": true,
                "depth_consistent_refusion": true,
                "sensor_noise_profile": null,
                "sensor_correction_profile": null,
                "lens_psf_profile": null
            },
            "physical_focus_policy": "calibrated_breathing_pupil_field_psf",
            "boundary_policy": "single_traceable_measured_focus_plane_no_cross_depth_interpolation",
            "glare_policy": "focus_evidence_suppression_only_measured_radiance_unchanged",
            "frequency_policy": "same_cfa_sparse_low_detail_measured_sources_only_envelope_clamped",
            "noise_model_calibrated": true,
            "lens_psf_calibrated": true,
            "flag_legend": {
                "disoccluded": {"bit": 128, "pixels": 12}
            },
            "frequency_flag_legend": {
                "split_sources": {"bit": 2, "pixels": 5}
            },
            "demosaic": {
                "backend": "metal_ahd",
                "adapter": "Apple M1",
                "fallback": false,
                "generative_reconstruction": false
            },
            "performance": {
                "decode_seconds": 0.4,
                "fusion_seconds": 1.2,
                "admitted_peak_memory_bytes": 1234
            }
        })
    }

    fn write_fusion_artifacts(root: &Path) {
        for name in [
            "capture_source_map.png",
            "capture_detail_source_map.png",
            "capture_fusion_flags.png",
            "capture_frequency_flags.png",
            "capture_sensor_correction_map.png",
            "capture_glare_map.png",
            "capture_boundary_trimap.png",
            "capture_fusion_overlay.png",
        ] {
            std::fs::write(root.join(name), b"png").unwrap();
        }
    }

    #[test]
    fn fusion_report_parser_returns_only_bounded_safe_manifest() {
        let directory = tempfile::tempdir().unwrap();
        write_fusion_artifacts(directory.path());

        let summary = parse_fusion_report_summary(
            &valid_fusion_report(),
            "capture_fusion_report.json",
            &"e".repeat(64),
            Some("2026-07-27T00:00:00Z".to_string()),
            directory.path(),
        )
        .unwrap();

        assert_eq!(summary.schema, "trueshot.fusion.provenance.v2");
        assert_eq!(summary.report_sha256, "e".repeat(64));
        assert_eq!(summary.width, 64);
        assert_eq!(summary.height, 48);
        assert!(summary.integrity_complete);
        assert!(summary.warnings.is_empty());
        assert_eq!(summary.flags["disoccluded"].pixels, 12);
        assert_eq!(
            summary.artifacts["overlay"].path,
            "capture_fusion_overlay.png"
        );
        assert_eq!(summary.performance.fusion_seconds, Some(1.2));
        assert!(!summary.demosaic.generative_reconstruction);
        assert!(summary.editable_base);
    }

    #[test]
    fn fusion_report_parser_rejects_artifact_path_escape() {
        let directory = tempfile::tempdir().unwrap();
        let mut report = valid_fusion_report();
        report["overlay"] = serde_json::json!("../secret.png");

        let result = parse_fusion_report_summary(
            &report,
            "capture_fusion_report.json",
            &"e".repeat(64),
            None,
            directory.path(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn fusion_report_parser_rejects_generative_archival_claims() {
        let directory = tempfile::tempdir().unwrap();
        let mut report = valid_fusion_report();
        report["demosaic"]["generative_reconstruction"] = serde_json::json!(true);

        let result = parse_fusion_report_summary(
            &report,
            "capture_fusion_report.json",
            &"e".repeat(64),
            None,
            directory.path(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn fusion_artifact_allowlist_is_narrow() {
        assert!(is_allowed_fusion_artifact(
            "nested/capture_fusion_overlay.png"
        ));
        assert!(is_allowed_fusion_artifact("capture_source_map.png"));
        assert!(is_allowed_fusion_artifact(
            "capture_edit_deadbeef_fusion_edit_map.png"
        ));
        assert!(is_allowed_fusion_artifact("capture_sensor_correction.png"));
        assert!(!is_allowed_fusion_artifact("capture_fusion_report.json"));
        assert!(!is_allowed_fusion_artifact("capture.tiff"));
    }

    #[test]
    fn fusion_report_parser_exposes_immutable_revision_map() {
        let directory = tempfile::tempdir().unwrap();
        write_fusion_artifacts(directory.path());
        std::fs::write(directory.path().join("capture_fusion_edit_map.png"), b"png").unwrap();
        let mut report = valid_fusion_report();
        report["revision_group_id"] = serde_json::json!("b".repeat(64));
        report["fusion_edit"] = serde_json::json!({
            "schema": "trueshot.fusion.edits.v1",
            "digest": "c".repeat(64),
            "base_report_sha256": "d".repeat(64),
            "map": "capture_fusion_edit_map.png",
            "operations": 1,
            "edited_pixels": 12
        });

        let summary = parse_fusion_report_summary(
            &report,
            "capture_edit_fusion_report.json",
            &"e".repeat(64),
            None,
            directory.path(),
        )
        .unwrap();

        assert!(!summary.editable_base);
        assert_eq!(summary.capture_group_id, Some("a".repeat(64)));
        assert_eq!(summary.revision_group_id, Some("b".repeat(64)));
        assert_eq!(
            summary.artifacts["edit"].path,
            "capture_fusion_edit_map.png"
        );
        assert_eq!(summary.fusion_edit_digest, Some("c".repeat(64)));
        assert!(summary.artifacts["edit"].present);
        assert!(summary.integrity_complete);
    }

    #[test]
    fn fusion_edit_binding_requires_complete_modern_identity() {
        let report = valid_fusion_report();
        assert_eq!(
            fusion_edit_binding_from_report(&report),
            Some(("a".repeat(64), 64, 48, 12, 20, 6))
        );

        let mut malformed = report;
        malformed["capture_group_id"] = serde_json::json!("A".repeat(64));
        assert!(fusion_edit_binding_from_report(&malformed).is_none());
    }

    #[test]
    fn fusion_report_parser_rejects_revision_identity_as_editable_base() {
        let directory = tempfile::tempdir().unwrap();
        write_fusion_artifacts(directory.path());
        let mut report = valid_fusion_report();
        report["revision_group_id"] = serde_json::json!("b".repeat(64));

        let summary = parse_fusion_report_summary(
            &report,
            "capture_fusion_report.json",
            &"e".repeat(64),
            None,
            directory.path(),
        )
        .unwrap();

        assert!(!summary.editable_base);
    }

    #[cfg(unix)]
    #[test]
    fn bounded_edit_directory_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), root.path().join(".trueshot")).unwrap();

        let error =
            ensure_bounded_subdirectory(root.path(), &[".trueshot", "fusion_edits"]).unwrap_err();

        assert!(error.to_string().contains("not a real directory"));
        assert!(!outside.path().join("fusion_edits").exists());
    }

    #[cfg(unix)]
    #[test]
    fn fusion_report_parser_does_not_follow_artifact_symlinks_outside_output() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write_fusion_artifacts(directory.path());
        std::fs::remove_file(directory.path().join("capture_fusion_overlay.png")).unwrap();
        let outside_file = outside.path().join("capture_fusion_overlay.png");
        std::fs::write(&outside_file, b"secret").unwrap();
        symlink(
            &outside_file,
            directory.path().join("capture_fusion_overlay.png"),
        )
        .unwrap();

        let summary = parse_fusion_report_summary(
            &valid_fusion_report(),
            "capture_fusion_report.json",
            &"e".repeat(64),
            None,
            directory.path(),
        )
        .unwrap();

        assert!(!summary.integrity_complete);
        assert!(!summary.artifacts["overlay"].present);
    }
}
