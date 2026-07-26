use crate::events::{EventBus, LogLevel, SystemEvent};
use crate::project::ScanProject;
use crate::scanning::session::ScanSession;
use crate::scanning::workflow::{ScanAction, ScanWorkflow}; // Import Workflow types
use crate::scanning::{QualityLevel, SmartScanStrategy};
use crate::vision::change_detection::SceneChangeDetector;
use anyhow::Result;
use image::{ImageBuffer, Rgb};
use std::sync::Arc;
use tokio::sync::Mutex;

// Hardware traits
use crate::scheduler::Scheduler;
use trueshot_device_manager::{CameraManager, Turntable};

// New Task System
use crate::scanning::tasks::hardware::{
    CheckCenteringTask, CheckExposureTask, HomeTurntableTask, VerifyHardwareTask,
};
use crate::scanning::tasks::workflow::{
    BackgroundScanTask, CalibrateTask, PromptUserTask, SmartScanTask, StartProcessingTask,
};
use crate::scanning::tasks::{DirectorContext, ScanTask};

#[derive(Debug, PartialEq, Clone)]
pub enum DirectorState {
    Idle,
    ExecutingStep(usize),   // Index of current step
    WaitingForUser(String), // Paused for user interaction (was WaitingForFlip/Stabilization)
    Error(String),
}

pub struct Director {
    state: Arc<Mutex<DirectorState>>,
    detector: Arc<Mutex<SceneChangeDetector>>,
    project: Arc<Mutex<Option<ScanProject>>>,
    bus: Arc<EventBus>,

    // Hardware
    cameras: Arc<Mutex<CameraManager>>,
    turntable: Arc<Mutex<Box<dyn Turntable>>>,

    // Logic
    strategy: Arc<Mutex<SmartScanStrategy>>,
    workflow: Arc<Mutex<Option<ScanWorkflow>>>, // Current Workflow
    session: Arc<Mutex<Option<ScanSession>>>,
    current_step: Arc<Mutex<usize>>,
    current_task: Arc<Mutex<Option<Box<dyn ScanTask>>>>, // Active Task
    scheduler: Option<Arc<Scheduler>>,

    // Safety
    background_reference: Arc<Mutex<Option<ImageBuffer<Rgb<u8>, Vec<u8>>>>>,
}

impl Director {
    pub fn new(
        bus: Arc<EventBus>,
        cameras: Arc<Mutex<CameraManager>>,
        turntable: Box<dyn Turntable>,
        scheduler: Option<Arc<Scheduler>>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(DirectorState::Idle)),
            detector: Arc::new(Mutex::new(SceneChangeDetector::new())),
            project: Arc::new(Mutex::new(None)),
            bus,
            cameras,
            turntable: Arc::new(Mutex::new(turntable)),
            strategy: Arc::new(Mutex::new(SmartScanStrategy::new(QualityLevel::Standard))),
            workflow: Arc::new(Mutex::new(None)),
            session: Arc::new(Mutex::new(None)),
            current_step: Arc::new(Mutex::new(0)),
            current_task: Arc::new(Mutex::new(None)),
            scheduler,
            background_reference: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a context object sharing the director's resources
    pub fn context(&self) -> DirectorContext {
        DirectorContext {
            state: self.state.clone(),
            detector: self.detector.clone(),
            project: self.project.clone(),
            bus: self.bus.clone(),
            cameras: self.cameras.clone(),
            turntable: self.turntable.clone(),
            strategy: self.strategy.clone(),
            workflow: self.workflow.clone(),
            session: self.session.clone(),
            current_step: self.current_step.clone(),
            scheduler: self.scheduler.clone(),
            background_reference: self.background_reference.clone(),
        }
    }

    pub async fn set_project(&self, project: ScanProject) {
        let mut p = self.project.lock().await;
        *p = Some(project);

        // Start Standard Workflow by default when project is loaded
        self.start_workflow(ScanWorkflow::standard()).await;
    }

    pub async fn start_workflow(&self, wf: ScanWorkflow) {
        let mut w = self.workflow.lock().await;
        *w = Some(wf);

        let mut s = self.session.lock().await;
        *s = Some(ScanSession::new());

        let mut step = self.current_step.lock().await;
        *step = 0;

        // Kickoff first step
        self.execute_step(0).await;
    }

    fn execute_step<'a>(
        &'a self,
        step_idx: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let wf_guard = self.workflow.lock().await;

