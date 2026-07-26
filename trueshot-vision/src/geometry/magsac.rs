//! MAGSAC++ - State-of-the-Art Robust Estimation
//!
//! Marginalizing Sample Consensus - threshold-free model quality evaluation.
//! Reference: Barath et al., "MAGSAC++: A Fast, Reliable and Accurate Robust Estimator"

use rand::seq::index;
use rand::Rng;

/// MAGSAC++ configuration
#[derive(Debug, Clone)]
pub struct MagsacConfig {
    /// Maximum assumed noise level (sigma)
    pub sigma_max: f64,
    /// Number of sigma values to marginalize over
    pub num_sigmas: usize,
    /// Maximum iterations
    pub max_iterations: usize,
    /// Confidence level for adaptive stopping
    pub confidence: f64,
    /// Use Progressive NAPSAC sampling
    pub use_pnapsac: bool,
    /// IRLS refinement iterations
    pub irls_iterations: usize,
}

impl Default for MagsacConfig {
    fn default() -> Self {
        Self {
            sigma_max: 10.0,
            num_sigmas: 10,
            max_iterations: 1000,
            confidence: 0.99,
            use_pnapsac: true,
            irls_iterations: 5,
        }
    }
}

/// MAGSAC++ result
#[derive(Debug)]
pub struct MagsacResult<M> {
    pub model: M,
    pub inliers: Vec<usize>,
    pub weights: Vec<f64>,
    pub score: f64,
    pub iterations: usize,
}

/// Trait for MAGSAC-compatible models
pub trait MagsacModel: Clone + Sized {
    type Point: Clone;

    /// Minimum samples for estimation
    fn min_samples() -> usize;

    /// Estimate from minimal sample
    fn estimate(points: &[Self::Point]) -> Option<Self>;

    /// Estimate from weighted points (for IRLS)
    fn weighted_estimate(points: &[Self::Point], weights: &[f64]) -> Option<Self>;

    /// Squared error for a point
    fn squared_error(&self, point: &Self::Point) -> f64;
}

/// MAGSAC++ estimator
pub struct Magsac {
    config: MagsacConfig,
}

impl Magsac {
    pub fn new(config: MagsacConfig) -> Self {
        Self { config }
    }

    /// Run MAGSAC++ estimation
    pub fn estimate<M: MagsacModel>(&self, data: &[M::Point]) -> Option<MagsacResult<M>> {
        let n = data.len();
        let min_samples = M::min_samples();

        if n < min_samples {
            return None;
        }

        let mut rng = rand::thread_rng();
        let mut best_model: Option<M> = None;
        let mut best_score = 0.0;
        let mut best_weights = vec![0.0; n];

        let mut iteration_limit = self.config.max_iterations;
        let mut iterations_run = 0;

        while iterations_run < iteration_limit {
            iterations_run += 1;
            // Sample minimal set
            let sample = if self.config.use_pnapsac {
                self.pnapsac_sample(&mut rng, n, min_samples, data)
            } else {
                self.random_sample(&mut rng, n, min_samples)
            };

            // Estimate model
            let sample_points: Vec<M::Point> = sample.iter().map(|&i| data[i].clone()).collect();

            let mut model = match M::estimate(&sample_points) {
                Some(m) => m,
                None => continue,
            };

            // Compute marginalized quality score (threshold-free!)
            let (score, weights) = self.compute_magsac_score(&model, data);

            if score > best_score {
                // IRLS refinement
                model = self.irls_refine(model, data, &weights);
                let (refined_score, refined_weights) = self.compute_magsac_score(&model, data);

                if refined_score > best_score {
                    best_score = refined_score;
                    best_model = Some(model);
                    best_weights = refined_weights;

                    // Update iteration count
                    let inlier_ratio = self.estimate_inlier_ratio(&best_weights);
                    if inlier_ratio > 0.1 {
                        let required = self.adaptive_iterations(inlier_ratio, min_samples);
                        iteration_limit = iteration_limit.min(required.max(iterations_run));
                    }
                }
            }
        }

        best_model.map(|model| {
            let inliers: Vec<usize> = best_weights
                .iter()
                .enumerate()
                .filter(|(_, &w)| w > 0.5)
                .map(|(i, _)| i)
                .collect();

            MagsacResult {
                model,
                inliers,
                weights: best_weights,
                score: best_score,
                iterations: iterations_run,
            }
        })
    }

    /// Compute MAGSAC quality score by marginalizing over sigma values
    fn compute_magsac_score<M: MagsacModel>(
        &self,
        model: &M,
        data: &[M::Point],
    ) -> (f64, Vec<f64>) {
        let n = data.len();
        let mut weights = vec![0.0; n];
        let mut total_score = 0.0;

        // Compute squared residuals
        let residuals: Vec<f64> = data.iter().map(|p| model.squared_error(p)).collect();

        // Marginalize over sigma values
        for sigma_idx in 1..=self.config.num_sigmas {
            let sigma = self.config.sigma_max * sigma_idx as f64 / self.config.num_sigmas as f64;
            let sigma_sq = sigma * sigma;
            let threshold_sq = 3.84 * sigma_sq; // Chi-squared 95% for 1 DOF

            for (i, &r_sq) in residuals.iter().enumerate() {
                if r_sq < threshold_sq {
                    // Gaussian weight
                    let weight = (-r_sq / (2.0 * sigma_sq)).exp();
                    weights[i] += weight;
                    total_score += weight;
                }
            }
        }

        // Normalize weights
        for w in &mut weights {
            *w /= self.config.num_sigmas as f64;
        }

        (total_score, weights)
    }

