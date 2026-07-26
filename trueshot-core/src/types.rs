//! Core data structures for the pixelcollapse2 pipeline.

use ndarray::Array3;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Alignment information for each frame
#[derive(Debug, Clone)]
pub struct AlignmentInfo {
    pub dx: f64,    // Subpixel shift in x
    pub dy: f64,    // Subpixel shift in y
    pub scale: f64, // Magnification scale (1.0 = no change, >1.0 = zoomed in, <1.0 = zoomed out)
}

/// Rectangle region in image coordinates (f64 for subpixel precision)
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Convert to integer bounds (inclusive)
    pub fn to_bounds(&self) -> (usize, usize, usize, usize) {
        let x0 = self.x.floor() as usize;
        let y0 = self.y.floor() as usize;
        let x1 = (self.x + self.width).ceil() as usize;
        let y1 = (self.y + self.height).ceil() as usize;
        (x0, y0, x1, y1)
    }

    /// Erode/dilate by factor (negative = erode, positive = dilate)
    pub fn adjust(&self, factor: f64) -> Self {
        let dx = self.width * factor;
        let dy = self.height * factor;
        Self {
            x: self.x - dx,
            y: self.y - dy,
            width: self.width + 2.0 * dx,
            height: self.height + 2.0 * dy,
        }
    }
}

/// Bayer frame in linear f64 space (H×W×4 for RGGB)
#[derive(Debug, Clone)]
pub struct BayerFrame {
    /// Raw Bayer data: shape (height, width, 4) for [R, G1, G2, B] channels
    pub data: Array3<f64>,
    /// Metadata for this frame
    pub meta: FrameMeta,
}

/// Per-frame metadata extracted from EXIF
#[derive(Debug, Clone)]
pub struct FrameMeta {
    pub path: PathBuf,
    pub focus_position: u16,
    pub focus_step: u8,     // 0-6 for 7 steps
    pub exposure_ev: f64,   // EV relative to reference
    pub shutter_speed: f64, // Actual shutter speed in seconds
    pub iso: u16,
    pub aperture: f64,     // F-number
    pub focal_length: f64, // mm
    pub rotation_deg: f32, // Parsed from filename
    pub vantage: String,   // "low", "mid", "high"
    pub black_level: f64,  // From EXIF, typically 1024 for Z9
    pub cam_mul: [f32; 4], // Camera white balance multipliers [R, G, B, G2]
}

/// Sequence metadata for a complete capture set
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub focus_steps: u8,          // Typically 7
    pub exposures: Vec<f64>,      // EV values, sorted (DEPRECATED - use shutter_speeds instead)
    pub shutter_speeds: Vec<f64>, // Actual exposure times in seconds (for HDR weighting)
    pub ref_focus: u8,            // Reference focus step (middle)
    pub ref_exp: f64,             // Reference exposure EV (0.0)
    pub rot_deg: f32,             // Rotation angle
    pub vantage: String,          // Camera height
    pub burst_factor: u8,         // Burst averaging factor (1 = no averaging)
    pub bone_id: String,          // Identifier from filename
    pub cam_mul: [f32; 4],        // Camera white balance multipliers [R, G, B, G2]
}

/// Complete sequence of frames for processing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sequence {
    /// All frame paths, sorted by focus then exposure
    pub paths: Vec<PathBuf>,
    /// Sequence metadata
    pub meta: Meta,
}

impl Sequence {
    /// Get reference frame index (middle focus, reference exposure)
    pub fn ref_index(&self) -> usize {
        let exp_idx = self
            .meta
            .exposures
            .iter()
            .position(|&e| (e - self.meta.ref_exp).abs() < 0.01)
            .unwrap_or(self.meta.exposures.len() / 2);

        self.meta.ref_focus as usize * self.meta.exposures.len() + exp_idx
    }

    /// Total number of frames
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

/// Global processing options
#[derive(Debug, Clone)]
pub struct ProcessingOptions {
    /// Batch size (None = auto-detect)
    pub batch_size: Option<usize>,
    /// Maximum number of sequences to process in parallel (None = auto-detect)
    pub max_parallel_sequences: Option<usize>,
    /// Keep original NEF files after processing
    pub save_original: bool,
    /// Cloud rsync target (e.g., "user@host:/path")
    pub cloud_target: Option<String>,
    /// Run COLMAP photogrammetry after processing
    pub run_photogrammetry: bool,
    /// Run validation (PSNR/BRISQUE)
    pub validate: bool,
    /// Force full decode (disable selective loading)
    pub full_decode: bool,

    // Algorithm parameters
    pub noise_sigma: f64,     // Noise estimate
    pub lambda_wavelet: f64,  // Wavelet sparsity weight
    pub lambda_curvelet: f64, // Curvelet sparsity weight
    pub num_focus_steps: u8,
    pub num_exposures: usize,
    pub ref_focus_step: u8,
    pub ref_exposure_ev: f64,

    // Auto-refinement parameters
    /// Enable automatic quality assessment and refinement loop
    pub auto_refine: bool,
    /// Maximum number of refinement iterations
    pub max_refine_loops: usize,
    /// Open output in viewer after successful refinement
    pub open_on_pass: bool,

    // Timing and profiling parameters
    /// Enable verbose timing output to console
    pub verbose_timing: bool,
    /// Path to JSON timing log file (append mode)
    pub log_timing: Option<PathBuf>,

    // Hierarchical processing parameters (always enabled)
    /// Grading threshold multiplier
    pub grade_k: f64,

    // Export parameters
    /// Export format: "tiff16" (default), "png", "jpeg"
    pub export_format: String,
}

impl Default for ProcessingOptions {
    fn default() -> Self {
        Self {
            batch_size: None,
            max_parallel_sequences: None,
            save_original: true, // Default to safer option
            cloud_target: None,
            run_photogrammetry: false,
            validate: true,
            full_decode: false,
            noise_sigma: 10.0,
            lambda_wavelet: 0.1,
            lambda_curvelet: 0.1,
            num_focus_steps: 7,
            num_exposures: 3,
            ref_focus_step: 3,
            ref_exposure_ev: 0.0,
            auto_refine: false,
            max_refine_loops: 0,
            open_on_pass: false,
            verbose_timing: true,
            log_timing: None,
            grade_k: 1.5,
            export_format: "tiff16".to_string(),
        }
    }
}

/// Processing result for a sequence
#[derive(Debug)]
pub struct ProcessingResult {
    /// Fused RGB image (u8, tone-mapped)
    pub rgb: Array3<u8>,
    /// Foreground mask (u8, 0 or 255)
    pub mask: ndarray::Array2<u8>,
    /// Processing time in seconds
    pub elapsed_secs: f64,
    /// Quality metrics (if validation enabled)
    pub metrics: Option<QualityMetrics>,
}

/// Quality metrics for validation
#[derive(Debug, Clone, Default)]
pub struct QualityMetrics {
    pub psnr: Option<f64>,    // Peak signal-to-noise ratio
    pub ssim: Option<f64>,    // Structural similarity
    pub brisque: Option<f64>, // Blind image quality score
}
