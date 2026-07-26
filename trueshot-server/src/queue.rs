use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use trueshot_core::scheduler::{JobInfo, JobStatus, SchedulerObserver};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueJobRecord {
    pub id: Uuid,
    pub request_id: Option<Uuid>,
    pub kind: String,
    pub name: String,
    pub status: String,
    pub progress: f32,
    pub attempts: i64,
    pub max_attempts: i64,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QueueJobPayload {
    pub id: Uuid,
    pub kind: String,
    pub name: String,
    pub payload: Value,
    pub attempts: i64,
    pub max_attempts: i64,
}

pub struct JobQueue {
    pool: SqlitePool,
    db_path: PathBuf,
}

impl JobQueue {
    pub async fn new(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create job db directory {}", parent.display())
            })?;
        }
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))?
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        let queue = Self {
            pool,
            db_path: db_path.to_path_buf(),
        };
        queue.ensure_schema().await?;
        Ok(queue)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub async fn enqueue(
        &self,
        request_id: Uuid,
        kind: &str,
        name: &str,
        payload: &Value,
        max_attempts: i64,
    ) -> Result<(QueueJobRecord, bool)> {
        if let Some(existing) = self.get_by_request_id(request_id).await? {
            return Ok((existing, false));
        }
        let now = Utc::now().to_rfc3339();
        let payload_json = payload.to_string();
        let id = request_id;
        sqlx::query(
            r#"INSERT INTO jobs
            (id, request_id, kind, name, payload, status, progress, created_at, attempt_count, max_attempts, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(id.to_string())
        .bind(request_id.to_string())
        .bind(kind)
        .bind(name)
        .bind(payload_json)
        .bind("pending")
        .bind(0.0_f32)
        .bind(&now)
        .bind(0_i64)
        .bind(max_attempts)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        let record = self
            .get_job(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to read job after insert"))?;
        Ok((record, true))
    }

    pub async fn list_jobs(&self) -> Result<Vec<QueueJobRecord>> {
        let rows = sqlx::query(
            r#"SELECT id, request_id, kind, name, status, progress, attempt_count, max_attempts,
               created_at, started_at, finished_at, last_error
               FROM jobs ORDER BY created_at DESC"#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_record).collect()
    }

    pub async fn get_job(&self, id: Uuid) -> Result<Option<QueueJobRecord>> {
        let row = sqlx::query(
            r#"SELECT id, request_id, kind, name, status, progress, attempt_count, max_attempts,
               created_at, started_at, finished_at, last_error
               FROM jobs WHERE id = ?"#,
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_record).transpose()
    }

    pub async fn get_job_detail(&self, id: Uuid) -> Result<Option<(QueueJobRecord, Value)>> {
        let row = sqlx::query(
            r#"SELECT id, request_id, kind, name, status, progress, attempt_count, max_attempts,
               created_at, started_at, finished_at, last_error, payload
               FROM jobs WHERE id = ?"#,
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_record_with_payload).transpose()
    }

    pub async fn get_by_request_id(&self, request_id: Uuid) -> Result<Option<QueueJobRecord>> {
        let row = sqlx::query(
            r#"SELECT id, request_id, kind, name, status, progress, attempt_count, max_attempts,
               created_at, started_at, finished_at, last_error
               FROM jobs WHERE request_id = ?"#,
        )
        .bind(request_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_record).transpose()
    }

    pub async fn load_pending_jobs(&self) -> Result<Vec<QueueJobPayload>> {
        let rows = sqlx::query(
            r#"SELECT id, kind, name, payload, attempt_count, max_attempts
               FROM jobs WHERE status IN ('pending', 'running')"#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_payload).collect()
    }

    pub async fn load_retry_jobs(&self) -> Result<Vec<QueueJobPayload>> {
        let rows = sqlx::query(
            r#"SELECT id, kind, name, payload, attempt_count, max_attempts
               FROM jobs WHERE status = 'failed' AND attempt_count < max_attempts"#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_payload).collect()
    }

    pub async fn mark_pending(&self, id: Uuid) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE jobs
               SET status = 'pending',
                   progress = 0.0,
                   started_at = NULL,
                   finished_at = NULL,
                   last_error = NULL,
                   updated_at = ?
               WHERE id = ?"#,
        )
        .bind(&now)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn sync_job_info(
        &self,
        job_id: Uuid,
        status: &str,
        progress: f32,
        started_at: Option<DateTime<Utc>>,
        finished_at: Option<DateTime<Utc>>,
        last_error: Option<String>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let started_at = started_at.map(|t| t.to_rfc3339());
        let finished_at = finished_at.map(|t| t.to_rfc3339());
        sqlx::query(
            r#"UPDATE jobs
               SET status = ?,
                   progress = ?,
                   started_at = COALESCE(started_at, ?),
                   finished_at = COALESCE(?, finished_at),
                   last_error = ?,
                   attempt_count = CASE
                       WHEN ? = 'running' AND started_at IS NULL THEN attempt_count + 1
                       ELSE attempt_count
                   END,
                   updated_at = ?
               WHERE id = ?"#,
        )
        .bind(status)
        .bind(progress)
        .bind(started_at)
        .bind(finished_at)
        .bind(last_error)
        .bind(status)
        .bind(&now)
        .bind(job_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn sync_progress(&self, job_id: Uuid, progress: f32) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE jobs
               SET progress = ?,
                   updated_at = ?
               WHERE id = ?"#,
        )
        .bind(progress)
        .bind(&now)
        .bind(job_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn ensure_schema(&self) -> Result<()> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS job_queue_migrations (
                version INTEGER PRIMARY KEY
            )"#,
        )
        .execute(&self.pool)
        .await?;
        let current: Option<i64> =
            sqlx::query_scalar("SELECT MAX(version) FROM job_queue_migrations")
                .fetch_one(&self.pool)
                .await?;
        if current.unwrap_or(0) < 1 {
            sqlx::query(
                r#"CREATE TABLE IF NOT EXISTS jobs (
                    id TEXT PRIMARY KEY,
                    request_id TEXT,
                    kind TEXT NOT NULL,
                    name TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    status TEXT NOT NULL,
                    progress REAL NOT NULL,
                    created_at TEXT NOT NULL,
                    started_at TEXT,
                    finished_at TEXT,
                    attempt_count INTEGER NOT NULL,
                    max_attempts INTEGER NOT NULL,
                    last_error TEXT,
                    updated_at TEXT NOT NULL
                )"#,
            )
            .execute(&self.pool)
            .await?;
            sqlx::query(
                r#"CREATE UNIQUE INDEX IF NOT EXISTS idx_jobs_request_id
                   ON jobs(request_id)"#,
            )
            .execute(&self.pool)
            .await?;
            sqlx::query("INSERT INTO job_queue_migrations (version) VALUES (1)")
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }
}

pub struct QueueObserver {
    queue: Arc<JobQueue>,
    webhook_state: Arc<Mutex<HashMap<Uuid, String>>>,
    webhook_client: Client,
}

impl QueueObserver {
    pub fn new(queue: Arc<JobQueue>) -> Self {
        Self {
            queue,
            webhook_state: Arc::new(Mutex::new(HashMap::new())),
            webhook_client: Client::new(),
        }
    }

    fn should_notify(&self, job_id: Uuid, status: &str) -> bool {
        let mut guard = self.webhook_state.lock().unwrap();
        match guard.get(&job_id) {
            Some(prev) if prev == status => false,
            _ => {
                guard.insert(job_id, status.to_string());
                true
            }
        }
    }
}

impl SchedulerObserver for QueueObserver {
    fn on_job_update(&self, job: JobInfo) {
        let queue = self.queue.clone();
        let client = self.webhook_client.clone();
        let (status, last_error) = match job.status.clone() {
            JobStatus::Pending => ("pending", None),
            JobStatus::Running => ("running", None),
            JobStatus::Completed => ("completed", None),
            JobStatus::Cancelled => ("cancelled", None),
            JobStatus::Failed(err) => ("failed", Some(err)),
        };
        let should_notify = self.should_notify(job.id, status);
        let started_at = job.started_at;
        let finished_at = job.finished_at;
        let progress = job.progress;
        tokio::spawn(async move {
            let _ = queue
                .sync_job_info(
                    job.id,
                    status,
                    progress,
                    started_at,
                    finished_at,
                    last_error,
                )
                .await;
            if should_notify {
                if let Ok(Some((record, payload))) = queue.get_job_detail(job.id).await {
                    let urls = extract_webhook_urls(&payload);
                    if !urls.is_empty() {
                        let webhook_payload = build_webhook_payload(&record, &payload);
                        for url in urls.into_iter().take(4) {
                            if let Ok(parsed) = reqwest::Url::parse(&url) {
                                if parsed.scheme() == "http" || parsed.scheme() == "https" {
                                    let res = client
                                        .post(parsed)
                                        .header("User-Agent", "TrueShot/automation")
                                        .json(&webhook_payload)
                                        .send()
                                        .await;
                                    if let Err(err) = res {
                                        tracing::warn!(
                                            "Webhook delivery failed for {}: {}",
                                            record.id,
                                            err
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    fn on_job_progress(&self, job_id: Uuid, progress: f32) {
        let queue = self.queue.clone();
        tokio::spawn(async move {
            let _ = queue.sync_progress(job_id, progress).await;
        });
    }
}

fn row_to_record(row: sqlx::sqlite::SqliteRow) -> Result<QueueJobRecord> {
    let id: String = row.try_get("id")?;
    let request_id: Option<String> = row.try_get("request_id")?;
    let status: String = row.try_get("status")?;
    let created_at: String = row.try_get("created_at")?;
    let started_at: Option<String> = row.try_get("started_at")?;
    let finished_at: Option<String> = row.try_get("finished_at")?;
    Ok(QueueJobRecord {
        id: Uuid::parse_str(&id)?,
        request_id: request_id.and_then(|value| Uuid::parse_str(&value).ok()),
        kind: row.try_get("kind")?,
        name: row.try_get("name")?,
        status,
        progress: row.try_get::<f64, _>("progress")? as f32,
        attempts: row.try_get("attempt_count")?,
        max_attempts: row.try_get("max_attempts")?,
        created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
        started_at: started_at
            .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
            .map(|dt| dt.with_timezone(&Utc)),
        finished_at: finished_at
            .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
            .map(|dt| dt.with_timezone(&Utc)),
        last_error: row.try_get("last_error")?,
    })
}

fn row_to_record_with_payload(row: sqlx::sqlite::SqliteRow) -> Result<(QueueJobRecord, Value)> {
    let payload: String = row.try_get("payload")?;
    let parsed: Value = serde_json::from_str(&payload)?;
    let record = row_to_record(row)?;
    Ok((record, parsed))
}

fn row_to_payload(row: sqlx::sqlite::SqliteRow) -> Result<QueueJobPayload> {
    let id: String = row.try_get("id")?;
    let payload: String = row.try_get("payload")?;
    let parsed: Value = serde_json::from_str(&payload)?;
    Ok(QueueJobPayload {
        id: Uuid::parse_str(&id)?,
        kind: row.try_get("kind")?,
        name: row.try_get("name")?,
        payload: parsed,
        attempts: row.try_get("attempt_count")?,
        max_attempts: row.try_get("max_attempts")?,
    })
}

fn extract_webhook_urls(payload: &Value) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(url) = payload.get("webhook_url").and_then(|value| value.as_str()) {
        urls.push(url.to_string());
    }
    if let Some(list) = payload.get("webhooks").and_then(|value| value.as_array()) {
        for entry in list {
            if let Some(url) = entry.as_str() {
                urls.push(url.to_string());
            }
        }
    }
    urls
}

fn build_webhook_payload(record: &QueueJobRecord, payload: &Value) -> Value {
    serde_json::json!({
        "event": "job.status",
        "job": {
            "id": record.id,
            "request_id": record.request_id,
            "kind": record.kind,
            "name": record.name,
            "status": record.status,
            "progress": record.progress,
            "attempts": record.attempts,
            "max_attempts": record.max_attempts,
            "created_at": record.created_at,
            "started_at": record.started_at,
            "finished_at": record.finished_at,
            "last_error": record.last_error,
        },
        "payload": payload,
        "sent_at": Utc::now(),
    })
}
