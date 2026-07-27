//! Health Check Endpoints for Production Monitoring
//!
//! Provides `/health` and `/ready` endpoints for load balancers,
//! container orchestration (K8s), and monitoring systems.

use crate::licensing::lock_license_gate;
use crate::state::AppState;
use actix_web::{web, HttpResponse, Responder};
use serde::Serialize;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime};
use utoipa::ToSchema;

/// Server start time (initialized once)
static START_TIME: OnceLock<Instant> = OnceLock::new();

/// Initialize start time (call at server startup)
pub fn init_start_time() {
    START_TIME.get_or_init(Instant::now);
}

/// Health check response
#[derive(Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub uptime_seconds: u64,
    pub timestamp: u64,
}

/// Readiness check response  
#[derive(Serialize, ToSchema)]
pub struct ReadinessResponse {
    pub status: &'static str,
    pub checks: ReadinessChecks,
}

#[derive(Serialize, ToSchema)]
pub struct ReadinessChecks {
    pub gpu_available: bool,
    pub license_valid: bool,
    pub license_status: String,
    pub storage_accessible: bool,
}

/// GET /health - Basic liveness check
/// Returns 200 if server is running
#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses(
        (status = 200, description = "Health response", body = HealthResponse)
    )
)]
pub async fn health() -> impl Responder {
    let uptime = START_TIME.get().map(|t| t.elapsed().as_secs()).unwrap_or(0);

    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    HttpResponse::Ok().json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds: uptime,
        timestamp,
    })
}

/// GET /ready - Readiness check for load balancers
/// Returns 200 only if all subsystems are operational
#[utoipa::path(
    get,
    path = "/ready",
    tag = "health",
    responses(
        (status = 200, description = "Ready"),
        (status = 503, description = "Not ready", body = ReadinessResponse)
    )
)]
pub async fn ready(state: web::Data<AppState>) -> impl Responder {
    let gpu_available = true; // GPU always assumed available
    let (license_valid, license_status) = {
        let mut gate = match lock_license_gate(&state) {
            Ok(gate) => gate,
            Err(response) => return response,
        };
        let snapshot = gate.status_snapshot();
        (snapshot.license_valid, snapshot.status)
    };
    let storage_accessible = std::env::temp_dir().exists();

    let all_ready = gpu_available && license_valid && storage_accessible;

    let response = ReadinessResponse {
        status: if all_ready { "ready" } else { "not_ready" },
        checks: ReadinessChecks {
            gpu_available,
            license_valid,
            license_status,
            storage_accessible,
        },
    };

    if all_ready {
        HttpResponse::Ok().json(response)
    } else {
        HttpResponse::ServiceUnavailable().json(response)
    }
}

/// Configure health routes
pub fn configure(cfg: &mut web::ServiceConfig) {
    init_start_time();
    cfg.service(
        web::scope("")
            .route("/health", web::get().to(health))
            .route("/ready", web::get().to(ready)),
    );
}
