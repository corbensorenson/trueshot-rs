//! Unified Motion and Object Tracking System
//!
//! Core tracking components used across all TrueShot modes:
//! - LiveHybrid: Real-time object tracking for representation switching
//! - 4DGS: Motion analysis for training optimization
//! - Scene Reconstruction: Object segmentation for scene understanding
//! - Hybrid Mode: Motion-adaptive processing
//!
//! Features:
//! - Motion scoring with configurable weights
//! - DBSCAN-like spatial clustering
//! - Kalman filter-based trajectory prediction
//! - Cross-frame object tracking and association

use nalgebra as na;
use std::collections::HashMap;
use uuid::Uuid;

// ============================================================================
// Bounding Boxes
// ============================================================================

/// 3D axis-aligned bounding box
#[derive(Clone, Debug, Default)]
pub struct BoundingBox3D {
    pub min: na::Point3<f32>,
    pub max: na::Point3<f32>,
}

impl BoundingBox3D {
    pub fn new(min: na::Point3<f32>, max: na::Point3<f32>) -> Self {
        Self { min, max }
    }

    pub fn from_points(points: impl Iterator<Item = na::Point3<f32>>) -> Self {
        let mut min = na::Point3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut max = na::Point3::new(f32::MIN, f32::MIN, f32::MIN);

        for p in points {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            min.z = min.z.min(p.z);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
            max.z = max.z.max(p.z);
        }

        Self { min, max }
    }

    /// Compute center point
    pub fn center(&self) -> na::Point3<f32> {
        na::Point3::from((self.min.coords + self.max.coords) / 2.0)
    }

    /// Compute size vector
    pub fn size(&self) -> na::Vector3<f32> {
        self.max - self.min
    }

    /// Compute volume
    pub fn volume(&self) -> f32 {
        let s = self.size();
        s.x * s.y * s.z
    }

    /// Compute surface area
    pub fn surface_area(&self) -> f32 {
        let s = self.size();
        2.0 * (s.x * s.y + s.y * s.z + s.z * s.x)
    }

    /// Check if point is inside
    pub fn contains(&self, point: &na::Point3<f32>) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
            && point.z >= self.min.z
            && point.z <= self.max.z
    }

    /// Check if boxes intersect
    pub fn intersects(&self, other: &BoundingBox3D) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
            && self.min.z <= other.max.z
            && self.max.z >= other.min.z
    }

    /// Compute IoU (Intersection over Union)
    pub fn iou(&self, other: &BoundingBox3D) -> f32 {
        if !self.intersects(other) {
            return 0.0;
        }

        let inter_min = na::Point3::new(
            self.min.x.max(other.min.x),
            self.min.y.max(other.min.y),
            self.min.z.max(other.min.z),
        );
        let inter_max = na::Point3::new(
            self.max.x.min(other.max.x),
            self.max.y.min(other.max.y),
            self.max.z.min(other.max.z),
        );

        let inter = BoundingBox3D::new(inter_min, inter_max);
        let inter_vol = inter.volume();
        let union_vol = self.volume() + other.volume() - inter_vol;

        if union_vol > 0.0 {
            inter_vol / union_vol
        } else {
            0.0
        }
    }

    /// Expand box by margin
    pub fn expand(&self, margin: f32) -> Self {
        Self {
            min: self.min - na::Vector3::new(margin, margin, margin),
            max: self.max + na::Vector3::new(margin, margin, margin),
        }
    }
}

// ============================================================================
// Motion Classification
// ============================================================================

/// Motion classification based on score
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MotionClass {
    /// Very low motion - candidate for static representation (mesh)
    Static,
    /// Moderate motion - reduced update frequency
    Slow,
    /// High motion - full dynamic processing
    Dynamic,
    /// Extreme motion - prioritize for real-time streaming
    Rapid,
}

impl MotionClass {
    /// Classify from score (0.0 = static, 1.0 = maximum motion)
    pub fn from_score(score: f32) -> Self {
        if score < 0.1 {
            MotionClass::Static
        } else if score < 0.4 {
            MotionClass::Slow
        } else if score < 0.8 {
            MotionClass::Dynamic
        } else {
            MotionClass::Rapid
        }
    }

