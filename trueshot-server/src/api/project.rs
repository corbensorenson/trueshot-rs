use actix_files::NamedFile;
use actix_web::{delete, get, post, put, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use crate::state::AppState;
use crate::auth::require_admin;
use crate::audit::AuditEvent;
use crate::fs_safety::{
    resolve_project_child,
    resolve_project_child_file,
    resolve_project_dir,
    resolve_project_file,
    project_size_bytes,
};
use crate::at_rest::{
    ProjectKeyStore,
    encrypt_project_scopes,
    decrypt_project_scopes,
    encrypt_file_in_place,
    decrypt_file_in_place,
    mark_project_encrypted,
    clear_project_encrypted,
    policy_for_project,
    require_master_key,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use utoipa::ToSchema;
use walkdir::WalkDir;
use std::time::UNIX_EPOCH;

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
pub async fn create_project(req: HttpRequest, json: web::Json<CreateProjectRequest>, state: web::Data<AppState>) -> impl Responder {
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
            tracing::warn!("Failed to write project metadata {:?}: {}", metadata_path, e);
        }
    }

    if let Some(policy) = policy_for_project(&state.config.paths.projects_dir, &json.name, &state.config.privacy) {
        if let Err(err) = mark_project_encrypted(&state.config.paths.projects_dir, &json.name, &policy.scopes) {
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
pub async fn purge_project_raw(req: HttpRequest, path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
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
    }).await;

    match result {
        Ok(list) => HttpResponse::Ok().json(list),
        Err(e) => HttpResponse::InternalServerError().body(e.to_string())
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
pub async fn open_project_fs(req: HttpRequest, path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
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
         return HttpResponse::InternalServerError().body(format!("Failed to open filesystem: {}", e));
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
use sha2::{Digest, Sha256};

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
    state: web::Data<AppState>
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

    let max_upload_bytes = state.config.server.max_upload_bytes.unwrap_or(10 * 1024 * 1024 * 1024);
    let max_project_bytes = state.config.server.max_project_bytes.unwrap_or(100 * 1024 * 1024 * 1024);

    let existing_size = match project_size_bytes(&project_path) {
        Ok(size) => size,
        Err(resp) => return resp,
    };
    let mut total_written: u64 = 0;
    let mut imported_files: Vec<serde_json::Value> = Vec::new();
    
    // Iterate over multipart stream
    while let Ok(Some(mut field)) = payload.try_next().await {
        let content_disposition = field.content_disposition();
        let filename = content_disposition.and_then(|cd| cd.get_filename()).map(|f| f.to_string());
        
    if let Some(fname) = filename {
        if !is_allowed_model_extension(&fname) {
            return HttpResponse::BadRequest().body("Unsupported file type");
        }
            let filepath = match resolve_project_file(&state.config.paths.projects_dir, &id, &fname) {
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
                            return HttpResponse::PayloadTooLarge().body("Upload exceeded max size");
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
                    },
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
            if policy_for_project(&state.config.paths.projects_dir, &id, &state.config.privacy).is_some() {
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
    let file_path = match resolve_project_child_file(&state.config.paths.projects_dir, &id, "output", &tail) {
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
        ("scope" = Option<String>, Query, description = "output|processed|all (default output)"),
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

    let add_scope = |root: std::path::PathBuf, prefix: &str, assets: &mut Vec<(i64, ProjectAsset)>| {
        for entry in WalkDir::new(&root).follow_links(false).into_iter().filter_map(Result::ok) {
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
            let modified_label = modified_at.map(|ts| DateTime::<Utc>::from(UNIX_EPOCH + std::time::Duration::from_secs(ts as u64)).to_rfc3339());
            let path = format!("{}/{}", prefix, rel_str);
            assets.push((
                modified_at.unwrap_or(0),
                ProjectAsset {
                    path,
                    bytes,
                    modified_at: modified_label,
                },
            ));
        }
    };

    match scope.as_str() {
        "output" => {
            let dir = match resolve_project_child(&state.config.paths.projects_dir, &id, "output") {
                Ok(path) => path,
                Err(resp) => return resp,
            };
            add_scope(dir, "output", &mut assets);
        }
        "processed" => {
            let dir = match resolve_project_child(&state.config.paths.projects_dir, &id, "processed") {
                Ok(path) => path,
                Err(resp) => return resp,
            };
            add_scope(dir, "processed", &mut assets);
        }
        "all" => {
            let dir = match resolve_project_child(&state.config.paths.projects_dir, &id, "output") {
                Ok(path) => path,
                Err(resp) => return resp,
            };
            add_scope(dir, "output", &mut assets);
            let dir = match resolve_project_child(&state.config.paths.projects_dir, &id, "processed") {
                Ok(path) => path,
                Err(resp) => return resp,
            };
            add_scope(dir, "processed", &mut assets);
        }
        _ => {
            return HttpResponse::BadRequest().body("scope must be output, processed, or all");
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

    let response = license.or_else(|| license_from_config(&state.config)).unwrap_or_default();
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
        serde_json::from_slice::<serde_json::Value>(&payload).unwrap_or_else(|_| serde_json::json!({}))
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
    let file_path = match resolve_project_child_file(&state.config.paths.projects_dir, &id, "processed", &tail) {
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
    let file_path = match resolve_project_child_file(&state.config.paths.projects_dir, &id, "raw", &tail) {
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
        .unwrap_or_else(|| vec!["raw".to_string(), "processed".to_string(), "output".to_string()]);
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
        .unwrap_or_else(|| vec!["raw".to_string(), "processed".to_string(), "output".to_string()]);
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
        "glb" => matches!(mime.as_str(), "model/gltf-binary" | "application/octet-stream"),
        "gltf" => matches!(mime.as_str(), "model/gltf+json" | "application/json" | "text/plain"),
        "obj" => matches!(mime.as_str(), "text/plain" | "application/octet-stream"),
        "ply" => matches!(mime.as_str(), "application/octet-stream" | "text/plain"),
        "usdz" => matches!(mime.as_str(), "model/vnd.usdz+zip" | "application/zip" | "application/octet-stream"),
        "usd" => matches!(mime.as_str(), "model/vnd.usd" | "text/plain" | "application/octet-stream"),
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

fn vector_norm(values: [f64; 3]) -> f64 {
    (values[0] * values[0] + values[1] * values[1] + values[2] * values[2]).sqrt()
}

fn project_key_store(state: &AppState) -> Result<ProjectKeyStore, HttpResponse> {
    let master_key = require_master_key(&state.config.privacy, &state.config.paths.projects_dir)
        .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))?;
    Ok(ProjectKeyStore::new(&state.config.paths.projects_dir, master_key))
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
        Err(e) => Err(HttpResponse::InternalServerError().body(format!(
            "Antivirus scan error: {e}"
        ))),
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
    if let Err(err) = state.audit.append_with_redaction(event, &state.config.privacy) {
        tracing::warn!("audit log failed for {}: {}", req.path(), err);
    }
}
