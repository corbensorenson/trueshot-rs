use std::sync::Arc;
use tokio::sync::Mutex;
use anyhow::Result;

use crate::project::ScanProject;
use crate::events::EventBus;
use crate::scanning::SmartScanStrategy;
use crate::scanning::workflow::ScanWorkflow;
use crate::scanning::session::ScanSession;
use crate::scheduler::Scheduler;
use crate::vision::change_detection::SceneChangeDetector;
use image::{ImageBuffer, Rgb};

use trueshot_device_manager::{CameraManager, Turntable};

pub mod hardware;
pub mod workflow;

/// Shared context passed to every ScanTask
pub struct DirectorContext {
    pub state: Arc<Mutex<crate::director::DirectorState>>,
    pub detector: Arc<Mutex<SceneChangeDetector>>,
    pub project: Arc<Mutex<Option<ScanProject>>>,
    pub bus: Arc<EventBus>,
    
    // Hardware
    pub cameras: Arc<Mutex<CameraManager>>,
    pub turntable: Arc<Mutex<Box<dyn Turntable>>>,
    
    // Logic
    pub strategy: Arc<Mutex<SmartScanStrategy>>,
    pub workflow: Arc<Mutex<Option<ScanWorkflow>>>, // Current Workflow
    pub session: Arc<Mutex<Option<ScanSession>>>,
    pub current_step: Arc<Mutex<usize>>,
    pub scheduler: Option<Arc<Scheduler>>, // Shared, can be optional
    
    // Safety
    pub background_reference: Arc<Mutex<Option<ImageBuffer<Rgb<u8>, Vec<u8>>>>>,
}

#[async_trait::async_trait]
pub trait ScanTask: Send + Sync {
    /// Execute the primary logic of the task (called when entering the step)
    /// Returns true if the task is complete and we should advance immediately.
    /// Returns false if the task is asynchronous/long-running or waits for per-frame updates.
    async fn on_enter(&self, ctx: &DirectorContext) -> Result<bool>;
    
    /// Called manually by per-frame loop if the task needs it.
    /// Returns true if the step is complete and we should advance.
    async fn on_frame(&self, _ctx: &DirectorContext, _frame: &ImageBuffer<Rgb<u8>, Vec<u8>>) -> Result<bool> {
        Ok(false)
    }
    
    /// Name for logging
    fn name(&self) -> &'static str;
}
