//! Bundle Adjustment - Camera and Point Optimization
//!
//! High-accuracy camera pose and 3D point refinement using
//! Levenberg-Marquardt optimization.
//!
//! This is the gold standard for sub-pixel accurate reconstruction.
//!
//! Reference: "Bundle Adjustment — A Modern Synthesis" (Triggs et al.)

use crate::cv::DistortionModel;
use anyhow::Result;
use nalgebra as na;
use std::collections::HashMap;

/// Bundle adjustment configuration
#[derive(Debug, Clone)]
pub struct BundleAdjustmentConfig {
    /// Maximum number of iterations
    pub max_iterations: usize,
    /// Convergence threshold (change in cost)
    pub tolerance: f64,
    /// Initial damping factor (lambda)
    pub initial_lambda: f64,
    /// Damping increase factor
    pub lambda_up: f64,
    /// Damping decrease factor
    pub lambda_down: f64,
    /// Fix first camera (gauge fixing)
    pub fix_first_camera: bool,
    /// Optimize intrinsics
    pub optimize_intrinsics: bool,
    /// Use robust loss (Huber)
    pub use_robust_loss: bool,
    /// Huber delta
    pub huber_delta: f64,
}

impl Default for BundleAdjustmentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 100,
            tolerance: 1e-10,
            initial_lambda: 1e-3,
            lambda_up: 10.0,
            lambda_down: 0.1,
            fix_first_camera: true,
            optimize_intrinsics: false,
            use_robust_loss: true,
            huber_delta: 1.0,
        }
    }
}

/// Camera parameters for bundle adjustment
#[derive(Debug, Clone)]
pub struct BACamera {
    /// Camera ID
    pub id: u32,
    /// Rotation (axis-angle representation for optimization)
    pub rotation: na::Vector3<f64>,
    /// Translation
    pub translation: na::Vector3<f64>,
    /// Focal length
    pub fx: f64,
    pub fy: f64,
    /// Principal point
    pub cx: f64,
    pub cy: f64,
    /// Distortion model
    pub distortion_model: DistortionModel,
    /// Radial/tangential distortion coefficients
    pub k1: f64,
    pub k2: f64,
    pub p1: f64,
    pub p2: f64,
    pub k3: f64,
    pub k4: f64,
    pub k5: f64,
    pub k6: f64,
}

impl BACamera {
    /// Create from rotation matrix and translation
    pub fn from_rt(
        id: u32,
        r: &na::Matrix3<f64>,
        t: &na::Vector3<f64>,
        k: &na::Matrix3<f64>,
    ) -> Self {
        let rotation = na::Rotation3::from_matrix_unchecked(*r);
        let axis_angle = rotation.scaled_axis();

        Self {
            id,
            rotation: axis_angle,
            translation: *t,
            fx: k[(0, 0)],
            fy: k[(1, 1)],
            cx: k[(0, 2)],
            cy: k[(1, 2)],
            distortion_model: DistortionModel::None,
            k1: 0.0,
            k2: 0.0,
            p1: 0.0,
            p2: 0.0,
            k3: 0.0,
            k4: 0.0,
            k5: 0.0,
            k6: 0.0,
        }
    }

    /// Get rotation matrix
    pub fn rotation_matrix(&self) -> na::Matrix3<f64> {
        na::Rotation3::new(self.rotation).matrix().clone_owned()
    }

    /// Get intrinsics matrix
    pub fn intrinsics(&self) -> na::Matrix3<f64> {
        na::Matrix3::new(self.fx, 0.0, self.cx, 0.0, self.fy, self.cy, 0.0, 0.0, 1.0)
    }

    /// Project 3D point to 2D
    pub fn project(&self, point: &na::Point3<f64>) -> na::Point2<f64> {
        let r = self.rotation_matrix();
        let cam_point = r * point.coords + self.translation;

        if cam_point.z <= 0.0 {
            return na::Point2::new(f64::NAN, f64::NAN);
        }

        let x = cam_point.x / cam_point.z;
        let y = cam_point.y / cam_point.z;

        let (xd, yd) = match self.distortion_model {
            DistortionModel::None => (x, y),
            DistortionModel::BrownConrady => distort_brown_conrady(self, x, y),
            DistortionModel::Fisheye => distort_fisheye(self, x, y),
        };

        na::Point2::new(self.fx * xd + self.cx, self.fy * yd + self.cy)
    }

