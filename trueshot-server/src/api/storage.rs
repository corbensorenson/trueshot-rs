//! Storage API - Cloud Drive & NAS Management
//!
//! Endpoints for managing storage connections:
//! - OAuth flow for Google Drive, Dropbox, OneDrive
//! - NAS/S3 configuration
//! - Storage browsing and sync

use actix_web::{delete, get, post, web, HttpRequest, HttpResponse, Responder};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64_URL, Engine as _};
use chrono::{DateTime, Utc};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tar::{Archive, Builder};
use tokio::sync::RwLock;
use trueshot_core::cloud_client::{CloudStorage, S3Client};
use trueshot_core::security::{StoredToken, TokenStore};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::auth::require_admin;
use crate::config::AppConfig;
use crate::licensing::require_license_feature;
use crate::state::AppState;
use trueshot_core::licensing::Feature;

const OAUTH_STATE_TTL_MINUTES: i64 = 10;

fn new_state_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    B64_URL.encode(bytes)
}

// ============================================================================
// Types
// ============================================================================

/// Cloud storage provider
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloudProvider {
    GoogleDrive,
    Dropbox,
    OneDrive,
    ICloudDrive,
    S3,
    Gcs,
    Azure,
    Nas,
}

impl CloudProvider {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::GoogleDrive => "Google Drive",
            Self::Dropbox => "Dropbox",
            Self::OneDrive => "OneDrive",
            Self::ICloudDrive => "iCloud Drive",
            Self::S3 => "Amazon S3",
            Self::Gcs => "Google Cloud Storage",
            Self::Azure => "Azure Blob Storage",
            Self::Nas => "Network Storage",
        }
    }

    pub fn requires_oauth(&self) -> bool {
        matches!(self, Self::GoogleDrive | Self::Dropbox | Self::OneDrive)
    }
}

#[derive(Debug, Clone)]
struct OAuthAvailability {
    available: bool,
    missing_fields: Vec<&'static str>,
    env_hint: &'static str,
}

fn oauth_env_hint(provider: CloudProvider) -> &'static str {
    match provider {
        CloudProvider::GoogleDrive => "GOOGLE_CLIENT_ID/GOOGLE_CLIENT_SECRET",
        CloudProvider::Dropbox => "DROPBOX_CLIENT_ID/DROPBOX_CLIENT_SECRET",
        CloudProvider::OneDrive => "ONEDRIVE_CLIENT_ID/ONEDRIVE_CLIENT_SECRET",
        _ => "N/A",
    }
}

fn oauth_config_availability(provider: CloudProvider, config: &OAuthConfig) -> OAuthAvailability {
    let mut missing_fields = Vec::new();
    if config.client_id.trim().is_empty() {
        missing_fields.push("client_id");
    }
    if config.client_secret.trim().is_empty() {
        missing_fields.push("client_secret");
    }
    if config.redirect_uri.trim().is_empty() {
        missing_fields.push("redirect_uri");
    }
    OAuthAvailability {
        available: missing_fields.is_empty(),
        missing_fields,
        env_hint: oauth_env_hint(provider),
    }
}

/// OAuth configuration for cloud providers
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OAuthConfig {
    pub client_id: String,
    #[serde(skip_serializing)]
    pub client_secret: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

/// OAuth tokens (stored securely)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OAuthTokens {
    pub access_token: String,
    #[serde(skip_serializing)]
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub token_type: String,
}

