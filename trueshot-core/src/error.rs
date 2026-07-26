//! Unified Error Handling for TrueShot
//!
//! Provides a comprehensive error type for all TrueShot operations with
//! user-friendly messages and proper error chaining.

use thiserror::Error;

/// Unified error type for all TrueShot operations
#[derive(Error, Debug)]
pub enum TrueShotError {
    // ================== Hardware Errors ==================
    #[error("Camera Error: {0}")]
    Camera(String),
    
    #[error("Device not found: {0}")]
    DeviceNotFound(String),
    
    #[error("Hardware communication failed: {0}")]
    HardwareCommunication(String),
    
    // ================== Processing Errors ==================
    #[error("Vision Processing Error: {0}")]
    Vision(String),
    
    #[error("AI Inference Error: {0}")]
    AI(String),
    
    #[error("Pipeline Error: {0}")]
    Pipeline(String),
    
    #[error("Processing Error: {0}")]
    Processing(String),
    
    // ================== 3D/Graphics Errors ==================
    #[error("3D Gaussian Splatting Error: {0}")]
    GaussianSplatting(String),
    
    #[error("Mesh Generation Failed: {0}")]
    MeshGeneration(String),
    
    #[error("GPU Error: {0}")]
    Gpu(String),
    
    #[error("Rendering Error: {0}")]
    Rendering(String),
    
    // ================== Streaming Errors ==================
    #[error("Streaming Error: {0}")]
    Streaming(String),
    
    #[error("Compression Failed: {0}")]
    Compression(String),
    
    #[error("Network Error: {0}")]
    Network(String),
    
    // ================== Storage/Config Errors ==================
    #[error("Storage Error: {0}")]
    Storage(#[from] std::io::Error),
    
    #[error("Configuration Error: {0}")]
    Config(#[from] config::ConfigError),
    
    #[error("I/O Error: {0}")]
    Io(String),
    
    // ================== Control Flow ==================
    #[error("Operation cancelled by user")]
    Cancelled,
    
    #[error("Operation timed out after {0} seconds")]
    Timeout(u64),
    
    #[error("Invalid state: {0}")]
    InvalidState(String),
    
    // ================== Catch-all ==================
    #[error("Unknown Error: {0}")]
    Unknown(String),
}

impl TrueShotError {
    /// Get a user-friendly suggestion for how to resolve this error
    pub fn suggestion(&self) -> Option<&'static str> {
        match self {
            Self::DeviceNotFound(_) => Some("Check that the device is connected and powered on."),
            Self::HardwareCommunication(_) => Some("Try unplugging and reconnecting the device."),
            Self::Gpu(_) => Some("Ensure GPU drivers are up-to-date and the device meets minimum requirements."),
            Self::Cancelled => Some("You can restart the operation when ready."),
            Self::Timeout(_) => Some("Try reducing the workload or checking system resources."),
            Self::Network(_) => Some("Check your internet connection and try again."),
            _ => None,
        }
    }
    
    /// Check if this error is recoverable (operation can be retried)
    pub fn is_recoverable(&self) -> bool {
        matches!(self, 
            Self::Network(_) | 
            Self::Timeout(_) | 
            Self::HardwareCommunication(_) |
            Self::Streaming(_)
        )
    }
}

/// Convenience type alias for TrueShotError results
pub type Result<T> = std::result::Result<T, TrueShotError>;

/// Re-export as Error for convenience
pub type Error = TrueShotError;

// ============================================================================
// From implementations for common error types
// ============================================================================

impl From<String> for TrueShotError {
    fn from(s: String) -> Self {
        TrueShotError::Unknown(s)
    }
}

impl From<&str> for TrueShotError {
    fn from(s: &str) -> Self {
        TrueShotError::Unknown(s.to_string())
    }
}
