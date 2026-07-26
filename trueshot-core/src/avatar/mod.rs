//! Avatar Capture System
//!
//! Complete avatar creation pipeline:
//! - Multi-camera human scanning with 4DGS
//! - SMPL-X body model fitting
//! - Automatic skeleton rigging (HumanRig-inspired)
//! - Clothing layer separation
//! - Blendshape extraction for expressions
//! - Voice cloning profile generation
//! - Accessory attachment points

pub mod animation;
pub use animation::{ARKitBlendshapes, IKSolver, IKChain, IKTarget, AnimationDriver};

use nalgebra as na;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::f32::consts::PI;

use crate::reconstruction::multicam_sfm::{
    CameraId, CameraIntrinsics, HighResImage, MultiCamConfig, MultiCamSfm,
};

// ============================================================================
// Core Types
// ============================================================================

/// Complete avatar data
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Avatar {
    /// Unique identifier
    pub id: String,
    /// Avatar name/display name
    pub name: String,
    /// Body mesh and rig
    pub body: AvatarBody,
    /// Clothing layers
    pub clothing: Vec<ClothingLayer>,
    /// Face blendshapes for expressions
    pub blendshapes: Vec<Blendshape>,
    /// Voice profile for TTS
    pub voice_profile: Option<VoiceProfile>,
    /// Accessory attachment points
    pub attachments: Vec<AttachmentPoint>,
    /// Creation metadata
    pub metadata: AvatarMetadata,
}

/// Avatar body with skeleton rig
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AvatarBody {
    /// SMPL-X template with fitted parameters
    pub smpl_params: SMPLXParams,
    /// Skeleton hierarchy
    pub skeleton: Skeleton,
    /// Mesh vertices (in T-pose)
    pub vertices: Vec<na::Point3<f32>>,
    /// Mesh faces
    pub faces: Vec<[u32; 3]>,
    /// UV coordinates
    pub uvs: Vec<[f32; 2]>,
    /// Skin weights per vertex
    pub skin_weights: Vec<SkinWeight>,
    /// Texture map (optional)
    pub texture: Option<TextureData>,
    /// Normal map (optional)
    pub normal_map: Option<TextureData>,
}

/// SMPL-X body model parameters
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SMPLXParams {
    /// Body shape (beta) parameters - 10 dims
    pub betas: [f32; 10],
    /// Expression parameters - 10 dims
    pub expression: [f32; 10],
    /// Global body orientation (axis-angle)
    pub global_orient: [f32; 3],
    /// Body pose parameters (21 joints + hands)
    pub body_pose: Vec<[f32; 3]>,
    /// Left hand pose
    pub left_hand_pose: Vec<[f32; 3]>,
    /// Right hand pose
    pub right_hand_pose: Vec<[f32; 3]>,
    /// Jaw pose
    pub jaw_pose: [f32; 3],
    /// Left eye orientation
    pub left_eye_pose: [f32; 3],
    /// Right eye orientation
    pub right_eye_pose: [f32; 3],
    /// Translation
    pub translation: [f32; 3],
}

impl Default for SMPLXParams {
    fn default() -> Self {
        Self {
            betas: [0.0; 10],
            expression: [0.0; 10],
            global_orient: [0.0, 0.0, 0.0],
            body_pose: vec![[0.0, 0.0, 0.0]; 21],
            left_hand_pose: vec![[0.0, 0.0, 0.0]; 15],
            right_hand_pose: vec![[0.0, 0.0, 0.0]; 15],
            jaw_pose: [0.0, 0.0, 0.0],
            left_eye_pose: [0.0, 0.0, 0.0],
            right_eye_pose: [0.0, 0.0, 0.0],
            translation: [0.0, 0.0, 0.0],
        }
    }
}

/// Skeleton for animation
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Skeleton {
    /// All joints
    pub joints: Vec<Joint>,
    /// Root joint index
    pub root: usize,
}

/// Single joint in skeleton
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Joint {
    /// Joint name (e.g., "LeftShoulder", "RightKnee")
    pub name: String,
    /// Parent joint index (None for root)
    pub parent: Option<usize>,
    /// Children joint indices
    pub children: Vec<usize>,
    /// Local position relative to parent
    pub local_position: na::Point3<f32>,
    /// Local rotation (quaternion)
    pub local_rotation: na::UnitQuaternion<f32>,
    /// Bind pose (inverse of world transform at T-pose)
    pub inverse_bind_pose: na::Matrix4<f32>,
}

/// Skin weight for a vertex
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkinWeight {
    /// Joint indices (up to 4)
    pub joints: [u8; 4],
    /// Corresponding weights
    pub weights: [f32; 4],
}

/// Clothing layer (separate from body mesh)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClothingLayer {
    /// Layer ID
    pub id: String,
    /// Display name
    pub name: String,
    /// Clothing type
    pub clothing_type: ClothingType,
    /// Vertices (in T-pose, matching body space)
    pub vertices: Vec<na::Point3<f32>>,
    /// Faces
    pub faces: Vec<[u32; 3]>,
    /// UVs
    pub uvs: Vec<[f32; 2]>,
    /// Offsets from body surface
    pub body_offsets: Vec<f32>,
    /// Skin weights (matching body skeleton)
    pub skin_weights: Vec<SkinWeight>,
    /// Texture
    pub texture: Option<TextureData>,
    /// Is currently visible
    pub visible: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ClothingType {
    Top,
    Bottom,
    FullBody,
    Shoes,
    Hat,
    Glasses,
    Accessory,
}

/// Blendshape for facial expressions
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Blendshape {
    /// Blendshape name (e.g., "smile", "eyebrowRaise")
    pub name: String,
    /// Vertex deltas when fully activated
    pub deltas: Vec<na::Vector3<f32>>,
    /// Current weight (0-1)
    pub weight: f32,
}

/// Voice profile for TTS
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VoiceProfile {
    /// Reference audio samples (for cloning)
    pub reference_samples: Vec<Vec<f32>>,
    /// Sample rate
    pub sample_rate: u32,
    /// Total reference duration
    pub total_duration: f32,
    /// Extracted voice embedding (for TTS)
    pub embedding: Option<Vec<f32>>,
    /// Voice settings
    pub settings: VoiceSettings,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VoiceSettings {
    /// Speaking speed multiplier
    pub speed: f32,
    /// Pitch shift (semitones)
    pub pitch_shift: f32,
    /// Stability (0-1)
    pub stability: f32,
    /// Similarity boost (0-1)
    pub similarity_boost: f32,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            speed: 1.0,
            pitch_shift: 0.0,
            stability: 0.75,
            similarity_boost: 0.75,
        }
    }
}