            // 1. Check if workflow is complete
            if let Some(wf) = wf_guard.as_ref() {
                if step_idx >= wf.steps.len() {
                    let mut state = self.state.lock().await;
                    *state = DirectorState::Idle;
                    self.bus.publish(SystemEvent::SystemMessage(
                        "Workflow Complete".into(),
                        LogLevel::Success,
                    ));
                    self.bus.publish(SystemEvent::ScanComplete);

                    let mut task_guard = self.current_task.lock().await;
                    *task_guard = None;
                    return;
                }

                let step = &wf.steps[step_idx];

                // 2. Set Status
                {
                    let mut state = self.state.lock().await;
                    *state = DirectorState::ExecutingStep(step_idx);
                }

                // 3. Create Task Factory
                let task: Box<dyn ScanTask> = match step {
                    ScanAction::HomeTurntable => Box::new(HomeTurntableTask),
                    ScanAction::VerifyHardware => Box::new(VerifyHardwareTask),
                    ScanAction::CheckExposure => Box::new(CheckExposureTask),
                    ScanAction::CheckCentering => Box::new(CheckCenteringTask),
                    ScanAction::Calibrate { quality } => Box::new(CalibrateTask { quality: *quality }),
                    ScanAction::PromptUser { message } => Box::new(PromptUserTask { message: message.clone() }),
                    ScanAction::WaitForSDCard => Box::new(PromptUserTask { 
                        message: "Please insert SD Cards into Computer (if applicable) and click Continue.".into() 
                    }),
                    ScanAction::SmartScan { quality, capture } => Box::new(SmartScanTask { 
                        quality: *quality, 
                        capture: capture.clone(),
                        step_idx 
                    }),
                    ScanAction::StartProcessing => Box::new(StartProcessingTask),
                    ScanAction::CaptureBackground => Box::new(BackgroundScanTask { step_idx }),
                };

                drop(wf_guard); // Release workflow lock early

                // 4. Install Task and Execute `on_enter`
                {
                    let mut task_guard = self.current_task.lock().await;
                    *task_guard = Some(task);
                }

                // 5. Run Entry Logic
                // We re-acquire lock to run on_enter.
                // Note: We deliberately hold the task lock during on_enter if we want to block frame processing?
                // OR we get the task, run it, then if it says "done", we advance?
                // Issue: If we hold `current_task` lock, `process_frame` blocks.
                // If `on_enter` takes 10s (HomeTurntable code is blocking), `process_frame` receives no input?
                // This mimics original behavior where `execute_step` was blocking.

                let ctx = self.context();
                let task_complete = {
                    let task_guard = self.current_task.lock().await;
                    if let Some(t) = task_guard.as_ref() {
                        match t.on_enter(&ctx).await {
                            Ok(done) => done,
                            Err(e) => {
                                // Handle Error
                                self.bus.publish(SystemEvent::SystemMessage(
                                    format!("Step Error: {}", e),
                                    LogLevel::Error,
                                ));
                                let mut state = self.state.lock().await;
                                *state = DirectorState::Error(e.to_string());
                                return;
                            }
                        }
                    } else {
                        false // Should not happen
                    }
                };

                if task_complete {
                    self.advance_step().await;
                }
                // Else: Task remains active. `process_frame` will drive it via `on_frame`.
            }
        })
    }

    async fn advance_step(&self) {
        let mut step = self.current_step.lock().await;
        *step += 1;
        self.execute_step(*step).await;
    }

    /// Convenience method to manually trigger scanning (used by tests/CLI)
    pub async fn start_scan(&self) -> Result<()> {
        self.start_workflow(ScanWorkflow::standard()).await;
        Ok(())
    }

    /// Abort the current scan immediately
    pub async fn stop_scan(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        *state = DirectorState::Idle;
        let mut w = self.workflow.lock().await;
        *w = None;
        let mut task = self.current_task.lock().await;
        *task = None;

        // Safety Home
        let mut tt = self.turntable.lock().await;
        let _ = tt.home().await;

        self.bus.publish(SystemEvent::SystemMessage(
            "Scan Aborted by User".into(),
            LogLevel::Warning,
        ));
        Ok(())
    }

    /// Load project helper for tests and offline workflows
    pub async fn load_project(&self, project: ScanProject) -> Result<()> {
        self.set_project(project).await;
        Ok(())
    }

    /// Main loop called every frame by the camera system
    pub async fn process_frame(&self, frame: &ImageBuffer<Rgb<u8>, Vec<u8>>) {
        let state_guard = self.state.lock().await;

        // Fast exit checks
        if let DirectorState::Idle | DirectorState::Error(_) = *state_guard {
            return;
        }

        // Release state guard to avoid holding it during task execution
        drop(state_guard);

        // Delegate to Task
        let task_guard = self.current_task.lock().await;

        if let Some(task) = task_guard.as_ref() {
            let ctx = self.context();
            match task.on_frame(&ctx, frame).await {
                Ok(true) => {
                    // Task signalled completion
                    drop(task_guard); // Must drop before advancing
                    self.advance_step().await;
                }
                Ok(false) => {
                    // Continue
                }
                Err(e) => {
                    // Task Error
                    drop(task_guard); // Release task
                    let mut state = self.state.lock().await;
                    *state = DirectorState::Error(e.to_string());
                    self.bus.publish(SystemEvent::SystemMessage(
                        format!("Task Error: {}", e),
                        LogLevel::Error,
                    ));
                }
            }
        }
    }
}
