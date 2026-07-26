use anyhow::{Context, Result};
use chrono::Duration as ChronoDuration;
use config::{Config, File};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use sysinfo::System;
use trueshot_core::inventory::{Inventory, Sequence, SequenceStatus};
use trueshot_device_manager::{Foldio360, Turntable};
use uuid::Uuid;

// Daemon Loop (SOTA 10)
// Keeps BLE connection alive and polls for jobs.

#[derive(Debug, Deserialize, Clone)]
struct DaemonConfig {
    paths: DaemonPaths,
    daemon: DaemonSettings,
}

#[derive(Debug, Deserialize, Clone)]
struct DaemonPaths {
    projects_dir: PathBuf,
    inventory_db: PathBuf,
}

#[derive(Debug, Deserialize, Clone)]
struct DaemonSettings {
    poll_interval_secs: Option<u64>,
    capture_stable_secs: Option<u64>,
    capture_min_images: Option<usize>,
    capture_timeout_secs: Option<u64>,
    default_mode: Option<String>,
    default_quality: Option<String>,
    lease_ttl_secs: Option<u64>,
    lease_renew_secs: Option<u64>,
    max_failures: Option<u32>,
}

#[derive(Debug, Clone)]
struct LeaseOwner {
    id: Uuid,
    name: String,
}

struct LeaseRenewer {
    stop: Arc<AtomicBool>,
}

impl Drop for LeaseRenewer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("Starting TrueShot Daemon...");
    let cfg = load_daemon_config()?;
    let poll_interval = Duration::from_secs(cfg.daemon.poll_interval_secs.unwrap_or(5));
    let stable_secs = cfg.daemon.capture_stable_secs.unwrap_or(10);
    let min_images = cfg.daemon.capture_min_images.unwrap_or(20);
    let capture_timeout = cfg.daemon.capture_timeout_secs.unwrap_or(3600);
    let lease_ttl = ChronoDuration::seconds(cfg.daemon.lease_ttl_secs.unwrap_or(180) as i64);
    let lease_renew = Duration::from_secs(cfg.daemon.lease_renew_secs.unwrap_or(60));
    let max_failures = cfg.daemon.max_failures.unwrap_or(3);
    let owner = LeaseOwner {
        id: Uuid::new_v4(),
        name: format!(
            "{}:{}",
            System::host_name().unwrap_or_else(|| "unknown-host".to_string()),
            std::process::id()
        ),
    };

    // 1. Connect to Hardware
    let mut turntable = Foldio360::new();
    match turntable.connect().await {
        Ok(_) => tracing::info!("Turntable Connected"),
        Err(e) => tracing::warn!("Turntable not found: {}", e),
    }

    // 2. Open Inventory
    let inventory = Arc::new(Inventory::new(&cfg.paths.inventory_db)?);
    let model_count = inventory.list_models().map(|m| m.len()).unwrap_or(0);
    tracing::info!("Inventory loaded (models: {})", model_count);

    tracing::info!("Daemon Ready. Polling for jobs...");

    loop {
        if let Err(err) = process_planned_sequences(
            inventory.clone(),
            &cfg,
            &owner,
            lease_ttl,
            lease_renew,
            max_failures,
            stable_secs,
            min_images,
            capture_timeout,
        )
        .await
        {
            tracing::error!("Job runner error: {}", err);
        }

        // Keep-alive heartbeat
        tokio::time::sleep(poll_interval).await;
    }
}

fn load_daemon_config() -> Result<DaemonConfig> {
    let cfg = Config::builder()
        .set_default("paths.projects_dir", "./projects")?
        .set_default("paths.inventory_db", "./inventory.redb")?
        .set_default("daemon.poll_interval_secs", 5)?
        .set_default("daemon.capture_stable_secs", 10)?
        .set_default("daemon.capture_min_images", 20)?
        .set_default("daemon.capture_timeout_secs", 3600)?
        .set_default("daemon.default_mode", "hybrid")?
        .set_default("daemon.default_quality", "high")?
        .set_default("daemon.lease_ttl_secs", 180)?
        .set_default("daemon.lease_renew_secs", 60)?
        .set_default("daemon.max_failures", 3)?
        .add_source(File::with_name("config").required(false))
        .add_source(config::Environment::with_prefix("TRUESHOT").separator("__"))
        .build()
        .context("Failed to load daemon config")?;
    Ok(cfg.try_deserialize()?)
}

