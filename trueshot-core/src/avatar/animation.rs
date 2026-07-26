//! Unified Avatar Animation System
//!
//! Real-time avatar animation using multiple input sources:
//! - Facial landmark detection (478-point MediaPipe)
//! - Body pose estimation
//! - IK solving for motion retargeting
//! - Blendshape driving from facial features
//!
//! Designed to work with both SMPL-X (avatar/mod.rs) and VRM (live_hybrid/avatar.rs)

use nalgebra as na;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// ARKit-Compatible Blendshapes (52 parameters)
// ============================================================================

/// ARKit-compatible facial blendshape parameters
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ARKitBlendshapes {
    // Eye
    pub eye_blink_left: f32,
    pub eye_blink_right: f32,
    pub eye_look_down_left: f32,
    pub eye_look_down_right: f32,
    pub eye_look_in_left: f32,
    pub eye_look_in_right: f32,
    pub eye_look_out_left: f32,
    pub eye_look_out_right: f32,
    pub eye_look_up_left: f32,
    pub eye_look_up_right: f32,
    pub eye_squint_left: f32,
    pub eye_squint_right: f32,
    pub eye_wide_left: f32,
    pub eye_wide_right: f32,

    // Eyebrow
    pub brow_down_left: f32,
    pub brow_down_right: f32,
    pub brow_inner_up: f32,
    pub brow_outer_up_left: f32,
    pub brow_outer_up_right: f32,

    // Nose
    pub nose_sneer_left: f32,
    pub nose_sneer_right: f32,

    // Cheek
    pub cheek_puff: f32,
    pub cheek_squint_left: f32,
    pub cheek_squint_right: f32,

    // Mouth
    pub mouth_close: f32,
    pub mouth_funnel: f32,
    pub mouth_pucker: f32,
    pub mouth_left: f32,
    pub mouth_right: f32,
    pub mouth_smile_left: f32,
    pub mouth_smile_right: f32,
    pub mouth_frown_left: f32,
    pub mouth_frown_right: f32,
    pub mouth_dimple_left: f32,
    pub mouth_dimple_right: f32,
    pub mouth_stretch_left: f32,
    pub mouth_stretch_right: f32,
    pub mouth_roll_lower: f32,
    pub mouth_roll_upper: f32,
    pub mouth_shrug_lower: f32,
    pub mouth_shrug_upper: f32,
    pub mouth_press_left: f32,
    pub mouth_press_right: f32,
    pub mouth_lower_down_left: f32,
    pub mouth_lower_down_right: f32,
    pub mouth_upper_up_left: f32,
    pub mouth_upper_up_right: f32,

    // Jaw
    pub jaw_forward: f32,
    pub jaw_left: f32,
    pub jaw_right: f32,
    pub jaw_open: f32,

    // Tongue
    pub tongue_out: f32,
}

impl ARKitBlendshapes {
    /// Convert to array of 52 values
    pub fn to_array(&self) -> [f32; 52] {
        [
            self.eye_blink_left,
            self.eye_blink_right,
            self.eye_look_down_left,
            self.eye_look_down_right,
            self.eye_look_in_left,
            self.eye_look_in_right,
            self.eye_look_out_left,
            self.eye_look_out_right,
            self.eye_look_up_left,
            self.eye_look_up_right,
            self.eye_squint_left,
            self.eye_squint_right,
            self.eye_wide_left,
            self.eye_wide_right,
            self.brow_down_left,
            self.brow_down_right,
            self.brow_inner_up,
            self.brow_outer_up_left,
            self.brow_outer_up_right,
            self.nose_sneer_left,
            self.nose_sneer_right,
            self.cheek_puff,
            self.cheek_squint_left,
            self.cheek_squint_right,
            self.mouth_close,
            self.mouth_funnel,
            self.mouth_pucker,
            self.mouth_left,
            self.mouth_right,
            self.mouth_smile_left,
            self.mouth_smile_right,
            self.mouth_frown_left,
            self.mouth_frown_right,
            self.mouth_dimple_left,
            self.mouth_dimple_right,
            self.mouth_stretch_left,
            self.mouth_stretch_right,
            self.mouth_roll_lower,
            self.mouth_roll_upper,
            self.mouth_shrug_lower,
            self.mouth_shrug_upper,
            self.mouth_press_left,
            self.mouth_press_right,
            self.mouth_lower_down_left,
            self.mouth_lower_down_right,
            self.mouth_upper_up_left,
            self.mouth_upper_up_right,
            self.jaw_forward,
            self.jaw_left,
            self.jaw_right,
            self.jaw_open,
            self.tongue_out,
        ]
    }

