//! Bundle Adjustment Tests
//!
//! Tests for the Levenberg-Marquardt bundle adjustment optimizer.

use nalgebra as na;
use trueshot_sfm::optimization::{bundle_adjust_lm, BundleAdjustmentConfig, Observation};
use trueshot_sfm::{CameraIntrinsics, CameraPose, DistortionModel, Point3D};

/// Create a simple test scene with known geometry
fn create_test_scene() -> (
    Vec<Point3D>,
    Vec<CameraPose>,
    Vec<CameraIntrinsics>,
    Vec<Observation>,
) {
    // 4 3D points in a unit cube
    let points = vec![
        Point3D {
            position: na::Point3::new(0.0, 0.0, 5.0),
            color: [255, 0, 0],
            error: 0.0,
            track: vec![(0, 0), (1, 0)],
        },
        Point3D {
            position: na::Point3::new(1.0, 0.0, 5.0),
            color: [0, 255, 0],
            error: 0.0,
            track: vec![(0, 1), (1, 1)],
        },
        Point3D {
            position: na::Point3::new(0.0, 1.0, 5.0),
            color: [0, 0, 255],
            error: 0.0,
            track: vec![(0, 2), (1, 2)],
        },
        Point3D {
            position: na::Point3::new(1.0, 1.0, 5.0),
            color: [255, 255, 0],
            error: 0.0,
            track: vec![(0, 3), (1, 3)],
        },
    ];

    // Two cameras looking at the points
    let poses = vec![
        CameraPose::identity(),
        CameraPose {
            rotation: na::UnitQuaternion::from_euler_angles(0.0, 0.1, 0.0),
            translation: na::Vector3::new(0.5, 0.0, 0.0),
        },
    ];

    // Simple pinhole cameras
    let intrinsics = vec![
        CameraIntrinsics {
            fx: 500.0,
            fy: 500.0,
            cx: 320.0,
            cy: 240.0,
            width: 640,
            height: 480,
            distortion: vec![],
            distortion_model: DistortionModel::None,
        },
        CameraIntrinsics {
            fx: 500.0,
            fy: 500.0,
            cx: 320.0,
            cy: 240.0,
            width: 640,
            height: 480,
            distortion: vec![],
            distortion_model: DistortionModel::None,
        },
    ];

    // Generate observations by projecting points to cameras
    let mut observations = Vec::new();

    for (point_idx, point) in points.iter().enumerate() {
        for (cam_idx, pose) in poses.iter().enumerate() {
            let K = &intrinsics[cam_idx];

            // Project point to camera
            let p_cam = pose.rotation.inverse() * (point.position.coords - pose.translation);

            if p_cam.z > 0.0 {
                let x = K.fx * p_cam.x / p_cam.z + K.cx;
                let y = K.fy * p_cam.y / p_cam.z + K.cy;

                observations.push(Observation {
                    point_idx,
                    camera_idx: cam_idx,
                    x,
                    y,
                    time_offset: 0.0,
                });
            }
        }
    }

    (points, poses, intrinsics, observations)
}

#[test]
fn test_bundle_adjustment_converges_on_perfect_data() {
    let (mut points, mut poses, intrinsics, observations) = create_test_scene();

    let config = BundleAdjustmentConfig {
        max_iterations: 50,
        use_huber_loss: false,
        ..Default::default()
    };

    let pose_priors = vec![None; poses.len()];
    let camera_motions = vec![None; poses.len()];
    let result = bundle_adjust_lm(
        &mut points,
        &mut poses,
        &intrinsics,
        &observations,
        &pose_priors,
        &camera_motions,
        &config,
    );

    // With perfect data, cost should be very small
    assert!(
        result.final_cost < 1.0,
        "Final cost should be small: {}",
        result.final_cost
    );
    assert!(result.rmse < 1.0, "RMSE should be small: {}", result.rmse);
}

#[test]
fn test_bundle_adjustment_improves_noisy_data() {
    let (mut points, mut poses, intrinsics, observations) = create_test_scene();

    // Add noise to point positions
    use rand::Rng;
    let mut rng = rand::thread_rng();
    for point in &mut points {
        point.position.x += rng.gen_range(-0.1..0.1);
        point.position.y += rng.gen_range(-0.1..0.1);
        point.position.z += rng.gen_range(-0.1..0.1);
    }

    let config = BundleAdjustmentConfig {
        max_iterations: 100,
        use_huber_loss: true,
        ..Default::default()
    };

    let pose_priors = vec![None; poses.len()];
    let camera_motions = vec![None; poses.len()];
    let result = bundle_adjust_lm(
        &mut points,
        &mut poses,
        &intrinsics,
        &observations,
        &pose_priors,
        &camera_motions,
        &config,
    );

    // Cost should decrease
    assert!(
        result.final_cost <= result.initial_cost,
        "Cost should not increase: {} > {}",
        result.final_cost,
        result.initial_cost
    );
}

#[test]
fn test_bundle_adjustment_with_perturbed_poses() {
    let (mut points, mut poses, intrinsics, observations) = create_test_scene();

    // Perturb camera poses slightly
    poses[1].translation += na::Vector3::new(0.05, 0.05, 0.0);

    let config = BundleAdjustmentConfig::default();

    let pose_priors = vec![None; poses.len()];
    let camera_motions = vec![None; poses.len()];
    let result = bundle_adjust_lm(
        &mut points,
        &mut poses,
        &intrinsics,
        &observations,
        &pose_priors,
        &camera_motions,
        &config,
    );

    assert!(result.iterations > 0, "Should run at least one iteration");
    assert!(
        result.final_cost <= result.initial_cost * 1.1,
        "Cost should not increase significantly"
    );
}

#[test]
fn test_bundle_adjustment_config_default() {
    let config = BundleAdjustmentConfig::default();

    assert!(config.max_iterations > 0);
    assert!(config.initial_lambda > 0.0);
    assert!(config.lambda_increase > 1.0);
    assert!(config.lambda_decrease < 1.0 && config.lambda_decrease > 0.0);
}

#[test]
fn test_bundle_adjustment_empty_input() {
    let mut points = Vec::new();
    let mut poses = Vec::new();
    let intrinsics = Vec::new();
    let observations = Vec::new();

    let config = BundleAdjustmentConfig::default();

    // Should handle empty input gracefully
    let pose_priors = vec![None; poses.len()];
    let camera_motions = vec![None; poses.len()];
    let result = bundle_adjust_lm(
        &mut points,
        &mut poses,
        &intrinsics,
        &observations,
        &pose_priors,
        &camera_motions,
        &config,
    );

    assert!(
        result.converged,
        "Should converge immediately on empty input"
    );
    assert_eq!(result.iterations, 0);
}

#[test]
fn test_huber_loss_reduces_outlier_influence() {
    let (points, poses, intrinsics, mut observations) = create_test_scene();

    // Add a gross outlier observation
    if !observations.is_empty() {
        observations[0].x += 500.0; // Very wrong observation
    }

    // With Huber loss
    let config_huber = BundleAdjustmentConfig {
        max_iterations: 50,
        use_huber_loss: true,
        huber_delta: 1.0,
        ..Default::default()
    };

    let pose_priors = vec![None; poses.len()];
    let camera_motions = vec![None; poses.len()];
    let result_huber = bundle_adjust_lm(
        &mut points.clone(),
        &mut poses.clone(),
        &intrinsics,
        &observations,
        &pose_priors,
        &camera_motions,
        &config_huber,
    );

    // Should still produce reasonable result despite outlier
    assert!(
        result_huber.rmse < 100.0,
        "RMSE with Huber should be reasonable: {}",
        result_huber.rmse
    );
}
