//! Live Hybrid Pipeline
//!
//! End-to-end pipeline orchestrating:
//! - Object segmentation and tracking
//! - Motion scoring and classification
//! - Progressive meshification for static objects
//! - Avatar binding for humans
//! - Unified rendering
//! - Efficient streaming

use std::collections::HashMap;
use std::time::Instant;
use uuid::Uuid;

use crate::gaussian_splatting::gaussian_4d::{Dynamic4DScene, Gaussian4D};

use super::avatar::{Avatar, AvatarTracker, BlendshapePreset, PoseEstimator};
use super::meshification::{MeshificationConfig, MeshificationPipeline};
use super::motion_score::{MotionClassification, MotionScorer, MotionScorerConfig};
use super::scene_graph::{HybridScene, HybridSceneNode, ObjectRepresentation};
use super::segmentation::{ObjectSegmenter, TrackedObject};
use super::streaming::{EncoderConfig, StreamEncoder, StreamPacket};
use super::transitions::TransitionManager;
use super::unified_renderer::{
    UnifiedCamera, UnifiedFrame, UnifiedRenderer, UnifiedRendererConfig,
};

/// Pipeline configuration
#[derive(Clone, Debug)]
pub struct LiveHybridConfig {
    /// Motion scoring configuration
    pub motion: MotionScorerConfig,
    /// Meshification configuration
    pub meshification: MeshificationConfig,
    /// Renderer configuration
    pub renderer: UnifiedRendererConfig,
    /// Streaming configuration
    pub streaming: EncoderConfig,
    /// Enable automatic meshification
    pub auto_meshify: bool,
    /// Enable avatar detection and binding
    pub enable_avatars: bool,
    /// Minimum height to consider as human (meters)
    pub human_min_height: f32,
    /// Object segmentation distance threshold
    pub segmentation_distance: f32,
}

impl Default for LiveHybridConfig {
    fn default() -> Self {
        Self {
            motion: MotionScorerConfig::default(),
            meshification: MeshificationConfig::default(),
            renderer: UnifiedRendererConfig::default(),
            streaming: EncoderConfig::default(),
            auto_meshify: true,
            enable_avatars: true,
            human_min_height: 1.4,
            segmentation_distance: 0.5,
        }
    }
}

/// Pipeline statistics
#[derive(Clone, Debug, Default)]
pub struct PipelineStats {
    pub frames_processed: u64,
    pub objects_tracked: usize,
    pub gaussians_processed: u64,
    pub meshes_created: u32,
    pub avatars_bound: usize,
    pub stream_bytes_sent: u64,
    pub avg_frame_time_ms: f32,
    pub last_frame_time_ms: f32,
}

/// Main Live Hybrid Pipeline
pub struct LiveHybridPipeline {
    config: LiveHybridConfig,

    // Core components
    scene: HybridScene,
    segmenter: ObjectSegmenter,
    motion_scorer: MotionScorer,
    meshifier: MeshificationPipeline,
    avatar_tracker: AvatarTracker,
    pose_estimator: PoseEstimator,
    transitions: TransitionManager,
    renderer: UnifiedRenderer,
    encoder: StreamEncoder,

    // State
    tracked_objects: Vec<TrackedObject>,
    previous_gaussians: HashMap<Uuid, Vec<Gaussian4D>>,
    frame_count: u64,
    stats: PipelineStats,

    // Timing
    frame_start: Option<Instant>,
    frame_times: Vec<f32>,
}

impl LiveHybridPipeline {
    pub fn new(config: LiveHybridConfig) -> Self {
        let segmenter = ObjectSegmenter::new(100, config.segmentation_distance, 50);

        Self {
            scene: HybridScene::new("LiveHybrid Scene"),
            segmenter,
            motion_scorer: MotionScorer::new(config.motion.clone()),
            meshifier: MeshificationPipeline::new(config.meshification.clone()),
            avatar_tracker: AvatarTracker::new(),
            pose_estimator: PoseEstimator::new(),
            transitions: TransitionManager::new(),
            renderer: UnifiedRenderer::new(config.renderer.clone()),
            encoder: StreamEncoder::new(config.streaming.clone()),
            tracked_objects: Vec::new(),
            previous_gaussians: HashMap::new(),
            frame_count: 0,
            stats: PipelineStats::default(),
            frame_start: None,
            frame_times: Vec::with_capacity(100),
            config,
        }
    }

