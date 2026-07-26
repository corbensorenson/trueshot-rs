//! Integration tests for MultiCamSfm
//!
//! Tests the multi-camera SfM pipeline with livescan and SD card reconstruction.

use std::path::PathBuf;
use trueshot_core::reconstruction::{
    CameraId, HighResImage, LivescanFrame, MultiCamConfig, MultiCamSfm,
};
use trueshot_sfm::{CameraIntrinsics, CameraPose, DistortionModel, FeatureType};

fn create_test_intrinsics() -> CameraIntrinsics {
    CameraIntrinsics {
        fx: 500.0,
        fy: 500.0,
        cx: 320.0,
        cy: 240.0,
        width: 640,
        height: 480,
        distortion: vec![],
        distortion_model: DistortionModel::None,
    }
}

#[test]
fn test_multicam_creation_with_10_webcams() {
    let config = MultiCamConfig {
        num_webcams: 10,
        livescan_feature_type: FeatureType::Orb,
        highres_feature_type: FeatureType::Sift,
        ..Default::default()
    };

    let sfm = MultiCamSfm::new(config);
    assert_eq!(sfm.livescan_pose_count(), 0);
}

#[test]
fn test_register_multiple_cameras() {
    let mut sfm = MultiCamSfm::new(MultiCamConfig::default());
    let intrinsics = create_test_intrinsics();

    // Register 10 webcams
    for i in 0..10 {
        sfm.register_camera(CameraId::webcam(i), intrinsics.clone());
    }

    // Each should be registered
    for i in 0..10 {
        let cam_id = CameraId::webcam(i);
        assert!(sfm.camera_intrinsics.contains_key(&cam_id));
    }
}

#[test]
fn test_livescan_frame_processing() {
    let mut sfm = MultiCamSfm::new(MultiCamConfig::default());
    let intrinsics = create_test_intrinsics();

    // Register camera
    let cam_id = CameraId::webcam(0);
    sfm.register_camera(cam_id.clone(), intrinsics.clone());

    // Process frames at different angles
    for angle in (0..360).step_by(10) {
        let frame = LivescanFrame {
            camera_id: cam_id.clone(),
            timestamp_ms: angle as u64 * 100,
            pose: CameraPose::identity(),
            intrinsics: intrinsics.clone(),
            image_data: vec![0; 640 * 480 * 3],
            width: 640,
            height: 480,
            turntable_angle: angle as f32,
            features: vec![],
        };

        sfm.process_livescan_frame(frame).unwrap();
    }

    // Should have 36 poses (360/10)
    assert_eq!(sfm.livescan_pose_count(), 36);
}

#[test]
fn test_livescan_pose_retrieval() {
    let mut sfm = MultiCamSfm::new(MultiCamConfig::default());
    let intrinsics = create_test_intrinsics();
    let cam_id = CameraId::webcam(0);

    sfm.register_camera(cam_id.clone(), intrinsics.clone());

    // Add frame at 45 degrees
    let frame = LivescanFrame {
        camera_id: cam_id.clone(),
        timestamp_ms: 4500,
        pose: CameraPose::identity(),
        intrinsics: intrinsics.clone(),
        image_data: vec![],
        width: 640,
        height: 480,
        turntable_angle: 45.0,
        features: vec![],
    };

    sfm.process_livescan_frame(frame).unwrap();

    // Should be able to retrieve pose at ~45 degrees
    let pose = sfm.get_livescan_pose(&cam_id, 45.0);
    assert!(pose.is_some());

    // Should also work for 44.5 (rounds to 45)
    let pose = sfm.get_livescan_pose(&cam_id, 44.5);
    assert!(pose.is_some());
}

#[test]
fn test_sd_card_ingestion() {
    let mut sfm = MultiCamSfm::new(MultiCamConfig::default());
    let intrinsics = create_test_intrinsics();

    // Create mock high-res images
    let images: Vec<HighResImage> = (0..10)
        .map(|i| HighResImage {
            camera_id: CameraId::dslr("nikon_z9"),
            path: PathBuf::from(format!("/tmp/test_img_{}.nef", i)),
            width: 8256,
            height: 5504,
            intrinsics: intrinsics.clone(),
            timestamp_ms: Some(i as u64 * 1000),
            focus_distance: Some(2.0 + (i as f32 * 0.1)),
            exposure_value: None,
            bracket_group: None,
            pixels: None,
        })
        .collect();

    sfm.ingest_sd_card(images).unwrap();

    // Verify images were ingested
    assert_eq!(sfm.pending_highres.len(), 10);
}

#[test]
fn test_focus_group_detection() {
    let mut sfm = MultiCamSfm::new(MultiCamConfig::default());
    let intrinsics = create_test_intrinsics();

    // Create focus-stacked images (3 focus distances, 2 groups)
    let mut images = Vec::new();

    // Group 0: 3 focus distances
    for focus in 0..3 {
        images.push(HighResImage {
            camera_id: CameraId::dslr("nikon_z9"),
            path: PathBuf::from(format!("/tmp/group0_focus{}.nef", focus)),
            width: 8256,
            height: 5504,
            intrinsics: intrinsics.clone(),
            timestamp_ms: Some(focus as u64 * 100),
            focus_distance: Some(1.5 + (focus as f32 * 0.5)),
            exposure_value: None,
            bracket_group: Some(0),
            pixels: None,
        });
    }

    // Group 1: 3 focus distances
    for focus in 0..3 {
        images.push(HighResImage {
            camera_id: CameraId::dslr("nikon_z9"),
            path: PathBuf::from(format!("/tmp/group1_focus{}.nef", focus)),
            width: 8256,
            height: 5504,
            intrinsics: intrinsics.clone(),
            timestamp_ms: Some(1000 + focus as u64 * 100),
            focus_distance: Some(1.5 + (focus as f32 * 0.5)),
            exposure_value: None,
            bracket_group: Some(1),
            pixels: None,
        });
    }

    sfm.ingest_sd_card(images).unwrap();

    // Should have 2 focus groups
    assert_eq!(sfm.focus_groups.len(), 2);
    assert_eq!(sfm.focus_groups.get(&0).map(|g| g.len()), Some(3));
    assert_eq!(sfm.focus_groups.get(&1).map(|g| g.len()), Some(3));
}

#[test]
fn test_dslr_camera_id() {
    let cam = CameraId::dslr("nikon_z9");
    assert_eq!(cam.0, "dslr_nikon_z9");

    let cam2 = CameraId::dslr("canon_r5");
    assert_eq!(cam2.0, "dslr_canon_r5");

    // Different cameras should not be equal
    assert_ne!(cam, cam2);
}

#[test]
fn test_multicam_config_defaults() {
    let config = MultiCamConfig::default();

    assert_eq!(config.num_webcams, 1);
    assert_eq!(config.livescan_feature_type, FeatureType::Orb);
    assert_eq!(config.highres_feature_type, FeatureType::Sift);
    assert_eq!(config.livescan_max_features, 500);
    assert_eq!(config.highres_max_features, 8000);
    assert!(config.enable_dense);
}

#[test]
fn test_empty_reconstruction_fails_gracefully() {
    let sfm = MultiCamSfm::new(MultiCamConfig::default());

    // No images ingested - should handle gracefully
    // (In production this would return an error)
    assert!(sfm.sparse_reconstruction().is_none());
    assert!(sfm.mesh().is_none());
    assert!(sfm.depth_maps().is_empty());
}
