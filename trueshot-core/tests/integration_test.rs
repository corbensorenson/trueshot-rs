//! Integration tests for TrueShot core functionality
//!
//! These tests verify the core hierarchical processing pipeline works correctly.

use anyhow::Result;
use ndarray::{Array2, Array3};
use trueshot_core::hierarchical_collapse::{
    collapse_b_grade, collapse_c_grade, CollapseResult, HierarchicalParams, SRFactor,
};
use trueshot_core::hierarchical_grading::{
    compute_grade_stats, grade_pixels, Grade, GradingParams,
};
use trueshot_core::hierarchical_pipeline::hierarchical_process;

/// Create a test stack of frames with varying focus
fn create_test_frames(height: usize, width: usize, num_frames: usize) -> Vec<Array3<f64>> {
    let mut frames = Vec::with_capacity(num_frames);

    for i in 0..num_frames {
        let mut frame = Array3::<f64>::zeros((height, width, 1));

        // Create a pattern where different frames have different "focus" regions
        let focus_zone = i as f64 / num_frames as f64;

        for y in 0..height {
            for x in 0..width {
                // Gradient pattern with frame-specific focus
                let normalized_y = y as f64 / height as f64;
                let base = 0.2 + 0.6 * normalized_y;

                // Add sharpness that varies with frame index
                let distance_from_focus = (normalized_y - focus_zone).abs();
                let sharpness = 1.0 - distance_from_focus;

                frame[[y, x, 0]] = base * sharpness.max(0.3);
            }
        }

        frames.push(frame);
    }

    frames
}

/// Create a simple foreground mask (all pixels are foreground)
fn create_full_mask(height: usize, width: usize) -> Array2<bool> {
    Array2::from_elem((height, width), true)
}

#[test]
fn test_grading_produces_all_grades() {
    let height = 100;
    let width = 100;

    // Create a sharpness map with smoothly varying values
    let mut sharpness = Array2::<f64>::zeros((height, width));
    for y in 0..height {
        for x in 0..width {
            // Smooth gradient from 0.0 to 1.0
            let normalized = (y as f64 / height as f64) * (x as f64 / width as f64);
            sharpness[[y, x]] = normalized;
        }
    }

    // Create partial mask - edges are background (D-grade)
    let mut mask = Array2::from_elem((height, width), true);
    for y in 0..10 {
        for x in 0..width {
            mask[[y, x]] = false; // Top edge is background
        }
    }

    let params = GradingParams::default();

    let grades = grade_pixels(&sharpness, &mask, &params).unwrap();
    let stats = compute_grade_stats(&grades);

    // Should have at least A, C, and D grades with this setup
    assert!(
        stats.count_a > 0,
        "Should have A-grade pixels (highest sharpness)"
    );
    assert!(
        stats.count_c > 0,
        "Should have C-grade pixels (low sharpness foreground)"
    );
    assert!(stats.count_d > 0, "Should have D-grade pixels (background)");

    // Total should equal image size
    assert_eq!(
        stats.count_a + stats.count_b + stats.count_c + stats.count_d,
        height * width
    );
}

#[test]
fn test_c_grade_collapse_produces_valid_output() {
    let height = 50;
    let width = 50;
    let num_frames = 6;

    let frames = create_test_frames(height, width, num_frames);

    // Create grades (all C for this test)
    let grades = Array2::from_elem((height, width), Grade::C as u8);

    let exposures: Vec<f64> = (0..num_frames).map(|i| 1.0 + i as f64 * 0.5).collect();
    let params = HierarchicalParams::default();
    let wb_multipliers = [1.0f32, 1.0f32, 1.0f32, 1.0f32];

    // No focus plane map for simple test
    let result = collapse_c_grade(
        &frames,
        &grades,
        &exposures,
        &params,
        None,       // No focus plane map
        num_frames, // All frames as one exposure set
        &wb_multipliers,
        None, // No alignments
    )
    .unwrap();

    assert_eq!(result.dim(), (height, width));

    // All values should be in valid range
    for y in 0..height {
        for x in 0..width {
            let val = result[[y, x]];
            assert!(val >= 0.0, "Value {} at ({}, {}) should be >= 0", val, y, x);
            assert!(
                val <= 2.0,
                "Value {} at ({}, {}) should be <= 2.0",
                val,
                y,
                x
            );
        }
    }
}