    /// Number of parameters
    pub fn num_params(&self, optimize_intrinsics: bool) -> usize {
        if optimize_intrinsics {
            10
        } else {
            6
        }
    }
}

fn distort_brown_conrady(camera: &BACamera, x: f64, y: f64) -> (f64, f64) {
    let r2 = x * x + y * y;
    let r4 = r2 * r2;
    let r6 = r4 * r2;

    let radial_num = 1.0 + camera.k1 * r2 + camera.k2 * r4 + camera.k3 * r6;
    let radial_den = 1.0 + camera.k4 * r2 + camera.k5 * r4 + camera.k6 * r6;
    let radial = if radial_den.abs() > 1e-12 {
        radial_num / radial_den
    } else {
        radial_num
    };

    let x_tan = 2.0 * camera.p1 * x * y + camera.p2 * (r2 + 2.0 * x * x);
    let y_tan = camera.p1 * (r2 + 2.0 * y * y) + 2.0 * camera.p2 * x * y;

    (x * radial + x_tan, y * radial + y_tan)
}

fn distort_fisheye(camera: &BACamera, x: f64, y: f64) -> (f64, f64) {
    let r = (x * x + y * y).sqrt();
    if r < 1e-12 {
        return (x, y);
    }
    let theta = r.atan();
    let theta2 = theta * theta;
    let theta4 = theta2 * theta2;
    let theta6 = theta4 * theta2;
    let theta8 = theta4 * theta4;
    let theta_d = theta
        * (1.0 + camera.k1 * theta2 + camera.k2 * theta4 + camera.k3 * theta6 + camera.k4 * theta8);
    let scale = theta_d / r;
    (x * scale, y * scale)
}

/// 3D point for bundle adjustment
#[derive(Debug, Clone)]
pub struct BAPoint3D {
    /// Point ID
    pub id: u32,
    /// Position
    pub position: na::Point3<f64>,
    /// Color (optional)
    pub color: [u8; 3],
}

/// 2D observation (feature detection in image)
#[derive(Debug, Clone)]
pub struct Observation {
    /// Camera ID
    pub camera_id: u32,
    /// Point ID
    pub point_id: u32,
    /// 2D position in image
    pub position: na::Point2<f64>,
    /// Weight (inverse variance)
    pub weight: f64,
}

/// Bundle adjustment problem
pub struct BundleAdjustment {
    config: BundleAdjustmentConfig,
    cameras: Vec<BACamera>,
    points: Vec<BAPoint3D>,
    observations: Vec<Observation>,

    // Index maps
    camera_index: HashMap<u32, usize>,
    point_index: HashMap<u32, usize>,
}

impl BundleAdjustment {
    /// Create new bundle adjustment problem
    pub fn new(config: BundleAdjustmentConfig) -> Self {
        Self {
            config,
            cameras: Vec::new(),
            points: Vec::new(),
            observations: Vec::new(),
            camera_index: HashMap::new(),
            point_index: HashMap::new(),
        }
    }

    /// Add camera
    pub fn add_camera(&mut self, camera: BACamera) {
        let idx = self.cameras.len();
        self.camera_index.insert(camera.id, idx);
        self.cameras.push(camera);
    }

    /// Add 3D point
    pub fn add_point(&mut self, point: BAPoint3D) {
        let idx = self.points.len();
        self.point_index.insert(point.id, idx);
        self.points.push(point);
    }

    /// Add observation
    pub fn add_observation(&mut self, obs: Observation) {
        self.observations.push(obs);
    }

