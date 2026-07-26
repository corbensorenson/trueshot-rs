//! Pipeline Integration Tests
//!
//! End-to-end tests for the SfM pipeline.

use trueshot_sfm::{SfmPipeline, SfmConfig, FeatureType, SparseReconstruction, DistortionModel, CameraIntrinsics};
use std::path::PathBuf;
use tempfile::TempDir;

/// Create synthetic test images
fn create_test_images(dir: &std::path::Path, count: usize) -> Vec<PathBuf> {
    use image::{RgbImage, Rgb};
    
    let mut paths = Vec::new();
    
    for i in 0..count {
        let mut img = RgbImage::new(640, 480);
        
        // Create a pattern that varies between images (simulate camera motion)
        let offset = i as u32 * 20;
        
        // Checkerboard with offset
        for y in 0..480 {
            for x in 0..640 {
                let is_white = (((x + offset) / 50) + (y / 50)) % 2 == 0;
                let color = if is_white { 240 } else { 20 };
                img.put_pixel(x, y, Rgb([color, color, color]));
            }
        }
        
        // Add some unique markers per image
        let marker_x = 100 + i as u32 * 50;
        for dy in 0..30 {
            for dx in 0..30 {
                if marker_x + dx < 640 {
                    img.put_pixel(marker_x + dx, 100 + dy, Rgb([255, 0, 0]));
                }
            }
        }
        
        let path = dir.join(format!("test_image_{}.png", i));
        img.save(&path).unwrap();
        paths.push(path);
    }
    
    paths
}

#[test]
fn test_pipeline_creation() {
    let config = SfmConfig::default();
    let pipeline = SfmPipeline::new(config);
    
    // Pipeline should be empty initially
    assert!(pipeline.get_reconstruction().is_none());
}

#[test] 
fn test_sfm_config_default() {
    let config = SfmConfig::default();
    
    assert_eq!(config.feature_type, FeatureType::Orb);
    assert!(config.max_features > 0);
    assert!(config.match_ratio > 0.0 && config.match_ratio < 1.0);
    assert!(config.min_matches > 0);
    assert!(config.ba_iterations > 0);
}

#[test]
fn test_pipeline_add_images() {
    let temp_dir = TempDir::new().unwrap();
    let image_paths = create_test_images(temp_dir.path(), 3);
    
    let mut pipeline = SfmPipeline::new(SfmConfig::default());
    
    // Convert to string slices
    let result = pipeline.add_images(&image_paths);
    
    assert!(result.is_ok(), "Should successfully add images: {:?}", result.err());
}

#[test]
fn test_pipeline_requires_minimum_images() {
    let temp_dir = TempDir::new().unwrap();
    let image_paths = create_test_images(temp_dir.path(), 1);
    
    let mut pipeline = SfmPipeline::new(SfmConfig::default());
    pipeline.add_images(&image_paths).unwrap();
    
    // Should fail with only 1 image
    let result = pipeline.run();
    
    assert!(result.is_err(), "Should fail with only 1 image");
}

#[test]
fn test_reconstruction_export_ply() {
    // Create a minimal reconstruction
    use nalgebra as na;
    use trueshot_sfm::{Point3D, CameraPose, CameraIntrinsics};
    
    let reconstruction = SparseReconstruction {
        points: vec![
            Point3D {
                position: na::Point3::new(0.0, 0.0, 1.0),
                color: [255, 0, 0],
                error: 0.1,
                track: vec![(0, 0)],
            },
            Point3D {
                position: na::Point3::new(1.0, 0.0, 1.0),
                color: [0, 255, 0],
                error: 0.1,
                track: vec![(0, 1)],
            },
        ],
        cameras: vec![CameraIntrinsics {
            fx: 500.0,
            fy: 500.0,
            cx: 320.0,
            cy: 240.0,
            width: 640,
            height: 480,
            distortion: vec![],
            distortion_model: DistortionModel::None,
        }],
        poses: vec![CameraPose::identity()],
        image_names: vec!["test.jpg".to_string()],
    };
    
    let temp_dir = TempDir::new().unwrap();
    let ply_path = temp_dir.path().join("test.ply");
    
    let result = reconstruction.export_ply(&ply_path);
    
    assert!(result.is_ok(), "PLY export failed: {:?}", result.err());
    assert!(ply_path.exists(), "PLY file should exist");
    
    // Check file content
    let content = std::fs::read_to_string(&ply_path).unwrap();
    assert!(content.contains("ply"), "Should be a PLY file");
    assert!(content.contains("element vertex 2"), "Should have 2 vertices");
}

