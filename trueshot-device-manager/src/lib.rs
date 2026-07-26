pub mod camera;
pub mod turntable;
pub mod audio;
pub mod storage;
pub mod sensor;
// pub mod calibration; // Port calibration later if desired

pub use camera::*;
pub use audio::*;
pub use storage::*;
pub use sensor::*;

// Re-export common types
pub use camera::{Camera, CameraManager, CameraConfig};
pub use turntable::{Turntable, SerialTurntable, Foldio360, MockTurntable, TurntableFeedbackConfig};
pub use audio::{AudioDevice, AudioManager, AudioCaptureStream};
pub use storage::{StorageManager, StorageConfig, StorageType};
pub use sensor::{Sensor, SensorManager, LeapMotionController};