    /// Create from array of 52 values
    pub fn from_array(arr: &[f32; 52]) -> Self {
        Self {
            eye_blink_left: arr[0],
            eye_blink_right: arr[1],
            eye_look_down_left: arr[2],
            eye_look_down_right: arr[3],
            eye_look_in_left: arr[4],
            eye_look_in_right: arr[5],
            eye_look_out_left: arr[6],
            eye_look_out_right: arr[7],
            eye_look_up_left: arr[8],
            eye_look_up_right: arr[9],
            eye_squint_left: arr[10],
            eye_squint_right: arr[11],
            eye_wide_left: arr[12],
            eye_wide_right: arr[13],
            brow_down_left: arr[14],
            brow_down_right: arr[15],
            brow_inner_up: arr[16],
            brow_outer_up_left: arr[17],
            brow_outer_up_right: arr[18],
            nose_sneer_left: arr[19],
            nose_sneer_right: arr[20],
            cheek_puff: arr[21],
            cheek_squint_left: arr[22],
            cheek_squint_right: arr[23],
            mouth_close: arr[24],
            mouth_funnel: arr[25],
            mouth_pucker: arr[26],
            mouth_left: arr[27],
            mouth_right: arr[28],
            mouth_smile_left: arr[29],
            mouth_smile_right: arr[30],
            mouth_frown_left: arr[31],
            mouth_frown_right: arr[32],
            mouth_dimple_left: arr[33],
            mouth_dimple_right: arr[34],
            mouth_stretch_left: arr[35],
            mouth_stretch_right: arr[36],
            mouth_roll_lower: arr[37],
            mouth_roll_upper: arr[38],
            mouth_shrug_lower: arr[39],
            mouth_shrug_upper: arr[40],
            mouth_press_left: arr[41],
            mouth_press_right: arr[42],
            mouth_lower_down_left: arr[43],
            mouth_lower_down_right: arr[44],
            mouth_upper_up_left: arr[45],
            mouth_upper_up_right: arr[46],
            jaw_forward: arr[47],
            jaw_left: arr[48],
            jaw_right: arr[49],
            jaw_open: arr[50],
            tongue_out: arr[51],
        }
    }

    /// Blend with another set
    pub fn blend(&self, other: &Self, t: f32) -> Self {
        let a = self.to_array();
        let b = other.to_array();
        let mut result = [0.0f32; 52];
        for i in 0..52 {
            result[i] = a[i] + (b[i] - a[i]) * t;
        }
        Self::from_array(&result)
    }
}

// ============================================================================
// Inverse Kinematics
// ============================================================================

/// IK target for a bone chain
#[derive(Clone, Debug)]
pub struct IKTarget {
    /// Target position in world space
    pub position: na::Point3<f32>,
    /// Target rotation (optional)
    pub rotation: Option<na::UnitQuaternion<f32>>,
    /// Weight (0-1)
    pub weight: f32,
    /// Pole vector for elbow/knee direction
    pub pole_vector: Option<na::Vector3<f32>>,
}

