//! Unified Device Manager API
//!
//! Single endpoint returning all device types for the Device Manager UI.

use crate::auth::require_admin;
use crate::state::AppState;
use actix_web::{get, post, web, HttpRequest, HttpResponse, Responder};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sysinfo::{CpuRefreshKind, DiskKind, Disks, MemoryRefreshKind, RefreshKind, System};
use utoipa::ToSchema;

// ============================================================================
// Unified Device Types
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    Camera,
    DepthCamera,
    Phone,
    Turntable,
    Sensor,
    Microphone,
    Light,
    Storage,
    RobotArm,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionType {
    Usb,
    Network,
    Bluetooth,
    Serial,
    Cloud,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    Connected,
    Disconnected,
    Error,
    Busy,
    Initializing,
    Ready, // For phones that are ready to capture
}

/// Unified device representation for frontend
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UnifiedDevice {
    pub id: String,
    #[serde(rename = "type")]
    pub device_type: DeviceType,
    pub name: String,
    pub nickname: Option<String>,
    pub status: DeviceStatus,
    pub connection: ConnectionType,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub serial_number: Option<String>,
    pub firmware_version: Option<String>,
    pub battery_level: Option<u8>,
    pub last_seen: DateTime<Utc>,
    pub enabled: bool,
    pub group_id: Option<String>,
    pub metadata: serde_json::Value,
}

/// Summary statistics for all devices
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DeviceStats {
    pub total: usize,
    pub connected: usize,
    pub by_type: std::collections::HashMap<String, usize>,
    pub by_connection: std::collections::HashMap<String, usize>,
}

// ============================================================================
// API Endpoints
// ============================================================================

