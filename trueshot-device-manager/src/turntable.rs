//! Turntable Control for TrueShot
//!
//! Controls hardware turntables for photogrammetry.
//! Supports Foldio360 (via BLE) and generic serial turntables.

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::{Manager, Peripheral};
use futures::stream::StreamExt;
use parking_lot::Mutex;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use uuid::Uuid;

#[async_trait]
pub trait Turntable: Send + Sync {
    async fn connect(&mut self) -> Result<()>;
    async fn disconnect(&mut self) -> Result<()>;
    async fn rotate(&mut self, degrees: f32) -> Result<()>;
    async fn rotate_to(&mut self, angle: f32) -> Result<()>;
    async fn home(&mut self) -> Result<()>;
    async fn set_origin(&mut self) -> Result<()>;
    fn get_rotation(&self) -> f32;
    async fn is_connected(&self) -> bool;
}

#[derive(Debug, Clone)]
pub struct TurntableFeedbackConfig {
    pub query_command: Option<String>,
    pub query_timeout: Duration,
    pub max_angle_error_deg: f32,
    pub auto_correct: bool,
}

impl Default for TurntableFeedbackConfig {
    fn default() -> Self {
        Self {
            query_command: None,
            query_timeout: Duration::from_millis(800),
            max_angle_error_deg: 2.0,
            auto_correct: false,
        }
    }
}

// --- Serial Turntable Implementation ---

pub struct SerialTurntable {
    port_name: String,
    baud_rate: u32,
    // Fix: Mutex makes it Sync, Arc allows cloning if needed (though we use &mut self for ops usually)
    // Actually Turntable trait passes &mut self, so Arc might not be strictly needed for inner if struct is not cloned?
    // But to be safe and allow shared access if needed later.
    // The error was &SerialTurntable not Send -> SerialTurntable not Sync.
    // Box<dyn SerialPort> is Send but !Sync.
    // Mutex<T> is Sync if T is Send. Box<dyn SerialPort> is Send. So Mutex<Box<...>> is Sync.
    port: Arc<Mutex<Option<Box<dyn serialport::SerialPort>>>>,
    current_angle: f32,
    feedback: TurntableFeedbackConfig,
}

impl SerialTurntable {
    pub fn new(port_name: &str, baud_rate: u32) -> Self {
        Self {
            port_name: port_name.to_string(),
            baud_rate,
            port: Arc::new(Mutex::new(None)),
            current_angle: 0.0,
            feedback: TurntableFeedbackConfig::default(),
        }
    }

    pub fn set_feedback_config(&mut self, config: TurntableFeedbackConfig) {
        self.feedback = config;
    }

    async fn query_angle(&self) -> Result<Option<f32>> {
        let cmd = match &self.feedback.query_command {
            Some(cmd) if !cmd.trim().is_empty() => cmd.clone(),
            _ => return Ok(None),
        };

        let port_clone = self.port.clone();
        let timeout = self.feedback.query_timeout;
        tokio::task::spawn_blocking(move || -> Result<Option<f32>> {
            let mut lock = port_clone.lock();
            let port = lock.as_mut().ok_or_else(|| anyhow!("Not connected"))?;

            port.set_timeout(Duration::from_millis(100)).ok();
            port.write_all(cmd.as_bytes())?;
            port.flush()?;

            let start = Instant::now();
            let mut buf: Vec<u8> = Vec::new();
            let mut read_buf = [0u8; 64];

            while start.elapsed() < timeout {
                match port.read(&mut read_buf) {
                    Ok(0) => {}
                    Ok(n) => {
                        buf.extend_from_slice(&read_buf[..n]);
                        if buf.contains(&b'\n') || buf.contains(&b'\r') {
                            break;
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(err) => return Err(err.into()),
                }
            }

            if buf.is_empty() {
                return Ok(None);
            }

            Ok(parse_angle_from_bytes(&buf))
        })
        .await?
    }

    async fn rotate_internal(&mut self, degrees: f32, verify: bool) -> Result<()> {
        self.rotate_raw(degrees).await?;
        let expected = self.current_angle;

        if verify {
            if let Some(measured) = self.query_angle().await? {
                let delta = angle_delta(expected, measured);
                self.current_angle = normalize_angle(measured);
                if delta.abs() > self.feedback.max_angle_error_deg {
                    tracing::warn!(
                        "Turntable angle drift detected: expected {:.2}°, measured {:.2}° (delta {:.2}°)",
                        expected,
                        measured,
                        delta
                    );
                    if self.feedback.auto_correct {
                        let correction = expected - measured;
                        tracing::info!("Applying turntable correction {:.2}°", correction);
                        self.rotate_raw(correction).await?;
                        self.current_angle = expected;
                    }
                }
            }
        }

        Ok(())
    }

    async fn rotate_raw(&mut self, degrees: f32) -> Result<()> {
        let port_clone = self.port.clone();
        let degrees_val = degrees;

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut lock = port_clone.lock();
            if let Some(port) = lock.as_mut() {
                let cmd = format!("R{}\n", degrees_val);
                port.write_all(cmd.as_bytes())?;
                Ok(())
            } else {
                Err(anyhow!("Not connected"))
            }
        })
        .await??;

        tokio::time::sleep(tokio::time::Duration::from_millis(
            (degrees.abs() * 100.0) as u64,
        ))
        .await;
        let expected = normalize_angle(self.current_angle + degrees);
        self.current_angle = expected;
        Ok(())
    }
}

