//! License error types

use thiserror::Error;

/// Errors that can occur during license operations
#[derive(Error, Debug)]
pub enum LicenseError {
    #[error("No license found")]
    NoLicense,

    #[error("License has expired")]
    Expired,

    #[error("Device not activated on this license")]
    DeviceNotActivated,

    #[error("Maximum device limit reached ({0} devices)")]
    DeviceLimitReached(u32),

    #[error("Invalid license signature: {0}")]
    InvalidSignature(String),

    #[error("Signature verification failed")]
    SignatureVerificationFailed,

    #[error("License file not found: {0}")]
    FileNotFound(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Offline grace period expired")]
    GracePeriodExpired,

    #[error("Feature not available in current license: {0}")]
    FeatureNotAvailable(String),

    #[error("License activation failed: {0}")]
    ActivationFailed(String),

    #[error("Invalid license key format")]
    InvalidKeyFormat,

    #[error("Missing license public key in release build")]
    MissingPublicKey,

    #[error("Integrity check failed: {0}")]
    IntegrityFailure(String),
}
