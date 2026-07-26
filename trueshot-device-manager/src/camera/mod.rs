use anyhow::Result;
use std::path::PathBuf;

pub mod registry;
pub mod insta360;
pub mod webcam;
pub mod kinect;

#[cfg(feature = "gphoto2")]
pub mod gphoto;

pub use registry::{
    CameraRegistry,
    CameraCapabilities,
    CalibrationData,
    ColorCalibrationData,
    CameraSettings,
    CameraRole,
};

pub use insta360::Insta360Link;
pub use webcam::GenericWebcam;
pub use insta360::Gimbal;
pub use kinect::{KinectCamera, KinectLedColor, KinectStreamType, DepthCamera};

#[derive(Debug, Clone)]
pub struct CameraConfig {
    pub iso: Option<String>,
    pub shutter_speed: Option<String>,
    pub aperture: Option<String>,
    pub wb: Option<String>,
    pub capture_target: Option<String>, // "Internal RAM" vs "Memory Card"
    pub resolution: Option<(u32, u32)>,
    pub fps: Option<u32>,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            iso: None,
            shutter_speed: None,
            aperture: None,
            wb: None,
            capture_target: None,
            resolution: None,
            fps: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReconciliationReport {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

pub trait Camera: Send + Sync {
    fn id(&self) -> String;
    fn capture(&self, config: &CameraConfig) -> Result<PathBuf>;
    fn capture_preview(&self) -> Result<Vec<u8>>;
    fn set_config(&self, config: &CameraConfig) -> Result<()>;
    fn battery_level(&self) -> Result<u8>;
    
    // PTZ Control
    fn ptz(&self, _pan: f32, _tilt: f32, _zoom: f32) -> Result<()> {
        // Default no-op
        Ok(())
    }

    // Focus Control
    fn set_focus_point(&self, _x: f32, _y: f32) -> Result<()> {
         // Default no-op
         Ok(())
    }
    
    fn trigger_autofocus(&self) -> Result<()> {
        // Default no-op
        Ok(())
    }

    // Manual Lens Drive (Focus stepping)
    fn drive_focus(&self, _step: i32) -> Result<()> {
         // Default no-op
         Ok(())
    }

    // Depth Camera Methods
    fn capture_depth(&self) -> Result<Vec<u16>> {
        Err(anyhow::anyhow!("Depth capture not supported"))
    }
    
    fn capture_infrared(&self) -> Result<Vec<u8>> {
        Err(anyhow::anyhow!("IR capture not supported"))
    }
    
    // Motor/Tilt Control
    fn set_tilt(&self, _degrees: i8) -> Result<()> {
        Ok(()) // Default no-op
    }
    
    fn get_tilt(&self) -> Result<i8> {
        Ok(0) // Default level
    }
    
    // LED Control
    fn set_led(&self, _color: KinectLedColor) -> Result<()> {
        Ok(())
    }
    
    // Accelerometer
    fn get_accelerometer(&self) -> Result<(f64, f64, f64)> {
        Ok((0.0, 0.0, 1.0)) // Default: gravity pointing down
    }
    
    // Downcasting helpers
    fn as_gimbal(&self) -> Option<&dyn Gimbal> { None }
    fn as_depth_camera(&self) -> Option<&dyn DepthCamera> { None }
    // Removed as_gimbal_mut as we are now using shared ownership (&self)
}

// Lazy Camera Wrapper
pub struct LazyNokhwaCamera {
    index: nokhwa::utils::CameraIndex,
    name: String,
    is_insta: bool,
    enabled: bool,
    inner: std::sync::Arc<std::sync::Mutex<Option<Box<dyn Camera>>>>,
}

impl LazyNokhwaCamera {
    pub fn new(index: nokhwa::utils::CameraIndex, name: String, is_insta: bool, enabled: bool) -> Self {
        Self { 
            index, 
            name, 
            is_insta, 
            enabled,
            inner: std::sync::Arc::new(std::sync::Mutex::new(None)) 
        }
    }
    
    fn ensure_init(&self) -> Result<()> {
        if !self.enabled {
            return Err(anyhow::anyhow!("Camera is disabled"));
        }

        let mut inner_guard = self.inner.lock().map_err(|e| anyhow::anyhow!("Lock fail: {}", e))?;
        if inner_guard.is_some() { return Ok(()); }
        
        let idx = self.index.clone();
        let is_insta = self.is_insta;
        let name = self.name.clone();
        
        tracing::info!("Lazy initializing camera: {}", name);
        
        let idx_u32 = if let nokhwa::utils::CameraIndex::Index(i) = idx { i } else { 0 };
        let result = std::panic::catch_unwind(|| {
            if is_insta {
                 Insta360Link::new(idx_u32).ok().map(|c| Box::new(c) as Box<dyn Camera>)
            } else {
                 GenericWebcam::new(idx_u32, &name).ok().map(|c| Box::new(c) as Box<dyn Camera>)
            }
        });
        
        match result {
             Ok(Some(c)) => {
                 *inner_guard = Some(c);
                 Ok(())
             },
             Ok(None) => Err(anyhow::anyhow!("Failed to initialize camera (Open failed)")),
             Err(_) => {
                 tracing::error!("Panic during lazy init of {}", name);
                 Err(anyhow::anyhow!("Camera initialization crashed"))
             }
        }
    }
}

// Camera is Sync + Send
unsafe impl Send for LazyNokhwaCamera {}
unsafe impl Sync for LazyNokhwaCamera {}

impl Camera for LazyNokhwaCamera {
    fn id(&self) -> String {
        if let nokhwa::utils::CameraIndex::Index(idx) = self.index {
             if self.is_insta { format!("Insta360_{}", idx) } else { format!("Webcam_{}", idx) }
        } else {
             format!("Unknown_{}", self.name)
        }
    }
    
    fn capture(&self, config: &CameraConfig) -> Result<PathBuf> {
        self.ensure_init()?;
        let inner_guard = self.inner.lock().map_err(|e| anyhow::anyhow!("Lock fail: {}", e))?;
        if let Some(inner) = &*inner_guard {
            inner.capture(config)
        } else {
             Err(anyhow::anyhow!("Camera not active"))
        }
    }
    
    fn capture_preview(&self) -> Result<Vec<u8>> {
        self.ensure_init()?;
        let inner_guard = self.inner.lock().map_err(|e| anyhow::anyhow!("Lock fail: {}", e))?;
        if let Some(inner) = &*inner_guard {
            inner.capture_preview()
        } else {
             Err(anyhow::anyhow!("Camera not active"))
        }
    }

    fn set_config(&self, config: &CameraConfig) -> Result<()> {
         self.ensure_init()?;
         let mut inner_guard = self.inner.lock().map_err(|e| anyhow::anyhow!("Lock fail: {}", e))?;
         if let Some(inner) = &mut *inner_guard {
             inner.set_config(config)
         } else {
             Ok(())
         }
     }
 
     fn battery_level(&self) -> Result<u8> {
         Ok(100)
     }

    // Pass through other methods...
    fn ptz(&self, pan: f32, tilt: f32, zoom: f32) -> Result<()> {
        self.ensure_init()?;
        let mut inner_guard = self.inner.lock().map_err(|e| anyhow::anyhow!("Lock fail: {}", e))?;
        if let Some(inner) = &mut *inner_guard {
            inner.ptz(pan, tilt, zoom)
        } else {
             Err(anyhow::anyhow!("Camera not active"))
        }
    }
    
    fn set_focus_point(&self, x: f32, y: f32) -> Result<()> {
        self.ensure_init()?;
        let mut inner_guard = self.inner.lock().map_err(|e| anyhow::anyhow!("Lock fail: {}", e))?;
        if let Some(inner) = &mut *inner_guard {
            inner.set_focus_point(x, y)
        } else {
            Ok(())
        }
    }
    
    fn drive_focus(&self, step: i32) -> Result<()> {
        self.ensure_init()?;
        let mut inner_guard = self.inner.lock().map_err(|e| anyhow::anyhow!("Lock fail: {}", e))?;
        if let Some(inner) = &mut *inner_guard {
            inner.drive_focus(step)
        } else {
             Ok(())
        }
    }
}

pub struct CameraManager {
    pub cameras: Vec<std::sync::Arc<dyn Camera>>,
    pub registry: CameraRegistry,
}

impl CameraManager {
    pub fn new() -> Self {
        Self { 
            cameras: Vec::new(),
            registry: CameraRegistry::new(),
        }
    }

    pub async fn reconcile_cameras(&mut self, _include_mock: bool) -> Result<ReconciliationReport> {
        // Internal enum to unify discovery
        enum Discovered {
            #[cfg(feature = "gphoto2")]
            GPhoto(gphoto::GPhotoCamera),
            Nokhwa { id: String, index: nokhwa::utils::CameraIndex, name: String, is_insta: bool },
            Kinect { id: String, index: u32 },
            #[cfg(feature = "gphoto2")]
            KeepAlive(String), // ID to keep
        }
        
        impl Discovered {
            fn id(&self) -> String {
                match self {
                    #[cfg(feature = "gphoto2")]
                    Discovered::GPhoto(c) => c.id(),
                    Discovered::Nokhwa { id, .. } => id.clone(),
                    Discovered::Kinect { id, .. } => id.clone(),
                    #[cfg(feature = "gphoto2")]
                    Discovered::KeepAlive(id) => id.clone(),
                }
            }
        }

        // 0. Check for existing generic GPhoto camera to prevent USB contention (Flapping)
        #[cfg(feature = "gphoto2")]
        let gphoto_active = {
            let mut active = false;
            for cam in &self.cameras {
                if cam.id().contains("GPhoto") {
                    // Ping it
                    if cam.battery_level().is_ok() {
                        active = true;
                    }
                    break;
                }
            }
            active
        };

        // 1. Discovery (Blocking)
        let discovered_list = tokio::task::spawn_blocking(move || {
            
            let mut list = Vec::new();

            #[cfg(feature = "gphoto2")]
            if gphoto_active {
                 tracing::info!("GPhoto camera active, skipping scan to prevent flapping.");
                 list.push(Discovered::KeepAlive("GPhoto_DSLR_1".to_string()));
            } else {
                match gphoto::GPhotoCamera::detect_all() {
                    Ok(cams) => {
                        tracing::info!("GPhoto discovery found {} cameras", cams.len());
                        for c in cams {
                            list.push(Discovered::GPhoto(c));
                        }
                    }
                    Err(e) => tracing::error!("GPhoto discovery CRITICAL failure: {}", e),
                }
            }

            let query_result = std::panic::catch_unwind(|| {
                nokhwa::query(nokhwa::utils::ApiBackend::Auto)
            });

            match query_result {
                Ok(Ok(cameras)) => {
                    for cam_info in cameras {
                        let name = cam_info.human_name();
                        let name_lower = name.to_lowercase();
                        tracing::info!("Nokhwa Found Candidate: '{}' (Index: {:?})", name, cam_info.index());
                        
                        if name_lower.contains("facetime") || name_lower.contains("continuity") || name.eq("Capture") { 
                            tracing::info!("Ignoring filtered camera: {}", name);
                            continue; 
                        }

                        
                        // Treat "Capture" as a generic name from Hubs sometimes? Keeping filter for now.
                        // NOTE: If hub camera is named "Capture", it is ignored. I should relax this if user complains about missing cam.
                        // User said: "not detecting... because of usb hub".
                        // Maybe the name is generic.
                        // I will remove "Capture" from filter just in case.
                         if name_lower.contains("facetime") || name_lower.contains("continuity") { 
                            tracing::info!("Ignoring filtered camera: {}", name);
                            continue; 
                        }

                        // Note: LifeCam VX-5000 filter removed - these cameras now work with safe mode
                        // Legacy LifeCam cameras will use 640x480 MJPEG/YUYV fallback in webcam.rs
                        
                        // Skip Kinect NUI Camera - it's handled separately below
                        if name_lower.contains("xbox nui") || name_lower.contains("kinect") {
                            tracing::info!("Skipping Kinect (handled separately): {}", name);
                            continue;
                        }
                        
                        if let nokhwa::utils::CameraIndex::Index(idx) = cam_info.index() {
                            let is_insta = name_lower.contains("insta360");
                            let id = if is_insta { format!("Insta360_{}", idx) } else { format!("Webcam_{}", idx) };
                            list.push(Discovered::Nokhwa { id, index: cam_info.index().clone(), name, is_insta });
                        }
                    }
                },
                Ok(Err(e)) => tracing::error!("Nokhwa query failed: {}", e),
                Err(e) => tracing::error!("Nokhwa query panicked: {:?}", e),
            }
            
            // Kinect Detection (separate from nokhwa)
            if let Some(kinect_index) = KinectCamera::detect() {
                tracing::info!("Kinect v1 detected at index {}", kinect_index);
                let id = format!("Kinect_{}", kinect_index);
                list.push(Discovered::Kinect { id, index: kinect_index });
            }
            
            // Mock removed
            list
        }).await?;

        // 2. Diff
        // We need to match existing cameras by ID.
        // Issue: GPhoto cameras are *moved* out of existing if kept?
        // Or we just check IDs.
        
            
            // 2. Diff
            // We need to match existing cameras by ID.
            // Issue: GPhoto cameras are *moved* out of existing if kept?
            // Or we just check IDs.
            
            let mut new_map: std::collections::HashMap<String, Discovered> = std::collections::HashMap::new();
            for d in discovered_list {
                new_map.insert(d.id(), d);
            }

            let mut report = ReconciliationReport { added: vec![], removed: vec![] };
            let mut final_cameras: Vec<std::sync::Arc<dyn Camera>> = Vec::new();
            
            // Take old cameras
            let old_cameras = std::mem::take(&mut self.cameras);
            
            for cam in old_cameras {
                let id = cam.id();
                // Health Check
                if let Err(e) = cam.battery_level() {
                     tracing::warn!("Existing camera {} failed health check: {}. Removing.", id, e);
                     report.removed.push(id.clone());
                     continue; 
                }

                if new_map.contains_key(&id) {
                    // Keep it
                    final_cameras.push(cam);
                    new_map.remove(&id); // Remove from map so we don't re-add
                } else {
                    // It's not in new map (discovery), but it passed health check?
                    // If it's pure USB discovery, new_map should have everything connected.
                    // If GPhoto, we might have skipped discovery.
                    
                    #[cfg(feature = "gphoto2")]
                    if id.contains("GPhoto") && gphoto_active {
                        // We skipped gphoto discovery, so we trust the health check
                        final_cameras.push(cam);
                        continue;
                    }

                    // Else, it's truly gone
                    report.removed.push(id.clone());
                }
            }
            
            // 3. Add New
            for (id, disc) in new_map {
                report.added.push(id.clone());
                match disc {
                    #[cfg(feature = "gphoto2")]
                    Discovered::GPhoto(c) => {
                       self.registry.register_camera(id.clone(), "DSLR".to_string(), CameraRole::HighResCapture, c.capabilities.clone());
                       final_cameras.push(std::sync::Arc::new(c));
                    },
                    Discovered::Nokhwa { id, index, name, is_insta } => {
                        // Check registry for enabled state (Default false)
                        let enabled = self.registry.get_profile(&id).map(|p| p.enabled).unwrap_or(false);
                        
                        // Use Lazy Camera to prevent startup crash
                        let cam = LazyNokhwaCamera::new(index, name.clone(), is_insta, enabled);
                        
                        // Register with guessed capabilities
                         self.registry.register_camera(
                              id.clone(), 
                              name.to_string(), 
                              CameraRole::LiveFeedback,
                              if is_insta {
                                  CameraCapabilities {
                                      has_gimbal: true, has_zoom: true, has_autofocus: true,
                                      resolutions: vec![ (1920, 1080), (1280, 720) ],
                                      frame_rates: vec![30, 60],
                                      ..Default::default()
                                  }
                              } else {
                                  CameraCapabilities {
                                      resolutions: vec![ (640, 480) ],
                                      frame_rates: vec![30],
                                      ..Default::default()
                                  }
                              }
                          );
                        final_cameras.push(std::sync::Arc::new(cam));
                    },
                    Discovered::Kinect { id, index } => {
                        // Initialize Kinect camera
                        match KinectCamera::new(index) {
                            Ok(kinect) => {
                                // Register with full depth camera capabilities
                                self.registry.register_camera(
                                    id.clone(),
                                    "Xbox Kinect v1".to_string(),
                                    CameraRole::DepthCamera,
                                    CameraCapabilities {
                                        resolutions: vec![(640, 480)],
                                        frame_rates: vec![30],
                                        has_gimbal: false,
                                        has_zoom: false,
                                        has_autofocus: false,
                                        // Depth camera capabilities
                                        has_depth: true,
                                        has_infrared: true,
                                        has_motor_tilt: true,
                                        has_accelerometer: true,
                                        has_audio_array: true,
                                        depth_resolution: Some(KinectCamera::DEPTH_RESOLUTION),
                                        tilt_range_degrees: Some(KinectCamera::TILT_RANGE),
                                        audio_channels: Some(KinectCamera::AUDIO_CHANNELS),
                                        depth_range_meters: Some((
                                            KinectCamera::DEPTH_MIN_METERS,
                                            KinectCamera::DEPTH_MAX_METERS
                                        )),
                                        ..Default::default()
                                    }
                                );
                                final_cameras.push(std::sync::Arc::new(kinect));
                                tracing::info!("Kinect {} initialized successfully", id);
                            }
                            Err(e) => {
                                tracing::error!("Failed to initialize Kinect {}: {}", id, e);
                                report.added.retain(|a| a != &id);
                            }
                        }
                    },
                    #[cfg(feature = "gphoto2")]
                    Discovered::KeepAlive(_) => {
                    // Should not happen if it matched an existing camera, as it would be removed from new_map.
                    // If here, we tried to keep a camera that didn't exist. Ignore.
                }
            }
        }
        
        self.cameras = final_cameras;
        Ok(report)
    }
    
    pub fn trigger_all(&self, config: &CameraConfig) -> Vec<Result<PathBuf>> {
        self.cameras.iter()
            .map(|cam| cam.capture(config))
            .collect()
    }

    pub fn trigger_group(&self, role: CameraRole, config: &CameraConfig) -> Vec<Result<PathBuf>> {
        // Parallel execution would be better here, but requires changing signature to use rayon or threads.
        // For now, straightforward iteration filtering by role.
        // We need to look up role from registry.
        
        let mut results = Vec::new();
        for cam in &self.cameras {
             if let Some(profile) = self.registry.get_profile(&cam.id()) {
                 if profile.role == role {
                     results.push(cam.capture(config));
                 }
             }
        }
        results
    }
    
    pub fn sync_settings(&self, config: &CameraConfig) -> Vec<Result<()>> {
        self.cameras.iter()
            .map(|cam| cam.set_config(config))
            .collect()
    }
    
    // Returns a cloned Arc, allowing shared access
    pub fn get_camera(&self, index: usize) -> Option<std::sync::Arc<dyn Camera>> {
        self.cameras.get(index).cloned()
    }
    
    // Returns a cloned Arc, allowing shared access
    pub fn get_camera_by_id(&self, id: &str) -> Option<std::sync::Arc<dyn Camera>> {
        self.cameras.iter().find(|c| c.id() == id).cloned()
    }

    pub fn get_gimbal(&self, index: usize) -> Option<std::sync::Arc<dyn Camera>> {
         // This logic is tricky with Arc. 
         // We can't cast Arc<dyn Camera> to Arc<dyn Gimbal> easily without check.
         // But we can just return the camera and let caller check?
         // Or rely on `as_gimbal()` on the instance.
         // Actually `as_gimbal` returns `&dyn Gimbal`.
         // So:
         self.cameras.get(index).cloned()
    }
}

// MockCamera Removed
