use config::{Config, ConfigError, File};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub paths: PathConfig,
    pub hardware: HardwareConfig,
    #[serde(default)]
    pub privacy: PrivacyConfig,
    #[serde(default)]
    pub legal: LegalConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub api_key: Option<String>,
    pub hmac_secret_path: Option<PathBuf>,
    pub allowed_origins: Option<Vec<String>>,
    pub trusted_proxy_cidrs: Option<Vec<String>>,
    pub cookie_secure: Option<bool>,
    pub public_base_url: Option<String>,
    pub frontend_base_url: Option<String>,
    pub tls: Option<TlsConfig>,
    pub tls_proxy: Option<bool>,
    pub hsts_max_age_seconds: Option<u64>,
    pub hsts_include_subdomains: Option<bool>,
    pub hsts_preload: Option<bool>,
    pub telemetry_enabled: Option<bool>,
    pub telemetry_otlp_endpoint: Option<String>,
    pub telemetry_sample_ratio: Option<f64>,
    pub telemetry_service_name: Option<String>,
    pub telemetry_service_version: Option<String>,
    pub metrics_enabled: Option<bool>,
    pub metrics_path: Option<String>,
    pub refresh_token_ttl_seconds: Option<u64>,
    pub csrf_required: Option<bool>,
    pub rate_limit_enabled: Option<bool>,
    pub rate_limit_ip_per_minute: Option<u32>,
    pub rate_limit_ip_burst: Option<u32>,
    pub rate_limit_user_per_minute: Option<u32>,
    pub rate_limit_user_burst: Option<u32>,
    pub job_max_attempts: Option<u32>,
    pub job_retry_interval_seconds: Option<u64>,
    pub admin_token_ttl_seconds: Option<u64>,
    pub guest_token_ttl_seconds: Option<u64>,
    pub max_upload_bytes: Option<u64>,
    pub max_phone_upload_bytes: Option<u64>,
    pub max_phone_upload_rate_bytes_per_minute: Option<u64>,
    pub max_project_bytes: Option<u64>,
    pub min_free_bytes: Option<u64>,
    pub antivirus_command: Option<String>,
    pub antivirus_args: Option<Vec<String>>,
    pub calibration_max_rms: Option<f64>,
    pub calibration_max_age_days: Option<i64>,
    pub calibration_max_deltae: Option<f32>,
    pub redis_url: Option<String>,
    pub redis_connect_timeout_ms: Option<u64>,
    pub redis_response_timeout_ms: Option<u64>,
    pub redis_reconnect_initial_ms: Option<u64>,
    pub redis_reconnect_max_ms: Option<u64>,
    pub redis_event_buffer_capacity: Option<usize>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PathConfig {
    pub projects_dir: PathBuf,
    pub inventory_db: PathBuf,
    pub jobs_db: PathBuf,
    pub auth_db: PathBuf,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HardwareConfig {
    pub camera_indices: Vec<u32>,
    pub turntable_type: String,
    pub serial_port: Option<String>,
    pub mock_devices: Option<bool>,
    pub turntable_diameter_cm: Option<f32>,
    pub turntable_feedback: Option<TurntableFeedbackConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TurntableFeedbackConfig {
    pub query_command: Option<String>,
    pub query_timeout_ms: Option<u64>,
    pub max_angle_error_deg: Option<f32>,
    pub auto_correct: Option<bool>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct PrivacyConfig {
    #[serde(default)]
    pub retention_raw_days: Option<u32>,
    #[serde(default)]
    pub retention_processed_days: Option<u32>,
    #[serde(default)]
    pub retention_output_days: Option<u32>,
    #[serde(default)]
    pub audit_log_days: Option<u32>,
    #[serde(default)]
    pub redact_device_id: Option<bool>,
    #[serde(default)]
    pub redact_operator_id: Option<bool>,
    #[serde(default)]
    pub redact_session_id: Option<bool>,
    #[serde(default)]
    pub redact_capture_hashes: Option<bool>,
    #[serde(default)]
    pub audit_anchor_url: Option<String>,
    #[serde(default)]
    pub audit_anchor_required: Option<bool>,
    #[serde(default)]
    pub audit_anchor_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub audit_redact_actor: Option<bool>,
    #[serde(default)]
    pub audit_redact_ip: Option<bool>,
    #[serde(default)]
    pub audit_redact_resource: Option<bool>,
    #[serde(default)]
    pub audit_redact_details: Option<bool>,
    #[serde(default)]
    pub audit_redact_keys: Option<Vec<String>>,
    #[serde(default)]
    pub provenance_key_path: Option<PathBuf>,
    #[serde(default)]
    pub encryption_master_key_path: Option<PathBuf>,
    #[serde(default)]
    pub encrypt_at_rest: Option<bool>,
    #[serde(default)]
    pub encrypt_scopes: Option<Vec<String>>,
    #[serde(default)]
    pub encrypt_min_age_seconds: Option<u64>,
    #[serde(default)]
    pub encrypt_sweep_interval_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct LegalConfig {
    pub license_title: Option<String>,
    pub license_url: Option<String>,
    pub data_ownership: Option<String>,
    pub export_rights: Option<String>,
}

impl AppConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let s = Config::builder()
            .add_source(File::with_name("config"))
            // Add environment variables (TRUESHOT_SERVER__PORT=3001)
            .add_source(config::Environment::with_prefix("TRUESHOT").separator("__"))
            .build()?;

        s.try_deserialize()
    }
}
