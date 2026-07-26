//! Capture Module
//!
//! Advanced capture modes including HDR and Focus Stacking.

pub mod focus_stack;
pub mod hdr;

pub use focus_stack::{FocusStackConfig, FocusStacker, StackAlgorithm, StackDirection};
pub use hdr::{calculate_bracket_evs, HdrAlgorithm, HdrConfig, HdrMerger};
