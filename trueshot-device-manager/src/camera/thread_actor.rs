use super::CameraConfig;
use anyhow::{anyhow, Context, Result};
use nokhwa::{
    pixel_format::RgbFormat,
    utils::{
        CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
    },
    Camera,
};
use std::any::Any;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_INITIALIZATION_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_QUEUE_CAPACITY: usize = 2;

enum CameraCommand {
    Capture(SyncSender<std::result::Result<PathBuf, String>>),
    Preview(SyncSender<std::result::Result<Vec<u8>, String>>),
    Configure(CameraConfig, SyncSender<std::result::Result<(), String>>),
    Shutdown(SyncSender<()>),
}

trait CameraBackend {
    fn is_available(&self) -> bool;
    fn capture(&mut self) -> Result<PathBuf>;
    fn preview(&mut self) -> Result<Vec<u8>>;
    fn configure(&mut self, config: &CameraConfig) -> Result<()>;
}

struct NokhwaBackend {
    camera: Option<Camera>,
    id: String,
    capture_sequence: u64,
}

impl NokhwaBackend {
    fn open(
        index: u32,
        id: String,
        formats: Vec<RequestedFormatType>,
        allow_unavailable: bool,
    ) -> Result<Self> {
        let camera_index = CameraIndex::Index(index);
        let mut last_error = None;

        for format_type in formats {
            let requested = RequestedFormat::new::<RgbFormat>(format_type);
            let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe({
                let camera_index = camera_index.clone();
                move || Camera::new(camera_index, requested)
            }));

            match opened {
                Ok(Ok(mut camera)) => match camera.open_stream() {
                    Ok(()) => {
                        return Ok(Self {
                            camera: Some(camera),
                            id,
                            capture_sequence: 0,
                        });
                    }
                    Err(error) => {
                        tracing::warn!(
                            "Camera {} stream open failed with {:?}: {}",
                            index,
                            format_type,
                            error
                        );
                        last_error = Some(error.to_string());
                    }
                },
                Ok(Err(error)) => {
                    tracing::debug!(
                        "Camera {} initialization failed with {:?}: {}",
                        index,
                        format_type,
                        error
                    );
                    last_error = Some(error.to_string());
                }
                Err(payload) => {
                    let message = panic_message(payload);
                    tracing::warn!(
                        "Camera {} initialization panicked with {:?}: {}",
                        index,
                        format_type,
                        message
                    );
                    last_error = Some(format!("backend panic: {message}"));
                }
            }
        }

        if allow_unavailable {
            tracing::warn!(
                "Camera stream {} is unavailable; non-camera controls remain enabled",
                id
            );
            Ok(Self {
                camera: None,
                id,
                capture_sequence: 0,
            })
        } else {
            Err(anyhow!(
                "failed to initialize camera {} with any format: {}",
                index,
                last_error.unwrap_or_else(|| "no compatible format".to_string())
            ))
        }
    }

    fn camera_mut(&mut self) -> Result<&mut Camera> {
        self.camera
            .as_mut()
            .ok_or_else(|| anyhow!("camera stream is not available"))
    }

    fn frame_jpeg(&mut self) -> Result<Vec<u8>> {
        let frame = self.camera_mut()?.frame()?;
        if frame.source_frame_format() == FrameFormat::MJPEG {
            return Ok(frame.buffer().to_vec());
        }

        let image = frame.decode_image::<RgbFormat>()?;
        let mut jpeg = Vec::new();
        image.write_to(
            &mut std::io::Cursor::new(&mut jpeg),
            image::ImageFormat::Jpeg,
        )?;
        Ok(jpeg)
    }
}

impl CameraBackend for NokhwaBackend {
    fn is_available(&self) -> bool {
        self.camera.is_some()
    }

    fn capture(&mut self) -> Result<PathBuf> {
        let jpeg = self.frame_jpeg()?;
        self.capture_sequence = self.capture_sequence.wrapping_add(1);
        let safe_id: String = self
            .id
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        let filename = format!(
            "capture_{}_{}_{}.jpg",
            safe_id,
            chrono::Utc::now().timestamp_millis(),
            self.capture_sequence
        );
        let path = std::env::temp_dir().join(filename);
        let partial = path.with_extension("jpg.partial");
        std::fs::write(&partial, jpeg)
            .with_context(|| format!("failed to write {}", partial.display()))?;
        std::fs::rename(&partial, &path)
            .with_context(|| format!("failed to publish {}", path.display()))?;
        Ok(path)
    }

    fn preview(&mut self) -> Result<Vec<u8>> {
        self.frame_jpeg()
    }

