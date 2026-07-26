//! Camera and Sensor Detection Test
//! 
//! Run with: cargo run --example test_cameras -p trueshot-device-manager

use trueshot_device_manager::camera::{CameraManager, KinectCamera};
use trueshot_device_manager::sensor::{SensorManager, LeapMotionController, LeapMotionMode};

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();
    
    println!("=== TrueShot Device Detection Test ===\n");
    
    // Test Kinect detection
    println!("1. Testing Kinect detection...");
    match KinectCamera::detect() {
        Some(index) => {
            println!("   ✅ Kinect v1 detected at index {}", index);
            println!("   - RGB: 640x480 @ 30fps");
            println!("   - Depth: 640x480, 11-bit (2048 levels)");
            println!("   - Tilt range: ±27 degrees");
        }
        None => {
            println!("   ❌ No Kinect detected");
        }
    }
    
    // Test Leap Motion detection
    println!("\n2. Testing Leap Motion detection...");
    match LeapMotionController::detect() {
        Some(index) => {
            println!("   ✅ Leap Motion detected at index {}", index);
            println!("   - Stereo IR Cameras: 640x240 @ 120fps");
            println!("   - Tracking range: 25-600mm");
            println!("   - Modes: Gesture Control, Stereo Scanning");
        }
        None => {
            println!("   ❌ No Leap Motion detected");
        }
    }
    
    // Test camera reconciliation
    println!("\n3. Testing CameraManager reconciliation...");
    let mut cam_manager = CameraManager::new();
    
    match cam_manager.reconcile_cameras(false).await {
        Ok(report) => {
            println!("   Cameras added: {:?}", report.added);
        }
        Err(e) => {
            println!("   ❌ Error: {}", e);
        }
    }
    
    // Test sensor detection
    println!("\n4. Testing SensorManager...");
    let mut sensor_manager = SensorManager::new();
    let sensors_added = sensor_manager.detect_sensors();
    println!("   Sensors added: {:?}", sensors_added);
    
    // List all devices
    println!("\n5. All detected devices:");
    
    println!("\n   CAMERAS:");
    for (i, cam) in cam_manager.cameras.iter().enumerate() {
        let id = cam.id();
        let profile = cam_manager.registry.get_profile(&id);
        
        println!("   [{}] {}", i, id);
        if let Some(p) = profile {
            println!("       Name: {}", p.name);
            println!("       Role: {:?}", p.role);
            let caps = &p.capabilities;
            if caps.has_depth {
                println!("       Type: Depth Camera");
            } else {
                println!("       Type: RGB Camera");
            }
        }
    }
    
    println!("\n   SENSORS:");
    for (i, sensor) in sensor_manager.sensors.iter().enumerate() {
        let caps = sensor.capabilities();
        println!("   [{}] {}", i, sensor.id());
        println!("       Name: {}", sensor.name());
        println!("       Type: {:?}", sensor.sensor_type());
        println!("       Hand Tracking: {}", caps.has_hand_tracking);
        println!("       Stereo Cameras: {}", caps.has_stereo_cameras);
        if let Some(range) = caps.tracking_range_mm {
            println!("       Range: {}mm - {}mm", range.0, range.1);
        }
    }
    
    println!("\n=== Test Complete ===");
}
