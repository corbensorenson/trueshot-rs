//! Unit tests for TrueShot core components

use anyhow::Result;
use trueshot_core::config::AppConfig;
use trueshot_core::object_detection::BoundingBox;
use std::sync::Arc;
use std::thread;

#[test]
fn test_app_config_load_defaults() -> Result<()> {
    let config = AppConfig::load()?;
    assert!(!config.server.host.is_empty());
    assert!(config.server.port > 0);
    assert!(!config.paths.data_dir.as_os_str().is_empty());
    assert!(!config.paths.temp_dir.as_os_str().is_empty());
    Ok(())
}

#[test]
fn test_app_config_serialization_roundtrip() -> Result<()> {
    let config = AppConfig::load()?;
    let json = serde_json::to_string_pretty(&config)?;
    let decoded: AppConfig = serde_json::from_str(&json)?;
    assert_eq!(config.server.host, decoded.server.host);
    assert_eq!(config.server.port, decoded.server.port);
    assert_eq!(config.paths.data_dir, decoded.paths.data_dir);
    assert_eq!(config.paths.temp_dir, decoded.paths.temp_dir);
    assert_eq!(config.photogrammetry.use_gpu, decoded.photogrammetry.use_gpu);
    Ok(())
}

#[test]
fn test_bounding_box_operations() {
    let bbox = BoundingBox {
        x: 100,
        y: 200,
        width: 300,
        height: 400,
    };

    assert_eq!(bbox.area(), 300 * 400);
    assert!(bbox.contains_point(200, 300));
    assert!(!bbox.contains_point(50, 100));
    assert!(!bbox.contains_point(500, 700));

    let other = BoundingBox {
        x: 150,
        y: 250,
        width: 200,
        height: 200,
    };

    let intersection = bbox.intersection(&other).expect("Expected intersection");
    assert_eq!(intersection.x, 150);
    assert_eq!(intersection.y, 250);
    assert_eq!(intersection.width, 200);
    assert_eq!(intersection.height, 150);
}

#[test]
fn test_bounding_box_scaling() {
    let bbox = BoundingBox {
        x: 100,
        y: 200,
        width: 300,
        height: 400,
    };

    let scaled = bbox.scale(2.0);
    assert_eq!(scaled.x, 200);
    assert_eq!(scaled.y, 400);
    assert_eq!(scaled.width, 600);
    assert_eq!(scaled.height, 800);

    let scaled_down = bbox.scale(0.5);
    assert_eq!(scaled_down.x, 50);
    assert_eq!(scaled_down.y, 100);
    assert_eq!(scaled_down.width, 150);
    assert_eq!(scaled_down.height, 200);
}

#[test]
fn test_bounding_box_clamping_via_intersection() {
    let bbox = BoundingBox {
        x: 50,
        y: 100,
        width: 300,
        height: 400,
    };

    let bounds = BoundingBox {
        x: 0,
        y: 0,
        width: 200,
        height: 300,
    };

    let clamped = bbox.intersection(&bounds).expect("Expected clamped intersection");
    assert_eq!(clamped.x, 50);
    assert_eq!(clamped.y, 100);
    assert_eq!(clamped.width, 150);
    assert_eq!(clamped.height, 200);
}

#[test]
fn test_memory_safety() {
    for i in 0..1000 {
        let bbox = BoundingBox {
            x: i,
            y: i,
            width: 100,
            height: 100,
        };
        let _area = bbox.area();
    }
}

#[test]
fn test_concurrent_access() -> Result<()> {
    let config = Arc::new(AppConfig::load()?);
    let mut handles = Vec::new();

    for _ in 0..10 {
        let config_clone = Arc::clone(&config);
        let handle = thread::spawn(move || {
            for _ in 0..100 {
                let _json = serde_json::to_string(&*config_clone).unwrap();
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    Ok(())
}