/// Storage connection state
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StorageConnection {
    pub id: String,
    pub provider: CloudProvider,
    pub name: String,
    pub email: Option<String>,
    pub status: StorageConnectionStatus,
    pub connected_at: DateTime<Utc>,
    pub last_sync: Option<DateTime<Utc>>,
    pub used_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub base_path: String,
    pub endpoint: Option<String>,
    pub bucket: Option<String>,
    pub region: Option<String>,
    #[serde(skip_serializing)]
    pub tokens: Option<OAuthTokens>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StorageConnectionStatus {
    Connected,
    Disconnected,
    Syncing,
    Error,
    NeedsReauth,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupJobStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupJobType {
    Backup,
    Restore,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BackupJob {
    pub id: String,
    pub job_type: BackupJobType,
    pub project_id: String,
    pub label: Option<String>,
    pub storage_id: Option<String>,
    pub remote_path: Option<String>,
    pub status: BackupJobStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub archive_path: Option<String>,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
    pub error: Option<String>,
    pub restore_project_id: Option<String>,
}

/// Storage state manager
#[derive(Clone)]
pub struct StorageState {
    pub connections: Arc<RwLock<HashMap<String, StorageConnection>>>,
    pub oauth_configs: Arc<RwLock<HashMap<CloudProvider, OAuthConfig>>>,
    pub oauth_states: Arc<RwLock<HashMap<String, OAuthState>>>,
    pub http_client: reqwest::Client,
    pub token_store: Arc<TokenStore>,
    pub connections_path: std::path::PathBuf,
    pub backups: Arc<RwLock<HashMap<String, BackupJob>>>,
    pub backups_path: std::path::PathBuf,
    pub frontend_base_url: String,
}

#[derive(Clone, Debug)]
pub struct OAuthState {
    pub provider: CloudProvider,
    pub created_at: DateTime<Utc>,
}

impl StorageState {
    pub fn new(config: &AppConfig) -> Result<Self, trueshot_core::security::TokenStoreError> {
        let token_dir = token_store_dir();
        let token_store = Arc::new(TokenStore::new(&token_dir)?);
        let connections_path = token_dir.join("storage.connections.json");
        let backups_path = token_dir.join("storage.backups.json");
        let oauth_redirect_base = oauth_redirect_base_url(config);
        let frontend_base_url = frontend_base_url(config);
        let oauth_configs = Self::default_oauth_configs(&oauth_redirect_base);
        Self::log_oauth_config_status(&oauth_configs);
        let connections =
            load_connections(&connections_path, &token_store, &oauth_configs).unwrap_or_default();
        let backups = load_backups(&backups_path).unwrap_or_default();

        Ok(Self {
            connections: Arc::new(RwLock::new(connections)),
            oauth_configs: Arc::new(RwLock::new(oauth_configs)),
            oauth_states: Arc::new(RwLock::new(HashMap::new())),
            http_client: reqwest::Client::new(),
            token_store,
            connections_path,
            backups: Arc::new(RwLock::new(backups)),
            backups_path,
            frontend_base_url,
        })
    }

    fn default_oauth_configs(oauth_redirect_base: &str) -> HashMap<CloudProvider, OAuthConfig> {
        let mut configs = HashMap::new();

        // These would be loaded from environment/config in production
        // Using placeholders - users need to provide their own OAuth credentials

        configs.insert(
            CloudProvider::GoogleDrive,
            OAuthConfig {
                client_id: std::env::var("GOOGLE_CLIENT_ID").unwrap_or_default(),
                client_secret: std::env::var("GOOGLE_CLIENT_SECRET").unwrap_or_default(),
                redirect_uri: format!(
                    "{}/api/storage/oauth/google_drive/callback",
                    oauth_redirect_base
                ),
                scopes: vec![
                    "https://www.googleapis.com/auth/drive.file".to_string(),
                    "https://www.googleapis.com/auth/drive.metadata.readonly".to_string(),
                ],
            },
        );

        configs.insert(
            CloudProvider::Dropbox,
            OAuthConfig {
                client_id: std::env::var("DROPBOX_CLIENT_ID").unwrap_or_default(),
                client_secret: std::env::var("DROPBOX_CLIENT_SECRET").unwrap_or_default(),
                redirect_uri: format!("{}/api/storage/oauth/dropbox/callback", oauth_redirect_base),
                scopes: vec![
                    "files.content.read".to_string(),
                    "files.content.write".to_string(),
                ],
            },
        );

        configs.insert(
            CloudProvider::OneDrive,
            OAuthConfig {
                client_id: std::env::var("ONEDRIVE_CLIENT_ID").unwrap_or_default(),
                client_secret: std::env::var("ONEDRIVE_CLIENT_SECRET").unwrap_or_default(),
                redirect_uri: format!(
                    "{}/api/storage/oauth/onedrive/callback",
                    oauth_redirect_base
                ),
                scopes: vec!["Files.ReadWrite".to_string(), "offline_access".to_string()],
            },
        );

        configs
    }

    fn log_oauth_config_status(configs: &HashMap<CloudProvider, OAuthConfig>) {
        for provider in [
            CloudProvider::GoogleDrive,
            CloudProvider::Dropbox,
            CloudProvider::OneDrive,
        ] {
            match configs.get(&provider) {
                Some(config) => {
                    let availability = oauth_config_availability(provider, config);
                    if !availability.available {
                        tracing::warn!(
                            "OAuth credentials missing for {} (set {}).",
                            provider.display_name(),
                            availability.env_hint
                        );
                    }
                }
                None => {
                    tracing::warn!(
                        "OAuth configuration missing for {}.",
                        provider.display_name()
                    );
                }
            }
        }
    }
}

// ============================================================================
// API Endpoints
// ============================================================================

/// List available storage providers
#[utoipa::path(
    get,
    path = "/api/storage/providers",
    tag = "storage",
    responses(
        (status = 200, description = "Provider list", body = serde_json::Value)
    )
)]
#[get("/api/storage/providers")]
pub async fn list_providers(req: HttpRequest, state: web::Data<StorageState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    struct ProviderInfo {
        provider: CloudProvider,
        id: &'static str,
        icon: &'static str,
        description: &'static str,
        mac_only: bool,
    }

    let infos = [
        ProviderInfo {
            provider: CloudProvider::GoogleDrive,
            id: "google_drive",
            icon: "google",
            description: "Connect your Google Drive for cloud backup",
            mac_only: false,
        },
        ProviderInfo {
            provider: CloudProvider::Dropbox,
            id: "dropbox",
            icon: "dropbox",
            description: "Sync projects to Dropbox",
            mac_only: false,
        },
        ProviderInfo {
            provider: CloudProvider::OneDrive,
            id: "onedrive",
            icon: "microsoft",
            description: "Connect Microsoft OneDrive",
            mac_only: false,
        },
        ProviderInfo {
            provider: CloudProvider::ICloudDrive,
            id: "icloud",
            icon: "apple",
            description: "Use iCloud Drive (macOS only)",
            mac_only: true,
        },
        ProviderInfo {
            provider: CloudProvider::S3,
            id: "s3",
            icon: "aws",
            description: "S3-compatible object storage",
            mac_only: false,
        },
        ProviderInfo {
            provider: CloudProvider::Gcs,
            id: "gcs",
            icon: "google",
            description: "Google Cloud Storage (S3-compatible)",
            mac_only: false,
        },
        ProviderInfo {
            provider: CloudProvider::Azure,
            id: "azure",
            icon: "azure",
            description: "Azure Blob Storage (S3-compatible)",
            mac_only: false,
        },
        ProviderInfo {
            provider: CloudProvider::Nas,
            id: "nas",
            icon: "server",
            description: "Connect via SMB, NFS, or WebDAV",
            mac_only: false,
        },
    ];

    let configs = state.oauth_configs.read().await;
    let mut providers = Vec::with_capacity(infos.len());
    for info in infos {
        let mut entry = serde_json::json!({
            "id": info.id,
            "name": info.provider.display_name(),
            "icon": info.icon,
            "requires_oauth": info.provider.requires_oauth(),
            "description": info.description,
        });
        if info.mac_only {
            entry["mac_only"] = serde_json::Value::Bool(true);
        }
        if info.provider.requires_oauth() {
            if let Some(config) = configs.get(&info.provider) {
                let availability = oauth_config_availability(info.provider, config);
                entry["available"] = serde_json::Value::Bool(availability.available);
                entry["oauth_configured"] = serde_json::Value::Bool(availability.available);
                if !availability.available {
                    entry["unavailable_reason"] = serde_json::Value::String(format!(
                        "Missing OAuth credentials. Set {}.",
                        availability.env_hint
                    ));
                    entry["missing_fields"] = serde_json::json!(availability.missing_fields);
                    entry["setup_hint"] = serde_json::Value::String(format!(
                        "Configure {} and restart the server.",
                        availability.env_hint
                    ));
                }
            } else {
                entry["available"] = serde_json::Value::Bool(false);
                entry["oauth_configured"] = serde_json::Value::Bool(false);
                entry["unavailable_reason"] =
                    serde_json::Value::String("OAuth config missing".to_string());
            }
        }
        providers.push(entry);
    }

    HttpResponse::Ok().json(providers)
}

/// List connected storage accounts
#[utoipa::path(
    get,
    path = "/api/storage",
    tag = "storage",
    responses(
        (status = 200, description = "Storage connections", body = [StorageConnection]),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/storage")]
pub async fn list_storage(req: HttpRequest, state: web::Data<StorageState>) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    {
        let providers = state.token_store.list_providers().unwrap_or_default();
        let mut connections = state.connections.write().await;
        for provider_key in providers {
            if let Some(provider) = provider_from_connection_id(&provider_key) {
                if connections.contains_key(&provider_key) {
                    continue;
                }
                if let Ok(token) = state.token_store.load_token(&provider_key) {
                    connections.insert(
                        provider_key.clone(),
                        StorageConnection {
                            id: provider_key.clone(),
                            provider,
                            name: provider.display_name().to_string(),
                            email: token.email.clone(),
                            status: StorageConnectionStatus::Connected,
                            connected_at: token.created_at,
                            last_sync: None,
                            used_bytes: None,
                            total_bytes: None,
                            base_path: "/TrueShot".to_string(),
                            endpoint: None,
                            bucket: None,
                            region: None,
                            tokens: None,
                        },
                    );
                }
            }
        }
    }

    let connections = state.connections.read().await;
    let list: Vec<_> = connections
        .values()
        .cloned()
        .map(|mut conn| {
            if conn.provider.requires_oauth() && conn.tokens.is_none() {
                conn.status = StorageConnectionStatus::NeedsReauth;
            }
            conn
        })
        .collect();
    HttpResponse::Ok().json(list)
}

/// Get storage connection by ID
#[utoipa::path(
    get,
    path = "/api/storage/{id}",
    tag = "storage",
    params(("id" = String, Path, description = "Storage id")),
    responses(
        (status = 200, description = "Storage connection", body = StorageConnection),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/storage/{id}")]
pub async fn get_storage(
    req: HttpRequest,
    state: web::Data<StorageState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    let id = path.into_inner();
    let connections = state.connections.read().await;

    match connections.get(&id) {
        Some(conn) => HttpResponse::Ok().json(conn),
        None => HttpResponse::NotFound().body("Storage connection not found"),
    }
}

/// Get OAuth authorization URL
#[utoipa::path(
    get,
    path = "/api/storage/oauth/{provider}/url",
    tag = "storage",
    params(("provider" = String, Path, description = "Provider name")),
    responses(
        (status = 200, description = "OAuth URL", body = serde_json::Value),
        (status = 400, description = "Bad request")
    )
)]
#[get("/api/storage/oauth/{provider}/url")]
pub async fn get_oauth_url(
    req: HttpRequest,
    app_state: web::Data<AppState>,
    state: web::Data<StorageState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) =
        require_license_feature(&app_state, Feature::CloudSyncBackup, "cloud_sync_backup")
    {
        return resp;
    }
    let provider_str = path.into_inner();

    let provider = match provider_str.as_str() {
        "google_drive" => CloudProvider::GoogleDrive,
        "dropbox" => CloudProvider::Dropbox,
        "onedrive" => CloudProvider::OneDrive,
        _ => return HttpResponse::BadRequest().body("Unknown provider"),
    };

    let configs = state.oauth_configs.read().await;
    let config = match configs.get(&provider) {
        Some(c) => c,
        None => return HttpResponse::InternalServerError().body("OAuth not configured"),
    };

    let availability = oauth_config_availability(provider, config);
    if !availability.available {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "oauth_not_configured",
            "message": format!(
                "OAuth credentials not configured for {}. Set {} environment variables.",
                provider.display_name(),
                availability.env_hint
            ),
            "missing_fields": availability.missing_fields,
        }));
    }

    // Generate state token for CSRF protection
    let state_token = new_state_token();
    {
        let mut states = state.oauth_states.write().await;
        // Clean up expired states
        let cutoff = Utc::now() - chrono::Duration::minutes(OAUTH_STATE_TTL_MINUTES);
        states.retain(|_, v| v.created_at > cutoff);
        states.insert(
            state_token.clone(),
            OAuthState {
                provider,
                created_at: Utc::now(),
            },
        );
    }

    let auth_url = match provider {
        CloudProvider::GoogleDrive => {
            format!(
                "https://accounts.google.com/o/oauth2/v2/auth?\
                client_id={}&\
                redirect_uri={}&\
                response_type=code&\
                scope={}&\
                access_type=offline&\
                state={}",
                config.client_id,
                urlencoding::encode(&config.redirect_uri),
                urlencoding::encode(&config.scopes.join(" ")),
                state_token
            )
        }
        CloudProvider::Dropbox => {
            format!(
                "https://www.dropbox.com/oauth2/authorize?\
                client_id={}&\
                redirect_uri={}&\
                response_type=code&\
                token_access_type=offline&\
                state={}",
                config.client_id,
                urlencoding::encode(&config.redirect_uri),
                state_token
            )
        }
        CloudProvider::OneDrive => {
            format!(
                "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?\
                client_id={}&\
                redirect_uri={}&\
                response_type=code&\
                scope={}&\
                state={}",
                config.client_id,
                urlencoding::encode(&config.redirect_uri),
                urlencoding::encode(&config.scopes.join(" ")),
                state_token
            )
        }
        _ => return HttpResponse::BadRequest().body("Provider does not use OAuth"),
    };

    HttpResponse::Ok().json(serde_json::json!({
        "url": auth_url,
        "state": state_token
    }))
}

