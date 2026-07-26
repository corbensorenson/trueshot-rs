//! Levenberg-Marquardt Bundle Adjustment
//!
//! Production-quality bundle adjustment using the Levenberg-Marquardt algorithm.
//! Optimizes camera poses and 3D points simultaneously.

use crate::distortion::distort_normalized;
use crate::{CameraIntrinsics, CameraMotion, CameraPose, ImageData, Point3D};
use nalgebra as na;
use rayon::prelude::*;

/// Bundle adjustment configuration
#[derive(Clone, Debug)]
pub struct BundleAdjustmentConfig {
    /// Maximum iterations
    pub max_iterations: usize,
    /// Initial lambda (damping factor)
    pub initial_lambda: f64,
    /// Lambda increase factor on rejection
    pub lambda_increase: f64,
    /// Lambda decrease factor on acceptance
    pub lambda_decrease: f64,
    /// Gradient tolerance for convergence
    pub gradient_tolerance: f64,
    /// Parameter change tolerance for convergence
    pub parameter_tolerance: f64,
    /// Function tolerance for convergence
    pub function_tolerance: f64,
    /// Use Huber loss for robustness
    pub use_huber_loss: bool,
    /// Huber loss delta
    pub huber_delta: f64,
    /// Use pose priors when provided
    pub use_pose_priors: bool,
    /// Pose prior rotation sigma (radians)
    pub pose_prior_rotation_sigma: f64,
    /// Pose prior translation sigma (world units)
    pub pose_prior_translation_sigma: f64,
}

impl Default for BundleAdjustmentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 100,
            initial_lambda: 1e-3,
            lambda_increase: 10.0,
            lambda_decrease: 0.1,
            gradient_tolerance: 1e-10,
            parameter_tolerance: 1e-8,
            function_tolerance: 1e-6,
            use_huber_loss: true,
            huber_delta: 1.0,
            use_pose_priors: true,
            pose_prior_rotation_sigma: 0.05,
            pose_prior_translation_sigma: 0.02,
        }
    }
}

/// Bundle adjustment result
#[derive(Clone, Debug)]
pub struct BundleAdjustmentResult {
    pub initial_cost: f64,
    pub final_cost: f64,
    pub iterations: usize,
    pub converged: bool,
    pub rmse: f64,
}

/// Observation: 2D keypoint linked to 3D point
#[derive(Clone, Debug)]
pub struct Observation {
    pub point_idx: usize,
    pub camera_idx: usize,
    pub x: f64,
    pub y: f64,
    /// Time offset (seconds) for rolling shutter compensation
    pub time_offset: f64,
}

/// Pose prior for a camera (camera-to-world) with uncertainty.
#[derive(Clone, Debug)]
pub struct PosePrior {
    pub pose: CameraPose,
    pub rotation_sigma: f64,
    pub translation_sigma: f64,
}

impl PosePrior {
    fn rotation_weight(&self) -> f64 {
        if self.rotation_sigma <= 0.0 {
            1.0
        } else {
            1.0 / self.rotation_sigma
        }
    }
    fn translation_weight(&self) -> f64 {
        if self.translation_sigma <= 0.0 {
            1.0
        } else {
            1.0 / self.translation_sigma
        }
    }
}