    /// Solve bundle adjustment using Levenberg-Marquardt
    pub fn solve(&mut self) -> Result<BundleAdjustmentResult> {
        let start_time = std::time::Instant::now();

        let mut lambda = self.config.initial_lambda;
        let mut prev_cost = self.compute_cost();

        let mut iterations = 0;
        let mut converged = false;

        #[cfg(feature = "logging")]
        eprintln!(
            "🔧 Bundle Adjustment: {} cameras, {} points, {} observations",
            self.cameras.len(),
            self.points.len(),
            self.observations.len()
        );

        for iter in 0..self.config.max_iterations {
            iterations = iter + 1;

            // Compute Jacobian and residuals
            let (j, residuals) = self.compute_jacobian_and_residuals();

            // Normal equations: (J^T * J + lambda * diag(J^T * J)) * delta = J^T * r
            let jtj = &j.transpose() * &j;
            let jtr = &j.transpose() * &residuals;

            // Add damping
            let mut jtj_damped = jtj.clone();
            for i in 0..jtj_damped.nrows() {
                jtj_damped[(i, i)] += lambda * jtj[(i, i)].max(1e-10);
            }

            // Solve for delta
            let delta = match jtj_damped.clone().lu().solve(&jtr) {
                Some(d) => d,
                None => {
                    lambda *= self.config.lambda_up;
                    continue;
                }
            };

            // Apply update (tentatively)
            self.apply_update(&delta);

            // Compute new cost
            let new_cost = self.compute_cost();

            if new_cost < prev_cost {
                // Accept update
                lambda *= self.config.lambda_down;

                let improvement = prev_cost - new_cost;

                if improvement < self.config.tolerance {
                    converged = true;
                    break;
                }

                prev_cost = new_cost;
            } else {
                // Reject update
                self.revert_update(&delta);
                lambda *= self.config.lambda_up;
            }
        }

        let final_cost = self.compute_cost();
        let elapsed = start_time.elapsed();

        // Compute reprojection errors
        let reprojection_errors = self.compute_reprojection_errors();
        let mean_error = if reprojection_errors.is_empty() {
            0.0
        } else {
            reprojection_errors.iter().sum::<f64>() / reprojection_errors.len() as f64
        };
        let max_error = reprojection_errors.iter().cloned().fold(0.0, f64::max);

        Ok(BundleAdjustmentResult {
            iterations,
            converged,
            initial_cost: prev_cost,
            final_cost,
            mean_reprojection_error: mean_error,
            max_reprojection_error: max_error,
            elapsed_ms: elapsed.as_millis() as u64,
        })
    }

    /// Compute total cost (sum of squared reprojection errors) - PARALLELIZED
    fn compute_cost(&self) -> f64 {
        use rayon::prelude::*;

        let huber_delta = self.config.huber_delta;
        let use_robust = self.config.use_robust_loss;

        self.observations
            .par_iter()
            .filter_map(|obs| {
                let camera_idx = *self.camera_index.get(&obs.camera_id)?;
                let point_idx = *self.point_index.get(&obs.point_id)?;

                let camera = &self.cameras[camera_idx];
                let point = &self.points[point_idx];

                let projected = camera.project(&point.position);
                if projected.x.is_nan() {
                    return None;
                }

                let error = obs.position - projected;
                let error_sq = error.norm_squared();

                // Apply robust loss if configured
                let loss = if use_robust {
                    huber_loss_static(error_sq.sqrt(), huber_delta)
                } else {
                    error_sq
                };

                Some(obs.weight * loss)
            })
            .sum()
    }
}

/// Standalone Huber loss function for parallel processing
fn huber_loss_static(e: f64, delta: f64) -> f64 {
    if e <= delta {
        0.5 * e * e
    } else {
        delta * (e - 0.5 * delta)
    }
}