/// Attachment point for accessories
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttachmentPoint {
    /// Point name
    pub name: String,
    /// Type of attachment
    pub attachment_type: AttachmentType,
    /// Joint to attach to
    pub joint_name: String,
    /// Local offset from joint
    pub offset: na::Vector3<f32>,
    /// Local rotation
    pub rotation: na::UnitQuaternion<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum AttachmentType {
    Head,
    LeftHand,
    RightHand,
    LeftEar,
    RightEar,
    Neck,
    Waist,
    Back,
    LeftFoot,
    RightFoot,
}

/// Avatar creation metadata
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AvatarMetadata {
    pub created_at: String,
    pub source_scan_id: Option<String>,
    pub capture_duration_seconds: f32,
    pub num_cameras: u32,
    pub num_frames: u32,
    pub software_version: String,
}

/// Texture data reference
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextureData {
    /// Path or embedded base64
    pub data: String,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Format
    pub format: TextureFormat,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TextureFormat {
    RGBA8,
    RGB8,
    PNG,
    JPEG,
}

// ============================================================================
// Avatar Capture Pipeline
// ============================================================================

/// Avatar capture session
pub struct AvatarCaptureSession {
    /// Session ID
    pub id: String,
    /// Capture state
    state: AvatarCaptureState,
    /// Captured frames per camera
    frames: HashMap<String, Vec<CapturedAvatarFrame>>,
    /// Audio capture for voice cloning
    audio_samples: Vec<f32>,
    /// Configuration
    config: AvatarCaptureConfig,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AvatarCaptureState {
    Idle,
    Calibrating,
    CapturingTPose,
    CapturingExpressions,
    CapturingMotion,
    CapturingVoice,
    Processing,
    Complete,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct CapturedAvatarFrame {
    pub timestamp: f64,
    pub camera_id: String,
    pub image_data: Vec<u8>,
    pub pose_landmarks: Option<PoseLandmarks>,
    pub face_landmarks: Option<FaceLandmarks>,
}

#[derive(Clone, Debug)]
pub struct PoseLandmarks {
    pub landmarks: Vec<Landmark3D>,
    pub confidence: f32,
}

#[derive(Clone, Debug)]
pub struct FaceLandmarks {
    pub landmarks: Vec<Landmark3D>,
    pub confidence: f32,
    pub expression: Option<[f32; 10]>,
}

#[derive(Clone, Debug)]
pub struct Landmark3D {
    pub position: na::Point3<f32>,
    pub visibility: f32,
}

#[derive(Clone, Debug)]
pub struct AvatarCaptureConfig {
    /// Number of cameras
    pub num_cameras: usize,
    /// Frames per camera for T-pose
    pub tpose_frames: usize,
    /// Frames per expression
    pub expression_frames: usize,
    /// Motion capture frames
    pub motion_frames: usize,
    /// Voice sample duration
    pub voice_sample_duration: f32,
    /// Enable clothing separation
    pub separate_clothing: bool,
    /// Extract blendshapes
    pub extract_blendshapes: bool,
}

impl Default for AvatarCaptureConfig {
    fn default() -> Self {
        Self {
            num_cameras: 4,
            tpose_frames: 30,
            expression_frames: 60,
            motion_frames: 300,
            voice_sample_duration: 30.0,
            separate_clothing: true,
            extract_blendshapes: true,
        }
    }
}

impl AvatarCaptureSession {
    pub fn new(config: AvatarCaptureConfig) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            state: AvatarCaptureState::Idle,
            frames: HashMap::new(),
            audio_samples: Vec::new(),
            config,
        }
    }
    
    /// Start T-pose capture
    pub fn start_tpose_capture(&mut self) {
        self.state = AvatarCaptureState::CapturingTPose;
    }
    
    /// Start expression capture
    pub fn start_expression_capture(&mut self) {
        self.state = AvatarCaptureState::CapturingExpressions;
    }
    
    /// Start motion capture
    pub fn start_motion_capture(&mut self) {
        self.state = AvatarCaptureState::CapturingMotion;
    }
    
    /// Start voice capture
    pub fn start_voice_capture(&mut self) {
        self.state = AvatarCaptureState::CapturingVoice;
    }
    
    /// Add captured frame
    pub fn add_frame(&mut self, frame: CapturedAvatarFrame) {
        let camera_id = frame.camera_id.clone();
        self.frames
            .entry(camera_id)
            .or_default()
            .push(frame);
    }
    
    /// Add audio samples
    pub fn add_audio(&mut self, samples: &[f32]) {
        self.audio_samples.extend_from_slice(samples);
    }
    
    /// Get current state
    pub fn state(&self) -> &AvatarCaptureState {
        &self.state
    }
    
    /// Process capture into avatar
    pub fn process(&mut self) -> Result<Avatar, AvatarError> {
        self.state = AvatarCaptureState::Processing;
        
        // 1. Fit SMPL-X body model
        let smpl_params = self.fit_smpl_body()?;
        
        // 2. Generate skeleton with automatic rigging
        let skeleton = self.generate_skeleton(&smpl_params)?;
        
        // 3. Reconstruct mesh from multi-view
        let (vertices, faces, uvs) = self.reconstruct_mesh()?;
        
        // 4. Compute skin weights
        let skin_weights = self.compute_skin_weights(&vertices, &skeleton)?;
        
        // 5. Separate clothing if enabled
        let clothing = if self.config.separate_clothing {
            self.separate_clothing(&vertices, &faces, &uvs, &smpl_params, &skeleton)?
        } else {
            Vec::new()
        };
        
        // 6. Extract blendshapes if enabled
        let blendshapes = if self.config.extract_blendshapes {
            self.extract_blendshapes(&vertices, &skeleton)?
        } else {
            Vec::new()
        };
        
        // 7. Generate voice profile
        let voice_profile = if !self.audio_samples.is_empty() {
            Some(self.generate_voice_profile()?)
        } else {
            None
        };
        
        // 8. Create attachment points
        let attachments = self.create_attachment_points(&skeleton);
        
        let body = AvatarBody {
            smpl_params,
            skeleton,
            vertices,
            faces,
            uvs,
            skin_weights,
            texture: None,
            normal_map: None,
        };
        
        let avatar = Avatar {
            id: uuid::Uuid::new_v4().to_string(),
            name: "New Avatar".to_string(),
            body,
            clothing,
            blendshapes,
            voice_profile,
            attachments,
            metadata: AvatarMetadata {
                created_at: chrono::Utc::now().to_rfc3339(),
                source_scan_id: Some(self.id.clone()),
                capture_duration_seconds: self.total_capture_duration(),
                num_cameras: self.frames.len() as u32,
                num_frames: self.total_frames() as u32,
                software_version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };
        
        self.state = AvatarCaptureState::Complete;
        Ok(avatar)
    }
    
    // ========== Internal Methods ==========
    
    fn fit_smpl_body(&self) -> Result<SMPLXParams, AvatarError> {
        let mut params = SMPLXParams::default();

        if let Some(stats) = estimate_body_stats(&self.frames) {
            let height = stats.height.clamp(1.2, 2.2);
            let width = stats.width.max(0.15);
            let depth = stats.depth.max(0.12);
            params.betas[0] = ((height - 1.7) / 0.25).clamp(-3.0, 3.0);
            let width_ratio = width / height;
            let depth_ratio = depth / height;
            params.betas[1] = ((width_ratio - 0.25) / 0.05).clamp(-3.0, 3.0);
            params.betas[2] = ((depth_ratio - 0.18) / 0.05).clamp(-3.0, 3.0);
            params.translation = [stats.center.x, stats.center.y, stats.center.z];
        }

        let mut expr_sum = [0.0f32; 10];
        let mut expr_count = 0.0f32;
        for frames in self.frames.values() {
            for frame in frames {
                if let Some(face) = &frame.face_landmarks {
                    if let Some(expr) = face.expression {
                        for (idx, value) in expr.iter().enumerate() {
                            expr_sum[idx] += *value;
                        }
                        expr_count += 1.0;
                    }
                }
            }
        }
        if expr_count > 0.0 {
            for idx in 0..params.expression.len() {
                params.expression[idx] = expr_sum[idx] / expr_count;
            }
        }

        Ok(params)
    }
    
    fn generate_skeleton(&self, smpl_params: &SMPLXParams) -> Result<Skeleton, AvatarError> {
        let height = smplx_height_from_betas(&smpl_params.betas);
        let width_scale = 1.0 + smpl_params.betas[1] * 0.08;
        let depth_scale = 1.0 + smpl_params.betas[2] * 0.08;
        let translation = na::Vector3::new(
            smpl_params.translation[0],
            smpl_params.translation[1],
            smpl_params.translation[2],
        );

        let joint_world = default_smplx_joint_positions(height, width_scale, depth_scale, translation);

        let joints = SMPLX_JOINT_NAMES
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let parent = if i == 0 { None } else { Some(SMPLX_PARENTS[i] as usize) };
                let world_pos = joint_world.get(i).cloned().unwrap_or_else(na::Point3::origin);
                let local_pos = if let Some(parent_idx) = parent {
                    let parent_world = joint_world.get(parent_idx).cloned().unwrap_or_else(na::Point3::origin);
                    na::Point3::from(world_pos.coords - parent_world.coords)
                } else {
                    world_pos
                };

                let mut bind = na::Matrix4::identity();
                bind[(0, 3)] = world_pos.x;
                bind[(1, 3)] = world_pos.y;
                bind[(2, 3)] = world_pos.z;
                let inverse_bind_pose = bind.try_inverse().unwrap_or_else(na::Matrix4::identity);

                Joint {
                    name: name.to_string(),
                    parent,
                    children: SMPLX_PARENTS
                        .iter()
                        .enumerate()
                        .filter(|(_, &p)| p as usize == i)
                        .map(|(c, _)| c)
                        .collect(),
                    local_position: local_pos,
                    local_rotation: na::UnitQuaternion::identity(),
                    inverse_bind_pose,
                }
            })
            .collect();

        Ok(Skeleton { joints, root: 0 })
    }
    
    fn reconstruct_mesh(&self) -> Result<(Vec<na::Point3<f32>>, Vec<[u32; 3]>, Vec<[f32; 2]>), AvatarError> {
        let temp_root = std::env::temp_dir().join(format!("trueshot_avatar_{}", uuid::Uuid::new_v4()));
        let images_dir = temp_root.join("raw").join("images");
        std::fs::create_dir_all(&images_dir)
            .map_err(|e| AvatarError::ProcessingError(format!("Failed to create temp dir: {e}")))?;

        let mut images = Vec::new();
        for (camera_id, frames) in &self.frames {
            let safe_camera = sanitize_component(camera_id);
            for (idx, frame) in frames.iter().enumerate() {
                let img = match image::load_from_memory(&frame.image_data) {
                    Ok(img) => img.to_rgb8(),
                    Err(_) => continue,
                };

                let width = img.width();
                let height = img.height();
                let filename = format!("{}_{}.png", safe_camera, idx);
                let path = images_dir.join(filename);
                if let Err(err) = img.save(&path) {
                    return Err(AvatarError::ProcessingError(format!(
                        "Failed to persist avatar frame: {err}"
                    )));
                }

                let focal = (width.max(height) as f64) * 1.2;
                let intrinsics = CameraIntrinsics {
                    fx: focal,
                    fy: focal,
                    cx: width as f64 / 2.0,
                    cy: height as f64 / 2.0,
                    width,
                    height,
                    distortion: Vec::new(),
                    distortion_model: trueshot_sfm::DistortionModel::None,
                };
                let timestamp_ms = if frame.timestamp.is_finite() {
                    (frame.timestamp.max(0.0) * 1000.0) as u64
                } else {
                    0
                };

                images.push(HighResImage {
                    camera_id: CameraId(camera_id.clone()),
                    path,
                    width,
                    height,
                    intrinsics,
                    timestamp_ms: Some(timestamp_ms),
                    focus_distance: None,
                    exposure_value: None,
                    bracket_group: None,
                });
            }
        }

        if images.len() < 2 {
            return Err(AvatarError::InsufficientFrames);
        }

        let mut config = MultiCamConfig::default();
        config.enable_dense = true;
        config.mesh_resolution = config.mesh_resolution.max(192);

        let mut sfm = MultiCamSfm::new(config);
        sfm.ingest_sd_card(images)
            .map_err(|e| AvatarError::ProcessingError(format!("Avatar ingest failed: {e}")))?;
        sfm.run_reconstruction()
            .map_err(|e| AvatarError::ProcessingError(format!("Avatar reconstruction failed: {e}")))?;

        let mesh = sfm
            .mesh()
            .ok_or_else(|| AvatarError::ProcessingError("Avatar reconstruction produced no mesh".to_string()))?;
        let vertices: Vec<na::Point3<f32>> = mesh
            .vertices
            .iter()
            .map(|v| na::Point3::new(v.x as f32, v.y as f32, v.z as f32))
            .collect();
        let faces: Vec<[u32; 3]> = mesh
            .triangles
            .iter()
            .map(|tri| [tri[0] as u32, tri[1] as u32, tri[2] as u32])
            .collect();
        let uvs = generate_uvs_from_vertices(&vertices);

        let _ = std::fs::remove_dir_all(&temp_root);
        Ok((vertices, faces, uvs))
    }
    
    fn compute_skin_weights(
        &self,
        vertices: &[na::Point3<f32>],
        skeleton: &Skeleton,
    ) -> Result<Vec<SkinWeight>, AvatarError> {
        compute_skin_weights_for_vertices(vertices, skeleton)
    }
    
    fn separate_clothing(
        &self,
        vertices: &[na::Point3<f32>],
        faces: &[[u32; 3]],
        uvs: &[[f32; 2]],
        smpl_params: &SMPLXParams,
        skeleton: &Skeleton,
    ) -> Result<Vec<ClothingLayer>, AvatarError> {
        if vertices.is_empty() || faces.is_empty() {
            return Ok(Vec::new());
        }

        let (axis_origin, axis_dir, height) = skeleton_body_axis(skeleton, vertices);
        let width_scale = 1.0 + smpl_params.betas[1] * 0.08;
        let depth_scale = 1.0 + smpl_params.betas[2] * 0.08;
        let body_scale = 0.5 * (width_scale + depth_scale);
        let offset_threshold = (0.015 * height).max(0.005);

        let joint_map = skeleton_joint_map(skeleton);
        let head_pos = joint_map.get("head").cloned();
        let left_wrist = joint_map.get("left_wrist").cloned();
        let right_wrist = joint_map.get("right_wrist").cloned();
        let head_radius = 0.14 * height;
        let wrist_radius = 0.10 * height;

        let mut top_mask = vec![false; vertices.len()];
        let mut bottom_mask = vec![false; vertices.len()];
        let mut shoes_mask = vec![false; vertices.len()];
        let mut accessory_mask = vec![false; vertices.len()];
        let mut offsets = vec![0.0f32; vertices.len()];

        for (idx, v) in vertices.iter().enumerate() {
            let rel = v.coords - axis_origin;
            let t = rel.dot(&axis_dir);
            let height_norm = if height > 1e-4 { (t / height).clamp(0.0, 1.0) } else { 0.0 };
            let axis_point = axis_origin + axis_dir * t;
            let radial = (v.coords - axis_point).norm();
            let base_radius = body_radius(height_norm, height) * body_scale;
            let offset = radial - base_radius;
            offsets[idx] = offset;

            if offset <= offset_threshold {
                continue;
            }

            if height_norm < 0.12 {
                shoes_mask[idx] = true;
                continue;
            }

            let mut accessory = false;
            if let Some(head) = head_pos {
                if (v.coords - head.coords).norm() < head_radius {
                    accessory = true;
                }
            }
            if let Some(wrist) = left_wrist {
                if (v.coords - wrist.coords).norm() < wrist_radius {
                    accessory = true;
                }
            }
            if let Some(wrist) = right_wrist {
                if (v.coords - wrist.coords).norm() < wrist_radius {
                    accessory = true;
                }
            }

            if accessory {
                accessory_mask[idx] = true;
            } else if height_norm > 0.62 {
                top_mask[idx] = true;
            } else {
                bottom_mask[idx] = true;
            }
        }

        let mut layers = Vec::new();
        push_clothing_layer(
            &mut layers,
            "top",
            "Top",
            ClothingType::Top,
            vertices,
            faces,
            uvs,
            &top_mask,
            &offsets,
            skeleton,
        );
        push_clothing_layer(
            &mut layers,
            "bottom",
            "Bottom",
            ClothingType::Bottom,
            vertices,
            faces,
            uvs,
            &bottom_mask,
            &offsets,
            skeleton,
        );
        push_clothing_layer(
            &mut layers,
            "shoes",
            "Shoes",
            ClothingType::Shoes,
            vertices,
            faces,
            uvs,
            &shoes_mask,
            &offsets,
            skeleton,
        );
        push_clothing_layer(
            &mut layers,
            "accessory",
            "Accessory",
            ClothingType::Accessory,
            vertices,
            faces,
            uvs,
            &accessory_mask,
            &offsets,
            skeleton,
        );

        Ok(layers)
    }
    
    fn extract_blendshapes(
        &self,
        vertices: &[na::Point3<f32>],
        skeleton: &Skeleton,
    ) -> Result<Vec<Blendshape>, AvatarError> {
        // Extract blendshapes from expression frames
        let expression_names = [
            "neutral", "smile", "frown", "surprise", "angry",
            "eyebrowsUp", "eyebrowsDown", "eyesClosed", "mouthOpen", "pucker"
        ];

        let weights = mean_expression_weights(&self.frames, expression_names.len());
        let mut blendshapes: Vec<Blendshape> = expression_names
            .iter()
            .enumerate()
            .map(|(idx, name)| Blendshape {
                name: name.to_string(),
                deltas: vec![na::Vector3::zeros(); vertices.len()],
                weight: weights.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0),
            })
            .collect();

        if vertices.is_empty() {
            return Ok(blendshapes);
        }

        let joint_map = skeleton_joint_map(skeleton);
        let head = joint_map.get("head").cloned().unwrap_or_else(|| average_point(vertices));
        let neck = joint_map.get("neck").cloned().unwrap_or_else(|| head);
        let height = body_height_from_vertices(vertices);
        let face_radius = 0.14 * height;

        let mut up = head.coords - neck.coords;
        if up.norm() < 1e-4 {
            up = na::Vector3::y();
        }
        up = up.normalize();
        let mut forward = na::Vector3::new(0.0, 0.0, 1.0);
        if up.cross(&forward).norm() < 1e-4 {
            forward = na::Vector3::new(1.0, 0.0, 0.0);
        }
        let right = up.cross(&forward).normalize();
        let forward = right.cross(&up).normalize();

        let scale = (0.015 * height).max(0.003);
        let mouth_y = -0.05 * height;
        let eye_y = 0.02 * height;
        let brow_y = 0.07 * height;

        for (idx, v) in vertices.iter().enumerate() {
            let rel = v.coords - head.coords;
            let dist = rel.norm();
            if dist > face_radius {
                continue;
            }
            let dx = rel.dot(&right);
            let dy = rel.dot(&up);
            let _dz = rel.dot(&forward);

            let falloff = (-0.5 * (dist / (0.5 * face_radius)).powi(2)).exp();
            let mouth_factor = if dy < mouth_y { ((mouth_y - dy) / (0.08 * height)).clamp(0.0, 1.0) } else { 0.0 };
            let eye_factor = if dy > eye_y && dy < brow_y { ((dy - eye_y) / (0.05 * height)).clamp(0.0, 1.0) } else { 0.0 };
            let brow_factor = if dy >= brow_y { ((dy - brow_y) / (0.05 * height)).clamp(0.0, 1.0) } else { 0.0 };

            apply_blendshape_delta(&mut blendshapes[1].deltas[idx], right, up, forward, dx, scale * 0.6, mouth_factor, 0.2, 0.15, 0.0, falloff); // smile
            apply_blendshape_delta(&mut blendshapes[2].deltas[idx], right, up, forward, dx, scale * 0.5, mouth_factor, -0.2, 0.1, 0.0, falloff); // frown
            apply_blendshape_delta(&mut blendshapes[3].deltas[idx], right, up, forward, dx, scale * 0.7, mouth_factor, 0.4, 0.0, 0.2, falloff); // surprise
            apply_blendshape_delta(&mut blendshapes[4].deltas[idx], right, up, forward, dx, scale * 0.4, brow_factor, -0.25, -0.1, 0.0, falloff); // angry
            apply_blendshape_delta(&mut blendshapes[5].deltas[idx], right, up, forward, dx, scale * 0.5, brow_factor, 0.3, 0.05, 0.0, falloff); // brows up
            apply_blendshape_delta(&mut blendshapes[6].deltas[idx], right, up, forward, dx, scale * 0.4, brow_factor, -0.2, -0.05, 0.0, falloff); // brows down
            apply_blendshape_delta(&mut blendshapes[7].deltas[idx], right, up, forward, dx, scale * 0.3, eye_factor, -0.15, 0.0, 0.0, falloff); // eyes closed
            apply_blendshape_delta(&mut blendshapes[8].deltas[idx], right, up, forward, dx, scale * 0.6, mouth_factor, -0.25, 0.0, 0.2, falloff); // mouth open
            apply_blendshape_delta(&mut blendshapes[9].deltas[idx], right, up, forward, dx, scale * 0.5, mouth_factor, 0.0, -0.25, 0.15, falloff); // pucker
        }

        Ok(blendshapes)
    }
    
    fn generate_voice_profile(&self) -> Result<VoiceProfile, AvatarError> {
        // Extract voice characteristics for TTS cloning
        Ok(VoiceProfile {
            reference_samples: vec![self.audio_samples.clone()],
            sample_rate: 48000,
            total_duration: self.audio_samples.len() as f32 / 48000.0,
            embedding: None,  // Computed by TTS service
            settings: VoiceSettings::default(),
        })
    }
    
    fn create_attachment_points(&self, skeleton: &Skeleton) -> Vec<AttachmentPoint> {
        vec![
            AttachmentPoint {
                name: "head_top".to_string(),
                attachment_type: AttachmentType::Head,
                joint_name: "head".to_string(),
                offset: na::Vector3::new(0.0, 0.1, 0.0),
                rotation: na::UnitQuaternion::identity(),
            },
            AttachmentPoint {
                name: "left_hand".to_string(),
                attachment_type: AttachmentType::LeftHand,
                joint_name: "left_wrist".to_string(),
                offset: na::Vector3::new(0.0, 0.0, 0.0),
                rotation: na::UnitQuaternion::identity(),
            },
            AttachmentPoint {
                name: "right_hand".to_string(),
                attachment_type: AttachmentType::RightHand,
                joint_name: "right_wrist".to_string(),
                offset: na::Vector3::new(0.0, 0.0, 0.0),
                rotation: na::UnitQuaternion::identity(),
            },
            AttachmentPoint {
                name: "neck".to_string(),
                attachment_type: AttachmentType::Neck,
                joint_name: "neck".to_string(),
                offset: na::Vector3::new(0.0, 0.0, 0.0),
                rotation: na::UnitQuaternion::identity(),
            },
        ]
    }
    
    fn total_capture_duration(&self) -> f32 {
        let frame_count = self.total_frames();
        frame_count as f32 / 30.0  // Assuming 30fps
    }
    
    fn total_frames(&self) -> usize {
        self.frames.values().map(|v| v.len()).sum()
    }
}

