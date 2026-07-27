//! Resource-aware batch processing for handling large numbers of sequences.
//!
//! This module provides intelligent batching and parallel processing that:
//! - Monitors available system resources (RAM, CPU cores)
//! - Estimates memory requirements per sequence
//! - Batches sequences to avoid OOM errors
//! - Maximizes throughput while respecting resource limits

use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use sysinfo::System;

/// System resource information
#[derive(Debug, Clone)]
pub struct SystemResources {
    /// Total RAM in bytes
    pub total_memory: u64,
    /// Available RAM in bytes
    pub available_memory: u64,
    /// Number of physical CPU cores
    pub physical_cores: usize,
    /// Number of logical CPU cores (with hyperthreading)
    pub logical_cores: usize,
}

impl SystemResources {
    /// Query current system resources
    pub fn query() -> Self {
        let mut sys = System::new();
        sys.refresh_memory();

        let total_memory = sys.total_memory();
        let available_memory = sys.available_memory();

        let physical_cores = num_cpus::get_physical();
        let logical_cores = num_cpus::get();

        Self {
            total_memory,
            available_memory,
            physical_cores,
            logical_cores,
        }
    }

    /// Get available memory in GB
    pub fn available_memory_gb(&self) -> f64 {
        self.available_memory as f64 / (1024.0 * 1024.0 * 1024.0)
    }

    /// Get total memory in GB
    pub fn total_memory_gb(&self) -> f64 {
        self.total_memory as f64 / (1024.0 * 1024.0 * 1024.0)
    }
}

/// Memory estimation for a sequence
#[derive(Debug, Clone)]
pub struct SequenceMemoryEstimate {
    /// Number of frames in sequence
    pub num_frames: usize,
    /// Width of each frame
    pub width: usize,
    /// Height of each frame
    pub height: usize,
    /// Estimated peak memory usage in bytes
    pub peak_memory_bytes: u64,
}

impl SequenceMemoryEstimate {
    /// Estimate memory usage for a sequence
    ///
    /// Memory breakdown (peak usage, not cumulative):
    /// - Preprocessed frames: H × W × 1 × N × 8 bytes (single-channel Bayer)
    /// - Frame metadata: N × ~1 KB (negligible)
    /// - Sharpness masks: H × W × N × 1 byte (bool)
    /// - Foreground mask: H × W × 1 byte
    /// - Depth map: H × W × 4 bytes (f32)
    /// - Bayer stack for collapse: H × W × N × 8 bytes (f64)
    /// - Collapsed Bayer: H × W × 8 bytes
    /// - Demosaiced RGB: H × W × 3 × 8 bytes
    /// - Super-resolution (if enabled): (H×φ) × (W×φ) × 3 × 8 bytes
    /// - Working memory (temp arrays, FFT, etc.): ~20% overhead
    ///
    /// Note: We only keep metadata (not full frames) after preprocessing
    /// This saves ~33% memory compared to keeping full frames
    pub fn estimate(num_frames: usize, width: usize, height: usize, sr_factor: f64) -> Self {
        let pixels = (width * height) as u64;

        // Preprocessed frames (single-channel Bayer)
        let preprocessed = pixels * num_frames as u64 * 8;

        // Sharpness masks
        let _sharpness_masks = pixels * num_frames as u64;

        // Foreground mask
        let _fg_mask = pixels;

        // Depth map
        let _depth_map = pixels * 4;

        // Bayer stack for collapse (H × W × N)
        let bayer_stack = pixels * num_frames as u64 * 8;

        // Collapsed Bayer
        let collapsed_bayer = pixels * 8;

        // Demosaiced RGB
        let rgb = pixels * 3 * 8;

        // Super-resolution output (if enabled)
        let sr_pixels = ((width as f64 * sr_factor) as u64) * ((height as f64 * sr_factor) as u64);
        let sr_output = if sr_factor > 1.0 {
            sr_pixels * 3 * 8
        } else {
            0
        };

        // Peak memory is when we have:
        // - Preprocessed frames
        // - Bayer stack (during collapse)
        // - Collapsed output
        // - RGB output
        // - SR output (if enabled)
        // We don't count sharpness_masks + fg_mask + depth_map as they're small
        let peak_data = preprocessed + bayer_stack + collapsed_bayer + rgb + sr_output;

        // Add 20% for working memory (temp arrays, FFT buffers, etc.)
        // Reduced from 30% due to memory optimizations
        let peak_memory_bytes = (peak_data as f64 * 1.2) as u64;

        Self {
            num_frames,
            width,
            height,
            peak_memory_bytes,
        }
    }

