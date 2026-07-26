use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanSession {
    pub id: String,
    pub start_time: DateTime<Utc>,
    pub events: Vec<CaptureEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureEvent {
    pub timestamp: DateTime<Utc>,
    pub turntable_angle: f32,
    pub cameras: Vec<String>, // Camera IDs triggered
    pub capture_type: String, // "Single", "FocusStack", etc.
    pub file_count_expected: usize,
    #[serde(default)]
    pub file_count_verified: Option<usize>,
    #[serde(default)]
    pub verification_hashes: Option<Vec<String>>,
}

impl Default for ScanSession {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanSession {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            start_time: Utc::now(),
            events: Vec::new(),
        }
    }

    pub fn log_capture(
        &mut self,
        angle: f32,
        cameras: Vec<String>,
        capture_type: &str,
        file_count: usize,
    ) {
        self.events.push(CaptureEvent {
            timestamp: Utc::now(),
            turntable_angle: angle,
            cameras,
            capture_type: capture_type.to_string(),
            file_count_expected: file_count,
            file_count_verified: None,
            verification_hashes: None,
        });
    }

    pub fn record_verification(&mut self, index: usize, count: usize, hashes: Vec<String>) {
        if let Some(event) = self.events.get_mut(index) {
            event.file_count_verified = Some(count);
            event.verification_hashes = Some(hashes);
        }
    }
}
