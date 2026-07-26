/// Vision Lib Entry Point
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

// OpenCV-dependent modules (require "opencv" feature)
pub mod autocrop;
#[cfg(feature = "opencv")]
pub mod background;
#[cfg(feature = "opencv")]
pub mod background_sub;
pub mod barcode;
pub mod change_detection;
#[cfg(feature = "opencv")]
pub mod cv;
pub mod fiducial;
pub mod gauge;
pub mod inpainting;
pub mod iqa;
pub mod markers;
pub mod metrics;
pub mod polarization;
#[cfg(feature = "opencv")]
pub mod pose;
pub mod preview;
pub mod proc_gen;
#[cfg(feature = "opencv")]
pub mod sfm;
pub mod simd;
pub mod spectral;
#[cfg(feature = "opencv")]
pub mod tracker;
pub mod volume;

// Native implementations (no OpenCV required) - ALWAYS AVAILABLE
pub mod features;
pub mod geometry;
pub mod matching;

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
        pub fn new() -> anyhow::Result<Self> {
            Ok(Self)
        }
    }
}

#[cfg(test)]
mod tests;
