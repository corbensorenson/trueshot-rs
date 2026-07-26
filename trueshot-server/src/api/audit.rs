use actix_web::{get, web, HttpRequest, HttpResponse, Responder};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::auth::require_admin;
use crate::state::AppState;

#[derive(Debug, Deserialize, IntoParams)]
pub struct AuditQuery {
    pub limit: Option<usize>,
    pub verify: Option<bool>,
    pub verify_anchor: Option<bool>,
}

#[utoipa::path(
    get,
    path = "/api/audit",
    tag = "audit",
    params(AuditQuery),
    responses(
        (status = 200, description = "Audit records", body = serde_json::Value),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/audit")]
pub async fn get_audit(
    req: HttpRequest,
    state: web::Data<AppState>,
    query: web::Query<AuditQuery>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let limit = query.limit.unwrap_or(200).min(5000);
    let records = match state.audit.read(limit) {
        Ok(records) => records,
        Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
    };

    let integrity_ok = if query.verify.unwrap_or(false) {
        match state.audit.verify() {
            Ok(ok) => ok,
            Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
        }
    } else {
        true
    };

    let anchor_verification = if query.verify_anchor.unwrap_or(false) {
        match state.audit.verify_anchor() {
            Ok(result) => Some(result),
            Err(err) => return HttpResponse::InternalServerError().body(err.to_string()),
        }
    } else {
        None
    };

    HttpResponse::Ok().json(serde_json::json!({
        "records": records,
        "integrity_ok": integrity_ok,
        "anchor": anchor_verification,
        "path": state.audit.path().display().to_string(),
    }))
}
