#![recursion_limit = "256"]

use crate::auth::{AuthManager, AuthMiddleware};
use crate::config::AppConfig as Config;
use crate::queue::{JobQueue, QueueJobPayload, QueueObserver};
use actix_cors::Cors;
use actix_files as fs;
use actix_web::{
    middleware::{Condition, DefaultHeaders},
    web, App, HttpResponse, HttpServer,
};
use actix_web_prom::PrometheusMetricsBuilder;
use chrono::Utc;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use trueshot_core::crash_handler::init_crash_handler;
use trueshot_core::events::{EventBus, SystemEvent};
use trueshot_core::inventory::Inventory;
use trueshot_core::scheduler::{Scheduler, SchedulerObserver};
use trueshot_device_manager::{
    CameraManager, Foldio360, SerialTurntable, Turntable, TurntableFeedbackConfig,
};
use utoipa::OpenApi;

mod api;
mod api_doc;
mod at_rest;
mod audit;
mod auth;
mod auth_store;
mod config;
mod distributed_bus;
mod fs_safety;
mod guest;
mod intervalometer;
mod licensing;
mod queue;
mod rate_limit;
mod redis_runtime;
mod retention;
mod scan_types;
mod scan_wizard;
mod state;
mod telemetry;
mod tls;
mod trace_middleware;

use api_doc::ApiDoc;
use guest::SlavePhoneState;
use intervalometer::IntervalometerState;
use scan_wizard::ScanWizardState;
use state::AppState;

