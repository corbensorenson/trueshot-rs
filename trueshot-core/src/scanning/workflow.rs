use serde::{Serialize, Deserialize};
use crate::scanning::QualityLevel;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CaptureConfig {
    Single,
    FocusStack { count: usize, step: Option<u32> },
    Burst { count: usize, fps: u32 },
    Hdr { stops: Vec<f32> },
    
    /// Advanced: Combine Focus Stacking and HDR
    ComplexStack {
        focus_count: usize,
        focus_step_size: Option<u32>,
        hdr_stops: Vec<f32>, // e.g. [-2.0, 0.0, +2.0]
    },
    
    /// Automatically calculated configuration
    Auto { quality: QualityLevel },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ScanAction {
    /// Execute a smart scan pass with given quality parameters
    SmartScan { quality: QualityLevel, capture: CaptureConfig },
    
    /// Homing the turntable
    HomeTurntable,
    
    /// Prompt the user to perform an action (e.g. "Flip Object")
    /// The system waits for scene stabilization before proceeding.
    PromptUser { message: String },
    
    /// Trigger background processing
    StartProcessing,

    /// Capture background images for masking (turntable rotation)
    CaptureBackground,

    /// Verify connected hardware (Cameras, Turntable)
    VerifyHardware,

    /// Wait for user to insert SD cards (if applicable)
    WaitForSDCard,
    
    /// Calibrate settings (Focus, Step Size) based on Quality
    Calibrate { quality: QualityLevel },
    
    /// Verify Exposure (Histogram check)
    CheckExposure,
    
    /// Verify Object Centering (vs Background)
    CheckCentering,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanWorkflow {
    pub name: String,
    pub steps: Vec<ScanAction>,
}

impl ScanWorkflow {
    pub fn standard() -> Self {
        Self {
            name: "Standard Scan".to_string(),
            steps: vec![
                ScanAction::VerifyHardware,
                ScanAction::HomeTurntable,
                // Check exposure on empty stage (lighting check)
                ScanAction::CheckExposure,
                ScanAction::PromptUser { message: "Clear Turntable for Background".to_string() },
                ScanAction::CaptureBackground,
                ScanAction::PromptUser { message: "Place Object".to_string() },
                ScanAction::CheckCentering, // New Safety Check
                ScanAction::Calibrate { quality: QualityLevel::High },
                ScanAction::PromptUser { message: "Flip Object".to_string() },
                ScanAction::SmartScan { quality: QualityLevel::High, capture: CaptureConfig::Auto { quality: QualityLevel::High } },
                ScanAction::WaitForSDCard,
                ScanAction::StartProcessing,
            ]
        }
    }
    
    pub fn rapid() -> Self {
        Self {
             name: "Rapid Scan".to_string(),
             steps: vec![
                 ScanAction::HomeTurntable,
                 ScanAction::PromptUser { message: "Place Object".to_string() },
                 ScanAction::SmartScan { quality: QualityLevel::Preview, capture: CaptureConfig::Single },
                 ScanAction::StartProcessing,
             ]
        }
    }
}
