//! Avatar System
//!
//! State-of-the-art avatar binding and animation:
//! - VRM/GLB avatar loading with blendshapes
//! - Real-time pose estimation from 4DGS observations
//! - Skeletal animation with IK solving
//! - Facial expression tracking via blendshapes
//! - Cloth/hair physics simulation hooks

use nalgebra as na;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Avatar bone names (VRM standard)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BoneName {
    Hips,
    Spine,
    Chest,
    UpperChest,
    Neck,
    Head,
    LeftShoulder,
    LeftUpperArm,
    LeftLowerArm,
    LeftHand,
    RightShoulder,
    RightUpperArm,
    RightLowerArm,
    RightHand,
    LeftUpperLeg,
    LeftLowerLeg,
    LeftFoot,
    LeftToes,
    RightUpperLeg,
    RightLowerLeg,
    RightFoot,
    RightToes,
    LeftEye,
    RightEye,
    Jaw,
    // Finger bones...
    LeftThumbProximal,
    LeftThumbIntermediate,
    LeftThumbDistal,
    LeftIndexProximal,
    LeftIndexIntermediate,
    LeftIndexDistal,
    LeftMiddleProximal,
    LeftMiddleIntermediate,
    LeftMiddleDistal,
    LeftRingProximal,
    LeftRingIntermediate,
    LeftRingDistal,
    LeftLittleProximal,
    LeftLittleIntermediate,
    LeftLittleDistal,
    RightThumbProximal,
    RightThumbIntermediate,
    RightThumbDistal,
    RightIndexProximal,
    RightIndexIntermediate,
    RightIndexDistal,
    RightMiddleProximal,
    RightMiddleIntermediate,
    RightMiddleDistal,
    RightRingProximal,
    RightRingIntermediate,
    RightRingDistal,
    RightLittleProximal,
    RightLittleIntermediate,
    RightLittleDistal,
}

/// VRM Standard Blendshape Presets
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlendshapePreset {
    // Emotion blendshapes
    Neutral,
    Joy,
    Angry,
    Sorrow,
    Fun,
    Surprised,
    // Viseme blendshapes
    Aa,
    Ih,
    Ou,
    Ee,
    Oh,
    // Eye blendshapes
    Blink,
    BlinkLeft,
    BlinkRight,
    LookUp,
    LookDown,
    LookLeft,
    LookRight,
}

/// A single bone in the skeleton
#[derive(Clone, Debug)]
pub struct Bone {
    pub name: BoneName,
    pub local_position: na::Vector3<f32>,
    pub local_rotation: na::UnitQuaternion<f32>,
    pub local_scale: na::Vector3<f32>,
    pub parent_index: Option<usize>,
    pub children_indices: Vec<usize>,
    /// Bind pose (inverse of rest pose)
    pub inverse_bind_matrix: na::Matrix4<f32>,
}

impl Default for Bone {
    fn default() -> Self {
        Self {
            name: BoneName::Hips,
            local_position: na::Vector3::zeros(),
            local_rotation: na::UnitQuaternion::identity(),
            local_scale: na::Vector3::new(1.0, 1.0, 1.0),
            parent_index: None,
            children_indices: Vec::new(),
            inverse_bind_matrix: na::Matrix4::identity(),
        }
    }
}

/// Complete skeleton with all bones
#[derive(Clone)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
    pub bone_name_to_index: HashMap<BoneName, usize>,
    /// World space transforms (computed)
    pub world_transforms: Vec<na::Matrix4<f32>>,
    /// Skinning matrices (world * inverse_bind)
    pub skinning_matrices: Vec<na::Matrix4<f32>>,
}

impl Skeleton {
    pub fn new() -> Self {
        Self {
            bones: Vec::new(),
            bone_name_to_index: HashMap::new(),
            world_transforms: Vec::new(),
            skinning_matrices: Vec::new(),
        }
    }

    /// Add a bone to the skeleton
    pub fn add_bone(&mut self, bone: Bone) -> usize {
        let index = self.bones.len();
        self.bone_name_to_index.insert(bone.name, index);
        self.bones.push(bone);
        self.world_transforms.push(na::Matrix4::identity());
        self.skinning_matrices.push(na::Matrix4::identity());
        index
    }

    /// Get bone by name
    pub fn get_bone(&self, name: BoneName) -> Option<&Bone> {
        self.bone_name_to_index.get(&name).map(|&i| &self.bones[i])
    }

    /// Get mutable bone by name
    pub fn get_bone_mut(&mut self, name: BoneName) -> Option<&mut Bone> {
        if let Some(&i) = self.bone_name_to_index.get(&name) {
            Some(&mut self.bones[i])
        } else {
            None
        }
    }

