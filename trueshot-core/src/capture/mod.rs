//! Capture Module
//!
//! Advanced capture modes including HDR and Focus Stacking.

pub mod hdr;
pub mod focus_stack;

pub use hdr::{HdrMerger, HdrConfig, HdrAlgorithm, calculate_bracket_evs};
pub use focus_stack::{FocusStacker, FocusStackConfig, StackAlgorithm, StackDirection};