// --- Main ---

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    if let Some(out_path) = parse_openapi_args() {
        let spec = ApiDoc::openapi();
        let json = serde_json::to_string_pretty(&spec).unwrap_or_else(|_| "{}".to_string());
        if let Some(path) = out_path {
            std::fs::write(path, json)?;
        } else {
            println!("{json}");
        }
        return Ok(());
    }
    let _crash_guard = init_crash_handler(std::env::var("TRUESHOT_SENTRY_DSN").ok());
    // 1. Config & Logs
    let mut config = Config::load().expect("Failed to load config");
    let env = std::env::var("TRUESHOT_ENV").unwrap_or_else(|_| "development".to_string());
    let is_production = env == "production";
    let file_appender = tracing_appender::rolling::daily("logs", "trueshot.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false);
    let log_stdout = std::env::var("TRUESHOT_LOG_STDOUT")
        .map(|v| !matches!(v.to_lowercase().as_str(), "0" | "false" | "no"))
        .unwrap_or(true);
    let stdout_layer = if log_stdout {
        Some(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stdout)
                .with_ansi(false),
        )
    } else {
        None
    };
    let telemetry_settings = telemetry::TelemetrySettings::from_config(&config, is_production);
    let telemetry_tracer = telemetry::init_tracer(&telemetry_settings)
        .map_err(|e| std::io::Error::other(format!("Telemetry init failed: {}", e)))?;
    let telemetry_layer =
        telemetry_tracer.map(|tracer| tracing_opentelemetry::layer().with_tracer(tracer));
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(stdout_layer)
        .with(telemetry_layer)
        .init();
    info!("Inventory DB: {:?}", config.paths.inventory_db);
    info!("Camera index hints: {:?}", config.hardware.camera_indices);
    let admin_ttl =
        std::time::Duration::from_secs(config.server.admin_token_ttl_seconds.unwrap_or(3600));
    let guest_ttl =
        std::time::Duration::from_secs(config.server.guest_token_ttl_seconds.unwrap_or(900));
    let refresh_ttl = std::time::Duration::from_secs(
        config
            .server
            .refresh_token_ttl_seconds
            .unwrap_or(60 * 60 * 24 * 30),
    );

    if let Some(path) = config.privacy.provenance_key_path.as_ref() {
        std::env::set_var(
            "TRUESHOT_PROVENANCE_KEY_PATH",
            path.to_string_lossy().to_string(),
        );
    }
    if config.privacy.redact_device_id.unwrap_or(false) {
        std::env::set_var("TRUESHOT_REDACT_DEVICE_ID", "1");
    }
    if config.privacy.redact_operator_id.unwrap_or(false) {
        std::env::set_var("TRUESHOT_REDACT_OPERATOR_ID", "1");
    }
    if config.privacy.redact_session_id.unwrap_or(false) {
        std::env::set_var("TRUESHOT_REDACT_SESSION_ID", "1");
    }
    if config.privacy.redact_capture_hashes.unwrap_or(false) {
        std::env::set_var("TRUESHOT_REDACT_CAPTURE_HASHES", "1");
    }
    if let Some(value) = config.legal.license_title.as_ref() {
        std::env::set_var("TRUESHOT_LICENSE_TITLE", value);
    }
    if let Some(value) = config.legal.license_url.as_ref() {
        std::env::set_var("TRUESHOT_LICENSE_URL", value);
    }
    if let Some(value) = config.legal.data_ownership.as_ref() {
        std::env::set_var("TRUESHOT_DATA_OWNERSHIP", value);
    }
    if let Some(value) = config.legal.export_rights.as_ref() {
        std::env::set_var("TRUESHOT_EXPORT_RIGHTS", value);
    }
    let auth_store = Arc::new(
        auth_store::AuthStore::new(&config.paths.auth_db)
            .await
            .map_err(|e| std::io::Error::other(format!("Auth store init failed: {}", e)))?,
    );
    let auth = Arc::new(
        AuthManager::new(
            "trueshot".to_string(),
            admin_ttl,
            guest_ttl,
            refresh_ttl,
            auth_store,
        )
        .map_err(|e| std::io::Error::other(format!("Auth init failed: {}", e)))?,
    );
    if at_rest::encryption_required(&config.privacy, &config.paths.projects_dir) {
        at_rest::require_master_key(&config.privacy, &config.paths.projects_dir)
            .map_err(|e| std::io::Error::other(format!("Encryption master key required: {}", e)))?;
    }
    let rate_limiter = rate_limit::RateLimiter::from_config(&config, is_production).map(Arc::new);

    // 2. State
    let event_bus = Arc::new(EventBus::new());
    let worker_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let job_queue = Arc::new(
        JobQueue::new(&config.paths.jobs_db)
            .await
            .map_err(|e| std::io::Error::other(format!("Job queue init failed: {}", e)))?,
    );
    let observer: Arc<dyn SchedulerObserver> = Arc::new(QueueObserver::new(job_queue.clone()));
    let scheduler = Arc::new(Scheduler::with_observer(worker_count, Some(observer)));

    let redis_config = redis_runtime::RedisRuntimeConfig::from_server(&config.server);
    let redis_pool = config.server.redis_url.as_deref().and_then(|url| {
        match redis_runtime::RedisPool::new(url, redis_config.clone()) {
            Ok(pool) => Some(pool),
            Err(error) => {
                warn!("Redis disabled because its URL is invalid: {}", error);
                None
            }
        }
    });
    if let Some(redis_url) = config.server.redis_url.clone() {
        let bus = event_bus.clone();
        let redis_config = redis_config.clone();
        tokio::spawn(async move {
            if let Err(err) =
                distributed_bus::start_redis_bridge(bus, redis_url, redis_config).await
            {
                warn!("Redis event bus bridge failed: {}", err);
            }
        });
    }

    // Init Cameras
    let mut camera_manager = CameraManager::new();
    let mock_enabled = config.hardware.mock_devices.unwrap_or(false);
    match camera_manager.reconcile_cameras(mock_enabled).await {
        Ok(report) => info!(
            "Auto-detected {} cameras (Added: {:?})",
            camera_manager.cameras.len(),
            report.added
        ),
        Err(e) => warn!("Camera auto-detection failed: {}", e),
    }
    let camera_manager = Arc::new(AsyncMutex::new(camera_manager));
    let inventory = Arc::new(
        Inventory::new(&config.paths.inventory_db)
            .map_err(|e| std::io::Error::other(format!("Inventory init failed: {}", e)))?,
    );
    {
        let cm = camera_manager.lock().await;
        for profile in cm.registry.profiles.values() {
            if let Some(cal) = &profile.calibration {
                if let (Some(matrix), Some(dist), Some(width), Some(height)) = (
                    cal.intrinsics.clone(),
                    cal.distortion.clone(),
                    cal.image_width,
                    cal.image_height,
                ) {
                    let rms = cal.rms_error.unwrap_or(0.0);
                    let _ = inventory.upsert_camera_calibration(
                        &profile.id,
                        matrix,
                        dist,
                        rms,
                        width,
                        height,
                    );
                }
            }
        }
    }

    // Init Turntable (Background Scan Loop)
    let turntable: Arc<AsyncMutex<Option<Box<dyn Turntable>>>> = Arc::new(AsyncMutex::new(None));
    let turntable_status = Arc::new(Mutex::new("Scanning...".to_string()));
    let turntable_moving = Arc::new(Mutex::new(false));

    let tt_clone = turntable.clone();
    let status_clone = turntable_status.clone();
    let tt_config = config.hardware.clone();
    let tt_feedback_config =
        tt_config
            .turntable_feedback
            .as_ref()
            .map(|cfg| TurntableFeedbackConfig {
                query_command: cfg.query_command.clone(),
                query_timeout: std::time::Duration::from_millis(
                    cfg.query_timeout_ms.unwrap_or(800),
                ),
                max_angle_error_deg: cfg.max_angle_error_deg.unwrap_or(2.0),
                auto_correct: cfg.auto_correct.unwrap_or(false),
            });
    let eb_turntable = event_bus.clone(); // Clone for turntable loop

    tokio::spawn(async move {
        loop {
            // Check if already connected
            let mut is_still_connected = false;
            if tt_clone.lock().await.is_some() {
                if let Some(tt) = tt_clone.lock().await.as_ref() {
                    if tt.is_connected().await {
                        is_still_connected = true;
                    }
                }
            }

            if is_still_connected {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            } else {
                // Was connected, but now lost? Or just wasn't connected.
                // If we have a Some() but is_connected is false, we should drop it.
                let mut tt_lock = tt_clone.lock().await;
                if tt_lock.is_some() {
                    warn!("Turntable connection lost! Re-scanning...");
                    *tt_lock = None;
                    *status_clone.lock().unwrap() = "Disconnected".to_string();
                    eb_turntable.publish(SystemEvent::TurntableStatus {
                        connected: false,
                        angle: 0.0,
                        moving: false,
                    });
                }
            }

            info!("Starting Turntable Scan...");
            *status_clone.lock().unwrap() = "Scanning...".to_string();

            // Try Foldio360
            if tt_config.turntable_type == "foldio360" || tt_config.turntable_type == "auto" {
                let mut foldio = Foldio360::new();
                if let Some(feedback) = tt_feedback_config.clone() {
                    foldio.set_feedback_config(feedback);
                }
                if foldio.connect().await.is_ok() {
                    info!("Foldio360 Connected!");
                    *tt_clone.lock().await = Some(Box::new(foldio) as Box<dyn Turntable>);
                    *status_clone.lock().unwrap() = "Foldio360".to_string();

                    eb_turntable.publish(SystemEvent::DeviceConnected {
                        kind: "turntable".to_string(),
                        id: "Foldio360".to_string(),
                    });
                    eb_turntable.publish(SystemEvent::TurntableStatus {
                        connected: true,
                        angle: 0.0,
                        moving: false,
                    });

                    continue; // Connected!
                }
            }

            // Try Serial if config present
            if let Some(port) = &tt_config.serial_port {
                let mut serial = SerialTurntable::new(port, 115200); // Default baud
                if let Some(feedback) = tt_feedback_config.clone() {
                    serial.set_feedback_config(feedback);
                }
                if serial.connect().await.is_ok() {
                    info!("Serial Turntable Connected on {}", port);
                    *tt_clone.lock().await = Some(Box::new(serial) as Box<dyn Turntable>);
                    *status_clone.lock().unwrap() = "Serial".to_string();

                    eb_turntable.publish(SystemEvent::DeviceConnected {
                        kind: "turntable".to_string(),
                        id: "Serial".to_string(),
                    });
                    eb_turntable.publish(SystemEvent::TurntableStatus {
                        connected: true,
                        angle: 0.0,
                        moving: false,
                    });

                    continue; // Connected!
                }
            }

            *status_clone.lock().unwrap() = "Not Found".to_string();
            // info!("Turntable Scan Complete (None found). Retrying in 10s...");
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });

    // Background Camera Scanner
    let cm_clone = camera_manager.clone();
    let eb_clone = event_bus.clone();
    let mock_clone = mock_enabled;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            // Auto Update Logic
            {
                let mut cm = cm_clone.lock().await;
                match cm.reconcile_cameras(mock_clone).await {
                    Ok(report) => {
                        // Broadcast added
                        for id in report.added {
                            info!("Broadcast Connected: {}", id);
                            eb_clone.publish(SystemEvent::DeviceConnected {
                                kind: "camera".to_string(),
                                id,
                            });
                        }
                        // Broadcast removed
                        for id in report.removed {
                            info!("Broadcast Disconnected: {}", id);
                            eb_clone.publish(SystemEvent::DeviceDisconnected { id });
                        }
                    }
                    Err(_e) => {
                        // Reduce log spam?
                        // warn!("Camera reconciliation failed: {}", e);
                    }
                }
            }
        }
    });

    // System Stats (Background Update)
    let system_stats = Arc::new(Mutex::new(state::SystemStats {
        cpu_usage: 0.0,
        memory_used_mb: 0,
        memory_total_mb: 0,
        disk_free_gb: 0,
    }));
    let stats_clone = system_stats.clone();

    tokio::task::spawn_blocking(move || {
        use sysinfo::{Disks, System};

        let mut sys = System::new_all();
        let mut disks = Disks::new_with_refreshed_list();
        loop {
            sys.refresh_cpu_all();
            sys.refresh_memory();
            disks.refresh(false);

            let cpu_usage = sys.global_cpu_usage();
            let used_mem = sys.used_memory() / 1024 / 1024;
            let total_mem = sys.total_memory() / 1024 / 1024;

            let mut free_space = 0;
            for disk in disks.list() {
                free_space += disk.available_space();
            }
            let disk_gb = free_space / 1024 / 1024 / 1024;

            {
                let mut lock = stats_clone.lock().unwrap();
                lock.cpu_usage = cpu_usage;
                lock.memory_used_mb = used_mem;
                lock.memory_total_mb = total_mem;
                lock.disk_free_gb = disk_gb;
            }

            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });

    // Initialize phone state for slave phone management
    let max_phone_upload_bytes = config
        .server
        .max_phone_upload_bytes
        .unwrap_or(25 * 1024 * 1024);
    let max_phone_upload_rate_bytes_per_minute = config
        .server
        .max_phone_upload_rate_bytes_per_minute
        .unwrap_or(200 * 1024 * 1024);
    let max_phone_total_bytes = config
        .server
        .max_project_bytes
        .unwrap_or(100 * 1024 * 1024 * 1024);
    let min_free_bytes = config
        .server
        .min_free_bytes
        .unwrap_or(2 * 1024 * 1024 * 1024);
    let phone_state = Arc::new(SlavePhoneState::new(
        "./uploads/phones",
        max_phone_upload_bytes,
        max_phone_upload_rate_bytes_per_minute,
        max_phone_total_bytes,
        min_free_bytes,
    ));

    let audit_log_path = config.paths.projects_dir.join("_audit").join("audit.log");
    let audit_anchor_url = config
        .privacy
        .audit_anchor_url
        .clone()
        .filter(|url| !url.trim().is_empty());
    let audit_anchor_required = config
        .privacy
        .audit_anchor_required
        .unwrap_or(is_production);
    config.privacy.audit_anchor_required = Some(audit_anchor_required);
    if audit_anchor_required && audit_anchor_url.is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Audit anchor is required in production. Set privacy.audit_anchor_url.",
        ));
    }
    let audit_anchor_timeout_seconds = config.privacy.audit_anchor_timeout_seconds.unwrap_or(3);
    let audit = Arc::new(
        audit::AuditLog::new(
            audit_log_path,
            audit_anchor_url.map(|url| audit::AuditAnchorConfig {
                url,
                timeout_seconds: audit_anchor_timeout_seconds,
                required: audit_anchor_required,
            }),
        )
        .map_err(|e| std::io::Error::other(e.to_string()))?,
    );
    let license_gate = Arc::new(Mutex::new(licensing::LicenseGate::initialize()));
    let adaptive_capture =
        api::adaptive_capture::AdaptiveCaptureSessions::load(&config.paths.projects_dir)
            .map_err(|error| std::io::Error::other(error.to_string()))?;

    let state = web::Data::new(AppState {
        config: config.clone(),
        auth: auth.clone(),
        event_bus,
        scheduler,
        job_queue: job_queue.clone(),
        camera_manager,
        inventory,
        turntable,
        turntable_status,
        turntable_moving,
        system_stats,
        scan_wizard: Arc::new(AsyncMutex::new(ScanWizardState::default())),
        intervalometer: Arc::new(AsyncMutex::new(IntervalometerState::new())),
        adaptive_capture: Arc::new(AsyncMutex::new(adaptive_capture)),
        calibration_session: Arc::new(AsyncMutex::new(state::CalibrationSession::default())),
        audit,
        license_gate,
        redis_pool,
        phone_state: Some(phone_state.clone()),
    });

    retention::spawn_retention_task(state.clone());
    at_rest::spawn_encryption_task(state.clone());
    let retry_interval =
        std::time::Duration::from_secs(config.server.job_retry_interval_seconds.unwrap_or(30));
    let queue_bootstrap = job_queue.clone();
    let scheduler_bootstrap = state.scheduler.clone();
    tokio::spawn(async move {
        if let Ok(pending) = queue_bootstrap.load_pending_jobs().await {
            for job in pending {
                schedule_queue_job(queue_bootstrap.clone(), scheduler_bootstrap.clone(), job).await;
            }
        }
        let mut interval = tokio::time::interval(retry_interval);
        loop {
            interval.tick().await;
            if let Ok(retries) = queue_bootstrap.load_retry_jobs().await {
                for job in retries {
                    schedule_queue_job(queue_bootstrap.clone(), scheduler_bootstrap.clone(), job)
                        .await;
                }
            }
        }
    });

    if is_production && config.server.cookie_secure != Some(true) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "TRUESHOT_ENV=production requires server.cookie_secure=true",
        ));
    }
    let csrf_required = config.server.csrf_required.unwrap_or(is_production);
    let tls_proxy = config.server.tls_proxy.unwrap_or(false);
    let tls_configured = config.server.tls.is_some();
    if is_production && !tls_configured && !tls_proxy {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "TRUESHOT_ENV=production requires server.tls or server.tls_proxy=true",
        ));
    }
    if tls_proxy {
        let public_base_url = config.server.public_base_url.clone().unwrap_or_default();
        if !public_base_url.starts_with("https://") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "server.tls_proxy=true requires server.public_base_url to start with https://",
            ));
        }
    }
    if is_production {
        if let Some(public_base_url) = config.server.public_base_url.as_ref() {
            if !public_base_url.starts_with("https://") {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "TRUESHOT_ENV=production requires server.public_base_url to use https://",
                ));
            }
        }
    }
    let default_dev_origins = vec![
        "http://localhost:5173".to_string(),
        "http://127.0.0.1:5173".to_string(),
        "http://localhost:3000".to_string(),
        "http://127.0.0.1:3000".to_string(),
    ];
    let allowed_origins = config.server.allowed_origins.clone().unwrap_or_else(|| {
        if is_production {
            Vec::new()
        } else {
            default_dev_origins
        }
    });
    if is_production && allowed_origins.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "TRUESHOT_ENV=production requires server.allowed_origins to be configured",
        ));
    }

    info!(
        "Starting TrueShot Server (Actix) on {}:{}",
        config.server.host, config.server.port
    );

    let enable_hsts = is_production && (tls_configured || tls_proxy);
    let hsts_max_age = config.server.hsts_max_age_seconds.unwrap_or(31536000);
    let hsts_include_subdomains = config.server.hsts_include_subdomains.unwrap_or(true);
    let hsts_preload = config.server.hsts_preload.unwrap_or(false);
    let mut hsts_value = format!("max-age={}", hsts_max_age);
    if hsts_include_subdomains {
        hsts_value.push_str("; includeSubDomains");
    }
    if hsts_preload {
        hsts_value.push_str("; preload");
    }

    let metrics_enabled = config.server.metrics_enabled.unwrap_or(is_production);
    let metrics_path = config
        .server
        .metrics_path
        .clone()
        .unwrap_or_else(|| "/api/metrics".to_string());
    let prometheus = PrometheusMetricsBuilder::new("trueshot")
        .endpoint(metrics_path.as_str())
        .build()
        .map_err(|e| std::io::Error::other(format!("Metrics init failed: {}", e)))?;

    // 3. Server
    let server = HttpServer::new(move || {
        let mut cors = Cors::default()
            .allowed_methods(vec!["GET", "POST", "PUT", "DELETE", "OPTIONS"])
            .allowed_headers(vec![
                actix_web::http::header::AUTHORIZATION,
                actix_web::http::header::ACCEPT,
                actix_web::http::header::CONTENT_TYPE,
                actix_web::http::header::HeaderName::from_static("x-api-key"),
            ])
            .supports_credentials()
            .max_age(3600);
        for origin in &allowed_origins {
            cors = cors.allowed_origin(origin);
        }

        let phone_state_data = web::Data::from(phone_state.clone());

        App::new()
            .wrap(cors)
            .wrap(
                DefaultHeaders::new()
                    .add(("X-Content-Type-Options", "nosniff"))
                    .add(("Referrer-Policy", "strict-origin-when-cross-origin"))
                    .add(("X-Frame-Options", "SAMEORIGIN")),
            )
            .wrap(Condition::new(
                enable_hsts,
                DefaultHeaders::new().add(("Strict-Transport-Security", hsts_value.clone())),
            ))
            .wrap(Condition::new(metrics_enabled, prometheus.clone()))
            .wrap(trace_middleware::TraceContext::new())
            .wrap(AuthMiddleware::new(
                auth.clone(),
                config.server.api_key.clone(),
                csrf_required,
                rate_limiter.clone(),
            ))
            .app_data(state.clone())
            .app_data(phone_state_data.clone())
            // General
            .service(api::general::health_check)
            .service(api::general::get_logs)
            .service(api::general::export_logs)
            .service(api::system::get_system_stats)
            .configure(api::health::configure)
            // Audit
            .service(api::audit::get_audit)
            // Auth
            .service(api::auth::guest_token)
            .service(api::auth::bootstrap_status)
            .service(api::auth::bootstrap_admin)
            .service(api::auth::login)
            .service(api::auth::create_session)
            .service(api::auth::clear_session)
            .service(api::auth::refresh_session)
            .service(api::auth::logout_all)
            .service(api::auth::create_api_token)
            .service(api::auth::list_api_tokens)
            .service(api::auth::revoke_api_token)
            .service(api::auth::pairing_start)
            .service(api::auth::pairing_claim)
            // Licensing
            .service(api::license::get_license_status)
            .service(api::license::get_license_bundles)
            .service(api::license::get_license_entitlements)
            .service(api::license::get_license_catalog)
            .service(api::license::get_license_tiers)
            .service(api::license::create_license_trial)
            .service(api::license::create_license_trial_self)
            .service(api::license::import_license)
            .service(api::license::get_license_devices)
            .service(api::license::activate_license_device)
            .service(api::license::activate_license_key)
            .service(api::license::deactivate_license_device)
            // Projects
            .service(api::project::get_projects)
            .service(api::project::get_projects_legacy)
            .service(api::project::create_project)
            .service(api::project::purge_project_raw)
            .service(api::project::open_project_fs)
            .service(api::project::import_model)
            .service(api::project::download_raw_file)
            .service(api::project::download_output_file)
            .service(api::project::download_fusion_artifact)
            .service(api::project::create_fusion_edit)
            .service(api::project::download_processed_file)
            .service(api::project::get_imu_diagnostics)
            .service(api::project::get_project_license)
            .service(api::project::update_project_license)
            .service(api::project::list_project_assets)
            .service(api::project::list_fusion_reports)
            .service(api::project::encrypt_project)
            .service(api::project::decrypt_project)
            // Annotations
            .configure(api::annotations::configure)
            // Edits
            .service(api::edits::edit_mesh)
            .service(api::edits::edit_splat)
            .service(api::edits::get_edit_history)
            // Share links
            .service(api::share::create_share_link)
            .service(api::share::get_share_link)
            .service(api::share::get_share_asset)
            .service(api::share::get_share_analytics)
            .service(api::share::set_share_public)
            .service(api::share::get_share_public)
            .service(api::share::list_public_shares)
            .service(api::share::get_share_annotations)
            .service(api::share::share_card)
            .service(api::share::redirect_short_link)
            // Scan Wizard API
            .service(api::scan::get_background_status)
            .service(api::scan::capture_background)
            .service(api::scan::get_detection_status)
            .service(api::scan::get_quality_status)
            .service(api::scan::get_quality_history)
            .service(api::scan::get_uncertainty_map)
            .service(api::scan::analyze_object)
            .service(api::scan::compute_scan_plan)
            .service(api::scan::start_scan)
            .service(api::scan::stop_scan)
            .service(api::scan::get_scan_progress)
            .service(api::scan::execute_step)
            .service(api::scan::trigger_capture)
            .service(api::scan::get_sdcard_status)
            .service(api::scan::import_from_sdcard)
            // Calibration
            .configure(api::calibration::configure)
            // Jobs
            .service(api::jobs::submit_job)
            .service(api::jobs::list_jobs)
            .service(api::jobs::get_job)
            // Hardware
            .service(api::hardware::get_cameras)
            .service(api::hardware::update_camera_nickname)
            .service(api::hardware::camera_ptz)
            .service(api::hardware::camera_stream)
            .service(api::hardware::camera_focus_point)
            .service(api::hardware::camera_autofocus)
            .service(api::hardware::set_camera_config)
            .service(api::hardware::camera_drive_focus)
            .service(api::hardware::capture_photo)
            .service(api::hardware::capture_hdr_bracket)
            .service(api::hardware::capture_focus_stack)
            .service(api::hardware::capture_hdr_focus_stack)
            .service(api::adaptive_capture::start_adaptive_capture)
            .service(api::adaptive_capture::assimilate_adaptive_capture)
            .service(api::adaptive_capture::get_adaptive_capture)
            .service(api::adaptive_capture::get_adaptive_capture_provenance)
            .service(api::adaptive_capture::terminate_adaptive_capture)
            .service(api::hardware::set_camera_enabled)
            .service(api::hardware::start_intervalometer)
            .service(api::hardware::stop_intervalometer)
            .service(api::hardware::get_intervalometer_status)
            .service(api::hardware::get_turntable_status)
            .service(api::hardware::turntable_home)
            .service(api::hardware::turntable_rotate)
            .service(api::hardware::scan_hardware)
            // Guest + Slave Phone Portal
            .configure(guest::configure)
            .configure(|cfg| guest::slave::configure(cfg, phone_state_data.clone()))
            // Unified Device Manager
            .configure(api::devices::configure)
            // Cloud Storage
            .configure(api::storage::configure)
            // WS
            .route("/api/ws", web::get().to(api::websocket::ws_index))
            // XR
            .service(api::xr::start_xr_session)
            .service(api::xr::complete_xr_session)
            // API Documentation
            .service(api::docs::get_api_docs)
            // Static Assets (e.g. demo.ply) - served at /assets/
            .service(if is_production {
                fs::Files::new("/assets", "./trueshot-server/static/assets")
            } else {
                fs::Files::new("/assets", "./trueshot-server/static/assets").show_files_listing()
            })
            // Root info endpoint
            .route("/", web::get().to(|| async {
                HttpResponse::Ok().json(serde_json::json!({
                    "name": "TrueShot API",
                    "version": "6.8.0",
                    "docs": "/api/docs",
                    "frontend": "http://localhost:5173"
                }))
            }))
    });

    let server = if let Some(tls) = config.server.tls.as_ref() {
        let tls_config = tls::load_rustls_config(&tls.cert_path, &tls.key_path)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;
        server.bind_rustls_0_23(
            (config.server.host.as_str(), config.server.port),
            tls_config,
        )?
    } else {
        server.bind((config.server.host.as_str(), config.server.port))?
    };

    server.run().await
}

