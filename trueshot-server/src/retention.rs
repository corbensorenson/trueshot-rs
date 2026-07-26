use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::audit::AuditRecord;
use crate::state::AppState;

pub fn spawn_retention_task(state: actix_web::web::Data<AppState>) {
    let policy = state.config.privacy.clone();
    if policy.retention_raw_days.is_none()
        && policy.retention_processed_days.is_none()
        && policy.retention_output_days.is_none()
        && policy.audit_log_days.is_none()
    {
        return;
    }

    let state_clone = state.clone();
    tokio::spawn(async move {
        loop {
            if let Err(err) = enforce_retention(&state_clone).await {
                tracing::warn!("retention task failed: {}", err);
            }
            tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
        }
    });
}

async fn enforce_retention(state: &actix_web::web::Data<AppState>) -> Result<()> {
    let policy = &state.config.privacy;
    let projects_dir = &state.config.paths.projects_dir;
    let now = Utc::now();

    let entries = std::fs::read_dir(projects_dir)
        .with_context(|| format!("Failed to read projects dir: {}", projects_dir.display()))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('_') {
            continue;
        }
        let project_age_days = project_age_days(&path, now).unwrap_or(0);

        if let Some(days) = policy.retention_raw_days {
            if days > 0 && project_age_days > days as i64 {
                let raw_dir = path.join("raw");
                if raw_dir.exists() {
                    let _ = std::fs::remove_dir_all(&raw_dir);
                }
            }
        }
        if let Some(days) = policy.retention_processed_days {
            if days > 0 && project_age_days > days as i64 {
                let processed_dir = path.join("processed");
                if processed_dir.exists() {
                    let _ = std::fs::remove_dir_all(&processed_dir);
                }
            }
        }
        if let Some(days) = policy.retention_output_days {
            if days > 0 && project_age_days > days as i64 {
                let output_dir = path.join("output");
                if output_dir.exists() {
                    let _ = std::fs::remove_dir_all(&output_dir);
                }
            }
        }
    }

    if let Some(days) = policy.audit_log_days {
        if days > 0 {
            let anchor_enabled = policy
                .audit_anchor_url
                .as_ref()
                .map(|url| !url.trim().is_empty())
                .unwrap_or(false)
                || policy.audit_anchor_required.unwrap_or(false);
            if anchor_enabled {
                tracing::warn!("Skipping audit log retention because audit anchoring is enabled.");
            } else if prune_audit_log(&state.audit.path().to_path_buf(), days as i64).is_ok() {
                let _ = state.audit.refresh_last_hash();
            }
        }
    }

    Ok(())
}

fn project_age_days(project_dir: &Path, now: DateTime<Utc>) -> Option<i64> {
    let manifest = project_dir.join("project.json");
    if let Ok(content) = std::fs::read_to_string(&manifest) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(ts) = value.get("created_at").and_then(|v| v.as_str()) {
                if let Ok(parsed) = DateTime::parse_from_rfc3339(ts) {
                    return Some((now - parsed.with_timezone(&Utc)).num_days());
                }
            }
        }
    }
    std::fs::metadata(project_dir)
        .and_then(|m| m.modified().or_else(|_| m.created()))
        .ok()
        .map(DateTime::<Utc>::from)
        .map(|ts| (now - ts).num_days())
}

fn prune_audit_log(path: &PathBuf, retention_days: i64) -> Result<()> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(()),
    };
    let reader = std::io::BufReader::new(file);
    let cutoff = Utc::now() - Duration::days(retention_days);
    let mut kept: Vec<AuditRecord> = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<AuditRecord>(&line) {
            if record.event.timestamp > cutoff {
                kept.push(record);
            }
        }
    }
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    let mut prev_hash = "genesis".to_string();
    for record in kept {
        let mut rewritten = record.clone();
        rewritten.prev_hash = prev_hash;
        rewritten.hash.clear();
        let payload = serde_json::to_string(&rewritten)?;
        let mut hasher = Sha256::new();
        hasher.update(payload.as_bytes());
        rewritten.hash = hex::encode(hasher.finalize());
        prev_hash = rewritten.hash.clone();
        writeln!(out, "{}", serde_json::to_string(&rewritten)?)?;
    }
    Ok(())
}
