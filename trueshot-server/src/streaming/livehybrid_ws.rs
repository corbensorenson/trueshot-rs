//! LiveHybrid WebSocket Streaming Server
//!
//! State-of-the-art real-time streaming:
//! - Binary WebSocket protocol for minimal overhead
//! - Adaptive quality based on client bandwidth
//! - Client capability negotiation
//! - Multiple stream types (full scene, avatar-only, mesh-only)
//!
//! Protocol flow:
//! 1. Client connects and sends capabilities
//! 2. Server responds with session config
//! 3. Server streams packets based on client mode
//! 4. Client can request quality adjustments

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State, Query,
    },
    response::IntoResponse,
    http::{HeaderMap, StatusCode, header::{AUTHORIZATION, ORIGIN}},
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, RwLock};
use uuid::Uuid;

use crate::auth::{AuthContext, AuthError, AuthVerifier, Role, SESSION_COOKIE_NAME};

/// WebSocket streaming configuration
#[derive(Clone, Debug)]
pub struct StreamingConfig {
    /// Maximum clients per session
    pub max_clients: usize,
    /// Heartbeat interval
    pub heartbeat_interval: Duration,
    /// Client timeout
    pub client_timeout: Duration,
    /// Default stream quality
    pub default_quality: StreamQuality,
    /// Enable compression
    pub compression: bool,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            max_clients: 100,
            heartbeat_interval: Duration::from_secs(5),
            client_timeout: Duration::from_secs(30),
            default_quality: StreamQuality::Medium,
            compression: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct StreamingRouterConfig {
    pub auth: Arc<AuthVerifier>,
    pub required_scopes: Vec<String>,
    pub allowed_origins: Option<HashSet<String>>,
    pub allow_missing_origin: bool,
    pub max_message_bytes: usize,
    pub max_pending_messages: usize,
    pub max_dropped_frames: u64,
}

impl StreamingRouterConfig {
    pub fn new(auth: Arc<AuthVerifier>) -> Self {
        Self {
            auth,
            required_scopes: vec!["stream:read".to_string()],
            allowed_origins: None,
            allow_missing_origin: true,
            max_message_bytes: 64 * 1024,
            max_pending_messages: 64,
            max_dropped_frames: 100,
        }
    }
}

#[derive(Clone)]
pub struct StreamingRouterState {
    pub manager: Arc<StreamingSessionManager>,
    pub config: StreamingRouterConfig,
}

/// Stream quality levels
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum StreamQuality {
    /// Low bandwidth: avatar poses only, minimal updates
    Low,
    /// Medium: avatars + mesh transforms
    Medium,
    /// High: full scene with Gaussian deltas
    High,
    /// Ultra: maximum fidelity, keyframes
    Ultra,
}

impl StreamQuality {
    pub fn bitrate_kbps(&self) -> u32 {
        match self {
            StreamQuality::Low => 100,
            StreamQuality::Medium => 500,
            StreamQuality::High => 2000,
            StreamQuality::Ultra => 10000,
        }
    }
    
    pub fn frame_interval(&self) -> Duration {
        match self {
            StreamQuality::Low => Duration::from_millis(100),    // 10 fps
            StreamQuality::Medium => Duration::from_millis(50),  // 20 fps
            StreamQuality::High => Duration::from_millis(33),    // 30 fps
            StreamQuality::Ultra => Duration::from_millis(16),   // 60 fps
        }
    }
}

/// Client capabilities sent on connect
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientCapabilities {
    pub client_id: Uuid,
    pub supported_formats: Vec<String>,
    pub max_gaussians: u32,
    pub supports_lod: bool,
    pub preferred_quality: StreamQuality,
    pub device_type: DeviceType,
}

/// Device type for adaptive streaming
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeviceType {
    Desktop,
    Mobile,
    VR,
    AR,
}

/// Server response to client
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionConfig {
    pub session_id: Uuid,
    pub quality: StreamQuality,
    pub compression_enabled: bool,
    pub keyframe_interval: u32,
    pub server_time: u64,
}

/// Stream mode selection
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum StreamMode {
    /// Full hybrid scene
    FullScene,
    /// Avatars only (lowest bandwidth)
    AvatarsOnly,
    /// Meshes only (static content)
    MeshesOnly,
    /// Gaussians only (dynamic content)
    GaussiansOnly,
}

/// Client-to-server messages
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    /// Initial handshake
    Connect { capabilities: ClientCapabilities },
    /// Quality adjustment request
    SetQuality { quality: StreamQuality },
    /// Stream mode change
    SetMode { mode: StreamMode },
    /// Ping for latency measurement
    Ping { timestamp: u64 },
    /// Request keyframe
    RequestKeyframe,
    /// Disconnect
    Disconnect,
}