unsafe impl Send for SerialTurntable {}
unsafe impl Sync for SerialTurntable {}

#[async_trait]
impl Turntable for SerialTurntable {
    async fn connect(&mut self) -> Result<()> {
        let port_name = self.port_name.clone();
        let baud_rate = self.baud_rate;

        // Blocking open, maybe should be spawn_blocking but this is init
        let port = serialport::new(&port_name, baud_rate)
            .timeout(Duration::from_millis(1000))
            .open()
            .with_context(|| format!("Failed to open serial port {}", port_name))?;

        *self.port.lock() = Some(port);
        tracing::info!("Connected to serial turntable on {}", self.port_name);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        *self.port.lock() = None;
        Ok(())
    }

    async fn rotate(&mut self, degrees: f32) -> Result<()> {
        self.rotate_internal(degrees, true).await
    }

    async fn rotate_to(&mut self, angle: f32) -> Result<()> {
        let diff = angle - self.current_angle;
        self.rotate(diff).await
    }

    async fn home(&mut self) -> Result<()> {
        let port_clone = self.port.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut lock = port_clone.lock();
            if let Some(port) = lock.as_mut() {
                port.write_all(b"HOME\n")?;
                Ok(())
            } else {
                Err(anyhow!("Not connected"))
            }
        })
        .await??;

        self.current_angle = 0.0;
        Ok(())
    }

    async fn set_origin(&mut self) -> Result<()> {
        self.current_angle = 0.0;
        Ok(())
    }

    fn get_rotation(&self) -> f32 {
        self.current_angle
    }

    async fn is_connected(&self) -> bool {
        self.port.lock().is_some()
    }
}

// --- Foldio360 (Bluetooth) Implementation ---

pub struct Foldio360 {
    peripheral: Option<Peripheral>,
    current_angle: Arc<Mutex<f32>>,
    rotation_complete: Arc<Notify>,
    last_feedback_angle: Arc<Mutex<Option<f32>>>,
    feedback: TurntableFeedbackConfig,
}

impl Default for Foldio360 {
    fn default() -> Self {
        Self::new()
    }
}

impl Foldio360 {
    pub fn new() -> Self {
        Self {
            peripheral: None,
            current_angle: Arc::new(Mutex::new(0.0)),
            rotation_complete: Arc::new(Notify::new()),
            last_feedback_angle: Arc::new(Mutex::new(None)),
            feedback: TurntableFeedbackConfig::default(),
        }
    }

    pub fn set_feedback_config(&mut self, config: TurntableFeedbackConfig) {
        self.feedback = config;
    }
}