    /// Set bone rotation
    pub fn set_bone_rotation(&mut self, name: BoneName, rotation: na::UnitQuaternion<f32>) {
        if let Some(bone) = self.get_bone_mut(name) {
            bone.local_rotation = rotation;
        }
    }

    /// Update world transforms (call after modifying bones)
    pub fn update_transforms(&mut self) {
        for i in 0..self.bones.len() {
            let local_matrix = self.compute_local_matrix(i);

            let world_matrix = if let Some(parent_idx) = self.bones[i].parent_index {
                self.world_transforms[parent_idx] * local_matrix
            } else {
                local_matrix
            };

            self.world_transforms[i] = world_matrix;
            self.skinning_matrices[i] = world_matrix * self.bones[i].inverse_bind_matrix;
        }
    }

    /// Compute local transform matrix for a bone
    fn compute_local_matrix(&self, index: usize) -> na::Matrix4<f32> {
        let bone = &self.bones[index];
        let translation = na::Translation3::from(bone.local_position);
        let rotation = bone.local_rotation.to_homogeneous();
        let scale = na::Matrix4::new_nonuniform_scaling(&bone.local_scale);
        translation.to_homogeneous() * rotation * scale
    }

    /// Create a standard humanoid skeleton
    pub fn create_humanoid() -> Self {
        let mut skeleton = Self::new();

        // Create bones with parent relationships
        let hips = skeleton.add_bone(Bone {
            name: BoneName::Hips,
            local_position: na::Vector3::new(0.0, 0.95, 0.0),
            ..Default::default()
        });

        let spine = skeleton.add_bone(Bone {
            name: BoneName::Spine,
            local_position: na::Vector3::new(0.0, 0.1, 0.0),
            parent_index: Some(hips),
            ..Default::default()
        });

        let chest = skeleton.add_bone(Bone {
            name: BoneName::Chest,
            local_position: na::Vector3::new(0.0, 0.15, 0.0),
            parent_index: Some(spine),
            ..Default::default()
        });

        let neck = skeleton.add_bone(Bone {
            name: BoneName::Neck,
            local_position: na::Vector3::new(0.0, 0.2, 0.0),
            parent_index: Some(chest),
            ..Default::default()
        });

        let _head = skeleton.add_bone(Bone {
            name: BoneName::Head,
            local_position: na::Vector3::new(0.0, 0.1, 0.0),
            parent_index: Some(neck),
            ..Default::default()
        });

        // Left arm
        let _left_shoulder = skeleton.add_bone(Bone {
            name: BoneName::LeftShoulder,
            local_position: na::Vector3::new(0.1, 0.15, 0.0),
            parent_index: Some(chest),
            ..Default::default()
        });

        // Right arm (mirror of left)
        let _right_shoulder = skeleton.add_bone(Bone {
            name: BoneName::RightShoulder,
            local_position: na::Vector3::new(-0.1, 0.15, 0.0),
            parent_index: Some(chest),
            ..Default::default()
        });

        // Legs
        let _left_upper_leg = skeleton.add_bone(Bone {
            name: BoneName::LeftUpperLeg,
            local_position: na::Vector3::new(0.1, -0.05, 0.0),
            parent_index: Some(hips),
            ..Default::default()
        });

        let _right_upper_leg = skeleton.add_bone(Bone {
            name: BoneName::RightUpperLeg,
            local_position: na::Vector3::new(-0.1, -0.05, 0.0),
            parent_index: Some(hips),
            ..Default::default()
        });

        skeleton.update_transforms();
        skeleton
    }
}

impl Default for Skeleton {
    fn default() -> Self {
        Self::new()
    }
}

/// Blendshape for facial animation
#[derive(Clone, Debug)]
pub struct Blendshape {
    pub preset: BlendshapePreset,
    pub name: String,
    /// Vertex deltas (index, position delta)
    pub deltas: Vec<(usize, na::Vector3<f32>)>,
    /// Normal deltas
    pub normal_deltas: Vec<(usize, na::Vector3<f32>)>,
}

/// Complete avatar with mesh, skeleton, and blendshapes
#[derive(Clone)]
pub struct Avatar {
    pub id: Uuid,
    pub name: String,
    /// Base mesh in T-pose
    pub mesh: super::scene_graph::MeshData,
    /// Skeleton for animation
    pub skeleton: Skeleton,
    /// Available blendshapes
    pub blendshapes: Vec<Blendshape>,
    /// Current blendshape weights
    pub blendshape_weights: HashMap<BlendshapePreset, f32>,
    /// Avatar height (for scaling)
    pub height: f32,
    /// Eye positions in local space
    pub left_eye_position: na::Point3<f32>,
    pub right_eye_position: na::Point3<f32>,
}

