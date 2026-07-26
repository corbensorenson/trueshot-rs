/// Vision Lib Entry Point
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

// OpenCV-dependent modules (require "opencv" feature)
#[cfg(feature = "opencv")]
pub mod tracker;
#[cfg(feature = "opencv")]
pub mod background;
#[cfg(feature = "opencv")]
pub mod background_sub;
pub mod inpainting;
pub mod change_detection;
pub mod polarization;
pub mod spectral;
pub mod metrics;
pub mod markers;
pub mod autocrop;
pub mod simd;
pub mod proc_gen;
pub mod iqa;
pub mod barcode;
pub mod preview;
pub mod volume;
pub mod fiducial;
pub mod gauge;
#[cfg(feature = "opencv")]
pub mod cv;
#[cfg(feature = "opencv")]
pub mod sfm;
#[cfg(feature = "opencv")]
pub mod pose;

// Native implementations (no OpenCV required) - ALWAYS AVAILABLE
pub mod features;
pub mod matching;
pub mod geometry;

// Stubs for when OpenCV is not available
#[cfg(not(feature = "opencv"))]
pub mod cv {
    //! Stub module when OpenCV is not available
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum DistortionModel {
        None,
        BrownConrady,
        Fisheye,
    }
    pub struct FeatureMatcher;
    impl FeatureMatcher {
        pub fn new() -> anyhow::Result<Self> { Ok(Self) }
    }
}

#[cfg(test)]
mod tests;