    /// Iteratively Reweighted Least Squares refinement
    fn irls_refine<M: MagsacModel>(
        &self,
        mut model: M,
        data: &[M::Point],
        initial_weights: &[f64],
    ) -> M {
        let mut weights = initial_weights.to_vec();

        for _ in 0..self.config.irls_iterations {
            // Re-estimate with weights
            if let Some(refined) = M::weighted_estimate(data, &weights) {
                model = refined;
            }

            // Update weights using Cauchy loss
            for (i, point) in data.iter().enumerate() {
                let r_sq = model.squared_error(point);
                // Cauchy weight function: w = 1 / (1 + r^2/c^2)
                let c_sq = self.config.sigma_max * self.config.sigma_max;
                weights[i] = 1.0 / (1.0 + r_sq / c_sq);
            }
        }

        model
    }

    /// Progressive NAPSAC sampling (spatially coherent)
    fn pnapsac_sample<T: Clone>(
        &self,
        rng: &mut impl Rng,
        n: usize,
        k: usize,
        _data: &[T],
    ) -> Vec<usize> {
        // Start with random point
        let first = rng.gen_range(0..n);
        let mut sample = vec![first];

        // For simplicity, use uniform sampling here
        // Full P-NAPSAC would use spatial neighborhood
        while sample.len() < k {
            let idx = rng.gen_range(0..n);
            if !sample.contains(&idx) {
                sample.push(idx);
            }
        }

        sample
    }

    /// Simple random sampling
    fn random_sample(&self, rng: &mut impl Rng, n: usize, k: usize) -> Vec<usize> {
        index::sample(rng, n, k).into_vec()
    }

    /// Estimate inlier ratio from weights
    fn estimate_inlier_ratio(&self, weights: &[f64]) -> f64 {
        let inlier_count = weights.iter().filter(|&&w| w > 0.5).count();
        inlier_count as f64 / weights.len() as f64
    }

    /// Adaptive iteration count
    fn adaptive_iterations(&self, inlier_ratio: f64, min_samples: usize) -> usize {
        let p = inlier_ratio.powi(min_samples as i32);
        if p >= 1.0 {
            return 1;
        }
        if p <= 0.0 {
            return self.config.max_iterations;
        }

        ((1.0 - self.config.confidence).ln() / (1.0 - p).ln()).ceil() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct LineModel {
        slope: f64,
        intercept: f64,
    }

    impl MagsacModel for LineModel {
        type Point = (f64, f64);

        fn min_samples() -> usize {
            2
        }

        fn estimate(points: &[Self::Point]) -> Option<Self> {
            if points.len() < 2 {
                return None;
            }
            let (x1, y1) = points[0];
            let (x2, y2) = points[1];
            if (x2 - x1).abs() < 1e-10 {
                return None;
            }
            let slope = (y2 - y1) / (x2 - x1);
            let intercept = y1 - slope * x1;
            Some(LineModel { slope, intercept })
        }

        fn weighted_estimate(points: &[Self::Point], weights: &[f64]) -> Option<Self> {
            // Weighted least squares
            let mut sum_w = 0.0;
            let mut sum_wx = 0.0;
            let mut sum_wy = 0.0;
            let mut sum_wxx = 0.0;
            let mut sum_wxy = 0.0;

            for (i, &(x, y)) in points.iter().enumerate() {
                let w = weights[i];
                sum_w += w;
                sum_wx += w * x;
                sum_wy += w * y;
                sum_wxx += w * x * x;
                sum_wxy += w * x * y;
            }

            let denom = sum_w * sum_wxx - sum_wx * sum_wx;
            if denom.abs() < 1e-10 {
                return None;
            }

            let slope = (sum_w * sum_wxy - sum_wx * sum_wy) / denom;
            let intercept = (sum_wy - slope * sum_wx) / sum_w;

            Some(LineModel { slope, intercept })
        }

        fn squared_error(&self, point: &Self::Point) -> f64 {
            let (x, y) = *point;
            let predicted = self.slope * x + self.intercept;
            (y - predicted).powi(2)
        }
    }

    #[test]
    fn test_magsac_line() {
        let mut data: Vec<(f64, f64)> = (0..50).map(|i| (i as f64, 2.0 * i as f64 + 1.0)).collect();

        // Add outliers
        data.push((10.0, 500.0));
        data.push((20.0, -300.0));
        data.push((30.0, 800.0));

        let magsac = Magsac::new(MagsacConfig::default());
        let result: MagsacResult<LineModel> = magsac.estimate(&data).expect("Should find model");

        assert!((result.model.slope - 2.0).abs() < 0.1, "Slope should be ~2");
        assert!(
            (result.model.intercept - 1.0).abs() < 0.5,
            "Intercept should be ~1"
        );
        assert!(result.inliers.len() >= 45, "Should identify most inliers");
    }

    #[test]
    fn adaptive_stopping_reports_executed_iterations() {
        let data: Vec<(f64, f64)> = (0..100).map(|i| (i as f64, 3.0 * i as f64 - 2.0)).collect();
        let magsac = Magsac::new(MagsacConfig {
            max_iterations: 500,
            use_pnapsac: false,
            ..Default::default()
        });

        let result: MagsacResult<LineModel> = magsac.estimate(&data).expect("perfect line");
        assert_eq!(result.iterations, 1);
    }
}