/// OAuth callback handler
#[derive(Debug, Deserialize, IntoParams, ToSchema)]
pub struct OAuthCallback {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/storage/oauth/{provider}/callback",
    tag = "storage",
    params(("provider" = String, Path, description = "Provider name"), OAuthCallback),
    responses(
        (status = 200, description = "OAuth callback processed", body = serde_json::Value),
        (status = 400, description = "Bad request")
    )
)]
#[get("/api/storage/oauth/{provider}/callback")]
pub async fn oauth_callback(
    app_state: web::Data<AppState>,
    state: web::Data<StorageState>,
    path: web::Path<String>,
    query: web::Query<OAuthCallback>,
) -> impl Responder {
    if let Err(resp) =
        require_license_feature(&app_state, Feature::CloudSyncBackup, "cloud_sync_backup")
    {
        return resp;
    }
    let provider_str = path.into_inner();

    if let Some(error) = &query.error {
        return HttpResponse::BadRequest().body(format!("OAuth error: {}", error));
    }

    let code = match &query.code {
        Some(c) => c,
        None => return HttpResponse::BadRequest().body("Missing authorization code"),
    };
    let state_token = match &query.state {
        Some(s) => s,
        None => return HttpResponse::BadRequest().body("Missing state"),
    };

    let provider = match provider_str.as_str() {
        "google_drive" => CloudProvider::GoogleDrive,
        "dropbox" => CloudProvider::Dropbox,
        "onedrive" => CloudProvider::OneDrive,
        _ => return HttpResponse::BadRequest().body("Unknown provider"),
    };

    {
        let mut states = state.oauth_states.write().await;
        let entry = states.remove(state_token);
        match entry {
            Some(st) if st.provider == provider => {
                let cutoff = Utc::now() - chrono::Duration::minutes(OAUTH_STATE_TTL_MINUTES);
                if st.created_at < cutoff {
                    return HttpResponse::BadRequest().body("OAuth state expired");
                }
            }
            _ => return HttpResponse::BadRequest().body("Invalid OAuth state"),
        }
    }

    let (tokens, email) = match exchange_oauth_code(&state, provider, code).await {
        Ok(result) => result,
        Err(resp) => return resp,
    };

    let connection_id = provider_str.clone();

    let stored = StoredToken {
        provider: provider_str.clone(),
        access_token: tokens.access_token.clone(),
        refresh_token: tokens.refresh_token.clone(),
        expires_at: tokens.expires_at,
        email: email.clone(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    if let Err(e) = state.token_store.save_token(&stored) {
        return HttpResponse::InternalServerError().body(format!("Failed to persist tokens: {e}"));
    }

    let connection = StorageConnection {
        id: connection_id.clone(),
        provider,
        name: provider.display_name().to_string(),
        email,
        status: StorageConnectionStatus::Connected,
        connected_at: Utc::now(),
        last_sync: None,
        used_bytes: None,
        total_bytes: None,
        base_path: "/TrueShot".to_string(),
        endpoint: None,
        bucket: None,
        region: None,
        tokens: Some(tokens),
    };

    let mut connections = state.connections.write().await;
    connections.insert(connection_id.clone(), connection);
    if let Err(err) = persist_connections(&state.connections_path, &connections) {
        return err;
    }

    // Redirect back to frontend with success
    HttpResponse::Found()
        .append_header((
            "Location",
            format!(
                "{}/?storage_connected={}",
                state.frontend_base_url, connection_id
            ),
        ))
        .finish()
}

/// Add storage connection (for non-OAuth like S3/NAS)
#[derive(Debug, Deserialize)]
pub struct AddStorageRequest {
    pub provider: String,
    pub name: String,
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub bucket: Option<String>,
    pub base_path: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/storage",
    tag = "storage",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Storage added", body = StorageConnection),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/storage")]
pub async fn add_storage(
    req: HttpRequest,
    app_state: web::Data<AppState>,
    state: web::Data<StorageState>,
    body: web::Json<AddStorageRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) =
        require_license_feature(&app_state, Feature::CloudSyncBackup, "cloud_sync_backup")
    {
        return resp;
    }
    let provider = match body.provider.as_str() {
        "s3" => CloudProvider::S3,
        "gcs" => CloudProvider::Gcs,
        "azure" => CloudProvider::Azure,
        "nas" => CloudProvider::Nas,
        "icloud" => CloudProvider::ICloudDrive,
        _ => return HttpResponse::BadRequest().body("Use OAuth for this provider"),
    };

    if matches!(
        provider,
        CloudProvider::S3 | CloudProvider::Gcs | CloudProvider::Azure
    ) && body
        .bucket
        .as_ref()
        .map(|b| b.trim().is_empty())
        .unwrap_or(true)
    {
        return HttpResponse::BadRequest().body("Bucket is required for this provider");
    }
    if provider == CloudProvider::Nas
        && body
            .endpoint
            .as_ref()
            .map(|e| e.trim().is_empty())
            .unwrap_or(true)
    {
        return HttpResponse::BadRequest().body("Endpoint is required for NAS");
    }

    let connection_id = format!("{}:{}", body.provider, normalize_storage_id(&body.name));

    let connection = StorageConnection {
        id: connection_id.clone(),
        provider,
        name: body.name.clone(),
        email: None,
        status: StorageConnectionStatus::Connected,
        connected_at: Utc::now(),
        last_sync: None,
        used_bytes: None,
        total_bytes: None,
        base_path: body.base_path.clone().unwrap_or("/TrueShot".to_string()),
        endpoint: body.endpoint.clone(),
        bucket: body.bucket.clone(),
        region: body.region.clone(),
        tokens: None,
    };

    if body.access_key.is_some() || body.secret_key.is_some() {
        let stored = StoredToken {
            provider: connection_id.clone(),
            access_token: body.access_key.clone().unwrap_or_default(),
            refresh_token: body.secret_key.clone(),
            expires_at: None,
            email: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        if let Err(e) = state.token_store.save_token(&stored) {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to persist credentials: {e}"));
        }
    }

    let mut connections = state.connections.write().await;
    connections.insert(connection_id.clone(), connection.clone());
    if let Err(err) = persist_connections(&state.connections_path, &connections) {
        return err;
    }

    HttpResponse::Ok().json(connection)
}

/// Remove storage connection
#[utoipa::path(
    delete,
    path = "/api/storage/{id}",
    tag = "storage",
    params(("id" = String, Path, description = "Storage id")),
    responses(
        (status = 200, description = "Storage removed", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[delete("/api/storage/{id}")]
pub async fn remove_storage(
    req: HttpRequest,
    app_state: web::Data<AppState>,
    state: web::Data<StorageState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) =
        require_license_feature(&app_state, Feature::CloudSyncBackup, "cloud_sync_backup")
    {
        return resp;
    }
    let id = path.into_inner();
    let mut connections = state.connections.write().await;

    match connections.remove(&id) {
        Some(conn) => {
            let _ = state.token_store.delete_token(&id);
            if let Err(err) = persist_connections(&state.connections_path, &connections) {
                return err;
            }
            HttpResponse::Ok().json(
                serde_json::json!({"status": "removed", "provider": conn.provider.display_name()}),
            )
        }
        None => HttpResponse::NotFound().body("Storage connection not found"),
    }
}

/// Trigger sync on storage
#[utoipa::path(
    post,
    path = "/api/storage/{id}/sync",
    tag = "storage",
    params(("id" = String, Path, description = "Storage id")),
    responses(
        (status = 200, description = "Storage sync started", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/storage/{id}/sync")]
pub async fn sync_storage(
    req: HttpRequest,
    app_state: web::Data<AppState>,
    state: web::Data<StorageState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) =
        require_license_feature(&app_state, Feature::CloudSyncBackup, "cloud_sync_backup")
    {
        return resp;
    }
    let id = path.into_inner();
    let connection = {
        let connections = state.connections.read().await;
        connections.get(&id).cloned()
    };
    let Some(connection) = connection else {
        return HttpResponse::NotFound().body("Storage connection not found");
    };

    let token_store = state.token_store.clone();
    let validation =
        tokio::task::spawn_blocking(move || validate_storage_connection(&connection, &token_store))
            .await
            .map_err(|err| err.to_string())
            .and_then(|inner| inner);

    let now = Utc::now();
    let mut connections = state.connections.write().await;
    if let Some(conn) = connections.get_mut(&id) {
        conn.last_sync = Some(now);
        conn.status = if validation.is_ok() {
            StorageConnectionStatus::Connected
        } else {
            StorageConnectionStatus::Error
        };
        if let Err(err) = persist_connections(&state.connections_path, &connections) {
            return err;
        }
    }

    match validation {
        Ok(_) => HttpResponse::Ok().json(serde_json::json!({
            "status": "sync_ok",
            "id": id
        })),
        Err(err) => HttpResponse::BadRequest().json(serde_json::json!({
            "status": "sync_failed",
            "id": id,
            "error": err
        })),
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BackupStartRequest {
    pub project_id: String,
    pub label: Option<String>,
    pub storage_id: Option<String>,
    pub remote_path: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BackupRestoreRequest {
    pub job_id: String,
    pub target_project_id: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/storage/backup/jobs",
    tag = "storage",
    responses(
        (status = 200, description = "Backup job list", body = [BackupJob]),
        (status = 401, description = "Unauthorized")
    )
)]
#[get("/api/storage/backup/jobs")]
pub async fn list_backup_jobs(
    req: HttpRequest,
    app_state: web::Data<AppState>,
    state: web::Data<StorageState>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) =
        require_license_feature(&app_state, Feature::CloudSyncBackup, "cloud_sync_backup")
    {
        return resp;
    }
    let jobs = state.backups.read().await;
    let list: Vec<BackupJob> = jobs.values().cloned().collect();
    HttpResponse::Ok().json(list)
}

#[utoipa::path(
    get,
    path = "/api/storage/backup/jobs/{id}",
    tag = "storage",
    params(("id" = String, Path, description = "Backup job id")),
    responses(
        (status = 200, description = "Backup job", body = BackupJob),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[get("/api/storage/backup/jobs/{id}")]
pub async fn get_backup_job(
    req: HttpRequest,
    app_state: web::Data<AppState>,
    state: web::Data<StorageState>,
    path: web::Path<String>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) =
        require_license_feature(&app_state, Feature::CloudSyncBackup, "cloud_sync_backup")
    {
        return resp;
    }
    let id = path.into_inner();
    let jobs = state.backups.read().await;
    match jobs.get(&id) {
        Some(job) => HttpResponse::Ok().json(job),
        None => HttpResponse::NotFound().body("Backup job not found"),
    }
}

#[utoipa::path(
    post,
    path = "/api/storage/backup/start",
    tag = "storage",
    request_body = BackupStartRequest,
    responses(
        (status = 200, description = "Backup job created", body = BackupJob),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized")
    )
)]
#[post("/api/storage/backup/start")]
pub async fn start_backup(
    req: HttpRequest,
    app_state: web::Data<AppState>,
    state: web::Data<StorageState>,
    json: web::Json<BackupStartRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) =
        require_license_feature(&app_state, Feature::CloudSyncBackup, "cloud_sync_backup")
    {
        return resp;
    }
    let project_dir = app_state.config.paths.projects_dir.join(&json.project_id);
    if !project_dir.exists() {
        return HttpResponse::BadRequest().body("Project not found");
    }
    if json.remote_path.is_some() && json.storage_id.is_none() {
        return HttpResponse::BadRequest().body("remote_path requires storage_id");
    }

    let job_id = Uuid::new_v4().to_string();
    let mut storage_connection: Option<StorageConnection> = None;
    if let Some(storage_id) = json.storage_id.as_ref() {
        let connections = state.connections.read().await;
        storage_connection = connections.get(storage_id).cloned();
        if storage_connection.is_none() {
            return HttpResponse::BadRequest().body("Storage connection not found");
        }
    }

    let mut remote_path = json.remote_path.clone();
    if let (Some(conn), true) = (storage_connection.as_ref(), remote_path.is_none()) {
        remote_path = Some(default_backup_remote_path(
            &conn.base_path,
            &json.project_id,
            &job_id,
        ));
    }
    let backup_root = backup_root_dir(&app_state.config.paths.projects_dir);
    let archive_path = backup_root
        .join(&json.project_id)
        .join(format!("{}.tar.gz", job_id));
    let job = BackupJob {
        id: job_id.clone(),
        job_type: BackupJobType::Backup,
        project_id: json.project_id.clone(),
        label: json.label.clone(),
        storage_id: json.storage_id.clone(),
        remote_path: remote_path.clone(),
        status: BackupJobStatus::Pending,
        created_at: Utc::now(),
        started_at: None,
        finished_at: None,
        archive_path: Some(archive_path.to_string_lossy().to_string()),
        size_bytes: None,
        sha256: None,
        error: None,
        restore_project_id: None,
    };

    {
        let mut jobs = state.backups.write().await;
        jobs.insert(job_id.clone(), job.clone());
        if let Err(err) = persist_backups(&state.backups_path, &jobs) {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to persist backup job: {err}"));
        }
    }

    let backups = state.backups.clone();
    let backups_path = state.backups_path.clone();
    let job_id_clone = job_id.clone();
    let token_store = state.token_store.clone();
    let connection_for_upload = storage_connection.clone();
    let remote_path_for_upload = remote_path.clone();
    tokio::spawn(async move {
        update_backup_job_state(&backups, &backups_path, &job_id_clone, |job| {
            job.status = BackupJobStatus::Running;
            job.started_at = Some(Utc::now());
        })
        .await;

        let result = tokio::task::spawn_blocking(move || {
            let (size_bytes, sha256) = create_backup_archive(&project_dir, &archive_path)?;
            if let (Some(conn), Some(remote_path)) = (
                connection_for_upload.as_ref(),
                remote_path_for_upload.as_ref(),
            ) {
                upload_backup_archive(conn, &token_store, &archive_path, remote_path)?;
            }
            Ok((size_bytes, sha256))
        })
        .await
        .map_err(|err| err.to_string())
        .and_then(|inner| inner);

        match result {
            Ok((size_bytes, sha256)) => {
                update_backup_job_state(&backups, &backups_path, &job_id_clone, |job| {
                    job.status = BackupJobStatus::Completed;
                    job.finished_at = Some(Utc::now());
                    job.size_bytes = Some(size_bytes);
                    job.sha256 = Some(sha256);
                })
                .await;
            }
            Err(err) => {
                update_backup_job_state(&backups, &backups_path, &job_id_clone, |job| {
                    job.status = BackupJobStatus::Failed;
                    job.finished_at = Some(Utc::now());
                    job.error = Some(err);
                })
                .await;
            }
        }
    });

    HttpResponse::Ok().json(job)
}

#[utoipa::path(
    post,
    path = "/api/storage/backup/restore",
    tag = "storage",
    request_body = BackupRestoreRequest,
    responses(
        (status = 200, description = "Restore job created", body = BackupJob),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found")
    )
)]
#[post("/api/storage/backup/restore")]
pub async fn restore_backup(
    req: HttpRequest,
    app_state: web::Data<AppState>,
    state: web::Data<StorageState>,
    json: web::Json<BackupRestoreRequest>,
) -> impl Responder {
    if let Err(resp) = require_admin(&req) {
        return resp;
    }
    if let Err(resp) =
        require_license_feature(&app_state, Feature::CloudSyncBackup, "cloud_sync_backup")
    {
        return resp;
    }
    let job_id = json.job_id.clone();
    let jobs = state.backups.read().await;
    let source_job = match jobs.get(&job_id) {
        Some(job) => job.clone(),
        None => return HttpResponse::NotFound().body("Backup job not found"),
    };
    drop(jobs);

    if source_job.status != BackupJobStatus::Completed {
        return HttpResponse::BadRequest().body("Backup job is not completed");
    }
    let archive_path = match &source_job.archive_path {
        Some(path) => PathBuf::from(path),
        None => return HttpResponse::BadRequest().body("Backup archive missing"),
    };
    if !archive_path.exists() {
        if let (Some(storage_id), Some(remote_path)) =
            (&source_job.storage_id, &source_job.remote_path)
        {
            let connections = state.connections.read().await;
            let connection = match connections.get(storage_id) {
                Some(conn) => conn.clone(),
                None => return HttpResponse::BadRequest().body("Storage connection not found"),
            };
            drop(connections);
            let token_store = state.token_store.clone();
            let archive_path_clone = archive_path.clone();
            let remote_path_clone = remote_path.clone();
            let download_result = tokio::task::spawn_blocking(move || {
                download_backup_archive(
                    &connection,
                    &token_store,
                    &remote_path_clone,
                    &archive_path_clone,
                )
            })
            .await
            .map_err(|err| err.to_string())
            .and_then(|inner| inner);
            if let Err(err) = download_result {
                return HttpResponse::BadRequest().body(format!("Backup download failed: {err}"));
            }
        } else {
            return HttpResponse::BadRequest().body("Backup archive missing");
        }
    }
    if let Some(expected) = source_job.sha256.as_ref() {
        match compute_sha256(&archive_path) {
            Ok(actual) if actual == *expected => {}
            Ok(_) => return HttpResponse::BadRequest().body("Backup integrity check failed"),
            Err(err) => {
                return HttpResponse::BadRequest().body(format!("Backup hash check failed: {err}"))
            }
        }
    }

    let target_project_id = json
        .target_project_id
        .clone()
        .unwrap_or_else(|| source_job.project_id.clone());
    let target_dir = app_state.config.paths.projects_dir.join(&target_project_id);
    if target_dir.exists() {
        return HttpResponse::BadRequest().body("Target project already exists");
    }

    let restore_job_id = Uuid::new_v4().to_string();
    let restore_job = BackupJob {
        id: restore_job_id.clone(),
        job_type: BackupJobType::Restore,
        project_id: source_job.project_id.clone(),
        label: source_job.label.clone(),
        storage_id: source_job.storage_id.clone(),
        remote_path: source_job.remote_path.clone(),
        status: BackupJobStatus::Pending,
        created_at: Utc::now(),
        started_at: None,
        finished_at: None,
        archive_path: Some(archive_path.to_string_lossy().to_string()),
        size_bytes: source_job.size_bytes,
        sha256: source_job.sha256.clone(),
        error: None,
        restore_project_id: Some(target_project_id.clone()),
    };

    {
        let mut jobs = state.backups.write().await;
        jobs.insert(restore_job_id.clone(), restore_job.clone());
        if let Err(err) = persist_backups(&state.backups_path, &jobs) {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to persist restore job: {err}"));
        }
    }

    let backups = state.backups.clone();
    let backups_path = state.backups_path.clone();
    let restore_job_id_clone = restore_job_id.clone();
    let restore_job_id_for_restore = restore_job_id_clone.clone();
    let source_project_id = source_job.project_id.clone();
    let projects_dir = app_state.config.paths.projects_dir.clone();
    tokio::spawn(async move {
        update_backup_job_state(&backups, &backups_path, &restore_job_id_clone, |job| {
            job.status = BackupJobStatus::Running;
            job.started_at = Some(Utc::now());
        })
        .await;

        let result = tokio::task::spawn_blocking(move || {
            restore_backup_archive(
                &archive_path,
                &projects_dir,
                &source_project_id,
                &target_project_id,
                &restore_job_id_for_restore,
            )
        })
        .await
        .map_err(|err| err.to_string())
        .and_then(|inner| inner);

        match result {
            Ok(_) => {
                update_backup_job_state(&backups, &backups_path, &restore_job_id_clone, |job| {
                    job.status = BackupJobStatus::Completed;
                    job.finished_at = Some(Utc::now());
                })
                .await;
            }
            Err(err) => {
                update_backup_job_state(&backups, &backups_path, &restore_job_id_clone, |job| {
                    job.status = BackupJobStatus::Failed;
                    job.finished_at = Some(Utc::now());
                    job.error = Some(err);
                })
                .await;
            }
        }
    });

    HttpResponse::Ok().json(restore_job)
}

