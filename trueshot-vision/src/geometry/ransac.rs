//! RANSAC (Random Sample Consensus) Framework
//!
//! Generic RANSAC implementation for robust estimation.

use rand::seq::index;

/// RANSAC configuration
#[derive(Debug, Clone)]
pub struct RansacConfig {
    /// Maximum iterations
    pub max_iterations: usize,
    /// Inlier threshold (pixels for geometric models)
    pub threshold: f64,
    /// Confidence level (typically 0.99)
    pub confidence: f64,
    /// Minimum inlier ratio to succeed
    pub min_inlier_ratio: f64,
}

impl Default for RansacConfig {
    fn default() -> Self {
        Self {
            max_iterations: 1000,
            threshold: 3.0,
            confidence: 0.99,
            min_inlier_ratio: 0.3,
        }
    }
}

/// Result from RANSAC estimation
#[derive(Debug)]
pub struct RansacResult<M> {
    /// Best model found
    pub model: M,
    /// Indices of inliers
    pub inliers: Vec<usize>,
    /// Number of iterations run
    pub iterations: usize,
}

/// Trait for models that can be estimated with RANSAC
pub trait RansacModel: Clone {
    type Point: Clone;

    /// Minimum number of points needed to estimate model
    fn min_samples() -> usize;

    /// Estimate model from minimal sample
    fn estimate(points: &[Self::Point]) -> Option<Self>;

    /// Calculate error for a single point
    fn error(&self, point: &Self::Point) -> f64;
}

/// Generic RANSAC estimator
pub struct Ransac {
    config: RansacConfig,
}

impl Ransac {
    pub fn new(config: RansacConfig) -> Self {
        Self { config }
    }

    /// Run RANSAC on data points
    pub fn estimate<M: RansacModel>(&self, data: &[M::Point]) -> Option<RansacResult<M>> {
        let n = data.len();
        let min_samples = M::min_samples();

        if n < min_samples {
            return None;
        }

        let mut rng = rand::thread_rng();
        let mut best_model: Option<M> = None;
        let mut best_inliers: Vec<usize> = Vec::new();
        let mut best_inlier_count = 0;

        // Adaptive iteration count
        let mut iteration_limit = self.config.max_iterations;
        let mut iterations_run = 0;

        while iterations_run < iteration_limit {
            iterations_run += 1;
            let sample = index::sample(&mut rng, n, min_samples).into_vec();

            // Estimate model from sample
            let sample_points: Vec<M::Point> = sample.iter().map(|&i| data[i].clone()).collect();

            let model = match M::estimate(&sample_points) {
                Some(m) => m,
                None => continue,
            };

            // Count inliers
            let mut inliers = Vec::new();
            for (i, point) in data.iter().enumerate() {
                if model.error(point) < self.config.threshold {
                    inliers.push(i);
                }
            }

            // Update best model
            if inliers.len() > best_inlier_count {
                best_inlier_count = inliers.len();
                best_model = Some(model);
                best_inliers = inliers;

                // Update iteration count based on inlier ratio
                let inlier_ratio = best_inlier_count as f64 / n as f64;
                if inlier_ratio > self.config.min_inlier_ratio {
                    let required = self.compute_iterations(inlier_ratio, min_samples);
                    iteration_limit = iteration_limit.min(required.max(iterations_run));
                }
            }
        }

        // Check if we have enough inliers
        let inlier_ratio = best_inlier_count as f64 / n as f64;
        if inlier_ratio < self.config.min_inlier_ratio {
            return None;
        }

        best_model.map(|model| RansacResult {
            model,
            inliers: best_inliers,
            iterations: iterations_run,
        })
    }

    /// Compute required iterations for desired confidence
    fn compute_iterations(&self, inlier_ratio: f64, min_samples: usize) -> usize {
        let p = inlier_ratio.powi(min_samples as i32);
        if p >= 1.0 {
            return 1;
        }
        if p <= 0.0 {
            return self.config.max_iterations;
        }

        let n = (1.0 - self.config.confidence).ln() / (1.0 - p).ln();
        (n.ceil() as usize).min(self.config.max_iterations)
    }
}

