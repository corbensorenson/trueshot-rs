use crate::auth::require_admin;
use crate::licensing::require_license_feature;
use crate::state::AppState;
use actix_web::{get, post, web, HttpRequest, HttpResponse, Responder};
use serde::Deserialize;
use std::path::PathBuf;
use trueshot_core::licensing::Feature;
use trueshot_core::reconstruction::job::{UnifiedJob, UnifiedJobType};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, ToSchema)]
pub struct RemoteJobRequest {
    pub id: Uuid,
    pub kind: String,
    pub name: String,
    pub payload: serde_json::Value,
    pub webhook_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UnifiedJobPayload {
    workspace_path: PathBuf,
    livescan_path: Option<PathBuf>,
    dslr_path: Option<PathBuf>,
    job_type: Option<String>,
}

fn resolve_job_type(kind: &str, payload: &UnifiedJobPayload) -> Option<UnifiedJobType> {
    match kind {
        "unified_gaussian_splatting" => Some(UnifiedJobType::GaussianSplatting),
        "unified_photogrammetry" => Some(UnifiedJobType::Photogrammetry),
        _ => match payload.job_type.as_deref() {
            Some("gaussian_splatting") => Some(UnifiedJobType::GaussianSplatting),
            Some("photogrammetry") => Some(UnifiedJobType::Photogrammetry),
            _ => None,
        },
    }
}

#[utoipa::path(
    post,
    path = "/api/jobs",
    tag = "jobs",
    request_body = RemoteJobRequest,
    responses(
        (status = 200, description = "Job submitted", body = serde_json::Value),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/jobs")]
pub async fn submit_job(
    req: HttpRequest,
    json: web::Json<RemoteJobRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) =
        require_license_feature(&state, Feature::PipelineAutomation, "pipeline_automation")
    {
        return resp;
    }

    let mut payload = json.payload.clone();
    if let Some(url) = json.webhook_url.as_ref() {
        if payload.get("webhook_url").is_none() {
            if let Some(map) = payload.as_object_mut() {
                map.insert(
                    "webhook_url".to_string(),
                    serde_json::Value::String(url.clone()),
                );
            }
        }
    }

    let job = match build_job_from_payload(&json.kind, payload.clone()) {
        Ok(job) => job,
        Err(err) => return HttpResponse::BadRequest().body(err),
    };

    let max_attempts = state.config.server.job_max_attempts.unwrap_or(3) as i64;
    let (record, created) = match state
        .job_queue
        .enqueue(json.id, &json.kind, &json.name, &payload, max_attempts)
        .await
    {
        Ok(result) => result,
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };

    if created {
        if let Err(err) = state.scheduler.submit_with_id(record.id, job).await {
            let _ = state
                .job_queue
                .sync_job_info(
                    record.id,
                    "failed",
                    0.0,
                    None,
                    Some(chrono::Utc::now()),
                    Some(err.to_string()),
                )
                .await;
            return HttpResponse::InternalServerError().body(err.to_string());
        }
    }

    HttpResponse::Ok().json(serde_json::json!({
        "status": record.status,
        "request_id": json.id,
        "job_id": record.id,
        "job_name": record.name,
        "job_kind": record.kind,
        "attempts": record.attempts,
        "max_attempts": record.max_attempts,
    }))
}

#[utoipa::path(
    get,
    path = "/api/jobs",
    tag = "jobs",
    responses(
        (status = 200, description = "Job list", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/jobs")]
pub async fn list_jobs(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) =
        require_license_feature(&state, Feature::PipelineAutomation, "pipeline_automation")
    {
        return resp;
    }

    match state.job_queue.list_jobs().await {
        Ok(jobs) => HttpResponse::Ok().json(jobs),
        Err(err) => HttpResponse::InternalServerError().body(err.to_string()),
    }
}

#[utoipa::path(
    get,
    path = "/api/jobs/{id}",
    tag = "jobs",
    params(("id" = String, Path, description = "Job id")),
    responses(
        (status = 200, description = "Job detail", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/jobs/{id}")]
pub async fn get_job(
    req: HttpRequest,
    path: web::Path<Uuid>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) =
        require_license_feature(&state, Feature::PipelineAutomation, "pipeline_automation")
    {
        return resp;
    }

    match state.job_queue.get_job(path.into_inner()).await {
        Ok(Some(info)) => HttpResponse::Ok().json(info),
        Ok(None) => HttpResponse::NotFound().body("Job not found"),
        Err(err) => HttpResponse::InternalServerError().body(err.to_string()),
    }
}

pub(crate) fn build_job_from_payload(
    kind: &str,
    payload: serde_json::Value,
) -> Result<UnifiedJob, String> {
    let payload: UnifiedJobPayload = serde_json::from_value(payload)
        .map_err(|err| format!("Invalid payload format: {}", err))?;
    let job_type = match resolve_job_type(kind, &payload) {
        Some(job_type) => job_type,
        None => {
            return Err(format!("Unsupported job kind: {}", kind));
        }
    };

    validate_path_str("workspace_path", &payload.workspace_path)?;
    if let Some(livescan) = &payload.livescan_path {
        validate_path_str("livescan_path", livescan)?;
    }
    if let Some(dslr) = &payload.dslr_path {
        validate_path_str("dslr_path", dslr)?;
    }

    let mut job = UnifiedJob::new(payload.workspace_path, job_type);
    if let Some(livescan) = payload.livescan_path {
        job = job.with_livescan(livescan);
    }
    if let Some(dslr) = payload.dslr_path {
        job = job.with_dslr(dslr);
    }
    Ok(job)
}

fn validate_path_str(label: &str, path: &PathBuf) -> Result<(), String> {
    if path.exists() {
        Ok(())
    } else {
        Err(format!("{} does not exist", label))
    }
}
