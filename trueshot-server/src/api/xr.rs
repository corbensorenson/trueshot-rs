use actix_web::{post, web, HttpRequest, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::audit::AuditEvent;
use crate::auth::require_admin;
use crate::licensing::require_license_feature;
use trueshot_core::licensing::Feature;
use crate::state::AppState;

#[derive(Debug, Deserialize, ToSchema)]
pub struct XrSessionStartRequest {
    pub mode: String,
    pub device: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct XrSessionStartResponse {
    pub session_id: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct XrSessionCompleteRequest {
    pub session_id: String,
    pub mode: String,
    pub frame_count: usize,
    pub duration_seconds: Option<f32>,
}

#[utoipa::path(
    post,
    path = "/api/xr/session/start",
    tag = "xr",
    request_body = XrSessionStartRequest,
    responses(
        (status = 200, description = "XR session started", body = XrSessionStartResponse),
        (status = 401, description = "Unauthorized"),
        (status = 402, description = "Feature not entitled")
    )
)]
#[post("/api/xr/session/start")]
pub async fn start_xr_session(
    req: HttpRequest,
    json: web::Json<XrSessionStartRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) = require_license_feature(&state, Feature::WebXRScanning, "xr_scanning") {
        return resp;
    }

    let session_id = Uuid::new_v4().to_string();

    let (actor, role, ip) = audit_actor(&req);
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            actor,
            role,
            "xr_session_start",
            format!("xr/session/{}", session_id),
            "ok",
            ip,
            serde_json::json!({
                "mode": json.mode,
                "device": json.device,
                "notes": json.notes,
            }),
        ),
    );

    HttpResponse::Ok().json(XrSessionStartResponse { session_id })
}

#[utoipa::path(
    post,
    path = "/api/xr/session/complete",
    tag = "xr",
    request_body = XrSessionCompleteRequest,
    responses(
        (status = 200, description = "XR session completed", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 402, description = "Feature not entitled")
    )
)]
#[post("/api/xr/session/complete")]
pub async fn complete_xr_session(
    req: HttpRequest,
    json: web::Json<XrSessionCompleteRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) = require_license_feature(&state, Feature::WebXRScanning, "xr_scanning") {
        return resp;
    }

    let (actor, role, ip) = audit_actor(&req);
    log_audit(
        &req,
        &state,
        AuditEvent::new(
            actor,
            role,
            "xr_session_complete",
            format!("xr/session/{}", json.session_id),
            "ok",
            ip,
            serde_json::json!({
                "mode": json.mode,
                "frame_count": json.frame_count,
                "duration_seconds": json.duration_seconds,
            }),
        ),
    );

    HttpResponse::Ok().json(serde_json::json!({
        "ok": true,
        "session_id": json.session_id,
    }))
}

fn audit_actor(req: &HttpRequest) -> (String, String, Option<String>) {
    let (actor, role) = req.extensions().get::<crate::auth::AuthClaims>()
        .map(|claims| (claims.subject.clone(), format!("{:?}", claims.role)))
        .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));
    let ip = req.peer_addr().map(|p| p.ip().to_string());
    (actor, role, ip)
}

fn log_audit(req: &HttpRequest, state: &web::Data<AppState>, event: AuditEvent) {
    if let Err(err) = state.audit.append_with_redaction(event, &state.config.privacy) {
        tracing::warn!("audit log failed for {}: {}", req.path(), err);
    }
}