/// IK chain definition
#[derive(Clone, Debug)]
pub struct IKChain {
    /// Bone indices in the chain (root to end effector)
    pub bone_indices: Vec<usize>,
    /// Chain type
    pub chain_type: IKChainType,
    /// Length of each bone
    pub bone_lengths: Vec<f32>,
    /// Rotation limits per bone (min, max for each axis)
    pub rotation_limits: Vec<[(f32, f32); 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IKChainType {
    /// Simple two-bone IK (arm, leg)
    TwoBone,
    /// FABRIK for longer chains (spine, fingers)
    FABRIK,
    /// CCD for complex chains
    CCD,
    /// Look-at constraint (head, eyes)
    LookAt,
}

/// IK solver configuration
#[derive(Clone, Debug)]
pub struct IKSolverConfig {
    /// Maximum iterations for iterative solvers
    pub max_iterations: usize,
    /// Convergence threshold
    pub tolerance: f32,
    /// Damping factor for CCD
    pub damping: f32,
}

impl Default for IKSolverConfig {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            tolerance: 0.001,
            damping: 0.5,
        }
    }
}

/// Unified IK solver
pub struct IKSolver {
    config: IKSolverConfig,
    chains: HashMap<String, IKChain>,
}

impl IKSolver {
    pub fn new(config: IKSolverConfig) -> Self {
        Self {
            config,
            chains: HashMap::new(),
        }
    }

    /// Add an IK chain
    pub fn add_chain(&mut self, name: &str, chain: IKChain) {
        self.chains.insert(name.to_string(), chain);
    }

    /// Solve IK for a chain
    pub fn solve(
        &self,
        chain_name: &str,
        bone_positions: &mut [na::Point3<f32>],
        bone_rotations: &mut [na::UnitQuaternion<f32>],
        target: &IKTarget,
    ) -> bool {
        let chain = match self.chains.get(chain_name) {
            Some(c) => c,
            None => return false,
        };

        if target.weight <= 0.0 {
            return true;
        }

        match chain.chain_type {
            IKChainType::TwoBone => {
                self.solve_two_bone(chain, bone_positions, bone_rotations, target)
            }
            IKChainType::FABRIK => self.solve_fabrik(chain, bone_positions, target),
            IKChainType::CCD => self.solve_ccd(chain, bone_positions, bone_rotations, target),
            IKChainType::LookAt => self.solve_look_at(chain, bone_rotations, target),
        }
    }

    /// Two-bone IK (analytical solution for arms/legs)
    fn solve_two_bone(
        &self,
        chain: &IKChain,
        positions: &mut [na::Point3<f32>],
        rotations: &mut [na::UnitQuaternion<f32>],
        target: &IKTarget,
    ) -> bool {
        if chain.bone_indices.len() < 3 || chain.bone_lengths.len() < 2 {
            return false;
        }

        let root_idx = chain.bone_indices[0];
        let mid_idx = chain.bone_indices[1];
        let end_idx = chain.bone_indices[2];

        let root = positions[root_idx];
        let len_a = chain.bone_lengths[0];
        let len_b = chain.bone_lengths[1];

        // Distance to target
        let target_vec = target.position - root;
        let target_dist = target_vec.norm().min(len_a + len_b - 0.001);

        if target_dist < 0.001 {
            return true;
        }

        // Law of cosines for middle joint angle
        let cos_angle = ((len_a * len_a + len_b * len_b - target_dist * target_dist)
            / (2.0 * len_a * len_b))
            .clamp(-1.0, 1.0);
        let mid_angle = std::f32::consts::PI - cos_angle.acos();

        // Angle at root
        let cos_root = ((len_a * len_a + target_dist * target_dist - len_b * len_b)
            / (2.0 * len_a * target_dist))
            .clamp(-1.0, 1.0);
        let _root_angle = cos_root.acos();

        // Calculate new positions
        let forward = target_vec.normalize();
        let pole = chain
            .bone_lengths
            .first()
            .map(|_| target.pole_vector.unwrap_or(na::Vector3::y()))
            .unwrap_or(na::Vector3::y());

        let right = forward.cross(&pole).normalize();
        let up = right.cross(&forward);

        // Mid joint position
        let mid_forward = forward * (mid_angle / 2.0).cos() + up * (mid_angle / 2.0).sin();
        positions[mid_idx] = root + mid_forward * len_a;

        // End effector position
        let end_dir = (target.position - positions[mid_idx]).normalize();
        positions[end_idx] = positions[mid_idx] + end_dir * len_b;

        true
    }

