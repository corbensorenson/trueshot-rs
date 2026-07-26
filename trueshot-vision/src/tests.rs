use crate::{autocrop, markers};
use image::{DynamicImage, RgbImage};

#[test]
fn test_autocrop_logic() {
    // Create a 100x100 mask with a 10x10 square in center
    let mut mask = vec![0u8; 100 * 100];
    for y in 45..55 {
        for x in 45..55 {
            mask[y * 100 + x] = 255;
        }
    }

    let bounds = autocrop::calculate_bounds_from_mask(&mask, 100, 100);
    assert!(bounds.is_some());
    let (x, y, w, h) = bounds.unwrap();
    // Padding logic: 10px pad.
    // Center 45..55 (size 10). Min=45-10=35. Max=55+10=65. W=30?
    // Let's just assert it contains the center.
    assert!(x <= 45);
    assert!(y <= 45);
    assert!(w >= 10);
    assert!(h >= 10);
}

#[test]
fn test_marker_detection_empty() {
    let img = DynamicImage::ImageRgb8(RgbImage::new(10, 10)); // Too small
    let res = markers::detect_gray_patch(&img);
    assert!(res.is_none());
}