#[async_trait]
impl Turntable for Foldio360 {
    async fn connect(&mut self) -> Result<()> {
        tracing::info!("Scanning for Foldio360...");
        tracing::info!("DEBUG: Turntable Connect Step 1: Manager::new");
        let manager = Manager::new().await?;
        tracing::info!("DEBUG: Turntable Connect Step 2: Adapters");
        let adapters = manager.adapters().await?;
        tracing::info!("DEBUG: Turntable Connect Step 3: Select Adapter");
        let adapter = match adapters.into_iter().next() {
            Some(a) => a,
            None => {
                tracing::error!("Foldio360: No Bluetooth adapter found during scan.");
                return Err(anyhow!("No Bluetooth adapter found"));
            }
        };

        tracing::info!("DEBUG: Turntable Connect Step 4: Start Scan");
        if let Err(e) = adapter.start_scan(ScanFilter::default()).await {
            tracing::error!("Foldio360: Failed to start scan: {}", e);
            return Err(e.into());
        }
        tracing::info!("DEBUG: Turntable Connect Step 5: Waiting 10s...");
        tokio::time::sleep(Duration::from_secs(10)).await;
        tracing::info!("DEBUG: Turntable Connect Step 6: Get Peripherals");
        let peripherals = adapter.peripherals().await?;
        let mut target = None;

        let nordic_uart_service = Uuid::parse_str("6e400001-b5a3-f393-e0a9-e50e24dcca9e").unwrap();

        for p in peripherals {
            if let Ok(Some(props)) = p.properties().await {
                let name = props
                    .local_name
                    .clone()
                    .unwrap_or_else(|| "Unknown".to_string());
                let has_nordic_uart = props.services.contains(&nordic_uart_service);

                tracing::info!(
                    "Discovered device: {} (Services: {:?})",
                    name,
                    props.services
                );

                if name.to_lowercase().contains("foldio")
                    || name.to_lowercase().contains("360")
                    || has_nordic_uart
                {
                    target = Some(p);
                    tracing::info!("Foldio360 matched: {}", name);
                    break;
                }
            }
        }

        let target = target.ok_or_else(|| {
            tracing::warn!("Foldio360: Device not found in scan results.");
            anyhow!("Foldio360 not found")
        })?;

        target.connect().await?;
        target.discover_services().await?;

        // Subscribe to notifications (using UUIDs from example)
        let notify_uuid = Uuid::parse_str("6e400003-b5a3-f393-e0a9-e50e24dcca9e").unwrap();
        let chars = target.characteristics();
        let notify_char = chars
            .iter()
            .find(|c| c.uuid == notify_uuid)
            .ok_or_else(|| anyhow!("Notify char not found"))?;

        target.subscribe(notify_char).await?;

        let rotation_complete = self.rotation_complete.clone();
        let current_angle = self.current_angle.clone();
        let last_feedback_angle = self.last_feedback_angle.clone();
        let mut notification_stream = target.notifications().await?;

        tokio::spawn(async move {
            while let Some(data) = notification_stream.next().await {
                tracing::debug!(
                    "Received notification: {:?}",
                    String::from_utf8_lossy(&data.value)
                );
                if let Some(angle) = parse_angle_from_bytes(&data.value) {
                    *current_angle.lock() = normalize_angle(angle);
                    *last_feedback_angle.lock() = Some(normalize_angle(angle));
                }
                if data.value == b"OK" {
                    rotation_complete.notify_one();
                }
            }
        });

        self.peripheral = Some(target);
        tracing::info!("Connected to Foldio360");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        if let Some(p) = &self.peripheral {
            p.disconnect().await?;
        }
        self.peripheral = None;
        Ok(())
    }

    async fn rotate(&mut self, degrees: f32) -> Result<()> {
        let p = self
            .peripheral
            .as_ref()
            .ok_or_else(|| anyhow!("Not connected"))?;
        *self.last_feedback_angle.lock() = None;

        // Command format: "rotate(direction, degrees, speed, 1)"
        let dir = if degrees >= 0.0 { "CW" } else { "CCW" };
        let cmd = format!("rotate({},{},3,1)\r\n", dir, degrees.abs() as u32);

        let write_uuid = Uuid::parse_str("6e400002-b5a3-f393-e0a9-e50e24dcca9e").unwrap();
        let chars = p.characteristics();
        let write_char = chars
            .iter()
            .find(|c| c.uuid == write_uuid)
            .ok_or_else(|| anyhow!("Write char not found"))?;

        tracing::debug!("Sending command: {}", cmd.trim());
        // Use WithoutResponse for lower latency, fallback if needed is handled by device logic
        p.write(write_char, cmd.as_bytes(), WriteType::WithoutResponse)
            .await?;

        // Wait for notification with timeout to prevent hanging the system
        let timeout_duration = Duration::from_secs(5);
        match tokio::time::timeout(timeout_duration, self.rotation_complete.notified()).await {
            Ok(_) => {}
            Err(_) => {
                tracing::warn!("Foldio360 rotation acknowledgment timed out");
                // We assume it moved anyway to unblock UI
            }
        }

        let expected = normalize_angle(*self.current_angle.lock() + degrees);
        *self.current_angle.lock() = expected;

        let measured_opt = *self.last_feedback_angle.lock();
        if let Some(measured) = measured_opt {
            let delta = angle_delta(expected, measured);
            *self.current_angle.lock() = normalize_angle(measured);
            if delta.abs() > self.feedback.max_angle_error_deg {
                tracing::warn!(
                    "Foldio360 drift detected: expected {:.2}°, measured {:.2}° (delta {:.2}°)",
                    expected,
                    measured,
                    delta
                );
                if self.feedback.auto_correct {
                    let correction = expected - measured;
                    tracing::info!("Applying Foldio360 correction {:.2}°", correction);
                    self.rotate_raw(correction).await?;
                    *self.current_angle.lock() = expected;
                }
            }
        }
        Ok(())
    }