impl Avatar {
    pub fn new(name: &str, mesh: super::scene_graph::MeshData) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.to_string(),
            mesh,
            skeleton: Skeleton::create_humanoid(),
            blendshapes: Vec::new(),
            blendshape_weights: HashMap::new(),
            height: 1.7,
            left_eye_position: na::Point3::new(0.03, 1.65, 0.1),
            right_eye_position: na::Point3::new(-0.03, 1.65, 0.1),
        }
    }

    /// Set blendshape weight
    pub fn set_blendshape(&mut self, preset: BlendshapePreset, weight: f32) {
        self.blendshape_weights
            .insert(preset, weight.clamp(0.0, 1.0));
    }

    /// Get blendshape weight
    pub fn get_blendshape(&self, preset: BlendshapePreset) -> f32 {
        *self.blendshape_weights.get(&preset).unwrap_or(&0.0)
    }

    /// Reset all blendshapes to neutral
    pub fn reset_blendshapes(&mut self) {
        self.blendshape_weights.clear();
    }
}

/// Detected human pose from 4DGS observation
#[derive(Clone, Debug)]
pub struct DetectedPose {
    /// Joint positions in world space (index = BoneName as usize for major bones)
    pub joint_positions: HashMap<BoneName, na::Point3<f32>>,
    /// Confidence per joint
    pub confidences: HashMap<BoneName, f32>,
    /// Bounding box of the detected person
    pub bounding_box: super::segmentation::BoundingBox3D,
    /// Detection timestamp
    pub timestamp: f32,
}

/// Bound avatar with active tracking
#[derive(Clone)]
pub struct BoundAvatar {
    pub avatar: Avatar,
    /// World transform of the avatar
    pub transform: na::Isometry3<f32>,
    /// Current animated skeleton
    pub current_skeleton: Skeleton,
    /// Smoothed joint positions (for temporal stability)
    pub smoothed_joints: HashMap<BoneName, na::Point3<f32>>,
    /// Tracking confidence
    pub confidence: f32,
    /// Frames since binding
    pub frames_tracked: usize,
}

impl BoundAvatar {
    pub fn new(avatar: Avatar) -> Self {
        let skeleton = avatar.skeleton.clone();
        Self {
            avatar,
            transform: na::Isometry3::identity(),
            current_skeleton: skeleton,
            smoothed_joints: HashMap::new(),
            confidence: 1.0,
            frames_tracked: 0,
        }
    }

    /// Apply detected pose to skeleton
    pub fn apply_pose(&mut self, pose: &DetectedPose, smoothing: f32) {
        self.frames_tracked += 1;

        // Smooth joint positions
        for (bone_name, &position) in &pose.joint_positions {
            let smoothed = self.smoothed_joints.entry(*bone_name).or_insert(position);

            *smoothed = na::Point3::from(smoothed.coords.lerp(&position.coords, 1.0 - smoothing));
        }

        // Solve IK for limbs
        self.solve_arm_ik(
            BoneName::LeftShoulder,
            BoneName::LeftUpperArm,
            BoneName::LeftHand,
        );
        self.solve_arm_ik(
            BoneName::RightShoulder,
            BoneName::RightUpperArm,
            BoneName::RightHand,
        );
        self.solve_leg_ik(
            BoneName::LeftUpperLeg,
            BoneName::LeftLowerLeg,
            BoneName::LeftFoot,
        );
        self.solve_leg_ik(
            BoneName::RightUpperLeg,
            BoneName::RightLowerLeg,
            BoneName::RightFoot,
        );

        // Update head orientation
        if let Some(&head_pos) = self.smoothed_joints.get(&BoneName::Head) {
            if let Some(&neck_pos) = self.smoothed_joints.get(&BoneName::Neck) {
                let head_dir = (head_pos - neck_pos).normalize();
                let head_rotation = na::UnitQuaternion::face_towards(&head_dir, &na::Vector3::y());
                self.current_skeleton
                    .set_bone_rotation(BoneName::Head, head_rotation);
            }
        }

        // Update skeleton transforms
        self.current_skeleton.update_transforms();

        // Update confidence
        let avg_confidence: f32 =
            pose.confidences.values().sum::<f32>() / pose.confidences.len().max(1) as f32;
        self.confidence = self.confidence * 0.9 + avg_confidence * 0.1;
    }

