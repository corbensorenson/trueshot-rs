use actix_web::{web, HttpRequest, HttpResponse};
use actix_ws;
use futures::StreamExt as _;
use crate::state::AppState;
use trueshot_core::events::SystemEvent;
use crate::auth::{require_guest_or_admin, require_scope};
use tokio::sync::mpsc;

struct WsLimits {
    max_message_bytes: usize,
    max_pending_messages: usize,
    max_dropped_messages: u64,
}

fn ws_limits_from_env() -> WsLimits {
    let max_message_bytes = std::env::var("TRUESHOT_WS_MAX_MESSAGE_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(64 * 1024);
    let max_pending_messages = std::env::var("TRUESHOT_WS_MAX_PENDING")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(64);
    let max_dropped_messages = std::env::var("TRUESHOT_WS_MAX_DROPPED")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(50);
    WsLimits {
        max_message_bytes,
        max_pending_messages,
        max_dropped_messages,
    }
}

fn origin_allowed(req: &HttpRequest, allowed: &Option<Vec<String>>) -> bool {
    let Some(allowed) = allowed.as_ref() else {
        return true;
    };
    let origin = req
        .headers()
        .get("Origin")
        .and_then(|v| v.to_str().ok());
    match origin {
        Some(origin) => allowed.iter().any(|o| o == origin),
        None => true,
    }
}

#[utoipa::path(
    get,
    path = "/api/ws",
    tag = "websocket",
    responses(
        (status = 101, description = "WebSocket upgrade"),
        (status = 401, description = "Unauthorized")
    )
)]
pub async fn ws_index(
    req: HttpRequest,
    stream: web::Payload,
    state: web::Data<AppState>,
) -> Result<HttpResponse, actix_web::Error> {
    if !origin_allowed(&req, &state.config.server.allowed_origins) {
        return Ok(HttpResponse::Forbidden().body("Origin not allowed"));
    }
    if let Err(resp) = require_guest_or_admin(&req) {
        return Ok(resp);
    }
    if let Err(resp) = require_scope(&req, "system:read") {
        return Ok(resp);
    }
    let (res, session, msg_stream) = actix_ws::handle(&req, stream)?;
    
    // Spawn task to handle this session
    let event_rx = state.event_bus.subscribe();
    let state_clone = state.clone();
    let limits = ws_limits_from_env();
    let (tx, rx) = mpsc::channel::<String>(limits.max_pending_messages);
    
    actix_rt::spawn(async move {
        forward_events(event_rx, tx, limits).await;
    });
    actix_rt::spawn(async move {
        handle_ws_session(session, msg_stream, rx, state_clone).await;
    });

    Ok(res)
}

async fn handle_ws_session(
    mut session: actix_ws::Session,
    mut msg_stream: actix_ws::MessageStream,
    mut outbound: mpsc::Receiver<String>,
    state: web::Data<AppState>,
) {
    // 1. Initial State Sync (Send current devices to new client)
    // Cameras
    {
        let cm = state.camera_manager.lock().await;
        for cam in &cm.cameras {
            let event = SystemEvent::DeviceConnected { 
                kind: "camera".to_string(), 
                id: cam.id() 
            };
            if let Ok(json) = serde_json::to_string(&event) {
                let _ = session.text(json).await;
            }
        }
    }

    // Turntable
    {
        let tt = state.turntable.lock().await;
        if tt.is_some() {
            let event = SystemEvent::TurntableStatus { 
                connected: true, 
                angle: 0.0, 
                moving: *state.turntable_moving.lock().unwrap() 
            };
             if let Ok(json) = serde_json::to_string(&event) {
                let _ = session.text(json).await;
            }
        }
    }

    loop {
        tokio::select! {
             // Handle incoming messages from client (e.g. pings, or commands if we supported them)
             Some(Ok(msg)) = msg_stream.next() => {
                 match msg {
                     actix_ws::Message::Ping(bytes) => {
                         if session.pong(&bytes).await.is_err() { break; }
                     }
                     actix_ws::Message::Close(reason) => {
                         let _ = session.close(reason).await;
                         break;
                     }
                     _ => {} // Ignore text/binary for now
                 }
             }
             
             // Handle outgoing events from bounded queue
             Some(payload) = outbound.recv() => {
                 if session.text(payload).await.is_err() {
                     break;
                 }
             }
             
             else => break,
        }
    }
}

async fn forward_events(
    mut event_rx: tokio::sync::broadcast::Receiver<SystemEvent>,
    outbound: mpsc::Sender<String>,
    limits: WsLimits,
) {
    let mut dropped: u64 = 0;
    loop {
        match event_rx.recv().await {
            Ok(event) => {
                let payload = match serde_json::to_string(&event) {
                    Ok(payload) => payload,
                    Err(_) => continue,
                };
                if payload.len() > limits.max_message_bytes {
                    dropped += 1;
                    continue;
                }
                if outbound.try_send(payload).is_err() {
                    dropped += 1;
                }
                if dropped >= limits.max_dropped_messages {
                    tracing::warn!("WebSocket event backlog exceeded drop limit; terminating sender");
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                dropped += 1;
                if dropped >= limits.max_dropped_messages {
                    tracing::warn!("WebSocket lag exceeded drop limit; terminating sender");
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                break;
            }
        }
    }
    let _ = outbound.closed().await;
}
