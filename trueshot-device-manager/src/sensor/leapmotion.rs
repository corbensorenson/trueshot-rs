//! Leap Motion Controller Driver
//!
//! Dual-purpose integration for:
//! 1. **Gesture Control** - Hand tracking for UI navigation
//! 2. **Stereo Scanning** - Raw camera access for depth/point cloud generation
//!
//! ## Hardware Specifications
//! - 2x IR stereo cameras (640×240 each)
//! - 120 fps frame rate
//! - 135° field of view
//! - 25-600mm effective range
//! - 850nm IR wavelength
//!
//! ## USB Detection
//! - Vendor ID: 0xf182
//! - Product ID: 0x0003

use super::{Sensor, SensorCapabilities, SensorData, SensorType};
use anyhow::{anyhow, Result};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

/// Leap Motion operation mode
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum LeapMotionMode {
    /// Hand tracking for gesture control
    #[default]
    GestureControl,
    /// Raw stereo camera access for 3D scanning
    StereoScanning,
}

/// Recognized hand gestures
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HandGesture {
    None,
    OpenHand, // Palm open, fingers spread
    Fist,     // Closed fist
    Pinch,    // Thumb and index finger together
    Point,    // Index finger extended
    Grab,     // All fingers curling inward
    Swipe,    // Quick lateral movement
    Circle,   // Circular finger motion
}

/// Tracked hand data
#[derive(Debug, Clone)]
pub struct TrackedHand {
    /// Hand identifier
    pub id: u32,
    /// Left or right hand
    pub is_left: bool,
    /// Palm position in mm (x, y, z)
    pub palm_position: (f32, f32, f32),
    /// Palm velocity in mm/s
    pub palm_velocity: (f32, f32, f32),
    /// Palm normal vector
    pub palm_normal: (f32, f32, f32),
    /// Grab strength (0.0 = open, 1.0 = closed fist)
    pub grab_strength: f32,
    /// Pinch strength (0.0 = open, 1.0 = pinched)
    pub pinch_strength: f32,
    /// Current detected gesture
    pub gesture: HandGesture,
    /// Finger tip positions (thumb, index, middle, ring, pinky)
    pub finger_tips: [(f32, f32, f32); 5],
    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,
}

impl Default for TrackedHand {
    fn default() -> Self {
        Self {
            id: 0,
            is_left: true,
            palm_position: (0.0, 0.0, 0.0),
            palm_velocity: (0.0, 0.0, 0.0),
            palm_normal: (0.0, -1.0, 0.0),
            grab_strength: 0.0,
            pinch_strength: 0.0,
            gesture: HandGesture::None,
            finger_tips: [(0.0, 0.0, 0.0); 5],
            confidence: 0.0,
        }
    }
}

/// Internal state for Leap Motion
struct LeapState {
    hands: Vec<TrackedHand>,
    left_image: Vec<u8>,
    right_image: Vec<u8>,
    frame_id: u64,
    timestamp: u64,
}

impl Default for LeapState {
    fn default() -> Self {
        // 640x240 grayscale images
        let image_size = 640 * 240;
        Self {
            hands: Vec::new(),
            left_image: vec![0u8; image_size],
            right_image: vec![0u8; image_size],
            frame_id: 0,
            timestamp: 0,
        }
    }
}

/// Leap Motion Controller
///
/// Provides hand tracking and raw stereo camera access.
pub struct LeapMotionController {
    id: String,
    device_index: u32,
    mode: Arc<Mutex<LeapMotionMode>>,
    state: Arc<Mutex<LeapState>>,
    running: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
}

// Thread safety
unsafe impl Send for LeapMotionController {}
unsafe impl Sync for LeapMotionController {}

impl LeapMotionController {
    /// USB identifiers
    pub const VENDOR_ID: u16 = 0xf182;
    pub const PRODUCT_ID: u16 = 0x0003;

    /// Camera specifications
    pub const CAMERA_WIDTH: u32 = 640;
    pub const CAMERA_HEIGHT: u32 = 240;
    pub const FRAME_RATE: u32 = 120;
    pub const FOV_DEGREES: u32 = 135;
    pub const RANGE_MIN_MM: u32 = 25;
    pub const RANGE_MAX_MM: u32 = 600;

