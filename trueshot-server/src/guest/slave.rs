//! Slave Phone Control Module
//!
//! Server-controlled phone cameras for 3DGS scanning and synchronized capture.
//! Phones connect via WebSocket and wait for capture commands.

use actix_web::{get, post, web, HttpRequest, HttpResponse, Responder};
use actix_ws::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{RwLock, broadcast, mpsc};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use crate::auth::{require_admin, require_scope};
use crate::fs_safety::available_space_bytes;
use utoipa::ToSchema;

// ============================================================================
// Types
// ============================================================================

/// Phone operating mode
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PhoneMode {
    /// Self-controlled, event capture (existing GuestPortal)
    Guest,
    /// Server-controlled, for scanning/3DGS
    Slave,
}

/// Phone session state
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PhoneSession {
    pub id: String,
    pub mode: PhoneMode,
    pub name: String,
    pub device_info: String,
    pub connected_at: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub battery_level: u8,
    pub resolution: (u32, u32),
    pub is_ready: bool,
    pub is_capturing: bool,
    pub capture_count: u32,
    pub last_capture: Option<DateTime<Utc>>,
    /// Phone pose for SfM (yaw, pitch, roll in degrees)
    pub orientation: Option<(f32, f32, f32)>,
}

/// Capture command sent to phones
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CaptureCommand {
    pub capture_id: String,
    pub flash: bool,
    pub countdown_ms: u32,
    pub quality: u8,  // 1-100
    pub resolution: Option<(u32, u32)>,
}

/// Capture result from phone
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CaptureResult {
    pub capture_id: String,
    pub phone_id: String,
    pub timestamp: DateTime<Utc>,
    pub file_size: u64,
    pub resolution: (u32, u32),
    pub orientation: Option<(f32, f32, f32)>,
    pub success: bool,
    pub error: Option<String>,
}

/// WebSocket message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    // Phone → Server
    Register {
        name: String,
        device_info: String,
        resolution: (u32, u32),
        battery: u8,
    },
    Ready {
        ready: bool,
    },
    StatusUpdate {
        battery: u8,
        orientation: Option<(f32, f32, f32)>,
    },
    CaptureComplete {
        capture_id: String,
        timestamp: i64,
        file_size: u64,
        success: bool,
        error: Option<String>,
    },
    
    // Server → Phone
    Capture {
        capture_id: String,
        flash: bool,
        countdown_ms: u32,
        quality: u8,
    },
    SetResolution {
        width: u32,
        height: u32,
    },
    SetFlash {
        enabled: bool,
    },
    StartVideo,
    StopVideo,
    Ping,
    Registered {
        session_id: String,
        server_time: i64,
    },
}

// ============================================================================
// State
// ============================================================================

/// Slave phone controller state
#[derive(Clone)]
pub struct SlavePhoneState {
    pub phones: Arc<RwLock<HashMap<String, PhoneSession>>>,
    pub captures: Arc<RwLock<HashMap<String, Vec<CaptureResult>>>>,
    /// Channel to send commands to connected phones
    pub command_tx: Arc<RwLock<HashMap<String, mpsc::Sender<WsMessage>>>>,
    /// Broadcast for batch commands
    pub broadcast_tx: broadcast::Sender<WsMessage>,
    pub upload_dir: String,
    pub max_upload_bytes: u64,
    pub max_upload_rate_bytes_per_minute: u64,
    pub max_total_bytes: u64,
    pub min_free_bytes: u64,
    pub upload_buckets: Arc<RwLock<HashMap<String, UploadBucket>>>,
}

