//! Motion Scoring System
//!
//! Computes motion scores for objects to determine optimal representation:
//! - Static (score < 0.1): Queue for meshification
//! - Slow (0.1 ≤ score < 0.5): 4DGS with reduced updates
//! - Dynamic (score ≥ 0.5): Full 4DGS streaming
//!
//! Uses the unified tracking module for motion classification.

use crate::gaussian_splatting::gaussian_4d::Gaussian4D;
use nalgebra as na;

// Re-export unified MotionClass from tracking module
pub use crate::tracking::MotionClass;

/// Type alias for backwards compatibility with existing code
pub type MotionClassification = MotionClass;

/// Configuration for motion scoring
#[derive(Clone, Debug)]
pub struct MotionScorerConfig {
    /// Weight for position change
    pub alpha: f32,
    /// Weight for appearance change
    pub beta: f32,
    /// Weight for shape/deformation change
    pub gamma: f32,
    /// Maximum expected position delta (for normalization)
    pub max_position_delta: f32,
    /// Maximum expected appearance delta
    pub max_appearance_delta: f32,
    /// Maximum expected shape delta
    pub max_shape_delta: f32,
}

impl Default for MotionScorerConfig {
    fn default() -> Self {
        Self {
            alpha: 0.5,                // Position is most important
            beta: 0.3,                 // Appearance changes
            gamma: 0.2,                // Shape deformation
            max_position_delta: 1.0,   // 1 unit per frame
            max_appearance_delta: 0.5, // 50% color change
            max_shape_delta: 0.3,      // 30% covariance change
        }
    }
}

/// Motion scorer for objects
pub struct MotionScorer {
    config: MotionScorerConfig,
}

impl MotionScorer {
    pub fn new(config: MotionScorerConfig) -> Self {
        Self { config }
    }

    /// Compute motion score for a set of Gaussians between two frames
    pub fn compute_score(
        &self,
        prev_gaussians: &[Gaussian4D],
        curr_gaussians: &[Gaussian4D],
    ) -> f32 {
        if prev_gaussians.is_empty() || curr_gaussians.is_empty() {
            return 0.5; // Neutral score
        }

        let position_delta = self.compute_position_delta(prev_gaussians, curr_gaussians);
        let appearance_delta = self.compute_appearance_delta(prev_gaussians, curr_gaussians);
        let shape_delta = self.compute_shape_delta(prev_gaussians, curr_gaussians);

        let normalized_position = (position_delta / self.config.max_position_delta).clamp(0.0, 1.0);
        let normalized_appearance =
            (appearance_delta / self.config.max_appearance_delta).clamp(0.0, 1.0);
        let normalized_shape = (shape_delta / self.config.max_shape_delta).clamp(0.0, 1.0);

        let score = self.config.alpha * normalized_position
            + self.config.beta * normalized_appearance
            + self.config.gamma * normalized_shape;

        score.clamp(0.0, 1.0)
    }

    /// Classify based on score
    pub fn classify(&self, score: f32) -> MotionClassification {
        MotionClassification::from_score(score)
    }

    /// Compute average position delta (centroid movement)
    fn compute_position_delta(
        &self,
        prev_gaussians: &[Gaussian4D],
        curr_gaussians: &[Gaussian4D],
    ) -> f32 {
        let prev_centroid = Self::compute_centroid(prev_gaussians);
        let curr_centroid = Self::compute_centroid(curr_gaussians);

        na::distance(&prev_centroid, &curr_centroid)
    }

    /// Compute centroid of Gaussians
    fn compute_centroid(gaussians: &[Gaussian4D]) -> na::Point3<f32> {
        if gaussians.is_empty() {
            return na::Point3::origin();
        }

        let sum: na::Vector3<f32> = gaussians
            .iter()
            .map(|g| na::Vector3::new(g.center.x, g.center.y, g.center.z))
            .sum();

        na::Point3::from(sum / gaussians.len() as f32)
    }

    /// Compute average appearance (color) change
    fn compute_appearance_delta(
        &self,
        prev_gaussians: &[Gaussian4D],
        curr_gaussians: &[Gaussian4D],
    ) -> f32 {
        let n = prev_gaussians.len().min(curr_gaussians.len());
        if n == 0 {
            return 0.0;
        }

        let mut total_delta = 0.0;
        for (prev, curr) in prev_gaussians.iter().zip(curr_gaussians.iter()) {
            let color_delta = (0..3)
                .map(|i| (prev.color[i] - curr.color[i]).abs())
                .sum::<f32>()
                / 3.0;
            total_delta += color_delta;
        }

        total_delta / n as f32
    }

    /// Compute average shape (covariance) change
    fn compute_shape_delta(
        &self,
        prev_gaussians: &[Gaussian4D],
        curr_gaussians: &[Gaussian4D],
    ) -> f32 {
        let n = prev_gaussians.len().min(curr_gaussians.len());
        if n == 0 {
            return 0.0;
        }

        let mut total_delta = 0.0;
        for (prev, curr) in prev_gaussians.iter().zip(curr_gaussians.iter()) {
            // Compare covariance matrices (use Frobenius norm of difference)
            let prev_cov = prev.covariance.to_matrix();
            let curr_cov = curr.covariance.to_matrix();
            let diff = prev_cov - curr_cov;
            let frobenius = diff.iter().map(|x| x * x).sum::<f32>().sqrt();
            total_delta += frobenius;
        }

        total_delta / n as f32
    }
}

impl Default for MotionScorer {
    fn default() -> Self {
        Self::new(MotionScorerConfig::default())
    }
}

/// Score multiple objects in a scene
pub struct SceneMotionAnalyzer {
    scorer: MotionScorer,
}

impl SceneMotionAnalyzer {
    pub fn new() -> Self {
        Self {
            scorer: MotionScorer::default(),
        }
    }

    pub fn with_config(config: MotionScorerConfig) -> Self {
        Self {
            scorer: MotionScorer::new(config),
        }
    }

    /// Analyze all objects and return scores
    pub fn analyze_objects(
        &self,
        objects: &[(Vec<Gaussian4D>, Vec<Gaussian4D>)],
    ) -> Vec<(f32, MotionClassification)> {
        objects
            .iter()
            .map(|(prev, curr)| {
                let score = self.scorer.compute_score(prev, curr);
                let classification = self.scorer.classify(score);
                (score, classification)
            })
            .collect()
    }
}

impl Default for SceneMotionAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classification_thresholds() {
        assert_eq!(
            MotionClassification::from_score(0.0),
            MotionClassification::Static
        );
        assert_eq!(
            MotionClassification::from_score(0.05),
            MotionClassification::Static
        );
        assert_eq!(
            MotionClassification::from_score(0.1),
            MotionClassification::Slow
        );
        assert_eq!(
            MotionClassification::from_score(0.3),
            MotionClassification::Slow
        );
        assert_eq!(
            MotionClassification::from_score(0.5),
            MotionClassification::Dynamic
        );
        assert_eq!(
            MotionClassification::from_score(1.0),
            MotionClassification::Dynamic
        );
    }

    #[test]
    fn test_empty_gaussians() {
        let scorer = MotionScorer::default();
        let score = scorer.compute_score(&[], &[]);
        assert_eq!(score, 0.5); // Neutral
    }
}
