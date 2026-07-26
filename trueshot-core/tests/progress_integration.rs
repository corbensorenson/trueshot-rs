//! Integration tests for progress and cancellation system
//!
//! Tests CancellationToken, OperationEstimator, and progress tracking.

use trueshot_core::progress::{
    CancellationToken, OperationEstimator, OperationPreview,
    ProcessingPhase, ProgressTracker
};
use std::sync::Arc;
use std::thread;

#[test]
fn test_cancellation_token_basic() {
    let token = CancellationToken::new();
    
    assert!(!token.is_cancelled(), "Token should start uncancelled");
    
    token.cancel();
    
    assert!(token.is_cancelled(), "Token should be cancelled after cancel()");
}

#[test]
fn test_cancellation_token_check() {
    let token = CancellationToken::new();
    
    // Should succeed when not cancelled
    assert!(token.check().is_ok(), "check() should succeed when not cancelled");
    
    token.cancel();
    
    // Should fail when cancelled
    assert!(token.check().is_err(), "check() should fail when cancelled");
}

#[test]
fn test_cancellation_token_thread_safety() {
    let token = CancellationToken::new();
    let token_clone = token.clone();
    
    let handle = thread::spawn(move || {
        // Wait a bit then cancel
        thread::sleep(std::time::Duration::from_millis(50));
        token_clone.cancel();
    });
    
    // Initially not cancelled
    assert!(!token.is_cancelled());
    
    handle.join().unwrap();
    
    // Now should be cancelled
    assert!(token.is_cancelled(), "Cancellation should propagate across threads");
}

#[test]
fn test_cancellation_child_token() {
    let parent = CancellationToken::new();
    let child = parent.child();
    
    // Neither cancelled initially
    assert!(!parent.is_cancelled());
    assert!(!child.is_cancelled());
    
    // Cancel child
    child.cancel();
    
    // Both should be cancelled (shared state)
    assert!(parent.is_cancelled(), "Parent should see child cancellation");
    assert!(child.is_cancelled());
}

#[test]
fn test_operation_estimator_burst_collapse() {
    let preview = OperationEstimator::estimate_burst_collapse(10, (4000, 3000));
    
    assert_eq!(preview.name, "Burst Collapse");
    assert_eq!(preview.item_count, 10);
    assert!(preview.estimated_seconds > 0.0, "Should estimate time");
    assert!(preview.estimated_memory_mb > 0.0, "Should estimate memory");
    assert!(!preview.uses_gpu, "Burst collapse doesn't use GPU");
    assert!(!preview.phases.is_empty(), "Should list phases");
}

#[test]
fn test_operation_estimator_gaussian_splatting() {
    let preview = OperationEstimator::estimate_gaussian_splatting(100, 500_000);
    
    assert!(preview.name.contains("Gaussian"));
    assert_eq!(preview.item_count, 100);
    assert!(preview.uses_gpu, "3DGS uses GPU");
    assert!(preview.estimated_seconds > preview.estimated_seconds * 0.0, "Positive time");
}

#[test]
fn test_operation_estimator_photogrammetry_quality() {
    let low = OperationEstimator::estimate_photogrammetry(50, "low");
    let high = OperationEstimator::estimate_photogrammetry(50, "high");
    
    // Higher quality should take longer
    assert!(
        high.estimated_seconds > low.estimated_seconds,
        "High quality should take longer: {} vs {}",
        high.estimated_seconds,
        low.estimated_seconds
    );
    
    // Higher quality should use more memory
    assert!(
        high.estimated_memory_mb > low.estimated_memory_mb,
        "High quality should use more memory"
    );
}

#[test]
fn test_operation_preview_formatting() {
    let preview = OperationPreview {
        name: "Test Operation".to_string(),
        estimated_seconds: 125.0, // 2 min 5 sec
        estimated_memory_mb: 2500.0, // 2.4 GB
        estimated_disk_mb: 500.0,
        item_count: 42,
        uses_gpu: true,
        phases: vec!["Phase 1".to_string(), "Phase 2".to_string()],
    };
    
    let duration = preview.format_duration();
    assert!(duration.contains("minute"), "Should format as minutes: {}", duration);
    
    let memory = preview.format_memory();
    assert!(memory.contains("GB"), "Should format as GB: {}", memory);
    
    let summary = preview.summary();
    assert!(summary.contains("Test Operation"), "Summary should include name");
    assert!(summary.contains("42 items"), "Summary should include item count");
}

#[test]
fn test_progress_tracker_phases() {
    let tracker = ProgressTracker::new();
    
    // Start a phase
    tracker.start_phase(ProcessingPhase::ImageAnalysis, 100, "Analyzing images".to_string());
    
    let progress = tracker.get_current_progress();
    assert!(progress.is_some(), "Should have current progress");
    
    let p = progress.unwrap();
    assert_eq!(p.phase, ProcessingPhase::ImageAnalysis);
    assert_eq!(p.total, 100);
    
    // Update progress
    tracker.update_progress(50, Some("Halfway done".to_string()));
    
    let progress2 = tracker.get_current_progress();
    assert!(progress2.is_some());
    assert_eq!(progress2.unwrap().current, 50);
    
    // Complete phase
    tracker.complete_phase();
    
    let progress3 = tracker.get_current_progress();
    assert!(progress3.is_none(), "Phase should be complete");
}

#[test]
fn test_progress_tracker_callbacks() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    
    let tracker = ProgressTracker::new();
    let callback_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&callback_count);
    
    tracker.add_callback(move |_progress| {
        count_clone.fetch_add(1, Ordering::SeqCst);
    });
    
    // Start phase triggers callback
    tracker.start_phase(ProcessingPhase::BurstCollapse, 10, "Test".to_string());
    
    // Update triggers callback
    tracker.update_progress(5, None);
    
    assert!(
        callback_count.load(Ordering::SeqCst) >= 2,
        "Callbacks should be called"
    );
}
