//! External Storage Manager
//!
//! Unified storage backend supporting:
//! - NAS (SMB/CIFS, NFS)
//! - Amazon S3 and compatible (MinIO, R2)
//! - Google Cloud Storage
//! - Azure Blob Storage
//! - Local/network paths

use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use sysinfo::Disks;
use uuid::Uuid;
use walkdir::WalkDir;

// ============================================================================
// Types
// ============================================================================

/// Storage provider type
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum StorageType {
    /// Local filesystem
    Local,
    /// Network Attached Storage (SMB/NFS)
    Nas,
    /// Amazon S3 or compatible (MinIO, Cloudflare R2)
    S3,
    /// Google Cloud Storage
    Gcs,
    /// Azure Blob Storage
    Azure,
    /// Google Drive (OAuth)
    GoogleDrive,
    /// Dropbox (OAuth)
    Dropbox,
    /// Microsoft OneDrive (OAuth)
    OneDrive,
    /// Apple iCloud Drive (macOS native)
    ICloudDrive,
}

/// Storage connection status
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum StorageStatus {
    Connected,
    Disconnected,
    Syncing,
    Error,
    Initializing,
}

/// External storage configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Unique identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Storage type
    pub storage_type: StorageType,
    /// Connection endpoint
    pub endpoint: StorageEndpoint,
    /// Authentication credentials
    pub credentials: Option<StorageCredentials>,
    /// Base path within storage
    pub base_path: String,
    /// Is this the primary storage
    pub is_primary: bool,
    /// Sync settings
    pub sync_config: SyncConfig,
}

