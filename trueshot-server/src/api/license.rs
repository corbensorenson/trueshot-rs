use std::collections::BTreeMap;

use actix_web::{get, post, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::{require_admin, require_guest_or_admin, AuthContext};
use crate::audit::AuditEvent;
use crate::licensing::{bundle_catalog, tier_catalog, sync_trial_env, LicenseSnapshot};
use crate::state::AppState;

#[derive(Debug, Serialize, ToSchema)]
pub struct LicenseStatusResponse {
    pub status: String,
    pub license_valid: bool,
    pub tier: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub device_hash: Option<String>,
    pub init_error: Option<String>,
    pub verification_error: Option<String>,
    pub features: BTreeMap<String, bool>,
    pub bundles: BTreeMap<String, bool>,
    pub trial_active: bool,
    pub trial_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub trial_days_remaining: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LicenseEntitlementsResponse {
    pub status: String,
    pub license_valid: bool,
    pub tier: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub features: BTreeMap<String, bool>,
    pub bundles: BTreeMap<String, bool>,
    pub trial_available: bool,
    pub trial_reason: Option<String>,
    pub trial_active: bool,
    pub trial_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub trial_days_remaining: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LicenseBundleInfo {
    pub key: String,
    pub name: String,
    pub description: String,
    pub features: Vec<String>,
    pub price_usd: u32,
    pub billing: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LicenseTierInfo {
    pub key: String,
    pub name: String,
    pub max_devices: u32,
    pub price_usd: u32,
    pub billing: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LicenseDeviceInfo {
    pub fingerprint_hash: String,
    pub device_name: String,
    pub activated_at: chrono::DateTime<chrono::Utc>,
    pub last_seen: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TrialRequest {
    pub duration_days: Option<i64>,
    pub bundles: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ImportLicenseRequest {
    pub license_json: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ActivateDeviceRequest {
    pub device_name: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ActivateKeyRequest {
    pub license_key: String,
    pub device_name: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DeactivateDeviceRequest {
    pub fingerprint_hash: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LicenseActionResponse {
    pub ok: bool,
    pub message: String,
    pub status: LicenseStatusResponse,
}

#[utoipa::path(
    get,
    path = "/api/license/status",
    tag = "auth",
    responses(
        (status = 200, description = "License status", body = LicenseStatusResponse),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/license/status")]
pub async fn get_license_status(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let mut gate = state.license_gate.lock().unwrap();
    HttpResponse::Ok().json(snapshot_to_response(gate.status_snapshot()))
}

#[utoipa::path(
    get,
    path = "/api/license/bundles",
    tag = "auth",
    responses(
        (status = 200, description = "Available add-on bundles", body = [LicenseBundleInfo]),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/license/bundles")]
pub async fn get_license_bundles(req: HttpRequest) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let bundles = bundle_catalog()
        .into_iter()
        .map(|bundle| LicenseBundleInfo {
            key: bundle.key,
            name: bundle.name,
            description: bundle.description,
            features: bundle.features,
            price_usd: bundle.price_usd,
            billing: bundle.billing,
        })
        .collect::<Vec<_>>();
    HttpResponse::Ok().json(bundles)
}

#[utoipa::path(
    get,
    path = "/api/license/entitlements",
    tag = "auth",
    responses(
        (status = 200, description = "License entitlements", body = LicenseEntitlementsResponse),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/license/entitlements")]
pub async fn get_license_entitlements(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_guest_or_admin(&req) {
        return resp;
    }
    let snapshot = {
        let mut gate = state.license_gate.lock().unwrap();
        gate.status_snapshot()
    };
    sync_trial_env(&snapshot);
    let subject = req
        .extensions()
        .get::<AuthContext>()
        .map(|ctx| ctx.sub.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let trial_key_subject = format!("trial_self_issued:subject:{subject}");
    let trial_key_device = snapshot
        .device_hash
        .as_ref()
        .map(|hash| format!("trial_self_issued:device:{hash}"));

    let trial_subject_mark = state
        .auth
        .get_setting(&trial_key_subject)
        .await
        .unwrap_or(None);
    let trial_device_mark = match trial_key_device.as_ref() {
        Some(key) => state.auth.get_setting(key).await.unwrap_or(None),
        None => None,
    };

    let mut trial_available = true;
    let mut trial_reason = None;

    if snapshot.license_valid {
        trial_available = false;
        trial_reason = Some("license_active".to_string());
    } else if snapshot.status == "unavailable" {
        trial_available = false;
        trial_reason = Some("license_unavailable".to_string());
    } else if !trial_issuer_enabled() {
        trial_available = false;
        trial_reason = Some("trial_issuer_disabled".to_string());
    } else if trial_subject_mark.is_some() || trial_device_mark.is_some() {
        trial_available = false;
        trial_reason = Some("trial_already_used".to_string());
    }

    HttpResponse::Ok().json(LicenseEntitlementsResponse {
        status: snapshot.status,
        license_valid: snapshot.license_valid,
        tier: snapshot.tier,
        expires_at: snapshot.expires_at,
        features: snapshot.features,
        bundles: snapshot.bundles,
        trial_available,
        trial_reason,
        trial_active: snapshot.trial_active,
        trial_expires_at: snapshot.trial_expires_at,
        trial_days_remaining: snapshot.trial_days_remaining,
    })
}

#[utoipa::path(
    get,
    path = "/api/license/catalog",
    tag = "auth",
    responses(
        (status = 200, description = "Bundle catalog with pricing", body = [LicenseBundleInfo]),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/license/catalog")]
pub async fn get_license_catalog(req: HttpRequest) -> impl Responder {
    if let Err(resp) = require_guest_or_admin(&req) {
        return resp;
    }
    let bundles = bundle_catalog()
        .into_iter()
        .map(|bundle| LicenseBundleInfo {
            key: bundle.key,
            name: bundle.name,
            description: bundle.description,
            features: bundle.features,
            price_usd: bundle.price_usd,
            billing: bundle.billing,
        })
        .collect::<Vec<_>>();
    HttpResponse::Ok().json(bundles)
}

#[utoipa::path(
    get,
    path = "/api/license/devices",
    tag = "auth",
    responses(
        (status = 200, description = "Activated devices", body = [LicenseDeviceInfo]),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/license/devices")]
pub async fn get_license_devices(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let mut gate = state.license_gate.lock().unwrap();
    match gate.list_activated_devices() {
        Ok(devices) => HttpResponse::Ok().json(
            devices
                .into_iter()
                .map(|device| LicenseDeviceInfo {
                    fingerprint_hash: device.fingerprint_hash,
                    device_name: device.device_name,
                    activated_at: device.activated_at,
                    last_seen: device.last_seen,
                })
                .collect::<Vec<_>>(),
        ),
        Err(err) => HttpResponse::BadRequest().json(serde_json::json!({
            "ok": false,
            "error": err,
        })),
    }
}

#[utoipa::path(
    post,
    path = "/api/license/activate",
    tag = "auth",
    request_body = ActivateDeviceRequest,
    responses(
        (status = 200, description = "Device activated", body = LicenseActionResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/license/activate")]
pub async fn activate_license_device(
    req: HttpRequest,
    state: web::Data<AppState>,
    payload: web::Json<ActivateDeviceRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let mut gate = state.license_gate.lock().unwrap();
    match gate.activate_current_device(payload.device_name.clone()) {
        Ok(snapshot) => {
            let (actor, role, ip) = audit_actor(&req);
            log_audit(
                &req,
                &state,
                AuditEvent::new(
                    actor,
                    role,
                    "license.activate_device",
                    "license/device",
                    "success",
                    ip,
                    serde_json::json!({}),
                ),
            );
            HttpResponse::Ok().json(LicenseActionResponse {
                ok: true,
                message: "Device activated".to_string(),
                status: snapshot_to_response(snapshot),
            })
        }
        Err(err) => HttpResponse::BadRequest().json(serde_json::json!({
            "ok": false,
            "error": err,
        })),
    }
}

#[utoipa::path(
    post,
    path = "/api/license/activate-key",
    tag = "auth",
    request_body = ActivateKeyRequest,
    responses(
        (status = 200, description = "License activated from key", body = LicenseActionResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/license/activate-key")]
pub async fn activate_license_key(
    req: HttpRequest,
    state: web::Data<AppState>,
    payload: web::Json<ActivateKeyRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let mut gate = state.license_gate.lock().unwrap();
    match gate.activate_with_key(&payload.license_key, payload.device_name.clone()) {
        Ok(snapshot) => {
            let (actor, role, ip) = audit_actor(&req);
            log_audit(
                &req,
                &state,
                AuditEvent::new(
                    actor,
                    role,
                    "license.activate_key",
                    "license",
                    "success",
                    ip,
                    serde_json::json!({}),
                ),
            );
            HttpResponse::Ok().json(LicenseActionResponse {
                ok: true,
                message: "License activated".to_string(),
                status: snapshot_to_response(snapshot),
            })
        }
        Err(err) => HttpResponse::BadRequest().json(serde_json::json!({
            "ok": false,
            "error": err,
        })),
    }
}

#[utoipa::path(
    post,
    path = "/api/license/deactivate",
    tag = "auth",
    request_body = DeactivateDeviceRequest,
    responses(
        (status = 200, description = "Device deactivated", body = LicenseActionResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/license/deactivate")]
pub async fn deactivate_license_device(
    req: HttpRequest,
    state: web::Data<AppState>,
    payload: web::Json<DeactivateDeviceRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let mut gate = state.license_gate.lock().unwrap();
    match gate.deactivate_device(&payload.fingerprint_hash) {
        Ok(snapshot) => {
            let (actor, role, ip) = audit_actor(&req);
            log_audit(
                &req,
                &state,
                AuditEvent::new(
                    actor,
                    role,
                    "license.deactivate_device",
                    "license/device",
                    "success",
                    ip,
                    serde_json::json!({ "fingerprint_hash": payload.fingerprint_hash }),
                ),
            );
            HttpResponse::Ok().json(LicenseActionResponse {
                ok: true,
                message: "Device deactivated".to_string(),
                status: snapshot_to_response(snapshot),
            })
        }
        Err(err) => HttpResponse::BadRequest().json(serde_json::json!({
            "ok": false,
            "error": err,
        })),
    }
}

#[utoipa::path(
    get,
    path = "/api/license/tiers",
    tag = "auth",
    responses(
        (status = 200, description = "Core license tiers", body = [LicenseTierInfo]),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/license/tiers")]
pub async fn get_license_tiers(req: HttpRequest) -> impl Responder {
    if let Err(resp) = require_guest_or_admin(&req) {
        return resp;
    }
    let tiers = tier_catalog()
        .into_iter()
        .map(|tier| LicenseTierInfo {
            key: tier.key,
            name: tier.name,
            max_devices: tier.max_devices,
            price_usd: tier.price_usd,
            billing: tier.billing,
        })
        .collect::<Vec<_>>();
    HttpResponse::Ok().json(tiers)
}

#[utoipa::path(
    post,
    path = "/api/license/trial",
    tag = "auth",
    request_body = TrialRequest,
    responses(
        (status = 200, description = "Trial created", body = LicenseActionResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/license/trial")]
pub async fn create_license_trial(
    req: HttpRequest,
    state: web::Data<AppState>,
    payload: web::Json<TrialRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let duration_days = payload.duration_days.unwrap_or(14).clamp(1, 90);
    let bundles = payload.bundles.clone().unwrap_or_default();
    let mut gate = state.license_gate.lock().unwrap();
    match gate.create_trial(duration_days, &bundles) {
        Ok(snapshot) => HttpResponse::Ok().json(LicenseActionResponse {
            ok: true,
            message: format!("Trial created for {} day(s)", duration_days),
            status: snapshot_to_response(snapshot),
        }),
        Err(err) => HttpResponse::BadRequest().json(serde_json::json!({
            "ok": false,
            "error": err,
        })),
    }
}

#[utoipa::path(
    post,
    path = "/api/license/trial/self",
    tag = "auth",
    request_body = TrialRequest,
    responses(
        (status = 200, description = "Trial created", body = LicenseActionResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 429, description = "Trial already used")
    )
)]
#[post("/api/license/trial/self")]
pub async fn create_license_trial_self(
    req: HttpRequest,
    state: web::Data<AppState>,
    payload: web::Json<TrialRequest>,
) -> impl Responder {
    if let Err(resp) = require_guest_or_admin(&req) {
        return resp;
    }
    let snapshot = {
        let mut gate = state.license_gate.lock().unwrap();
        gate.status_snapshot()
    };
    if snapshot.license_valid {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "ok": false,
            "error": "license_active",
            "message": "A valid license is already active for this device."
        }));
    }
    if !trial_issuer_enabled() {
        return HttpResponse::Forbidden().json(serde_json::json!({
            "ok": false,
            "error": "trial_issuer_disabled",
            "message": "Trial issuance is disabled on this server."
        }));
    }

    let subject = req
        .extensions()
        .get::<AuthContext>()
        .map(|ctx| ctx.sub.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let trial_key_subject = format!("trial_self_issued:subject:{subject}");
    let trial_key_device = snapshot
        .device_hash
        .as_ref()
        .map(|hash| format!("trial_self_issued:device:{hash}"));

    let trial_subject_mark = state
        .auth
        .get_setting(&trial_key_subject)
        .await
        .unwrap_or(None);
    let trial_device_mark = match trial_key_device.as_ref() {
        Some(key) => state.auth.get_setting(key).await.unwrap_or(None),
        None => None,
    };
    if trial_subject_mark.is_some() || trial_device_mark.is_some() {
        return HttpResponse::TooManyRequests().json(serde_json::json!({
            "ok": false,
            "error": "trial_already_used",
            "message": "A trial has already been issued for this user or device."
        }));
    }

    let duration_days = payload.duration_days.unwrap_or(14).clamp(1, 30);
    let bundles = payload.bundles.clone().unwrap_or_default();
    let mut gate = state.license_gate.lock().unwrap();
    match gate.create_trial(duration_days, &bundles) {
        Ok(snapshot) => {
            let issued_at = chrono::Utc::now().to_rfc3339();
            let payload = serde_json::json!({
                "issued_at": issued_at,
                "subject": subject,
                "bundles": bundles,
            });
            let _ = state.auth.set_setting(&trial_key_subject, &payload.to_string()).await;
            if let Some(key) = trial_key_device {
                let _ = state.auth.set_setting(&key, &payload.to_string()).await;
            }
            HttpResponse::Ok().json(LicenseActionResponse {
                ok: true,
                message: format!("Trial created for {} day(s)", duration_days),
                status: snapshot_to_response(snapshot),
            })
        }
        Err(err) => HttpResponse::BadRequest().json(serde_json::json!({
            "ok": false,
            "error": err,
        })),
    }
}

#[utoipa::path(
    post,
    path = "/api/license/import",
    tag = "auth",
    request_body = ImportLicenseRequest,
    responses(
        (status = 200, description = "License imported", body = LicenseActionResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/license/import")]
pub async fn import_license(
    req: HttpRequest,
    state: web::Data<AppState>,
    payload: web::Json<ImportLicenseRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let mut gate = state.license_gate.lock().unwrap();
    match gate.import_license_json(&payload.license_json) {
        Ok(()) => {
            let (actor, role, ip) = audit_actor(&req);
            log_audit(
                &req,
                &state,
                AuditEvent::new(
                    actor,
                    role,
                    "license.import",
                    "license",
                    "success",
                    ip,
                    serde_json::json!({}),
                ),
            );
            HttpResponse::Ok().json(LicenseActionResponse {
                ok: true,
                message: "License imported".to_string(),
                status: snapshot_to_response(gate.status_snapshot()),
            })
        }
        Err(err) => HttpResponse::BadRequest().json(serde_json::json!({
            "ok": false,
            "error": err,
        })),
    }
}

fn snapshot_to_response(snapshot: LicenseSnapshot) -> LicenseStatusResponse {
    sync_trial_env(&snapshot);
    LicenseStatusResponse {
        status: snapshot.status,
        license_valid: snapshot.license_valid,
        tier: snapshot.tier,
        expires_at: snapshot.expires_at,
        device_hash: snapshot.device_hash,
        init_error: snapshot.init_error,
        verification_error: snapshot.verification_error,
        features: snapshot.features,
        bundles: snapshot.bundles,
        trial_active: snapshot.trial_active,
        trial_expires_at: snapshot.trial_expires_at,
        trial_days_remaining: snapshot.trial_days_remaining,
    }
}

fn trial_issuer_enabled() -> bool {
    let value = std::env::var("TRUESHOT_LICENSE_ENABLE_LOCAL_TRIAL_ISSUER")
        .unwrap_or_default()
        .to_lowercase();
    matches!(value.as_str(), "1" | "true" | "yes" | "on")
        || std::env::var("TRUESHOT_LICENSE_DEV_MODE").is_ok()
}

fn audit_actor(req: &HttpRequest) -> (String, String, Option<String>) {
    let (actor, role) = match req.extensions().get::<AuthContext>() {
        Some(ctx) => (ctx.sub.clone(), format!("{:?}", ctx.role)),
        None => ("unknown".to_string(), "unknown".to_string()),
    };
    let ip = req.peer_addr().map(|p| p.ip().to_string());
    (actor, role, ip)
}

fn log_audit(req: &HttpRequest, state: &web::Data<AppState>, event: AuditEvent) {
    if let Err(err) = state.audit.append_with_redaction(event, &state.config.privacy) {
        tracing::warn!("audit log failed for {}: {}", req.path(), err);
    }
}