    /// Get peak memory in GB
    pub fn peak_memory_gb(&self) -> f64 {
        self.peak_memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
    }
}

/// Peak working-set estimate for the native grouped fusion path.
#[derive(Debug, Clone)]
pub struct NativeSequenceMemoryEstimate {
    pub input_arena_bytes: u64,
    pub fusion_output_bytes: u64,
    pub worker_scratch_bytes: u64,
    pub peak_memory_bytes: u64,
}

impl NativeSequenceMemoryEstimate {
    pub fn estimate(
        frame_count: usize,
        width: usize,
        height: usize,
        fusion_workers: usize,
        tile_size: usize,
        focus_coarse_stride: usize,
        glare_radius_pixels: usize,
        local_alignment_cell_size: usize,
        analysis_max_dimension: usize,
    ) -> Result<Self> {
        let pixels = u64::try_from(
            width
                .checked_mul(height)
                .context("Native sequence dimensions overflow")?,
        )?;
        let input_arena_bytes = pixels
            .checked_mul(frame_count as u64)
            .and_then(|value| value.checked_mul(2))
            .context("Native input arena estimate overflow")?;
        // Bayer, normalized/metric depth, confidence, radiance uncertainty,
        // source map, provenance flags, glare map, and foreground mask.
        let fusion_output_bytes = pixels
            .checked_mul(25)
            .context("Native fusion output estimate overflow")?;
        let focus_halo = focus_coarse_stride
            .max(1)
            .saturating_mul(2)
            .max(glare_radius_pixels)
            .max(3);
        let tile_edge = tile_size.saturating_add(focus_halo.saturating_mul(2)) as u64;
        // Extended plane state, glare likelihood/distance/integrals, and the
        // worst-case regional grid are 44 B/pixel.
        // Tile state retains top-two regional/detail radiance evidence plus
        // previous/adjacent responses and glare provenance: 66 B.
        let scratch_per_worker = tile_edge
            .checked_mul(tile_edge)
            .and_then(|value| value.checked_mul(44))
            .and_then(|value| {
                value.checked_add(
                    (tile_size as u64)
                        .checked_mul(tile_size as u64)?
                        .checked_mul(66)?,
                )
            })
            .context("Native fusion scratch estimate overflow")?;
        let worker_scratch_bytes = scratch_per_worker
            .checked_mul(fusion_workers.max(1) as u64)
            .context("Native worker scratch estimate overflow")?;
        let analysis_edge = analysis_max_dimension.max(32) as u64;
        // Two f64 analysis planes, two two-channel f32 gradient planes, FFT
        // workspaces, and bounded score-search scratch.
        let analysis_bytes = analysis_edge
            .checked_mul(analysis_edge)
            .and_then(|value| value.checked_mul(64))
            .context("Native alignment estimate overflow")?;
        let motion_grid_edge = analysis_max_dimension
            .max(32)
            .div_ceil(local_alignment_cell_size.max(1)) as u64;
        // Compact residual/confidence/state cells retained for every frame.
        let local_motion_bytes = motion_grid_edge
            .checked_mul(motion_grid_edge)
            .and_then(|value| value.checked_mul(frame_count as u64))
            .and_then(|value| value.checked_mul(24))
            .context("Native local-motion estimate overflow")?;
        // Sensor-surface projection retains original depth, log distance, and
        // a one-byte provenance mask.
        let visibility_working_bytes = pixels
            .checked_mul(9)
            .context("Native visibility estimate overflow")?;
        let fusion_peak = input_arena_bytes
            .checked_add(fusion_output_bytes)
            .and_then(|value| value.checked_add(worker_scratch_bytes))
            .and_then(|value| value.checked_add(analysis_bytes))
            .and_then(|value| value.checked_add(local_motion_bytes))
            .and_then(|value| value.checked_add(visibility_working_bytes))
            .context("Native fusion peak estimate overflow")?;
        // Linear/display RGB, u8 preview, all retained fusion diagnostics, and
        // PNG's temporary endian conversion for the exact u16 source map.
        let postprocess_peak = pixels
            .checked_mul(12 + 12 + 3 + 20 + 2 + 2)
            .and_then(|value| value.checked_add(12 * 1024 * 1024))
            .context("Native postprocess peak estimate overflow")?;
        let peak_memory_bytes = fusion_peak.max(postprocess_peak);
        Ok(Self {
            input_arena_bytes,
            fusion_output_bytes,
            worker_scratch_bytes,
            // Reserve 15% for allocator fragmentation and library internals.
            peak_memory_bytes: peak_memory_bytes + peak_memory_bytes / 7,
        })
    }
}

