// Object pose estimation using solvePnP
// Tracks 3D coordinate frame (X, Y, Z axes) of the object

use anyhow::Result;
use image::{ImageBuffer, Rgb};
use nalgebra as na;
use opencv::{
    calib3d,
    core::{self, Mat, Point2d, Point2f, Point3f, Vector, Scalar},
    prelude::*,
};

use crate::cv::{Feature, CameraIntrinsics, DistortionModel};

/// Object pose in 3D space
#[derive(Debug, Clone)]
pub struct ObjectPose {
    /// Rotation as quaternion
    pub rotation: na::UnitQuaternion<f32>,
    /// Translation vector (position)
    pub translation: na::Vector3<f32>,
    /// Confidence (0-1)
    pub confidence: f32,
}

impl Default for ObjectPose {
    fn default() -> Self {
        Self {
            rotation: na::UnitQuaternion::identity(),
            translation: na::Vector3::zeros(),
            confidence: 0.0,
        }
    }
}

impl ObjectPose {
    /// Get the 3D axes of the object coordinate frame
    /// Returns (X-axis, Y-axis, Z-axis) as unit vectors
    pub fn get_axes(&self) -> (na::Vector3<f32>, na::Vector3<f32>, na::Vector3<f32>) {
        let x_axis = self.rotation * na::Vector3::x();
        let y_axis = self.rotation * na::Vector3::y();
        let z_axis = self.rotation * na::Vector3::z();
        (x_axis, y_axis, z_axis)
    }
}

/// Estimate object pose from 2D-3D correspondences using solvePnP
/// This gives us the 3D coordinate frame of the object!
pub fn estimate_pose_pnp(
    points_3d: &[na::Point3<f32>],
    points_2d: &[(f32, f32)],
    intrinsics: &CameraIntrinsics,
) -> Result<ObjectPose> {
    if points_3d.len() < 4 || points_2d.len() < 4 || points_3d.len() != points_2d.len() {
        anyhow::bail!("Need at least 4 point correspondences");
    }

    // Convert to OpenCV format
    let mut object_points = Vector::<Point3f>::new();
    for p in points_3d {
        object_points.push(Point3f::new(p.x, p.y, p.z));
    }

    let mut image_points = Vector::<Point2f>::new();
    let mut pre_undistorted = false;
    for &(x, y) in points_2d {
        let (xn, yn, corrected) = corrected_normalized_point(x as f64, y as f64, intrinsics);
        pre_undistorted |= corrected;
        let x_u = (intrinsics.fx * xn + intrinsics.cx) as f32;
        let y_u = (intrinsics.fy * yn + intrinsics.cy) as f32;
        image_points.push(Point2f::new(x_u, y_u));
    }

    // Camera matrix
    let camera_matrix = Mat::from_slice_2d(&[
        &[intrinsics.fx, 0.0, intrinsics.cx],
        &[0.0, intrinsics.fy, intrinsics.cy],
        &[0.0, 0.0, 1.0],
    ])?;

    // Distortion coefficients (skip if we already undistorted/compensated)
    let dist_coeffs = if pre_undistorted {
        Mat::default()
    } else {
        intrinsics.to_opencv_dist_coeffs()?
    };

    // Rotation and translation vectors
    let mut rvec = Mat::default();
    let mut tvec = Mat::default();

    // Solve PnP using RANSAC for robustness
    let success = calib3d::solve_pnp(
        &object_points,
        &image_points,
        &camera_matrix,
        &dist_coeffs,
        &mut rvec,
        &mut tvec,
        false, // use_extrinsic_guess
        calib3d::SOLVEPNP_ITERATIVE,
    )?;

    if !success {
        anyhow::bail!("solvePnP failed");
    }

    // Convert rotation vector to quaternion
    let rvec_data: Vec<f64> = rvec.data_typed()?.to_vec();
    let rx = rvec_data[0] as f32;
    let ry = rvec_data[1] as f32;
    let rz = rvec_data[2] as f32;

    // Rodrigues formula: angle = ||rvec||, axis = rvec / ||rvec||
    let angle = (rx * rx + ry * ry + rz * rz).sqrt();
    let rotation = if angle > 0.001 {
        let axis = na::Unit::new_normalize(na::Vector3::new(rx / angle, ry / angle, rz / angle));
        na::UnitQuaternion::from_axis_angle(&axis, angle)
    } else {
        na::UnitQuaternion::identity()
    };

    // Translation vector
    let tvec_data: Vec<f64> = tvec.data_typed()?.to_vec();
    let translation = na::Vector3::new(
        tvec_data[0] as f32,
        tvec_data[1] as f32,
        tvec_data[2] as f32,
    );

    let confidence = reprojection_confidence(points_3d, points_2d, &rotation, &translation, intrinsics);

    Ok(ObjectPose {
        rotation,
        translation,
        confidence,
    })
}