fn parse_openapi_args() -> Option<Option<PathBuf>> {
    let mut args = std::env::args().skip(1);
    let mut openapi = false;
    let mut out_path = None;
    while let Some(arg) = args.next() {
        if arg == "--openapi" {
            openapi = true;
        } else if arg == "--openapi-out" {
            if let Some(path) = args.next() {
                out_path = Some(PathBuf::from(path));
            }
        }
    }
    if openapi || out_path.is_some() {
        Some(out_path)
    } else {
        None
    }
}

async fn schedule_queue_job(
    job_queue: Arc<JobQueue>,
    scheduler: Arc<Scheduler>,
    job: QueueJobPayload,
) {
    if let Err(err) = job_queue.mark_pending(job.id).await {
        tracing::warn!("Failed to mark job pending {}: {}", job.id, err);
    }
    let job_payload = job.payload.clone();
    let unified = match crate::api::jobs::build_job_from_payload(&job.kind, job_payload) {
        Ok(job) => job,
        Err(err) => {
            let _ = job_queue
                .sync_job_info(job.id, "failed", 0.0, None, Some(Utc::now()), Some(err))
                .await;
            return;
        }
    };
    if let Err(err) = scheduler.submit_with_id(job.id, unified).await {
        if !err.to_string().contains("Job id already exists") {
            let _ = job_queue
                .sync_job_info(
                    job.id,
                    "failed",
                    0.0,
                    None,
                    Some(Utc::now()),
                    Some(err.to_string()),
                )
                .await;
        }
    }
}