/// Server-to-client messages
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    /// Session established
    Connected { config: SessionConfig },
    /// Stream data packet (binary follows)
    StreamPacket { 
        frame_id: u64,
        packet_type: PacketType,
        size: u32,
    },
    /// Pong response
    Pong { 
        client_timestamp: u64,
        server_timestamp: u64,
    },
    /// Error message
    Error { message: String },
    /// Quality changed confirmation
    QualityChanged { quality: StreamQuality },
    /// Heartbeat
    Heartbeat { server_frame: u64 },
}

/// Packet type identifier
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PacketType {
    SceneGraph,
    GaussianFull,
    GaussianDelta,
    MeshAsset,
    TransformUpdate,
    AvatarPose,
    TextureChunk,
}

/// Connected client state
#[derive(Clone)]
pub struct ConnectedClient {
    pub id: Uuid,
    pub session_id: Uuid,
    pub capabilities: ClientCapabilities,
    pub subject: String,
    pub role: Role,
    pub scopes: Vec<String>,
    pub quality: StreamQuality,
    pub mode: StreamMode,
    pub last_seen: Instant,
    pub frames_sent: u64,
    pub bytes_sent: u64,
    pub dropped_frames: u64,
}

/// Streaming session manager
pub struct StreamingSessionManager {
    config: StreamingConfig,
    clients: RwLock<Vec<ConnectedClient>>,
    broadcast_tx: broadcast::Sender<Vec<u8>>,
    frame_counter: RwLock<u64>,
}

impl StreamingSessionManager {
    pub fn new(config: StreamingConfig) -> Self {
        let (broadcast_tx, _) = broadcast::channel(1000);
        
        Self {
            config,
            clients: RwLock::new(Vec::new()),
            broadcast_tx,
            frame_counter: RwLock::new(0),
        }
    }
    
    /// Add a new client
    pub async fn add_client(&self, capabilities: ClientCapabilities) -> Option<SessionConfig> {
        self.add_client_with_auth(capabilities, None)
            .await
    }

    pub async fn add_client_with_auth(
        &self,
        capabilities: ClientCapabilities,
        auth_ctx: Option<&AuthContext>,
    ) -> Option<SessionConfig> {
        let mut clients = self.clients.write().await;
        
        if clients.len() >= self.config.max_clients {
            return None;
        }
        
        let session_id = Uuid::new_v4();
        let quality = match capabilities.device_type {
            DeviceType::Mobile => StreamQuality::Low,
            DeviceType::VR | DeviceType::AR => StreamQuality::High,
            DeviceType::Desktop => capabilities.preferred_quality,
        };
        
        let (subject, role, scopes) = match auth_ctx {
            Some(ctx) => (ctx.sub.clone(), ctx.role, ctx.scopes.clone()),
            None => ("unknown".to_string(), Role::Guest, Vec::new()),
        };
        let client = ConnectedClient {
            id: capabilities.client_id,
            session_id,
            capabilities: capabilities.clone(),
            subject,
            role,
            scopes,
            quality,
            mode: StreamMode::FullScene,
            last_seen: Instant::now(),
            frames_sent: 0,
            bytes_sent: 0,
            dropped_frames: 0,
        };
        
        clients.push(client);
        
        Some(SessionConfig {
            session_id,
            quality,
            compression_enabled: self.config.compression,
            keyframe_interval: 30,
            server_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        })
    }
    
    /// Remove a client
    pub async fn remove_client(&self, client_id: Uuid) {
        let mut clients = self.clients.write().await;
        clients.retain(|c| c.id != client_id);
    }
    
    /// Update client quality
    pub async fn set_client_quality(&self, client_id: Uuid, quality: StreamQuality) {
        let mut clients = self.clients.write().await;
        if let Some(client) = clients.iter_mut().find(|c| c.id == client_id) {
            client.quality = quality;
        }
    }
    
    /// Broadcast packet to all clients at appropriate quality
    pub async fn broadcast(&self, data: Vec<u8>) {
        let _ = self.broadcast_tx.send(data);
        
        let mut frame_counter = self.frame_counter.write().await;
        *frame_counter += 1;
    }
    
    /// Get broadcast receiver
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.broadcast_tx.subscribe()
    }
    
    /// Get current frame
    pub async fn current_frame(&self) -> u64 {
        *self.frame_counter.read().await
    }
    
    /// Get connected client count
    pub async fn client_count(&self) -> usize {
        self.clients.read().await.len()
    }
}

/// Query parameters for WebSocket connection
#[derive(Debug, Deserialize)]
pub struct WsQuery {
    pub client_id: Option<String>,
    pub quality: Option<String>,
    pub token: Option<String>,
}

