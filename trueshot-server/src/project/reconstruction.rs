use axum::{
    extract::{State, Path},
    response::IntoResponse,
    Json,
};
use std::path::PathBuf;
use std::sync::Arc;
use trueshot_core::project::ScanProject;
use serde::Deserialize;
use super::handlers::ProjectState;
use crate::at_rest::{ProjectKeyStore, decrypt_file_in_place};

pub async fn get_reconstruction(
    State(state): State<ProjectState>,
    Path(project_name): Path<String>,
) -> impl IntoResponse {
    // 1. Find Model/Sequence with this name to get ID (Naive match)
    // Actually our DB tracks Models and Sequences. ScanProject is a wrapper.
    // Let's assume project_name IS the directory name for simplicity in this version.
    
    // Security check: No parent traversal
    if project_name.contains("..") {
        return (axum::http::StatusCode::BAD_REQUEST, "Invalid project path").into_response();
    }
    
    let path = state.base_dir.join(&project_name).join("processed/sfm/dense.ply");
    
    let mut resolved = path.clone();
    if !resolved.exists() {
        let enc_path = PathBuf::from(format!("{}.enc", resolved.display()));
        if enc_path.exists() {
            let key_store = ProjectKeyStore::new(&state.base_dir);
            if let Ok(key) = key_store.load_or_create(&project_name) {
                if let Ok(restored) = decrypt_file_in_place(&enc_path, &key) {
                    resolved = restored;
                }
            }
        }
    }

    if resolved.exists() {
         // In prod we should stream this file or redirect to static handler
         // For API simplicity, we return the relative path that the static handler serves
         
         // Static mount is at /projects
         let clean_path = format!("/projects/{}/processed/sfm/dense.ply", project_name);
         Json(serde_json::json!({ "url": clean_path })).into_response()
    } else {
        (axum::http::StatusCode::NOT_FOUND, "Reconstruction not found").into_response()
    }
}