    /// FABRIK solver for general chains
    fn solve_fabrik(
        &self,
        chain: &IKChain,
        positions: &mut [na::Point3<f32>],
        target: &IKTarget,
    ) -> bool {
        let n = chain.bone_indices.len();
        if n < 2 {
            return false;
        }

        let root = positions[chain.bone_indices[0]];

        for _ in 0..self.config.max_iterations {
            // Forward reaching (from end effector to root)
            positions[chain.bone_indices[n - 1]] = target.position;

            for i in (1..n).rev() {
                let curr_idx = chain.bone_indices[i];
                let prev_idx = chain.bone_indices[i - 1];
                let len = chain.bone_lengths[i - 1];

                let dir = (positions[prev_idx] - positions[curr_idx]).normalize();
                positions[prev_idx] = positions[curr_idx] + dir * len;
            }

            // Backward reaching (from root to end effector)
            positions[chain.bone_indices[0]] = root;

            for i in 0..n - 1 {
                let curr_idx = chain.bone_indices[i];
                let next_idx = chain.bone_indices[i + 1];
                let len = chain.bone_lengths[i];

                let dir = (positions[next_idx] - positions[curr_idx]).normalize();
                positions[next_idx] = positions[curr_idx] + dir * len;
            }

            // Check convergence
            let end_pos = positions[chain.bone_indices[n - 1]];
            if na::distance(&end_pos, &target.position) < self.config.tolerance {
                return true;
            }
        }

        true
    }

    /// CCD (Cyclic Coordinate Descent) solver
    fn solve_ccd(
        &self,
        chain: &IKChain,
        positions: &mut [na::Point3<f32>],
        rotations: &mut [na::UnitQuaternion<f32>],
        target: &IKTarget,
    ) -> bool {
        let n = chain.bone_indices.len();
        if n < 2 {
            return false;
        }

        for _ in 0..self.config.max_iterations {
            // Iterate from end effector parent to root
            for i in (0..n - 1).rev() {
                let bone_idx = chain.bone_indices[i];
                let end_idx = chain.bone_indices[n - 1];

                let bone_pos = positions[bone_idx];
                let end_pos = positions[end_idx];

                // Vector from bone to end effector
                let to_end = end_pos - bone_pos;
                // Vector from bone to target
                let to_target = target.position - bone_pos;

                if to_end.norm() < 0.001 || to_target.norm() < 0.001 {
                    continue;
                }

                // Rotation to align end with target
                let rotation = na::UnitQuaternion::rotation_between(&to_end, &to_target)
                    .unwrap_or(na::UnitQuaternion::identity());

                // Apply damped rotation
                let damped = na::UnitQuaternion::identity().slerp(&rotation, self.config.damping);
                rotations[bone_idx] = damped * rotations[bone_idx];

                // Update positions along chain
                for j in i + 1..n {
                    let prev_idx = chain.bone_indices[j - 1];
                    let curr_idx = chain.bone_indices[j];
                    let len = chain.bone_lengths[j - 1];

                    let dir = rotations[prev_idx] * na::Vector3::y();
                    positions[curr_idx] = positions[prev_idx] + dir * len;
                }
            }

            // Check convergence
            let end_pos = positions[chain.bone_indices[n - 1]];
            if na::distance(&end_pos, &target.position) < self.config.tolerance {
                return true;
            }
        }

        true
    }