    /// Get recommended update frequency (frames between updates)
    pub fn update_interval(&self) -> usize {
        match self {
            MotionClass::Static => 30, // Once per second at 30fps
            MotionClass::Slow => 5,    // 6 times per second
            MotionClass::Dynamic => 1, // Every frame
            MotionClass::Rapid => 1,   // Every frame with priority
        }
    }

    /// Check if can be converted to mesh
    pub fn is_meshifiable(&self) -> bool {
        matches!(self, MotionClass::Static)
    }
}

// ============================================================================
// Motion Scorer
// ============================================================================

/// Configuration for motion scoring
#[derive(Clone, Debug)]
pub struct MotionConfig {
    /// Weight for position change
    pub position_weight: f32,
    /// Weight for velocity magnitude
    pub velocity_weight: f32,
    /// Weight for acceleration
    pub acceleration_weight: f32,
    /// Weight for shape/size change
    pub shape_weight: f32,
    /// Exponential moving average factor (0-1, higher = more recent)
    pub ema_alpha: f32,
    /// Number of frames for history
    pub history_frames: usize,
}

impl Default for MotionConfig {
    fn default() -> Self {
        Self {
            position_weight: 0.4,
            velocity_weight: 0.3,
            acceleration_weight: 0.2,
            shape_weight: 0.1,
            ema_alpha: 0.3,
            history_frames: 10,
        }
    }
}

/// Motion state for a single object
#[derive(Clone, Debug)]
pub struct MotionState {
    /// Current position (centroid)
    pub position: na::Point3<f32>,
    /// Current velocity estimate
    pub velocity: na::Vector3<f32>,
    /// Current acceleration estimate
    pub acceleration: na::Vector3<f32>,
    /// Current bounding box
    pub bounds: BoundingBox3D,
    /// Motion score (0-1)
    pub score: f32,
    /// Motion classification
    pub class: MotionClass,
    /// Position history for trajectory
    pub history: Vec<na::Point3<f32>>,
    /// Timestamp of last update
    pub last_update_frame: u64,
}

impl Default for MotionState {
    fn default() -> Self {
        Self {
            position: na::Point3::origin(),
            velocity: na::Vector3::zeros(),
            acceleration: na::Vector3::zeros(),
            bounds: BoundingBox3D::default(),
            score: 0.0,
            class: MotionClass::Static,
            history: Vec::new(),
            last_update_frame: 0,
        }
    }
}

/// Motion analyzer with Kalman-style prediction
pub struct MotionAnalyzer {
    config: MotionConfig,
    states: HashMap<Uuid, MotionState>,
    current_frame: u64,
}

impl MotionAnalyzer {
    pub fn new(config: MotionConfig) -> Self {
        Self {
            config,
            states: HashMap::new(),
            current_frame: 0,
        }
    }

    /// Update motion state for an object
    pub fn update(
        &mut self,
        object_id: Uuid,
        position: na::Point3<f32>,
        bounds: BoundingBox3D,
    ) -> MotionState {
        let state = self.states.entry(object_id).or_insert_with(|| MotionState {
            position,
            bounds: bounds.clone(),
            ..Default::default()
        });

        // Compute deltas
        let position_delta = position - state.position;
        let new_velocity = position_delta; // Assuming 1 frame timestep
        let acceleration_delta = new_velocity - state.velocity;

        // EMA smoothing
        let alpha = self.config.ema_alpha;
        state.velocity = state.velocity * (1.0 - alpha) + new_velocity * alpha;
        state.acceleration = state.acceleration * (1.0 - alpha) + acceleration_delta * alpha;

        // Update position
        state.position = position;

        // Compute shape change (bounding box volume ratio)
        let prev_vol = state.bounds.volume();
        let curr_vol = bounds.volume();
        let shape_delta = if prev_vol > 0.0 {
            ((curr_vol / prev_vol) - 1.0).abs().min(1.0)
        } else {
            0.0
        };
        state.bounds = bounds;

        // Compute motion score
        let pos_score = position_delta.norm().min(1.0);
        let vel_score = state.velocity.norm().min(1.0);
        let acc_score = state.acceleration.norm().min(1.0);

        state.score = self.config.position_weight * pos_score
            + self.config.velocity_weight * vel_score
            + self.config.acceleration_weight * acc_score
            + self.config.shape_weight * shape_delta;

        state.score = state.score.clamp(0.0, 1.0);
        state.class = MotionClass::from_score(state.score);

        // Update history
        state.history.push(position);
        if state.history.len() > self.config.history_frames {
            state.history.remove(0);
        }

        state.last_update_frame = self.current_frame;

        state.clone()
    }