    fn configure(&mut self, config: &CameraConfig) -> Result<()> {
        let Some((width, height)) = config.resolution else {
            return Ok(());
        };
        let camera = self.camera_mut()?;
        let current = camera.camera_format();
        let frame_rate = config.fps.unwrap_or(current.frame_rate());
        if current.resolution() == Resolution::new(width, height)
            && current.frame_rate() == frame_rate
        {
            return Ok(());
        }

        let previous = current;
        camera.stop_stream()?;
        let requested =
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(CameraFormat::new(
                Resolution::new(width, height),
                FrameFormat::MJPEG,
                frame_rate,
            )));

        if let Err(error) = camera
            .set_camera_requset(requested)
            .and_then(|_| camera.open_stream())
        {
            let rollback =
                RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(previous));
            let _ = camera.set_camera_requset(rollback);
            let _ = camera.open_stream();
            return Err(anyhow!(
                "failed to switch camera to {}x{}@{}: {}",
                width,
                height,
                frame_rate,
                error
            ));
        }

        Ok(())
    }
}

pub(super) struct ThreadOwnedCamera {
    sender: SyncSender<CameraCommand>,
    worker: Mutex<Option<JoinHandle<()>>>,
    command_timeout: Duration,
    available: bool,
}

impl ThreadOwnedCamera {
    pub(super) fn open(
        index: u32,
        id: String,
        formats: Vec<RequestedFormatType>,
        allow_unavailable: bool,
    ) -> Result<Self> {
        Self::spawn_backend(
            format!("trueshot-camera-{index}"),
            DEFAULT_QUEUE_CAPACITY,
            DEFAULT_COMMAND_TIMEOUT,
            DEFAULT_INITIALIZATION_TIMEOUT,
            move || NokhwaBackend::open(index, id, formats, allow_unavailable),
        )
    }

    fn spawn_backend<B, F>(
        thread_name: String,
        queue_capacity: usize,
        command_timeout: Duration,
        initialization_timeout: Duration,
        factory: F,
    ) -> Result<Self>
    where
        B: CameraBackend + 'static,
        F: FnOnce() -> Result<B> + Send + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel(queue_capacity.max(1));
        let (initialization_sender, initialization_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let initialized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(factory));
                let mut backend = match initialized {
                    Ok(Ok(backend)) => backend,
                    Ok(Err(error)) => {
                        let _ = initialization_sender.send(Err(error.to_string()));
                        return;
                    }
                    Err(payload) => {
                        let _ = initialization_sender.send(Err(format!(
                            "camera initialization panicked: {}",
                            panic_message(payload)
                        )));
                        return;
                    }
                };

                let available = backend.is_available();
                if initialization_sender.send(Ok(available)).is_err() {
                    return;
                }
                run_worker(&mut backend, receiver);
            })
            .context("failed to spawn camera actor thread")?;

        let available = match initialization_receiver.recv_timeout(initialization_timeout) {
            Ok(Ok(available)) => available,
            Ok(Err(error)) => {
                drop(sender);
                drop(worker);
                return Err(anyhow!(error));
            }
            Err(RecvTimeoutError::Timeout) => {
                drop(sender);
                drop(worker);
                return Err(anyhow!(
                    "camera initialization exceeded {:.1}s deadline",
                    initialization_timeout.as_secs_f64()
                ));
            }
            Err(RecvTimeoutError::Disconnected) => {
                drop(sender);
                let _ = worker.join();
                return Err(anyhow!("camera actor exited during initialization"));
            }
        };

        Ok(Self {
            sender,
            worker: Mutex::new(Some(worker)),
            command_timeout,
            available,
        })
    }

    pub(super) fn is_available(&self) -> bool {
        self.available
    }

    pub(super) fn capture(&self) -> Result<PathBuf> {
        self.request(CameraCommand::Capture)
    }

    pub(super) fn preview(&self) -> Result<Vec<u8>> {
        self.request(CameraCommand::Preview)
    }

    pub(super) fn configure(&self, config: CameraConfig) -> Result<()> {
        self.request(|response| CameraCommand::Configure(config, response))
    }

    fn request<T, F>(&self, command: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(SyncSender<std::result::Result<T, String>>) -> CameraCommand,
    {
        let (response_sender, response_receiver) = mpsc::sync_channel(1);
        match self.sender.try_send(command(response_sender)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                return Err(anyhow!("camera command queue is full"));
            }
            Err(TrySendError::Disconnected(_)) => {
                return Err(anyhow!("camera actor is not running"));
            }
        }

        match response_receiver.recv_timeout(self.command_timeout) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(anyhow!(error)),
            Err(RecvTimeoutError::Timeout) => Err(anyhow!(
                "camera command exceeded {:.1}s deadline",
                self.command_timeout.as_secs_f64()
            )),
            Err(RecvTimeoutError::Disconnected) => {
                Err(anyhow!("camera actor exited before responding"))
            }
        }
    }
}