/// Full bundle adjustment with Levenberg-Marquardt
pub fn bundle_adjust_lm(
    points: &mut Vec<Point3D>,
    poses: &mut Vec<CameraPose>,
    intrinsics: &[CameraIntrinsics],
    observations: &[Observation],
    pose_priors: &[Option<PosePrior>],
    camera_motions: &[Option<CameraMotion>],
    config: &BundleAdjustmentConfig,
) -> BundleAdjustmentResult {
    let num_points = points.len();
    let num_cameras = poses.len();
    let has_priors = config.use_pose_priors && pose_priors.iter().any(|p| p.is_some());

    if num_points == 0 || num_cameras == 0 || (observations.is_empty() && !has_priors) {
        return BundleAdjustmentResult {
            initial_cost: 0.0,
            final_cost: 0.0,
            iterations: 0,
            converged: true,
            rmse: 0.0,
        };
    }

    // Parameter vector: [camera_params..., point_params...]
    // Camera: 6 params (3 rotation axis-angle, 3 translation)
    // Point: 3 params (x, y, z)
    let num_camera_params = num_cameras * 6;
    let num_point_params = num_points * 3;
    let total_params = num_camera_params + num_point_params;

    // Pack parameters
    let mut params = pack_parameters(poses, points);

    // Compute initial cost
    let initial_delta = scheduled_huber_delta(config, 0);
    let initial_cost = compute_cost(
        &params,
        points,
        poses,
        intrinsics,
        observations,
        pose_priors,
        camera_motions,
        config,
        initial_delta,
    );
    let mut current_cost = initial_cost;
    let mut lambda = config.initial_lambda;

    let mut result = BundleAdjustmentResult {
        initial_cost,
        final_cost: initial_cost,
        iterations: 0,
        converged: false,
        rmse: if observations.is_empty() {
            0.0
        } else {
            (initial_cost / observations.len() as f64).sqrt()
        },
    };

    for iter in 0..config.max_iterations {
        let huber_delta = scheduled_huber_delta(config, iter);
        // Compute Jacobian and residuals
        let (jacobian, residuals) = compute_jacobian_residuals(
            &params,
            num_cameras,
            num_points,
            intrinsics,
            observations,
            pose_priors,
            camera_motions,
            config,
            huber_delta,
        );

        // Compute JtJ and Jtr (normal equations)
        let jacobian_t = jacobian.transpose();
        let normal_eq = &jacobian_t * &jacobian;
        let gradient = &jacobian_t * &residuals;

        // Check gradient convergence
        let grad_norm = gradient.norm();
        if grad_norm < config.gradient_tolerance {
            result.converged = true;
            break;
        }

        // Levenberg-Marquardt update: (JtJ + lambda * diag(JtJ)) * delta = -Jtr
        let mut damped_normal = normal_eq.clone();
        for i in 0..total_params {
            damped_normal[(i, i)] += lambda * normal_eq[(i, i)].max(1e-6);
        }

        // Solve for delta
        let decomp = na::Cholesky::new(damped_normal.clone());
        let delta = match decomp {
            Some(chol) => chol.solve(&(-&gradient)),
            None => {
                // Fallback to pseudo-inverse if Cholesky fails
                lambda *= config.lambda_increase;
                continue;
            }
        };

        // Check parameter convergence
        let param_change = delta.norm() / (params.norm() + 1e-10);
        if param_change < config.parameter_tolerance {
            result.converged = true;
            break;
        }

        // Trial update
        let new_params = &params + &delta;

        // Unpack to temporary structures
        let (trial_poses, trial_points) = unpack_parameters(&new_params, num_cameras, num_points);

        // Compute new cost
        let new_cost = compute_cost_direct(
            &trial_poses,
            &trial_points,
            intrinsics,
            observations,
            pose_priors,
            camera_motions,
            config,
            huber_delta,
        );

        // Accept or reject
        if new_cost < current_cost {
            // Accept
            params = new_params;

            let cost_change = (current_cost - new_cost) / current_cost;
            current_cost = new_cost;
            lambda *= config.lambda_decrease;

            if cost_change < config.function_tolerance {
                result.converged = true;
                break;
            }
        } else {
            // Reject
            lambda *= config.lambda_increase;
        }

        result.iterations = iter + 1;
    }

    // Unpack final parameters
    let (final_poses, final_points) = unpack_parameters(&params, num_cameras, num_points);
    *poses = final_poses;
    *points = final_points
        .into_iter()
        .enumerate()
        .map(|(i, pos)| {
            let mut p = points[i].clone();
            p.position = pos;
            p
        })
        .collect();

    result.final_cost = current_cost;
    result.rmse = if observations.is_empty() {
        0.0
    } else {
        reprojection_rmse(points, poses, intrinsics, observations, camera_motions)
    };

    tracing::info!(
        "Bundle adjustment: {} iterations, cost {:.4} -> {:.4}, RMSE: {:.4}px, converged: {}",
        result.iterations,
        result.initial_cost,
        result.final_cost,
        result.rmse,
        result.converged
    );

    result
}

