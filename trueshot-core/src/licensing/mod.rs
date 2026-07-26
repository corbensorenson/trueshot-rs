//! TrueShot Licensing System
//!
//! Device-bound licensing with cryptographic verification for commercial deployment.
//! Supports offline verification with periodic heartbeat.

mod device;
mod encryption;
mod error;
mod integrity;
mod license;
mod manager;

pub use device::DeviceFingerprint;
#[cfg(any(test, feature = "dev_license"))]
pub use encryption::generate_dev_license;
pub use encryption::{LicenseData, LicenseType, LicenseVerifier, SignedLicense};
pub use error::LicenseError;
pub use integrity::{IntegrityChecker, IntegrityStatus, UsageCounter, UsageLimitError};
pub use license::{ActivatedDevice, License, LicenseFeatures, LicenseTier};
pub use manager::{LicenseManager, LicenseStatus};

/// Feature flags that can be gated by license
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    /// Basic scanning functionality
    BasicScanning,
    /// High resolution output (4K+)
    Resolution4K,
    /// Maximum resolution (8K)
    Resolution8K,
    /// 3D Gaussian Splatting
    GaussianSplatting,
    /// 4D Gaussian Splatting (dynamic scenes)
    FourDGS,
    /// WebXR VR scanning
    WebXRScanning,
    /// Commercial use
    CommercialUse,
    /// Beta features early access
    BetaFeatures,
    /// Priority support access
    PrioritySupport,
    /// Unlimited scans per month
    UnlimitedScans,
    /// Adaptive room-scale reconstruction workflows
    RoomReconstruction,
    /// Full avatar capture and reconstruction workflows
    AvatarReconstruction,
    /// Advanced capture automation (HDR/focus stack/intervalometer orchestration)
    AdvancedCaptureAutomation,
    /// Cloud/NAS sync and backup/restore pipelines
    CloudSyncBackup,
    /// Public sharing, gallery discovery, and review collaboration
    TeamCollaboration,
    /// Pipeline automation APIs and webhook integrations
    PipelineAutomation,
}
