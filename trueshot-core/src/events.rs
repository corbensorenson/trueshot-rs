use tokio::sync::broadcast;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SystemEvent {
    CaptureStarted(u32), // Camera Index
    CaptureProgress(u32, f32), // Index, Progress 0.0-1.0
    ScanComplete,
    CaptureCompleted(u32, String), // Index, Path
    CaptureFailed(u32, String), // Index, Error
    
    ProcessingStarted(String), // Job ID
    ProcessingProgress(String, f32), // Job ID, Progress
    ProcessingCompleted(String),
    ProcessingFailed(String, String),
    
    // Hardware Events
    DeviceConnected { kind: String, id: String },
    DeviceDisconnected { id: String },
    TurntableStatus { connected: bool, angle: f32, moving: bool },

    TurntableRotating(f32), // Angle
    TurntableStopped(f32),
    
    SystemMessage(String, LogLevel),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
    Success,
}

pub struct EventBus {
    tx: broadcast::Sender<SystemEvent>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(100);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SystemEvent> {
        self.tx.subscribe()
    }

    pub fn publish(&self, event: SystemEvent) {
        let _ = self.tx.send(event);
    }
}