fn pack_parameters(poses: &[CameraPose], points: &[Point3D]) -> na::DVector<f64> {
    let num_params = poses.len() * 6 + points.len() * 3;
    let mut params = na::DVector::zeros(num_params);

    // Pack cameras
    for (i, pose) in poses.iter().enumerate() {
        let axis_angle = pose.rotation.scaled_axis();
        params[i * 6] = axis_angle.x;
        params[i * 6 + 1] = axis_angle.y;
        params[i * 6 + 2] = axis_angle.z;
        params[i * 6 + 3] = pose.translation.x;
        params[i * 6 + 4] = pose.translation.y;
        params[i * 6 + 5] = pose.translation.z;
    }

    // Pack points
    let offset = poses.len() * 6;
    for (i, point) in points.iter().enumerate() {
        params[offset + i * 3] = point.position.x;
        params[offset + i * 3 + 1] = point.position.y;
        params[offset + i * 3 + 2] = point.position.z;
    }

    params
}

fn unpack_parameters(
    params: &na::DVector<f64>,
    num_cameras: usize,
    num_points: usize,
) -> (Vec<CameraPose>, Vec<na::Point3<f64>>) {
    let mut poses = Vec::with_capacity(num_cameras);
    let mut points = Vec::with_capacity(num_points);

    // Unpack cameras
    for i in 0..num_cameras {
        let axis_angle = na::Vector3::new(params[i * 6], params[i * 6 + 1], params[i * 6 + 2]);
        let rotation = na::UnitQuaternion::from_scaled_axis(axis_angle);
        let translation = na::Vector3::new(params[i * 6 + 3], params[i * 6 + 4], params[i * 6 + 5]);
        poses.push(CameraPose {
            rotation,
            translation,
        });
    }

    // Unpack points
    let offset = num_cameras * 6;
    for i in 0..num_points {
        points.push(na::Point3::new(
            params[offset + i * 3],
            params[offset + i * 3 + 1],
            params[offset + i * 3 + 2],
        ));
    }

    (poses, points)
}

fn compute_cost(
    params: &na::DVector<f64>,
    points: &[Point3D],
    poses: &[CameraPose],
    intrinsics: &[CameraIntrinsics],
    observations: &[Observation],
    pose_priors: &[Option<PosePrior>],
    camera_motions: &[Option<CameraMotion>],
    config: &BundleAdjustmentConfig,
    huber_delta: f64,
) -> f64 {
    let (trial_poses, trial_points) = unpack_parameters(params, poses.len(), points.len());
    compute_cost_direct(
        &trial_poses,
        &trial_points,
        intrinsics,
        observations,
        pose_priors,
        camera_motions,
        config,
        huber_delta,
    )
}

fn compute_cost_direct(
    poses: &[CameraPose],
    points: &[na::Point3<f64>],
    intrinsics: &[CameraIntrinsics],
    observations: &[Observation],
    pose_priors: &[Option<PosePrior>],
    camera_motions: &[Option<CameraMotion>],
    config: &BundleAdjustmentConfig,
    huber_delta: f64,
) -> f64 {
    let obs_cost: f64 = observations
        .par_iter()
        .map(|obs| {
            if obs.camera_idx >= poses.len() || obs.point_idx >= points.len() {
                return 0.0;
            }

            let pose = &poses[obs.camera_idx];
            let point = &points[obs.point_idx];
            let k = &intrinsics[obs.camera_idx.min(intrinsics.len() - 1)];
            let motion = camera_motions.get(obs.camera_idx).and_then(|m| m.as_ref());
            let pose_eff = pose_with_motion(pose, motion, obs.time_offset);

            // Project point
            if let Some((px, py)) = project_point(point, &pose_eff, k) {
                let dx = px - obs.x;
                let dy = py - obs.y;
                let sq_error = dx * dx + dy * dy;

                if config.use_huber_loss {
                    huber_loss(sq_error.sqrt(), huber_delta)
                } else {
                    sq_error
                }
            } else {
                100.0 // Large cost for points behind camera
            }
        })
        .sum();

    let mut prior_cost = 0.0;
    if config.use_pose_priors && !pose_priors.is_empty() {
        for (cam_idx, prior_opt) in pose_priors.iter().enumerate() {
            let prior = match prior_opt {
                Some(p) => p,
                None => continue,
            };
            if cam_idx >= poses.len() {
                continue;
            }
            let pose = &poses[cam_idx];
            let delta = prior.pose.rotation.inverse() * pose.rotation;
            let rot_err = delta.scaled_axis();
            let trans_err = pose.translation - prior.pose.translation;
            let rw = prior.rotation_weight();
            let tw = prior.translation_weight();
            prior_cost += rw * rw * rot_err.dot(&rot_err) + tw * tw * trans_err.dot(&trans_err);
        }
    }

    obs_cost + prior_cost
}