/// Essential matrix model for RANSAC
#[derive(Clone)]
pub struct EssentialMatrixModel {
    pub matrix: nalgebra::Matrix3<f64>,
}

/// Point pair for essential matrix estimation
#[derive(Clone)]
pub struct PointPair {
    pub p1: (f64, f64),
    pub p2: (f64, f64),
    pub k1: nalgebra::Matrix3<f64>,
    pub k2: nalgebra::Matrix3<f64>,
}

impl RansacModel for EssentialMatrixModel {
    type Point = PointPair;

    fn min_samples() -> usize {
        8 // 8-point algorithm
    }

    fn estimate(points: &[Self::Point]) -> Option<Self> {
        if points.len() < 8 {
            return None;
        }

        let pts1: Vec<(f64, f64)> = points.iter().map(|p| p.p1).collect();
        let pts2: Vec<(f64, f64)> = points.iter().map(|p| p.p2).collect();

        // Use fundamental matrix estimation and convert to essential
        let f = super::estimate_fundamental_8point(&pts1, &pts2)?;

        // Assuming same camera for now
        let k = &points[0].k1;
        let e = super::fundamental_to_essential(&f, k);

        Some(EssentialMatrixModel { matrix: e })
    }

    fn error(&self, point: &Self::Point) -> f64 {
        // Sampson error for essential matrix
        let k_inv = point
            .k1
            .try_inverse()
            .unwrap_or(nalgebra::Matrix3::identity());

        let x1 = k_inv * nalgebra::Vector3::new(point.p1.0, point.p1.1, 1.0);
        let x2 = k_inv * nalgebra::Vector3::new(point.p2.0, point.p2.1, 1.0);

        // Epipolar constraint: x2^T * E * x1 = 0
        let ex1 = self.matrix * x1;
        let etx2 = self.matrix.transpose() * x2;

        let x2tex1 = x2.dot(&(self.matrix * x1));

        // Sampson error
        let denom = ex1.x.powi(2) + ex1.y.powi(2) + etx2.x.powi(2) + etx2.y.powi(2);
        if denom < 1e-10 {
            return f64::MAX;
        }

        (x2tex1.powi(2) / denom).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct LinearModel {
        slope: f64,
        intercept: f64,
    }

    impl RansacModel for LinearModel {
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

            Some(LinearModel { slope, intercept })
        }

        fn error(&self, point: &Self::Point) -> f64 {
            let (x, y) = point;
            let predicted = self.slope * x + self.intercept;
            (y - predicted).abs()
        }
    }

    #[test]
    fn test_ransac_line() {
        // Create points on a line with some outliers
        let mut data: Vec<(f64, f64)> = (0..20).map(|i| (i as f64, 2.0 * i as f64 + 1.0)).collect();

        // Add outliers
        data.push((5.0, 100.0));
        data.push((10.0, -50.0));

        let ransac = Ransac::new(RansacConfig {
            threshold: 0.5,
            ..Default::default()
        });

        let result: RansacResult<LinearModel> = ransac.estimate(&data).expect("Should find model");

        // Should find approximately y = 2x + 1
        assert!((result.model.slope - 2.0).abs() < 0.1);
        assert!((result.model.intercept - 1.0).abs() < 0.5);
        assert!(result.inliers.len() >= 19); // Most inliers
    }

    #[test]
    fn adaptive_stopping_reports_executed_iterations() {
        let data: Vec<(f64, f64)> = (0..100).map(|i| (i as f64, 3.0 * i as f64 - 2.0)).collect();
        let ransac = Ransac::new(RansacConfig {
            max_iterations: 500,
            threshold: 1e-9,
            min_inlier_ratio: 0.5,
            ..Default::default()
        });

        let result: RansacResult<LinearModel> = ransac.estimate(&data).expect("perfect line");
        assert_eq!(result.iterations, 1);
    }
}