impl BundleAdjustment {
    /// Compute Jacobian and residuals
    fn compute_jacobian_and_residuals(&self) -> (na::DMatrix<f64>, na::DVector<f64>) {
        let num_camera_params = if self.config.optimize_intrinsics {
            10
        } else {
            6
        };
        let first_camera_start = if self.config.fix_first_camera { 1 } else { 0 };
        let num_camera_variables = (self.cameras.len() - first_camera_start) * num_camera_params;
        let num_point_variables = self.points.len() * 3;
        let num_variables = num_camera_variables + num_point_variables;
        let num_observations = self.observations.len() * 2; // x and y

        let mut j = na::DMatrix::zeros(num_observations, num_variables);
        let mut residuals = na::DVector::zeros(num_observations);

        for (obs_idx, obs) in self.observations.iter().enumerate() {
            let camera_idx = match self.camera_index.get(&obs.camera_id) {
                Some(&idx) => idx,
                None => continue,
            };
            let point_idx = match self.point_index.get(&obs.point_id) {
                Some(&idx) => idx,
                None => continue,
            };

            let camera = &self.cameras[camera_idx];
            let point = &self.points[point_idx];

            let projected = camera.project(&point.position);
            if projected.x.is_nan() {
                continue;
            }

            let error = obs.position - projected;
            let row = obs_idx * 2;
            residuals[row] = error.x * obs.weight.sqrt();
            residuals[row + 1] = error.y * obs.weight.sqrt();

            // Jacobian w.r.t. camera parameters
            if camera_idx >= first_camera_start {
                let cam_col = (camera_idx - first_camera_start) * num_camera_params;
                let j_cam = self.compute_camera_jacobian(camera, point);
                for i in 0..2 {
                    for k in 0..num_camera_params.min(6) {
                        j[(row + i, cam_col + k)] = j_cam[(i, k)] * obs.weight.sqrt();
                    }
                }
            }

            // Jacobian w.r.t. point parameters
            let point_col = num_camera_variables + point_idx * 3;
            let j_point = self.compute_point_jacobian(camera, point);
            for i in 0..2 {
                for k in 0..3 {
                    j[(row + i, point_col + k)] = j_point[(i, k)] * obs.weight.sqrt();
                }
            }
        }

        (j, residuals)
    }

    /// Compute Jacobian of projection w.r.t. camera parameters
    fn compute_camera_jacobian(&self, camera: &BACamera, point: &BAPoint3D) -> na::Matrix2x6<f64> {
        let epsilon = 1e-8;
        let mut j = na::Matrix2x6::zeros();

        let p0 = camera.project(&point.position);

        // Numerical differentiation for camera parameters
        for i in 0..6 {
            let mut cam_perturbed = camera.clone();
            match i {
                0 => cam_perturbed.rotation.x += epsilon,
                1 => cam_perturbed.rotation.y += epsilon,
                2 => cam_perturbed.rotation.z += epsilon,
                3 => cam_perturbed.translation.x += epsilon,
                4 => cam_perturbed.translation.y += epsilon,
                5 => cam_perturbed.translation.z += epsilon,
                _ => {}
            }
            let p1 = cam_perturbed.project(&point.position);
            j[(0, i)] = (p1.x - p0.x) / epsilon;
            j[(1, i)] = (p1.y - p0.y) / epsilon;
        }

        j
    }

    /// Compute Jacobian of projection w.r.t. point parameters
    fn compute_point_jacobian(&self, camera: &BACamera, point: &BAPoint3D) -> na::Matrix2x3<f64> {
        let epsilon = 1e-8;
        let mut j = na::Matrix2x3::zeros();

        let p0 = camera.project(&point.position);

        for i in 0..3 {
            let mut pos_perturbed = point.position;
            match i {
                0 => pos_perturbed.x += epsilon,
                1 => pos_perturbed.y += epsilon,
                2 => pos_perturbed.z += epsilon,
                _ => {}
            }
            let p1 = camera.project(&pos_perturbed);
            j[(0, i)] = (p1.x - p0.x) / epsilon;
            j[(1, i)] = (p1.y - p0.y) / epsilon;
        }

        j
    }

    /// Apply parameter update
    fn apply_update(&mut self, delta: &na::DVector<f64>) {
        let num_camera_params = if self.config.optimize_intrinsics {
            10
        } else {
            6
        };
        let first_camera_start = if self.config.fix_first_camera { 1 } else { 0 };
        let num_camera_variables = (self.cameras.len() - first_camera_start) * num_camera_params;

        // Update cameras
        for (i, camera) in self.cameras.iter_mut().enumerate().skip(first_camera_start) {
            let col = (i - first_camera_start) * num_camera_params;
            camera.rotation.x -= delta[col];
            camera.rotation.y -= delta[col + 1];
            camera.rotation.z -= delta[col + 2];
            camera.translation.x -= delta[col + 3];
            camera.translation.y -= delta[col + 4];
            camera.translation.z -= delta[col + 5];
        }

        // Update points
        for (i, point) in self.points.iter_mut().enumerate() {
            let col = num_camera_variables + i * 3;
            point.position.x -= delta[col];
            point.position.y -= delta[col + 1];
            point.position.z -= delta[col + 2];
        }
    }