#[test]
fn test_camera_intrinsics_to_matrix() {
    use trueshot_sfm::CameraIntrinsics;
    
    let K = CameraIntrinsics {
        fx: 500.0,
        fy: 500.0,
        cx: 320.0,
        cy: 240.0,
        width: 640,
        height: 480,
        distortion: vec![],
        distortion_model: DistortionModel::None,
    };
    
    let mat = K.to_matrix();
    
    assert!((mat[(0, 0)] - 500.0).abs() < 1e-10, "fx should be 500");
    assert!((mat[(1, 1)] - 500.0).abs() < 1e-10, "fy should be 500");
    assert!((mat[(0, 2)] - 320.0).abs() < 1e-10, "cx should be 320");
    assert!((mat[(1, 2)] - 240.0).abs() < 1e-10, "cy should be 240");
    assert!((mat[(2, 2)] - 1.0).abs() < 1e-10, "bottom right should be 1");
}

#[test]
fn test_camera_pose_identity() {
    use trueshot_sfm::CameraPose;
    use nalgebra as na;
    
    let pose = CameraPose::identity();
    
    // Identity rotation
    let angle = pose.rotation.angle();
    assert!(angle.abs() < 1e-10, "Identity rotation should have zero angle");
    
    // Zero translation
    assert!(pose.translation.norm() < 1e-10, "Identity translation should be zero");
}

#[test]
fn test_camera_pose_to_matrix() {
    use trueshot_sfm::CameraPose;
    use nalgebra as na;
    
    let pose = CameraPose {
        rotation: na::UnitQuaternion::from_euler_angles(0.0, 0.0, std::f64::consts::FRAC_PI_2),
        translation: na::Vector3::new(1.0, 2.0, 3.0),
    };
    
    let mat = pose.to_matrix();
    
    // Check it's a 4x4 matrix
    assert_eq!(mat.nrows(), 4);
    assert_eq!(mat.ncols(), 4);
    
    // Check translation in last column
    assert!((mat[(0, 3)] - 1.0).abs() < 1e-10);
    assert!((mat[(1, 3)] - 2.0).abs() < 1e-10);
    assert!((mat[(2, 3)] - 3.0).abs() < 1e-10);
    
    // Bottom row should be [0, 0, 0, 1]
    assert!((mat[(3, 3)] - 1.0).abs() < 1e-10);
}

#[test]
fn test_point3d_structure() {
    use trueshot_sfm::Point3D;
    use nalgebra as na;
    
    let point = Point3D {
        position: na::Point3::new(1.0, 2.0, 3.0),
        color: [128, 64, 32],
        error: 0.5,
        track: vec![(0, 10), (1, 20), (2, 30)],
    };
    
    assert_eq!(point.position.x, 1.0);
    assert_eq!(point.position.y, 2.0);
    assert_eq!(point.position.z, 3.0);
    assert_eq!(point.color, [128, 64, 32]);
    assert_eq!(point.error, 0.5);
    assert_eq!(point.track.len(), 3);
}

#[test]
fn test_feature_type_variants() {
    use trueshot_sfm::FeatureType;
    
    let orb = FeatureType::Orb;
    let sift = FeatureType::Sift;
    let akaze = FeatureType::Akaze;
    
    // Check equality
    assert_eq!(orb, FeatureType::Orb);
    assert_ne!(orb, sift);
    assert_ne!(sift, akaze);
}

#[test]
fn test_sfm_config_custom() {
    let config = SfmConfig {
        feature_type: FeatureType::Sift,
        max_features: 5000,
        match_ratio: 0.8,
        min_matches: 50,
        ba_iterations: 100,
        local_ba_window: 6,
        local_ba_stride: 2,
        local_ba_iterations: 20,
        local_ba_min_points: 150,
        local_ba_min_rmse: 0.9,
        enable_dense: false,
        num_threads: 4,
    };
    
    assert_eq!(config.feature_type, FeatureType::Sift);
    assert_eq!(config.max_features, 5000);
    assert_eq!(config.match_ratio, 0.8);
    assert_eq!(config.min_matches, 50);
    assert_eq!(config.ba_iterations, 100);
    assert!(!config.enable_dense);
    assert_eq!(config.num_threads, 4);
}