fn project_point(
    point: &na::Point3<f64>,
    pose: &CameraPose,
    k: &CameraIntrinsics,
) -> Option<(f64, f64)> {
    // Transform to camera frame
    let p_cam = pose.rotation.inverse() * (point - na::Point3::from(pose.translation));

    if p_cam.z <= 0.0 {
        return None;
    }

    // Project with distortion
    let xn = p_cam.x / p_cam.z;
    let yn = p_cam.y / p_cam.z;
    let (xd, yd) = distort_normalized(k.distortion_model, &k.distortion, xn, yn);
    let x = k.fx * xd + k.cx;
    let y = k.fy * yd + k.cy;

    Some((x, y))
}

pub(crate) fn pose_with_motion(
    pose: &CameraPose,
    motion: Option<&CameraMotion>,
    time_offset: f64,
) -> CameraPose {
    if motion.is_none() || time_offset.abs() <= 1e-9 {
        return pose.clone();
    }
    let motion = motion.unwrap();
    let delta_rot = na::UnitQuaternion::from_scaled_axis(motion.angular_velocity * time_offset);
    CameraPose {
        rotation: delta_rot * pose.rotation,
        translation: pose.translation + motion.linear_velocity * time_offset,
    }
}

fn reprojection_rmse(
    points: &[Point3D],
    poses: &[CameraPose],
    intrinsics: &[CameraIntrinsics],
    observations: &[Observation],
    camera_motions: &[Option<CameraMotion>],
) -> f64 {
    let mut total = 0.0;
    let mut count = 0usize;
    for obs in observations {
        if obs.point_idx >= points.len()
            || obs.camera_idx >= poses.len()
            || obs.camera_idx >= intrinsics.len()
        {
            continue;
        }
        let point = &points[obs.point_idx];
        let pose = &poses[obs.camera_idx];
        let motion = camera_motions.get(obs.camera_idx).and_then(|m| m.as_ref());
        let pose_eff = pose_with_motion(pose, motion, obs.time_offset);
        if let Some((px, py)) =
            project_point(&point.position, &pose_eff, &intrinsics[obs.camera_idx])
        {
            let dx = px - obs.x;
            let dy = py - obs.y;
            total += dx * dx + dy * dy;
            count += 1;
        }
    }
    if count == 0 {
        return 0.0;
    }
    (total / count as f64).sqrt()
}

fn huber_loss(r: f64, delta: f64) -> f64 {
    if r.abs() <= delta {
        0.5 * r * r
    } else {
        delta * (r.abs() - 0.5 * delta)
    }
}

fn scheduled_huber_delta(config: &BundleAdjustmentConfig, iter: usize) -> f64 {
    if !config.use_huber_loss {
        return config.huber_delta;
    }
    let max_iter = config.max_iterations.max(1);
    let t = if max_iter <= 1 {
        1.0
    } else {
        (iter as f64 / (max_iter - 1) as f64).clamp(0.0, 1.0)
    };
    let start = config.huber_delta * 3.0;
    start + (config.huber_delta - start) * t
}

fn skew(v: &na::Vector3<f64>) -> na::Matrix3<f64> {
    na::Matrix3::new(0.0, -v.z, v.y, v.z, 0.0, -v.x, -v.y, v.x, 0.0)
}

fn right_jacobian_inv(w: &na::Vector3<f64>) -> na::Matrix3<f64> {
    let theta = w.norm();
    let hat = skew(w);
    let identity = na::Matrix3::identity();
    if theta < 1e-8 {
        return identity + 0.5 * hat + (1.0 / 12.0) * (hat * hat);
    }
    let theta2 = theta * theta;
    let theta_sin = theta.sin();
    let theta_cos = theta.cos();
    let coeff = 1.0 / theta2 - (1.0 + theta_cos) / (2.0 * theta * theta_sin);
    identity + 0.5 * hat + coeff * (hat * hat)
}