impl Drop for ThreadOwnedCamera {
    fn drop(&mut self) {
        let (acknowledge_sender, acknowledge_receiver) = mpsc::sync_channel(1);
        let acknowledged = self
            .sender
            .try_send(CameraCommand::Shutdown(acknowledge_sender))
            .is_ok()
            && acknowledge_receiver
                .recv_timeout(self.command_timeout)
                .is_ok();

        let worker = self.worker.lock().ok().and_then(|mut worker| worker.take());
        if acknowledged {
            if let Some(worker) = worker {
                let _ = worker.join();
            }
        } else {
            // Dropping a JoinHandle detaches a backend call that ignored its deadline,
            // keeping application shutdown bounded.
            drop(worker);
        }
    }
}

fn run_worker<B: CameraBackend>(backend: &mut B, receiver: Receiver<CameraCommand>) {
    let mut faulted = false;
    while let Ok(command) = receiver.recv() {
        match command {
            CameraCommand::Capture(response) => {
                let result = run_operation(&mut faulted, || backend.capture());
                let _ = response.send(result);
            }
            CameraCommand::Preview(response) => {
                let result = run_operation(&mut faulted, || backend.preview());
                let _ = response.send(result);
            }
            CameraCommand::Configure(config, response) => {
                let result = run_operation(&mut faulted, || backend.configure(&config));
                let _ = response.send(result);
            }
            CameraCommand::Shutdown(acknowledge) => {
                let _ = acknowledge.send(());
                break;
            }
        }
    }
}

fn run_operation<T, F>(faulted: &mut bool, operation: F) -> std::result::Result<T, String>
where
    F: FnOnce() -> Result<T>,
{
    if *faulted {
        return Err("camera actor is faulted after a backend panic".to_string());
    }

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error.to_string()),
        Err(payload) => {
            *faulted = true;
            Err(format!(
                "camera backend panicked: {}",
                panic_message(payload)
            ))
        }
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct MockBackend {
        owner: thread::ThreadId,
        dropped: Arc<AtomicBool>,
        preview_delay: Duration,
        panic_on_preview: bool,
    }

    impl Drop for MockBackend {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    impl MockBackend {
        fn assert_owner(&self) {
            assert_eq!(self.owner, thread::current().id());
        }
    }

    impl CameraBackend for MockBackend {
        fn is_available(&self) -> bool {
            true
        }

        fn capture(&mut self) -> Result<PathBuf> {
            self.assert_owner();
            Ok(PathBuf::from("mock.jpg"))
        }

        fn preview(&mut self) -> Result<Vec<u8>> {
            self.assert_owner();
            if self.panic_on_preview {
                panic!("injected backend panic");
            }
            thread::sleep(self.preview_delay);
            Ok(vec![1, 2, 3])
        }

        fn configure(&mut self, _config: &CameraConfig) -> Result<()> {
            self.assert_owner();
            Ok(())
        }
    }

    fn mock_actor(
        timeout: Duration,
        preview_delay: Duration,
        panic_on_preview: bool,
        dropped: Arc<AtomicBool>,
    ) -> ThreadOwnedCamera {
        ThreadOwnedCamera::spawn_backend(
            "trueshot-camera-test".to_string(),
            1,
            timeout,
            Duration::from_secs(1),
            move || {
                Ok(MockBackend {
                    owner: thread::current().id(),
                    dropped,
                    preview_delay,
                    panic_on_preview,
                })
            },
        )
        .unwrap()
    }

    #[test]
    fn backend_stays_on_owner_thread_and_shutdown_drops_it() {
        let dropped = Arc::new(AtomicBool::new(false));
        let actor = mock_actor(
            Duration::from_secs(1),
            Duration::ZERO,
            false,
            dropped.clone(),
        );
        assert_eq!(actor.capture().unwrap(), PathBuf::from("mock.jpg"));
        assert_eq!(actor.preview().unwrap(), vec![1, 2, 3]);
        actor.configure(CameraConfig::default()).unwrap();
        drop(actor);
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[test]
    fn command_timeout_is_bounded() {
        let actor = mock_actor(
            Duration::from_millis(20),
            Duration::from_millis(100),
            false,
            Arc::new(AtomicBool::new(false)),
        );
        let started = std::time::Instant::now();
        let error = actor.preview().unwrap_err().to_string();
        assert!(error.contains("deadline"));
        assert!(started.elapsed() < Duration::from_millis(80));
    }

    #[test]
    fn backend_panic_faults_actor_without_crossing_thread_boundary() {
        let actor = mock_actor(
            Duration::from_secs(1),
            Duration::ZERO,
            true,
            Arc::new(AtomicBool::new(false)),
        );
        let panic_error = actor.preview().unwrap_err().to_string();
        assert!(panic_error.contains("panicked"));
        let fault_error = actor.capture().unwrap_err().to_string();
        assert!(fault_error.contains("faulted"));
    }
}