    /// Detect Leap Motion device
    pub fn detect() -> Option<u32> {
        // Check via system_profiler on macOS
        if let Ok(output) = std::process::Command::new("system_profiler")
            .args(["SPUSBDataType"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("Leap Motion") {
                tracing::info!("Leap Motion Controller detected via USB");
                return Some(0);
            }
        }
        None
    }

    /// Create new Leap Motion controller instance
    pub fn new(device_index: u32) -> Result<Self> {
        let controller = Self {
            id: format!("LeapMotion_{}", device_index),
            device_index,
            mode: Arc::new(Mutex::new(LeapMotionMode::default())),
            state: Arc::new(Mutex::new(LeapState::default())),
            running: Arc::new(AtomicBool::new(false)),
            connected: Arc::new(AtomicBool::new(false)),
        };

        controller.initialize()?;

        Ok(controller)
    }

    /// Initialize the Leap Motion device
    fn initialize(&self) -> Result<()> {
        tracing::info!("Initializing Leap Motion device {}", self.device_index);

        self.running.store(true, Ordering::SeqCst);
        self.connected.store(true, Ordering::SeqCst);

        // Spawn tracking thread
        let state = Arc::clone(&self.state);
        let running = Arc::clone(&self.running);
        let mode = Arc::clone(&self.mode);
        let device_index = self.device_index;

        thread::spawn(move || {
            tracing::info!(
                "Leap Motion tracking thread started for device {}",
                device_index
            );

            while running.load(Ordering::SeqCst) {
                let current_mode = *mode.lock().unwrap_or_else(|e| e.into_inner());

                if let Ok(mut state) = state.lock() {
                    state.frame_id += 1;
                    state.timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_micros() as u64)
                        .unwrap_or(0);

                    match current_mode {
                        LeapMotionMode::GestureControl => {
                            // In production, this would poll LeapC API for hand data
                            // Simulate hand detection
                        }
                        LeapMotionMode::StereoScanning => {
                            // In production, this would capture raw stereo images
                            // via LeapC image API
                        }
                    }
                }

                // ~120fps
                thread::sleep(Duration::from_micros(8333));
            }

            tracing::info!("Leap Motion tracking thread stopped");
        });

        Ok(())
    }

    /// Set operation mode
    pub fn set_mode(&self, mode: LeapMotionMode) -> Result<()> {
        let mut current = self.mode.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
        *current = mode;
        tracing::info!("Leap Motion mode set to {:?}", mode);
        Ok(())
    }

    /// Get current mode
    pub fn get_mode(&self) -> Result<LeapMotionMode> {
        let current = self.mode.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
        Ok(*current)
    }

    /// Get tracked hands
    pub fn get_hands(&self) -> Result<Vec<TrackedHand>> {
        let state = self
            .state
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        Ok(state.hands.clone())
    }

    /// Get stereo camera images
    pub fn get_stereo_images(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        let state = self
            .state
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        Ok((state.left_image.clone(), state.right_image.clone()))
    }

    /// Compute disparity map from stereo images
    ///
    /// This generates depth information using stereo correspondence.
    /// Output is a grayscale depth map where brighter = closer.
    pub fn compute_disparity(&self) -> Result<Vec<u8>> {
        let (left, right) = self.get_stereo_images()?;

        // In production, use OpenCV's StereoSGBM or similar
        // For now, simple block matching approximation
        let width = Self::CAMERA_WIDTH as usize;
        let height = Self::CAMERA_HEIGHT as usize;
        let mut disparity = vec![0u8; width * height];

        let block_size = 15;
        let max_disparity = 64;
        let half_block = block_size / 2;

        for y in half_block..(height - half_block) {
            for x in (half_block + max_disparity)..(width - half_block) {
                let mut best_disparity = 0;
                let mut best_sad = u32::MAX;

                // Search for matching block in right image
                for d in 0..max_disparity {
                    let mut sad = 0u32;

                    for by in 0..block_size {
                        for bx in 0..block_size {
                            let ly = y - half_block + by;
                            let lx = x - half_block + bx;
                            let rx = lx - d;

                            let left_pixel = left[ly * width + lx] as i32;
                            let right_pixel = right[ly * width + rx] as i32;
                            sad += (left_pixel - right_pixel).unsigned_abs();
                        }
                    }

                    if sad < best_sad {
                        best_sad = sad;
                        best_disparity = d;
                    }
                }

                // Convert disparity to grayscale (inverted: closer = brighter)
                disparity[y * width + x] =
                    ((max_disparity - best_disparity) * 255 / max_disparity) as u8;
            }
        }

        Ok(disparity)
    }