struct BodyStats {
    height: f32,
    width: f32,
    depth: f32,
    center: na::Vector3<f32>,
}

fn estimate_body_stats(frames: &HashMap<String, Vec<CapturedAvatarFrame>>) -> Option<BodyStats> {
    let mut min = na::Vector3::new(f32::MAX, f32::MAX, f32::MAX);
    let mut max = na::Vector3::new(f32::MIN, f32::MIN, f32::MIN);
    let mut sum = na::Vector3::zeros();
    let mut count = 0.0f32;

    let mut record = |pos: na::Point3<f32>| {
        min.x = min.x.min(pos.x);
        min.y = min.y.min(pos.y);
        min.z = min.z.min(pos.z);
        max.x = max.x.max(pos.x);
        max.y = max.y.max(pos.y);
        max.z = max.z.max(pos.z);
        sum += pos.coords;
        count += 1.0;
    };

    for frames in frames.values() {
        for frame in frames {
            if let Some(pose) = &frame.pose_landmarks {
                if pose.confidence >= 0.2 {
                    for landmark in &pose.landmarks {
                        if landmark.visibility >= 0.2 {
                            record(landmark.position);
                        }
                    }
                }
            }
            if let Some(face) = &frame.face_landmarks {
                if face.confidence >= 0.2 {
                    for landmark in &face.landmarks {
                        if landmark.visibility >= 0.2 {
                            record(landmark.position);
                        }
                    }
                }
            }
        }
    }

    if count < 1.0 {
        return None;
    }

    let center = sum / count;
    let extents = na::Vector3::new(max.x - min.x, max.y - min.y, max.z - min.z);
    let mut dims = [extents.x.abs(), extents.y.abs(), extents.z.abs()];
    dims.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let depth = dims[0];
    let width = dims[1];
    let height = dims[2];

    Some(BodyStats {
        height,
        width,
        depth,
        center,
    })
}

