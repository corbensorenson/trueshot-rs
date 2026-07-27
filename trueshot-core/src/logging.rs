//! Real-time file logging for TrueShot pipeline using tracing
//!
//! This module provides comprehensive logging that writes immediately to files
//! so that logs are preserved even if the process crashes.

use crate::events::{EventBus, LogLevel, SystemEvent};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*};
use tracing_subscriber::{layer::Context as LayerContext, registry::LookupSpan, Layer};

// Global guard to keep file writer alive - using OnceLock for safety
static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

pub struct EventBusLayer {
    bus: Arc<EventBus>,
}

impl EventBusLayer {
    pub fn new(bus: Arc<EventBus>) -> Self {
        Self { bus }
    }
}

impl<S> Layer<S> for EventBusLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: LayerContext<'_, S>) {
        let level = match *event.metadata().level() {
            tracing::Level::ERROR => LogLevel::Error,
            tracing::Level::WARN => LogLevel::Warning,
            tracing::Level::INFO => LogLevel::Info,
            _ => return, // Skip debug/trace for bus
        };

        // Avoid infinite loop if EventBus logs itself
        if event.metadata().target().contains("events") {
            return;
        }

        // We can't access event fields easily in tracing without a visitor.
        // For now, we publish a generic message with the module target.
        let msg = format!("[{}] Log event occurred", event.metadata().target());
        self.bus.publish(SystemEvent::SystemMessage(msg, level));
    }
}

/// Initialize TrueShot logging with default settings
pub fn init_default_logging() -> Result<PathBuf> {
    let log_dir = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("logs");

    init_logging(&log_dir)
}

/// Initialize TrueShot logging with custom directory
pub fn init_logging<P: AsRef<Path>>(log_dir: P) -> Result<PathBuf> {
    let log_dir = log_dir.as_ref();
    std::fs::create_dir_all(log_dir)
        .with_context(|| format!("Failed to create log directory: {}", log_dir.display()))?;

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let filename = format!("trueshot_{}.log", timestamp);
    let log_path = log_dir.join(&filename);

    let file = std::fs::File::create(&log_path)
        .with_context(|| format!("Failed to create log file: {}", log_path.display()))?;

    let (non_blocking, guard) = tracing_appender::non_blocking(file);

    // Store guard globally to keep worker thread alive
    let _ = LOG_GUARD.set(guard);

    // Configure subscriber
    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(false)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true);

    let stderr_layer = fmt::layer().with_writer(std::io::stderr).with_target(false);

    // Initialize tracing
    // We use try_init() to handle cases where it might be called multiple times in tests
    // or if another crate initiated logging.
    if tracing_subscriber::registry()
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
        .is_ok()
    {
        // Redirect log crate events to tracing
        let _ = tracing_log::LogTracer::init();

        tracing::info!("=== TRUESHOT LOGGING INITIALIZED ===");
        tracing::info!("Log file: {}", log_path.display());
    } else {
        eprintln!(
            "Tracing subscriber already initialized, new log file might not capture everything."
        );
    }

    Ok(log_path)
}

/// Read the latest log file content
pub fn read_latest_log<P: AsRef<Path>>(log_dir: P) -> Result<String> {
    let log_dir = log_dir.as_ref();

    // Find the most recent log file
    let mut log_files: Vec<_> = std::fs::read_dir(log_dir)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension()? == "log" && path.file_name()?.to_str()?.starts_with("trueshot_") {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    log_files.sort_by(|a, b| {
        std::fs::metadata(b)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            .cmp(
                &std::fs::metadata(a)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            )
    });

    if let Some(latest_log) = log_files.first() {
        std::fs::read_to_string(latest_log)
            .with_context(|| format!("Failed to read log file: {}", latest_log.display()))
    } else {
        Ok("No log files found".to_string())
    }
}
