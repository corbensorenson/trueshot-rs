//! WGPU Compute Infrastructure
//! 
//! Provides a high-level abstraction for running exact math operations on the GPU.
//! Targeted for: FFT, Wavelet Transforms, and Pixel Fusion.

#[cfg(feature = "gpu")]
use wgpu;
use anyhow::Result;
#[cfg(feature = "gpu")]
use anyhow::Context;
#[cfg(feature = "gpu")]
use std::sync::Arc;

#[cfg(not(feature = "gpu"))]
pub struct GpuContext; 

#[cfg(feature = "gpu")]
pub struct GpuContext {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
}

#[cfg(feature = "gpu")]
impl GpuContext {
    pub async fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }).await.context("Failed to find suitable GPU adapter")?;
        
        tracing::info!("Initializing GPU Backend: {:?}", adapter.get_info());

        let (device, queue) = adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("TrueShot Computer"),
                required_features: wgpu::Features::empty(), // Add features like SPIR-V if needed later
                required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            },
            None,
        ).await.context("Failed to request device")?;

        Ok(Self {
            device: Arc::new(device),
            queue: Arc::new(queue),
        })
    }
    
    // Helper to create a storage buffer
    pub fn create_buffer_init<T: bytemuck::Pod>(&self, label: &str, contents: &[T]) -> wgpu::Buffer {
        use wgpu::util::DeviceExt;
        self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(contents),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        })
    }
}

// Stub for non-gpu builds
#[cfg(not(feature = "gpu"))]
impl GpuContext {
    pub async fn new() -> Result<Self> {
        tracing::warn!("GPU feature disabled");
        Ok(Self)
    }
}