    /// Look-at constraint
    fn solve_look_at(
        &self,
        chain: &IKChain,
        rotations: &mut [na::UnitQuaternion<f32>],
        target: &IKTarget,
    ) -> bool {
        if chain.bone_indices.is_empty() {
            return false;
        }

        let bone_idx = chain.bone_indices[0];

        if let Some(target_rot) = target.rotation {
            rotations[bone_idx] = rotations[bone_idx].slerp(&target_rot, target.weight);
        }

        true
    }
}

impl Default for IKSolver {
    fn default() -> Self {
        Self::new(IKSolverConfig::default())
    }
}

// ============================================================================
// Animation Retargeting
// ============================================================================

/// Bone mapping for retargeting between skeletons
#[derive(Clone, Debug)]
pub struct BoneMapping {
    /// Source bone name
    pub source: String,
    /// Target bone name
    pub target: String,
    /// Rotation offset
    pub rotation_offset: na::UnitQuaternion<f32>,
    /// Scale factor
    pub scale: f32,
}

/// Animation retargeting system
pub struct AnimationRetargeter {
    mappings: Vec<BoneMapping>,
    source_rest_pose: HashMap<String, na::UnitQuaternion<f32>>,
    target_rest_pose: HashMap<String, na::UnitQuaternion<f32>>,
}

impl AnimationRetargeter {
    pub fn new() -> Self {
        Self {
            mappings: Vec::new(),
            source_rest_pose: HashMap::new(),
            target_rest_pose: HashMap::new(),
        }
    }

    /// Add a bone mapping
    pub fn add_mapping(&mut self, mapping: BoneMapping) {
        self.mappings.push(mapping);
    }

    /// Set source rest pose
    pub fn set_source_rest(&mut self, bone: &str, rotation: na::UnitQuaternion<f32>) {
        self.source_rest_pose.insert(bone.to_string(), rotation);
    }

    /// Set target rest pose
    pub fn set_target_rest(&mut self, bone: &str, rotation: na::UnitQuaternion<f32>) {
        self.target_rest_pose.insert(bone.to_string(), rotation);
    }

    /// Retarget a pose from source to target skeleton
    pub fn retarget(
        &self,
        source_rotations: &HashMap<String, na::UnitQuaternion<f32>>,
    ) -> HashMap<String, na::UnitQuaternion<f32>> {
        let mut target_rotations = HashMap::new();

        for mapping in &self.mappings {
            if let Some(source_rot) = source_rotations.get(&mapping.source) {
                // Get rest poses
                let source_rest = self
                    .source_rest_pose
                    .get(&mapping.source)
                    .copied()
                    .unwrap_or(na::UnitQuaternion::identity());
                let target_rest = self
                    .target_rest_pose
                    .get(&mapping.target)
                    .copied()
                    .unwrap_or(na::UnitQuaternion::identity());

                // Compute relative rotation from rest
                let relative = source_rest.inverse() * source_rot;

                // Apply to target with mapping offset
                let target_rot = target_rest * mapping.rotation_offset * relative;

                target_rotations.insert(mapping.target.clone(), target_rot);
            }
        }

        target_rotations
    }

    /// Create standard VRM to SMPL-X mapping
    pub fn create_vrm_to_smplx_mapping() -> Self {
        let mut retargeter = Self::new();

        // Core body mappings
        let mappings = [
            ("Hips", "pelvis"),
            ("Spine", "spine1"),
            ("Chest", "spine2"),
            ("UpperChest", "spine3"),
            ("Neck", "neck"),
            ("Head", "head"),
            ("LeftShoulder", "left_collar"),
            ("LeftUpperArm", "left_shoulder"),
            ("LeftLowerArm", "left_elbow"),
            ("LeftHand", "left_wrist"),
            ("RightShoulder", "right_collar"),
            ("RightUpperArm", "right_shoulder"),
            ("RightLowerArm", "right_elbow"),
            ("RightHand", "right_wrist"),
            ("LeftUpperLeg", "left_hip"),
            ("LeftLowerLeg", "left_knee"),
            ("LeftFoot", "left_ankle"),
            ("RightUpperLeg", "right_hip"),
            ("RightLowerLeg", "right_knee"),
            ("RightFoot", "right_ankle"),
        ];

        for (source, target) in mappings {
            retargeter.add_mapping(BoneMapping {
                source: source.to_string(),
                target: target.to_string(),
                rotation_offset: na::UnitQuaternion::identity(),
                scale: 1.0,
            });
        }

        retargeter
    }
}

