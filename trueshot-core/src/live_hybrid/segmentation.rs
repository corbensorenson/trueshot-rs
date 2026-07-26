//! Object Segmentation
//!
//! Segments a 4DGS scene into individual objects for tracking and
//! representation optimization.
//!
//! Uses the unified tracking module for BoundingBox3D, gaining IoU and other
//! advanced methods not in the original implementation.

use nalgebra as na;
use uuid::Uuid;

use crate::gaussian_splatting::gaussian_4d::{Dynamic4DScene, Gaussian4D};

// Re-export unified BoundingBox3D from tracking module
pub use crate::tracking::BoundingBox3D;

/// A segmented object within a scene
#[derive(Clone, Debug)]
pub struct SegmentedObject {
    /// Unique identifier
    pub id: Uuid,
    /// Indices of Gaussians belonging to this object
    pub gaussian_indices: Vec<usize>,
    /// 3D bounding box
    pub bounding_box: BoundingBox3D,
    /// Centroid position
    pub centroid: na::Point3<f32>,
    /// Estimated surface area
    pub surface_area: f32,
    /// Semantic label (if detected)
    pub label: Option<String>,
}

/// A tracked object across multiple frames
#[derive(Clone, Debug)]
pub struct TrackedObject {
    /// Unique ID (consistent across frames)
    pub id: Uuid,
    /// Current segmented object
    pub current: SegmentedObject,
    /// Previous frame's Gaussians (for motion scoring)
    pub previous_gaussians: Vec<Gaussian4D>,
    /// Track history (centroids over time)
    pub track_history: Vec<na::Point3<f32>>,
    /// Frames since first detection
    pub frames_tracked: usize,
    /// Frames since last update
    pub frames_since_update: usize,
}

impl TrackedObject {
    pub fn new(segment: SegmentedObject) -> Self {
        Self {
            id: segment.id,
            current: segment.clone(),
            previous_gaussians: Vec::new(),
            track_history: vec![segment.centroid],
            frames_tracked: 1,
            frames_since_update: 0,
        }
    }

    pub fn update(&mut self, segment: SegmentedObject, gaussians: Vec<Gaussian4D>) {
        self.previous_gaussians = self.get_current_gaussians_clone();
        self.current = segment;
        self.track_history.push(self.current.centroid);
        if self.track_history.len() > 100 {
            self.track_history.remove(0);
        }
        self.frames_tracked += 1;
        self.frames_since_update = 0;
    }

    fn get_current_gaussians_clone(&self) -> Vec<Gaussian4D> {
        // This would normally reference the scene, simplified here
        Vec::new()
    }

    /// Check if track is stale (not updated recently)
    pub fn is_stale(&self, max_frames: usize) -> bool {
        self.frames_since_update > max_frames
    }

    /// Get velocity estimate from track history
    pub fn estimated_velocity(&self) -> na::Vector3<f32> {
        if self.track_history.len() < 2 {
            return na::Vector3::zeros();
        }
        let n = self.track_history.len();
        self.track_history[n - 1] - self.track_history[n - 2]
    }
}

/// Object segmenter using spatial clustering
pub struct ObjectSegmenter {
    /// Minimum Gaussians to form an object
    min_gaussians: usize,
    /// Distance threshold for clustering
    cluster_distance: f32,
    /// Maximum objects to detect
    max_objects: usize,
}

impl Default for ObjectSegmenter {
    fn default() -> Self {
        Self {
            min_gaussians: 100,
            cluster_distance: 0.5,
            max_objects: 50,
        }
    }
}

impl ObjectSegmenter {
    pub fn new(min_gaussians: usize, cluster_distance: f32, max_objects: usize) -> Self {
        Self {
            min_gaussians,
            cluster_distance,
            max_objects,
        }
    }

