use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::oneshot;
use utoipa::ToSchema;

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct IntervalometerRamp {
    #[serde(default)]
    pub shutter_start: Option<String>,
    #[serde(default)]
    pub shutter_end: Option<String>,
    #[serde(default)]
    pub iso_start: Option<String>,
    #[serde(default)]
    pub iso_end: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct IntervalometerStatus {
    pub camera_id: String,
    pub active: bool,
    pub interval_ms: u64,
    #[serde(default)]
    pub total_frames: Option<u32>,
    pub captured_frames: u32,
    pub started_at: String,
    #[serde(default)]
    pub last_capture_at: Option<String>,
    #[serde(default)]
    pub next_capture_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub ramp: Option<IntervalometerRamp>,
}

pub struct IntervalometerTask {
    pub status: IntervalometerStatus,
    pub cancel: Option<oneshot::Sender<()>>,
}

pub struct IntervalometerState {
    pub tasks: HashMap<String, IntervalometerTask>,
}

impl IntervalometerState {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    pub fn status(&self, camera_id: &str) -> Option<IntervalometerStatus> {
        self.tasks.get(camera_id).map(|task| task.status.clone())
    }

    pub fn set_task(&mut self, camera_id: String, task: IntervalometerTask) {
        self.tasks.insert(camera_id, task);
    }

    pub fn stop_task(&mut self, camera_id: &str) -> Option<IntervalometerStatus> {
        if let Some(task) = self.tasks.get_mut(camera_id) {
            task.status.active = false;
            task.status.next_capture_at = None;
            task.status.last_capture_at = Some(Utc::now().to_rfc3339());
            if let Some(cancel) = task.cancel.take() {
                let _ = cancel.send(());
            }
            return Some(task.status.clone());
        }
        None
    }
}
