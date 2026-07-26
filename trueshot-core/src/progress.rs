use serde::{Deserialize, Serialize};
use std::collections::HashMap;
/// Progress tracking and runtime prediction system
///
/// This module provides centralized progress tracking that can be used by both
/// GUI and CLI interfaces, along with runtime prediction based on system resources.
///
/// Also provides `CancellationToken` for cancelling long-running operations.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sysinfo::{DiskKind, Disks, MemoryRefreshKind, RefreshKind, System};

// ============================================================================
// Cancellation Token
// ============================================================================

/// A token for cancelling long-running operations
///
/// # Example
/// ```ignore
/// let token = CancellationToken::new();
///
/// // Start long operation in background
/// let token_clone = token.clone();
/// std::thread::spawn(move || {
///     for i in 0..1000 {
///         if token_clone.is_cancelled() {
///             return; // Early exit
///         }
///         // Do work...
///     }
/// });
///
/// // Cancel from main thread
/// token.cancel();
/// ```
#[derive(Clone, Debug)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Create a new cancellation token
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Cancel the operation
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Check if cancellation has been requested
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Reset the token (for reuse)
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
    }

    /// Create a child token that shares cancellation state
    pub fn child(&self) -> CancellationToken {
        Self {
            cancelled: Arc::clone(&self.cancelled),
        }
    }

    /// Check cancellation and return error if cancelled
    pub fn check(&self) -> Result<(), crate::error::TrueShotError> {
        if self.is_cancelled() {
            Err(crate::error::TrueShotError::Cancelled)
        } else {
            Ok(())
        }
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Current processing phase
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProcessingPhase {
    Initialization,
    ImageAnalysis,
    BurstCollapse,
    GlobalAlignment,
    SelectiveLoading,
    BackgroundRemoval,
    DepthMapping,
    QifFusion,
    PostProcessing,
    Export,
    Complete,
}

impl ProcessingPhase {
    pub fn name(&self) -> &'static str {
        match self {
            ProcessingPhase::Initialization => "Initialization",
            ProcessingPhase::ImageAnalysis => "Image Analysis & Grouping",
            ProcessingPhase::BurstCollapse => "Burst Collapse",
            ProcessingPhase::GlobalAlignment => "Global Alignment & Warp Calculation",
            ProcessingPhase::SelectiveLoading => "Selective Loading & Alignment",
            ProcessingPhase::BackgroundRemoval => "Background Removal",
            ProcessingPhase::DepthMapping => "Depth Map Generation",
            ProcessingPhase::QifFusion => "QIF Fusion",
            ProcessingPhase::PostProcessing => "Post-Processing",
            ProcessingPhase::Export => "Export",
            ProcessingPhase::Complete => "Complete",
        }
    }

    pub fn phase_number(&self) -> u8 {
        match self {
            ProcessingPhase::Initialization => 0,
            ProcessingPhase::ImageAnalysis => 1,
            ProcessingPhase::BurstCollapse => 2,
            ProcessingPhase::GlobalAlignment => 3,
            ProcessingPhase::SelectiveLoading => 4,
            ProcessingPhase::BackgroundRemoval => 5,
            ProcessingPhase::DepthMapping => 6,
            ProcessingPhase::QifFusion => 7,
            ProcessingPhase::PostProcessing => 8,
            ProcessingPhase::Export => 9,
            ProcessingPhase::Complete => 10,
        }
    }

    pub fn total_phases() -> u8 {
        10
    }
}

/// Progress information for a specific phase
#[derive(Debug, Clone)]
pub struct PhaseProgress {
    pub phase: ProcessingPhase,
    pub current: u64,
    pub total: u64,
    pub message: String,
    pub start_time: Instant,
    pub estimated_remaining: Option<Duration>,
}

impl PhaseProgress {
    pub fn new(phase: ProcessingPhase, total: u64, message: String) -> Self {
        Self {
            phase,
            current: 0,
            total,
            message,
            start_time: Instant::now(),
            estimated_remaining: None,
        }
    }

    pub fn progress_percent(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.current as f32 / self.total as f32) * 100.0
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }
}

/// System resource information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemResources {
    pub cpu_cores: u32,
    pub memory_gb: f32,
    pub available_memory_gb: f32,
    pub gpu_available: bool,
    pub storage_type: StorageType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageType {
    Ssd,
    Hdd,
    Unknown,
}