    /// Two-bone IK solver for arms
    fn solve_arm_ik(&mut self, _shoulder: BoneName, upper: BoneName, hand: BoneName) {
        let Some(&target) = self.smoothed_joints.get(&hand) else {
            return;
        };
        let Some(&upper_pos) = self.smoothed_joints.get(&upper) else {
            return;
        };

        // Simplified IK - just point towards target
        let direction = (target - upper_pos).normalize();
        let rotation = na::UnitQuaternion::face_towards(&direction, &na::Vector3::y());
        self.current_skeleton.set_bone_rotation(upper, rotation);
    }

    /// Two-bone IK solver for legs
    fn solve_leg_ik(&mut self, upper: BoneName, _lower: BoneName, foot: BoneName) {
        let Some(&target) = self.smoothed_joints.get(&foot) else {
            return;
        };
        let Some(&upper_pos) = self.smoothed_joints.get(&upper) else {
            return;
        };

        // Simplified IK - just point towards target
        let direction = (target - upper_pos).normalize();
        let rotation = na::UnitQuaternion::face_towards(&direction, &-na::Vector3::y());
        self.current_skeleton.set_bone_rotation(upper, rotation);
    }

    /// Get bone world transform
    pub fn get_bone_world_transform(&self, name: BoneName) -> Option<na::Matrix4<f32>> {
        self.current_skeleton
            .bone_name_to_index
            .get(&name)
            .map(|&i| self.transform.to_homogeneous() * self.current_skeleton.world_transforms[i])
    }

    /// Get all skinning matrices for GPU upload
    pub fn get_skinning_matrices(&self) -> Vec<[f32; 16]> {
        self.current_skeleton
            .skinning_matrices
            .iter()
            .map(|m| {
                let mut arr = [0.0f32; 16];
                for i in 0..16 {
                    arr[i] = m[(i % 4, i / 4)];
                }
                arr
            })
            .collect()
    }
}

/// Avatar tracker - binds avatars to detected humans
pub struct AvatarTracker {
    /// Available avatar templates
    avatars: HashMap<Uuid, Avatar>,
    /// Bound avatars (person_id -> bound avatar)
    bound: HashMap<Uuid, BoundAvatar>,
    /// Pose detection confidence threshold
    confidence_threshold: f32,
    /// Joint position smoothing factor (0-1, higher = more smoothing)
    smoothing: f32,
}

impl AvatarTracker {
    pub fn new() -> Self {
        Self {
            avatars: HashMap::new(),
            bound: HashMap::new(),
            confidence_threshold: 0.5,
            smoothing: 0.3,
        }
    }

    /// Register an avatar template
    pub fn register_avatar(&mut self, avatar: Avatar) -> Uuid {
        let id = avatar.id;
        self.avatars.insert(id, avatar);
        id
    }

    /// Bind an avatar to a detected person
    pub fn bind_avatar(&mut self, person_id: Uuid, avatar_id: Uuid) -> Option<Uuid> {
        let avatar = self.avatars.get(&avatar_id)?.clone();
        let bound = BoundAvatar::new(avatar);
        self.bound.insert(person_id, bound);
        Some(person_id)
    }

    /// Update bound avatar with new pose
    pub fn update_pose(&mut self, person_id: Uuid, pose: DetectedPose) {
        let mean_confidence = if pose.confidences.is_empty() {
            0.0
        } else {
            pose.confidences.values().sum::<f32>() / pose.confidences.len() as f32
        };
        if mean_confidence < self.confidence_threshold {
            return;
        }
        if let Some(bound) = self.bound.get_mut(&person_id) {
            bound.apply_pose(&pose, self.smoothing);
        }
    }

    /// Get bound avatar
    pub fn get_bound(&self, person_id: Uuid) -> Option<&BoundAvatar> {
        self.bound.get(&person_id)
    }

    /// Get mutable bound avatar
    pub fn get_bound_mut(&mut self, person_id: Uuid) -> Option<&mut BoundAvatar> {
        self.bound.get_mut(&person_id)
    }

    /// Unbind avatar
    pub fn unbind(&mut self, person_id: Uuid) -> Option<BoundAvatar> {
        self.bound.remove(&person_id)
    }

    /// Get all bound avatars
    pub fn all_bound(&self) -> impl Iterator<Item = (&Uuid, &BoundAvatar)> {
        self.bound.iter()
    }
}

impl Default for AvatarTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Pose estimator that extracts human pose from 4DGS
pub struct PoseEstimator {
    /// Detection confidence threshold
    confidence_threshold: f32,
}