#[derive(Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

struct CreditState {
    available: u64,
}

struct CreditPoolInner {
    total: u64,
    state: Mutex<CreditState>,
    changed: Condvar,
}

/// Byte-denominated admission control for memory-intensive group work.
#[derive(Clone)]
pub struct MemoryCreditPool {
    inner: Arc<CreditPoolInner>,
}

pub struct MemoryCreditPermit {
    pool: Arc<CreditPoolInner>,
    bytes: u64,
}

impl MemoryCreditPool {
    pub fn new(total_bytes: u64) -> Result<Self> {
        if total_bytes == 0 {
            anyhow::bail!("Memory credit budget must be greater than zero");
        }
        Ok(Self {
            inner: Arc::new(CreditPoolInner {
                total: total_bytes,
                state: Mutex::new(CreditState {
                    available: total_bytes,
                }),
                changed: Condvar::new(),
            }),
        })
    }

    pub fn total_bytes(&self) -> u64 {
        self.inner.total
    }

    pub fn available_bytes(&self) -> u64 {
        self.inner
            .state
            .lock()
            .map(|state| state.available)
            .unwrap_or(0)
    }

    pub fn acquire(
        &self,
        bytes: u64,
        cancellation: &CancellationToken,
    ) -> Result<MemoryCreditPermit> {
        if bytes > self.inner.total {
            anyhow::bail!(
                "Group requires {:.1} MiB but memory budget is {:.1} MiB",
                bytes as f64 / (1024.0 * 1024.0),
                self.inner.total as f64 / (1024.0 * 1024.0)
            );
        }
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("Memory credit lock poisoned"))?;
        while state.available < bytes {
            if cancellation.is_cancelled() {
                anyhow::bail!("Processing cancelled while waiting for memory");
            }
            let (next, _) = self
                .inner
                .changed
                .wait_timeout(state, Duration::from_millis(100))
                .map_err(|_| anyhow::anyhow!("Memory credit lock poisoned"))?;
            state = next;
        }
        state.available -= bytes;
        Ok(MemoryCreditPermit {
            pool: self.inner.clone(),
            bytes,
        })
    }
}