impl SlavePhoneState {
    pub fn new(
        upload_dir: impl Into<String>,
        max_upload_bytes: u64,
        max_upload_rate_bytes_per_minute: u64,
        max_total_bytes: u64,
        min_free_bytes: u64,
    ) -> Self {
        let (broadcast_tx, _) = broadcast::channel(100);
        Self {
            phones: Arc::new(RwLock::new(HashMap::new())),
            captures: Arc::new(RwLock::new(HashMap::new())),
            command_tx: Arc::new(RwLock::new(HashMap::new())),
            broadcast_tx,
            upload_dir: upload_dir.into(),
            max_upload_bytes,
            max_upload_rate_bytes_per_minute,
            max_total_bytes,
            min_free_bytes,
            upload_buckets: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Send command to specific phone
    pub async fn send_to_phone(&self, phone_id: &str, msg: WsMessage) -> Result<(), String> {
        let channels = self.command_tx.read().await;
        if let Some(tx) = channels.get(phone_id) {
            tx.send(msg).await.map_err(|e| e.to_string())
        } else {
            Err("Phone not connected".to_string())
        }
    }
    
    /// Broadcast command to all phones
    pub fn broadcast(&self, msg: WsMessage) -> Result<usize, String> {
        self.broadcast_tx.send(msg).map_err(|e| e.to_string())
    }
    
    /// Get all ready phones
    pub async fn ready_phones(&self) -> Vec<PhoneSession> {
        let phones = self.phones.read().await;
        phones.values()
            .filter(|p| p.is_ready && p.mode == PhoneMode::Slave)
            .cloned()
            .collect()
    }

    async fn allow_upload(&self, phone_id: &str, size_bytes: u64) -> bool {
        if size_bytes > self.max_upload_bytes {
            return false;
        }
        if self.min_free_bytes > 0 {
            if let Some(available) = available_space_bytes(std::path::Path::new(&self.upload_dir)) {
                if available < self.min_free_bytes.saturating_add(size_bytes) {
                    return false;
                }
            }
        }
        if self.max_total_bytes > 0 {
            let total = dir_size_bytes(std::path::Path::new(&self.upload_dir));
            if total.saturating_add(size_bytes) > self.max_total_bytes {
                return false;
            }
        }
        let mut buckets = self.upload_buckets.write().await;
        let now = Instant::now();
        let bucket = buckets.entry(phone_id.to_string()).or_insert(UploadBucket {
            window_start: now,
            bytes: 0,
        });
        if now.duration_since(bucket.window_start).as_secs() >= 60 {
            bucket.window_start = now;
            bucket.bytes = 0;
        }
        if bucket.bytes.saturating_add(size_bytes) > self.max_upload_rate_bytes_per_minute {
            return false;
        }
        bucket.bytes = bucket.bytes.saturating_add(size_bytes);
        true
    }
}

// ============================================================================
// API Handlers
// ============================================================================

/// List all connected slave phones
#[utoipa::path(
    get,
    path = "/api/phones",
    tag = "phones",
    responses(
        (status = 200, description = "Phone list", body = [PhoneSession]),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/phones")]
pub async fn list_phones(
    req: HttpRequest,
    state: web::Data<SlavePhoneState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let phones = state.phones.read().await;
    let list: Vec<_> = phones.values().cloned().collect();
    HttpResponse::Ok().json(list)
}

/// Get phone by ID
#[utoipa::path(
    get,
    path = "/api/phones/{phone_id}",
    tag = "phones",
    params(("phone_id" = String, Path, description = "Phone id")),
    responses(
        (status = 200, description = "Phone detail", body = PhoneSession),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/phones/{phone_id}")]
pub async fn get_phone(
    req: HttpRequest,
    state: web::Data<SlavePhoneState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let phone_id = path.into_inner();
    let phones = state.phones.read().await;
    
    match phones.get(&phone_id) {
        Some(phone) => HttpResponse::Ok().json(phone),
        None => HttpResponse::NotFound().body("Phone not found"),
    }
}

/// Trigger capture on single phone
#[utoipa::path(
    post,
    path = "/api/phones/{phone_id}/capture",
    tag = "phones",
    params(("phone_id" = String, Path, description = "Phone id")),
    request_body = CaptureCommand,
    responses(
        (status = 200, description = "Capture sent", body = serde_json::Value),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/phones/{phone_id}/capture")]
pub async fn capture_phone(
    req: HttpRequest,
    state: web::Data<SlavePhoneState>,
    path: web::Path<String>,
    body: Option<web::Json<CaptureCommand>>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let phone_id = path.into_inner();
    
    let cmd = body.map(|b| b.into_inner()).unwrap_or_else(|| CaptureCommand {
        capture_id: uuid::Uuid::new_v4().to_string(),
        flash: false,
        countdown_ms: 0,
        quality: 90,
        resolution: None,
    });
    
    let msg = WsMessage::Capture {
        capture_id: cmd.capture_id.clone(),
        flash: cmd.flash,
        countdown_ms: cmd.countdown_ms,
        quality: cmd.quality,
    };
    
    match state.send_to_phone(&phone_id, msg).await {
        Ok(_) => HttpResponse::Ok().json(serde_json::json!({
            "status": "sent",
            "capture_id": cmd.capture_id
        })),
        Err(e) => HttpResponse::BadRequest().body(e),
    }
}

/// Trigger capture on ALL ready phones
#[utoipa::path(
    post,
    path = "/api/phones/capture-all",
    tag = "phones",
    request_body = CaptureCommand,
    responses(
        (status = 200, description = "Capture broadcast", body = serde_json::Value),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/phones/capture-all")]
pub async fn capture_all_phones(
    req: HttpRequest,
    state: web::Data<SlavePhoneState>,
    body: Option<web::Json<CaptureCommand>>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let capture_id = uuid::Uuid::new_v4().to_string();
    
    let cmd = body.map(|b| b.into_inner()).unwrap_or_else(|| CaptureCommand {
        capture_id: capture_id.clone(),
        flash: false,
        countdown_ms: 0,
        quality: 90,
        resolution: None,
    });
    
    let msg = WsMessage::Capture {
        capture_id: cmd.capture_id.clone(),
        flash: cmd.flash,
        countdown_ms: cmd.countdown_ms,
        quality: cmd.quality,
    };
    
    let ready_count = state.ready_phones().await.len();
    
    match state.broadcast(msg) {
        Ok(sent) => HttpResponse::Ok().json(serde_json::json!({
            "status": "broadcast",
            "capture_id": cmd.capture_id,
            "phones_ready": ready_count,
            "messages_sent": sent
        })),
        Err(e) => HttpResponse::InternalServerError().body(e),
    }
}

/// Set resolution for phone
#[utoipa::path(
    post,
    path = "/api/phones/{phone_id}/resolution",
    tag = "phones",
    params(("phone_id" = String, Path, description = "Phone id")),
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Resolution set", body = serde_json::Value),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/phones/{phone_id}/resolution")]
pub async fn set_phone_resolution(
    req: HttpRequest,
    state: web::Data<SlavePhoneState>,
    path: web::Path<String>,
    body: web::Json<(u32, u32)>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let phone_id = path.into_inner();
    let (width, height) = body.into_inner();
    
    let msg = WsMessage::SetResolution { width, height };
    
    match state.send_to_phone(&phone_id, msg).await {
        Ok(_) => HttpResponse::Ok().json(serde_json::json!({"status": "sent"})),
        Err(e) => HttpResponse::BadRequest().body(e),
    }
}

/// Get capture results
#[utoipa::path(
    get,
    path = "/api/phones/captures/{capture_id}",
    tag = "phones",
    params(("capture_id" = String, Path, description = "Capture id")),
    responses(
        (status = 200, description = "Capture results", body = [CaptureResult]),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/phones/captures/{capture_id}")]
pub async fn get_capture_results(
    req: HttpRequest,
    state: web::Data<SlavePhoneState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let capture_id = path.into_inner();
    let captures = state.captures.read().await;
    
    match captures.get(&capture_id) {
        Some(results) => HttpResponse::Ok().json(results),
        None => HttpResponse::Ok().json(Vec::<CaptureResult>::new()),
    }
}

/// WebSocket endpoint for slave phones
#[utoipa::path(
    get,
    path = "/api/phones/ws",
    tag = "phones",
    responses(
        (status = 101, description = "WebSocket upgrade"),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/phones/ws")]
pub async fn phone_websocket(
    req: HttpRequest,
    stream: web::Payload,
    state: web::Data<SlavePhoneState>,
) -> Result<HttpResponse, actix_web::Error> {
    if let Err(resp) = require_scope(&req, "phone:connect") {
        return Ok(resp);
    }
    let (res, mut session, mut msg_stream) = actix_ws::handle(&req, stream)?;
    
    let state = state.get_ref().clone();
    let phone_id = uuid::Uuid::new_v4().to_string();
    let phone_id_clone = phone_id.clone();
    
    // Create command channel for this phone
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<WsMessage>(32);
    
    // Subscribe to broadcast
    let mut broadcast_rx = state.broadcast_tx.subscribe();
    
    // Store command channel
    {
        let mut channels = state.command_tx.write().await;
        channels.insert(phone_id.clone(), cmd_tx);
    }
    
    actix_web::rt::spawn(async move {
        loop {
            tokio::select! {
                // Messages from phone
                Some(msg) = msg_stream.next() => {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                                handle_phone_message(&state, &phone_id, ws_msg, &mut session).await;
                            }
                        }
                        Ok(Message::Binary(data)) => {
                            // Handle image upload
                            handle_image_upload(&state, &phone_id, data.to_vec()).await;
                        }
                        Ok(Message::Ping(data)) => {
                            let _ = session.pong(&data).await;
                        }
                        Ok(Message::Close(_)) | Err(_) => break,
                        _ => {}
                    }
                }
                
                // Commands to send to this phone
                Some(cmd) = cmd_rx.recv() => {
                    if let Ok(json) = serde_json::to_string(&cmd) {
                        let _ = session.text(json).await;
                    }
                }
                
                // Broadcast commands
                Ok(cmd) = broadcast_rx.recv() => {
                    // Check if this phone should receive broadcast
                    let phones = state.phones.read().await;
                    if let Some(phone) = phones.get(&phone_id) {
                        if phone.is_ready && phone.mode == PhoneMode::Slave {
                            if let Ok(json) = serde_json::to_string(&cmd) {
                                let _ = session.text(json).await;
                            }
                        }
                    }
                }
            }
        }
        
        // Cleanup on disconnect
        let mut phones = state.phones.write().await;
        phones.remove(&phone_id_clone);
        let mut channels = state.command_tx.write().await;
        channels.remove(&phone_id_clone);
        
        tracing::info!("Phone {} disconnected", phone_id_clone);
    });
    
    Ok(res)
}

/// Handle incoming phone message
async fn handle_phone_message(
    state: &SlavePhoneState,
    phone_id: &str,
    msg: WsMessage,
    session: &mut actix_ws::Session,
) {
    match msg {
        WsMessage::Register { name, device_info, resolution, battery } => {
            let phone = PhoneSession {
                id: phone_id.to_string(),
                mode: PhoneMode::Slave,
                name,
                device_info,
                connected_at: Utc::now(),
                last_seen: Utc::now(),
                battery_level: battery,
                resolution,
                is_ready: false,
                is_capturing: false,
                capture_count: 0,
                last_capture: None,
                orientation: None,
            };
            
            let mut phones = state.phones.write().await;
            phones.insert(phone_id.to_string(), phone);
            
            // Send registration confirmation
            let response = WsMessage::Registered {
                session_id: phone_id.to_string(),
                server_time: Utc::now().timestamp_millis(),
            };
            if let Ok(json) = serde_json::to_string(&response) {
                let _ = session.text(json).await;
            }
            
            tracing::info!("Phone {} registered", phone_id);
        }
        
        WsMessage::Ready { ready } => {
            let mut phones = state.phones.write().await;
            if let Some(phone) = phones.get_mut(phone_id) {
                phone.is_ready = ready;
                phone.last_seen = Utc::now();
            }
            tracing::info!("Phone {} ready: {}", phone_id, ready);
        }
        
        WsMessage::StatusUpdate { battery, orientation } => {
            let mut phones = state.phones.write().await;
            if let Some(phone) = phones.get_mut(phone_id) {
                phone.battery_level = battery;
                phone.orientation = orientation;
                phone.last_seen = Utc::now();
            }
        }
        
        WsMessage::CaptureComplete { capture_id, timestamp, file_size, success, error } => {
            // Record capture result
            let result = CaptureResult {
                capture_id: capture_id.clone(),
                phone_id: phone_id.to_string(),
                timestamp: DateTime::from_timestamp_millis(timestamp)
                    .unwrap_or_else(Utc::now),
                file_size,
                resolution: (0, 0), // Will be updated with image
                orientation: None,
                success,
                error,
            };
            
            let mut captures = state.captures.write().await;
            captures.entry(capture_id).or_insert_with(Vec::new).push(result);
            
            // Update phone stats
            let mut phones = state.phones.write().await;
            if let Some(phone) = phones.get_mut(phone_id) {
                phone.is_capturing = false;
                phone.capture_count += 1;
                phone.last_capture = Some(Utc::now());
            }
        }
        
        _ => {}
    }
}

/// Handle binary image upload from phone
async fn handle_image_upload(
    state: &SlavePhoneState,
    phone_id: &str,
    data: Vec<u8>,
) {
    let size = data.len() as u64;
    if !state.allow_upload(phone_id, size).await {
        tracing::warn!(
            "Rejected capture from {} ({} bytes) - exceeds upload limits",
            phone_id,
            size
        );
        return;
    }
    if !is_jpeg_payload(&data) {
        tracing::warn!("Rejected capture from {} - invalid JPEG payload", phone_id);
        return;
    }
    // Save image to upload directory
    let filename = format!("{}_{}.jpg", phone_id, Utc::now().timestamp_millis());
    let path = std::path::Path::new(&state.upload_dir).join(&filename);
    
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    
    match tokio::fs::write(&path, &data).await {
        Ok(_) => {
            tracing::info!("Saved capture from {} ({} bytes)", phone_id, data.len());
        }
        Err(e) => {
            tracing::error!("Failed to save capture from {}: {}", phone_id, e);
        }
    }
}

/// Configure slave phone routes
pub fn configure(cfg: &mut web::ServiceConfig, state: web::Data<SlavePhoneState>) {
    cfg.app_data(state)
        .service(list_phones)
        .service(get_phone)
        .service(capture_phone)
        .service(capture_all_phones)
        .service(set_phone_resolution)
        .service(get_capture_results)
        .service(phone_websocket);
}

#[derive(Debug, Clone)]
struct UploadBucket {
    window_start: Instant,
    bytes: u64,
}

fn is_jpeg_payload(data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }
    let soi = data[0] == 0xFF && data[1] == 0xD8;
    let eoi = data[data.len() - 2] == 0xFF && data[data.len() - 1] == 0xD9;
    soi && eoi
}

fn dir_size_bytes(root: &std::path::Path) -> u64 {
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        if let Ok(entry) = entry {
            if entry.file_type().is_file() {
                if let Ok(meta) = entry.metadata() {
                    total = total.saturating_add(meta.len());
                }
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_ws_message_serialization() {
        let msg = WsMessage::Capture {
            capture_id: "test".to_string(),
            flash: true,
            countdown_ms: 1000,
            quality: 90,
        };
        
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("capture"));
        assert!(json.contains("flash"));
    }
    
    #[test]
    fn test_phone_mode() {
        assert_eq!(PhoneMode::Slave, PhoneMode::Slave);
        assert_ne!(PhoneMode::Guest, PhoneMode::Slave);
    }
}