/// Runtime statistics for a completed phase
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseStats {
    pub phase: ProcessingPhase,
    pub duration: Duration,
    pub items_processed: u64,
    pub memory_peak_mb: f32,
    pub cpu_usage_percent: f32,
    pub system_resources: SystemResources,
}

/// Runtime prediction engine
#[derive(Debug, Clone)]
pub struct RuntimePredictor {
    historical_stats: HashMap<ProcessingPhase, Vec<PhaseStats>>,
    current_resources: SystemResources,
}

impl Default for RuntimePredictor {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimePredictor {
    pub fn new() -> Self {
        Self {
            historical_stats: HashMap::new(),
            current_resources: Self::detect_system_resources(),
        }
    }

    pub fn add_phase_stats(&mut self, stats: PhaseStats) {
        self.historical_stats
            .entry(stats.phase.clone())
            .or_default()
            .push(stats);
    }

    pub fn predict_phase_duration(&self, phase: &ProcessingPhase, items: u64) -> Option<Duration> {
        let stats = self.historical_stats.get(phase)?;
        if stats.is_empty() {
            return None;
        }

        // Find stats with similar system resources
        let similar_stats: Vec<_> = stats
            .iter()
            .filter(|s| self.is_similar_system(&s.system_resources))
            .collect();

        if similar_stats.is_empty() {
            return None;
        }

        // Calculate average time per item
        let total_time: Duration = similar_stats.iter().map(|s| s.duration).sum();
        let total_items: u64 = similar_stats.iter().map(|s| s.items_processed).sum();

        if total_items == 0 {
            return None;
        }

        let avg_time_per_item = total_time / total_items as u32;
        Some(avg_time_per_item * items as u32)
    }

    fn is_similar_system(&self, other: &SystemResources) -> bool {
        // Consider systems similar if they have similar specs
        let cpu_diff = (self.current_resources.cpu_cores as f32 - other.cpu_cores as f32).abs();
        let memory_diff = (self.current_resources.memory_gb - other.memory_gb).abs();

        cpu_diff <= 2.0 && memory_diff <= 4.0
    }

