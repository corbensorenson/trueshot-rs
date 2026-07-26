//! GPU acceleration module using wgpu
//!
//! Provides GPU-accelerated implementations of compute-intensive operations:
//! - Mertens weight computation (contrast, saturation, exposedness)
//! - Weighted pixel collapse (hierarchical grading)
//! - VNG demosaicing
//! - Super-resolution upsampling
//!
//! Falls back to CPU if GPU is unavailable or disabled.

#[cfg(feature = "gpu")]
pub mod gpu_context;

#[cfg(feature = "gpu")]
pub mod gpu_collapse;

#[cfg(feature = "gpu")]
pub mod gpu_mertens;

#[cfg(feature = "gpu")]
pub mod gpu_sharpness;

#[cfg(feature = "gpu")]
pub mod gpu_postprocess;

// Re-export main types
#[cfg(feature = "gpu")]
pub use gpu_context::{GpuContext, get_gpu_context};

#[cfg(feature = "gpu")]
pub use gpu_collapse::gpu_collapse_pixels;

#[cfg(feature = "gpu")]
pub use gpu_mertens::gpu_compute_mertens_weights;

#[cfg(feature = "gpu")]
pub use gpu_sharpness::gpu_compute_sharpness_masks;

#[cfg(feature = "gpu")]
pub use gpu_postprocess::gpu_postprocess;

/// Check if GPU acceleration is available
pub fn is_gpu_available() -> bool {
    #[cfg(feature = "gpu")]
    {
        gpu_context::is_gpu_available()
    }
    #[cfg(not(feature = "gpu"))]
    {
        false
    }
}