/// Storage endpoint configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StorageEndpoint {
    /// Local filesystem path
    LocalPath(PathBuf),
    /// Network path (SMB/NFS)
    NetworkPath {
        protocol: NetworkProtocol,
        host: String,
        share: String,
        port: Option<u16>,
    },
    /// Cloud storage bucket
    CloudBucket {
        endpoint: String,
        region: Option<String>,
        bucket: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum NetworkProtocol {
    Smb,
    Nfs,
    Webdav,
}

/// Storage authentication credentials
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StorageCredentials {
    /// Access key / username
    pub access_key: String,
    /// Secret key / password (encrypted in storage)
    #[serde(skip_serializing)]
    pub secret_key: String,
    /// Session token (for temporary credentials)
    pub session_token: Option<String>,
}

/// Sync configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncConfig {
    /// Auto-sync new projects
    pub auto_sync: bool,
    /// Sync interval in seconds (0 = manual only)
    pub sync_interval_secs: u64,
    /// Sync direction
    pub direction: SyncDirection,
    /// File patterns to include
    pub include_patterns: Vec<String>,
    /// File patterns to exclude
    pub exclude_patterns: Vec<String>,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            auto_sync: true,
            sync_interval_secs: 300, // 5 minutes
            direction: SyncDirection::Bidirectional,
            include_patterns: vec!["*".to_string()],
            exclude_patterns: vec![
                ".DS_Store".to_string(),
                "*.tmp".to_string(),
                "Thumbs.db".to_string(),
            ],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SyncDirection {
    Upload,
    Download,
    Bidirectional,
}

/// Storage usage statistics
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StorageStats {
    /// Total capacity in bytes
    pub total_bytes: u64,
    /// Used space in bytes
    pub used_bytes: u64,
    /// Number of files
    pub file_count: u64,
    /// Last sync timestamp
    pub last_sync: Option<chrono::DateTime<chrono::Utc>>,
    /// Pending sync items
    pub pending_sync_count: u64,
}

/// Connected storage with runtime state
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectedStorage {
    pub config: StorageConfig,
    pub status: StorageStatus,
    pub stats: Option<StorageStats>,
    pub last_error: Option<String>,
}

// ============================================================================
// Storage Manager
// ============================================================================

/// Manages all external storage connections
pub struct StorageManager {
    /// Configured storages
    storages: HashMap<String, ConnectedStorage>,
    /// Config file path
    config_path: PathBuf,
}

impl StorageManager {
    pub fn new(config_path: PathBuf) -> Self {
        Self {
            storages: HashMap::new(),
            config_path,
        }
    }

    /// Load storage configurations from disk
    pub fn load(&mut self) -> Result<(), StorageError> {
        if !self.config_path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&self.config_path)
            .map_err(|e| StorageError::IoError(e.to_string()))?;

        let configs: Vec<StorageConfig> =
            serde_json::from_str(&content).map_err(|e| StorageError::ConfigError(e.to_string()))?;

        for config in configs {
            self.storages.insert(
                config.id.clone(),
                ConnectedStorage {
                    config,
                    status: StorageStatus::Disconnected,
                    stats: None,
                    last_error: None,
                },
            );
        }

        Ok(())
    }

    /// Save storage configurations to disk
    pub fn save(&self) -> Result<(), StorageError> {
        let configs: Vec<&StorageConfig> = self.storages.values().map(|s| &s.config).collect();

        let content = serde_json::to_string_pretty(&configs)
            .map_err(|e| StorageError::ConfigError(e.to_string()))?;

        std::fs::write(&self.config_path, content)
            .map_err(|e| StorageError::IoError(e.to_string()))?;

        Ok(())
    }

    /// Add a new storage
    pub fn add_storage(&mut self, config: StorageConfig) -> Result<(), StorageError> {
        if self.storages.contains_key(&config.id) {
            return Err(StorageError::AlreadyExists(config.id));
        }

        self.storages.insert(
            config.id.clone(),
            ConnectedStorage {
                config,
                status: StorageStatus::Initializing,
                stats: None,
                last_error: None,
            },
        );

        self.save()?;
        Ok(())
    }

    /// Remove a storage
    pub fn remove_storage(&mut self, id: &str) -> Result<(), StorageError> {
        if self.storages.remove(id).is_none() {
            return Err(StorageError::NotFound(id.to_string()));
        }
        self.save()?;
        Ok(())
    }

    /// Get all storages
    pub fn list_storages(&self) -> Vec<&ConnectedStorage> {
        self.storages.values().collect()
    }

    /// Get storage by ID
    pub fn get_storage(&self, id: &str) -> Option<&ConnectedStorage> {
        self.storages.get(id)
    }

    /// Connect to storage
    pub fn connect(&mut self, id: &str) -> Result<(), StorageError> {
        let storage = self
            .storages
            .get_mut(id)
            .ok_or_else(|| StorageError::NotFound(id.to_string()))?;

        // Attempt connection based on type
        match &storage.config.storage_type {
            StorageType::Local => {
                let root = filesystem_root_from_endpoint(&storage.config.endpoint)?;
                let target = root.join(&storage.config.base_path);
                match validate_filesystem_path(&target) {
                    Ok(stats) => {
                        storage.status = StorageStatus::Connected;
                        storage.stats = Some(stats);
                        storage.last_error = None;
                    }
                    Err(err) => {
                        storage.status = StorageStatus::Error;
                        storage.last_error = Some(err.to_string());
                    }
                }
            }
            StorageType::Nas => {
                let root = filesystem_root_from_endpoint(&storage.config.endpoint)?;
                let target = root.join(&storage.config.base_path);
                match validate_filesystem_path(&target) {
                    Ok(stats) => {
                        storage.status = StorageStatus::Connected;
                        storage.stats = Some(stats);
                        storage.last_error = None;
                    }
                    Err(err) => {
                        storage.status = StorageStatus::Error;
                        storage.last_error = Some(err.to_string());
                    }
                }
            }
            StorageType::S3 => match validate_object_store(&storage.config) {
                Ok(()) => {
                    storage.status = StorageStatus::Connected;
                    storage.last_error = None;
                }
                Err(err) => {
                    storage.status = StorageStatus::Error;
                    storage.last_error = Some(err.to_string());
                }
            },
            StorageType::Gcs => match validate_object_store(&storage.config) {
                Ok(()) => {
                    storage.status = StorageStatus::Connected;
                    storage.last_error = None;
                }
                Err(err) => {
                    storage.status = StorageStatus::Error;
                    storage.last_error = Some(err.to_string());
                }
            },
            StorageType::Azure => match validate_object_store(&storage.config) {
                Ok(()) => {
                    storage.status = StorageStatus::Connected;
                    storage.last_error = None;
                }
                Err(err) => {
                    storage.status = StorageStatus::Error;
                    storage.last_error = Some(err.to_string());
                }
            },
            // OAuth-based cloud drives - handled by API layer
            StorageType::GoogleDrive
            | StorageType::Dropbox
            | StorageType::OneDrive
            | StorageType::ICloudDrive => {
                storage.status = StorageStatus::Connected;
            }
        }

        Ok(())
    }

    /// Disconnect from storage
    pub fn disconnect(&mut self, id: &str) -> Result<(), StorageError> {
        let storage = self
            .storages
            .get_mut(id)
            .ok_or_else(|| StorageError::NotFound(id.to_string()))?;

        storage.status = StorageStatus::Disconnected;
        Ok(())
    }

    /// Get stats for local path (static method to avoid borrow issues)
    fn compute_local_stats(path: &PathBuf) -> StorageStats {
        let mut disks = Disks::new_with_refreshed_list();
        disks.refresh(false);

        let disk = select_disk_for_path(disks.list(), path.as_path());
        let (total_bytes, used_bytes) = disk
            .map(|disk| {
                let total = disk.total_space();
                let used = total.saturating_sub(disk.available_space());
                (total, used)
            })
            .unwrap_or((0, 0));

        let file_count = count_files_with_limit(path, 2_000_000);

        StorageStats {
            total_bytes,
            used_bytes,
            file_count,
            last_sync: None,
            pending_sync_count: 0,
        }
    }

    /// Upload file to storage
    pub async fn upload_file(
        &self,
        storage_id: &str,
        local_path: &PathBuf,
        remote_path: &str,
    ) -> Result<(), StorageError> {
        let storage = self
            .storages
            .get(storage_id)
            .ok_or_else(|| StorageError::NotFound(storage_id.to_string()))?;

        if storage.status != StorageStatus::Connected {
            return Err(StorageError::NotConnected(storage_id.to_string()));
        }

        // Implementation would use appropriate SDK
        match &storage.config.storage_type {
            StorageType::S3 => {
                // Use aws-sdk-s3
                self.upload_to_s3(&storage.config, local_path, remote_path)
                    .await
            }
            StorageType::Gcs => {
                // Use google-cloud-storage
                self.upload_to_gcs(&storage.config, local_path, remote_path)
                    .await
            }
            StorageType::Azure => {
                self.upload_to_object_store(&storage.config, local_path, remote_path)
                    .await
            }
            StorageType::Nas | StorageType::Local => {
                // Use filesystem copy
                self.upload_to_filesystem(&storage.config, local_path, remote_path)
            }
            _ => Err(StorageError::UnsupportedOperation),
        }
    }

    async fn upload_to_s3(
        &self,
        config: &StorageConfig,
        local_path: &PathBuf,
        remote_path: &str,
    ) -> Result<(), StorageError> {
        self.upload_to_object_store(config, local_path, remote_path)
            .await
    }

    async fn upload_to_gcs(
        &self,
        config: &StorageConfig,
        local_path: &PathBuf,
        remote_path: &str,
    ) -> Result<(), StorageError> {
        self.upload_to_object_store(config, local_path, remote_path)
            .await
    }

    async fn upload_to_object_store(
        &self,
        config: &StorageConfig,
        local_path: &PathBuf,
        remote_path: &str,
    ) -> Result<(), StorageError> {
        let bucket = bucket_from_config(config)?;
        let key = join_remote_path(&config.base_path, remote_path);
        let data = std::fs::read(local_path).map_err(|e| StorageError::IoError(e.to_string()))?;
        bucket
            .put_object(key, &data)
            .await
            .map_err(|e| StorageError::ConnectionError(e.to_string()))?;
        Ok(())
    }

    fn upload_to_filesystem(
        &self,
        config: &StorageConfig,
        local_path: &PathBuf,
        remote_path: &str,
    ) -> Result<(), StorageError> {
        let base = match &config.endpoint {
            StorageEndpoint::LocalPath(p) => p.clone(),
            StorageEndpoint::NetworkPath { host, share, .. } => {
                PathBuf::from(format!("//{}/{}", host, share))
            }
            _ => return Err(StorageError::UnsupportedOperation),
        };

        let dest = base.join(&config.base_path).join(remote_path);

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StorageError::IoError(e.to_string()))?;
        }

        std::fs::copy(local_path, dest).map_err(|e| StorageError::IoError(e.to_string()))?;

        Ok(())
    }

    /// Download file from storage
    pub async fn download_file(
        &self,
        storage_id: &str,
        remote_path: &str,
        local_path: &PathBuf,
    ) -> Result<(), StorageError> {
        let storage = self
            .storages
            .get(storage_id)
            .ok_or_else(|| StorageError::NotFound(storage_id.to_string()))?;

        if storage.status != StorageStatus::Connected {
            return Err(StorageError::NotConnected(storage_id.to_string()));
        }

        match &storage.config.storage_type {
            StorageType::S3 | StorageType::Gcs | StorageType::Azure => {
                let bucket = bucket_from_config(&storage.config)?;
                let key = join_remote_path(&storage.config.base_path, remote_path);
                let data = bucket
                    .get_object(key)
                    .await
                    .map_err(|e| StorageError::ConnectionError(e.to_string()))?
                    .to_vec();
                if let Some(parent) = local_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| StorageError::IoError(e.to_string()))?;
                }
                std::fs::write(local_path, data)
                    .map_err(|e| StorageError::IoError(e.to_string()))?;
                Ok(())
            }
            StorageType::Nas | StorageType::Local => {
                let base = filesystem_root_from_endpoint(&storage.config.endpoint)?;
                let source = base.join(&storage.config.base_path).join(remote_path);
                if let Some(parent) = local_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| StorageError::IoError(e.to_string()))?;
                }
                std::fs::copy(source, local_path)
                    .map_err(|e| StorageError::IoError(e.to_string()))?;
                Ok(())
            }
            _ => Err(StorageError::UnsupportedOperation),
        }
    }

    /// List files in remote path
    pub async fn list_files(
        &self,
        storage_id: &str,
        _path: &str,
    ) -> Result<Vec<RemoteFile>, StorageError> {
        let storage = self
            .storages
            .get(storage_id)
            .ok_or_else(|| StorageError::NotFound(storage_id.to_string()))?;

        if storage.status != StorageStatus::Connected {
            return Err(StorageError::NotConnected(storage_id.to_string()));
        }

        // Placeholder
        Ok(Vec::new())
    }
}