    fn detect_system_resources() -> SystemResources {
        let mut sys =
            System::new_with_specifics(RefreshKind::new().with_memory(MemoryRefreshKind::new()));
        sys.refresh_memory();

        let total_memory_kb = sys.total_memory() as f32;
        let available_memory_kb = sys.available_memory() as f32;

        let memory_gb = total_memory_kb / (1024.0 * 1024.0);
        let available_memory_gb = available_memory_kb / (1024.0 * 1024.0);

        let storage_type = detect_storage_type();

        let gpu_available = crate::resource_manager::detect_gpu_capability();

        SystemResources {
            cpu_cores: num_cpus::get() as u32,
            memory_gb,
            available_memory_gb,
            gpu_available,
            storage_type,
        }
    }
}

fn detect_storage_type() -> StorageType {
    let disks = Disks::new_with_refreshed_list();
    let cwd = std::env::current_dir().ok();
    let mut best_match: Option<(usize, DiskKind)> = None;

    for disk in disks.list() {
        let mount = disk.mount_point();
        let mount_str = mount.to_string_lossy();
        if let Some(dir) = cwd.as_ref() {
            if dir.starts_with(mount) {
                let len = mount_str.len();
                if best_match
                    .as_ref()
                    .map(|(best_len, _)| len > *best_len)
                    .unwrap_or(true)
                {
                    best_match = Some((len, disk.kind()));
                }
            }
        }
    }

    let kind = match best_match {
        Some((_, kind)) => kind,
        None => disks
            .list()
            .first()
            .map(|d| d.kind())
            .unwrap_or(DiskKind::Unknown(0)),
    };

    match kind {
        DiskKind::SSD => StorageType::Ssd,
        DiskKind::HDD => StorageType::Hdd,
        DiskKind::Unknown(_) => StorageType::Unknown,
    }
}

/// Type alias for progress callback function
type ProgressCallback = Box<dyn Fn(&PhaseProgress) + Send + Sync>;

/// Global progress tracker
pub struct ProgressTracker {
    current_phase: Arc<Mutex<Option<PhaseProgress>>>,
    predictor: Arc<Mutex<RuntimePredictor>>,
    callbacks: Arc<Mutex<Vec<ProgressCallback>>>,
}

impl ProgressTracker {
    pub fn new() -> Self {
        Self {
            current_phase: Arc::new(Mutex::new(None)),
            predictor: Arc::new(Mutex::new(RuntimePredictor::new())),
            callbacks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn start_phase(&self, phase: ProcessingPhase, total: u64, message: String) {
        let progress = PhaseProgress::new(phase, total, message);

        // Update current phase
        {
            let mut current = self.current_phase.lock().unwrap();
            *current = Some(progress.clone());
        }

        // Notify callbacks
        self.notify_callbacks(&progress);

        tracing::info!(
            "=== PHASE {}: {} ===",
            progress.phase.phase_number(),
            progress.phase.name()
        );
        tracing::info!("{}", progress.message);
    }

    pub fn update_progress(&self, current: u64, message: Option<String>) {
        let mut should_notify = false;
        let mut progress_clone = None;

        {
            let mut current_phase = self.current_phase.lock().unwrap();
            if let Some(ref mut progress) = *current_phase {
                progress.current = current;
                if let Some(msg) = message {
                    progress.message = msg;
                }

                // Update estimated remaining time
                if progress.current > 0 && progress.total > progress.current {
                    let elapsed = progress.elapsed();
                    let rate = progress.current as f32 / elapsed.as_secs_f32();
                    let remaining_items = progress.total - progress.current;
                    let estimated_seconds = remaining_items as f32 / rate;
                    progress.estimated_remaining = Some(Duration::from_secs_f32(estimated_seconds));
                }

                should_notify = true;
                progress_clone = Some(progress.clone());
            }
        }

        if should_notify {
            if let Some(progress) = progress_clone {
                self.notify_callbacks(&progress);
            }
        }
    }

    pub fn complete_phase(&self) {
        let stats = {
            let mut current_phase = self.current_phase.lock().unwrap();
            if let Some(progress) = current_phase.take() {
                let stats = PhaseStats {
                    phase: progress.phase.clone(),
                    duration: progress.elapsed(),
                    items_processed: progress.total,
                    memory_peak_mb: 0.0,    // Would be tracked during processing
                    cpu_usage_percent: 0.0, // Would be tracked during processing
                    system_resources: RuntimePredictor::detect_system_resources(),
                };

                tracing::info!(
                    "Phase {} completed in {:.2}s",
                    progress.phase.name(),
                    stats.duration.as_secs_f32()
                );

                Some(stats)
            } else {
                None
            }
        };

        if let Some(stats) = stats {
            let mut predictor = self.predictor.lock().unwrap();
            predictor.add_phase_stats(stats);
        }
    }

    pub fn get_current_progress(&self) -> Option<PhaseProgress> {
        self.current_phase.lock().unwrap().clone()
    }

    pub fn add_callback<F>(&self, callback: F)
    where
        F: Fn(&PhaseProgress) + Send + Sync + 'static,
    {
        let mut callbacks = self.callbacks.lock().unwrap();
        callbacks.push(Box::new(callback));
    }

    fn notify_callbacks(&self, progress: &PhaseProgress) {
        let callbacks = self.callbacks.lock().unwrap();
        for callback in callbacks.iter() {
            callback(progress);
        }
    }

    pub fn predict_remaining_time(&self, phase: &ProcessingPhase, items: u64) -> Option<Duration> {
        let predictor = self.predictor.lock().unwrap();
        predictor.predict_phase_duration(phase, items)
    }
}

impl Default for ProgressTracker {
    fn default() -> Self {
        Self::new()
    }
}

lazy_static::lazy_static! {
    pub static ref PROGRESS_TRACKER: ProgressTracker = ProgressTracker::new();
}

/// Convenience functions for progress tracking
pub fn start_phase(phase: ProcessingPhase, total: u64, message: String) {
    PROGRESS_TRACKER.start_phase(phase, total, message);
}

pub fn update_progress(current: u64, message: Option<String>) {
    PROGRESS_TRACKER.update_progress(current, message);
}

pub fn complete_phase() {
    PROGRESS_TRACKER.complete_phase();
}

pub fn get_current_progress() -> Option<PhaseProgress> {
    PROGRESS_TRACKER.get_current_progress()
}

pub fn add_progress_callback<F>(callback: F)
where
    F: Fn(&PhaseProgress) + Send + Sync + 'static,
{
    PROGRESS_TRACKER.add_callback(callback);
}

// ============================================================================
// Operation Estimation (Phase 3: UX Polish)
// ============================================================================

/// Estimated resource requirements for an operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationPreview {
    /// Human-readable operation name
    pub name: String,
    /// Estimated duration in seconds
    pub estimated_seconds: f64,
    /// Estimated memory usage in MB
    pub estimated_memory_mb: f64,
    /// Estimated disk space required in MB
    pub estimated_disk_mb: f64,
    /// Number of items to process
    pub item_count: usize,
    /// Whether GPU will be used
    pub uses_gpu: bool,
    /// Phases that will be executed
    pub phases: Vec<String>,
}

impl OperationPreview {
    /// Format duration as human-readable string
    pub fn format_duration(&self) -> String {
        let seconds = self.estimated_seconds;
        if seconds < 60.0 {
            format!("{:.0} seconds", seconds)
        } else if seconds < 3600.0 {
            let minutes = seconds / 60.0;
            format!("{:.1} minutes", minutes)
        } else {
            let hours = seconds / 3600.0;
            format!("{:.1} hours", hours)
        }
    }