async fn process_planned_sequences(
    inventory: Arc<Inventory>,
    cfg: &DaemonConfig,
    owner: &LeaseOwner,
    lease_ttl: ChronoDuration,
    lease_renew: Duration,
    max_failures: u32,
    stable_secs: u64,
    min_images: usize,
    timeout_secs: u64,
) -> Result<()> {
    requeue_stale_sequences(inventory.clone(), max_failures).await?;
    let planned = inventory.list_sequences_by_status(SequenceStatus::Planned)?;
    if planned.is_empty() {
        return Ok(());
    }

    for seq in planned {
        let claimed =
            inventory.try_acquire_sequence_lease(&seq.id, &owner.id, &owner.name, lease_ttl)?;
        if !claimed {
            continue;
        }
        let claimed_status = inventory.transition_sequence_status(
            &seq.id,
            SequenceStatus::Planned,
            SequenceStatus::Capturing,
        )?;
        if claimed_status.is_none() {
            let _ = inventory.release_sequence_lease(&seq.id, &owner.id);
            continue;
        }
        let _lease_guard =
            start_lease_renewer(inventory.clone(), seq.id, owner.id, lease_ttl, lease_renew);
        tracing::info!("Claiming sequence {} ({})", seq.id, seq.name);
        let result: Result<()> = async {
            let root = resolve_sequence_root(&seq, &cfg.paths.projects_dir)?;
            let _ = inventory.update_sequence_folder(&seq.id, root.to_string_lossy().as_ref());

            let input_dir = resolve_capture_dir(&root)?;
            wait_for_capture_ready(&input_dir, stable_secs, min_images, timeout_secs).await?;

            if inventory
                .transition_sequence_status(
                    &seq.id,
                    SequenceStatus::Capturing,
                    SequenceStatus::Processing,
                )?
                .is_none()
            {
                tracing::warn!("Sequence {} lost claim before processing", seq.id);
                return Ok(());
            }
            run_sequence_processing(&seq, &root, cfg)?;
            inventory.update_sequence_status(&seq.id, SequenceStatus::Completed)?;
            let _ = inventory.record_sequence_success(&seq.id);
            Ok(())
        }
        .await;

        if let Err(err) = result {
            tracing::error!("Processing failed for {}: {}", seq.id, err);
            let _ = inventory.update_sequence_status(&seq.id, SequenceStatus::Failed);
            let _ = inventory.record_sequence_failure(&seq.id, &err.to_string());
        }
        let _ = inventory.release_sequence_lease(&seq.id, &owner.id);
    }

    Ok(())
}

async fn requeue_stale_sequences(inventory: Arc<Inventory>, max_failures: u32) -> Result<()> {
    let stale_statuses = [SequenceStatus::Capturing, SequenceStatus::Processing];
    for status in stale_statuses {
        let sequences = inventory.list_sequences_by_status(status.clone())?;
        for seq in sequences {
            let lease = inventory.get_sequence_lease(&seq.id)?;
            let lease_valid = lease
                .as_ref()
                .map(|l| l.expires_at > chrono::Utc::now())
                .unwrap_or(false);
            if lease_valid {
                continue;
            }
            let runtime = inventory.record_sequence_failure(&seq.id, "stale lease")?;
            if runtime.failure_count >= max_failures {
                let _ = inventory.update_sequence_status(&seq.id, SequenceStatus::Failed);
                continue;
            }
            let _ = inventory.update_sequence_status(&seq.id, SequenceStatus::Planned);
        }
    }
    Ok(())
}