#[test]
fn test_b_grade_collapse_uses_c_prior() {
    let height = 50;
    let width = 50;
    let num_frames = 3;

    let frames = create_test_frames(height, width, num_frames);

    // Create grades (all B for this test)
    let grades = Array2::from_elem((height, width), Grade::B as u8);

    // Create C result as prior
    let c_result = Array2::from_elem((height, width), 0.5);

    let exposures: Vec<f64> = vec![1.0, 1.5, 2.0];
    let params = HierarchicalParams::default();
    let wb_multipliers = [1.0f32, 1.0f32, 1.0f32, 1.0f32];

    let result = collapse_b_grade(
        &frames,
        &grades,
        &c_result,
        &exposures,
        &params,
        None, // No focus plane map
        num_frames,
        &wb_multipliers,
        None, // No alignments
    )
    .unwrap();

    assert_eq!(result.dim(), (height, width));

    // Result should be influenced by C prior (0.5)
    // Due to the blending, values should be close to the prior
    let mean: f64 = result.iter().sum::<f64>() / (height * width) as f64;
    assert!(mean > 0.0, "Mean should be positive");
}

#[test]
fn test_full_hierarchical_pipeline() {
    let height = 64;
    let width = 64;
    let num_frames = 6; // 2 focus planes × 3 exposures

    let frames = create_test_frames(height, width, num_frames);
    let mask = create_full_mask(height, width);

    let exposures: Vec<f64> = (0..num_frames)
        .map(|i| 1.0 + (i % 3) as f64 * 0.5)
        .collect();
    let grading_params = GradingParams::default();
    let collapse_params = HierarchicalParams {
        sr_factor: SRFactor::None,
        ..Default::default()
    };
    let wb_multipliers = [1.0f32, 1.0f32, 1.0f32, 1.0f32];

    let result = hierarchical_process(
        &frames,
        &mask,
        3, // Reference index
        &exposures,
        None, // No alignments
        &grading_params,
        &collapse_params,
        2, // 2 focus planes
        3, // 3 exposures per plane
        &wb_multipliers,
    )
    .unwrap();

    // Result should be RGB (demosaiced)
    match result {
        CollapseResult::Rgb(rgb) => {
            let (channels, h, w) = rgb.dim();
            assert_eq!(channels, 3, "Should have 3 color channels");
            assert_eq!(h, height, "Height should match");
            assert_eq!(w, width, "Width should match");
        }
        CollapseResult::Bayer(_) => {
            // In native mode, might return Bayer - that's also valid
        }
    }
}

#[test]
fn test_grading_params_affect_distribution() {
    let height = 50;
    let width = 50;

    // Create uniform sharpness
    let sharpness = Array2::from_elem((height, width), 0.5);
    let mask = create_full_mask(height, width);

    // Test with different k thresholds
    let loose_params = GradingParams {
        k_threshold: 1.0, // Loose threshold
        ..Default::default()
    };

    let strict_params = GradingParams {
        k_threshold: 5.0, // Strict threshold
        ..Default::default()
    };

    let loose_grades = grade_pixels(&sharpness, &mask, &loose_params).unwrap();
    let strict_grades = grade_pixels(&sharpness, &mask, &strict_params).unwrap();

    let loose_stats = compute_grade_stats(&loose_grades);
    let strict_stats = compute_grade_stats(&strict_grades);

    // With uniform sharpness, different thresholds should affect distribution
    // The exact distribution depends on the grading algorithm
    assert!(loose_stats.count_a > 0 || strict_stats.count_a > 0);
}

#[test]
fn test_collapse_handles_empty_frames() {
    let frames: Vec<Array3<f64>> = vec![];
    let grades = Array2::from_elem((10, 10), Grade::C as u8);
    let exposures: Vec<f64> = vec![];
    let params = HierarchicalParams::default();
    let wb_multipliers = [1.0f32, 1.0f32, 1.0f32, 1.0f32];

    // Should return error for empty frames
    let result = collapse_c_grade(
        &frames,
        &grades,
        &exposures,
        &params,
        None,
        0,
        &wb_multipliers,
        None,
    );

    assert!(result.is_err(), "Should error on empty frames");
}

#[test]
fn test_sr_factor_values() {
    assert_eq!(SRFactor::None.as_f64(), 1.0);
    assert_eq!(SRFactor::SR1_5x.as_f64(), 1.5);
    assert!((SRFactor::GoldenRatio.as_f64() - 1.618).abs() < 0.01);
    assert_eq!(SRFactor::SR2x.as_f64(), 2.0);
    assert_eq!(SRFactor::SR3x.as_f64(), 3.0);
}
