use crate::audit::AuditLog;
use crate::auth::AuthManager;
use crate::config::AppConfig;
use crate::guest::SlavePhoneState;
use crate::intervalometer::IntervalometerState;
use crate::queue::JobQueue;
use crate::redis_runtime::RedisPool;
use crate::scan_wizard::ScanWizardState;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;
use trueshot_core::events::EventBus;
use trueshot_core::inventory::Inventory;
use trueshot_core::scheduler::Scheduler;
use trueshot_device_manager::{CameraManager, Turntable};

pub struct SystemStats {
    pub cpu_usage: f32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub disk_free_gb: u64,
}

#[derive(Default)]
pub struct CalibrationSession {
    pub captured_frames: Vec<PathBuf>,
    pub camera_id: Option<String>,
}

pub struct AppState {
    pub config: AppConfig,
    pub auth: Arc<AuthManager>,
    pub event_bus: Arc<EventBus>,
    pub scheduler: Arc<Scheduler>,
    pub job_queue: Arc<JobQueue>,
    pub fusion_revision_executor: Arc<crate::fusion_revision::FusionRevisionExecutor>,
    pub camera_manager: Arc<AsyncMutex<CameraManager>>,
    pub inventory: Arc<Inventory>,
    pub turntable: Arc<AsyncMutex<Option<Box<dyn Turntable>>>>,
    pub turntable_status: Arc<Mutex<String>>,
    pub turntable_moving: Arc<Mutex<bool>>,
    pub system_stats: Arc<Mutex<SystemStats>>,
    pub scan_wizard: Arc<AsyncMutex<ScanWizardState>>,
    pub intervalometer: Arc<AsyncMutex<IntervalometerState>>,
    pub adaptive_capture: Arc<AsyncMutex<crate::api::adaptive_capture::AdaptiveCaptureSessions>>,
    pub calibration_session: Arc<AsyncMutex<CalibrationSession>>,
    pub project_file_mutations: Arc<AsyncMutex<()>>,
    pub audit: Arc<AuditLog>,
    pub license_gate: Arc<Mutex<crate::licensing::LicenseGate>>,
    pub redis_pool: Option<Arc<RedisPool>>,
    /// Phone state for guest/slave phone management
    pub phone_state: Option<Arc<SlavePhoneState>>,
}