fn select_disk_for_path<'a>(disks: &'a [sysinfo::Disk], path: &Path) -> Option<&'a sysinfo::Disk> {
    let mut best: Option<&sysinfo::Disk> = None;
    let mut best_len = 0usize;
    for disk in disks {
        let mount = disk.mount_point();
        if path.starts_with(mount) {
            let len = mount.as_os_str().to_string_lossy().len();
            if len > best_len {
                best_len = len;
                best = Some(disk);
            }
        }
    }
    best.or_else(|| disks.first())
}

fn count_files_with_limit(root: &Path, limit: u64) -> u64 {
    if limit == 0 {
        return 0;
    }
    let mut count = 0u64;
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_file() {
            count += 1;
            if count >= limit {
                break;
            }
        }
    }
    count
}

fn filesystem_root_from_endpoint(endpoint: &StorageEndpoint) -> Result<PathBuf, StorageError> {
    match endpoint {
        StorageEndpoint::LocalPath(path) => Ok(path.clone()),
        StorageEndpoint::NetworkPath { host, share, .. } => {
            Ok(PathBuf::from(format!("//{}/{}", host, share)))
        }
        StorageEndpoint::CloudBucket { .. } => Err(StorageError::UnsupportedOperation),
    }
}

fn validate_filesystem_path(path: &PathBuf) -> Result<StorageStats, StorageError> {
    if !path.exists() {
        std::fs::create_dir_all(path).map_err(|e| StorageError::IoError(e.to_string()))?;
    }
    let marker = path.join(format!(".trueshot_sync_{}.txt", Uuid::new_v4()));
    let payload = format!("trueshot-sync-{}", Uuid::new_v4());
    {
        let mut file =
            std::fs::File::create(&marker).map_err(|e| StorageError::IoError(e.to_string()))?;
        file.write_all(payload.as_bytes())
            .map_err(|e| StorageError::IoError(e.to_string()))?;
    }
    let read_back =
        std::fs::read_to_string(&marker).map_err(|e| StorageError::IoError(e.to_string()))?;
    let _ = std::fs::remove_file(&marker);
    if read_back != payload {
        return Err(StorageError::ConnectionError(
            "Filesystem validation mismatch".to_string(),
        ));
    }
    Ok(StorageManager::compute_local_stats(path))
}

