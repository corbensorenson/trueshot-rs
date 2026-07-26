use anyhow::Result;
use image::{ImageBuffer, Rgb};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::Mutex;
use trueshot_core::director::Director;
use trueshot_core::events::{EventBus, SystemEvent};
use trueshot_core::scanning::workflow::{CaptureConfig, ScanAction, ScanWorkflow};
use trueshot_core::scanning::QualityLevel;
use trueshot_device_manager::{
    Camera, CameraCapabilities, CameraConfig, CameraManager, CameraRole, MockTurntable,
};

struct FileBackedCamera {
    source_dir: PathBuf,
    sequence: AtomicUsize,
}

impl Camera for FileBackedCamera {
    fn id(&self) -> String {
        "integration-camera".to_string()
    }

    fn capture(&self, _config: &CameraConfig) -> Result<PathBuf> {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let path = self.source_dir.join(format!("capture-{sequence}.nef"));
        std::fs::write(&path, b"deterministic mock raw payload")?;
        Ok(path)
    }

    fn capture_preview(&self) -> Result<Vec<u8>> {
        Ok(vec![0; 16])
    }

    fn set_config(&self, _config: &CameraConfig) -> Result<()> {
        Ok(())
    }

    fn battery_level(&self) -> Result<u8> {
        Ok(100)
    }
}

#[tokio::test]
async fn test_full_scan_cycle_mock() -> Result<()> {
    let temp_dir = tempdir()?;
    let bus = Arc::new(EventBus::new());
    let turntable = Box::new(MockTurntable::new());
    let source_dir = temp_dir.path().join("camera-source");
    std::fs::create_dir_all(&source_dir)?;
    let mut camera_manager = CameraManager::new();
    camera_manager.registry.register_camera(
        "integration-camera".to_string(),
        "Integration Camera".to_string(),
        CameraRole::HighResCapture,
        CameraCapabilities::default(),
    );
    camera_manager.cameras.push(Arc::new(FileBackedCamera {
        source_dir,
        sequence: AtomicUsize::new(0),
    }));
    let cameras = Arc::new(Mutex::new(camera_manager));
    let director = Arc::new(Director::new(bus.clone(), cameras, turntable, None));

    use trueshot_core::project::ScanProject;
    let project = ScanProject::new("Test Project", temp_dir.path())?;
    let mut rx = bus.subscribe();
    director.load_project(project).await?;

    // Loading a project must never move hardware or begin a scan.
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), rx.recv())
            .await
            .is_err()
    );

    director
        .start_workflow(ScanWorkflow {
            name: "Bounded mock scan".to_string(),
            steps: vec![ScanAction::SmartScan {
                quality: QualityLevel::Preview,
                capture: CaptureConfig::Single,
            }],
        })
        .await;

    let started = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if matches!(rx.recv().await?, SystemEvent::CaptureStarted(_)) {
                return Ok::<_, tokio::sync::broadcast::error::RecvError>(true);
            }
        }
    })
    .await??;
    assert!(started);

    let frame = ImageBuffer::from_pixel(16, 16, Rgb([128, 128, 128]));
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        director.process_frame(&frame),
    )
    .await?;
    let progressed = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if matches!(rx.recv().await?, SystemEvent::CaptureProgress(_, _)) {
                return Ok::<_, tokio::sync::broadcast::error::RecvError>(true);
            }
        }
    })
    .await??;
    assert!(progressed);

    Ok(())
}

#[tokio::test]
async fn empty_camera_capture_fails_without_deadlock() -> Result<()> {
    let temp_dir = tempdir()?;
    let bus = Arc::new(EventBus::new());
    let director = Director::new(
        bus.clone(),
        Arc::new(Mutex::new(CameraManager::new())),
        Box::new(MockTurntable::new()),
        None,
    );
    let project = trueshot_core::project::ScanProject::new("No Camera", temp_dir.path())?;
    director.load_project(project).await?;

    let mut rx = bus.subscribe();
    director
        .start_workflow(ScanWorkflow {
            name: "No-camera failure".to_string(),
            steps: vec![ScanAction::SmartScan {
                quality: QualityLevel::Preview,
                capture: CaptureConfig::Single,
            }],
        })
        .await;

    let frame = ImageBuffer::from_pixel(16, 16, Rgb([128, 128, 128]));
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        director.process_frame(&frame),
    )
    .await?;

    let error = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if let SystemEvent::SystemMessage(message, _) = rx.recv().await? {
                if message.contains("No active cameras") {
                    return Ok::<_, tokio::sync::broadcast::error::RecvError>(message);
                }
            }
        }
    })
    .await??;
    assert!(error.contains("No active cameras"));

    Ok(())
}
