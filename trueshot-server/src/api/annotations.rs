use actix_web::{get, post, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use hex;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::require_admin;
use crate::fs_safety::resolve_project_child;
use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
pub struct AnnotationPoint {
    pub id: String,
    pub label: String,
    pub position: [f32; 3],
    pub created_at: i64,
    pub author: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
pub struct AnnotationLayer {
    pub asset_path: String,
    pub layer: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub annotations: Vec<AnnotationPoint>,
}

#[derive(Debug, Deserialize, ToSchema, Clone)]
pub struct AnnotationPointInput {
    pub id: Option<String>,
    pub label: String,
    pub position: [f32; 3],
    pub created_at: Option<i64>,
    pub author: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AnnotationRequest {
    pub asset_path: String,
    pub layer: Option<String>,
    pub annotations: Vec<AnnotationPointInput>,
    pub merge: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct AnnotationQuery {
    pub asset_path: String,
    pub layer: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/annotations",
    tag = "project",
    params(
        ("id" = String, Path, description = "Project id"),
        ("asset_path" = String, Query, description = "Asset path"),
        ("layer" = Option<String>, Query, description = "Annotation layer")
    ),
    responses(
        (status = 200, description = "Annotations", body = AnnotationLayer),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/projects/{id}/annotations")]
pub async fn get_project_annotations(
    req: HttpRequest,
    path: web::Path<String>,
    query: web::Query<AnnotationQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let project_id = path.into_inner();
    let layer = query.layer.clone().unwrap_or_else(|| "default".to_string());
    match load_annotations_for_asset(&state, &project_id, &query.asset_path, &layer) {
        Ok(layer) => HttpResponse::Ok().json(layer),
        Err(resp) => resp,
    }
}

#[utoipa::path(
    post,
    path = "/api/projects/{id}/annotations",
    tag = "project",
    params(("id" = String, Path, description = "Project id")),
    request_body = AnnotationRequest,
    responses(
        (status = 200, description = "Annotations updated", body = AnnotationLayer),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/projects/{id}/annotations")]
pub async fn save_project_annotations(
    req: HttpRequest,
    path: web::Path<String>,
    payload: web::Json<AnnotationRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let project_id = path.into_inner();
    let layer = payload.layer.clone().unwrap_or_else(|| "default".to_string());
    let author = req
        .extensions()
        .get::<crate::auth::AuthContext>()
        .map(|ctx| ctx.sub.clone());

    let saved = match save_annotations_for_asset(
        &state,
        &project_id,
        &payload.asset_path,
        &layer,
        payload.annotations.clone(),
        payload.merge.unwrap_or(true),
        author,
    ) {
        Ok(layer) => layer,
        Err(resp) => return resp,
    };
    HttpResponse::Ok().json(saved)
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(get_project_annotations)
        .service(save_project_annotations);
}

pub(crate) fn load_annotations_for_asset(
    state: &AppState,
    project_id: &str,
    asset_path: &str,
    layer: &str,
) -> Result<AnnotationLayer, HttpResponse> {
    if !asset_path.starts_with("output/") && !asset_path.starts_with("processed/") {
        return Err(HttpResponse::BadRequest().body("asset_path must begin with output/ or processed/"));
    }
    let annotations_path = annotation_file_path(&state.config.paths.projects_dir, project_id, asset_path, layer)?;
    if !annotations_path.exists() {
        let now = unix_timestamp();
        return Ok(AnnotationLayer {
            asset_path: asset_path.to_string(),
            layer: layer.to_string(),
            created_at: now,
            updated_at: now,
            annotations: Vec::new(),
        });
    }
    let payload = std::fs::read_to_string(&annotations_path)
        .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))?;
    let parsed: AnnotationLayer = serde_json::from_str(&payload)
        .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))?;
    Ok(parsed)
}

pub(crate) fn save_annotations_for_asset(
    state: &AppState,
    project_id: &str,
    asset_path: &str,
    layer: &str,
    annotations: Vec<AnnotationPointInput>,
    merge: bool,
    author: Option<String>,
) -> Result<AnnotationLayer, HttpResponse> {
    if !asset_path.starts_with("output/") && !asset_path.starts_with("processed/") {
        return Err(HttpResponse::BadRequest().body("asset_path must begin with output/ or processed/"));
    }
    let annotations_path = annotation_file_path(&state.config.paths.projects_dir, project_id, asset_path, layer)?;
    if let Some(parent) = annotations_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))?;
    }

    let now = unix_timestamp();
    let existing = if merge && annotations_path.exists() {
        let payload = std::fs::read_to_string(&annotations_path)
            .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))?;
        serde_json::from_str::<AnnotationLayer>(&payload).unwrap_or_else(|_| AnnotationLayer {
            asset_path: asset_path.to_string(),
            layer: layer.to_string(),
            created_at: now,
            updated_at: now,
            annotations: Vec::new(),
        })
    } else {
        AnnotationLayer {
            asset_path: asset_path.to_string(),
            layer: layer.to_string(),
            created_at: now,
            updated_at: now,
            annotations: Vec::new(),
        }
    };

    let mut map = std::collections::HashMap::new();
    for item in existing.annotations.into_iter() {
        map.insert(item.id.clone(), item);
    }
    let mut created_at = existing.created_at;
    if created_at == 0 {
        created_at = now;
    }
    for input in annotations {
        let id = input.id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let entry = AnnotationPoint {
            id: id.clone(),
            label: input.label,
            position: input.position,
            created_at: input.created_at.unwrap_or(now),
            author: input.author.or_else(|| author.clone()),
        };
        map.insert(id, entry);
    }
    let mut merged = map.into_values().collect::<Vec<_>>();
    merged.sort_by_key(|item| item.created_at);
    let layer_out = AnnotationLayer {
        asset_path: asset_path.to_string(),
        layer: layer.to_string(),
        created_at,
        updated_at: now,
        annotations: merged,
    };
    let payload = serde_json::to_string_pretty(&layer_out)
        .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))?;
    std::fs::write(&annotations_path, payload)
        .map_err(|e| HttpResponse::InternalServerError().body(e.to_string()))?;
    Ok(layer_out)
}

fn annotation_file_path(
    projects_dir: &std::path::Path,
    project_id: &str,
    asset_path: &str,
    layer: &str,
) -> Result<std::path::PathBuf, HttpResponse> {
    let output_dir = resolve_project_child(projects_dir, project_id, "output")?;
    let annotations_dir = output_dir.join("annotations");
    let key = format!("{}|{}", asset_path, layer);
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let hash = hex::encode(hasher.finalize());
    Ok(annotations_dir.join(format!("{hash}.json")))
}

fn unix_timestamp() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs() as i64
}