    async fn rotate_to(&mut self, angle: f32) -> Result<()> {
        let diff = angle - *self.current_angle.lock();
        self.rotate(diff).await
    }

    async fn home(&mut self) -> Result<()> {
        self.rotate_to(0.0).await
    }

    async fn set_origin(&mut self) -> Result<()> {
        *self.current_angle.lock() = 0.0;
        Ok(())
    }

    fn get_rotation(&self) -> f32 {
        *self.current_angle.lock()
    }

    async fn is_connected(&self) -> bool {
        if let Some(p) = &self.peripheral {
            p.is_connected().await.unwrap_or_default()
        } else {
            false
        }
    }
}

impl Foldio360 {
    async fn rotate_raw(&mut self, degrees: f32) -> Result<()> {
        let p = self
            .peripheral
            .as_ref()
            .ok_or_else(|| anyhow!("Not connected"))?;
        let dir = if degrees >= 0.0 { "CW" } else { "CCW" };
        let cmd = format!("rotate({},{},3,1)\r\n", dir, degrees.abs() as u32);
        let write_uuid = Uuid::parse_str("6e400002-b5a3-f393-e0a9-e50e24dcca9e").unwrap();
        let chars = p.characteristics();
        let write_char = chars
            .iter()
            .find(|c| c.uuid == write_uuid)
            .ok_or_else(|| anyhow!("Write char not found"))?;
        p.write(write_char, cmd.as_bytes(), WriteType::WithoutResponse)
            .await?;
        tokio::time::sleep(Duration::from_millis((degrees.abs() * 100.0) as u64)).await;
        let expected = normalize_angle(*self.current_angle.lock() + degrees);
        *self.current_angle.lock() = expected;
        Ok(())
    }
}

fn parse_angle_from_bytes(bytes: &[u8]) -> Option<f32> {
    let text = String::from_utf8_lossy(bytes);
    for token in text.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')) {
        if token.is_empty() {
            continue;
        }
        if let Ok(val) = token.parse::<f32>() {
            if val.is_finite() {
                return Some(val);
            }
        }
    }
    None
}

fn normalize_angle(angle: f32) -> f32 {
    let mut a = angle % 360.0;
    if a < 0.0 {
        a += 360.0;
    }
    a
}

fn angle_delta(expected: f32, measured: f32) -> f32 {
    let mut delta = (measured - expected + 180.0) % 360.0;
    if delta < 0.0 {
        delta += 360.0;
    }
    delta - 180.0
}

// --- Mock Implementation ---

pub struct MockTurntable {
    angle: f32,
    connected: bool,
}

impl Default for MockTurntable {
    fn default() -> Self {
        Self::new()
    }
}

impl MockTurntable {
    pub fn new() -> Self {
        Self {
            angle: 0.0,
            connected: false,
        }
    }
}

#[async_trait]
impl Turntable for MockTurntable {
    async fn connect(&mut self) -> Result<()> {
        self.connected = true;
        tracing::info!("MockTurntable connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        Ok(())
    }

    async fn rotate(&mut self, degrees: f32) -> Result<()> {
        self.angle = (self.angle + degrees) % 360.0;
        tracing::info!(
            "MockTurntable rotated by {}. Now at {}",
            degrees,
            self.angle
        );
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        Ok(())
    }

    async fn rotate_to(&mut self, angle: f32) -> Result<()> {
        let diff = angle - self.angle;
        self.rotate(diff).await
    }

    async fn home(&mut self) -> Result<()> {
        self.rotate_to(0.0).await
    }

    async fn set_origin(&mut self) -> Result<()> {
        self.angle = 0.0;
        tracing::info!("MockTurntable origin set");
        Ok(())
    }

    fn get_rotation(&self) -> f32 {
        self.angle
    }

    async fn is_connected(&self) -> bool {
        self.connected
    }
}