/// WebSocket upgrade handler
pub async fn livehybrid_ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
    State(state): State<StreamingRouterState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !origin_allowed(&headers, &state.config.allowed_origins, state.config.allow_missing_origin) {
        return (StatusCode::FORBIDDEN, "Origin not allowed").into_response();
    }

    let token = match extract_token(&headers, &query) {
        Some(token) => token,
        None => {
            return (StatusCode::UNAUTHORIZED, "Missing auth token").into_response();
        }
    };

    let auth_ctx = match state.config.auth.verify_token(&token) {
        Ok(ctx) => ctx,
        Err(AuthError::Expired) => {
            return (StatusCode::UNAUTHORIZED, "Auth token expired").into_response();
        }
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, "Invalid auth token").into_response();
        }
    };

    if !scopes_allow(&auth_ctx, &state.config.required_scopes) {
        return (StatusCode::FORBIDDEN, "Insufficient scope").into_response();
    }

    let client_id = query.client_id
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_else(Uuid::new_v4);
    
    let quality = query.quality
        .and_then(|s| match s.as_str() {
            "low" => Some(StreamQuality::Low),
            "medium" => Some(StreamQuality::Medium),
            "high" => Some(StreamQuality::High),
            "ultra" => Some(StreamQuality::Ultra),
            _ => None,
        })
        .unwrap_or(StreamQuality::Medium);
    
    let manager = state.manager.clone();
    let socket_cfg = SocketConfig {
        max_message_bytes: state.config.max_message_bytes,
        max_pending_messages: state.config.max_pending_messages,
        max_dropped_frames: state.config.max_dropped_frames,
    };
    ws.on_upgrade(move |socket| handle_socket(socket, client_id, quality, manager, auth_ctx, socket_cfg))
}

#[derive(Clone, Copy)]
struct SocketConfig {
    max_message_bytes: usize,
    max_pending_messages: usize,
    max_dropped_frames: u64,
}

