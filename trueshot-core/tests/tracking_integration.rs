//! Integration tests for object tracking pipeline
//!
//! Tests segmentation, motion analysis, and object tracking with synthetic data.

use nalgebra as na;
use trueshot_core::tracking::{
    BoundingBox3D, MotionAnalyzer, MotionClass, MotionConfig, ObjectSegmenter, ObjectTracker,
    SegmentationConfig, SegmentedObject, TrackerConfig,
};
use uuid::Uuid;

fn make_segment(id: Uuid, bounds: BoundingBox3D) -> SegmentedObject {
    let centroid = bounds.center();
    SegmentedObject {
        id,
        point_indices: Vec::new(),
        bounds,
        centroid,
        surface_area: 0.0,
        label: None,
        confidence: 1.0,
    }
}

/// Create synthetic clustered point cloud
fn create_clustered_points(num_clusters: usize, points_per_cluster: usize) -> Vec<na::Point3<f32>> {
    let mut points = Vec::with_capacity(num_clusters * points_per_cluster);

    let cluster_centers = vec![
        na::Point3::new(0.0, 0.0, 0.0),
        na::Point3::new(5.0, 0.0, 0.0),
        na::Point3::new(0.0, 5.0, 0.0),
        na::Point3::new(0.0, 0.0, 5.0),
        na::Point3::new(5.0, 5.0, 0.0),
    ];

    for i in 0..num_clusters.min(cluster_centers.len()) {
        let center = cluster_centers[i];
        for j in 0..points_per_cluster {
            // Add points in a small sphere around the center
            let offset = na::Vector3::new(
                ((j * 17) as f32 * 0.123).sin() * 0.3,
                ((j * 31) as f32 * 0.456).cos() * 0.3,
                ((j * 47) as f32 * 0.789).sin() * 0.3,
            );
            points.push(center + offset);
        }
    }

    points
}

#[test]
fn test_segmentation_finds_clusters() {
    let points = create_clustered_points(3, 50);

    let segmenter = ObjectSegmenter::new(SegmentationConfig {
        min_cluster_size: 10,
        eps_distance: 1.0,
        ..Default::default()
    });

    let segments = segmenter.segment(&points);

    // Should find 3 clusters
    assert!(
        segments.len() >= 2 && segments.len() <= 4,
        "Expected 2-4 clusters, got {}",
        segments.len()
    );

    // Each segment should have points
    for segment in &segments {
        assert!(
            !segment.point_indices.is_empty(),
            "Segment should have points"
        );
    }
}

#[test]
fn test_motion_analyzer_velocity() {
    let mut analyzer = MotionAnalyzer::new(MotionConfig::default());
    let id = Uuid::new_v4();

    // Simulate object moving along X axis
    for i in 0..10 {
        let pos = na::Point3::new(i as f32 * 0.5, 0.0, 0.0);
        let bounds = BoundingBox3D::new(
            pos - na::Vector3::new(0.5, 0.5, 0.5),
            pos + na::Vector3::new(0.5, 0.5, 0.5),
        );
        analyzer.update(id, pos, bounds);
        analyzer.advance_frame();
    }

    // Check velocity is detected
    let state = analyzer.get_state(&id);
    assert!(state.is_some(), "Should track velocity");

    let vel = state.unwrap().velocity;
    assert!(vel.x > 0.0, "Object should be moving in +X direction");
    assert!(vel.y.abs() < 0.1, "Minimal Y movement");
}

#[test]
fn test_motion_classification() {
    // Static object
    let static_score = 0.01;
    assert_eq!(MotionClass::from_score(static_score), MotionClass::Static);

    // Slow moving
    let slow_score = 0.3;
    assert_eq!(MotionClass::from_score(slow_score), MotionClass::Slow);

    // Dynamic
    let dynamic_score = 0.7;
    assert_eq!(MotionClass::from_score(dynamic_score), MotionClass::Dynamic);

    // Rapid
    let rapid_score = 1.5;
    assert_eq!(MotionClass::from_score(rapid_score), MotionClass::Rapid);
}

#[test]
fn test_bounding_box_iou() {
    let box1 = BoundingBox3D::new(
        na::Point3::new(0.0, 0.0, 0.0),
        na::Point3::new(2.0, 2.0, 2.0),
    );

    let box2 = BoundingBox3D::new(
        na::Point3::new(1.0, 1.0, 1.0),
        na::Point3::new(3.0, 3.0, 3.0),
    );

    let iou = box1.iou(&box2);

    // Intersection is 1x1x1 = 1
    // Union is 8 + 8 - 1 = 15
    // IoU should be 1/15 ≈ 0.067
    assert!(iou > 0.05 && iou < 0.1, "IoU should be ~0.067, got {}", iou);
}

#[test]
fn test_bounding_box_no_overlap() {
    let box1 = BoundingBox3D::new(
        na::Point3::new(0.0, 0.0, 0.0),
        na::Point3::new(1.0, 1.0, 1.0),
    );

    let box2 = BoundingBox3D::new(
        na::Point3::new(5.0, 5.0, 5.0),
        na::Point3::new(6.0, 6.0, 6.0),
    );

    let iou = box1.iou(&box2);

    assert_eq!(iou, 0.0, "Non-overlapping boxes should have IoU = 0");
}

#[test]
fn test_object_tracker_persistence() {
    let mut tracker = ObjectTracker::new(TrackerConfig::default(), MotionConfig::default());
    let id1 = Uuid::new_v4();
    let id2 = Uuid::new_v4();

    // Create initial detections
    let det1 = BoundingBox3D::new(
        na::Point3::new(0.0, 0.0, 0.0),
        na::Point3::new(1.0, 1.0, 1.0),
    );

    let det2 = BoundingBox3D::new(
        na::Point3::new(5.0, 0.0, 0.0),
        na::Point3::new(6.0, 1.0, 1.0),
    );

    let dets_frame1 = vec![
        make_segment(id1, det1.clone()),
        make_segment(id2, det2.clone()),
    ];
    let _ = tracker.update(dets_frame1);

    // Second frame - slightly moved
    let det1_moved = BoundingBox3D::new(
        na::Point3::new(0.1, 0.0, 0.0),
        na::Point3::new(1.1, 1.0, 1.0),
    );

    let det2_moved = BoundingBox3D::new(
        na::Point3::new(5.1, 0.0, 0.0),
        na::Point3::new(6.1, 1.0, 1.0),
    );

    let dets_frame2 = vec![make_segment(id1, det1_moved), make_segment(id2, det2_moved)];
    let _ = tracker.update(dets_frame2);

    // Third frame to confirm tracks
    let det1_moved2 = BoundingBox3D::new(
        na::Point3::new(0.2, 0.0, 0.0),
        na::Point3::new(1.2, 1.0, 1.0),
    );
    let det2_moved2 = BoundingBox3D::new(
        na::Point3::new(5.2, 0.0, 0.0),
        na::Point3::new(6.2, 1.0, 1.0),
    );
    let dets_frame3 = vec![
        make_segment(id1, det1_moved2),
        make_segment(id2, det2_moved2),
    ];
    let tracked3 = tracker.update(dets_frame3);

    // Should maintain same IDs
    assert_eq!(tracked3.len(), 2, "Should still track 2 objects");
    let ids2: Vec<_> = tracked3.iter().map(|t| t.id).collect();

    // IDs should persist (same objects)
    assert!(
        ids2.contains(&id1) && ids2.contains(&id2),
        "Object IDs should persist across frames"
    );
}