impl Drop for MemoryCreditPermit {
    fn drop(&mut self) {
        if let Ok(mut state) = self.pool.state.lock() {
            state.available = state
                .available
                .saturating_add(self.bytes)
                .min(self.pool.total);
            self.pool.changed.notify_all();
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PipelinePressureSample {
    pub decoded_megapixels: f64,
    pub decode_seconds: f64,
    pub writer_wait_seconds: f64,
    pub available_memory_ratio: f64,
    pub major_page_faults: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerAdjustment {
    pub previous_workers: usize,
    pub workers: usize,
    pub changed: bool,
}

/// Conservative closed-loop tuning for the bounded NEF decoder pool.
///
/// Writer wait is the portable storage-queue signal: with a one-deep writer,
/// waiting to retire the previous export means destination throughput is
/// saturated regardless of the underlying filesystem or device.
pub struct AdaptiveDecodeController {
    workers: usize,
    min_workers: usize,
    max_workers: usize,
    stable_samples: usize,
    throughput_ewma: Option<f64>,
}

impl AdaptiveDecodeController {
    pub fn new(initial_workers: usize, max_workers: usize) -> Self {
        let max_workers = max_workers.max(1);
        Self {
            workers: initial_workers.clamp(1, max_workers),
            min_workers: 1,
            max_workers,
            stable_samples: 0,
            throughput_ewma: None,
        }
    }

    pub fn workers(&self) -> usize {
        self.workers
    }

    pub fn observe(&mut self, sample: PipelinePressureSample) -> WorkerAdjustment {
        let previous_workers = self.workers;
        let decode_seconds = sample.decode_seconds.max(1e-6);
        let throughput = sample.decoded_megapixels.max(0.0) / decode_seconds;
        let prior_throughput = self.throughput_ewma;
        self.throughput_ewma = Some(match prior_throughput {
            Some(prior) => prior * 0.75 + throughput * 0.25,
            None => throughput,
        });

        let writer_saturated =
            sample.writer_wait_seconds > (sample.decode_seconds * 0.35).max(0.050);
        let memory_pressure =
            sample.available_memory_ratio.is_finite() && sample.available_memory_ratio < 0.15;
        let paging_pressure = sample.major_page_faults >= 32;
        let throughput_regressed = prior_throughput
            .is_some_and(|prior| throughput < prior * 0.82 && sample.decode_seconds > 0.050);

        if writer_saturated || memory_pressure || paging_pressure || throughput_regressed {
            let reduction = (self.workers / 4).max(1);
            self.workers = self.workers.saturating_sub(reduction).max(self.min_workers);
            self.stable_samples = 0;
        } else {
            self.stable_samples += 1;
            if self.stable_samples >= 4 && self.workers < self.max_workers {
                self.workers += 1;
                self.stable_samples = 0;
            }
        }

        WorkerAdjustment {
            previous_workers,
            workers: self.workers,
            changed: self.workers != previous_workers,
        }
    }
}

/// Batch processing configuration
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// Maximum number of sequences to process in parallel
    pub max_parallel_sequences: usize,
    /// Memory safety margin (fraction of available memory to reserve)
    pub memory_safety_margin: f64,
    /// Whether to enable GPU acceleration (future)
    pub enable_gpu: bool,
}

impl BatchConfig {
    /// Create optimal batch configuration based on system resources
    pub fn auto_configure(resources: &SystemResources, avg_sequence_memory_gb: f64) -> Self {
        // Reserve 20% of memory for OS and other processes
        let memory_safety_margin = 0.20;
        let usable_memory_gb = resources.available_memory_gb() * (1.0 - memory_safety_margin);

        // Calculate how many sequences can fit in memory
        let max_by_memory = if avg_sequence_memory_gb > 0.0 {
            (usable_memory_gb / avg_sequence_memory_gb).floor() as usize
        } else {
            resources.physical_cores
        };

        // Limit by CPU cores (no point running more sequences than cores)
        // Use physical cores, not logical (hyperthreading doesn't help much for this workload)
        let max_by_cpu = resources.physical_cores;

        // Take the minimum of memory and CPU constraints
        let max_parallel_sequences = max_by_memory.min(max_by_cpu).max(1);

        tracing::info!(
            "Auto-configured batch processing: {} parallel sequences (limited by {})",
            max_parallel_sequences,
            if max_by_memory < max_by_cpu {
                "memory"
            } else {
                "CPU"
            }
        );
        tracing::info!(
            "  Available memory: {:.1} GB, per-sequence estimate: {:.1} GB",
            usable_memory_gb,
            avg_sequence_memory_gb
        );

        Self {
            max_parallel_sequences,
            memory_safety_margin,
            enable_gpu: detect_gpu_capability(),
        }
    }

    /// Create configuration with manual settings
    pub fn manual(max_parallel_sequences: usize) -> Self {
        Self {
            max_parallel_sequences,
            memory_safety_margin: 0.20,
            enable_gpu: detect_gpu_capability(),
        }
    }
}

/// Detect GPU capability using WGPU adapter enumeration
///
/// Returns true if a suitable GPU adapter is available for compute operations.
/// This checks for discrete GPUs first, then integrated GPUs.
pub fn detect_gpu_capability() -> bool {
    // Use pollster to block on async GPU detection
    // This is acceptable during initialization
    #[cfg(feature = "gpu")]
    {
        if gpu_disabled() {
            tracing::info!("GPU disabled via TRUESHOT_DISABLE_GPU");
            return false;
        }
        use std::sync::OnceLock;

        // Cache the result to avoid repeated detection
        static GPU_AVAILABLE: OnceLock<bool> = OnceLock::new();

        *GPU_AVAILABLE.get_or_init(|| match pollster::block_on(detect_gpu_async()) {
            Ok(available) => {
                if available {
                    tracing::info!("GPU compute capability detected");
                } else {
                    tracing::info!("No GPU compute capability available");
                }
                available
            }
            Err(e) => {
                tracing::warn!("GPU detection failed: {}", e);
                false
            }
        })
    }

    #[cfg(not(feature = "gpu"))]
    {
        tracing::debug!("GPU feature not enabled at compile time");
        false
    }
}

/// Async GPU detection using WGPU
#[cfg(feature = "gpu")]
async fn detect_gpu_async() -> Result<bool, Box<dyn std::error::Error>> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });

    // Try to get a high-performance adapter first
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await;

    if let Some(adapter) = adapter {
        let info = adapter.get_info();
        tracing::info!(
            "Found GPU: {} ({:?}, {:?})",
            info.name,
            info.device_type,
            info.backend
        );

        // Check if it supports compute shaders
        let features = adapter.features();
        let has_compute = true; // Basic compute is always available in WGPU

        // Prefer discrete GPUs
        let is_suitable = matches!(
            info.device_type,
            wgpu::DeviceType::DiscreteGpu | wgpu::DeviceType::IntegratedGpu
        );

        Ok(is_suitable && has_compute)
    } else {
        Ok(false)
    }
}

/// GPU information for diagnostics
#[derive(Debug, Clone)]
pub struct GpuInfo {
    /// GPU name
    pub name: String,
    /// Device type (Discrete, Integrated, etc.)
    pub device_type: String,
    /// Backend (Vulkan, Metal, DX12, etc.)
    pub backend: String,
    /// Available VRAM in bytes (if known)
    pub vram_bytes: Option<u64>,
}

/// Get detailed GPU information
#[cfg(feature = "gpu")]
pub fn get_gpu_info() -> Option<GpuInfo> {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await?;

        let info = adapter.get_info();

        Some(GpuInfo {
            name: info.name.clone(),
            device_type: format!("{:?}", info.device_type),
            backend: format!("{:?}", info.backend),
            vram_bytes: None, // WGPU doesn't expose VRAM directly
        })
    })
}

#[cfg(not(feature = "gpu"))]
pub fn get_gpu_info() -> Option<GpuInfo> {
    None
}