    /// Segment a scene into objects using spatial clustering
    pub fn segment_scene(&self, scene: &Dynamic4DScene) -> Vec<SegmentedObject> {
        let gaussians = &scene.gaussians;
        if gaussians.is_empty() {
            return Vec::new();
        }

        // Simple DBSCAN-like clustering
        let mut visited = vec![false; gaussians.len()];
        let mut clusters: Vec<Vec<usize>> = Vec::new();

        for i in 0..gaussians.len() {
            if visited[i] {
                continue;
            }

            let mut cluster = Vec::new();
            self.expand_cluster(gaussians, i, &mut cluster, &mut visited);

            if cluster.len() >= self.min_gaussians {
                clusters.push(cluster);
                if clusters.len() >= self.max_objects {
                    break;
                }
            }
        }

        // Convert clusters to objects
        clusters
            .into_iter()
            .map(|indices| self.create_object(gaussians, indices))
            .collect()
    }

    /// Expand cluster using region growing
    fn expand_cluster(
        &self,
        gaussians: &[Gaussian4D],
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
            let pos = na::Point3::new(
                gaussians[idx].center.x,
                gaussians[idx].center.y,
                gaussians[idx].center.z,
            );

            for (j, g) in gaussians.iter().enumerate() {
                if visited[j] {
                    continue;
                }
                let other = na::Point3::new(g.center.x, g.center.y, g.center.z);
                if na::distance(&pos, &other) < self.cluster_distance {
                    stack.push(j);
                }
            }
        }
    }

    /// Create a SegmentedObject from cluster indices
    fn create_object(&self, gaussians: &[Gaussian4D], indices: Vec<usize>) -> SegmentedObject {
        let points = indices.iter().map(|&i| {
            let g = &gaussians[i];
            na::Point3::new(g.center.x, g.center.y, g.center.z)
        });

        let bbox = BoundingBox3D::from_points(points.clone());
        let centroid = bbox.center();

        // Estimate surface area from bounding box
        let s = bbox.size();
        let surface_area = 2.0 * (s.x * s.y + s.y * s.z + s.z * s.x);

        SegmentedObject {
            id: Uuid::new_v4(),
            gaussian_indices: indices,
            bounding_box: bbox,
            centroid,
            surface_area,
            label: None,
        }
    }

    /// Track objects across frames
    pub fn track_objects(
        &self,
        prev_tracks: &mut Vec<TrackedObject>,
        curr_segments: Vec<SegmentedObject>,
        scene: &Dynamic4DScene,
    ) {
        let mut matched = vec![false; curr_segments.len()];

        // Match existing tracks to new segments
        for track in prev_tracks.iter_mut() {
            let mut best_match: Option<(usize, f32)> = None;

            for (i, seg) in curr_segments.iter().enumerate() {
                if matched[i] {
                    continue;
                }

                let dist = na::distance(&track.current.centroid, &seg.centroid);
                if dist < self.cluster_distance * 2.0 && best_match.map_or(true, |(_, d)| dist < d)
                {
                    best_match = Some((i, dist));
                }
            }

            if let Some((idx, _)) = best_match {
                matched[idx] = true;
                let gaussians: Vec<Gaussian4D> = curr_segments[idx]
                    .gaussian_indices
                    .iter()
                    .map(|&i| scene.gaussians[i].clone())
                    .collect();
                track.update(curr_segments[idx].clone(), gaussians);
            } else {
                track.frames_since_update += 1;
            }
        }

        // Create new tracks for unmatched segments
        for (i, seg) in curr_segments.into_iter().enumerate() {
            if !matched[i] {
                prev_tracks.push(TrackedObject::new(seg));
            }
        }

        // Remove stale tracks
        prev_tracks.retain(|t| !t.is_stale(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounding_box() {
        let points = vec![
            na::Point3::new(0.0, 0.0, 0.0),
            na::Point3::new(1.0, 2.0, 3.0),
        ];
        let bbox = BoundingBox3D::from_points(points.into_iter());

        assert_eq!(bbox.min, na::Point3::new(0.0, 0.0, 0.0));
        assert_eq!(bbox.max, na::Point3::new(1.0, 2.0, 3.0));
        assert_eq!(bbox.volume(), 6.0);
    }

    #[test]
    fn test_segmenter_empty_scene() {
        let segmenter = ObjectSegmenter::default();
        let scene = Dynamic4DScene::new(1.0, 30.0);
        let objects = segmenter.segment_scene(&scene);
        assert!(objects.is_empty());
    }
}
