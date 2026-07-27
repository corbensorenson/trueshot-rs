//! Capture Module
//!
//! Advanced capture modes including HDR and Focus Stacking.

pub mod adaptive_planner;
pub mod focus_stack;
pub mod hdr;

pub use adaptive_planner::{
    build_camera_candidates, plan_next_capture, AdaptiveCaptureIteration,
    AdaptiveCaptureProvenance, AdaptiveCaptureTermination, AdaptivePlannerConfig,
    CandidateBuildReport, CandidateEvaluation, CandidateRejectionReason, CandidateUtility,
    CaptureCandidate, CapturePlanDecision, CapturePosterior, FocusProbe, RadianceProbe,
    ADAPTIVE_CAPTURE_PROVENANCE_SCHEMA,
};
pub use focus_stack::{FocusStackConfig, FocusStacker, StackAlgorithm, StackDirection};
pub use hdr::{calculate_bracket_evs, HdrAlgorithm, HdrConfig, HdrMerger};