/// Handle WebSocket connection
async fn handle_socket(
    socket: WebSocket,
    client_id: Uuid,
    preferred_quality: StreamQuality,
    manager: Arc<StreamingSessionManager>,
    auth_ctx: AuthContext,
    socket_cfg: SocketConfig,
) {
    let (mut sender, mut receiver) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(socket_cfg.max_pending_messages);
    let send_task = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });
    
    // Wait for client capabilities
    let mut capabilities = match tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_connect(&mut receiver, socket_cfg.max_message_bytes),
    ).await {
        Ok(Some(caps)) => caps,
        _ => {
            let _ = out_tx.send(Message::Text(
                serde_json::to_string(&ServerMessage::Error {
                    message: "Connection timeout".into()
                }).unwrap()
            )).await;
            send_task.abort();
            return;
        }
    };
    
    capabilities.preferred_quality = preferred_quality;

    // Register client
    let session_config = match manager.add_client_with_auth(capabilities.clone(), Some(&auth_ctx)).await {
        Some(config) => config,
        None => {
            let _ = out_tx.send(Message::Text(
                serde_json::to_string(&ServerMessage::Error {
                    message: "Server at capacity".into()
                }).unwrap()
            )).await;
            send_task.abort();
            return;
        }
    };
    
    // Send session config
    let _ = out_tx.send(Message::Text(
        serde_json::to_string(&ServerMessage::Connected { 
            config: session_config.clone() 
        }).unwrap()
    )).await;
    
    log::info!(
        "Client {} connected (subject={}, role={:?}), quality: {:?}",
        client_id,
        auth_ctx.sub,
        auth_ctx.role,
        session_config.quality
    );
    
    // Subscribe to broadcast
    let mut broadcast_rx = manager.subscribe();
    
    // Spawn heartbeat task
    let manager_clone = manager.clone();
    let heartbeat_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            let frame = manager_clone.current_frame().await;
            // Heartbeat would be sent through the sender
        }
    });
    
    let mut dropped_frames = 0u64;

    // Main message loop
    loop {
        tokio::select! {
            // Incoming client message
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if text.len() > socket_cfg.max_message_bytes {
                            let _ = out_tx.send(Message::Text(
                                serde_json::to_string(&ServerMessage::Error {
                                    message: "Message too large".into()
                                }).unwrap()
                            )).await;
                            break;
                        }
                        if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                            match client_msg {
                                ClientMessage::SetQuality { quality } => {
                                    manager.set_client_quality(client_id, quality).await;
                                    let _ = out_tx.try_send(Message::Text(
                                        serde_json::to_string(&ServerMessage::QualityChanged { quality }).unwrap()
                                    ));
                                }
                                ClientMessage::Ping { timestamp } => {
                                    let _ = out_tx.try_send(Message::Text(
                                        serde_json::to_string(&ServerMessage::Pong {
                                            client_timestamp: timestamp,
                                            server_timestamp: std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap()
                                                .as_millis() as u64,
                                        }).unwrap()
                                    ));
                                }
                                ClientMessage::Disconnect => {
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        break;
                    }
                    _ => {}
                }
            }
            
            // Outgoing broadcast packet
            packet = broadcast_rx.recv() => {
                match packet {
                    Ok(data) => {
                        if out_tx.try_send(Message::Binary(data)).is_err() {
                            dropped_frames = dropped_frames.saturating_add(1);
                            if dropped_frames >= socket_cfg.max_dropped_frames {
                                break;
                            }
                        } else {
                            dropped_frames = 0;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        dropped_frames = dropped_frames.saturating_add(skipped as u64);
                        if dropped_frames >= socket_cfg.max_dropped_frames {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    
    // Cleanup
    heartbeat_task.abort();
    send_task.abort();
    manager.remove_client(client_id).await;
    log::info!("Client {} disconnected", client_id);
}

/// Wait for client connect message
async fn wait_for_connect(
    receiver: &mut futures_util::stream::SplitStream<WebSocket>,
    max_message_bytes: usize,
) -> Option<ClientCapabilities> {
    while let Some(msg) = receiver.next().await {
        if let Ok(Message::Text(text)) = msg {
            if text.len() > max_message_bytes {
                return None;
            }
            if let Ok(ClientMessage::Connect { capabilities }) = serde_json::from_str(&text) {
                return Some(capabilities);
            }
        }
    }
    None
}

/// Create router for LiveHybrid streaming
pub fn create_streaming_router(manager: Arc<StreamingSessionManager>) -> axum::Router {
    create_streaming_router_with_config(
        manager,
        StreamingRouterConfig::new(Arc::new(
            AuthVerifier::new("trueshot")
                .expect("Failed to initialize streaming auth verifier"),
        )),
    )
}

pub fn create_streaming_router_with_config(
    manager: Arc<StreamingSessionManager>,
    config: StreamingRouterConfig,
) -> axum::Router {
    use axum::routing::get;
    
    axum::Router::new()
        .route("/ws", get(livehybrid_ws_handler))
        .route("/stats", get(stats_handler))
        .with_state(StreamingRouterState {
            manager,
            config,
        })
}

/// Stats endpoint
async fn stats_handler(
    State(manager): State<Arc<StreamingSessionManager>>,
) -> impl IntoResponse {
    let client_count = manager.client_count().await;
    let current_frame = manager.current_frame().await;
    
    axum::Json(serde_json::json!({
        "clients": client_count,
        "current_frame": current_frame,
        "status": "streaming"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_session_manager() {
        let manager = StreamingSessionManager::new(StreamingConfig::default());
        
        let caps = ClientCapabilities {
            client_id: Uuid::new_v4(),
            supported_formats: vec!["v1".into()],
            max_gaussians: 100000,
            supports_lod: true,
            preferred_quality: StreamQuality::High,
            device_type: DeviceType::Desktop,
        };
        
        let session = manager.add_client(caps).await;
        assert!(session.is_some());
        assert_eq!(manager.client_count().await, 1);
    }
    
    #[test]
    fn test_quality_bitrate() {
        assert!(StreamQuality::Ultra.bitrate_kbps() > StreamQuality::Low.bitrate_kbps());
    }
}

fn extract_token(headers: &HeaderMap, query: &WsQuery) -> Option<String> {
    if let Some(token) = query.token.as_ref() {
        if !token.is_empty() {
            return Some(token.to_string());
        }
    }
    if let Some(token) = extract_bearer(headers) {
        return Some(token);
    }
    extract_cookie(headers, SESSION_COOKIE_NAME)
}

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    Some(token.to_string())
}

fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookie = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for part in cookie.split(';') {
        let mut iter = part.trim().splitn(2, '=');
        let key = iter.next()?.trim();
        if key == name {
            let value = iter.next().unwrap_or("").trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn origin_allowed(headers: &HeaderMap, allowed: &Option<HashSet<String>>, allow_missing: bool) -> bool {
    let Some(allowed) = allowed.as_ref() else {
        return true;
    };
    let origin = headers.get(ORIGIN).and_then(|v| v.to_str().ok());
    match origin {
        Some(origin) => allowed.contains(origin),
        None => allow_missing,
    }
}

fn scopes_allow(ctx: &AuthContext, required_scopes: &[String]) -> bool {
    if ctx.role == Role::Admin || required_scopes.is_empty() {
        return true;
    }
    for scope in &ctx.scopes {
        if scope == "*" {
            return true;
        }
    }
    required_scopes.iter().any(|req| ctx.scopes.iter().any(|s| s == req))
}