fn reprojection_confidence(
    points_3d: &[na::Point3<f32>],
    points_2d: &[(f32, f32)],
    rotation: &na::UnitQuaternion<f32>,
    translation: &na::Vector3<f32>,
    intrinsics: &CameraIntrinsics,
) -> f32 {
    let mut total_err = 0.0f32;
    let mut count = 0u32;
    for (p3, (u_obs, v_obs)) in points_3d.iter().zip(points_2d.iter()) {
        let pc = rotation.transform_point(p3) + translation;
        if pc.z <= 1e-6 {
            continue;
        }
        let xn = pc.x / pc.z;
        let yn = pc.y / pc.z;
        let (xd, yd) = intrinsics.distort_normalized(xn as f64, yn as f64);
        let u = (intrinsics.fx * xd + intrinsics.cx) as f32;
        let v = (intrinsics.fy * yd + intrinsics.cy) as f32;
        let du = u - u_obs;
        let dv = v - v_obs;
        total_err += du * du + dv * dv;
        count += 1;
    }
    if count == 0 {
        return 0.0;
    }
    let rmse = (total_err / count as f32).sqrt();
    let confidence = 1.0 / (1.0 + rmse);
    confidence.clamp(0.0, 1.0)
}

fn corrected_normalized_point(x: f64, y: f64, intrinsics: &CameraIntrinsics) -> (f64, f64, bool) {
    let mut xn = (x - intrinsics.cx) / intrinsics.fx;
    let mut yn = (y - intrinsics.cy) / intrinsics.fy;
    let mut corrected = false;

    if intrinsics.distortion_model != DistortionModel::None {
        let (xu, yu) = intrinsics.undistort_normalized(xn, yn);
        xn = xu;
        yn = yu;
        corrected = true;
    }

    if let (Some(rs), Some(motion)) = (&intrinsics.rolling_shutter, &intrinsics.camera_motion) {
        let dt = rs.time_offset_seconds(x, y, intrinsics.width, intrinsics.height);
        if dt.abs() > 1e-9 {
            let ray = na::Vector3::new(xn, yn, 1.0).normalize();
            let delta = na::UnitQuaternion::from_scaled_axis(-motion.angular_velocity * dt);
            let ray_corr = delta * ray;
            xn = ray_corr.x / ray_corr.z;
            yn = ray_corr.y / ray_corr.z;
            corrected = true;
        }
    }

    (xn, yn, corrected)
}

/// Estimate camera motion from feature matches using Essential Matrix
/// Returns (rotation, translation) of camera motion
pub fn estimate_camera_motion(
    features1: &[Feature],
    features2: &[Feature],
    matches: &[(usize, usize)],
    intrinsics: &CameraIntrinsics,
) -> Result<(na::UnitQuaternion<f32>, na::Vector3<f32>)> {
    log::debug!("🔍 estimate_camera_motion: {} matches", matches.len());
    if matches.len() < 5 {
        anyhow::bail!("Need at least 5 matches for essential matrix (got {})", matches.len());
    }

    // Convert to OpenCV format
    let mut points1 = Vector::<Point2f>::new();
    let mut points2 = Vector::<Point2f>::new();

    for &(idx1, idx2) in matches {
        let (x1, y1) = features1[idx1].point;
        let (x2, y2) = features2[idx2].point;
        let (x1n, y1n, _) = corrected_normalized_point(x1 as f64, y1 as f64, intrinsics);
        let (x2n, y2n, _) = corrected_normalized_point(x2 as f64, y2 as f64, intrinsics);
        points1.push(Point2f::new((intrinsics.fx * x1n + intrinsics.cx) as f32, (intrinsics.fy * y1n + intrinsics.cy) as f32));
        points2.push(Point2f::new((intrinsics.fx * x2n + intrinsics.cx) as f32, (intrinsics.fy * y2n + intrinsics.cy) as f32));
    }

    // Camera matrix
    let camera_matrix = Mat::from_slice_2d(&[
        &[intrinsics.fx, 0.0, intrinsics.cx],
        &[0.0, intrinsics.fy, intrinsics.cy],
        &[0.0, 0.0, 1.0],
    ])?;

    // Find essential matrix using RANSAC
    let mut mask = Mat::default();
    let essential_matrix = calib3d::find_essential_mat(
        &points1,
        &points2,
        &camera_matrix,
        calib3d::RANSAC,
        0.999, // confidence
        1.0,   // threshold
        1000,  // max_iters
        &mut mask,
    )?;

    // Recover pose from essential matrix
    let mut R = Mat::default();
    let mut t = Mat::default();
    let mut triangulated_points = Mat::default();
    let inliers = calib3d::recover_pose_triangulated(
        &essential_matrix,
        &points1,
        &points2,
        &camera_matrix,
        &mut R,
        &mut t,
        1000.0, // distance_threshold
        &mut mask,
        &mut triangulated_points,
    )?;

    log::debug!("🔍 Essential matrix: {} inliers from {} matches", inliers, matches.len());
    if inliers < 5 {
        anyhow::bail!("Not enough inliers: {} < 5", inliers);
    }

    // Convert rotation matrix to quaternion
    let R_data: Vec<f64> = R.data_typed()?.to_vec();
    let rotation_matrix = na::Matrix3::new(
        R_data[0] as f32, R_data[1] as f32, R_data[2] as f32,
        R_data[3] as f32, R_data[4] as f32, R_data[5] as f32,
        R_data[6] as f32, R_data[7] as f32, R_data[8] as f32,
    );
    let rotation = na::UnitQuaternion::from_matrix(&rotation_matrix);

    // Translation vector
    let t_data: Vec<f64> = t.data_typed()?.to_vec();
    let translation = na::Vector3::new(
        t_data[0] as f32,
        t_data[1] as f32,
        t_data[2] as f32,
    );

    let (roll, pitch, yaw) = rotation.euler_angles();
    log::info!("✅ Camera motion: {} inliers, rotation=(r:{:.2}, p:{:.2}, y:{:.2}), translation=({:.3}, {:.3}, {:.3})",
              inliers, roll, pitch, yaw, translation.x, translation.y, translation.z);

    Ok((rotation, translation))
}