    /// Format memory as human-readable string
    pub fn format_memory(&self) -> String {
        if self.estimated_memory_mb < 1024.0 {
            format!("{:.0} MB", self.estimated_memory_mb)
        } else {
            format!("{:.1} GB", self.estimated_memory_mb / 1024.0)
        }
    }

    /// Format disk space as human-readable string
    pub fn format_disk(&self) -> String {
        if self.estimated_disk_mb < 1024.0 {
            format!("{:.0} MB", self.estimated_disk_mb)
        } else {
            format!("{:.1} GB", self.estimated_disk_mb / 1024.0)
        }
    }

    /// Get a summary suitable for display
    pub fn summary(&self) -> String {
        format!(
            "{}: ~{}, {} RAM, {} disk, {} items",
            self.name,
            self.format_duration(),
            self.format_memory(),
            self.format_disk(),
            self.item_count
        )
    }
}

/// Trait for operations that can provide upfront estimates
pub trait ProgressAware {
    /// Get an estimate of what this operation will do before running it
    fn estimate(&self) -> OperationPreview;

    /// Get a cancellation token for this operation
    fn cancellation_token(&self) -> Option<CancellationToken> {
        None
    }
}

/// Estimator for common TrueShot operations
pub struct OperationEstimator;

impl OperationEstimator {
    /// Estimate time for burst collapse processing
    pub fn estimate_burst_collapse(num_frames: usize, resolution: (u32, u32)) -> OperationPreview {
        let pixels = resolution.0 as f64 * resolution.1 as f64;
        let megapixels = pixels / 1_000_000.0;

        // Empirical: ~0.5s per frame per megapixel on modern CPU
        let seconds_per_frame = megapixels * 0.5;
        let total_seconds = seconds_per_frame * num_frames as f64;

        // Memory: ~16 bytes per pixel per frame in memory
        let memory_mb = (pixels * num_frames as f64 * 16.0) / (1024.0 * 1024.0);

        OperationPreview {
            name: "Burst Collapse".to_string(),
            estimated_seconds: total_seconds,
            estimated_memory_mb: memory_mb.min(16384.0), // Cap at 16GB estimate
            estimated_disk_mb: megapixels * 3.0,         // Output size
            item_count: num_frames,
            uses_gpu: false,
            phases: vec![
                "Preprocessing".to_string(),
                "Alignment".to_string(),
                "Hierarchical Collapse".to_string(),
                "Post-processing".to_string(),
            ],
        }
    }

    /// Estimate time for 3D Gaussian Splatting training
    pub fn estimate_gaussian_splatting(
        num_images: usize,
        target_gaussians: usize,
    ) -> OperationPreview {
        // Empirical: ~30s per 1000 images + ~0.001s per gaussian per iteration
        let base_time = num_images as f64 * 0.03;
        let training_time = target_gaussians as f64 * 0.001 * 30000.0; // 30k iterations
        let total_seconds = base_time + training_time;

        // Memory: ~24 bytes per gaussian
        let memory_mb = (target_gaussians as f64 * 24.0) / (1024.0 * 1024.0) + 2048.0; // + base GPU memory

        OperationPreview {
            name: "3D Gaussian Splatting".to_string(),
            estimated_seconds: total_seconds,
            estimated_memory_mb: memory_mb,
            estimated_disk_mb: (target_gaussians as f64 * 48.0) / (1024.0 * 1024.0), // PLY output
            item_count: num_images,
            uses_gpu: true,
            phases: vec![
                "SfM Initialization".to_string(),
                "Point Cloud Generation".to_string(),
                "Gaussian Initialization".to_string(),
                "Training (30k iterations)".to_string(),
                "Export".to_string(),
            ],
        }
    }