    /// Generate point cloud from disparity map
    ///
    /// Returns Vec of (x, y, z) points in millimeters
    pub fn generate_point_cloud(&self) -> Result<Vec<(f32, f32, f32)>> {
        let disparity = self.compute_disparity()?;
        let width = Self::CAMERA_WIDTH as usize;
        let height = Self::CAMERA_HEIGHT as usize;

        // Leap Motion baseline and focal length (approximate)
        let baseline = 4.0; // ~4mm between cameras
        let focal_length = Self::CAMERA_WIDTH as f32 * 0.8; // Approximate

        let mut points = Vec::new();
        let cx = width as f32 / 2.0;
        let cy = height as f32 / 2.0;

        for y in 0..height {
            for x in 0..width {
                let d = disparity[y * width + x] as f32;

                if d > 10.0 {
                    // Minimum disparity threshold
                    // Z = baseline * focal_length / disparity
                    let z = baseline * focal_length / d;
                    let px = (x as f32 - cx) * z / focal_length;
                    let py = (y as f32 - cy) * z / focal_length;

                    // Filter to valid range
                    if z > Self::RANGE_MIN_MM as f32 && z < Self::RANGE_MAX_MM as f32 {
                        points.push((px, py, z));
                    }
                }
            }
        }

        Ok(points)
    }

    /// Interpret gesture from hand data
    pub fn interpret_gesture(hand: &TrackedHand) -> HandGesture {
        // Priority order of gesture detection
        if hand.grab_strength > 0.9 {
            return HandGesture::Fist;
        }

        if hand.pinch_strength > 0.8 {
            return HandGesture::Pinch;
        }

        if hand.grab_strength < 0.2 {
            return HandGesture::OpenHand;
        }

        HandGesture::None
    }

    /// Stop the controller
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);
    }
}

impl Drop for LeapMotionController {
    fn drop(&mut self) {
        self.stop();
        tracing::info!("Leap Motion device {} closed", self.device_index);
    }
}

impl Sensor for LeapMotionController {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn name(&self) -> String {
        "Leap Motion Controller".to_string()
    }

    fn sensor_type(&self) -> SensorType {
        SensorType::Combined
    }

    fn capabilities(&self) -> SensorCapabilities {
        SensorCapabilities {
            has_hand_tracking: true,
            has_stereo_cameras: true,
            has_depth: true, // Via stereo disparity
            has_accelerometer: false,
            has_gyroscope: false,
            frame_rate: Some(Self::FRAME_RATE),
            resolution: Some((Self::CAMERA_WIDTH, Self::CAMERA_HEIGHT)),
            tracking_range_mm: Some((Self::RANGE_MIN_MM, Self::RANGE_MAX_MM)),
        }
    }

    fn poll(&self) -> Result<SensorData> {
        let mode = self.get_mode()?;

        match mode {
            LeapMotionMode::GestureControl => {
                let hands = self.get_hands()?;
                let state = self
                    .state
                    .lock()
                    .map_err(|e| anyhow!("Lock error: {}", e))?;
                Ok(SensorData::HandTracking {
                    hands,
                    timestamp: state.timestamp,
                })
            }
            LeapMotionMode::StereoScanning => {
                let (left, right) = self.get_stereo_images()?;
                let state = self
                    .state
                    .lock()
                    .map_err(|e| anyhow!("Lock error: {}", e))?;
                Ok(SensorData::StereoImages {
                    left,
                    right,
                    width: Self::CAMERA_WIDTH,
                    height: Self::CAMERA_HEIGHT,
                    timestamp: state.timestamp,
                })
            }
        }
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_leap_detection() {
        if let Some(index) = LeapMotionController::detect() {
            println!("Leap Motion detected at index {}", index);
        } else {
            println!("No Leap Motion detected (OK for CI)");
        }
    }

    #[test]
    fn test_gesture_interpretation() {
        let mut hand = TrackedHand {
            grab_strength: 0.95,
            ..TrackedHand::default()
        };
        assert_eq!(
            LeapMotionController::interpret_gesture(&hand),
            HandGesture::Fist
        );

        hand.grab_strength = 0.1;
        hand.pinch_strength = 0.9;
        assert_eq!(
            LeapMotionController::interpret_gesture(&hand),
            HandGesture::Pinch
        );

        hand.grab_strength = 0.1;
        hand.pinch_strength = 0.1;
        assert_eq!(
            LeapMotionController::interpret_gesture(&hand),
            HandGesture::OpenHand
        );
    }
}