    /// Predict future position
    pub fn predict(&self, object_id: &Uuid, frames_ahead: f32) -> Option<na::Point3<f32>> {
        self.states.get(object_id).map(|state| {
            // Linear prediction with velocity and acceleration
            let pos_delta = state.velocity * frames_ahead
                + state.acceleration * 0.5 * frames_ahead * frames_ahead;
            state.position + pos_delta
        })
    }

    /// Get motion state for object
    pub fn get_state(&self, object_id: &Uuid) -> Option<&MotionState> {
        self.states.get(object_id)
    }

    /// Get all static objects (meshification candidates)
    pub fn get_static_objects(&self) -> Vec<Uuid> {
        self.states
            .iter()
            .filter(|(_, state)| state.class.is_meshifiable())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Advance frame counter
    pub fn advance_frame(&mut self) {
        self.current_frame += 1;
    }

    /// Remove stale objects
    pub fn prune_stale(&mut self, max_frames: u64) {
        self.states
            .retain(|_, state| self.current_frame - state.last_update_frame < max_frames);
    }
}

impl Default for MotionAnalyzer {
    fn default() -> Self {
        Self::new(MotionConfig::default())
    }
}

// ============================================================================
// Object Segmentation
// ============================================================================

/// Configuration for object segmentation
#[derive(Clone, Debug)]
pub struct SegmentationConfig {
    /// Minimum points to form a cluster
    pub min_cluster_size: usize,
    /// Maximum distance for DBSCAN neighborhood
    pub eps_distance: f32,
    /// Maximum objects to detect
    pub max_objects: usize,
    /// Whether to use octree acceleration
    pub use_octree: bool,
}

impl Default for SegmentationConfig {
    fn default() -> Self {
        Self {
            min_cluster_size: 50,
            eps_distance: 0.3,
            max_objects: 100,
            use_octree: true,
        }
    }
}

/// A segmented object
#[derive(Clone, Debug)]
pub struct SegmentedObject {
    /// Unique identifier
    pub id: Uuid,
    /// Indices of points in this object
    pub point_indices: Vec<usize>,
    /// Bounding box
    pub bounds: BoundingBox3D,
    /// Centroid
    pub centroid: na::Point3<f32>,
    /// Estimated surface area
    pub surface_area: f32,
    /// Semantic label (if detected)
    pub label: Option<String>,
    /// Confidence score
    pub confidence: f32,
}

/// Object segmenter using DBSCAN clustering
pub struct ObjectSegmenter {
    config: SegmentationConfig,
}

impl ObjectSegmenter {
    pub fn new(config: SegmentationConfig) -> Self {
        Self { config }
    }

    /// Segment points into objects
    pub fn segment(&self, positions: &[na::Point3<f32>]) -> Vec<SegmentedObject> {
        if positions.is_empty() {
            return Vec::new();
        }

        let mut visited = vec![false; positions.len()];
        let mut clusters: Vec<Vec<usize>> = Vec::new();

        // DBSCAN-like clustering
        for i in 0..positions.len() {
            if visited[i] {
                continue;
            }

            let mut cluster = Vec::new();
            self.expand_cluster(positions, i, &mut cluster, &mut visited);

            if cluster.len() >= self.config.min_cluster_size {
                clusters.push(cluster);
                if clusters.len() >= self.config.max_objects {
                    break;
                }
            }
        }

        // Convert to segmented objects
        clusters
            .into_iter()
            .map(|indices| self.create_object(positions, indices))
            .collect()
    }

