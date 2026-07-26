//! Bundle Adjustment Optimization
//!
//! Refines camera poses and 3D points using Levenberg-Marquardt.

pub mod bundle_adjustment;

pub use bundle_adjustment::{
    BundleAdjustmentConfig, BundleAdjustmentResult, Observation,
    PosePrior, bundle_adjust_lm, build_observations_with_images,
};

use crate::{Point3D, CameraPose, ImageData, CameraIntrinsics, CameraMotion};
use nalgebra as na;

/// Run bundle adjustment to refine reconstruction
pub fn bundle_adjust(
    points: &mut Vec<Point3D>,
    poses: &mut Vec<CameraPose>,
    images: &[ImageData],
    max_iterations: usize,
) -> anyhow::Result<()> {
    tracing::info!("Running {} iterations of bundle adjustment (LM)", max_iterations);
    if points.is_empty() || poses.is_empty() {
        return Ok(());
    }
    let intrinsics: Vec<CameraIntrinsics> = images.iter().map(|i| i.intrinsics.clone()).collect();
    let observations = build_observations_with_images(points, images);
    if observations.is_empty() {
        return Ok(());
    }
    let config = BundleAdjustmentConfig {
        max_iterations,
        ..Default::default()
    };
    let pose_priors: Vec<Option<PosePrior>> = if config.use_pose_priors {
        images.iter().map(|img| {
            img.prior_pose.as_ref().map(|pose| PosePrior {
                pose: pose.clone(),
                rotation_sigma: config.pose_prior_rotation_sigma,
                translation_sigma: config.pose_prior_translation_sigma,
            })
        }).collect()
    } else {
        vec![None; images.len()]
    };
    let camera_motions: Vec<Option<CameraMotion>> = images.iter().map(|img| img.camera_motion.clone()).collect();
    let _ = bundle_adjust_lm(
        points,
        poses,
        &intrinsics,
        &observations,
        &pose_priors,
        &camera_motions,
        &config,
    );
    Ok(())
}

pub fn local_bundle_adjust(
    points: &mut Vec<Point3D>,
    poses: &mut Vec<CameraPose>,
    images: &[ImageData],
    window: usize,
    stride: usize,
    iterations: usize,
    min_points: usize,
    min_rmse: f64,
) -> anyhow::Result<()> {
    if window < 2 || poses.len() < 2 {
        return Ok(());
    }
    let stride = stride.max(1);
    let mut start = 0usize;
    while start + 1 < poses.len() {
        let end = (start + window).min(poses.len());
        if end - start < 2 {
            break;
        }
        let camera_indices: Vec<usize> = (start..end).collect();
        let mut camera_map = std::collections::HashMap::new();
        for (local_idx, global_idx) in camera_indices.iter().enumerate() {
            camera_map.insert(*global_idx, local_idx);
        }

        let mut local_points: Vec<Point3D> = Vec::new();
        let mut point_map: Vec<usize> = Vec::new();
        for (global_idx, point) in points.iter().enumerate() {
            let mut track = Vec::new();
            for &(cam_idx, kp_idx) in &point.track {
                if let Some(&local_cam_idx) = camera_map.get(&cam_idx) {
                    track.push((local_cam_idx, kp_idx));
                }
            }
            if track.len() >= 2 {
                let mut local_point = point.clone();
                local_point.track = track;
                local_points.push(local_point);
                point_map.push(global_idx);
            }
        }

        if local_points.len() < min_points {
            start += stride;
            continue;
        }

        let local_poses: Vec<CameraPose> = camera_indices.iter().map(|&i| poses[i].clone()).collect();
        let local_intrinsics: Vec<CameraIntrinsics> =
            camera_indices.iter().map(|&i| images[i].intrinsics.clone()).collect();

        let observations = build_local_observations(&local_points, images, &camera_indices);
        if observations.is_empty() {
            start += stride;
            continue;
        }
        let local_motions: Vec<Option<CameraMotion>> =
            camera_indices.iter().map(|&idx| images[idx].camera_motion.clone()).collect();
        let rmse = reprojection_rmse(
            &local_points,
            &local_poses,
            &local_intrinsics,
            &observations,
            &local_motions,
        );
        if rmse < min_rmse {
            start += stride;
            continue;
        }

        let config = BundleAdjustmentConfig {
            max_iterations: iterations,
            ..Default::default()
        };
        let mut refined_poses = local_poses.clone();
        let mut refined_points = local_points.clone();
        let local_priors: Vec<Option<PosePrior>> = if config.use_pose_priors {
            camera_indices.iter().map(|&idx| {
                images[idx].prior_pose.as_ref().map(|pose| PosePrior {
                    pose: pose.clone(),
                    rotation_sigma: config.pose_prior_rotation_sigma,
                    translation_sigma: config.pose_prior_translation_sigma,
                })
            }).collect()
        } else {
            vec![None; camera_indices.len()]
        };
        let _ = bundle_adjust_lm(
            &mut refined_points,
            &mut refined_poses,
            &local_intrinsics,
            &observations,
            &local_priors,
            &local_motions,
            &config,
        );

        for (local_idx, global_idx) in camera_indices.iter().enumerate() {
            poses[*global_idx] = refined_poses[local_idx].clone();
        }
        for (local_idx, global_idx) in point_map.iter().enumerate() {
            points[*global_idx].position = refined_points[local_idx].position;
        }

        start += stride;
    }
    Ok(())
}

fn build_local_observations(
    points: &[Point3D],
    images: &[ImageData],
    camera_indices: &[usize],
) -> Vec<Observation> {
    let mut observations = Vec::new();
    for (point_idx, point) in points.iter().enumerate() {
        for &(local_cam_idx, kp_idx) in &point.track {
            if let Some(global_cam_idx) = camera_indices.get(local_cam_idx) {
                if *global_cam_idx >= images.len() {
                    continue;
                }
                let image = &images[*global_cam_idx];
                if kp_idx >= image.keypoints.len() {
                    continue;
                }
                let kp = &image.keypoints[kp_idx];
                let time_offset = image
                    .rolling_shutter
                    .as_ref()
                    .map(|rs| rs.time_offset_seconds(kp.x as f64, kp.y as f64, image.intrinsics.width, image.intrinsics.height))
                    .unwrap_or(0.0);
                observations.push(Observation {
                    point_idx,
                    camera_idx: local_cam_idx,
                    x: kp.x as f64,
                    y: kp.y as f64,
                    time_offset,
                });
            }
        }
    }
    observations
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
        if obs.point_idx >= points.len() || obs.camera_idx >= poses.len() || obs.camera_idx >= intrinsics.len() {
            continue;
        }
        let point = &points[obs.point_idx];
        let pose = &poses[obs.camera_idx];
        let motion = camera_motions.get(obs.camera_idx).and_then(|m| m.as_ref());
        let pose_eff = crate::optimization::bundle_adjustment::pose_with_motion(pose, motion, obs.time_offset);
        let intr = &intrinsics[obs.camera_idx];
        let p_cam = pose_eff.rotation.inverse() * (point.position - na::Point3::from(pose_eff.translation));
        if p_cam.z <= 0.0 {
            continue;
        }
        let x = intr.fx * p_cam.x / p_cam.z + intr.cx;
        let y = intr.fy * p_cam.y / p_cam.z + intr.cy;
        let dx = x - obs.x;
        let dy = y - obs.y;
        total += dx * dx + dy * dy;
        count += 1;
    }
    if count == 0 {
        return 0.0;
    }
    (total / count as f64).sqrt()
}