fn smplx_height_from_betas(betas: &[f32; 10]) -> f32 {
    (1.7 + betas[0] * 0.25).clamp(1.2, 2.2)
}

fn default_smplx_joint_positions(
    height: f32,
    width_scale: f32,
    depth_scale: f32,
    translation: na::Vector3<f32>,
) -> Vec<na::Point3<f32>> {
    let h = height;
    let p = |x: f32, y: f32, z: f32| {
        na::Point3::from(na::Vector3::new(x * width_scale, y, z * depth_scale) + translation)
    };

    let pelvis = p(0.0, 0.0, 0.0);
    let spine1 = p(0.0, 0.12 * h, 0.0);
    let spine2 = p(0.0, 0.24 * h, 0.0);
    let spine3 = p(0.0, 0.36 * h, 0.0);
    let neck = p(0.0, 0.43 * h, 0.0);
    let head = p(0.0, 0.52 * h, 0.0);
    let jaw = p(0.0, 0.49 * h, 0.04 * h);
    let left_eye = p(-0.035 * h, 0.54 * h, 0.05 * h);
    let right_eye = p(0.035 * h, 0.54 * h, 0.05 * h);

    let left_hip = p(-0.09 * h, -0.05 * h, 0.0);
    let right_hip = p(0.09 * h, -0.05 * h, 0.0);
    let left_knee = p(-0.09 * h, -0.35 * h, 0.0);
    let right_knee = p(0.09 * h, -0.35 * h, 0.0);
    let left_ankle = p(-0.09 * h, -0.58 * h, 0.0);
    let right_ankle = p(0.09 * h, -0.58 * h, 0.0);
    let left_foot = p(-0.09 * h, -0.64 * h, 0.06 * h);
    let right_foot = p(0.09 * h, -0.64 * h, 0.06 * h);

    let left_collar = p(-0.12 * h, 0.40 * h, 0.0);
    let right_collar = p(0.12 * h, 0.40 * h, 0.0);
    let left_shoulder = p(-0.20 * h, 0.39 * h, 0.0);
    let right_shoulder = p(0.20 * h, 0.39 * h, 0.0);
    let left_elbow = p(-0.40 * h, 0.30 * h, 0.02 * h);
    let right_elbow = p(0.40 * h, 0.30 * h, 0.02 * h);
    let left_wrist = p(-0.55 * h, 0.22 * h, 0.04 * h);
    let right_wrist = p(0.55 * h, 0.22 * h, 0.04 * h);

    let mut joints = HashMap::new();
    joints.insert("pelvis", pelvis);
    joints.insert("left_hip", left_hip);
    joints.insert("right_hip", right_hip);
    joints.insert("spine1", spine1);
    joints.insert("left_knee", left_knee);
    joints.insert("right_knee", right_knee);
    joints.insert("spine2", spine2);
    joints.insert("left_ankle", left_ankle);
    joints.insert("right_ankle", right_ankle);
    joints.insert("spine3", spine3);
    joints.insert("left_foot", left_foot);
    joints.insert("right_foot", right_foot);
    joints.insert("neck", neck);
    joints.insert("left_collar", left_collar);
    joints.insert("right_collar", right_collar);
    joints.insert("head", head);
    joints.insert("left_shoulder", left_shoulder);
    joints.insert("right_shoulder", right_shoulder);
    joints.insert("left_elbow", left_elbow);
    joints.insert("right_elbow", right_elbow);
    joints.insert("left_wrist", left_wrist);
    joints.insert("right_wrist", right_wrist);
    joints.insert("jaw", jaw);
    joints.insert("left_eye_smplhf", left_eye);
    joints.insert("right_eye_smplhf", right_eye);

    let finger_step = 0.03 * h;
    let left_dir = na::Vector3::new(-1.0, 0.0, 0.0);
    let right_dir = na::Vector3::new(1.0, 0.0, 0.0);

    add_finger_chain(&mut joints, left_wrist, left_dir, finger_step, ["left_index1", "left_index2", "left_index3"], na::Vector3::new(0.0, 0.0, 0.03 * h));
    add_finger_chain(&mut joints, left_wrist, left_dir, finger_step, ["left_middle1", "left_middle2", "left_middle3"], na::Vector3::new(0.0, 0.0, 0.02 * h));
    add_finger_chain(&mut joints, left_wrist, left_dir, finger_step, ["left_ring1", "left_ring2", "left_ring3"], na::Vector3::new(0.0, 0.0, 0.01 * h));
    add_finger_chain(&mut joints, left_wrist, left_dir, finger_step, ["left_pinky1", "left_pinky2", "left_pinky3"], na::Vector3::new(0.0, 0.0, 0.0));
    add_thumb_chain(&mut joints, left_wrist, -1.0, finger_step, ["left_thumb1", "left_thumb2", "left_thumb3"]);

    add_finger_chain(&mut joints, right_wrist, right_dir, finger_step, ["right_index1", "right_index2", "right_index3"], na::Vector3::new(0.0, 0.0, 0.03 * h));
    add_finger_chain(&mut joints, right_wrist, right_dir, finger_step, ["right_middle1", "right_middle2", "right_middle3"], na::Vector3::new(0.0, 0.0, 0.02 * h));
    add_finger_chain(&mut joints, right_wrist, right_dir, finger_step, ["right_ring1", "right_ring2", "right_ring3"], na::Vector3::new(0.0, 0.0, 0.01 * h));
    add_finger_chain(&mut joints, right_wrist, right_dir, finger_step, ["right_pinky1", "right_pinky2", "right_pinky3"], na::Vector3::new(0.0, 0.0, 0.0));
    add_thumb_chain(&mut joints, right_wrist, 1.0, finger_step, ["right_thumb1", "right_thumb2", "right_thumb3"]);

    SMPLX_JOINT_NAMES
        .iter()
        .map(|name| joints.get(name).cloned().unwrap_or(pelvis))
        .collect()
}

