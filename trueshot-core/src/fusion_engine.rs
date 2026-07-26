//! FusionEngine: Core image processing pipeline

use crate::types::{Meta, BayerFrame, ProcessingOptions};
use crate::preprocessing::preprocess_stack;
use crate::hierarchical_pipeline::hierarchical_process;
use crate::postprocess::postprocess;
use crate::timing::HierarchicalTimer;
use crate::timed_scope;
use anyhow::Result;
use ndarray::{Array2, Array3};
use crate::hierarchical_grading::GradingParams;
use crate::hierarchical_collapse::{HierarchicalParams, SRFactor, CollapseResult};
use crate::types::AlignmentInfo;
use crate::demosaic_ahd::ahd_demosaic;

pub struct FusionResult {
    /// Display-ready RGB image (sRGB, gamma corrected, 8-bit)
    pub rgb_u8: Array3<u8>,
    /// Linear RGB image (ProPhoto/Linear, 16-bit precision, for archiving)
    pub rgb_f64: Array3<f64>,
    /// Foreground mask
    pub mask: Array2<u8>,
    /// Depth Map (normalized 0.0-1.0)
    pub depth_map: Array2<f64>,
}

pub struct FusionEngine {
    options: ProcessingOptions,
}

impl FusionEngine {
    pub fn new(options: ProcessingOptions) -> Self {
        Self { options }
    }

    /// Process a stack of Bayer frames into a final RGB image
    pub fn process(
        &self, 
        frames: Vec<BayerFrame>, 
        meta: &Meta, 
        _timer: &mut HierarchicalTimer
    ) -> Result<FusionResult> {
        // 1. Preprocess
        let skip_align = false;
        let stack = timed_scope!(timer, "preprocess", {
            preprocess_stack(frames, meta, self.options.noise_sigma, skip_align)?
        });

        // 2. Hierarchical Collapse (Returns RGB + Depth)
        let (collapsed_rgb, depth_map) = timed_scope!(timer, "hierarchical_collapse", {
            self.run_hierarchical_collapse(&stack, meta)?
        });

        // 3. Post-process (f64 linear -> u8 sRGB)
        let rgb_u8 = timed_scope!(timer, "postprocess", {
            postprocess(&collapsed_rgb)?
        });

        // 4. Prepare Mask
        let mask_u8 = stack.fg_mask.mapv(|b| if b { 255u8 } else { 0u8 });

        Ok(FusionResult {
            rgb_u8,
            rgb_f64: collapsed_rgb,
            mask: mask_u8,
            depth_map,
        })
    }

    fn run_hierarchical_collapse(
        &self,
        stack: &crate::preprocessing::PreprocessedStack,
        meta: &Meta,
    ) -> Result<(Array3<f64>, Array2<f64>)> { // Return Depth too
        let num_frames = stack.frames.len();
        
        let alignments: Vec<AlignmentInfo> = stack.alignments.iter()
            .map(|(dx, dy, scale)| AlignmentInfo { dx: *dx, dy: *dy, scale: *scale })
            .collect();

        let exposures: Vec<f64> = (0..num_frames)
            .map(|i| {
                let exp_idx = i % meta.shutter_speeds.len();
                meta.shutter_speeds[exp_idx]
            })
            .collect();

        let grading_params = GradingParams {
            k_threshold: self.options.grade_k,
            percentile_thresholds: [75.0, 50.0, 25.0],
            pyramid_levels: 2,
        };

        let collapse_params = HierarchicalParams {
            sr_factor: SRFactor::None,
            lambda_b: 0.1,
            lambda_a: 0.05,
            exposure_sigma: 0.2,
            denoise_strength: self.options.noise_sigma,
        };

        let reference_idx = num_frames / 2;
        let wb_multipliers = stack.frame_metadata[0].cam_mul;

        let collapse_result = hierarchical_process(
            &stack.frames,
            &stack.fg_mask,
            reference_idx,
            &exposures,
            Some(&alignments),
            &grading_params,
            &collapse_params,
            meta.focus_steps as usize,
            meta.exposures.len(),
            &wb_multipliers,
        )?;
        
        let depth_map = self.estimate_depth_map_from_focus(stack, meta);

        let rgb = match collapse_result {
            CollapseResult::Rgb(rgb_data) => {
                let (_channels, h, w) = rgb_data.dim();
                let mut rgb = Array3::<f64>::zeros((h, w, 3));
                for y in 0..h {
                    for x in 0..w {
                        rgb[[y, x, 0]] = rgb_data[[0, y, x]];
                        rgb[[y, x, 1]] = rgb_data[[1, y, x]];
                        rgb[[y, x, 2]] = rgb_data[[2, y, x]];
                    }
                }
                rgb
            }
            CollapseResult::Bayer(collapsed_bayer) => {
                let (h, w) = collapsed_bayer.dim();
                let mut bayer_collapsed = Array3::<f64>::zeros((h, w, 1));
                for y in 0..h {
                    for x in 0..w {
                        bayer_collapsed[[y, x, 0]] = collapsed_bayer[[y, x]];
                    }
                }
                let rgb_cam: [[f32; 4]; 3] = [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                ];
                ahd_demosaic(&bayer_collapsed, &rgb_cam)?
            }
        };

        Ok((rgb, depth_map))
    }

    fn estimate_depth_map_from_focus(
        &self,
        stack: &crate::preprocessing::PreprocessedStack,
        meta: &Meta,
    ) -> Array2<f64> {
        let num_focus = meta.focus_steps as usize;
        let num_exposures = meta.exposures.len().max(1);
        if stack.frames.is_empty() {
            return Array2::<f64>::zeros((0, 0));
        }
        if num_focus < 2 {
            let (h, w, _) = stack.frames[0].dim();
            return Array2::<f64>::zeros((h, w));
        }

        let ref_exp = num_exposures / 2;
        let (h, w, _) = stack.frames[0].dim();
        let mut best_focus = Array2::<usize>::zeros((h, w));
        let mut best_sharp = Array2::<f64>::zeros((h, w));

        for focus_idx in 0..num_focus {
            let frame_idx = focus_idx * num_exposures + ref_exp;
            if frame_idx >= stack.frames.len() {
                break;
            }
            let frame = &stack.frames[frame_idx];
            if h < 3 || w < 3 {
                continue;
            }
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let center = frame[[y, x, 0]];
                    let lap = frame[[y - 1, x, 0]]
                        + frame[[y + 1, x, 0]]
                        + frame[[y, x - 1, 0]]
                        + frame[[y, x + 1, 0]]
                        - 4.0 * center;
                    let sharp = lap.abs();
                    if sharp > best_sharp[[y, x]] {
                        best_sharp[[y, x]] = sharp;
                        best_focus[[y, x]] = focus_idx;
                    }
                }
            }
        }

        let denom = (num_focus - 1).max(1) as f64;
        let mut depth_map = Array2::<f64>::zeros((h, w));
        for y in 0..h {
            for x in 0..w {
                depth_map[[y, x]] = best_focus[[y, x]] as f64 / denom;
            }
        }

        // Light smoothing to reduce speckle noise
        if h > 2 && w > 2 {
            let mut smoothed = depth_map.clone();
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let mut sum = 0.0;
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let yy = (y as isize + dy) as usize;
                            let xx = (x as isize + dx) as usize;
                            sum += depth_map[[yy, xx]];
                        }
                    }
                    smoothed[[y, x]] = sum / 9.0;
                }
            }
            depth_map = smoothed;
        }

        depth_map
    }
}