impl Default for AnimationRetargeter {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Real-time Animation Driver
// ============================================================================

/// Animation driver that combines multiple input sources
pub struct AnimationDriver {
    /// IK solver for body
    pub ik_solver: IKSolver,
    /// Animation retargeter
    pub retargeter: AnimationRetargeter,
    /// Blendshape smoothing (EMA alpha)
    pub blendshape_smoothing: f32,
    /// Previous blendshapes for smoothing
    prev_blendshapes: ARKitBlendshapes,
    /// Bone rotation cache
    bone_rotations: HashMap<String, na::UnitQuaternion<f32>>,
}

impl AnimationDriver {
    pub fn new() -> Self {
        Self {
            ik_solver: IKSolver::default(),
            retargeter: AnimationRetargeter::default(),
            blendshape_smoothing: 0.3,
            prev_blendshapes: ARKitBlendshapes::default(),
            bone_rotations: HashMap::new(),
        }
    }

    /// Update blendshapes with input from facial tracking
    pub fn update_blendshapes(&mut self, raw_blendshapes: &ARKitBlendshapes) -> ARKitBlendshapes {
        // Apply EMA smoothing
        let smoothed = self
            .prev_blendshapes
            .blend(raw_blendshapes, self.blendshape_smoothing);
        self.prev_blendshapes = smoothed.clone();
        smoothed
    }

    /// Update bone rotations
    pub fn update_bone_rotation(&mut self, bone: &str, rotation: na::UnitQuaternion<f32>) {
        self.bone_rotations.insert(bone.to_string(), rotation);
    }

    /// Get current bone rotations
    pub fn get_bone_rotations(&self) -> &HashMap<String, na::UnitQuaternion<f32>> {
        &self.bone_rotations
    }

    /// Apply IK target
    pub fn apply_ik(
        &self,
        chain_name: &str,
        positions: &mut [na::Point3<f32>],
        rotations: &mut [na::UnitQuaternion<f32>],
        target: &IKTarget,
    ) -> bool {
        self.ik_solver
            .solve(chain_name, positions, rotations, target)
    }
}

impl Default for AnimationDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blendshape_array_conversion() {
        let bs = ARKitBlendshapes {
            eye_blink_left: 0.5,
            mouth_smile_left: 0.8,
            ..ARKitBlendshapes::default()
        };

        let arr = bs.to_array();
        let bs2 = ARKitBlendshapes::from_array(&arr);

        assert_eq!(bs2.eye_blink_left, 0.5);
        assert_eq!(bs2.mouth_smile_left, 0.8);
    }

    #[test]
    fn test_blendshape_blend() {
        let a = ARKitBlendshapes {
            eye_blink_left: 0.0,
            ..Default::default()
        };
        let b = ARKitBlendshapes {
            eye_blink_left: 1.0,
            ..Default::default()
        };

        let blended = a.blend(&b, 0.5);
        assert!((blended.eye_blink_left - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_ik_chain_creation() {
        let mut solver = IKSolver::default();

        solver.add_chain(
            "left_arm",
            IKChain {
                bone_indices: vec![0, 1, 2],
                chain_type: IKChainType::TwoBone,
                bone_lengths: vec![0.3, 0.25],
                rotation_limits: vec![],
            },
        );

        assert!(solver.chains.contains_key("left_arm"));
    }
}