/// Configure storage routes
pub fn configure(cfg: &mut web::ServiceConfig) {
    let state = web::Data::new(
        StorageState::new(&AppConfig::load().expect("Failed to load config"))
            .expect("Failed to initialize storage token store"),
    );

    cfg.app_data(state)
        .service(list_providers)
        .service(list_storage)
        .service(get_storage)
        .service(get_oauth_url)
        .service(oauth_callback)
        .service(add_storage)
        .service(remove_storage)
        .service(sync_storage)
        .service(list_backup_jobs)
        .service(get_backup_job)
        .service(start_backup)
        .service(restore_backup);
}

fn backup_root_dir(projects_dir: &Path) -> PathBuf {
    projects_dir.join("_backups")
}

async fn update_backup_job_state<F>(
    backups: &Arc<RwLock<HashMap<String, BackupJob>>>,
    backups_path: &Path,
    job_id: &str,
    update: F,
) where
    F: FnOnce(&mut BackupJob),
{
    let mut jobs = backups.write().await;
    if let Some(job) = jobs.get_mut(job_id) {
        update(job);
        if let Err(err) = persist_backups(backups_path, &jobs) {
            tracing::warn!("Failed to persist backup job {}: {}", job_id, err);
        }
    }
}

fn create_backup_archive(project_dir: &Path, archive_path: &Path) -> Result<(u64, String), String> {
    if !project_dir.exists() {
        return Err("Project directory not found".to_string());
    }
    if let Some(parent) = archive_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let file = File::create(archive_path).map_err(|e| e.to_string())?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = Builder::new(encoder);
    let root_name = project_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());
    builder
        .append_dir_all(root_name, project_dir)
        .map_err(|e| e.to_string())?;
    let encoder = builder.into_inner().map_err(|e| e.to_string())?;
    encoder.finish().map_err(|e| e.to_string())?;

    let size_bytes = std::fs::metadata(archive_path)
        .map_err(|e| e.to_string())?
        .len();
    let sha256 = compute_sha256(archive_path)?;
    Ok((size_bytes, sha256))
}

