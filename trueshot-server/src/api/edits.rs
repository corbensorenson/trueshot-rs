use actix_web::{get, post, web, HttpRequest, HttpResponse, Responder};
use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::auth::require_admin;
use crate::fs_safety::{ensure_filename, resolve_project_child, resolve_project_child_file};
use crate::state::AppState;

use trueshot_core::export::ply::{export_ply, PlyExportOptions};
use trueshot_core::mesh::editing::{apply_mesh_edits, MeshEditOp};
use trueshot_core::mesh::io::load_mesh;
use trueshot_core::gaussian_splatting::splat_edit::{
    apply_splat_edits, load_splat, save_splat, save_spz, SplatEditOp,
};

#[derive(Debug, Deserialize)]
pub struct MeshEditRequest {
    pub input_path: String,
    pub output_name: Option<String>,
    pub output_format: Option<String>,
    pub ops: Vec<MeshEditOp>,
}

#[derive(Debug, Deserialize)]
pub struct SplatEditRequest {
    pub input_path: String,
    pub output_name: Option<String>,
    pub write_spz: Option<bool>,
    pub ops: Vec<SplatEditOp>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EditHistoryEntry {
    pub id: String,
    pub created_at: String,
    pub asset_type: String,
    pub input_path: String,
    pub output_path: String,
    pub operations: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct EditResponse {
    pub id: String,
    pub output_path: String,
    pub history_path: String,
}

#[utoipa::path(
    post,
    path = "/api/projects/{id}/edits/mesh",
    tag = "edits",
    params(("id" = String, Path, description = "Project id")),
    request_body = MeshEditRequest,
    responses(
        (status = 200, description = "Mesh edit applied", body = EditResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/projects/{id}/edits/mesh")]
pub async fn edit_mesh(
    req: HttpRequest,
    path: web::Path<String>,
    payload: web::Json<MeshEditRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let project_id = path.into_inner();
    let input = payload.input_path.trim();
    if input.is_empty() {
        return HttpResponse::BadRequest().body("Missing input_path");
    }
    let input_path = match resolve_project_child_file(&state.config.paths.projects_dir, &project_id, "output", input) {
        Ok(path) => path,
        Err(resp) => return resp,
    };
    let output_format = payload
        .output_format
        .as_deref()
        .unwrap_or("ply")
        .to_lowercase();
    if output_format != "ply" {
        return HttpResponse::BadRequest().body("Only ply output_format is supported");
    }
    let output_dir = match resolve_project_child(&state.config.paths.projects_dir, &project_id, "output") {
        Ok(path) => path.join("edits").join(Utc::now().format("%Y%m%dT%H%M%SZ").to_string()),
        Err(resp) => return resp,
    };
    if let Err(err) = tokio::fs::create_dir_all(&output_dir).await {
        return HttpResponse::InternalServerError().body(err.to_string());
    }

    let output_name = payload
        .output_name
        .clone()
        .unwrap_or_else(|| format!("mesh_edit_{}.ply", Uuid::new_v4()));
    if let Err(resp) = ensure_filename(&output_name) {
        return resp;
    }
    let output_path = output_dir.join(&output_name);

    let result = apply_mesh_edit_pipeline(&input_path, &output_path, &payload.ops);
    let response = match result {
        Ok(()) => {
            let entry = EditHistoryEntry {
                id: Uuid::new_v4().to_string(),
                created_at: Utc::now().to_rfc3339(),
                asset_type: "mesh".to_string(),
                input_path: input.to_string(),
                output_path: format!("output/edits/{}/{}", output_dir.file_name().unwrap().to_string_lossy(), output_name),
                operations: serde_json::to_value(&payload.ops).unwrap_or(serde_json::json!([])),
            };
            let history_path = match append_edit_history(&state.config.paths.projects_dir, &project_id, entry) {
                Ok(path) => path,
                Err(err) => {
                    return HttpResponse::InternalServerError().body(err.to_string());
                }
            };
            HttpResponse::Ok().json(EditResponse {
                id: Uuid::new_v4().to_string(),
                output_path: output_path.to_string_lossy().to_string(),
                history_path,
            })
        }
        Err(err) => HttpResponse::InternalServerError().body(err.to_string()),
    };
    response
}

#[utoipa::path(
    post,
    path = "/api/projects/{id}/edits/splat",
    tag = "edits",
    params(("id" = String, Path, description = "Project id")),
    request_body = SplatEditRequest,
    responses(
        (status = 200, description = "Splat edit applied", body = EditResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/projects/{id}/edits/splat")]
pub async fn edit_splat(
    req: HttpRequest,
    path: web::Path<String>,
    payload: web::Json<SplatEditRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let project_id = path.into_inner();
    let input = payload.input_path.trim();
    if input.is_empty() {
        return HttpResponse::BadRequest().body("Missing input_path");
    }
    let input_path = match resolve_project_child_file(&state.config.paths.projects_dir, &project_id, "output", input) {
        Ok(path) => path,
        Err(resp) => return resp,
    };
    let output_dir = match resolve_project_child(&state.config.paths.projects_dir, &project_id, "output") {
        Ok(path) => path.join("edits").join(Utc::now().format("%Y%m%dT%H%M%SZ").to_string()),
        Err(resp) => return resp,
    };
    if let Err(err) = tokio::fs::create_dir_all(&output_dir).await {
        return HttpResponse::InternalServerError().body(err.to_string());
    }

    let output_name = payload
        .output_name
        .clone()
        .unwrap_or_else(|| format!("splat_edit_{}.splat", Uuid::new_v4()));
    if let Err(resp) = ensure_filename(&output_name) {
        return resp;
    }
    let output_path = output_dir.join(&output_name);
    let write_spz = payload.write_spz.unwrap_or(true);

    let result = apply_splat_edit_pipeline(&input_path, &output_path, &payload.ops, write_spz);
    let response = match result {
        Ok(()) => {
            let entry = EditHistoryEntry {
                id: Uuid::new_v4().to_string(),
                created_at: Utc::now().to_rfc3339(),
                asset_type: "splat".to_string(),
                input_path: input.to_string(),
                output_path: format!("output/edits/{}/{}", output_dir.file_name().unwrap().to_string_lossy(), output_name),
                operations: serde_json::to_value(&payload.ops).unwrap_or(serde_json::json!([])),
            };
            let history_path = match append_edit_history(&state.config.paths.projects_dir, &project_id, entry) {
                Ok(path) => path,
                Err(err) => {
                    return HttpResponse::InternalServerError().body(err.to_string());
                }
            };
            HttpResponse::Ok().json(EditResponse {
                id: Uuid::new_v4().to_string(),
                output_path: output_path.to_string_lossy().to_string(),
                history_path,
            })
        }
        Err(err) => HttpResponse::InternalServerError().body(err.to_string()),
    };
    response
}

#[utoipa::path(
    get,
    path = "/api/projects/{id}/edits/history",
    tag = "edits",
    params(("id" = String, Path, description = "Project id")),
    responses(
        (status = 200, description = "Edit history", body = [EditHistoryEntry]),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/projects/{id}/edits/history")]
pub async fn get_edit_history(
    req: HttpRequest,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let project_id = path.into_inner();
    match read_edit_history(&state.config.paths.projects_dir, &project_id) {
        Ok(history) => HttpResponse::Ok().json(history),
        Err(err) => HttpResponse::InternalServerError().body(err.to_string()),
    }
}

fn apply_mesh_edit_pipeline(input: &Path, output: &Path, ops: &[MeshEditOp]) -> Result<()> {
    let mut mesh = load_mesh(input)?;
    apply_mesh_edits(&mut mesh, ops)?;
    let options = PlyExportOptions {
        binary: true,
        include_normals: true,
        include_colors: !mesh.colors.is_empty(),
        include_uvs: !mesh.uvs.is_empty(),
        comment: Some("TrueShot mesh edit".to_string()),
    };
    export_ply(&mesh, output, &options)?;
    Ok(())
}

fn apply_splat_edit_pipeline(input: &Path, output: &Path, ops: &[SplatEditOp], write_spz_file: bool) -> Result<()> {
    let points = load_splat(input)?;
    let edited = apply_splat_edits(points, ops);
    save_splat(output, &edited)?;
    if write_spz_file {
        let spz_path = output.with_extension("spz");
        let _ = save_spz(&spz_path, &edited);
    }
    Ok(())
}

fn history_path(root: &Path, project_id: &str) -> Result<PathBuf> {
    let output_dir = resolve_project_child(root, project_id, "output")
        .map_err(|_| anyhow::anyhow!("Invalid project"))?;
    Ok(output_dir.join("edits").join("history.json"))
}

fn read_edit_history(root: &Path, project_id: &str) -> Result<Vec<EditHistoryEntry>> {
    let path = history_path(root, project_id)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let payload = std::fs::read_to_string(&path)?;
    let history: Vec<EditHistoryEntry> = serde_json::from_str(&payload)?;
    Ok(history)
}

fn append_edit_history(root: &Path, project_id: &str, entry: EditHistoryEntry) -> Result<String> {
    let path = history_path(root, project_id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut history = if path.exists() {
        let payload = std::fs::read_to_string(&path)?;
        serde_json::from_str::<Vec<EditHistoryEntry>>(&payload).unwrap_or_default()
    } else {
        Vec::new()
    };
    history.push(entry);
    let json = serde_json::to_string_pretty(&history)?;
    std::fs::write(&path, json)?;
    Ok(path.to_string_lossy().to_string())
}