fn gpu_disabled() -> bool {
    match std::env::var("TRUESHOT_DISABLE_GPU") {
        Ok(val) => matches!(val.to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

/// Progress tracking for batch processing
#[derive(Debug, Clone)]
pub struct BatchProgress {
    /// Total number of sequences
    pub total_sequences: usize,
    /// Number of completed sequences
    pub completed: Arc<AtomicUsize>,
    /// Number of failed sequences
    pub failed: Arc<AtomicUsize>,
}

impl BatchProgress {
    /// Create new progress tracker
    pub fn new(total_sequences: usize) -> Self {
        Self {
            total_sequences,
            completed: Arc::new(AtomicUsize::new(0)),
            failed: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Mark a sequence as completed
    pub fn mark_completed(&self) {
        self.completed.fetch_add(1, Ordering::SeqCst);
    }

    /// Mark a sequence as failed
    pub fn mark_failed(&self) {
        self.failed.fetch_add(1, Ordering::SeqCst);
    }

    /// Get current progress (0.0 to 1.0)
    pub fn progress(&self) -> f64 {
        let completed = self.completed.load(Ordering::SeqCst);
        let failed = self.failed.load(Ordering::SeqCst);
        (completed + failed) as f64 / self.total_sequences as f64
    }

    /// Get number of completed sequences
    pub fn get_completed(&self) -> usize {
        self.completed.load(Ordering::SeqCst)
    }

    /// Get number of failed sequences
    pub fn get_failed(&self) -> usize {
        self.failed.load(Ordering::SeqCst)
    }

    /// Get number of remaining sequences
    pub fn get_remaining(&self) -> usize {
        let completed = self.completed.load(Ordering::SeqCst);
        let failed = self.failed.load(Ordering::SeqCst);
        self.total_sequences.saturating_sub(completed + failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_estimation() {
        // Test with typical bone scan: 60 frames, 8256x5504
        let estimate = SequenceMemoryEstimate::estimate(60, 8256, 5504, 1.0);

        // Legacy f64 processing is intentionally expensive at full Z9 size.
        assert!(estimate.peak_memory_gb() > 20.0);
        assert!(estimate.peak_memory_gb() < 64.0);

        println!(
            "Estimated memory for 60 frames (8256x5504): {:.1} GB",
            estimate.peak_memory_gb()
        );
    }

    #[test]
    fn test_batch_config() {
        let resources = SystemResources::query();
        let config = BatchConfig::auto_configure(&resources, 30.0);

        // Should be at least 1, at most number of cores
        assert!(config.max_parallel_sequences >= 1);
        assert!(config.max_parallel_sequences <= resources.physical_cores);

        println!(
            "Auto-configured: {} parallel sequences",
            config.max_parallel_sequences
        );
        println!(
            "System: {} cores, {:.1} GB RAM",
            resources.physical_cores,
            resources.total_memory_gb()
        );
    }

    #[test]
    fn native_estimate_matches_bounded_group_architecture() {
        let estimate =
            NativeSequenceMemoryEstimate::estimate(21, 1310, 1304, 8, 256, 4, 20, 24, 512).unwrap();
        let peak_mib = estimate.peak_memory_bytes as f64 / (1024.0 * 1024.0);
        assert!(
            (220.0..245.0).contains(&peak_mib),
            "peak was {peak_mib:.1} MiB"
        );
        assert_eq!(estimate.input_arena_bytes, 21 * 1310 * 1304 * 2);
    }

    #[test]
    fn memory_credits_return_on_drop() {
        let pool = MemoryCreditPool::new(1024).unwrap();
        let cancellation = CancellationToken::default();
        {
            let _permit = pool.acquire(768, &cancellation).unwrap();
            assert_eq!(pool.available_bytes(), 256);
        }
        assert_eq!(pool.available_bytes(), 1024);
        assert!(pool.acquire(2048, &cancellation).is_err());
    }

    #[test]
    fn adaptive_workers_back_off_under_pressure_and_grow_when_stable() {
        let mut controller = AdaptiveDecodeController::new(8, 12);
        let pressure = PipelinePressureSample {
            decoded_megapixels: 40.0,
            decode_seconds: 1.0,
            writer_wait_seconds: 0.5,
            available_memory_ratio: 0.5,
            major_page_faults: 0,
        };
        assert_eq!(controller.observe(pressure).workers, 6);

        let stable = PipelinePressureSample {
            writer_wait_seconds: 0.0,
            ..pressure
        };
        for _ in 0..3 {
            assert!(!controller.observe(stable).changed);
        }
        assert_eq!(controller.observe(stable).workers, 7);

        let paging = PipelinePressureSample {
            major_page_faults: 64,
            ..stable
        };
        assert_eq!(controller.observe(paging).workers, 6);
    }
}
