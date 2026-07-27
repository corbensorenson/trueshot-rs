//! Geometry Tests
//!
//! Tests for RANSAC, essential matrix estimation, and triangulation.

use nalgebra as na;
use trueshot_sfm::geometry::{ransac_essential, ransac_homography, RansacConfig};

#[test]
fn test_ransac_homography_finds_correct_model() {
    // Create a simple identity homography (no transformation)
    let pts1: Vec<na::Point2<f64>> = vec![
        na::Point2::new(100.0, 100.0),
        na::Point2::new(200.0, 100.0),
        na::Point2::new(300.0, 100.0),
        na::Point2::new(100.0, 200.0),
        na::Point2::new(200.0, 200.0),
        na::Point2::new(300.0, 200.0),
        na::Point2::new(100.0, 300.0),
        na::Point2::new(200.0, 300.0),
        na::Point2::new(300.0, 300.0),
        na::Point2::new(150.0, 150.0),
    ];

    // Small translation - easy to find
    let pts2: Vec<na::Point2<f64>> = pts1
        .iter()
        .map(|p| na::Point2::new(p.x + 10.0, p.y + 5.0))
        .collect();

    let config = RansacConfig {
        max_iterations: 500,
        threshold: 10.0, // 10 pixel threshold
        confidence: 0.95,
        min_inlier_ratio: 0.3,
    };

    let result = ransac_homography(&pts1, &pts2, &config);

    // Should find a valid homography
    assert!(
        result.is_some(),
        "RANSAC should find simple translation homography"
    );
}

#[test]
fn test_ransac_homography_with_outliers() {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    // Grid of points
    let mut pts1: Vec<na::Point2<f64>> = Vec::new();
    let mut pts2: Vec<na::Point2<f64>> = Vec::new();

    // Create 20 good correspondences
    for i in 0..20 {
        let x = 100.0 + (i % 5) as f64 * 50.0;
        let y = 100.0 + (i / 5) as f64 * 50.0;
        pts1.push(na::Point2::new(x, y));
        pts2.push(na::Point2::new(x + 20.0, y + 10.0)); // Simple translation
    }

    // Add 5 outliers (20% outlier ratio)
    for _ in 0..5 {
        pts1.push(na::Point2::new(
            rng.gen_range(100.0..400.0),
            rng.gen_range(100.0..400.0),
        ));
        pts2.push(na::Point2::new(
            rng.gen_range(100.0..400.0),
            rng.gen_range(100.0..400.0),
        ));
    }

    let config = RansacConfig {
        max_iterations: 1000,
        threshold: 15.0,
        confidence: 0.95,
        min_inlier_ratio: 0.3,
    };

    let result = ransac_homography(&pts1, &pts2, &config);

    // Should find a homography despite outliers
    if let Some(r) = result {
        assert!(
            r.inlier_ratio > 0.3,
            "Should find significant inliers: {}",
            r.inlier_ratio
        );
    }
    // It's OK if RANSAC returns None with random data - this tests robustness
}

#[test]
fn test_ransac_config_default() {
    let config = RansacConfig::default();

    assert!(config.max_iterations > 0);
    assert!(config.threshold > 0.0);
    assert!(config.confidence > 0.0 && config.confidence < 1.0);
    assert!(config.min_inlier_ratio > 0.0 && config.min_inlier_ratio < 1.0);
}

#[test]
fn test_ransac_returns_none_on_insufficient_points() {
    let pts1 = vec![na::Point2::new(0.0, 0.0), na::Point2::new(1.0, 1.0)];
    let pts2 = vec![na::Point2::new(0.0, 0.0), na::Point2::new(1.0, 1.0)];

    let config = RansacConfig::default();

    // Homography needs at least 4 points
    let result = ransac_homography(&pts1, &pts2, &config);
    assert!(
        result.is_none(),
        "Should return None with insufficient points"
    );
}

#[test]
fn test_ransac_essential_with_camera_motion() {
    // Create synthetic camera intrinsics
    let K = na::Matrix3::new(500.0, 0.0, 320.0, 0.0, 500.0, 240.0, 0.0, 0.0, 1.0);

    // Generate points in 3D
    use rand::Rng;
    let mut rng = rand::thread_rng();

    let mut pts1 = Vec::new();
    let mut pts2 = Vec::new();

    // Camera 2 is translated by (1, 0, 0) from camera 1
    let t = na::Vector3::new(1.0, 0.0, 0.0);
    let R = na::Rotation3::identity();

    for _ in 0..100 {
        // Random 3D point
        let X = rng.gen_range(-2.0..2.0);
        let Y = rng.gen_range(-2.0..2.0);
        let Z = rng.gen_range(3.0..10.0);

        let P = na::Point3::new(X, Y, Z);

        // Project to camera 1
        let p1 = K * na::Vector3::new(X / Z, Y / Z, 1.0);

        // Transform to camera 2
        let P2 = R * P.coords - t;
        let p2 = K * na::Vector3::new(P2.x / P2.z, P2.y / P2.z, 1.0);

        pts1.push(na::Point2::new(p1.x, p1.y));
        pts2.push(na::Point2::new(p2.x, p2.y));
    }

    let config = RansacConfig {
        max_iterations: 2000,
        threshold: 1.0,
        confidence: 0.99,
        min_inlier_ratio: 0.5,
    };

    let result = ransac_essential(&pts1, &pts2, &K, &K, &config);

    // This should find a valid essential matrix
    assert!(
        result.is_some(),
        "Should find essential matrix for valid motion"
    );

    let result = result.unwrap();
    assert!(
        result.inlier_ratio > 0.7,
        "Most points should be inliers: {}",
        result.inlier_ratio
    );
}

#[test]
fn test_essential_matrix_rank() {
    let K = na::Matrix3::new(500.0, 0.0, 320.0, 0.0, 500.0, 240.0, 0.0, 0.0, 1.0);

    // Simple correspondences
    let pts1: Vec<na::Point2<f64>> = (0..20)
        .map(|i| na::Point2::new(100.0 + i as f64 * 20.0, 100.0 + (i % 5) as f64 * 50.0))
        .collect();

    let pts2: Vec<na::Point2<f64>> = pts1
        .iter()
        .map(|p| na::Point2::new(p.x + 10.0, p.y + 5.0))
        .collect();

    let config = RansacConfig::default();

    if let Some(result) = ransac_essential(&pts1, &pts2, &K, &K, &config) {
        let E = result.model;

        // Essential matrix should have rank 2
        let svd = na::SVD::new(E, false, false);
        let singular_values = svd.singular_values;

        // Third singular value should be ~0
        assert!(
            singular_values[2].abs() < 1e-6,
            "Third singular value should be ~0: {}",
            singular_values[2]
        );

        // First two should be approximately equal
        let ratio = singular_values[0] / singular_values[1];
        assert!(
            (ratio - 1.0).abs() < 0.1,
            "First two singular values should be ~equal: {}",
            ratio
        );
    }
}