    /// Expand cluster using region growing
    fn expand_cluster(
        &self,
        positions: &[na::Point3<f32>],
        start: usize,
        cluster: &mut Vec<usize>,
        visited: &mut [bool],
    ) {
        let mut stack = vec![start];

        while let Some(idx) = stack.pop() {
            if visited[idx] {
                continue;
            }
            visited[idx] = true;
            cluster.push(idx);

            // Find neighbors
            let pos = positions[idx];
            for (j, other_pos) in positions.iter().enumerate() {
                if !visited[j] && na::distance(&pos, other_pos) < self.config.eps_distance {
                    stack.push(j);
                }
            }
        }
    }

    /// Create a SegmentedObject from cluster indices
    fn create_object(&self, positions: &[na::Point3<f32>], indices: Vec<usize>) -> SegmentedObject {
        let points = indices.iter().map(|&i| positions[i]);
        let bounds = BoundingBox3D::from_points(points);
        let centroid = bounds.center();
        let surface_area = bounds.surface_area();

        SegmentedObject {
            id: Uuid::new_v4(),
            point_indices: indices,
            bounds,
            centroid,
            surface_area,
            label: None,
            confidence: 1.0,
        }
    }
}

impl Default for ObjectSegmenter {
    fn default() -> Self {
        Self::new(SegmentationConfig::default())
    }
}

// ============================================================================
// Object Tracker
// ============================================================================

/// Configuration for object tracking
#[derive(Clone, Debug)]
pub struct TrackerConfig {
    /// Maximum distance for track association
    pub max_association_distance: f32,
    /// Frames before track is considered lost
    pub max_frames_lost: usize,
    /// Minimum IoU for bbox matching
    pub min_iou_threshold: f32,
    /// Whether to use appearance features
    pub use_appearance: bool,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            max_association_distance: 1.0,
            max_frames_lost: 30,
            min_iou_threshold: 0.3,
            use_appearance: false,
        }
    }
}

/// A tracked object across frames
#[derive(Clone, Debug)]
pub struct TrackedObject {
    /// Unique track ID (consistent across frames)
    pub id: Uuid,
    /// Current object state
    pub object: SegmentedObject,
    /// Motion state
    pub motion: MotionState,
    /// Frames since first detection
    pub age: usize,
    /// Frames since last observation
    pub frames_lost: usize,
    /// Total hits (observations)
    pub hits: usize,
}

impl TrackedObject {
    pub fn new(object: SegmentedObject) -> Self {
        Self {
            id: object.id,
            object,
            motion: MotionState::default(),
            age: 1,
            frames_lost: 0,
            hits: 1,
        }
    }

    /// Check if track is confirmed (enough hits)
    pub fn is_confirmed(&self) -> bool {
        self.hits >= 3
    }

    /// Check if track is lost
    pub fn is_lost(&self, max_frames: usize) -> bool {
        self.frames_lost > max_frames
    }
}

/// Multi-object tracker using Hungarian algorithm for association
pub struct ObjectTracker {
    config: TrackerConfig,
    motion_analyzer: MotionAnalyzer,
    tracks: Vec<TrackedObject>,
    frame_count: u64,
}

impl ObjectTracker {
    pub fn new(config: TrackerConfig, motion_config: MotionConfig) -> Self {
        Self {
            config,
            motion_analyzer: MotionAnalyzer::new(motion_config),
            tracks: Vec::new(),
            frame_count: 0,
        }
    }