fn restore_backup_archive(
    archive_path: &Path,
    projects_dir: &Path,
    source_project_id: &str,
    target_project_id: &str,
    job_id: &str,
) -> Result<(), String> {
    if !archive_path.exists() {
        return Err("Backup archive not found".to_string());
    }
    let temp_dir = projects_dir.join(format!("_restore_{}", job_id));
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;

    let file = File::open(archive_path).map_err(|e| e.to_string())?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    archive.unpack(&temp_dir).map_err(|e| e.to_string())?;

    let extracted = temp_dir.join(source_project_id);
    if !extracted.exists() {
        return Err("Backup archive did not contain expected project directory".to_string());
    }

    let target_dir = projects_dir.join(target_project_id);
    if target_dir.exists() {
        return Err("Target project already exists".to_string());
    }
    std::fs::rename(&extracted, &target_dir).map_err(|e| e.to_string())?;
    std::fs::remove_dir_all(&temp_dir).map_err(|e| e.to_string())?;
    Ok(())
}

fn compute_sha256(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn normalize_remote_path(path: &str) -> String {
    path.trim().trim_matches('/').to_string()
}

fn join_remote_path(base_path: &str, suffix: &str) -> String {
    let base = normalize_remote_path(base_path);
    let suffix = suffix.trim_start_matches('/');
    if base.is_empty() {
        suffix.to_string()
    } else if suffix.is_empty() {
        base
    } else {
        format!("{}/{}", base, suffix)
    }
}

fn default_backup_remote_path(base_path: &str, project_id: &str, job_id: &str) -> String {
    let suffix = format!("backups/{}/{}.tar.gz", project_id, job_id);
    join_remote_path(base_path, &suffix)
}

fn storage_credentials(
    token_store: &TokenStore,
    storage_id: &str,
) -> Result<(String, String), String> {
    let token = token_store
        .load_token(storage_id)
        .map_err(|_| "Storage credentials not found".to_string())?;
    if token.access_token.trim().is_empty() {
        return Err("Storage access key missing".to_string());
    }
    let secret = token
        .refresh_token
        .ok_or_else(|| "Storage secret key missing".to_string())?;
    Ok((token.access_token, secret))
}

fn s3_client_from_connection(
    connection: &StorageConnection,
    token_store: &TokenStore,
) -> Result<S3Client, String> {
    let bucket = connection
        .bucket
        .as_ref()
        .ok_or_else(|| "Storage bucket missing".to_string())?;
    let region = connection.region.as_deref().unwrap_or("us-east-1");
    let (access_key, secret_key) = storage_credentials(token_store, &connection.id)?;
    S3Client::new_with_credentials(
        bucket,
        region,
        connection.endpoint.as_deref(),
        &access_key,
        &secret_key,
        None,
    )
    .map_err(|e| format!("S3 client init failed: {e}"))
}

fn validate_storage_connection(
    connection: &StorageConnection,
    token_store: &TokenStore,
) -> Result<(), String> {
    match connection.provider {
        CloudProvider::Nas => validate_nas_connection(connection),
        CloudProvider::S3 | CloudProvider::Gcs | CloudProvider::Azure => {
            validate_s3_connection(connection, token_store)
        }
        _ => Ok(()),
    }
}

fn validate_nas_connection(connection: &StorageConnection) -> Result<(), String> {
    let endpoint = connection
        .endpoint
        .as_ref()
        .ok_or_else(|| "NAS endpoint missing".to_string())?;
    let base_path = normalize_remote_path(&connection.base_path);
    let root = if base_path.is_empty() {
        PathBuf::from(endpoint)
    } else {
        PathBuf::from(endpoint).join(base_path)
    };
    std::fs::create_dir_all(&root).map_err(|e| format!("NAS base path error: {e}"))?;
    let marker_name = format!(".trueshot_sync_{}.txt", Uuid::new_v4());
    let marker_path = root.join(marker_name);
    let payload = format!("trueshot-sync-{}", Uuid::new_v4());
    {
        let mut file = File::create(&marker_path).map_err(|e| format!("NAS write failed: {e}"))?;
        file.write_all(payload.as_bytes())
            .map_err(|e| format!("NAS write failed: {e}"))?;
    }
    let read_back =
        std::fs::read_to_string(&marker_path).map_err(|e| format!("NAS read failed: {e}"))?;
    let _ = std::fs::remove_file(&marker_path);
    if read_back != payload {
        return Err("NAS validation mismatch".to_string());
    }
    Ok(())
}

fn validate_s3_connection(
    connection: &StorageConnection,
    token_store: &TokenStore,
) -> Result<(), String> {
    let client = s3_client_from_connection(connection, token_store)?;
    let marker_key = join_remote_path(
        &connection.base_path,
        &format!("sync_checks/{}.txt", Uuid::new_v4()),
    );
    let temp_dir = std::env::temp_dir();
    let local_path = temp_dir.join(format!("trueshot_sync_{}.txt", Uuid::new_v4()));
    let local_download = temp_dir.join(format!("trueshot_sync_dl_{}.txt", Uuid::new_v4()));
    let payload = format!("trueshot-sync-{}", Uuid::new_v4());
    {
        let mut file = File::create(&local_path).map_err(|e| format!("Temp write failed: {e}"))?;
        file.write_all(payload.as_bytes())
            .map_err(|e| format!("Temp write failed: {e}"))?;
    }
    client
        .upload_file(&local_path, &marker_key)
        .map_err(|e| format!("S3 upload failed: {e}"))?;
    client
        .download_file(&marker_key, &local_download)
        .map_err(|e| format!("S3 download failed: {e}"))?;
    let read_back =
        std::fs::read_to_string(&local_download).map_err(|e| format!("Temp read failed: {e}"))?;
    let _ = std::fs::remove_file(&local_path);
    let _ = std::fs::remove_file(&local_download);
    if read_back != payload {
        return Err("S3 validation mismatch".to_string());
    }
    Ok(())
}

fn upload_backup_archive(
    connection: &StorageConnection,
    token_store: &TokenStore,
    archive_path: &Path,
    remote_path: &str,
) -> Result<(), String> {
    match connection.provider {
        CloudProvider::Nas => {
            let target = nas_target_path(connection, remote_path)?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("NAS create dir failed: {e}"))?;
            }
            std::fs::copy(archive_path, &target).map_err(|e| format!("NAS copy failed: {e}"))?;
            Ok(())
        }
        CloudProvider::S3 | CloudProvider::Gcs | CloudProvider::Azure => {
            let client = s3_client_from_connection(connection, token_store)?;
            let key = normalize_remote_path(remote_path);
            if key.is_empty() {
                return Err("Remote path missing".to_string());
            }
            client
                .upload_file(archive_path, &key)
                .map_err(|e| format!("S3 upload failed: {e}"))
        }
        _ => Err("Provider does not support backup uploads".to_string()),
    }
}

