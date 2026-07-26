use axum::{
    extract::{State, Path, FromRef},
    response::IntoResponse,
    Json,
};
use std::sync::Arc;
use trueshot_core::inventory::Inventory;
use trueshot_core::project::ScanProject;
use trueshot_core::director::Director;
use serde::Deserialize;

#[derive(Clone)]
pub struct ProjectState {
    pub inventory: Arc<Inventory>,
    pub director: Arc<Director>,
    pub base_dir: std::path::PathBuf,
}

#[derive(Deserialize)]
pub struct CreateProjectRequest {
    name: String,
    description: String,
}

pub async fn create_project(
    State(state): State<ProjectState>,
    Json(payload): Json<CreateProjectRequest>,
) -> impl IntoResponse {
    // 1. Create Model in Inventory
    let model = match state.inventory.create_model(&payload.name, &payload.description) {
        Ok(m) => m,
        Err(e) => return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // 2. Create Sequence in Inventory
    let _seq = match state.inventory.create_sequence(model.id, "Default Scan") {
        Ok(s) => s,
        Err(e) => return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // 3. Create ScanProject Logic (Filesystem)
    match ScanProject::new(&payload.name, &state.base_dir) {
        Ok(project) => {
            // 4. Load into Director
            state.director.set_project(project.clone()).await;
            
            Json(project).into_response()
        },
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn stop_scan(
    State(state): State<ProjectState>,
) -> impl IntoResponse {
    match state.director.stop_scan().await {
        Ok(_) => Json(serde_json::json!({ "status": "Stopped" })).into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
    }
}