    /// Process a new frame from the 4DGS source
    pub fn process_frame(&mut self, source_4dgs: &Dynamic4DScene, _time: f32) {
        self.frame_start = Some(Instant::now());
        self.frame_count += 1;

        // 1. Segment scene into objects
        let segments = self.segmenter.segment_scene(source_4dgs);

        // 2. Track objects across frames
        self.segmenter
            .track_objects(&mut self.tracked_objects, segments, source_4dgs);

        // 3. Score motion and classify each object - collect updates first
        let mut updates: Vec<(Uuid, Vec<Gaussian4D>, f32, MotionClassification, Vec<usize>)> =
            Vec::new();

        for tracked in &self.tracked_objects {
            let current_gaussians: Vec<_> = tracked
                .current
                .gaussian_indices
                .iter()
                .filter_map(|&i| source_4dgs.gaussians.get(i))
                .cloned()
                .collect();

            let prev = self
                .previous_gaussians
                .get(&tracked.id)
                .cloned()
                .unwrap_or_default();

            let score = self.motion_scorer.compute_score(&prev, &current_gaussians);
            let classification = self.motion_scorer.classify(score);

            updates.push((
                tracked.id,
                current_gaussians,
                score,
                classification,
                tracked.current.gaussian_indices.clone(),
            ));
        }

        // 4. Apply updates
        for (id, gaussians, score, classification, indices) in updates {
            self.update_or_create_node_by_id(
                id,
                &gaussians,
                score,
                classification,
                source_4dgs,
                &indices,
            );
            self.previous_gaussians.insert(id, gaussians);
        }

        // 5. Process meshification queue
        if self.config.auto_meshify {
            self.process_meshification();
        }

        // 6. Update avatar tracking
        if self.config.enable_avatars {
            self.update_avatars(source_4dgs);
        }

        // 7. Complete transitions
        let completed = self.transitions.update();
        for (id, final_repr) in completed {
            self.scene.update_representation(id, final_repr);
        }

        // Update stats
        self.update_stats();
    }

    /// Update or create scene node by ID
    fn update_or_create_node_by_id(
        &mut self,
        node_id: Uuid,
        gaussians: &[Gaussian4D],
        motion_score: f32,
        classification: MotionClassification,
        source_4dgs: &Dynamic4DScene,
        gaussian_indices: &[usize],
    ) {
        // Get bounding box from tracked objects
        let bounding_box = self
            .tracked_objects
            .iter()
            .find(|t| t.id == node_id)
            .map(|t| t.current.bounding_box.clone());

        if let Some(node) = self.scene.get_node_mut(node_id) {
            // Update existing node
            node.update_motion_score(motion_score);

            // Handle classification changes
            match (&node.representation, classification) {
                (ObjectRepresentation::Gaussian4D { .. }, MotionClassification::Static) => {
                    // Queue for meshification if stable
                    if node.is_stable_for(self.config.meshification.min_stable_frames) {
                        if let Some(bbox) = bounding_box {
                            self.meshifier.queue(node_id, gaussians.to_vec(), bbox);
                        }
                    }
                }
                (ObjectRepresentation::Mesh { .. }, MotionClassification::Dynamic) => {
                    // Object started moving - transition back to Gaussians
                    let mesh_repr = node.representation.clone();
                    let gaussian_repr = ObjectRepresentation::Gaussian4D {
                        scene: source_4dgs.clone(),
                        gaussian_indices: gaussian_indices.to_vec(),
                    };
                    self.transitions
                        .start_transition(node_id, mesh_repr, gaussian_repr);
                }
                _ => {}
            }
        } else {
            // Create new node
            let repr = ObjectRepresentation::Gaussian4D {
                scene: source_4dgs.clone(),
                gaussian_indices: gaussian_indices.to_vec(),
            };

            let mut node = HybridSceneNode::new(&format!("Object_{}", node_id.simple()), repr);
            node.id = node_id;
            node.motion_score = motion_score;

            self.scene.add_node(node);
        }
    }

    /// Process pending meshification jobs
    fn process_meshification(&mut self) {
        let results = self.meshifier.process();

        for result in results {
            self.stats.meshes_created += 1;

            // Get current representation for transition
            if let Some(node) = self.scene.get_node(result.object_id) {
                let from = node.representation.clone();
                let to = ObjectRepresentation::Mesh {
                    geometry: result.mesh,
                    texture_id: None, // Would store texture separately
                    lod_levels: result.lod_levels,
                };

                self.transitions
                    .start_transition(result.object_id, from, to);
            }
        }
    }

    /// Update avatar tracking
    fn update_avatars(&mut self, source_4dgs: &Dynamic4DScene) {
        for tracked in &self.tracked_objects {
            let height = tracked.current.bounding_box.size().y;

            // Check if object is human-sized
            if height >= self.config.human_min_height && height <= 2.5 {
                // Estimate pose
                if let Some(pose) = self
                    .pose_estimator
                    .estimate_pose(&source_4dgs.gaussians, &tracked.current.bounding_box)
                {
                    // Update or bind avatar
                    if self.avatar_tracker.get_bound(tracked.id).is_some() {
                        self.avatar_tracker.update_pose(tracked.id, pose);
                    }
                    // Note: Avatar binding would typically be triggered explicitly
                }
            }

            if let Some(bound) = self.avatar_tracker.get_bound(tracked.id) {
                let mut transform = super::scene_graph::Transform3D::default();
                transform.position = tracked.current.centroid.coords;

                let bone_transforms = bound.current_skeleton.skinning_matrices.clone();
                let blendshape_weights = collect_blendshape_weights(&bound.avatar);
                let geometry = bound.avatar.mesh.clone();

                let repr = ObjectRepresentation::Avatar {
                    avatar_id: bound.avatar.id,
                    geometry,
                    bone_transforms,
                    blendshape_weights,
                };

                if let Some(node) = self.scene.get_node_mut(tracked.id) {
                    node.representation = repr;
                    node.transform = transform;
                    node.last_updated = Instant::now();
                } else {
                    let mut node =
                        HybridSceneNode::new(&format!("Avatar_{}", tracked.id.simple()), repr);
                    node.id = tracked.id;
                    node.transform = transform;
                    self.scene.add_node(node);
                }
            }
        }

        self.stats.avatars_bound = self.avatar_tracker.all_bound().count();
    }