fn download_backup_archive(
    connection: &StorageConnection,
    token_store: &TokenStore,
    remote_path: &str,
    archive_path: &Path,
) -> Result<(), String> {
    match connection.provider {
        CloudProvider::Nas => {
            let source = nas_target_path(connection, remote_path)?;
            if !source.exists() {
                return Err("NAS backup not found".to_string());
            }
            if let Some(parent) = archive_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("Archive dir error: {e}"))?;
            }
            std::fs::copy(&source, archive_path).map_err(|e| format!("NAS copy failed: {e}"))?;
            Ok(())
        }
        CloudProvider::S3 | CloudProvider::Gcs | CloudProvider::Azure => {
            let client = s3_client_from_connection(connection, token_store)?;
            let key = normalize_remote_path(remote_path);
            if key.is_empty() {
                return Err("Remote path missing".to_string());
            }
            client
                .download_file(&key, archive_path)
                .map_err(|e| format!("S3 download failed: {e}"))
        }
        _ => Err("Provider does not support backup downloads".to_string()),
    }
}

fn nas_target_path(connection: &StorageConnection, remote_path: &str) -> Result<PathBuf, String> {
    let endpoint = connection
        .endpoint
        .as_ref()
        .ok_or_else(|| "NAS endpoint missing".to_string())?;
    let relative = normalize_remote_path(remote_path);
    if relative.is_empty() {
        return Err("Remote path missing".to_string());
    }
    Ok(PathBuf::from(endpoint).join(relative))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredConnection {
    id: String,
    provider: CloudProvider,
    name: String,
    email: Option<String>,
    connected_at: DateTime<Utc>,
    last_sync: Option<DateTime<Utc>>,
    used_bytes: Option<u64>,
    total_bytes: Option<u64>,
    base_path: String,
    endpoint: Option<String>,
    bucket: Option<String>,
    region: Option<String>,
}

fn persist_connections(
    path: &std::path::Path,
    connections: &HashMap<String, StorageConnection>,
) -> Result<(), HttpResponse> {
    let stored: Vec<StoredConnection> = connections
        .values()
        .map(|conn| StoredConnection {
            id: conn.id.clone(),
            provider: conn.provider,
            name: conn.name.clone(),
            email: conn.email.clone(),
            connected_at: conn.connected_at,
            last_sync: conn.last_sync,
            used_bytes: conn.used_bytes,
            total_bytes: conn.total_bytes,
            base_path: conn.base_path.clone(),
            endpoint: conn.endpoint.clone(),
            bucket: conn.bucket.clone(),
            region: conn.region.clone(),
        })
        .collect();
    let json = serde_json::to_string_pretty(&stored).map_err(|e| {
        HttpResponse::InternalServerError()
            .body(format!("Failed to serialize storage connections: {e}"))
    })?;
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Err(HttpResponse::InternalServerError()
                .body(format!("Failed to create storage dir: {e}")));
        }
    }
    if let Err(e) = std::fs::write(path, json) {
        return Err(HttpResponse::InternalServerError()
            .body(format!("Failed to persist storage connections: {e}")));
    }
    Ok(())
}

