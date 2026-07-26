//! BRDF Estimation from Focus Stacks (SOTA 5)
//!
//! Estimates surface roughness based on focus falloff curves.
//! Sharper peak = Glossy (low roughness).
//! Broad peak = Matte (high roughness).

use anyhow::Result;
use ndarray::Array2;

/// BRDF surface property maps estimated from focus stack behavior
pub struct BrdfMaps {
    /// Roughness map (0.0 = mirror glossy, 1.0 = diffuse matte)
    pub roughness: Array2<f64>,
    /// Specular intensity map (based on focus peak sharpness)
    pub specular: Array2<f64>,
}

/// Estimate roughness from focus curve width (FWHM - Full Width at Half Maximum)
///
/// For each pixel, we analyze how the focus metric varies across the focus stack.
/// - Sharp peak (narrow FWHM) → Glossy/reflective surface → Low roughness
/// - Broad peak (wide FWHM) → Matte/diffuse surface → High roughness
///
/// # Arguments
/// * `metric_stack` - Stack of focus sharpness maps (one per focus plane)
/// * `depth_map` - Best focus plane index for each pixel (normalized 0-1)
///
/// # Returns
/// * `BrdfMaps` containing roughness and specular maps
pub fn estimate_roughness(
    metric_stack: &[Array2<f64>],
    depth_map: &Array2<f64>,
) -> Result<BrdfMaps> {
    if metric_stack.is_empty() {
        anyhow::bail!("Empty metric stack provided");
    }

    let (h, w) = depth_map.dim();
    let num_planes = metric_stack.len();

    let mut roughness = Array2::zeros((h, w));
    let mut specular = Array2::zeros((h, w));

    // For single-plane stacks, assume neutral roughness
    if num_planes < 3 {
        roughness.fill(0.5);
        specular.fill(0.5);
        return Ok(BrdfMaps {
            roughness,
            specular,
        });
    }

    // Analyze focus curve at each pixel
    for y in 0..h {
        for x in 0..w {
            // Extract focus metric curve for this pixel across all focus planes
            let mut curve: Vec<f64> = Vec::with_capacity(num_planes);
            for plane in metric_stack {
                curve.push(plane[[y, x]]);
            }

            // Find peak value and its index
            let (peak_idx, peak_val) = curve
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
                .unwrap_or((0, &0.0));

            // Skip if no significant focus metric
            if *peak_val < 0.01 {
                roughness[[y, x]] = 0.5;
                specular[[y, x]] = 0.0;
                continue;
            }

            // Calculate FWHM (Full Width at Half Maximum)
            let half_max = peak_val * 0.5;

            // Find left edge (where curve drops to half max)
            let mut left_idx = peak_idx;
            for i in (0..peak_idx).rev() {
                if curve[i] < half_max {
                    left_idx = i;
                    break;
                }
            }

            // Find right edge
            let mut right_idx = peak_idx;
            for i in (peak_idx + 1)..num_planes {
                if curve[i] < half_max {
                    right_idx = i;
                    break;
                }
            }

            // FWHM in focus plane units
            let fwhm = (right_idx - left_idx) as f64;

            // Normalize FWHM to roughness (0-1)
            // Narrow FWHM (1-2) → low roughness (glossy)
            // Wide FWHM (>5) → high roughness (matte)
            let max_fwhm = (num_planes as f64 * 0.5).max(3.0);
            let normalized_fwhm = (fwhm / max_fwhm).clamp(0.0, 1.0);

            roughness[[y, x]] = normalized_fwhm;

            // Specular intensity based on peak sharpness
            // Higher peak with narrower curve = more specular
            let peak_sharpness = if fwhm > 0.0 {
                peak_val / fwhm
            } else {
                *peak_val
            };
            specular[[y, x]] = peak_sharpness.clamp(0.0, 1.0);
        }
    }

    tracing::debug!(
        "BRDF estimation complete: {}x{}, {} focus planes",
        w,
        h,
        num_planes
    );

    Ok(BrdfMaps {
        roughness,
        specular,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_brdf_estimation_basic() {
        // Create simple 3-plane stack with a peak in the middle
        let mut plane0 = Array2::zeros((4, 4));
        let mut plane1 = Array2::zeros((4, 4));
        let mut plane2 = Array2::zeros((4, 4));

        // Create a focus curve that peaks at plane1
        plane0.fill(0.2);
        plane1.fill(0.8); // Peak
        plane2.fill(0.2);

        let metric_stack = vec![plane0, plane1, plane2];
        let depth_map = Array2::zeros((4, 4));

        let result = estimate_roughness(&metric_stack, &depth_map).unwrap();

        // All pixels should have some roughness estimate
        assert!(result.roughness[[0, 0]] >= 0.0);
        assert!(result.roughness[[0, 0]] <= 1.0);
        assert!(result.specular[[0, 0]] >= 0.0);
    }

    #[test]
    fn test_empty_stack() {
        let empty: Vec<Array2<f64>> = vec![];
        let depth_map = Array2::zeros((4, 4));

        let result = estimate_roughness(&empty, &depth_map);
        assert!(result.is_err());
    }
}