    /// Render the current scene
    pub fn render(&mut self, camera: &UnifiedCamera, time: f32) -> UnifiedFrame {
        self.renderer.render(&self.scene, camera, time)
    }

    /// Get stream packets for current frame
    pub fn get_stream_packets(&mut self) -> Vec<StreamPacket> {
        let mut packets = Vec::new();

        // Encode transform updates for all nodes
        let transforms: Vec<_> = self
            .scene
            .nodes()
            .map(|n| (n.id, n.transform.clone()))
            .collect();

        if !transforms.is_empty() {
            packets.push(
                self.encoder
                    .encode_transforms(transforms, self.frame_count as f32 / 30.0),
            );
        }

        // Encode avatar poses
        for (person_id, bound) in self.avatar_tracker.all_bound() {
            let matrices = bound.get_skinning_matrices();
            let blendshapes: Vec<_> = bound
                .avatar
                .blendshape_weights
                .iter()
                .map(|(&k, &v)| (k, v))
                .collect();

            packets.push(
                self.encoder
                    .encode_avatar_pose(*person_id, &matrices, &blendshapes),
            );
        }

        self.stats.stream_bytes_sent = self.encoder.stats().bytes_sent;

        packets
    }

    /// Update statistics
    fn update_stats(&mut self) {
        if let Some(start) = self.frame_start {
            let frame_time = start.elapsed().as_secs_f32() * 1000.0;
            self.stats.last_frame_time_ms = frame_time;

            self.frame_times.push(frame_time);
            if self.frame_times.len() > 100 {
                self.frame_times.remove(0);
            }

            self.stats.avg_frame_time_ms =
                self.frame_times.iter().sum::<f32>() / self.frame_times.len() as f32;
        }

        self.stats.frames_processed = self.frame_count;
        self.stats.objects_tracked = self.tracked_objects.len();
    }

    /// Get pipeline statistics
    pub fn stats(&self) -> &PipelineStats {
        &self.stats
    }

    /// Get scene reference
    pub fn scene(&self) -> &HybridScene {
        &self.scene
    }

    /// Get mutable scene reference
    pub fn scene_mut(&mut self) -> &mut HybridScene {
        &mut self.scene
    }

    /// Register an avatar template
    pub fn register_avatar(&mut self, avatar: Avatar) -> Uuid {
        self.avatar_tracker.register_avatar(avatar)
    }

    /// Bind avatar to a person
    pub fn bind_avatar(&mut self, person_id: Uuid, avatar_id: Uuid) -> Option<Uuid> {
        self.avatar_tracker.bind_avatar(person_id, avatar_id)
    }

    /// Get tracked objects
    pub fn tracked_objects(&self) -> &[TrackedObject] {
        &self.tracked_objects
    }
}

impl Default for LiveHybridPipeline {
    fn default() -> Self {
        Self::new(LiveHybridConfig::default())
    }
}

fn collect_blendshape_weights(avatar: &Avatar) -> Vec<f32> {
    let order = [
        BlendshapePreset::Neutral,
        BlendshapePreset::Joy,
        BlendshapePreset::Angry,
        BlendshapePreset::Sorrow,
        BlendshapePreset::Fun,
        BlendshapePreset::Surprised,
        BlendshapePreset::Aa,
        BlendshapePreset::Ih,
        BlendshapePreset::Ou,
        BlendshapePreset::Ee,
        BlendshapePreset::Oh,
        BlendshapePreset::Blink,
        BlendshapePreset::BlinkLeft,
        BlendshapePreset::BlinkRight,
        BlendshapePreset::LookUp,
        BlendshapePreset::LookDown,
        BlendshapePreset::LookLeft,
        BlendshapePreset::LookRight,
    ];
    order
        .iter()
        .map(|preset| avatar.get_blendshape(*preset))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_creation() {
        let pipeline = LiveHybridPipeline::default();
        assert_eq!(pipeline.stats.frames_processed, 0);
    }

    #[test]
    fn test_empty_frame_processing() {
        let mut pipeline = LiveHybridPipeline::default();
        let scene = Dynamic4DScene::new(1.0, 30.0);

        pipeline.process_frame(&scene, 0.0);

        assert_eq!(pipeline.stats.frames_processed, 1);
    }
}