fn join_remote_path(base_path: &str, suffix: &str) -> String {
    let base = base_path.trim().trim_matches('/');
    let suffix = suffix.trim_start_matches('/');
    if base.is_empty() {
        suffix.to_string()
    } else if suffix.is_empty() {
        base.to_string()
    } else {
        format!("{}/{}", base, suffix)
    }
}

fn bucket_from_config(config: &StorageConfig) -> Result<Bucket, StorageError> {
    let (endpoint, region, bucket) = match &config.endpoint {
        StorageEndpoint::CloudBucket {
            endpoint,
            region,
            bucket,
        } => (endpoint.clone(), region.clone(), bucket.clone()),
        _ => return Err(StorageError::UnsupportedOperation),
    };
    let creds = config
        .credentials
        .as_ref()
        .ok_or(StorageError::AuthenticationFailed)?;
    let region = Region::Custom {
        region: region.unwrap_or_else(|| "us-east-1".to_string()),
        endpoint,
    };
    let credentials = Credentials::new(
        Some(&creds.access_key),
        Some(&creds.secret_key),
        creds.session_token.as_deref(),
        None,
        None,
    )
    .map_err(|e| StorageError::ConnectionError(e.to_string()))?;
    Bucket::new(&bucket, region, credentials)
        .map_err(|e| StorageError::ConnectionError(e.to_string()))
}