    /// Estimate time for mesh generation from gaussians
    pub fn estimate_mesh_extraction(
        num_gaussians: usize,
        grid_resolution: usize,
    ) -> OperationPreview {
        // Empirical: marching cubes is O(n^3) for grid size
        let voxels = (grid_resolution * grid_resolution * grid_resolution) as f64;
        let seconds = voxels * 0.0000001 + num_gaussians as f64 * 0.00001;

        let memory_mb = (voxels * 4.0) / (1024.0 * 1024.0) + 512.0;

        OperationPreview {
            name: "Mesh Extraction".to_string(),
            estimated_seconds: seconds,
            estimated_memory_mb: memory_mb,
            estimated_disk_mb: (grid_resolution * grid_resolution) as f64 * 0.1, // Estimated mesh size
            item_count: num_gaussians,
            uses_gpu: false,
            phases: vec![
                "Voxel Grid Generation".to_string(),
                "Marching Cubes".to_string(),
                "Mesh Optimization".to_string(),
                "Texture Mapping".to_string(),
            ],
        }
    }

    /// Estimate time for avatar capture
    pub fn estimate_avatar_capture(num_frames: usize, resolution: (u32, u32)) -> OperationPreview {
        let megapixels = (resolution.0 * resolution.1) as f64 / 1_000_000.0;

        // Empirical: ~0.1s per frame for pose estimation + landmark detection
        let per_frame = 0.1 + megapixels * 0.05;
        let total_seconds = per_frame * num_frames as f64 + 30.0; // + rigging time

        OperationPreview {
            name: "Avatar Capture".to_string(),
            estimated_seconds: total_seconds,
            estimated_memory_mb: 4096.0, // Neural networks
            estimated_disk_mb: 50.0,     // VRM file
            item_count: num_frames,
            uses_gpu: true,
            phases: vec![
                "Pose Estimation".to_string(),
                "Facial Landmark Detection".to_string(),
                "Skeleton Fitting".to_string(),
                "Mesh Binding".to_string(),
                "Blendshape Generation".to_string(),
            ],
        }
    }

    /// Estimate time for photogrammetry
    pub fn estimate_photogrammetry(num_images: usize, quality: &str) -> OperationPreview {
        let quality_multiplier = match quality {
            "low" => 0.5,
            "medium" => 1.0,
            "high" => 2.0,
            "ultra" => 4.0,
            _ => 1.0,
        };

        // Empirical: ~2s per image for feature extraction + O(n^2) matching
        let feature_time = num_images as f64 * 2.0 * quality_multiplier;
        let matching_time = (num_images * num_images) as f64 * 0.01 * quality_multiplier;
        let sfm_time = num_images as f64 * 5.0;
        let mvs_time = num_images as f64 * 30.0 * quality_multiplier;

        let total_seconds = feature_time + matching_time + sfm_time + mvs_time;

        OperationPreview {
            name: format!("Photogrammetry ({})", quality),
            estimated_seconds: total_seconds,
            estimated_memory_mb: 8192.0 * quality_multiplier,
            estimated_disk_mb: num_images as f64 * 10.0 * quality_multiplier,
            item_count: num_images,
            uses_gpu: true,
            phases: vec![
                "Feature Extraction".to_string(),
                "Feature Matching".to_string(),
                "Structure from Motion".to_string(),
                "Multi-View Stereo".to_string(),
                "Mesh Generation".to_string(),
                "Texture Mapping".to_string(),
            ],
        }
    }
}

#[cfg(test)]
mod estimation_tests {
    use super::*;

    #[test]
    fn test_operation_preview_formatting() {
        let preview = OperationEstimator::estimate_burst_collapse(10, (4000, 3000));

        assert!(!preview.format_duration().is_empty());
        assert!(!preview.format_memory().is_empty());
        assert!(!preview.summary().is_empty());
    }

    #[test]
    fn test_gaussian_splatting_estimate() {
        let preview = OperationEstimator::estimate_gaussian_splatting(100, 100_000);

        assert!(preview.estimated_seconds > 0.0);
        assert!(preview.uses_gpu);
        assert_eq!(preview.item_count, 100);
    }
}
