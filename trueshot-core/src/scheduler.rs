use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

pub mod cluster;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobInfo {
    pub id: Uuid,
    pub name: String,
    pub status: JobStatus,
    pub progress: f32, // 0.0 - 1.0
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// Serializable payload for dispatching a job to a remote cluster node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteJobPayload {
    pub kind: String,
    pub name: String,
    pub payload: Value,
}

/// A unit of work to be executed
#[async_trait]
pub trait Job: Send + Sync + 'static {
    /// Return a human readable name
    fn name(&self) -> &str;

    /// Execute the job logic
    /// `progress_tx` can be used to send progress updates (0.0 - 1.0)
    async fn execute(&self, progress_tx: mpsc::Sender<f32>) -> Result<()>;

    /// Report whether an execution error was caused by an operator cancellation.
    fn is_cancelled(&self) -> bool {
        false
    }

    /// Optional serialization for remote dispatch.
    /// Return None to indicate this job must run locally.
    fn remote_payload(&self) -> Option<RemoteJobPayload> {
        None
    }
}

/// The Scheduler manages the job queue and execution
pub struct Scheduler {
    queue: mpsc::Sender<QueuedJob>,
    jobs: Arc<DashMap<Uuid, JobInfo>>,
    observer: Option<Arc<dyn SchedulerObserver>>,
}

struct QueuedJob {
    id: Uuid,
    job: Box<dyn Job>,
}

pub trait SchedulerObserver: Send + Sync {
    fn on_job_update(&self, _job: JobInfo) {}
    fn on_job_progress(&self, _job_id: Uuid, _progress: f32) {}
}

impl Scheduler {
    pub fn new(worker_count: usize) -> Self {
        Self::with_observer(worker_count, None)
    }

    pub fn with_observer(
        worker_count: usize,
        observer: Option<Arc<dyn SchedulerObserver>>,
    ) -> Self {
        let (tx, mut rx) = mpsc::channel::<QueuedJob>(100);
        let jobs: Arc<DashMap<Uuid, JobInfo>> = Arc::new(DashMap::new());
        let jobs_clone = jobs.clone();
        let observer_clone = observer.clone();

        // Spawn worker pool
        // Simple dispatcher spawning tasks limited by semaphore
        tokio::spawn(async move {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(worker_count));

            while let Some(queued) = rx.recv().await {
                // If None, channel closed, exit
                let permit = semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("Semaphore closed");
                let jobs_map = jobs_clone.clone();
                let job_id = queued.id;
                let job_task = queued.job;
                let observer = observer_clone.clone();

                tokio::spawn(async move {
                    // Update Status to Running
                    if let Some(mut info) = jobs_map.get_mut(&job_id) {
                        info.status = JobStatus::Running;
                        info.started_at = Some(Utc::now());
                    }
                    if let Some(obs) = observer.as_ref() {
                        if let Some(info) = jobs_map.get(&job_id).map(|r| r.clone()) {
                            obs.on_job_update(info);
                        }
                    }

                    let (prog_tx, mut prog_rx) = mpsc::channel(10);

                    // Spawn progress listener
                    let jobs_map_prog = jobs_map.clone();
                    let observer_progress = observer.clone();
                    tokio::spawn(async move {
                        while let Some(p) = prog_rx.recv().await {
                            if let Some(mut info) = jobs_map_prog.get_mut(&job_id) {
                                info.progress = p;
                            }
                            if let Some(obs) = observer_progress.as_ref() {
                                obs.on_job_progress(job_id, p);
                            }
                            if let Some(obs) = observer_progress.as_ref() {
                                if let Some(info) = jobs_map_prog.get(&job_id).map(|r| r.clone()) {
                                    obs.on_job_update(info);
                                }
                            }
                        }
                    });

                    tracing::info!("Starting job: {}", job_id);
                    match job_task.execute(prog_tx).await {
                        Ok(_) => {
                            if let Some(mut info) = jobs_map.get_mut(&job_id) {
                                info.status = JobStatus::Completed;
                                info.finished_at = Some(Utc::now());
                                info.progress = 1.0;
                            }
                            if let Some(obs) = observer.as_ref() {
                                if let Some(info) = jobs_map.get(&job_id).map(|r| r.clone()) {
                                    obs.on_job_update(info);
                                }
                            }
                            tracing::info!("Job completed: {}", job_id);
                        }
                        Err(e) => {
                            if let Some(mut info) = jobs_map.get_mut(&job_id) {
                                info.status = if job_task.is_cancelled() {
                                    JobStatus::Cancelled
                                } else {
                                    JobStatus::Failed(e.to_string())
                                };
                                info.finished_at = Some(Utc::now());
                            }
                            if let Some(obs) = observer.as_ref() {
                                if let Some(info) = jobs_map.get(&job_id).map(|r| r.clone()) {
                                    obs.on_job_update(info);
                                }
                            }
                            if job_task.is_cancelled() {
                                tracing::info!("Job cancelled: {}", job_id);
                            } else {
                                tracing::error!("Job failed: {}: {:?}", job_id, e);
                            }
                        }
                    }

                    drop(permit); // Release slot
                });
            }
        });

        Self {
            queue: tx,
            jobs,
            observer,
        }
    }

    pub async fn submit<J: Job>(&self, job: J) -> Result<Uuid> {
        let id = Uuid::new_v4();
        self.submit_with_id(id, job).await
    }

    pub async fn submit_with_id<J: Job>(&self, id: Uuid, job: J) -> Result<Uuid> {
        if self.jobs.contains_key(&id) {
            return Err(anyhow::anyhow!("Job id already exists"));
        }
        let info = JobInfo {
            id,
            name: job.name().to_string(),
            status: JobStatus::Pending,
            progress: 0.0,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
        };

        self.jobs.insert(id, info.clone());
        if let Some(obs) = self.observer.as_ref() {
            obs.on_job_update(info);
        }

        self.queue
            .send(QueuedJob {
                id,
                job: Box::new(job),
            })
            .await
            .map_err(|_| anyhow::anyhow!("Scheduler closed"))?;

        Ok(id)
    }

    pub fn get_job(&self, id: &Uuid) -> Option<JobInfo> {
        self.jobs.get(id).map(|r| r.clone())
    }

    pub fn list_jobs(&self) -> Vec<JobInfo> {
        self.jobs.iter().map(|r| r.clone()).collect()
    }
}