fn validate_object_store(config: &StorageConfig) -> Result<(), StorageError> {
    let bucket = bucket_from_config(config)?;
    let marker_key = join_remote_path(
        &config.base_path,
        &format!("sync_checks/{}.txt", Uuid::new_v4()),
    );
    let payload = format!("trueshot-sync-{}", Uuid::new_v4());
    let rt =
        tokio::runtime::Runtime::new().map_err(|e| StorageError::ConnectionError(e.to_string()))?;
    rt.block_on(bucket.put_object(&marker_key, payload.as_bytes()))
        .map_err(|e| StorageError::ConnectionError(e.to_string()))?;
    let data = rt
        .block_on(bucket.get_object(&marker_key))
        .map_err(|e| StorageError::ConnectionError(e.to_string()))?;
    if data.as_slice() != payload.as_bytes() {
        return Err(StorageError::ConnectionError(
            "Object store validation mismatch".to_string(),
        ));
    }
    Ok(())
}

/// Remote file info
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteFile {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub is_directory: bool,
    pub modified: chrono::DateTime<chrono::Utc>,
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Clone, Debug)]
pub enum StorageError {
    IoError(String),
    ConfigError(String),
    ConnectionError(String),
    NotFound(String),
    AlreadyExists(String),
    NotConnected(String),
    AuthenticationFailed,
    UnsupportedOperation,
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::IoError(msg) => write!(f, "IO error: {}", msg),
            StorageError::ConfigError(msg) => write!(f, "Config error: {}", msg),
            StorageError::ConnectionError(msg) => write!(f, "Connection error: {}", msg),
            StorageError::NotFound(id) => write!(f, "Storage not found: {}", id),
            StorageError::AlreadyExists(id) => write!(f, "Storage already exists: {}", id),
            StorageError::NotConnected(id) => write!(f, "Storage not connected: {}", id),
            StorageError::AuthenticationFailed => write!(f, "Authentication failed"),
            StorageError::UnsupportedOperation => write!(f, "Unsupported operation"),
        }
    }
}

