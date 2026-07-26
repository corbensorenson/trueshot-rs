//! Feature Detection Tests
//!
//! Tests for ORB and SIFT feature detection algorithms.

use image::{DynamicImage, Rgb, RgbImage};
use trueshot_sfm::features::{detect_orb, detect_sift, Descriptor, Keypoint};

/// Create a test image with distinct features (checkerboard pattern)
fn create_test_image(width: u32, height: u32) -> DynamicImage {
    let mut img = RgbImage::new(width, height);

    // Create a checkerboard pattern - corners are easy to detect
    let square_size = 50;
    for y in 0..height {
        for x in 0..width {
            let is_white = ((x / square_size) + (y / square_size)) % 2 == 0;
            let color = if is_white { 255 } else { 0 };
            img.put_pixel(x, y, Rgb([color, color, color]));
        }
    }

    DynamicImage::ImageRgb8(img)
}

/// Create test image with gradient (for SIFT DoG testing)
fn create_gradient_image(width: u32, height: u32) -> DynamicImage {
    let mut img = RgbImage::new(width, height);

    for y in 0..height {
        for x in 0..width {
            let intensity = ((x as f32 / width as f32) * 255.0) as u8;
            img.put_pixel(x, y, Rgb([intensity, intensity, intensity]));
        }
    }

    DynamicImage::ImageRgb8(img)
}

/// Create test image with blob pattern (good for feature detection)
fn create_blob_image(width: u32, height: u32) -> DynamicImage {
    let mut img = RgbImage::new(width, height);

    // White background
    for y in 0..height {
        for x in 0..width {
            img.put_pixel(x, y, Rgb([200, 200, 200]));
        }
    }

    // Add dark blobs at specific locations
    let blobs = [(100, 100), (200, 150), (300, 200), (150, 300), (350, 350)];
    for (cx, cy) in blobs {
        for dy in -20i32..=20 {
            for dx in -20i32..=20 {
                let x = (cx as i32 + dx) as u32;
                let y = (cy as i32 + dy) as u32;
                if x < width && y < height && dx * dx + dy * dy < 400 {
                    img.put_pixel(x, y, Rgb([50, 50, 50]));
                }
            }
        }
    }

    DynamicImage::ImageRgb8(img)
}

#[test]
fn test_orb_detects_features_on_textured_image() {
    let img = create_test_image(640, 480);
    let (keypoints, descriptors) = detect_orb(&img, 500);

    // Should detect features on checkerboard corners
    assert!(
        !keypoints.is_empty(),
        "ORB should detect features on checkerboard"
    );
    assert!(
        keypoints.len() >= 10,
        "Should detect at least 10 features, got {}",
        keypoints.len()
    );

    // Keypoints and descriptors should match
    assert_eq!(
        keypoints.len(),
        descriptors.len(),
        "Keypoint/descriptor count mismatch"
    );
}

#[test]
fn test_orb_respects_max_features() {
    let img = create_test_image(640, 480);

    let max_features = 50;
    let (keypoints, _) = detect_orb(&img, max_features);

    assert!(
        keypoints.len() <= max_features,
        "Should not exceed max features: got {} > {}",
        keypoints.len(),
        max_features
    );
}

#[test]
fn test_orb_keypoints_have_valid_coordinates() {
    let img = create_test_image(640, 480);
    let (keypoints, _) = detect_orb(&img, 100);

    for kp in &keypoints {
        assert!(
            kp.x >= 0.0 && kp.x < 640.0,
            "Invalid x coordinate: {}",
            kp.x
        );
        assert!(
            kp.y >= 0.0 && kp.y < 480.0,
            "Invalid y coordinate: {}",
            kp.y
        );
        assert!(kp.scale > 0.0, "Scale should be positive: {}", kp.scale);
        assert!(
            kp.response > 0.0,
            "Response should be positive: {}",
            kp.response
        );
    }
}

#[test]
fn test_orb_descriptors_are_valid() {
    let img = create_test_image(640, 480);
    let (_, descriptors) = detect_orb(&img, 100);

    for desc in &descriptors {
        // ORB descriptors are 256 bits = 32 bytes
        assert_eq!(desc.data.len(), 32, "ORB descriptor should be 32 bytes");
    }
}

#[test]
fn test_orb_on_uniform_image_gives_few_features() {
    // Uniform image should have few or no features
    let mut img = RgbImage::new(200, 200);
    for y in 0..200 {
        for x in 0..200 {
            img.put_pixel(x, y, Rgb([128, 128, 128]));
        }
    }
    let img = DynamicImage::ImageRgb8(img);

    let (keypoints, _) = detect_orb(&img, 500);

    // Uniform image should have very few features
    assert!(
        keypoints.len() < 50,
        "Uniform image should have few features, got {}",
        keypoints.len()
    );
}

#[test]
fn test_sift_detects_features_on_blob_image() {
    let img = create_blob_image(400, 400);
    let (keypoints, descriptors) = detect_sift(&img, 500);

    // Should detect features around blobs
    assert!(!keypoints.is_empty(), "SIFT should detect features");

    // Keypoints and descriptors should match
    assert_eq!(
        keypoints.len(),
        descriptors.len(),
        "Keypoint/descriptor count mismatch"
    );
}

#[test]
fn test_sift_descriptors_are_128d() {
    let img = create_blob_image(400, 400);
    let (_, descriptors) = detect_sift(&img, 100);

    for desc in &descriptors {
        // SIFT descriptors are 128 dimensions
        assert_eq!(desc.data.len(), 128, "SIFT descriptor should be 128 bytes");
    }
}

#[test]
fn test_descriptor_hamming_distance() {
    let desc1 = Descriptor {
        data: vec![0b11110000; 32],
    };
    let desc2 = Descriptor {
        data: vec![0b11110000; 32],
    };
    let desc3 = Descriptor {
        data: vec![0b00001111; 32],
    };

    // Same descriptors should have 0 distance
    assert_eq!(desc1.hamming_distance(&desc2), 0);

    // All bits flipped should have max distance
    let diff = desc1.hamming_distance(&desc3);
    assert_eq!(diff, 32 * 8, "All bits different should be 256");
}

#[test]
fn test_descriptor_l2_distance() {
    let desc1 = Descriptor {
        data: vec![100; 128],
    };
    let desc2 = Descriptor {
        data: vec![100; 128],
    };
    let desc3 = Descriptor {
        data: vec![200; 128],
    };

    // Same descriptors should have 0 distance
    assert!(desc1.l2_distance(&desc2) < 0.001);

    // Different descriptors should have positive distance
    assert!(desc1.l2_distance(&desc3) > 100.0);
}

#[test]
fn test_feature_detection_is_deterministic() {
    let img = create_test_image(320, 240);

    let (kp1, _) = detect_orb(&img, 100);
    let (kp2, _) = detect_orb(&img, 100);

    // Same image should give same features
    assert_eq!(
        kp1.len(),
        kp2.len(),
        "Feature count should be deterministic"
    );

    // Check first few keypoints are at same positions
    for (a, b) in kp1.iter().zip(kp2.iter()).take(10) {
        assert!((a.x - b.x).abs() < 0.001, "X should match");
        assert!((a.y - b.y).abs() < 0.001, "Y should match");
    }
}