fn persist_backups(
    path: &std::path::Path,
    backups: &HashMap<String, BackupJob>,
) -> Result<(), std::io::Error> {
    let json = serde_json::to_string_pretty(backups)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, json)?;
    Ok(())
}

fn load_backups(path: &std::path::Path) -> Result<HashMap<String, BackupJob>, std::io::Error> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let content = std::fs::read_to_string(path)?;
    let backups: HashMap<String, BackupJob> = serde_json::from_str(&content)?;
    Ok(backups)
}

fn load_connections(
    path: &std::path::Path,
    token_store: &TokenStore,
    _oauth_configs: &HashMap<CloudProvider, OAuthConfig>,
) -> Result<HashMap<String, StorageConnection>, trueshot_core::security::TokenStoreError> {
    if !path.exists() {
        return Ok(load_connections_from_tokens(token_store));
    }
    let contents = std::fs::read_to_string(path)
        .map_err(|e| trueshot_core::security::TokenStoreError::Io(e.to_string()))?;
    let stored: Vec<StoredConnection> = serde_json::from_str(&contents)
        .map_err(|e| trueshot_core::security::TokenStoreError::Serialization(e.to_string()))?;
    let mut connections = HashMap::new();
    for entry in stored {
        let tokens = if entry.provider.requires_oauth() {
            token_store
                .load_token(&entry.id)
                .ok()
                .map(|stored| OAuthTokens {
                    access_token: stored.access_token,
                    refresh_token: stored.refresh_token,
                    expires_at: stored.expires_at,
                    token_type: "Bearer".to_string(),
                })
        } else {
            None
        };
        let status = if entry.provider.requires_oauth() && tokens.is_none() {
            StorageConnectionStatus::NeedsReauth
        } else {
            StorageConnectionStatus::Connected
        };
        connections.insert(
            entry.id.clone(),
            StorageConnection {
                id: entry.id,
                provider: entry.provider,
                name: entry.name,
                email: entry.email,
                status,
                connected_at: entry.connected_at,
                last_sync: entry.last_sync,
                used_bytes: entry.used_bytes,
                total_bytes: entry.total_bytes,
                base_path: entry.base_path,
                endpoint: entry.endpoint,
                bucket: entry.bucket,
                region: entry.region,
                tokens,
            },
        );
    }
    Ok(connections)
}

fn load_connections_from_tokens(token_store: &TokenStore) -> HashMap<String, StorageConnection> {
    let mut connections = HashMap::new();
    if let Ok(providers) = token_store.list_providers() {
        for provider_id in providers {
            let provider = match provider_from_connection_id(&provider_id) {
                Some(p) => p,
                None => CloudProvider::Nas,
            };
            let tokens = if provider.requires_oauth() {
                token_store
                    .load_token(&provider_id)
                    .ok()
                    .map(|stored| OAuthTokens {
                        access_token: stored.access_token,
                        refresh_token: stored.refresh_token,
                        expires_at: stored.expires_at,
                        token_type: "Bearer".to_string(),
                    })
            } else {
                None
            };
            let status = if provider.requires_oauth() && tokens.is_none() {
                StorageConnectionStatus::NeedsReauth
            } else {
                StorageConnectionStatus::Connected
            };
            let display_name = if provider.requires_oauth() {
                provider.display_name().to_string()
            } else {
                "Legacy Storage".to_string()
            };
            connections.insert(
                provider_id.clone(),
                StorageConnection {
                    id: provider_id.clone(),
                    provider,
                    name: display_name,
                    email: None,
                    status,
                    connected_at: Utc::now(),
                    last_sync: None,
                    used_bytes: None,
                    total_bytes: None,
                    base_path: "/TrueShot".to_string(),
                    endpoint: None,
                    bucket: None,
                    region: None,
                    tokens,
                },
            );
        }
    }
    connections
}

fn normalize_storage_id(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if c == ' ' || c == '-' || c == '_' {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "storage".to_string()
    } else {
        trimmed
    }
}