impl std::error::Error for StorageError {}

// ============================================================================
// API Response Types
// ============================================================================

/// Storage info for API responses
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StorageInfo {
    pub id: String,
    pub name: String,
    pub storage_type: StorageType,
    pub status: StorageStatus,
    pub endpoint: String,
    pub used_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub last_sync: Option<String>,
}

impl From<&ConnectedStorage> for StorageInfo {
    fn from(storage: &ConnectedStorage) -> Self {
        let endpoint = match &storage.config.endpoint {
            StorageEndpoint::LocalPath(p) => p.to_string_lossy().to_string(),
            StorageEndpoint::NetworkPath { host, share, .. } => format!("//{}/{}", host, share),
            StorageEndpoint::CloudBucket { bucket, .. } => bucket.clone(),
        };

        StorageInfo {
            id: storage.config.id.clone(),
            name: storage.config.name.clone(),
            storage_type: storage.config.storage_type.clone(),
            status: storage.status.clone(),
            endpoint,
            used_bytes: storage.stats.as_ref().map(|s| s.used_bytes),
            total_bytes: storage.stats.as_ref().map(|s| s.total_bytes),
            last_sync: storage
                .stats
                .as_ref()
                .and_then(|s| s.last_sync)
                .map(|t| t.to_rfc3339()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_config_serialize() {
        let config = StorageConfig {
            id: "test-s3".to_string(),
            name: "My S3 Bucket".to_string(),
            storage_type: StorageType::S3,
            endpoint: StorageEndpoint::CloudBucket {
                endpoint: "s3.amazonaws.com".to_string(),
                region: Some("us-east-1".to_string()),
                bucket: "my-bucket".to_string(),
            },
            credentials: Some(StorageCredentials {
                access_key: "AKIAIOSFODNN7EXAMPLE".to_string(),
                secret_key: "secret".to_string(),
                session_token: None,
            }),
            base_path: "trueshot/".to_string(),
            is_primary: false,
            sync_config: SyncConfig::default(),
        };

        let json = serde_json::to_string_pretty(&config).unwrap();
        assert!(json.contains("s3.amazonaws.com"));
        // Secret key should be skipped
        assert!(!json.contains("secret"));
    }

    #[test]
    fn test_storage_manager() {
        let temp_path = std::env::temp_dir().join("trueshot_storage_test.json");
        let mut manager = StorageManager::new(temp_path.clone());

        let config = StorageConfig {
            id: "local-test".to_string(),
            name: "Test Local".to_string(),
            storage_type: StorageType::Local,
            endpoint: StorageEndpoint::LocalPath(PathBuf::from("/tmp")),
            credentials: None,
            base_path: "".to_string(),
            is_primary: true,
            sync_config: SyncConfig::default(),
        };

        manager.add_storage(config).unwrap();
        assert_eq!(manager.list_storages().len(), 1);

        manager.connect("local-test").unwrap();
        let storage = manager.get_storage("local-test").unwrap();
        assert_eq!(storage.status, StorageStatus::Connected);

        // Cleanup
        let _ = std::fs::remove_file(temp_path);
    }
}
