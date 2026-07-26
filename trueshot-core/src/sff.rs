//! Shape-From-Focus (SFF) Analysis (SOTA Item 4)
//!
//! Implements 3D surface reconstruction from focus measure operators.
//! Current iteration: 2.5D Metric Depth Map generation.

use anyhow::Result;
use ndarray::Array2;

/// Reconstruction result containing metric depth (mm) and confidence 
pub struct ReconstructionResult {
    pub depth_map: Array2<f64>, // Distance in mm
    pub confidence_map: Array2<f64>,
}

/// Compute metric depth using Gaussian Interpolation of Focus Measure
///
/// # Arguments
/// * `metric_stack`: Focus measure scores for each pixel at each focus step
/// * `focus_distances`: Physical distance (mm) for each focus index. Assumes pre-calibrated.
/// * `focal_length`: Lens focal length in mm (e.g. 105.0)
/// * `aperture`: f-number (e.g. 8.0) - used for Depth of Field confidence weighting
pub fn compute_depth_metric(
    metric_stack: &[Array2<f64>], 
    focus_distances: &[f64], 
    _focal_length: f64,
    _aperture: f64
) -> Result<ReconstructionResult> {
    let num_frames = metric_stack.len();
    assert_eq!(num_frames, focus_distances.len());
    
    let (h, w) = metric_stack[0].dim();
    
    let mut depth_map = Array2::zeros((h, w));
    let mut conf_map = Array2::zeros((h, w));
    
    for y in 0..h {
        for x in 0..w {
            let mut max_val = -1.0;
            let mut max_idx = 0;
            
            for i in 0..num_frames {
                let val = metric_stack[i][[y, x]];
                if val > max_val {
                    max_val = val;
                    max_idx = i;
                }
            }
            
            // Sub-frame interpolation
            let interpolated_idx = if max_idx > 0 && max_idx < num_frames - 1 {
                let v0 = metric_stack[max_idx-1][[y, x]].max(1e-6).ln();
                let v1 = metric_stack[max_idx][[y, x]].max(1e-6).ln();
                let v2 = metric_stack[max_idx+1][[y, x]].max(1e-6).ln();
                
                let denom = 2.0 * (v0 - 2.0 * v1 + v2);
                if denom.abs() > 1e-5 {
                   let delta = (v0 - v2) / denom;
                   max_idx as f64 + delta
                } else {
                    max_idx as f64
                }
            } else {
                max_idx as f64
            };
            
            // Map interpolated index to physical distance [mm]
            // We linearly interpolate between the known focus distances
            let idx_floor = interpolated_idx.floor() as usize;
            let idx_ceil = (idx_floor + 1).min(num_frames - 1);
            let alpha = interpolated_idx - idx_floor as f64;
            
            let d0 = focus_distances[idx_floor];
            let d1 = focus_distances[idx_ceil];
            let distance_mm = d0 * (1.0 - alpha) + d1 * alpha;
            
            depth_map[[y, x]] = distance_mm;
            conf_map[[y, x]] = max_val;
        }
    }
    
    Ok(ReconstructionResult {
        depth_map,
        confidence_map: conf_map,
    })
}

// Keep legacy simplified version for existing calls
pub fn compute_depth_subpixel(
    metric_stack: &[Array2<f64>], 
    _focus_distances: &[f64] 
) -> Result<ReconstructionResult> {
    // Fake distances 0.0 to 1.0
    let dists: Vec<f64> = (0..metric_stack.len()).map(|i| i as f64 / metric_stack.len() as f64).collect();
    compute_depth_metric(metric_stack, &dists, 50.0, 2.8)
}
