//! Capture Module
//!
//! Advanced capture modes including HDR and Focus Stacking.

pub mod adaptive_planner;
pub mod adaptive_session;
pub mod focus_stack;
pub mod hdr;
pub mod raw_observation;

pub use adaptive_planner::{
    build_camera_candidates, plan_next_capture, AdaptiveCaptureIteration,
    AdaptiveCaptureProvenance, AdaptiveCaptureTermination, AdaptivePlannerConfig,
    CandidateBuildReport, CandidateEvaluation, CandidateRejectionReason, CandidateUtility,
    CaptureCandidate, CapturePlanDecision, CapturePosterior, FocusProbe, RadianceProbe,
    ADAPTIVE_CAPTURE_PROVENANCE_SCHEMA,
};
pub use adaptive_session::{
    AdaptiveSessionStatus, CaptureRuntimeTelemetry, MeasuredAdaptiveSession,
    MeasuredAdaptiveSessionSnapshot, MEASURED_ADAPTIVE_SESSION_SCHEMA,
};
pub use focus_stack::{FocusStackConfig, FocusStacker, StackAlgorithm, StackDirection};
pub use hdr::{calculate_bracket_evs, HdrAlgorithm, HdrConfig, HdrMerger};
pub use raw_observation::{
    observe_nef_reference, observe_nef_roi, observe_raw_roi, verify_observation_candidate,
    FocusResponseObservation, RadianceObservation, RawAssimilationReport, RawCaptureObservation,
    RawObservationConfig, RawPosteriorAccumulator,
};
