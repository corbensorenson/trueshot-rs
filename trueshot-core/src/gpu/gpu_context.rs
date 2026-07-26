//! GPU context management for wgpu
//!
//! Handles GPU device initialization, resource management, and fallback logic.

use anyhow::{Context, Result};
use std::env;
use std::sync::Arc;

/// GPU context for compute operations
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_info: wgpu::AdapterInfo,
}

impl GpuContext {
    /// Initialize GPU context
    ///
    /// Returns None if no suitable GPU is found
    pub async fn new() -> Result<Option<Self>> {
        if gpu_disabled() {
            tracing::info!("GPU disabled via TRUESHOT_DISABLE_GPU");
            return Ok(None);
        }
        // Create wgpu instance
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        // Request adapter (GPU)
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await;

        let adapter = match adapter {
            Some(a) => a,
            None => {
                tracing::warn!("No GPU adapter found - GPU acceleration unavailable");
                return Ok(None);
            }
        };

        let adapter_info = adapter.get_info();
        tracing::info!(
            "Found GPU: {} ({:?})",
            adapter_info.name,
            adapter_info.backend
        );

        // Get adapter limits to see what's supported
        let adapter_limits = adapter.limits();
        tracing::info!(
            "Adapter max_storage_buffer_binding_size: {} MB",
            adapter_limits.max_storage_buffer_binding_size / (1024 * 1024)
        );

        // Request higher limits for large image processing
        // Start with defaults and increase storage buffer binding size
        let mut limits = wgpu::Limits::default();

        // Request the maximum storage buffer binding size the adapter supports
        // This allows us to process larger images on GPU
        limits.max_storage_buffer_binding_size = adapter_limits.max_storage_buffer_binding_size;

        tracing::info!(
            "Requesting max_storage_buffer_binding_size: {} MB",
            limits.max_storage_buffer_binding_size / (1024 * 1024)
        );

        // Request device and queue
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("pixelcollapse2_gpu"),
                    required_features: wgpu::Features::empty(),
                    required_limits: limits,
                },
                None,
            )
            .await
            .context("Failed to create GPU device")?;

        Ok(Some(Self {
            device,
            queue,
            adapter_info,
        }))
    }

    /// Initialize GPU context (blocking)
    pub fn new_blocking() -> Result<Option<Self>> {
        pollster::block_on(Self::new())
    }

    /// Get GPU memory info
    pub fn memory_info(&self) -> String {
        format!(
            "{} ({:?})",
            self.adapter_info.name, self.adapter_info.backend
        )
    }
}

/// Check if GPU is available (blocking)
pub fn is_gpu_available() -> bool {
    if gpu_disabled() {
        return false;
    }
    match GpuContext::new_blocking() {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(e) => {
            tracing::warn!("GPU check failed: {}", e);
            false
        }
    }
}

/// Global GPU context (lazy initialization)
static GPU_CONTEXT: std::sync::OnceLock<Option<Arc<GpuContext>>> = std::sync::OnceLock::new();

/// Get or initialize global GPU context
pub fn get_gpu_context() -> Option<Arc<GpuContext>> {
    if gpu_disabled() {
        return None;
    }
    GPU_CONTEXT
        .get_or_init(|| {
            match GpuContext::new_blocking() {
                Ok(Some(ctx)) => {
                    tracing::info!("GPU context initialized: {}", ctx.memory_info());
                    Some(Arc::new(ctx))
                }
                Ok(None) => {
                    tracing::info!("No GPU available - using CPU fallback");
                    None
                }
                Err(e) => {
                    tracing::warn!("Failed to initialize GPU: {} - using CPU fallback", e);
                    None
                }
            }
        })
        .clone()
}

fn gpu_disabled() -> bool {
    match env::var("TRUESHOT_DISABLE_GPU") {
        Ok(val) => matches!(val.to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}