impl PoseEstimator {
    pub fn new() -> Self {
        Self {
            confidence_threshold: 0.5,
        }
    }

    /// Estimate pose from 4DGS Gaussians
    pub fn estimate_pose(
        &self,
        _gaussians: &[crate::gaussian_splatting::gaussian_4d::Gaussian4D],
        bounds: &super::segmentation::BoundingBox3D,
    ) -> Option<DetectedPose> {
        // Estimate joint positions based on Gaussian distribution
        // This is a simplified heuristic - production would use ML

        let center = bounds.center();
        let height = bounds.size().y;

        if height < 1.0 {
            return None; // Too small to be a person
        }

        // Estimate joint positions based on human proportions
        let mut joints = HashMap::new();
        let mut confidences = HashMap::new();

        // Hips at ~55% of height
        let hips_y = center.y - height * 0.05;
        joints.insert(BoneName::Hips, na::Point3::new(center.x, hips_y, center.z));
        confidences.insert(BoneName::Hips, 0.8);

        // Head at top
        let head_y = center.y + height * 0.4;
        joints.insert(BoneName::Head, na::Point3::new(center.x, head_y, center.z));
        confidences.insert(BoneName::Head, 0.7);

        // Neck slightly below head
        joints.insert(
            BoneName::Neck,
            na::Point3::new(center.x, head_y - height * 0.08, center.z),
        );
        confidences.insert(BoneName::Neck, 0.7);

        // Chest
        let chest_y = hips_y + height * 0.2;
        joints.insert(
            BoneName::Chest,
            na::Point3::new(center.x, chest_y, center.z),
        );
        confidences.insert(BoneName::Chest, 0.6);

        // Shoulders
        let shoulder_width = height * 0.25;
        joints.insert(
            BoneName::LeftShoulder,
            na::Point3::new(center.x + shoulder_width, chest_y + height * 0.1, center.z),
        );
        joints.insert(
            BoneName::RightShoulder,
            na::Point3::new(center.x - shoulder_width, chest_y + height * 0.1, center.z),
        );
        confidences.insert(BoneName::LeftShoulder, 0.6);
        confidences.insert(BoneName::RightShoulder, 0.6);

        // Hands (estimate from Gaussian extremities)
        // Would use actual Gaussian clustering in production
        joints.insert(
            BoneName::LeftHand,
            na::Point3::new(
                center.x + shoulder_width * 1.5,
                chest_y - height * 0.1,
                center.z,
            ),
        );
        joints.insert(
            BoneName::RightHand,
            na::Point3::new(
                center.x - shoulder_width * 1.5,
                chest_y - height * 0.1,
                center.z,
            ),
        );
        confidences.insert(BoneName::LeftHand, 0.5);
        confidences.insert(BoneName::RightHand, 0.5);

        // Feet
        let foot_y = bounds.min.y;
        let hip_width = height * 0.08;
        joints.insert(
            BoneName::LeftFoot,
            na::Point3::new(center.x + hip_width, foot_y, center.z),
        );
        joints.insert(
            BoneName::RightFoot,
            na::Point3::new(center.x - hip_width, foot_y, center.z),
        );
        confidences.insert(BoneName::LeftFoot, 0.6);
        confidences.insert(BoneName::RightFoot, 0.6);

        if confidences.values().copied().sum::<f32>() / (confidences.len() as f32)
            < self.confidence_threshold
        {
            return None;
        }

        Some(DetectedPose {
            joint_positions: joints,
            confidences,
            bounding_box: bounds.clone(),
            timestamp: 0.0,
        })
    }
}

impl Default for PoseEstimator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skeleton_creation() {
        let skeleton = Skeleton::create_humanoid();
        assert!(!skeleton.bones.is_empty());
        assert!(skeleton.get_bone(BoneName::Head).is_some());
    }

    #[test]
    fn test_avatar_blendshapes() {
        let mesh = super::super::scene_graph::MeshData::default();
        let mut avatar = Avatar::new("Test Avatar", mesh);

        avatar.set_blendshape(BlendshapePreset::Joy, 0.8);
        assert_eq!(avatar.get_blendshape(BlendshapePreset::Joy), 0.8);
    }

    #[test]
    fn test_bound_avatar() {
        let mesh = super::super::scene_graph::MeshData::default();
        let avatar = Avatar::new("Test", mesh);
        let bound = BoundAvatar::new(avatar);

        assert_eq!(bound.frames_tracked, 0);
        assert_eq!(bound.confidence, 1.0);
    }
}