fn token_store_dir() -> std::path::PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("TrueShot")
        .join("secrets")
}

fn frontend_base_url(config: &AppConfig) -> String {
    config
        .server
        .frontend_base_url
        .clone()
        .unwrap_or_else(|| "http://localhost:5173".to_string())
        .trim_end_matches('/')
        .to_string()
}

fn oauth_redirect_base_url(config: &AppConfig) -> String {
    if let Some(url) = &config.server.public_base_url {
        return url.trim_end_matches('/').to_string();
    }
    let host = if config.server.host == "0.0.0.0" {
        "localhost".to_string()
    } else {
        config.server.host.clone()
    };
    format!("http://{}:{}", host, config.server.port)
}

#[derive(Debug, Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    token_type: String,
}

#[derive(Debug, Deserialize)]
struct DropboxTokenResponse {
    access_token: String,
    token_type: String,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OneDriveTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    token_type: String,
}

async fn exchange_oauth_code(
    state: &StorageState,
    provider: CloudProvider,
    code: &str,
) -> Result<(OAuthTokens, Option<String>), HttpResponse> {
    let configs = state.oauth_configs.read().await;
    let config = configs
        .get(&provider)
        .ok_or_else(|| HttpResponse::InternalServerError().body("OAuth config missing"))?;

    if config.client_id.is_empty() || config.client_secret.is_empty() {
        return Err(HttpResponse::InternalServerError().body("OAuth credentials not configured"));
    }

    match provider {
        CloudProvider::GoogleDrive => {
            let token_resp = state
                .http_client
                .post("https://oauth2.googleapis.com/token")
                .form(&[
                    ("code", code),
                    ("client_id", config.client_id.as_str()),
                    ("client_secret", config.client_secret.as_str()),
                    ("redirect_uri", config.redirect_uri.as_str()),
                    ("grant_type", "authorization_code"),
                ])
                .send()
                .await
                .map_err(|e| {
                    HttpResponse::BadGateway().body(format!("Token exchange failed: {e}"))
                })?
                .error_for_status()
                .map_err(|e| {
                    HttpResponse::BadGateway().body(format!("Token exchange failed: {e}"))
                })?
                .json::<GoogleTokenResponse>()
                .await
                .map_err(|e| HttpResponse::BadGateway().body(format!("Token parse failed: {e}")))?;

            let email = state
                .http_client
                .get("https://www.googleapis.com/oauth2/v2/userinfo")
                .bearer_auth(&token_resp.access_token)
                .send()
                .await
                .map_err(|e| HttpResponse::BadGateway().body(format!("User info failed: {e}")))?
                .error_for_status()
                .map_err(|e| HttpResponse::BadGateway().body(format!("User info failed: {e}")))?
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| {
                    v.get("email")
                        .and_then(|e| e.as_str())
                        .map(|s| s.to_string())
                });

            Ok((
                OAuthTokens {
                    access_token: token_resp.access_token,
                    refresh_token: token_resp.refresh_token,
                    expires_at: token_resp
                        .expires_in
                        .map(|s| Utc::now() + chrono::Duration::seconds(s as i64)),
                    token_type: token_resp.token_type,
                },
                email,
            ))
        }
        CloudProvider::Dropbox => {
            let token_resp = state
                .http_client
                .post("https://api.dropboxapi.com/oauth2/token")
                .form(&[
                    ("code", code),
                    ("client_id", config.client_id.as_str()),
                    ("client_secret", config.client_secret.as_str()),
                    ("redirect_uri", config.redirect_uri.as_str()),
                    ("grant_type", "authorization_code"),
                ])
                .send()
                .await
                .map_err(|e| {
                    HttpResponse::BadGateway().body(format!("Token exchange failed: {e}"))
                })?
                .error_for_status()
                .map_err(|e| {
                    HttpResponse::BadGateway().body(format!("Token exchange failed: {e}"))
                })?
                .json::<DropboxTokenResponse>()
                .await
                .map_err(|e| HttpResponse::BadGateway().body(format!("Token parse failed: {e}")))?;

            let email = state
                .http_client
                .post("https://api.dropboxapi.com/2/users/get_current_account")
                .bearer_auth(&token_resp.access_token)
                .json(&serde_json::json!({}))
                .send()
                .await
                .map_err(|e| HttpResponse::BadGateway().body(format!("User info failed: {e}")))?
                .error_for_status()
                .map_err(|e| HttpResponse::BadGateway().body(format!("User info failed: {e}")))?
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| {
                    v.get("email")
                        .and_then(|e| e.as_str())
                        .map(|s| s.to_string())
                });

            Ok((
                OAuthTokens {
                    access_token: token_resp.access_token,
                    refresh_token: token_resp.refresh_token,
                    expires_at: token_resp
                        .expires_in
                        .map(|s| Utc::now() + chrono::Duration::seconds(s as i64)),
                    token_type: token_resp.token_type,
                },
                email,
            ))
        }
        CloudProvider::OneDrive => {
            let token_resp = state
                .http_client
                .post("https://login.microsoftonline.com/common/oauth2/v2.0/token")
                .form(&[
                    ("code", code),
                    ("client_id", config.client_id.as_str()),
                    ("client_secret", config.client_secret.as_str()),
                    ("redirect_uri", config.redirect_uri.as_str()),
                    ("grant_type", "authorization_code"),
                ])
                .send()
                .await
                .map_err(|e| {
                    HttpResponse::BadGateway().body(format!("Token exchange failed: {e}"))
                })?
                .error_for_status()
                .map_err(|e| {
                    HttpResponse::BadGateway().body(format!("Token exchange failed: {e}"))
                })?
                .json::<OneDriveTokenResponse>()
                .await
                .map_err(|e| HttpResponse::BadGateway().body(format!("Token parse failed: {e}")))?;

            let profile = state
                .http_client
                .get("https://graph.microsoft.com/v1.0/me")
                .bearer_auth(&token_resp.access_token)
                .send()
                .await
                .map_err(|e| HttpResponse::BadGateway().body(format!("User info failed: {e}")))?
                .error_for_status()
                .map_err(|e| HttpResponse::BadGateway().body(format!("User info failed: {e}")))?
                .json::<serde_json::Value>()
                .await
                .ok();

            let email = profile
                .as_ref()
                .and_then(|v| v.get("mail").and_then(|e| e.as_str()))
                .or_else(|| {
                    profile
                        .as_ref()
                        .and_then(|v| v.get("userPrincipalName").and_then(|e| e.as_str()))
                })
                .map(|s| s.to_string());

            Ok((
                OAuthTokens {
                    access_token: token_resp.access_token,
                    refresh_token: token_resp.refresh_token,
                    expires_at: token_resp
                        .expires_in
                        .map(|s| Utc::now() + chrono::Duration::seconds(s as i64)),
                    token_type: token_resp.token_type,
                },
                email,
            ))
        }
        _ => Err(HttpResponse::BadRequest().body("Provider does not support OAuth here")),
    }
}

fn provider_from_key(key: &str) -> Option<CloudProvider> {
    match key {
        "google_drive" => Some(CloudProvider::GoogleDrive),
        "dropbox" => Some(CloudProvider::Dropbox),
        "onedrive" => Some(CloudProvider::OneDrive),
        "icloud" => Some(CloudProvider::ICloudDrive),
        "s3" => Some(CloudProvider::S3),
        "gcs" => Some(CloudProvider::Gcs),
        "azure" => Some(CloudProvider::Azure),
        "nas" => Some(CloudProvider::Nas),
        _ => None,
    }
}

fn provider_from_connection_id(key: &str) -> Option<CloudProvider> {
    if let Some((prefix, _)) = key.split_once(':') {
        return provider_from_key(prefix);
    }
    provider_from_key(key)
}
