use crate::auth::{require_guest_or_admin, require_scope};
use crate::state::AppState;
use actix_web::{get, web, HttpRequest, HttpResponse, Responder};

/// Get system resource statistics
#[utoipa::path(
    get,
    path = "/api/system/stats",
    tag = "system",
    responses(
        (status = 200, description = "System stats", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/system/stats")]
pub async fn get_system_stats(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_guest_or_admin(&req) {
        return resp;
    }
    if let Err(resp) = require_scope(&req, "system:read") {
        return resp;
    }
    let stats = state.system_stats.lock().unwrap();
    HttpResponse::Ok().json(serde_json::json!({
        "cpu_usage": stats.cpu_usage,
        "memory_used_mb": stats.memory_used_mb,
        "memory_total_mb": stats.memory_total_mb,
        "disk_free_gb": stats.disk_free_gb
    }))
}