    /// Revert parameter update
    fn revert_update(&mut self, delta: &na::DVector<f64>) {
        // Just negate and apply
        let neg_delta = -delta.clone();
        self.apply_update(&neg_delta);
        // Then negate again to get back
        self.apply_update(&neg_delta);
    }

    /// Compute reprojection errors for all observations
    fn compute_reprojection_errors(&self) -> Vec<f64> {
        let mut errors = Vec::new();

        for obs in &self.observations {
            let camera_idx = match self.camera_index.get(&obs.camera_id) {
                Some(&idx) => idx,
                None => continue,
            };
            let point_idx = match self.point_index.get(&obs.point_id) {
                Some(&idx) => idx,
                None => continue,
            };

            let camera = &self.cameras[camera_idx];
            let point = &self.points[point_idx];

            let projected = camera.project(&point.position);
            if projected.x.is_nan() {
                continue;
            }

            let error = (obs.position - projected).norm();
            errors.push(error);
        }

        errors
    }

    /// Get optimized cameras
    pub fn cameras(&self) -> &[BACamera] {
        &self.cameras
    }

    /// Get optimized points
    pub fn points(&self) -> &[BAPoint3D] {
        &self.points
    }
}

/// Bundle adjustment result
#[derive(Debug, Clone)]
pub struct BundleAdjustmentResult {
    /// Number of iterations
    pub iterations: usize,
    /// Whether optimization converged
    pub converged: bool,
    /// Initial cost
    pub initial_cost: f64,
    /// Final cost
    pub final_cost: f64,
    /// Mean reprojection error in pixels
    pub mean_reprojection_error: f64,
    /// Maximum reprojection error in pixels
    pub max_reprojection_error: f64,
    /// Elapsed time in milliseconds
    pub elapsed_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_camera_projection() {
        let camera = BACamera {
            id: 0,
            rotation: na::Vector3::zeros(),
            translation: na::Vector3::new(0.0, 0.0, 5.0),
            fx: 500.0,
            fy: 500.0,
            cx: 320.0,
            cy: 240.0,
            distortion_model: DistortionModel::None,
            k1: 0.0,
            k2: 0.0,
            p1: 0.0,
            p2: 0.0,
            k3: 0.0,
            k4: 0.0,
            k5: 0.0,
            k6: 0.0,
        };

        let point = na::Point3::new(0.0, 0.0, 0.0);
        let projected = camera.project(&point);

        // Point at origin, camera 5 units back, should project to principal point
        assert!((projected.x - 320.0).abs() < 1e-6);
        assert!((projected.y - 240.0).abs() < 1e-6);
    }

    #[test]
    fn test_bundle_adjustment_basic() {
        let mut ba = BundleAdjustment::new(BundleAdjustmentConfig {
            max_iterations: 10,
            ..Default::default()
        });

        // Add a camera
        ba.add_camera(BACamera {
            id: 0,
            rotation: na::Vector3::zeros(),
            translation: na::Vector3::new(0.0, 0.0, 5.0),
            fx: 500.0,
            fy: 500.0,
            cx: 320.0,
            cy: 240.0,
            distortion_model: DistortionModel::None,
            k1: 0.0,
            k2: 0.0,
            p1: 0.0,
            p2: 0.0,
            k3: 0.0,
            k4: 0.0,
            k5: 0.0,
            k6: 0.0,
        });

        // Add a point
        ba.add_point(BAPoint3D {
            id: 0,
            position: na::Point3::new(0.0, 0.0, 0.0),
            color: [128, 128, 128],
        });

        // Add observation
        ba.add_observation(Observation {
            camera_id: 0,
            point_id: 0,
            position: na::Point2::new(320.0, 240.0),
            weight: 1.0,
        });

        let result = ba.solve().unwrap();
        assert!(result.mean_reprojection_error < 1.0);
    }
}
