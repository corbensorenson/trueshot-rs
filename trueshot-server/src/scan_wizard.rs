use chrono::{DateTime, Utc};
use image::RgbImage;
use image::GrayImage;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::scan_types::{BoundingBox, ObjectAnalysis, ScanPlan, ScanProgress, StepIntegrity, QualityAssessment, ScaleAnchor};

#[derive(Debug)]
pub struct ScanWizardState {
    pub background: Option<RgbImage>,
    pub background_captured_at: Option<DateTime<Utc>>,
    pub background_frames: u32,
    pub last_detection: Option<DetectionState>,
    pub last_analysis: Option<ObjectAnalysis>,
    pub last_quality: Option<QualityAssessment>,
    pub last_quality_at: Option<DateTime<Utc>>,
    pub last_uncertainty: Option<GrayImage>,
    pub last_preview: Option<RgbImage>,
    pub quality_history: Vec<QualityHistoryEntry>,
    pub plan: Option<ScanPlan>,
    pub runtime: Option<ScanRuntime>,
    pub scale_anchor: Option<ScaleAnchor>,
}

impl Default for ScanWizardState {
    fn default() -> Self {
        Self {
            background: None,
            background_captured_at: None,
            background_frames: 0,
            last_detection: None,
            last_analysis: None,
            last_quality: None,
            last_quality_at: None,
            last_uncertainty: None,
            last_preview: None,
            quality_history: Vec::new(),
            plan: None,
            runtime: None,
            scale_anchor: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct QualityHistoryEntry {
    pub captured_at: DateTime<Utc>,
    pub score: f32,
    pub pass: bool,
    pub issues: Vec<String>,
    pub actions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DetectionState {
    pub bbox: Option<BoundingBox>,
    pub stable_since: Option<Instant>,
    pub last_seen: Instant,
    pub confidence: f32,
}

#[derive(Debug)]
pub struct ScanRuntime {
    pub session_id: String,
    pub status: String,
    pub current_step: usize,
    pub total_steps: usize,
    pub photos_captured: u32,
    pub current_instruction: String,
    pub error_message: Option<String>,
    pub plan: ScanPlan,
    pub waiting_step: Option<usize>,
    pub started_at: DateTime<Utc>,
    pub cancel: Arc<AtomicBool>,
    pub step_integrity: Vec<StepIntegrity>,
    pub warnings: Vec<String>,
    pub quality: Option<QualityAssessment>,
    pub plan_revision: u32,
    pub plan_history: Vec<PlanRevision>,
    pub coverage: Vec<CoverageGrid>,
    pub added_view_keys: HashSet<String>,
    pub last_adapt_step: Option<usize>,
    pub auto_capture: bool,
    pub manual_capture_step: Option<usize>,
}

impl ScanRuntime {
    pub fn new(plan: ScanPlan, session_id: String, auto_capture: bool) -> Self {
        let coverage = build_coverage_grids(&plan);
        let now = Utc::now();
        let plan_history = vec![PlanRevision {
            revision: 0,
            created_at: now,
            reason: "initial".to_string(),
            total_steps: plan.steps.len(),
            total_photos: plan.total_photos,
            added_steps: 0,
        }];
        Self {
            session_id,
            status: "capturing".to_string(),
            current_step: 0,
            total_steps: plan.steps.len(),
            photos_captured: 0,
            current_instruction: String::new(),
            error_message: None,
            plan,
            waiting_step: None,
            started_at: Utc::now(),
            cancel: Arc::new(AtomicBool::new(false)),
            step_integrity: Vec::new(),
            warnings: Vec::new(),
            quality: None,
            plan_revision: 0,
            plan_history,
            coverage,
            added_view_keys: HashSet::new(),
            last_adapt_step: None,
            auto_capture,
            manual_capture_step: None,
        }
    }

    pub fn progress(&self) -> ScanProgress {
        let elapsed_seconds = (Utc::now() - self.started_at)
            .num_seconds()
            .max(0) as u64;
        ScanProgress {
            status: self.status.clone(),
            current_step: self.current_step as u32,
            total_steps: self.total_steps as u32,
            photos_captured: self.photos_captured,
            elapsed_seconds,
            current_instruction: self.current_instruction.clone(),
            error_message: self.error_message.clone(),
            step_integrity: self.step_integrity.clone(),
            warnings: self.warnings.clone(),
            quality: self.quality.clone(),
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn record_integrity(&mut self, integrity: StepIntegrity) {
        if let Some(existing) = self
            .step_integrity
            .iter_mut()
            .find(|entry| entry.step_index == integrity.step_index)
        {
            *existing = integrity;
        } else {
            self.step_integrity.push(integrity);
        }
    }

    pub fn set_warnings(&mut self, warnings: Vec<String>) {
        self.warnings = warnings;
    }

    pub fn record_plan_revision(&mut self, reason: &str, added_steps: usize) {
        self.plan_revision = self.plan_revision.saturating_add(1);
        self.plan_history.push(PlanRevision {
            revision: self.plan_revision,
            created_at: Utc::now(),
            reason: reason.to_string(),
            total_steps: self.plan.steps.len(),
            total_photos: self.plan.total_photos,
            added_steps,
        });
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRevision {
    pub revision: u32,
    pub created_at: DateTime<Utc>,
    pub reason: String,
    pub total_steps: usize,
    pub total_photos: u32,
    pub added_steps: usize,
}

#[derive(Debug, Clone)]
pub struct CoverageGrid {
    pub azimuth_bins: usize,
    pub elevation_bins: usize,
    pub counts: Vec<f32>,
}

impl CoverageGrid {
    pub fn new(azimuth_bins: usize, elevation_bins: usize) -> Self {
        let bins = azimuth_bins.max(1);
        let elevations = elevation_bins.max(1);
        Self {
            azimuth_bins: bins,
            elevation_bins: elevations,
            counts: vec![0.0; bins * elevations],
        }
    }

    pub fn update(&mut self, azimuth_bin: usize, elevation_bin: usize, value: f32) {
        let idx = self.index(azimuth_bin, elevation_bin);
        if idx < self.counts.len() {
            self.counts[idx] += value;
        }
    }

    pub fn get(&self, azimuth_bin: usize, elevation_bin: usize) -> f32 {
        let idx = self.index(azimuth_bin, elevation_bin);
        self.counts.get(idx).copied().unwrap_or(0.0)
    }

    fn index(&self, azimuth_bin: usize, elevation_bin: usize) -> usize {
        let a = azimuth_bin % self.azimuth_bins.max(1);
        let e = elevation_bin % self.elevation_bins.max(1);
        e * self.azimuth_bins.max(1) + a
    }
}

fn build_coverage_grids(plan: &ScanPlan) -> Vec<CoverageGrid> {
    let orientations = plan.object_orientations.max(1) as usize;
    let azimuth_bins = plan.photos_per_rotation.max(1) as usize;
    let elevation_bins = plan.camera_positions_per_orientation.max(1) as usize;
    (0..orientations)
        .map(|_| CoverageGrid::new(azimuth_bins, elevation_bins))
        .collect()
}