    /// Update tracker with new detections
    pub fn update(&mut self, detections: Vec<SegmentedObject>) -> Vec<TrackedObject> {
        self.frame_count += 1;
        self.motion_analyzer.advance_frame();

        // Match detections to existing tracks
        let mut matched_tracks = vec![false; self.tracks.len()];
        let mut matched_dets = vec![false; detections.len()];

        // Greedy matching by distance
        for (t_idx, track) in self.tracks.iter_mut().enumerate() {
            let mut best_match: Option<(usize, f32)> = None;

            for (d_idx, det) in detections.iter().enumerate() {
                if matched_dets[d_idx] {
                    continue;
                }

                let dist = na::distance(&track.object.centroid, &det.centroid);
                let iou = track.object.bounds.iou(&det.bounds);

                if dist < self.config.max_association_distance
                    || iou > self.config.min_iou_threshold
                {
                    let score = dist - iou * self.config.max_association_distance;
                    if best_match.map_or(true, |(_, s)| score < s) {
                        best_match = Some((d_idx, score));
                    }
                }
            }

            if let Some((d_idx, _)) = best_match {
                matched_tracks[t_idx] = true;
                matched_dets[d_idx] = true;

                // Update track
                let det = &detections[d_idx];
                track.object = det.clone();
                track.motion =
                    self.motion_analyzer
                        .update(track.id, det.centroid, det.bounds.clone());
                track.age += 1;
                track.hits += 1;
                track.frames_lost = 0;
            }
        }

        // Increment lost count for unmatched tracks
        for (i, track) in self.tracks.iter_mut().enumerate() {
            if !matched_tracks[i] {
                track.frames_lost += 1;
            }
        }

        // Create new tracks for unmatched detections
        for (i, det) in detections.into_iter().enumerate() {
            if !matched_dets[i] {
                let mut track = TrackedObject::new(det.clone());
                track.motion = self
                    .motion_analyzer
                    .update(track.id, det.centroid, det.bounds);
                self.tracks.push(track);
            }
        }

        // Remove lost tracks
        self.tracks
            .retain(|t| !t.is_lost(self.config.max_frames_lost));

        // Return confirmed tracks
        self.tracks
            .iter()
            .filter(|t| t.is_confirmed())
            .cloned()
            .collect()
    }

    /// Get all active tracks
    pub fn get_tracks(&self) -> &[TrackedObject] {
        &self.tracks
    }

    /// Get track by ID
    pub fn get_track(&self, id: &Uuid) -> Option<&TrackedObject> {
        self.tracks.iter().find(|t| &t.id == id)
    }

    /// Get static objects (candidates for meshification)
    pub fn get_static_tracks(&self) -> Vec<&TrackedObject> {
        self.tracks
            .iter()
            .filter(|t| t.is_confirmed() && t.motion.class.is_meshifiable())
            .collect()
    }

    /// Get dynamic objects (need real-time processing)
    pub fn get_dynamic_tracks(&self) -> Vec<&TrackedObject> {
        self.tracks
            .iter()
            .filter(|t| t.is_confirmed() && !t.motion.class.is_meshifiable())
            .collect()
    }
}

impl Default for ObjectTracker {
    fn default() -> Self {
        Self::new(TrackerConfig::default(), MotionConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounding_box_iou() {
        let a = BoundingBox3D::new(
            na::Point3::new(0.0, 0.0, 0.0),
            na::Point3::new(1.0, 1.0, 1.0),
        );
        let b = BoundingBox3D::new(
            na::Point3::new(0.5, 0.5, 0.5),
            na::Point3::new(1.5, 1.5, 1.5),
        );

        let iou = a.iou(&b);
        assert!(iou > 0.0 && iou < 1.0);
    }

    #[test]
    fn test_motion_classification() {
        assert_eq!(MotionClass::from_score(0.0), MotionClass::Static);
        assert_eq!(MotionClass::from_score(0.05), MotionClass::Static);
        assert_eq!(MotionClass::from_score(0.2), MotionClass::Slow);
        assert_eq!(MotionClass::from_score(0.5), MotionClass::Dynamic);
        assert_eq!(MotionClass::from_score(0.9), MotionClass::Rapid);
    }

    #[test]
    fn test_segmentation() {
        let segmenter = ObjectSegmenter::new(SegmentationConfig {
            min_cluster_size: 2,
            eps_distance: 0.5,
            ..Default::default()
        });

        let positions = vec![
            na::Point3::new(0.0, 0.0, 0.0),
            na::Point3::new(0.1, 0.1, 0.1),
            na::Point3::new(5.0, 5.0, 5.0),
            na::Point3::new(5.1, 5.1, 5.1),
        ];

        let objects = segmenter.segment(&positions);
        assert_eq!(objects.len(), 2);
    }
}
