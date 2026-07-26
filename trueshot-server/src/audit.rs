use crate::config::PrivacyConfig;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use trueshot_core::security::provenance::{verify_signature, ProvenanceSigner};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AuditAnchorConfig {
    pub url: String,
    pub timeout_seconds: u64,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub actor: String,
    pub role: String,
    pub action: String,
    pub resource: String,
    pub status: String,
    pub ip: Option<String>,
    pub details: serde_json::Value,
}

impl AuditEvent {
    pub fn new(
        actor: String,
        role: String,
        action: impl Into<String>,
        resource: impl Into<String>,
        status: impl Into<String>,
        ip: Option<String>,
        details: serde_json::Value,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            actor,
            role,
            action: action.into(),
            resource: resource.into(),
            status: status.into(),
            ip,
            details,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    pub event: AuditEvent,
    pub prev_hash: String,
    pub hash: String,
}

pub struct AuditLog {
    path: PathBuf,
    last_hash: Mutex<String>,
    anchor: Option<AuditAnchor>,
}

impl AuditLog {
    pub fn new(path: PathBuf, anchor: Option<AuditAnchorConfig>) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create audit directory: {}", parent.display())
            })?;
        }
        let last_hash = read_last_hash(&path).unwrap_or_else(|| "genesis".to_string());
        let anchor = if let Some(config) = anchor {
            let anchor_path = path.with_extension("anchor.log");
            Some(AuditAnchor::new(config, anchor_path)?)
        } else {
            None
        };
        Ok(Self {
            path,
            last_hash: Mutex::new(last_hash),
            anchor,
        })
    }

    pub fn append(&self, event: AuditEvent) -> Result<AuditRecord> {
        let mut guard = self.last_hash.lock().unwrap();
        let prev_hash = guard.clone();
        let mut record = AuditRecord {
            event,
            prev_hash,
            hash: String::new(),
        };
        let payload = serde_json::to_string(&record)?;
        let mut hasher = Sha256::new();
        hasher.update(payload.as_bytes());
        record.hash = hex::encode(hasher.finalize());

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("Failed to open audit log: {}", self.path.display()))?;
        writeln!(file, "{}", serde_json::to_string(&record)?)?;
        *guard = record.hash.clone();
        if let Some(anchor) = &self.anchor {
            if let Err(err) = anchor.anchor_record(&record) {
                if anchor.required {
                    return Err(err);
                }
                tracing::warn!("Audit anchor failed: {}", err);
            }
        }
        Ok(record)
    }

    pub fn append_with_redaction(
        &self,
        event: AuditEvent,
        policy: &PrivacyConfig,
    ) -> Result<AuditRecord> {
        let redacted = redact_event(event, policy);
        self.append(redacted)
    }

    pub fn read(&self, limit: usize) -> Result<Vec<AuditRecord>> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(_) => return Ok(Vec::new()),
        };
        let reader = BufReader::new(file);
        let mut records = Vec::new();
        for line in reader.lines().flatten() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(record) = serde_json::from_str::<AuditRecord>(&line) {
                records.push(record);
            }
        }
        if records.len() > limit {
            Ok(records[records.len() - limit..].to_vec())
        } else {
            Ok(records)
        }
    }

    pub fn verify(&self) -> Result<bool> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(_) => return Ok(true),
        };
        let reader = BufReader::new(file);
        let mut prev_hash = "genesis".to_string();
        for line in reader.lines().flatten() {
            if line.trim().is_empty() {
                continue;
            }
            let record: AuditRecord = serde_json::from_str(&line)?;
            if record.prev_hash != prev_hash {
                return Ok(false);
            }
            let mut temp = record.clone();
            temp.hash.clear();
            let payload = serde_json::to_string(&temp)?;
            let mut hasher = Sha256::new();
            hasher.update(payload.as_bytes());
            let computed = hex::encode(hasher.finalize());
            if computed != record.hash {
                return Ok(false);
            }
            prev_hash = record.hash.clone();
        }
        Ok(true)
    }

    pub fn verify_anchor(&self) -> Result<AuditAnchorVerification> {
        let anchor_path = self
            .anchor
            .as_ref()
            .map(|a| a.anchor_path.clone())
            .unwrap_or_else(|| self.path.with_extension("anchor.log"));

        if !anchor_path.exists() {
            return Ok(AuditAnchorVerification {
                ok: false,
                anchor_records: 0,
                anchored_records: 0,
                missing_records: 0,
                invalid_signatures: 0,
                anchor_errors: 0,
                unanchored_tail: 0,
                last_anchor_hash: None,
                issues: vec!["anchor_log_missing".to_string()],
            });
        }

        let audit_records = read_audit_records(&self.path)?;
        let mut audit_index: HashMap<String, (usize, String, String)> = HashMap::new();
        for (idx, record) in audit_records.iter().enumerate() {
            audit_index.insert(
                record.hash.clone(),
                (
                    idx,
                    record.prev_hash.clone(),
                    record.event.timestamp.to_rfc3339(),
                ),
            );
        }

        let anchor_records = read_anchor_records(&anchor_path)?;
        let mut anchored_records = 0usize;
        let mut missing_records = 0usize;
        let mut invalid_signatures = 0usize;
        let mut anchor_errors = 0usize;
        let mut issues = Vec::new();
        let mut last_anchor_hash = None;

        for anchor in &anchor_records {
            last_anchor_hash = Some(anchor.payload.record_hash.clone());
            if anchor.status != "anchored" {
                anchor_errors += 1;
                issues.push(format!(
                    "anchor_status_not_ok:{}",
                    anchor.payload.record_hash
                ));
            }
            let expected_payload = format!(
                "{}|{}|{}|{}|{}",
                anchor.payload.record_hash,
                anchor.payload.prev_hash,
                anchor.payload.event_id,
                anchor.payload.timestamp,
                anchor.payload.key_id,
            );
            if anchor.payload.signature_payload != expected_payload {
                invalid_signatures += 1;
                issues.push(format!(
                    "anchor_payload_mismatch:{}",
                    anchor.payload.record_hash
                ));
                continue;
            }
            let signature_ok = verify_signature(
                &anchor.payload.signer_public_key,
                &anchor.payload.signature_payload,
                &anchor.payload.signature,
            )
            .unwrap_or(false);
            if !signature_ok {
                invalid_signatures += 1;
                issues.push(format!(
                    "anchor_signature_invalid:{}",
                    anchor.payload.record_hash
                ));
                continue;
            }

            match audit_index.get(&anchor.payload.record_hash) {
                Some((_, prev_hash, timestamp)) => {
                    if prev_hash != &anchor.payload.prev_hash {
                        missing_records += 1;
                        issues.push(format!(
                            "anchor_prev_hash_mismatch:{}",
                            anchor.payload.record_hash
                        ));
                    } else if timestamp != &anchor.payload.timestamp {
                        missing_records += 1;
                        issues.push(format!(
                            "anchor_timestamp_mismatch:{}",
                            anchor.payload.record_hash
                        ));
                    } else {
                        anchored_records += 1;
                    }
                }
                None => {
                    missing_records += 1;
                    issues.push(format!(
                        "anchor_record_missing:{}",
                        anchor.payload.record_hash
                    ));
                }
            }
        }

        let unanchored_tail = if let Some(hash) = last_anchor_hash.as_ref() {
            if let Some((idx, _, _)) = audit_index.get(hash) {
                audit_records.len().saturating_sub(idx + 1)
            } else {
                audit_records.len()
            }
        } else {
            audit_records.len()
        };

        if unanchored_tail > 0 {
            issues.push(format!("unanchored_tail:{}", unanchored_tail));
        }

        let ok = issues.is_empty();

        Ok(AuditAnchorVerification {
            ok,
            anchor_records: anchor_records.len(),
            anchored_records,
            missing_records,
            invalid_signatures,
            anchor_errors,
            unanchored_tail,
            last_anchor_hash,
            issues,
        })
    }

    pub fn refresh_last_hash(&self) -> Result<()> {
        let last_hash = read_last_hash(&self.path).unwrap_or_else(|| "genesis".to_string());
        let mut guard = self.last_hash.lock().unwrap();
        *guard = last_hash;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn redact_event(mut event: AuditEvent, policy: &PrivacyConfig) -> AuditEvent {
    if policy.audit_redact_actor.unwrap_or(false) {
        event.actor = "redacted".to_string();
    }
    if policy.audit_redact_ip.unwrap_or(true) {
        event.ip = None;
    }
    if policy.audit_redact_resource.unwrap_or(false) {
        event.resource = "redacted".to_string();
    }
    if policy.audit_redact_details.unwrap_or(false) {
        let mut value = event.details;
        let keys = audit_redact_keys(policy);
        redact_json_value(&mut value, &keys);
        event.details = value;
    }
    event
}

fn audit_redact_keys(policy: &PrivacyConfig) -> HashSet<String> {
    let mut keys = vec![
        "token",
        "access_token",
        "refresh_token",
        "api_key",
        "secret",
        "password",
        "authorization",
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect::<HashSet<_>>();
    if let Some(extra) = &policy.audit_redact_keys {
        for key in extra {
            keys.insert(key.to_lowercase());
        }
    }
    keys
}

fn redact_json_value(value: &mut serde_json::Value, keys: &HashSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                let key_lower = k.to_lowercase();
                if keys.contains(&key_lower) {
                    *v = serde_json::Value::String("[redacted]".to_string());
                } else {
                    redact_json_value(v, keys);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_json_value(item, keys);
            }
        }
        _ => {}
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct AuditAnchorPayload {
    anchor_version: u32,
    record_hash: String,
    prev_hash: String,
    event_id: String,
    timestamp: String,
    key_id: String,
    signer_public_key: String,
    signature: String,
    signature_payload: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AuditAnchorRecord {
    anchored_at: String,
    url: String,
    status: String,
    error: Option<String>,
    http_status: Option<u16>,
    receipt: Option<String>,
    payload: AuditAnchorPayload,
}

#[derive(Debug, Clone)]
struct AuditAnchor {
    url: String,
    client: reqwest::blocking::Client,
    required: bool,
    anchor_path: PathBuf,
}

impl AuditAnchor {
    fn new(config: AuditAnchorConfig, anchor_path: PathBuf) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds.max(1)))
            .build()
            .with_context(|| "Failed to build audit anchor client")?;
        Ok(Self {
            url: config.url,
            client,
            required: config.required,
            anchor_path,
        })
    }

    fn anchor_record(&self, record: &AuditRecord) -> Result<()> {
        let signer = ProvenanceSigner::global();
        let key_id = signer.key_id();
        let signature_payload = format!(
            "{}|{}|{}|{}|{}",
            record.hash,
            record.prev_hash,
            record.event.id,
            record.event.timestamp.to_rfc3339(),
            key_id
        );
        let signature = signer.sign_bytes(signature_payload.as_bytes());
        let payload = AuditAnchorPayload {
            anchor_version: 1,
            record_hash: record.hash.clone(),
            prev_hash: record.prev_hash.clone(),
            event_id: record.event.id.clone(),
            timestamp: record.event.timestamp.to_rfc3339(),
            key_id,
            signer_public_key: signer.public_key_hex(),
            signature,
            signature_payload,
        };

        let mut status = "anchored".to_string();
        let mut error = None;
        let mut http_status = None;
        let mut receipt = None;

        match self.client.post(&self.url).json(&payload).send() {
            Ok(resp) => {
                let status_code = resp.status();
                http_status = Some(status_code.as_u16());
                let body = resp.text().unwrap_or_default();
                if !body.trim().is_empty() {
                    receipt = Some(truncate_receipt(&body));
                }
                if !status_code.is_success() {
                    status = "anchor_failed".to_string();
                    error = Some(format!("HTTP {}", status_code.as_u16()));
                }
            }
            Err(err) => {
                status = "anchor_failed".to_string();
                error = Some(err.to_string());
            }
        }

        self.write_anchor_log(
            &payload,
            &status,
            error.as_deref(),
            http_status,
            receipt.as_deref(),
        )?;

        if status == "anchor_failed" {
            anyhow::bail!(
                "Audit anchor request failed ({})",
                error.unwrap_or_else(|| "unknown error".to_string())
            );
        }

        Ok(())
    }

    fn write_anchor_log(
        &self,
        payload: &AuditAnchorPayload,
        status: &str,
        error: Option<&str>,
        http_status: Option<u16>,
        receipt: Option<&str>,
    ) -> Result<()> {
        if let Some(parent) = self.anchor_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create audit anchor directory: {}",
                    parent.display()
                )
            })?;
        }
        let record = AuditAnchorRecord {
            anchored_at: Utc::now().to_rfc3339(),
            url: self.url.clone(),
            status: status.to_string(),
            error: error.map(|s| s.to_string()),
            http_status,
            receipt: receipt.map(|s| s.to_string()),
            payload: payload.clone(),
        };
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.anchor_path)
            .with_context(|| {
                format!(
                    "Failed to open audit anchor log: {}",
                    self.anchor_path.display()
                )
            })?;
        writeln!(file, "{}", serde_json::to_string(&record)?)?;
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct AuditAnchorVerification {
    pub ok: bool,
    pub anchor_records: usize,
    pub anchored_records: usize,
    pub missing_records: usize,
    pub invalid_signatures: usize,
    pub anchor_errors: usize,
    pub unanchored_tail: usize,
    pub last_anchor_hash: Option<String>,
    pub issues: Vec<String>,
}

fn read_audit_records(path: &Path) -> Result<Vec<AuditRecord>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(Vec::new()),
    };
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines().flatten() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<AuditRecord>(&line) {
            records.push(record);
        }
    }
    Ok(records)
}

fn read_anchor_records(path: &Path) -> Result<Vec<AuditAnchorRecord>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(Vec::new()),
    };
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines().flatten() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<AuditAnchorRecord>(&line) {
            records.push(record);
        }
    }
    Ok(records)
}

fn truncate_receipt(body: &str) -> String {
    const MAX: usize = 4096;
    if body.len() <= MAX {
        body.to_string()
    } else {
        let mut out = body[..MAX].to_string();
        out.push_str("...");
        out
    }
}

fn read_last_hash(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut last_line = None;
    for line in reader.lines().flatten() {
        if !line.trim().is_empty() {
            last_line = Some(line);
        }
    }
    let last_line = last_line?;
    let record: AuditRecord = serde_json::from_str(&last_line).ok()?;
    Some(record.hash)
}