/// Triangulate 3D points from two views using proper camera matrices
/// Much better than simple depth estimation!
pub fn triangulate_points(
    features1: &[Feature],
    features2: &[Feature],
    matches: &[(usize, usize)],
    intrinsics: &CameraIntrinsics,
    rotation: &na::UnitQuaternion<f32>,
    translation: &na::Vector3<f32>,
) -> Result<Vec<na::Point3<f32>>> {
    if matches.is_empty() {
        return Ok(Vec::new());
    }

    // Convert to OpenCV format - use f64 to match projection matrices
    let mut points1 = Vector::<Point2d>::new();
    let mut points2 = Vector::<Point2d>::new();

    for &(idx1, idx2) in matches {
        let (x1, y1) = features1[idx1].point;
        let (x2, y2) = features2[idx2].point;
        points1.push(Point2d::new(x1 as f64, y1 as f64));
        points2.push(Point2d::new(x2 as f64, y2 as f64));
    }

    // Camera matrix
    let K = Mat::from_slice_2d(&[
        &[intrinsics.fx, 0.0, intrinsics.cx],
        &[0.0, intrinsics.fy, intrinsics.cy],
        &[0.0, 0.0, 1.0],
    ])?;

    // Projection matrix for first camera: P1 = K * [I | 0]
    let P1 = Mat::from_slice_2d(&[
        &[intrinsics.fx, 0.0, intrinsics.cx, 0.0],
        &[0.0, intrinsics.fy, intrinsics.cy, 0.0],
        &[0.0, 0.0, 1.0, 0.0],
    ])?;

    // Projection matrix for second camera: P2 = K * [R | t]
    let R_mat = rotation.to_rotation_matrix();
    let R = R_mat.matrix();
    let P2 = Mat::from_slice_2d(&[
        &[
            intrinsics.fx * R[(0, 0)] as f64 + intrinsics.cx * R[(2, 0)] as f64,
            intrinsics.fx * R[(0, 1)] as f64 + intrinsics.cx * R[(2, 1)] as f64,
            intrinsics.fx * R[(0, 2)] as f64 + intrinsics.cx * R[(2, 2)] as f64,
            intrinsics.fx * translation.x as f64 + intrinsics.cx * translation.z as f64,
        ],
        &[
            intrinsics.fy * R[(1, 0)] as f64 + intrinsics.cy * R[(2, 0)] as f64,
            intrinsics.fy * R[(1, 1)] as f64 + intrinsics.cy * R[(2, 1)] as f64,
            intrinsics.fy * R[(1, 2)] as f64 + intrinsics.cy * R[(2, 2)] as f64,
            intrinsics.fy * translation.y as f64 + intrinsics.cy * translation.z as f64,
        ],
        &[
            R[(2, 0)] as f64,
            R[(2, 1)] as f64,
            R[(2, 2)] as f64,
            translation.z as f64,
        ],
    ])?;

    // Triangulate points
    let mut points_4d = Mat::default();
    calib3d::triangulate_points(&P1, &P2, &points1, &points2, &mut points_4d)?;

    // Convert homogeneous coordinates to 3D points
    let mut points_3d = Vec::new();
    for i in 0..points_4d.cols() {
        let x = points_4d.at_2d::<f64>(0, i)?;
        let y = points_4d.at_2d::<f64>(1, i)?;
        let z = points_4d.at_2d::<f64>(2, i)?;
        let w = points_4d.at_2d::<f64>(3, i)?;

        if w.abs() > 1e-6 {
            points_3d.push(na::Point3::new(
                (x / w) as f32,
                (y / w) as f32,
                (z / w) as f32,
            ));
        }
    }

    log::debug!("📐 Triangulated {} 3D points from {} matches", points_3d.len(), matches.len());

    Ok(points_3d)
}
