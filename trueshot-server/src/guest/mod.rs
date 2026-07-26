//! Guest Portal API - Event Management & Crowd-Source Capture (Actix-web)
//!
//! Enables crowd-sourced video capture from guests' phones at events.
//! Also includes SlavePhone mode for server-controlled capture.

pub mod slave;

use crate::auth::{require_admin, require_scope};
use crate::config::AppConfig;
use crate::fs_safety::available_space_bytes;
use actix_web::http::StatusCode;
use actix_web::{delete, get, post, put, web, HttpRequest, HttpResponse, Responder};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use utoipa::ToSchema;
use walkdir::WalkDir;

pub use slave::SlavePhoneState;

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Event {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub status: EventStatus,
    pub config: EventConfig,
    pub stats: EventStats,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum EventStatus {
    Draft,
    Active,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EventConfig {
    pub collect_email: bool,
    pub allow_local_save: bool,
    pub max_recording_duration: u32,
    pub preferred_quality: String,
    pub sync_enabled: bool,
}

impl Default for EventConfig {
    fn default() -> Self {
        Self {
            collect_email: true,
            allow_local_save: true,
            max_recording_duration: 600,
            preferred_quality: "1080p".to_string(),
            sync_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct EventStats {
    pub total_guests: u32,
    pub active_guests: u32,
    pub recording_guests: u32,
    pub total_recordings: u32,
    pub total_data_size: u64,
    pub emails_collected: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GuestSession {
    pub id: String,
    pub event_id: String,
    pub device_info: String,
    pub connected_at: DateTime<Utc>,
    pub email: Option<String>,
    pub is_recording: bool,
    pub recording_start: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Recording {
    pub id: String,
    pub event_id: String,
    pub guest_id: String,
    pub started_at: DateTime<Utc>,
    pub duration: u32,
    pub file_size: u64,
    pub file_path: String,
    pub upload_complete: bool,
}

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateEventRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateEventRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<EventStatus>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GuestConnectRequest {
    pub device_info: String,
    pub email: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GuestConnectResponse {
    pub session_id: String,
    pub server_time: DateTime<Utc>,
    pub event: Event,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TimeSyncResponse {
    pub server_time: DateTime<Utc>,
    pub request_received: DateTime<Utc>,
}

// ============================================================================
// State
// ============================================================================

#[derive(Clone)]
pub struct GuestPortalState {
    pub events: Arc<RwLock<HashMap<String, Event>>>,
    pub guests: Arc<RwLock<HashMap<String, GuestSession>>>,
    pub recordings: Arc<RwLock<HashMap<String, Recording>>>,
    pub broadcast_tx: Arc<RwLock<HashMap<String, broadcast::Sender<String>>>>,
    pub upload_dir: String,
    pub max_total_bytes: u64,
    pub min_free_bytes: u64,
}

impl GuestPortalState {
    pub fn new(upload_dir: impl Into<String>, max_total_bytes: u64, min_free_bytes: u64) -> Self {
        Self {
            events: Arc::new(RwLock::new(HashMap::new())),
            guests: Arc::new(RwLock::new(HashMap::new())),
            recordings: Arc::new(RwLock::new(HashMap::new())),
            broadcast_tx: Arc::new(RwLock::new(HashMap::new())),
            upload_dir: upload_dir.into(),
            max_total_bytes,
            min_free_bytes,
        }
    }

    async fn get_broadcast(&self, event_id: &str) -> broadcast::Sender<String> {
        let mut channels = self.broadcast_tx.write().await;
        channels
            .entry(event_id.to_string())
            .or_insert_with(|| broadcast::channel(100).0)
            .clone()
    }
}

// ============================================================================
// API Handlers
// ============================================================================

/// List all events
#[utoipa::path(
    get,
    path = "/api/guest/events",
    tag = "guest",
    responses(
        (status = 200, description = "Event list", body = [Event]),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/guest/events")]
pub async fn list_events(
    http_req: HttpRequest,
    state: web::Data<GuestPortalState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&http_req) {
        return resp;
    }
    let events = state.events.read().await;
    HttpResponse::Ok().json(events.values().collect::<Vec<_>>())
}

/// Create a new event
#[utoipa::path(
    post,
    path = "/api/guest/events",
    tag = "guest",
    request_body = CreateEventRequest,
    responses(
        (status = 201, description = "Event created", body = Event),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/guest/events")]
pub async fn create_event(
    http_req: HttpRequest,
    state: web::Data<GuestPortalState>,
    payload: web::Json<CreateEventRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&http_req) {
        return resp;
    }
    let event = Event {
        id: uuid::Uuid::new_v4().to_string(),
        name: payload.name.clone(),
        description: payload.description.clone().unwrap_or_default(),
        created_at: Utc::now(),
        status: EventStatus::Draft,
        config: EventConfig::default(),
        stats: EventStats::default(),
    };

    let event_dir = Path::new(&state.upload_dir).join(&event.id);
    if let Err(e) = tokio::fs::create_dir_all(&event_dir).await {
        tracing::warn!("Failed to create guest upload dir {:?}: {}", event_dir, e);
    }

    let mut events = state.events.write().await;
    events.insert(event.id.clone(), event.clone());

    HttpResponse::Created().json(event)
}

/// Get single event
#[utoipa::path(
    get,
    path = "/api/guest/events/{event_id}",
    tag = "guest",
    params(("event_id" = String, Path, description = "Event id")),
    responses(
        (status = 200, description = "Event detail", body = Event),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/guest/events/{event_id}")]
pub async fn get_event(
    http_req: HttpRequest,
    state: web::Data<GuestPortalState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&http_req) {
        return resp;
    }
    let event_id = path.into_inner();
    let events = state.events.read().await;

    match events.get(&event_id) {
        Some(event) => HttpResponse::Ok().json(event),
        None => HttpResponse::NotFound().body("Event not found"),
    }
}

/// Update event
#[utoipa::path(
    put,
    path = "/api/guest/events/{event_id}",
    tag = "guest",
    params(("event_id" = String, Path, description = "Event id")),
    request_body = UpdateEventRequest,
    responses(
        (status = 200, description = "Event updated", body = Event),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[put("/api/guest/events/{event_id}")]
pub async fn update_event(
    http_req: HttpRequest,
    state: web::Data<GuestPortalState>,
    path: web::Path<String>,
    payload: web::Json<UpdateEventRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&http_req) {
        return resp;
    }
    let event_id = path.into_inner();
    let mut events = state.events.write().await;

    match events.get_mut(&event_id) {
        Some(event) => {
            if let Some(name) = &payload.name {
                event.name = name.clone();
            }
            if let Some(desc) = &payload.description {
                event.description = desc.clone();
            }
            if let Some(status) = &payload.status {
                event.status = *status;
            }
            HttpResponse::Ok().json(event.clone())
        }
        None => HttpResponse::NotFound().body("Event not found"),
    }
}

/// Delete event
#[utoipa::path(
    delete,
    path = "/api/guest/events/{event_id}",
    tag = "guest",
    params(("event_id" = String, Path, description = "Event id")),
    responses(
        (status = 200, description = "Event deleted", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[delete("/api/guest/events/{event_id}")]
pub async fn delete_event(
    http_req: HttpRequest,
    state: web::Data<GuestPortalState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&http_req) {
        return resp;
    }
    let event_id = path.into_inner();
    let mut events = state.events.write().await;

    if events.remove(&event_id).is_some() {
        HttpResponse::NoContent().finish()
    } else {
        HttpResponse::NotFound().body("Event not found")
    }
}

/// Activate event
#[utoipa::path(
    post,
    path = "/api/guest/events/{event_id}/activate",
    tag = "guest",
    params(("event_id" = String, Path, description = "Event id")),
    responses(
        (status = 200, description = "Event activated", body = Event),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/guest/events/{event_id}/activate")]
pub async fn activate_event(
    http_req: HttpRequest,
    state: web::Data<GuestPortalState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&http_req) {
        return resp;
    }
    let event_id = path.into_inner();
    let mut events = state.events.write().await;

    match events.get_mut(&event_id) {
        Some(event) => {
            event.status = EventStatus::Active;
            HttpResponse::Ok().json(event.clone())
        }
        None => HttpResponse::NotFound().body("Event not found"),
    }
}

/// Guest connect
#[utoipa::path(
    post,
    path = "/api/guest/events/{event_id}/connect",
    tag = "guest",
    params(("event_id" = String, Path, description = "Event id")),
    request_body = GuestConnectRequest,
    responses(
        (status = 200, description = "Guest connected", body = GuestConnectResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/guest/events/{event_id}/connect")]
pub async fn guest_connect(
    http_req: HttpRequest,
    state: web::Data<GuestPortalState>,
    path: web::Path<String>,
    payload: web::Json<GuestConnectRequest>,
) -> impl Responder {
    if let Err(resp) = require_scope(&http_req, "guest:connect") {
        return resp;
    }
    let event_id = path.into_inner();

    // Check event exists
    let events = state.events.read().await;
    let event = match events.get(&event_id) {
        Some(e) if e.status == EventStatus::Active => e.clone(),
        Some(_) => return HttpResponse::Forbidden().body("Event not active"),
        None => return HttpResponse::NotFound().body("Event not found"),
    };
    drop(events);

    let session = GuestSession {
        id: uuid::Uuid::new_v4().to_string(),
        event_id: event_id.clone(),
        device_info: payload.device_info.clone(),
        connected_at: Utc::now(),
        email: payload.email.clone(),
        is_recording: false,
        recording_start: None,
    };

    let mut guests = state.guests.write().await;
    guests.insert(session.id.clone(), session.clone());

    HttpResponse::Ok().json(GuestConnectResponse {
        session_id: session.id,
        server_time: Utc::now(),
        event,
    })
}

/// List guests for event
#[utoipa::path(
    get,
    path = "/api/guest/events/{event_id}/guests",
    tag = "guest",
    params(("event_id" = String, Path, description = "Event id")),
    responses(
        (status = 200, description = "Guest list", body = [GuestSession]),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/guest/events/{event_id}/guests")]
pub async fn list_guests(
    req: HttpRequest,
    state: web::Data<GuestPortalState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let event_id = path.into_inner();
    let guests = state.guests.read().await;

    let event_guests: Vec<_> = guests
        .values()
        .filter(|g| g.event_id == event_id)
        .cloned()
        .collect();

    HttpResponse::Ok().json(event_guests)
}

/// Time sync
#[utoipa::path(
    get,
    path = "/api/guest/events/{event_id}/sync",
    tag = "guest",
    params(("event_id" = String, Path, description = "Event id")),
    responses(
        (status = 200, description = "Time sync", body = TimeSyncResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/guest/events/{event_id}/sync")]
pub async fn time_sync(req: HttpRequest) -> impl Responder {
    if let Err(resp) = require_scope(&req, "guest:connect") {
        return resp;
    }
    let now = Utc::now();
    HttpResponse::Ok().json(TimeSyncResponse {
        server_time: now,
        request_received: now,
    })
}

/// Start all recording
#[utoipa::path(
    post,
    path = "/api/guest/events/{event_id}/recording/start",
    tag = "guest",
    params(("event_id" = String, Path, description = "Event id")),
    responses(
        (status = 200, description = "Recording started", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/guest/events/{event_id}/recording/start")]
pub async fn start_all_recording(
    req: HttpRequest,
    state: web::Data<GuestPortalState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let event_id = path.into_inner();
    let event_dir = Path::new(&state.upload_dir).join(&event_id);
    let current_size = dir_size_bytes(&event_dir);
    if state.max_total_bytes > 0 && current_size >= state.max_total_bytes {
        return HttpResponse::PayloadTooLarge().body("Event storage quota exceeded");
    }
    if state.min_free_bytes > 0 {
        if let Some(available) = available_space_bytes(&event_dir) {
            if available < state.min_free_bytes {
                return HttpResponse::build(StatusCode::INSUFFICIENT_STORAGE)
                    .body("Insufficient disk space");
            }
        }
    }
    let sender = state.get_broadcast(&event_id).await;
    let _ = sender.send(r#"{"type":"recording_command","action":"start"}"#.to_string());
    HttpResponse::Ok().body("Recording started")
}

/// Stop all recording
#[utoipa::path(
    post,
    path = "/api/guest/events/{event_id}/recording/stop",
    tag = "guest",
    params(("event_id" = String, Path, description = "Event id")),
    responses(
        (status = 200, description = "Recording stopped", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/guest/events/{event_id}/recording/stop")]
pub async fn stop_all_recording(
    req: HttpRequest,
    state: web::Data<GuestPortalState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let event_id = path.into_inner();
    let sender = state.get_broadcast(&event_id).await;
    let _ = sender.send(r#"{"type":"recording_command","action":"stop"}"#.to_string());
    HttpResponse::Ok().body("Recording stopped")
}

/// List recordings
#[utoipa::path(
    get,
    path = "/api/guest/events/{event_id}/recordings",
    tag = "guest",
    params(("event_id" = String, Path, description = "Event id")),
    responses(
        (status = 200, description = "Recordings list", body = [Recording]),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/guest/events/{event_id}/recordings")]
pub async fn list_recordings(
    req: HttpRequest,
    state: web::Data<GuestPortalState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let event_id = path.into_inner();
    let recordings = state.recordings.read().await;

    let event_recordings: Vec<_> = recordings
        .values()
        .filter(|r| r.event_id == event_id)
        .cloned()
        .collect();

    HttpResponse::Ok().json(event_recordings)
}

/// Configure guest portal routes
pub fn configure(cfg: &mut web::ServiceConfig) {
    let config = AppConfig::load().expect("Failed to load config");
    let max_total_bytes = config
        .server
        .max_project_bytes
        .unwrap_or(100 * 1024 * 1024 * 1024);
    let min_free_bytes = config
        .server
        .min_free_bytes
        .unwrap_or(2 * 1024 * 1024 * 1024);
    let state = web::Data::new(GuestPortalState::new(
        "./uploads/guest",
        max_total_bytes,
        min_free_bytes,
    ));

    cfg.app_data(state)
        .service(list_events)
        .service(create_event)
        .service(get_event)
        .service(update_event)
        .service(delete_event)
        .service(activate_event)
        .service(guest_connect)
        .service(list_guests)
        .service(time_sync)
        .service(start_all_recording)
        .service(stop_all_recording)
        .service(list_recordings);
}

fn dir_size_bytes(root: &Path) -> u64 {
    if !root.exists() {
        return 0;
    }
    let mut total = 0u64;
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if entry.file_type().is_file() {
            if let Ok(meta) = entry.metadata() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}
