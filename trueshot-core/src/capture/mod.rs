//! Capture Module
//!
//! Advanced capture modes including HDR and Focus Stacking.

pub mod adaptive_planner;
pub mod focus_stack;
pub mod hdr;

pub use adaptive_planner::{
    plan_next_capture, AdaptivePlannerConfig, CandidateUtility, CaptureCandidate,
    CapturePlanDecision, CapturePosterior, FocusProbe, RadianceProbe,
};
pub use focus_stack::{FocusStackConfig, FocusStacker, StackAlgorithm, StackDirection};
pub use hdr::{calculate_bracket_evs, HdrAlgorithm, HdrConfig, HdrMerger};