fn start_lease_renewer(
    inventory: Arc<Inventory>,
    sequence_id: Uuid,
    owner_id: Uuid,
    ttl: ChronoDuration,
    interval: Duration,
) -> LeaseRenewer {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();
    tokio::spawn(async move {
        loop {
            if stop_clone.load(Ordering::Relaxed) {
                break;
            }
            let _ = inventory.renew_sequence_lease(&sequence_id, &owner_id, ttl);
            tokio::time::sleep(interval).await;
        }
    });
    LeaseRenewer { stop }
}

fn resolve_sequence_root(seq: &Sequence, projects_dir: &Path) -> Result<PathBuf> {
    if !seq.folder_path.is_empty() {
        let path = PathBuf::from(&seq.folder_path);
        if path.is_absolute() {
            return Ok(path);
        }
        return Ok(projects_dir.join(path));
    }
    anyhow::bail!("Sequence {} has no folder_path", seq.id)
}

fn resolve_capture_dir(root: &Path) -> Result<PathBuf> {
    let raw_images = root.join("raw").join("images");
    if raw_images.is_dir() {
        return Ok(raw_images);
    }
    let images = root.join("images");
    if images.is_dir() {
        return Ok(images);
    }
    anyhow::bail!("No capture directory found under {}", root.display())
}

async fn wait_for_capture_ready(
    input_dir: &Path,
    stable_secs: u64,
    min_images: usize,
    timeout_secs: u64,
) -> Result<()> {
    let sentinel = input_dir
        .parent()
        .unwrap_or(input_dir)
        .join(".trueshot_capture_complete");
    let mut last_count = 0usize;
    let mut stable_for = 0u64;
    let start = Instant::now();

    loop {
        if sentinel.exists() {
            tracing::info!("Capture sentinel detected for {}", input_dir.display());
            return Ok(());
        }

        let count = count_images(input_dir)?;
        if count >= min_images && count == last_count {
            stable_for += 1;
        } else {
            stable_for = 0;
        }
        last_count = count;

        if stable_for >= stable_secs {
            tracing::info!(
                "Capture stabilized ({} images) for {}",
                count,
                input_dir.display()
            );
            return Ok(());
        }

        if start.elapsed().as_secs() > timeout_secs {
            anyhow::bail!("Capture timeout for {}", input_dir.display());
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn count_images(dir: &Path) -> Result<usize> {
    let mut count = 0usize;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && is_image_path(&path) {
            count += 1;
        }
    }
    Ok(count)
}

fn is_image_path(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => matches!(
            ext.to_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "tif" | "tiff"
        ),
        None => false,
    }
}

fn run_sequence_processing(seq: &Sequence, root: &Path, cfg: &DaemonConfig) -> Result<()> {
    let output_dir = root.join("processed");
    let mode = cfg
        .daemon
        .default_mode
        .clone()
        .unwrap_or_else(|| "hybrid".to_string());
    let quality = cfg
        .daemon
        .default_quality
        .clone()
        .unwrap_or_else(|| "high".to_string());

    let trueshot_bin = resolve_trueshot_bin()?;
    tracing::info!(
        "Launching processing for {} (mode={}, quality={})",
        seq.id,
        mode,
        quality
    );

    let status = Command::new(trueshot_bin)
        .args([
            "process",
            "--input",
            root.to_string_lossy().as_ref(),
            "--output",
            output_dir.to_string_lossy().as_ref(),
            "--mode",
            mode.as_str(),
            "--quality",
            quality.as_str(),
        ])
        .status()
        .context("Failed to launch trueshot process")?;

    if !status.success() {
        anyhow::bail!("Processing failed for sequence {}", seq.id);
    }
    Ok(())
}

fn resolve_trueshot_bin() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Missing executable dir"))?;
    let candidate = dir.join("trueshot");
    if candidate.exists() {
        return Ok(candidate);
    }
    let alt = dir.join("trueshot-cli");
    if alt.exists() {
        return Ok(alt);
    }
    anyhow::bail!("Unable to locate trueshot binary near {}", exe.display())
}
