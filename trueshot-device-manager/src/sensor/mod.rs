//! Sensor Module
//!
//! Unified interface for various sensors including:
//! - Leap Motion (hand tracking, stereo depth)
//! - Future: IMU, force sensors, etc.

pub mod leapmotion;

pub use leapmotion::{LeapMotionController, LeapMotionMode, TrackedHand, HandGesture};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Sensor type classification
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SensorType {
    HandTracker,
    DepthSensor,
    Accelerometer,
    Gyroscope,
    Combined,
}

/// Sensor capabilities
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SensorCapabilities {
    pub has_hand_tracking: bool,
    pub has_stereo_cameras: bool,
    pub has_depth: bool,
    pub has_accelerometer: bool,
    pub has_gyroscope: bool,
    pub frame_rate: Option<u32>,
    pub resolution: Option<(u32, u32)>,
    pub tracking_range_mm: Option<(u32, u32)>,  // (min, max)
}

/// Generic sensor trait
pub trait Sensor: Send + Sync {
    fn id(&self) -> String;
    fn name(&self) -> String;
    fn sensor_type(&self) -> SensorType;
    fn capabilities(&self) -> SensorCapabilities;
    
    /// Poll for latest sensor data
    fn poll(&self) -> Result<SensorData>;
    
    /// Check if sensor is connected and working
    fn is_connected(&self) -> bool;
}

/// Sensor data output
#[derive(Debug, Clone)]
pub enum SensorData {
    HandTracking {
        hands: Vec<TrackedHand>,
        timestamp: u64,
    },
    StereoImages {
        left: Vec<u8>,
        right: Vec<u8>,
        width: u32,
        height: u32,
        timestamp: u64,
    },
    Accelerometer {
        x: f64,
        y: f64,
        z: f64,
    },
    None,
}

/// Sensor manager for device orchestration
pub struct SensorManager {
    pub sensors: Vec<std::sync::Arc<dyn Sensor>>,
}

impl SensorManager {
    pub fn new() -> Self {
        Self { sensors: Vec::new() }
    }
    
    /// Detect and add available sensors
    pub fn detect_sensors(&mut self) -> Vec<String> {
        let mut added = Vec::new();
        
        // Detect Leap Motion
        if let Some(index) = LeapMotionController::detect() {
            if let Ok(leap) = LeapMotionController::new(index) {
                added.push(leap.id());
                self.sensors.push(std::sync::Arc::new(leap));
            }
        }
        
        added
    }
    
    /// Get sensor by ID
    pub fn get_sensor(&self, id: &str) -> Option<std::sync::Arc<dyn Sensor>> {
        self.sensors.iter().find(|s| s.id() == id).cloned()
    }
    
    /// Get all hand tracking sensors
    pub fn hand_trackers(&self) -> Vec<std::sync::Arc<dyn Sensor>> {
        self.sensors.iter()
            .filter(|s| s.capabilities().has_hand_tracking)
            .cloned()
            .collect()
    }
}

impl Default for SensorManager {
    fn default() -> Self {
        Self::new()
    }
}