/// Get all devices from all sources
#[utoipa::path(
    get,
    path = "/api/devices",
    tag = "devices",
    responses(
        (status = 200, description = "Device list", body = [UnifiedDevice]),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/devices")]
pub async fn get_all_devices(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let mut devices: Vec<UnifiedDevice> = Vec::new();

    // ========================================================================
    // 1. Cameras (from CameraManager)
    // ========================================================================
    {
        let cm = state.camera_manager.lock().await;

        // Battery levels
        let mut battery_levels: std::collections::HashMap<String, u8> =
            std::collections::HashMap::new();
        for cam in &cm.cameras {
            if let Ok(level) = cam.battery_level() {
                battery_levels.insert(cam.id(), level);
            }
        }

        // Connected IDs
        let connected_ids: Vec<String> = cm.cameras.iter().map(|c| c.id()).collect();

        // Convert registry profiles to unified devices
        for profile in cm.registry.profiles.values() {
            let connected = connected_ids.contains(&profile.id);
            let is_depth = profile.capabilities.has_depth;

            devices.push(UnifiedDevice {
                id: profile.id.clone(),
                device_type: if is_depth {
                    DeviceType::DepthCamera
                } else {
                    DeviceType::Camera
                },
                name: profile.name.clone(),
                nickname: profile.nickname.clone(),
                status: if connected {
                    DeviceStatus::Connected
                } else {
                    DeviceStatus::Disconnected
                },
                connection: ConnectionType::Usb,
                manufacturer: None,
                model: None,
                serial_number: None,
                firmware_version: None,
                battery_level: battery_levels.get(&profile.id).copied(),
                last_seen: Utc::now(),
                enabled: profile.enabled,
                group_id: None,
                metadata: serde_json::json!({
                    "role": format!("{:?}", profile.role),
                    "resolutions": profile.capabilities.resolutions,
                    "has_gimbal": profile.capabilities.has_gimbal,
                    "has_depth": profile.capabilities.has_depth,
                    "has_infrared": profile.capabilities.has_infrared,
                }),
            });
        }
    }

    // ========================================================================
    // 2. Turntable
    // ========================================================================
    {
        let tt_lock = state.turntable.lock().await;
        let connected = tt_lock.is_some();
        let angle = tt_lock.as_ref().map(|t| t.get_rotation()).unwrap_or(0.0);
        let moving = *state.turntable_moving.lock().unwrap();
        let tt_type = state.turntable_status.lock().unwrap().clone();

        devices.push(UnifiedDevice {
            id: "turntable-main".to_string(),
            device_type: DeviceType::Turntable,
            name: if tt_type.is_empty() {
                "Turntable".to_string()
            } else {
                tt_type
            },
            nickname: None,
            status: if connected {
                if moving {
                    DeviceStatus::Busy
                } else {
                    DeviceStatus::Connected
                }
            } else {
                DeviceStatus::Disconnected
            },
            connection: ConnectionType::Serial,
            manufacturer: None,
            model: None,
            serial_number: None,
            firmware_version: None,
            battery_level: None,
            last_seen: Utc::now(),
            enabled: true,
            group_id: None,
            metadata: serde_json::json!({
                "angle": angle,
                "moving": moving,
            }),
        });
    }

    // ========================================================================
    // 3. Phones (from SlavePhoneState)
    // ========================================================================
    if let Some(phone_state) = state.phone_state.as_ref() {
        let phones = phone_state.phones.read().await;
        for phone in phones.values() {
            devices.push(UnifiedDevice {
                id: phone.id.clone(),
                device_type: DeviceType::Phone,
                name: phone.name.clone(),
                nickname: None,
                status: if phone.is_ready {
                    DeviceStatus::Ready
                } else {
                    DeviceStatus::Connected
                },
                connection: ConnectionType::Network,
                manufacturer: None,
                model: Some(phone.device_info.clone()),
                serial_number: None,
                firmware_version: None,
                battery_level: Some(phone.battery_level),
                last_seen: phone.last_seen,
                enabled: true,
                group_id: None,
                metadata: serde_json::json!({
                    "mode": format!("{:?}", phone.mode),
                    "resolution": phone.resolution,
                    "capture_count": phone.capture_count,
                    "is_capturing": phone.is_capturing,
                    "last_capture": phone.last_capture,
                }),
            });
        }
    }

    // ========================================================================
    // 4. Sensors (host telemetry)
    // ========================================================================
    {
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let load_avg = System::load_average();
        let total_memory_mb = sys.total_memory() / 1024;
        let used_memory_mb = (sys.total_memory() - sys.available_memory()) / 1024;

        devices.push(UnifiedDevice {
            id: "sensor-host".to_string(),
            device_type: DeviceType::Sensor,
            name: "Host Sensors".to_string(),
            nickname: None,
            status: DeviceStatus::Connected,
            connection: ConnectionType::Network,
            manufacturer: None,
            model: None,
            serial_number: None,
            firmware_version: None,
            battery_level: None,
            last_seen: Utc::now(),
            enabled: true,
            group_id: None,
            metadata: serde_json::json!({
                "cpu_usage_percent": sys.global_cpu_usage(),
                "memory_used_mb": used_memory_mb,
                "memory_total_mb": total_memory_mb,
                "load_avg": {
                    "one": load_avg.one,
                    "five": load_avg.five,
                    "fifteen": load_avg.fifteen,
                }
            }),
        });
    }

    // ========================================================================
    // 5. Storage (local disks)
    // ========================================================================
    {
        let disks = Disks::new_with_refreshed_list();
        for disk in disks.list() {
            let total_gb = (disk.total_space() as f64 / (1024.0 * 1024.0 * 1024.0)).max(0.01);
            let free_gb = disk.available_space() as f64 / (1024.0 * 1024.0 * 1024.0);
            let used_gb = (total_gb - free_gb).max(0.0);
            let usage_pct = ((used_gb / total_gb) * 100.0).min(100.0);
            let kind = match disk.kind() {
                DiskKind::SSD => "ssd",
                DiskKind::HDD => "hdd",
                DiskKind::Unknown(_) => "unknown",
            };
            let name = disk.name().to_string_lossy().to_string();
            let mount = disk.mount_point().to_string_lossy().to_string();
            let connection = if disk.is_removable() {
                ConnectionType::Usb
            } else {
                ConnectionType::Network
            };
            devices.push(UnifiedDevice {
                id: format!("storage:{}", name),
                device_type: DeviceType::Storage,
                name: format!("Storage {}", name),
                nickname: None,
                status: DeviceStatus::Connected,
                connection,
                manufacturer: None,
                model: None,
                serial_number: None,
                firmware_version: None,
                battery_level: None,
                last_seen: Utc::now(),
                enabled: true,
                group_id: None,
                metadata: serde_json::json!({
                    "kind": kind,
                    "mount_point": mount,
                    "file_system": disk.file_system().to_string_lossy(),
                    "total_gb": total_gb,
                    "free_gb": free_gb,
                    "usage_percent": usage_pct,
                    "removable": disk.is_removable(),
                }),
            });
        }
    }

    HttpResponse::Ok().json(devices)
}

/// Get device statistics summary
#[utoipa::path(
    get,
    path = "/api/devices/stats",
    tag = "devices",
    responses(
        (status = 200, description = "Device stats", body = DeviceStats),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/devices/stats")]
pub async fn get_device_stats(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let mut stats = DeviceStats {
        total: 0,
        connected: 0,
        by_type: std::collections::HashMap::new(),
        by_connection: std::collections::HashMap::new(),
    };

    // Get all devices and compute stats
    // This is a simplified version - in production, cache this

    // Count cameras
    {
        let cm = state.camera_manager.lock().await;
        let connected_count = cm.cameras.len();
        let total_count = cm.registry.profiles.len();

        stats.total += total_count;
        stats.connected += connected_count;
        *stats.by_type.entry("camera".to_string()).or_insert(0) += total_count;
        *stats.by_connection.entry("usb".to_string()).or_insert(0) += total_count;
    }

    // Count phones
    if let Some(phone_state) = state.phone_state.as_ref() {
        let phones = phone_state.phones.read().await;
        let phone_count = phones.len();
        stats.total += phone_count;
        stats.connected += phone_count; // All listed phones are connected
        *stats.by_type.entry("phone".to_string()).or_insert(0) += phone_count;
        *stats
            .by_connection
            .entry("network".to_string())
            .or_insert(0) += phone_count;
    }

    // Count sensors (host telemetry)
    stats.total += 1;
    stats.connected += 1;
    *stats.by_type.entry("sensor".to_string()).or_insert(0) += 1;
    *stats
        .by_connection
        .entry("network".to_string())
        .or_insert(0) += 1;

    // Count storage devices
    let disks = Disks::new_with_refreshed_list();
    let storage_count = disks.list().len();
    if storage_count > 0 {
        stats.total += storage_count;
        stats.connected += storage_count;
        *stats.by_type.entry("storage".to_string()).or_insert(0) += storage_count;
        *stats
            .by_connection
            .entry("network".to_string())
            .or_insert(0) += storage_count;
    }

    // Count turntable
    {
        let tt = state.turntable.lock().await;
        stats.total += 1;
        if tt.is_some() {
            stats.connected += 1;
        }
        *stats.by_type.entry("turntable".to_string()).or_insert(0) += 1;
        *stats.by_connection.entry("serial".to_string()).or_insert(0) += 1;
    }

    HttpResponse::Ok().json(stats)
}

/// Trigger action on a device
#[derive(Debug, Deserialize, ToSchema)]
pub struct DeviceAction {
    pub action: String, // "capture", "enable", "disable", etc.
    #[serde(default)]
    pub params: serde_json::Value,
}

#[utoipa::path(
    post,
    path = "/api/devices/{id}/action",
    tag = "devices",
    params(("id" = String, Path, description = "Device id")),
    request_body = DeviceAction,
    responses(
        (status = 200, description = "Action result", body = serde_json::Value),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/devices/{id}/action")]
pub async fn device_action(
    req: HttpRequest,
    path: web::Path<String>,
    body: web::Json<DeviceAction>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let device_id = path.into_inner();
    let action = &body.action;

    // Route action based on device type
    // First, find which manager owns this device

    // Check if it's a phone
    if let Some(phone_state) = state.phone_state.as_ref() {
        let phones = phone_state.phones.read().await;
        if phones.contains_key(&device_id) {
            drop(phones); // Release read lock

            if action.as_str() == "capture" {
                let msg = crate::guest::slave::WsMessage::Capture {
                    capture_id: uuid::Uuid::new_v4().to_string(),
                    flash: body
                        .params
                        .get("flash")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    countdown_ms: body
                        .params
                        .get("countdown_ms")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    quality: body
                        .params
                        .get("quality")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(90) as u8,
                };

                match phone_state.send_to_phone(&device_id, msg).await {
                    Ok(_) => return HttpResponse::Ok().json(serde_json::json!({"status": "sent"})),
                    Err(e) => return HttpResponse::BadRequest().body(e),
                }
            }
        }
    }

    // Check if it's a camera
    {
        let cm = state.camera_manager.lock().await;
        if cm.registry.profiles.contains_key(&device_id) {
            match action.as_str() {
                "capture" => {
                    if let Some(cam) = cm.get_camera_by_id(&device_id) {
                        let config = trueshot_device_manager::camera::CameraConfig {
                            iso: None,
                            shutter_speed: None,
                            aperture: None,
                            wb: None,
                            capture_target: None,
                            resolution: None,
                            fps: None,
                        };
                        match cam.capture(&config) {
                            Ok(path) => {
                                return HttpResponse::Ok().json(serde_json::json!({
                                    "status": "captured",
                                    "path": path.display().to_string()
                                }))
                            }
                            Err(e) => {
                                return HttpResponse::InternalServerError().body(e.to_string())
                            }
                        }
                    }
                }
                "enable" => {
                    drop(cm);
                    let mut cm = state.camera_manager.lock().await;
                    let _ = cm.registry.set_enabled(&device_id, true);
                    return HttpResponse::Ok().json(serde_json::json!({"status": "enabled"}));
                }
                "disable" => {
                    drop(cm);
                    let mut cm = state.camera_manager.lock().await;
                    let _ = cm.registry.set_enabled(&device_id, false);
                    return HttpResponse::Ok().json(serde_json::json!({"status": "disabled"}));
                }
                _ => {}
            }
        }
    }

    // Check if it's the turntable
    if device_id == "turntable-main" {
        match action.as_str() {
            "home" => {
                let mut tt_lock = state.turntable.lock().await;
                if let Some(tt) = tt_lock.as_mut() {
                    if let Err(e) = tt.home().await {
                        return HttpResponse::InternalServerError().body(e.to_string());
                    }
                    return HttpResponse::Ok().json(serde_json::json!({"status": "homed"}));
                }
            }
            "rotate" => {
                let degrees = body
                    .params
                    .get("degrees")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as f32;

                let mut tt_lock = state.turntable.lock().await;
                if let Some(tt) = tt_lock.as_mut() {
                    if let Err(e) = tt.rotate(degrees).await {
                        return HttpResponse::InternalServerError().body(e.to_string());
                    }
                    return HttpResponse::Ok().json(serde_json::json!({
                        "status": "rotated",
                        "degrees": degrees
                    }));
                }
            }
            _ => {}
        }
    }

    HttpResponse::NotFound().body("Device not found or action not supported")
}

/// Batch action on all devices of a type
#[utoipa::path(
    post,
    path = "/api/devices/batch",
    tag = "devices",
    request_body = DeviceAction,
    responses(
        (status = 200, description = "Batch action result", body = serde_json::Value),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/devices/batch")]
pub async fn batch_device_action(
    req: HttpRequest,
    body: web::Json<DeviceAction>,
    state: web::Data<AppState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let action = &body.action;

    match action.as_str() {
        "capture_all_phones" => {
            if let Some(phone_state) = state.phone_state.as_ref() {
                let capture_id = uuid::Uuid::new_v4().to_string();
                let msg = crate::guest::slave::WsMessage::Capture {
                    capture_id: capture_id.clone(),
                    flash: false,
                    countdown_ms: 0,
                    quality: 90,
                };

                let ready_count = phone_state.ready_phones().await.len();

                match phone_state.broadcast(msg) {
                    Ok(sent) => {
                        return HttpResponse::Ok().json(serde_json::json!({
                            "status": "broadcast",
                            "capture_id": capture_id,
                            "phones_ready": ready_count,
                            "messages_sent": sent
                        }));
                    }
                    Err(e) => return HttpResponse::InternalServerError().body(e),
                }
            }
            HttpResponse::BadRequest().body("Phone state not available")
        }

        "scan" => {
            // Trigger hardware scan
            let mut cm = state.camera_manager.lock().await;
            let mock = state.config.hardware.mock_devices.unwrap_or(false);
            match cm.reconcile_cameras(mock).await {
                Ok(report) => HttpResponse::Ok().json(report),
                Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
            }
        }

        _ => HttpResponse::BadRequest().body("Unknown batch action"),
    }
}

/// Configure unified device routes
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(get_all_devices)
        .service(get_device_stats)
        .service(device_action)
        .service(batch_device_action);
}
