use actix_web::{get, post, web, HttpRequest, HttpResponse, Responder};
use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::api::project_asset::{
    commit_project_asset_stager, write_project_asset_bytes, OpenedProjectAsset,
};
use crate::auth::require_admin;
use crate::fs_safety::{
    ensure_filename, ensure_project_directory, resolve_project_child_file, stage_project_file,
};
use crate::state::AppState;

use trueshot_core::export::ply::{export_ply_to_writer, PlyExportOptions};
use trueshot_core::gaussian_splatting::splat_edit::{
    apply_splat_edits, load_splat_from_reader, save_splat_to_writer, save_spz_to_writer,
    SplatEditOp,
};
use trueshot_core::mesh::editing::{apply_mesh_edits, MeshEditOp};
use trueshot_core::mesh::io::load_mesh_from_reader;

const MAX_EDIT_HISTORY_BYTES: usize = 16 * 1024 * 1024;

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
    let input_path = match resolve_project_child_file(
        &state.config.paths.projects_dir,
        &project_id,
        "output",
        input,
    ) {
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
    let edit_group = format!("edits/{}", Utc::now().format("%Y%m%dT%H%M%SZ"));
    let output_dir = match resolve_project_child_file(
        &state.config.paths.projects_dir,
        &project_id,
        "output",
        &edit_group,
    ) {
        Ok(path) => path,
        Err(response) => return response,
    };
    if let Err(response) =
        ensure_project_directory(&state.config.paths.projects_dir, &project_id, &output_dir)
    {
        return response;
    }

    let output_name = payload
        .output_name
        .clone()
        .unwrap_or_else(|| format!("mesh_edit_{}.ply", Uuid::new_v4()));
    if let Err(resp) = ensure_filename(&output_name) {
        return resp;
    }
    let output_path = output_dir.join(&output_name);

    let result =
        apply_mesh_edit_pipeline(&state, &project_id, &input_path, &output_path, &payload.ops);
    let response = match result {
        Ok(()) => {
            let entry = EditHistoryEntry {
                id: Uuid::new_v4().to_string(),
                created_at: Utc::now().to_rfc3339(),
                asset_type: "mesh".to_string(),
                input_path: input.to_string(),
                output_path: format!(
                    "output/edits/{}/{}",
                    output_dir.file_name().unwrap().to_string_lossy(),
                    output_name
                ),
                operations: serde_json::to_value(&payload.ops).unwrap_or(serde_json::json!([])),
            };
            let history_path = {
                let _guard = state.project_file_mutations.lock().await;
                match append_edit_history(&state, &project_id, entry) {
                    Ok(path) => path,
                    Err(err) => {
                        return HttpResponse::InternalServerError().body(err.to_string());
                    }
                }
            };
            HttpResponse::Ok().json(EditResponse {
                id: Uuid::new_v4().to_string(),
                output_path: format!("output/{edit_group}/{output_name}"),
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
    let input_path = match resolve_project_child_file(
        &state.config.paths.projects_dir,
        &project_id,
        "output",
        input,
    ) {
        Ok(path) => path,
        Err(resp) => return resp,
    };
    let edit_group = format!("edits/{}", Utc::now().format("%Y%m%dT%H%M%SZ"));
    let output_dir = match resolve_project_child_file(
        &state.config.paths.projects_dir,
        &project_id,
        "output",
        &edit_group,
    ) {
        Ok(path) => path,
        Err(response) => return response,
    };
    if let Err(response) =
        ensure_project_directory(&state.config.paths.projects_dir, &project_id, &output_dir)
    {
        return response;
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

    let result = apply_splat_edit_pipeline(
        &state,
        &project_id,
        &input_path,
        &output_path,
        &payload.ops,
        write_spz,
    );
    let response = match result {
        Ok(()) => {
            let entry = EditHistoryEntry {
                id: Uuid::new_v4().to_string(),
                created_at: Utc::now().to_rfc3339(),
                asset_type: "splat".to_string(),
                input_path: input.to_string(),
                output_path: format!(
                    "output/edits/{}/{}",
                    output_dir.file_name().unwrap().to_string_lossy(),
                    output_name
                ),
                operations: serde_json::to_value(&payload.ops).unwrap_or(serde_json::json!([])),
            };
            let history_path = {
                let _guard = state.project_file_mutations.lock().await;
                match append_edit_history(&state, &project_id, entry) {
                    Ok(path) => path,
                    Err(err) => {
                        return HttpResponse::InternalServerError().body(err.to_string());
                    }
                }
            };
            HttpResponse::Ok().json(EditResponse {
                id: Uuid::new_v4().to_string(),
                output_path: format!("output/{edit_group}/{output_name}"),
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
    match read_edit_history(&state, &project_id) {
        Ok(history) => HttpResponse::Ok().json(history),
        Err(err) => HttpResponse::InternalServerError().body(err.to_string()),
    }
}

fn apply_mesh_edit_pipeline(
    state: &AppState,
    project_id: &str,
    input: &Path,
    output: &Path,
    ops: &[MeshEditOp],
) -> Result<()> {
    let format = input
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| anyhow::anyhow!("Mesh input has no format extension"))?;
    let source = OpenedProjectAsset::open(state, project_id, input)
        .map_err(|response| anyhow::anyhow!("Mesh input open failed: {}", response.status()))?;
    let mut mesh = load_mesh_from_reader(source.into_reader(), format)?;
    apply_mesh_edits(&mut mesh, ops)?;
    let options = PlyExportOptions {
        binary: true,
        include_normals: true,
        include_colors: !mesh.colors.is_empty(),
        include_uvs: !mesh.uvs.is_empty(),
        comment: Some("TrueShot mesh edit".to_string()),
    };
    let mut staged = stage_project_file(&state.config.paths.projects_dir, project_id, output, true)
        .map_err(|response| anyhow::anyhow!("Mesh output stage failed: {}", response.status()))?;
    export_ply_to_writer(&mesh, staged.file_mut(), &options)?;
    commit_project_asset_stager(state, project_id, output, staged)
        .map_err(|response| anyhow::anyhow!("Mesh output commit failed: {}", response.status()))?;
    Ok(())
}

fn apply_splat_edit_pipeline(
    state: &AppState,
    project_id: &str,
    input: &Path,
    output: &Path,
    ops: &[SplatEditOp],
    write_spz_file: bool,
) -> Result<()> {
    let source = OpenedProjectAsset::open(state, project_id, input)
        .map_err(|response| anyhow::anyhow!("Splat input open failed: {}", response.status()))?;
    let points = load_splat_from_reader(source.into_reader())?;
    let edited = apply_splat_edits(points, ops);
    let mut staged = stage_project_file(&state.config.paths.projects_dir, project_id, output, true)
        .map_err(|response| anyhow::anyhow!("Splat output stage failed: {}", response.status()))?;
    save_splat_to_writer(staged.file_mut(), &edited)?;
    commit_project_asset_stager(state, project_id, output, staged)
        .map_err(|response| anyhow::anyhow!("Splat output commit failed: {}", response.status()))?;
    if write_spz_file {
        let spz_path = output.with_extension("spz");
        let mut staged = stage_project_file(
            &state.config.paths.projects_dir,
            project_id,
            &spz_path,
            true,
        )
        .map_err(|response| anyhow::anyhow!("SPZ output stage failed: {}", response.status()))?;
        save_spz_to_writer(staged.file_mut(), &edited)?;
        commit_project_asset_stager(state, project_id, &spz_path, staged).map_err(|response| {
            anyhow::anyhow!("SPZ output commit failed: {}", response.status())
        })?;
    }
    Ok(())
}

fn history_path(root: &Path, project_id: &str) -> Result<PathBuf> {
    resolve_project_child_file(root, project_id, "output", "edits/history.json")
        .map_err(|response| anyhow::anyhow!("Invalid edit history path: {}", response.status()))
}

fn read_edit_history(state: &AppState, project_id: &str) -> Result<Vec<EditHistoryEntry>> {
    let path = history_path(&state.config.paths.projects_dir, project_id)?;
    let payload = match OpenedProjectAsset::open(state, project_id, &path) {
        Ok(asset) => asset
            .read_to_end_bounded(MAX_EDIT_HISTORY_BYTES)
            .map_err(|response| {
                anyhow::anyhow!("Edit history read failed: {}", response.status())
            })?,
        Err(response) if response.status() == actix_web::http::StatusCode::NOT_FOUND => {
            return Ok(Vec::new())
        }
        Err(response) => {
            return Err(anyhow::anyhow!(
                "Edit history open failed: {}",
                response.status()
            ))
        }
    };
    let history: Vec<EditHistoryEntry> = serde_json::from_slice(&payload)?;
    Ok(history)
}

fn append_edit_history(
    state: &AppState,
    project_id: &str,
    entry: EditHistoryEntry,
) -> Result<String> {
    let path = history_path(&state.config.paths.projects_dir, project_id)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Edit history has no parent"))?;
    ensure_project_directory(&state.config.paths.projects_dir, project_id, parent)
        .map_err(|response| anyhow::anyhow!("Edit directory failed: {}", response.status()))?;
    let mut history = read_edit_history(state, project_id)?;
    history.push(entry);
    let json = serde_json::to_vec_pretty(&history)?;
    write_project_asset_bytes(state, project_id, &path, &json)
        .map_err(|response| anyhow::anyhow!("Edit history write failed: {}", response.status()))?;
    Ok("output/edits/history.json".to_string())
}