fn add_finger_chain(
    joints: &mut HashMap<&'static str, na::Point3<f32>>,
    wrist: na::Point3<f32>,
    direction: na::Vector3<f32>,
    step: f32,
    names: [&'static str; 3],
    offset: na::Vector3<f32>,
) {
    let base = na::Point3::from(wrist.coords + offset);
    for (idx, name) in names.iter().enumerate() {
        let pos = base.coords + direction * step * (idx as f32 + 1.0);
        joints.insert(*name, na::Point3::from(pos));
    }
}

fn add_thumb_chain(
    joints: &mut HashMap<&'static str, na::Point3<f32>>,
    wrist: na::Point3<f32>,
    side: f32,
    step: f32,
    names: [&'static str; 3],
) {
    let dir = na::Vector3::new(0.7 * side, -0.3, 0.2);
    let base = na::Point3::from(wrist.coords + na::Vector3::new(0.7 * side, -0.4, -0.2) * step);
    for (idx, name) in names.iter().enumerate() {
        let pos = base.coords + dir * step * (idx as f32 + 1.0);
        joints.insert(*name, na::Point3::from(pos));
    }
}

fn skeleton_world_positions(skeleton: &Skeleton) -> Vec<na::Point3<f32>> {
    let mut cache = vec![None; skeleton.joints.len()];
    for idx in 0..skeleton.joints.len() {
        let _ = world_position(idx, skeleton, &mut cache);
    }
    cache.into_iter().map(|p| p.unwrap_or_else(na::Point3::origin)).collect()
}

fn world_position(
    idx: usize,
    skeleton: &Skeleton,
    cache: &mut Vec<Option<na::Point3<f32>>>,
) -> na::Point3<f32> {
    if let Some(pos) = cache[idx] {
        return pos;
    }

    let joint = &skeleton.joints[idx];
    let pos = if let Some(parent) = joint.parent {
        let parent_pos = world_position(parent, skeleton, cache);
        na::Point3::from(parent_pos.coords + joint.local_position.coords)
    } else {
        joint.local_position
    };
    cache[idx] = Some(pos);
    pos
}

fn generate_uvs_from_vertices(vertices: &[na::Point3<f32>]) -> Vec<[f32; 2]> {
    if vertices.is_empty() {
        return Vec::new();
    }

    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    for v in vertices {
        min_y = min_y.min(v.y);
        max_y = max_y.max(v.y);
    }
    let height = (max_y - min_y).max(1e-4);

    vertices
        .iter()
        .map(|v| {
            let theta = v.z.atan2(v.x);
            let u = (theta / (2.0 * PI)) + 0.5;
            let v_coord = (v.y - min_y) / height;
            [u, v_coord.clamp(0.0, 1.0)]
        })
        .collect()
}

fn compute_skin_weights_for_vertices(
    vertices: &[na::Point3<f32>],
    skeleton: &Skeleton,
) -> Result<Vec<SkinWeight>, AvatarError> {
    if vertices.is_empty() {
        return Ok(Vec::new());
    }

    let joint_positions = skeleton_world_positions(skeleton);
    if joint_positions.is_empty() {
        return Err(AvatarError::ProcessingError("Skeleton has no joints".to_string()));
    }

    let mut weights = Vec::with_capacity(vertices.len());
    for vertex in vertices {
        let mut distances: Vec<(usize, f32)> = joint_positions
            .iter()
            .enumerate()
            .map(|(idx, pos)| (idx, (vertex.coords - pos.coords).norm()))
            .collect();
        distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut joint_ids = [0u8; 4];
        let mut joint_weights = [0.0f32; 4];
        let mut total = 0.0f32;

        for (slot, (joint_idx, dist)) in distances.iter().take(4).enumerate() {
            let w = 1.0 / (dist + 1e-4);
            joint_ids[slot] = (*joint_idx as u8).min(u8::MAX);
            joint_weights[slot] = w;
            total += w;
        }

        if total > 0.0 {
            for slot in 0..4 {
                joint_weights[slot] /= total;
            }
        } else {
            joint_weights[0] = 1.0;
        }

        weights.push(SkinWeight {
            joints: joint_ids,
            weights: joint_weights,
        });
    }

    Ok(weights)
}

fn skeleton_joint_map(skeleton: &Skeleton) -> HashMap<String, na::Point3<f32>> {
    let positions = skeleton_world_positions(skeleton);
    let mut map = HashMap::new();
    for (idx, joint) in skeleton.joints.iter().enumerate() {
        let pos = positions.get(idx).cloned().unwrap_or_else(na::Point3::origin);
        map.insert(joint.name.clone(), pos);
    }
    map
}

fn skeleton_body_axis(
    skeleton: &Skeleton,
    vertices: &[na::Point3<f32>],
) -> (na::Vector3<f32>, na::Vector3<f32>, f32) {
    let joint_map = skeleton_joint_map(skeleton);
    let pelvis = joint_map.get("pelvis").cloned().unwrap_or_else(|| average_point(vertices));
    let neck = joint_map.get("neck").cloned().unwrap_or_else(|| {
        let mut p = pelvis;
        p.y += 0.5 * body_height_from_vertices(vertices);
        p
    });
    let mut axis = neck.coords - pelvis.coords;
    let height = axis.norm().max(1e-4);
    axis /= height;
    (pelvis.coords, axis, height)
}

fn body_radius(height_norm: f32, height: f32) -> f32 {
    let h = height.max(1e-4);
    let radius = if height_norm < 0.2 {
        lerp(0.11, 0.09, height_norm / 0.2)
    } else if height_norm < 0.5 {
        lerp(0.09, 0.12, (height_norm - 0.2) / 0.3)
    } else if height_norm < 0.8 {
        lerp(0.12, 0.10, (height_norm - 0.5) / 0.3)
    } else {
        lerp(0.10, 0.07, (height_norm - 0.8) / 0.2)
    };
    radius * h
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn push_clothing_layer(
    layers: &mut Vec<ClothingLayer>,
    id: &str,
    name: &str,
    clothing_type: ClothingType,
    vertices: &[na::Point3<f32>],
    faces: &[[u32; 3]],
    uvs: &[[f32; 2]],
    mask: &[bool],
    offsets: &[f32],
    skeleton: &Skeleton,
) {
    let mut index_map = vec![None; vertices.len()];
    let mut layer_vertices = Vec::new();
    let mut layer_uvs = Vec::new();
    let mut layer_offsets = Vec::new();
    for (idx, keep) in mask.iter().enumerate() {
        if !*keep {
            continue;
        }
        index_map[idx] = Some(layer_vertices.len() as u32);
        layer_vertices.push(vertices[idx]);
        if idx < uvs.len() {
            layer_uvs.push(uvs[idx]);
        } else {
            layer_uvs.push([0.0, 0.0]);
        }
        layer_offsets.push(offsets[idx]);
    }

    if layer_vertices.len() < 50 {
        return;
    }

    let mut layer_faces = Vec::new();
    for tri in faces {
        let a = tri[0] as usize;
        let b = tri[1] as usize;
        let c = tri[2] as usize;
        if let (Some(na), Some(nb), Some(nc)) = (index_map[a], index_map[b], index_map[c]) {
            layer_faces.push([na, nb, nc]);
        }
    }

    if layer_faces.len() < 25 {
        return;
    }

    let skin_weights = match compute_skin_weights_for_vertices(&layer_vertices, skeleton) {
        Ok(weights) => weights,
        Err(_) => Vec::new(),
    };

    layers.push(ClothingLayer {
        id: id.to_string(),
        name: name.to_string(),
        clothing_type,
        vertices: layer_vertices,
        faces: layer_faces,
        uvs: layer_uvs,
        body_offsets: layer_offsets,
        skin_weights,
        texture: None,
        visible: true,
    });
}

fn mean_expression_weights(
    frames: &HashMap<String, Vec<CapturedAvatarFrame>>,
    dims: usize,
) -> Vec<f32> {
    let mut sum = vec![0.0f32; dims];
    let mut count = 0.0f32;
    for frames in frames.values() {
        for frame in frames {
            if let Some(face) = &frame.face_landmarks {
                if let Some(expr) = face.expression {
                    for (idx, value) in expr.iter().enumerate().take(dims) {
                        sum[idx] += *value;
                    }
                    count += 1.0;
                }
            }
        }
    }
    if count > 0.0 {
        for value in &mut sum {
            *value /= count;
        }
    }
    sum
}

fn apply_blendshape_delta(
    delta: &mut na::Vector3<f32>,
    right: na::Vector3<f32>,
    up: na::Vector3<f32>,
    forward: na::Vector3<f32>,
    dx: f32,
    scale: f32,
    region_factor: f32,
    up_weight: f32,
    lateral_weight: f32,
    forward_weight: f32,
    falloff: f32,
) {
    if region_factor <= 0.0 {
        return;
    }
    let lateral_dir = if dx >= 0.0 { right } else { -right };
    let displacement = up * up_weight + lateral_dir * lateral_weight + forward * forward_weight;
    *delta += displacement * scale * region_factor * falloff;
}

fn average_point(points: &[na::Point3<f32>]) -> na::Point3<f32> {
    if points.is_empty() {
        return na::Point3::origin();
    }
    let mut sum = na::Vector3::zeros();
    for p in points {
        sum += p.coords;
    }
    na::Point3::from(sum / points.len() as f32)
}

fn body_height_from_vertices(vertices: &[na::Point3<f32>]) -> f32 {
    if vertices.is_empty() {
        return 1.0;
    }
    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    for v in vertices {
        min_y = min_y.min(v.y);
        max_y = max_y.max(v.y);
    }
    (max_y - min_y).max(1e-4)
}

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// SMPL-X skeleton constants
const SMPLX_JOINT_NAMES: [&str; 55] = [
    "pelvis", "left_hip", "right_hip", "spine1", "left_knee",
    "right_knee", "spine2", "left_ankle", "right_ankle", "spine3",
    "left_foot", "right_foot", "neck", "left_collar", "right_collar",
    "head", "left_shoulder", "right_shoulder", "left_elbow", "right_elbow",
    "left_wrist", "right_wrist", "jaw", "left_eye_smplhf", "right_eye_smplhf",
    // Remaining joints for hands and face
    "left_index1", "left_index2", "left_index3", "left_middle1", "left_middle2",
    "left_middle3", "left_pinky1", "left_pinky2", "left_pinky3", "left_ring1",
    "left_ring2", "left_ring3", "left_thumb1", "left_thumb2", "left_thumb3",
    "right_index1", "right_index2", "right_index3", "right_middle1", "right_middle2",
    "right_middle3", "right_pinky1", "right_pinky2", "right_pinky3", "right_ring1",
    "right_ring2", "right_ring3", "right_thumb1", "right_thumb2", "right_thumb3",
];

const SMPLX_PARENTS: [i32; 55] = [
    -1, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 12, 13, 14, 16, 17, 18, 19,
    15, 15, 15,  // Face
    20, 25, 26, 20, 28, 29, 20, 31, 32, 20, 34, 35, 20, 37, 38,  // Left hand
    21, 40, 41, 21, 43, 44, 21, 46, 47, 21, 49, 50, 21, 52, 53,  // Right hand
];

// ============================================================================
// Avatar Editor
// ============================================================================

/// Avatar editor for customization
pub struct AvatarEditor {
    avatar: Avatar,
    undo_stack: Vec<AvatarEditAction>,
    redo_stack: Vec<AvatarEditAction>,
}

#[derive(Clone, Debug)]
pub enum AvatarEditAction {
    ChangeClothingVisibility { layer_id: String, visible: bool },
    AddClothing { layer: ClothingLayer },
    RemoveClothing { layer_id: String },
    SetBlendshapeWeight { name: String, weight: f32 },
    SetTexture { target: TextureTarget, texture: TextureData },
    AddAccessory { attachment_point: String, accessory: Accessory },
    RemoveAccessory { attachment_point: String },
    ChangeName { name: String },
}

#[derive(Clone, Debug)]
pub enum TextureTarget {
    Body,
    Clothing(String),
    Accessory(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Accessory {
    pub id: String,
    pub name: String,
    pub mesh_path: String,
    pub texture_path: Option<String>,
    pub scale: f32,
    pub offset: na::Vector3<f32>,
    pub rotation: na::UnitQuaternion<f32>,
}

impl AvatarEditor {
    pub fn new(avatar: Avatar) -> Self {
        Self {
            avatar,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }
    
    /// Toggle clothing layer visibility
    pub fn set_clothing_visibility(&mut self, layer_id: &str, visible: bool) {
        if let Some(layer) = self.avatar.clothing.iter_mut().find(|l| l.id == layer_id) {
            let old_visible = layer.visible;
            layer.visible = visible;
            
            self.undo_stack.push(AvatarEditAction::ChangeClothingVisibility {
                layer_id: layer_id.to_string(),
                visible: old_visible,
            });
            self.redo_stack.clear();
        }
    }
    
    /// Add new clothing layer
    pub fn add_clothing(&mut self, layer: ClothingLayer) {
        self.undo_stack.push(AvatarEditAction::RemoveClothing {
            layer_id: layer.id.clone(),
        });
        self.avatar.clothing.push(layer);
        self.redo_stack.clear();
    }
    
    /// Remove clothing layer
    pub fn remove_clothing(&mut self, layer_id: &str) {
        if let Some(pos) = self.avatar.clothing.iter().position(|l| l.id == layer_id) {
            let layer = self.avatar.clothing.remove(pos);
            self.undo_stack.push(AvatarEditAction::AddClothing { layer });
            self.redo_stack.clear();
        }
    }
    
    /// Set blendshape weight
    pub fn set_blendshape_weight(&mut self, name: &str, weight: f32) {
        if let Some(bs) = self.avatar.blendshapes.iter_mut().find(|b| b.name == name) {
            let old_weight = bs.weight;
            bs.weight = weight.clamp(0.0, 1.0);
            
            self.undo_stack.push(AvatarEditAction::SetBlendshapeWeight {
                name: name.to_string(),
                weight: old_weight,
            });
            self.redo_stack.clear();
        }
    }
    
    /// Strip to base layer (censored body)
    pub fn strip_to_base(&mut self) {
        for layer in &mut self.avatar.clothing {
            layer.visible = false;
        }
    }
    
    /// Restore all clothing
    pub fn restore_all_clothing(&mut self) {
        for layer in &mut self.avatar.clothing {
            layer.visible = true;
        }
    }
    
    /// Undo last action
    pub fn undo(&mut self) -> bool {
        if let Some(action) = self.undo_stack.pop() {
            self.apply_action_inverse(&action);
            self.redo_stack.push(action);
            true
        } else {
            false
        }
    }
    
    /// Redo last undone action
    pub fn redo(&mut self) -> bool {
        if let Some(action) = self.redo_stack.pop() {
            self.apply_action(&action);
            self.undo_stack.push(action);
            true
        } else {
            false
        }
    }
    
    fn apply_action(&mut self, action: &AvatarEditAction) {
        match action {
            AvatarEditAction::ChangeClothingVisibility { layer_id, visible } => {
                if let Some(layer) = self.avatar.clothing.iter_mut().find(|l| l.id == *layer_id) {
                    layer.visible = *visible;
                }
            }
            AvatarEditAction::SetBlendshapeWeight { name, weight } => {
                if let Some(bs) = self.avatar.blendshapes.iter_mut().find(|b| b.name == *name) {
                    bs.weight = *weight;
                }
            }
            _ => {}
        }
    }
    
    fn apply_action_inverse(&mut self, action: &AvatarEditAction) {
        // Same as apply_action for toggle operations
        self.apply_action(action);
    }
    
    /// Get current avatar
    pub fn avatar(&self) -> &Avatar {
        &self.avatar
    }
    
    /// Consume editor and return avatar
    pub fn finalize(self) -> Avatar {
        self.avatar
    }
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Clone, Debug)]
pub enum AvatarError {
    CaptureError(String),
    ProcessingError(String),
    InvalidPose(String),
    InsufficientFrames,
    VoiceExtractionFailed,
}

impl std::fmt::Display for AvatarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AvatarError::CaptureError(msg) => write!(f, "Capture error: {}", msg),
            AvatarError::ProcessingError(msg) => write!(f, "Processing error: {}", msg),
            AvatarError::InvalidPose(msg) => write!(f, "Invalid pose: {}", msg),
            AvatarError::InsufficientFrames => write!(f, "Insufficient frames captured"),
            AvatarError::VoiceExtractionFailed => write!(f, "Voice extraction failed"),
        }
    }
}

impl std::error::Error for AvatarError {}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_avatar_capture_session() {
        let config = AvatarCaptureConfig::default();
        let mut session = AvatarCaptureSession::new(config);
        
        assert_eq!(*session.state(), AvatarCaptureState::Idle);
        
        session.start_tpose_capture();
        assert_eq!(*session.state(), AvatarCaptureState::CapturingTPose);
    }
    
    #[test]
    fn test_smplx_params_default() {
        let params = SMPLXParams::default();
        assert_eq!(params.betas, [0.0; 10]);
        assert_eq!(params.body_pose.len(), 21);
    }
    
    #[test]
    fn test_avatar_editor() {
        let avatar = Avatar {
            id: "test".to_string(),
            name: "Test Avatar".to_string(),
            body: AvatarBody {
                smpl_params: SMPLXParams::default(),
                skeleton: Skeleton { joints: Vec::new(), root: 0 },
                vertices: Vec::new(),
                faces: Vec::new(),
                uvs: Vec::new(),
                skin_weights: Vec::new(),
                texture: None,
                normal_map: None,
            },
            clothing: vec![
                ClothingLayer {
                    id: "shirt".to_string(),
                    name: "Shirt".to_string(),
                    clothing_type: ClothingType::Top,
                    vertices: Vec::new(),
                    faces: Vec::new(),
                    uvs: Vec::new(),
                    body_offsets: Vec::new(),
                    skin_weights: Vec::new(),
                    texture: None,
                    visible: true,
                }
            ],
            blendshapes: Vec::new(),
            voice_profile: None,
            attachments: Vec::new(),
            metadata: AvatarMetadata {
                created_at: String::new(),
                source_scan_id: None,
                capture_duration_seconds: 0.0,
                num_cameras: 0,
                num_frames: 0,
                software_version: String::new(),
            },
        };
        
        let mut editor = AvatarEditor::new(avatar);
        
        // Hide clothing
        editor.set_clothing_visibility("shirt", false);
        assert!(!editor.avatar().clothing[0].visible);
        
        // Undo
        editor.undo();
        assert!(editor.avatar().clothing[0].visible);
    }
}