fn compute_jacobian_residuals(
    params: &na::DVector<f64>,
    num_cameras: usize,
    num_points: usize,
    intrinsics: &[CameraIntrinsics],
    observations: &[Observation],
    pose_priors: &[Option<PosePrior>],
    camera_motions: &[Option<CameraMotion>],
    config: &BundleAdjustmentConfig,
    huber_delta: f64,
) -> (na::DMatrix<f64>, na::DVector<f64>) {
    let num_obs = observations.len();
    let total_params = num_cameras * 6 + num_points * 3;
    let mut prior_indices: Vec<(usize, PosePrior)> = Vec::new();
    if config.use_pose_priors && !pose_priors.is_empty() {
        for (cam_idx, prior_opt) in pose_priors.iter().enumerate() {
            if let Some(prior) = prior_opt {
                prior_indices.push((cam_idx, prior.clone()));
            }
        }
    }
    let num_prior = prior_indices.len();
    let total_residuals = num_obs * 2 + num_prior * 6;

    // 2 residuals per observation (x, y)
    let mut jacobian = na::DMatrix::<f64>::zeros(total_residuals, total_params);
    let mut residuals = na::DVector::<f64>::zeros(total_residuals);

    let (poses, points) = unpack_parameters(params, num_cameras, num_points);

    for (obs_idx, obs) in observations.iter().enumerate() {
        if obs.camera_idx >= num_cameras || obs.point_idx >= num_points {
            continue;
        }

        let k = &intrinsics[obs.camera_idx.min(intrinsics.len() - 1)];
        let point = &points[obs.point_idx];
        let pose = &poses[obs.camera_idx];
        let motion = camera_motions.get(obs.camera_idx).and_then(|m| m.as_ref());
        let pose_eff = pose_with_motion(pose, motion, obs.time_offset);

        // Current projection
        if let Some((px, py)) = project_point(point, &pose_eff, k) {
            let r_x = px - obs.x;
            let r_y = py - obs.y;
            let r_norm = (r_x * r_x + r_y * r_y).sqrt().max(1e-12);
            let weight = if config.use_huber_loss {
                if r_norm <= huber_delta {
                    1.0
                } else {
                    huber_delta / r_norm
                }
            } else {
                1.0
            };
            let w = weight.sqrt();
            residuals[obs_idx * 2] = w * r_x;
            residuals[obs_idx * 2 + 1] = w * r_y;

            let cam_offset = obs.camera_idx * 6;
            let point_offset = num_cameras * 6 + obs.point_idx * 3;

            // Analytic Jacobians for translation + point parameters
            let r_cw = pose_eff.rotation.inverse().to_rotation_matrix();
            let p_cam = r_cw * (point.coords - pose_eff.translation);
            let x = p_cam.x;
            let y = p_cam.y;
            let z = p_cam.z;
            if z.abs() > 1e-12 {
                let inv_z = 1.0 / z;
                let inv_z2 = inv_z * inv_z;

                let j_proj = na::Matrix::<f64, na::U2, na::U3, _>::new(
                    k.fx * inv_z,
                    0.0,
                    -k.fx * x * inv_z2,
                    0.0,
                    k.fy * inv_z,
                    -k.fy * y * inv_z2,
                );

                let j_point = j_proj * r_cw.matrix();
                let j_trans = -&j_point;
                let w_vec = pose_eff.rotation.scaled_axis();
                let j_r_inv = right_jacobian_inv(&w_vec);
                let j_rot_cam = -(skew(&p_cam) * j_r_inv);
                let j_rot = j_proj * j_rot_cam;

                // Translation jacobian
                jacobian[(obs_idx * 2, cam_offset + 3)] = w * j_trans[(0, 0)];
                jacobian[(obs_idx * 2, cam_offset + 4)] = w * j_trans[(0, 1)];
                jacobian[(obs_idx * 2, cam_offset + 5)] = w * j_trans[(0, 2)];
                jacobian[(obs_idx * 2 + 1, cam_offset + 3)] = w * j_trans[(1, 0)];
                jacobian[(obs_idx * 2 + 1, cam_offset + 4)] = w * j_trans[(1, 1)];
                jacobian[(obs_idx * 2 + 1, cam_offset + 5)] = w * j_trans[(1, 2)];

                // Point jacobian
                jacobian[(obs_idx * 2, point_offset)] = w * j_point[(0, 0)];
                jacobian[(obs_idx * 2, point_offset + 1)] = w * j_point[(0, 1)];
                jacobian[(obs_idx * 2, point_offset + 2)] = w * j_point[(0, 2)];
                jacobian[(obs_idx * 2 + 1, point_offset)] = w * j_point[(1, 0)];
                jacobian[(obs_idx * 2 + 1, point_offset + 1)] = w * j_point[(1, 1)];
                jacobian[(obs_idx * 2 + 1, point_offset + 2)] = w * j_point[(1, 2)];

                // Rotation jacobian (axis-angle)
                jacobian[(obs_idx * 2, cam_offset)] = w * j_rot[(0, 0)];
                jacobian[(obs_idx * 2, cam_offset + 1)] = w * j_rot[(0, 1)];
                jacobian[(obs_idx * 2, cam_offset + 2)] = w * j_rot[(0, 2)];
                jacobian[(obs_idx * 2 + 1, cam_offset)] = w * j_rot[(1, 0)];
                jacobian[(obs_idx * 2 + 1, cam_offset + 1)] = w * j_rot[(1, 1)];
                jacobian[(obs_idx * 2 + 1, cam_offset + 2)] = w * j_rot[(1, 2)];
            }
        }
    }

    if num_prior > 0 {
        let base_row = num_obs * 2;
        for (prior_idx, (cam_idx, prior)) in prior_indices.iter().enumerate() {
            if *cam_idx >= num_cameras {
                continue;
            }
            let pose = &poses[*cam_idx];
            let cam_offset = cam_idx * 6;
            let delta = prior.pose.rotation.inverse() * pose.rotation;
            let rot_err = delta.scaled_axis();
            let trans_err = pose.translation - prior.pose.translation;
            let rw = prior.rotation_weight();
            let tw = prior.translation_weight();
            let row = base_row + prior_idx * 6;

            residuals[row] = rw * rot_err.x;
            residuals[row + 1] = rw * rot_err.y;
            residuals[row + 2] = rw * rot_err.z;
            residuals[row + 3] = tw * trans_err.x;
            residuals[row + 4] = tw * trans_err.y;
            residuals[row + 5] = tw * trans_err.z;

            let j_rot = right_jacobian_inv(&rot_err);
            for i in 0..3 {
                for j in 0..3 {
                    jacobian[(row + i, cam_offset + j)] = rw * j_rot[(i, j)];
                }
            }
            jacobian[(row + 3, cam_offset + 3)] = tw;
            jacobian[(row + 4, cam_offset + 4)] = tw;
            jacobian[(row + 5, cam_offset + 5)] = tw;
        }
    }

    (jacobian, residuals)
}

/// Build observations using keypoints from images.
/// Uses point track (camera_idx, keypoint_idx) to fetch 2D coordinates.
pub fn build_observations_with_images(
    points: &[Point3D],
    images: &[ImageData],
) -> Vec<Observation> {
    let mut observations = Vec::new();

    for (point_idx, point) in points.iter().enumerate() {
        for &(camera_idx, kp_idx) in &point.track {
            if camera_idx >= images.len() {
                continue;
            }
            let image = &images[camera_idx];
            if kp_idx >= image.keypoints.len() {
                continue;
            }
            let kp = &image.keypoints[kp_idx];
            let time_offset = image
                .rolling_shutter
                .as_ref()
                .map(|rs| {
                    rs.time_offset_seconds(
                        kp.x as f64,
                        kp.y as f64,
                        image.intrinsics.width,
                        image.intrinsics.height,
                    )
                })
                .unwrap_or(0.0);
            observations.push(Observation {
                point_idx,
                camera_idx,
                x: kp.x as f64,
                y: kp.y as f64,
                time_offset,
            });
        }
    }

    observations
}
