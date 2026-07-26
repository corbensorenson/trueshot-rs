//! OpenAPI Documentation - Generated from route annotations.
//!
//! Provides API documentation at /api/docs

use actix_web::{get, HttpResponse, Responder};
use utoipa::OpenApi;

use crate::api_doc::ApiDoc;

/// Get OpenAPI specification as JSON
#[utoipa::path(
    get,
    path = "/api/docs",
    tag = "docs",
    responses(
        (status = 200, description = "OpenAPI spec", body = serde_json::Value)
    )
)]
#[get("/api/docs")]
pub async fn get_api_docs() -> impl Responder {
    let spec = ApiDoc::openapi();
    let json = serde_json::to_string(&spec).unwrap_or_else(|_| "{}".to_string());
    HttpResponse::Ok()
        .content_type("application/json")
        .body(json)
}
