// Structure from Motion (SfM) module
// Estimates camera poses and triangulates 3D points from image sequences

use anyhow::{Context, Result};
use crate::cv::{CameraIntrinsics, CameraPose, Feature};
use image::{ImageBuffer, Rgb};
use nalgebra as na;
use opencv::{
    core::{self, Mat, Point2f, Vector},
    calib3d,
    prelude::*,
};

/// 3D Point with color and confidence
#[derive(Debug, Clone)]
pub struct ColoredPoint {
    pub position: na::Point3<f32>,
    pub color: [u8; 3],
    pub confidence: f32,
}

/// Estimate relative camera pose between two frames using matched features
pub fn estimate_pose(
    features1: &[Feature],
    features2: &[Feature],
    matches: &[(usize, usize)],
    intrinsics: &CameraIntrinsics,
) -> Result<CameraPose> {
    if matches.len() < 8 {
        anyhow::bail!("Need at least 8 matches for pose estimation, got {}", matches.len());
    }

    // Extract matched points
    let mut points1 = Vector::<Point2f>::new();
    let mut points2 = Vector::<Point2f>::new();

    for &(idx1, idx2) in matches {
        let (x1, y1) = features1[idx1].point;
        let (x2, y2) = features2[idx2].point;
        points1.push(Point2f::new(x1, y1));
        points2.push(Point2f::new(x2, y2));
    }

    // Camera matrix
    let camera_matrix = intrinsics.to_camera_matrix()?;

    // Find essential matrix
    let mut mask = Mat::default();

    let essential_mat = calib3d::find_essential_mat(
        &points1,
        &points2,
        &camera_matrix,
        calib3d::RANSAC,
        0.999,
        1.0,
        1000, // max iterations
        &mut mask,
    )?;

    // Recover pose from essential matrix
    let mut rotation = Mat::default();
    let mut translation = Mat::default();
    let mut mask2 = Mat::default();

    calib3d::recover_pose(
        &essential_mat,
        &points1,
        &points2,
        &mut translation,
        &mut rotation,
        intrinsics.fx,
        core::Point2d::new(intrinsics.cx, intrinsics.cy),
        &mut mask2,
    )?;

    // Convert to nalgebra
    let r = rotation_mat_to_nalgebra(&rotation)?;
    let mut t = translation_mat_to_nalgebra(&translation)?;

    // Normalize translation to a reasonable scale
    // Monocular SfM has arbitrary scale - normalize to unit length
    let t_norm = t.norm();
    if t_norm > 1e-6 {
        t = t / t_norm;
        // Scale to reasonable camera motion (assume ~10cm movement between frames)
        t = t * 0.1;
    }

    log::debug!("Camera pose: rotation norm={:.3}, translation=({:.3}, {:.3}, {:.3})",
                r.determinant(), t.x, t.y, t.z);

    Ok(CameraPose {
        rotation: r,
        translation: t,
    })
}

/// Triangulate 3D points from matched features and camera poses
pub fn triangulate_points(
    features1: &[Feature],
    features2: &[Feature],
    matches: &[(usize, usize)],
    pose1: &CameraPose,
    pose2: &CameraPose,
    intrinsics: &CameraIntrinsics,
    image1: &ImageBuffer<Rgb<u8>, Vec<u8>>,
) -> Result<Vec<ColoredPoint>> {
    if matches.is_empty() {
        return Ok(Vec::new());
    }

    // Create projection matrices
    let k = intrinsics.to_nalgebra_matrix();
    
    // P1 = K * [R1 | t1]
    let mut p1_mat = na::Matrix3x4::zeros();
    p1_mat.fixed_view_mut::<3, 3>(0, 0).copy_from(&pose1.rotation);
    p1_mat.column_mut(3).copy_from(&pose1.translation);
    let proj1 = k * p1_mat;
    
    // P2 = K * [R2 | t2]
    let mut p2_mat = na::Matrix3x4::zeros();
    p2_mat.fixed_view_mut::<3, 3>(0, 0).copy_from(&pose2.rotation);
    p2_mat.column_mut(3).copy_from(&pose2.translation);
    let proj2 = k * p2_mat;

    let mut points_3d = Vec::new();
    let mut rejected_count = 0;

    for &(idx1, idx2) in matches {
        let (x1, y1) = features1[idx1].point;
        let (x2, y2) = features2[idx2].point;

        // Triangulate using DLT (Direct Linear Transform)
        let point_3d = match triangulate_point_dlt(
            (x1 as f64, y1 as f64),
            (x2 as f64, y2 as f64),
            &proj1,
            &proj2,
        ) {
            Ok(p) => p,
            Err(_) => {
                rejected_count += 1;
                continue;
            }
        };

        // Filter out points that are too far or behind camera
        let depth = point_3d.z;
        // More lenient depth range for normalized scale
        if depth < 0.01 || depth > 5.0 {  // Reasonable depth range for normalized coordinates
            rejected_count += 1;
            continue;
        }

        // Also filter by distance from origin (outlier rejection)
        let distance = (point_3d.x.powi(2) + point_3d.y.powi(2) + point_3d.z.powi(2)).sqrt();
        if distance > 2.0 {  // Points too far from origin are likely outliers
            rejected_count += 1;
            continue;
        }

        // Calculate reprojection error for quality check
        let reprojection_error = calculate_reprojection_error(
            &point_3d,
            (x1 as f64, y1 as f64),
            (x2 as f64, y2 as f64),
            &proj1,
            &proj2,
        );

        // Reject points with high reprojection error
        if reprojection_error > 5.0 {  // 5 pixels max error
            rejected_count += 1;
            continue;
        }

        // Calculate confidence based on reprojection error
        let confidence = (1.0 - (reprojection_error / 5.0).min(1.0)) as f32;

        // Get color from image
        let px = x1.clamp(0.0, (intrinsics.width - 1) as f32) as u32;
        let py = y1.clamp(0.0, (intrinsics.height - 1) as f32) as u32;
        let pixel = image1.get_pixel(px, py);

        points_3d.push(ColoredPoint {
            position: na::Point3::new(point_3d.x as f32, point_3d.y as f32, point_3d.z as f32),
            color: [pixel[0], pixel[1], pixel[2]],
            confidence,
        });
    }

    log::debug!("Triangulated {} 3D points ({} rejected)", points_3d.len(), rejected_count);
    Ok(points_3d)
}

/// Calculate reprojection error for a 3D point
fn calculate_reprojection_error(
    point_3d: &na::Point3<f64>,
    point1: (f64, f64),
    point2: (f64, f64),
    proj1: &na::Matrix3x4<f64>,
    proj2: &na::Matrix3x4<f64>,
) -> f64 {
    // Homogeneous 3D point
    let p_hom = na::Vector4::new(point_3d.x, point_3d.y, point_3d.z, 1.0);

    // Reproject to first view
    let p1_proj = proj1 * p_hom;
    let p1_2d = (p1_proj.x / p1_proj.z, p1_proj.y / p1_proj.z);
    let error1 = ((p1_2d.0 - point1.0).powi(2) + (p1_2d.1 - point1.1).powi(2)).sqrt();

    // Reproject to second view
    let p2_proj = proj2 * p_hom;
    let p2_2d = (p2_proj.x / p2_proj.z, p2_proj.y / p2_proj.z);
    let error2 = ((p2_2d.0 - point2.0).powi(2) + (p2_2d.1 - point2.1).powi(2)).sqrt();

    // Return average error
    (error1 + error2) / 2.0
}

/// Triangulate a single point using Direct Linear Transform
fn triangulate_point_dlt(
    point1: (f64, f64),
    point2: (f64, f64),
    proj1: &na::Matrix3x4<f64>,
    proj2: &na::Matrix3x4<f64>,
) -> Result<na::Point3<f64>> {
    // Build matrix A for DLT
    let mut a = na::Matrix4::zeros();
    
    // From first view
    a.row_mut(0).copy_from(&(point1.0 * proj1.row(2) - proj1.row(0)));
    a.row_mut(1).copy_from(&(point1.1 * proj1.row(2) - proj1.row(1)));
    
    // From second view
    a.row_mut(2).copy_from(&(point2.0 * proj2.row(2) - proj2.row(0)));
    a.row_mut(3).copy_from(&(point2.1 * proj2.row(2) - proj2.row(1)));

    // Solve using SVD
    let svd = na::SVD::new(a, true, true);
    let v = svd.v_t.context("SVD failed")?;
    let solution = v.row(3);

    // Homogeneous to 3D
    let w = solution[3];
    if w.abs() < 1e-10 {
        anyhow::bail!("Point at infinity");
    }

    Ok(na::Point3::new(
        solution[0] / w,
        solution[1] / w,
        solution[2] / w,
    ))
}

/// Convert OpenCV rotation Mat to nalgebra Matrix3
fn rotation_mat_to_nalgebra(mat: &Mat) -> Result<na::Matrix3<f64>> {
    if mat.rows() != 3 || mat.cols() != 3 {
        anyhow::bail!("Expected 3x3 rotation matrix");
    }

    let mut r = na::Matrix3::zeros();
    for i in 0..3 {
        for j in 0..3 {
            r[(i, j)] = *mat.at_2d::<f64>(i as i32, j as i32)?;
        }
    }

    Ok(r)
}

/// Convert OpenCV translation Mat to nalgebra Vector3
fn translation_mat_to_nalgebra(mat: &Mat) -> Result<na::Vector3<f64>> {
    if mat.rows() != 3 || mat.cols() != 1 {
        anyhow::bail!("Expected 3x1 translation vector");
    }

    Ok(na::Vector3::new(
        *mat.at_2d::<f64>(0, 0)?,
        *mat.at_2d::<f64>(1, 0)?,
        *mat.at_2d::<f64>(2, 0)?,
    ))
}

/// IMPROVED depth estimation from motion parallax with better scaling
/// Uses optical flow magnitude and direction to estimate relative depth
/// Key insight: Points on the object surface should have consistent depth
pub fn estimate_depth_from_motion(
    features1: &[Feature],
    features2: &[Feature],
    matches: &[(usize, usize)],
) -> Vec<f32> {
    let mut depths = Vec::new();

    if matches.is_empty() {
        return depths;
    }

    // Calculate optical flow for all matches
    let mut flows: Vec<(f32, f32, f32)> = Vec::new(); // (dx, dy, magnitude)
    for &(idx1, idx2) in matches {
        let (x1, y1) = features1[idx1].point;
        let (x2, y2) = features2[idx2].point;
        let dx = x2 - x1;
        let dy = y2 - y1;
        let mag = (dx * dx + dy * dy).sqrt();
        flows.push((dx, dy, mag));
    }

    // Find median flow magnitude (represents typical object motion)
    let mut magnitudes: Vec<f32> = flows.iter().map(|(_, _, m)| *m).collect();
    magnitudes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_flow = magnitudes[magnitudes.len() / 2].max(1.0);

    log::debug!("📏 Median optical flow: {:.2} pixels", median_flow);

    // Estimate depth for each point
    // Key principle: Inverse relationship between flow and depth
    // Closer points move more, farther points move less
    // Assume median flow corresponds to 0.4m depth (typical handheld distance)
    let base_depth = 0.4;
    for &(_dx, _dy, mag) in &flows {
        // Normalize flow by median
        let relative_flow = mag / median_flow;

        // Depth estimation:
        // - High flow (>1.0) = closer than median
        // - Low flow (<1.0) = farther than median
        let depth = if relative_flow > 0.1 {
            base_depth / relative_flow
        } else {
            // Very small flow = background or static point
            2.0
        };

        // Clamp to reasonable range for handheld objects
        depths.push(depth.clamp(0.15, 1.5));
    }

    // REFINEMENT: Smooth depths using local neighborhood
    // Points close in image space should have similar depths
    let smoothed_depths = smooth_depths(&depths, features2, matches);

    // Calc min and max for log
    let min_d = smoothed_depths.iter().fold(f32::INFINITY, |a, &b| a.min(b));
    let max_d = smoothed_depths.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
    log::debug!("📊 Depth range: {:.3}m to {:.3}m", min_d, max_d);

    smoothed_depths
}

/// Smooth depth estimates using spatial coherence
/// Points close in image space should have similar depths
fn smooth_depths(
    depths: &[f32],
    features: &[Feature],
    matches: &[(usize, usize)],
) -> Vec<f32> {
    let mut smoothed = depths.to_vec();
    let radius = 50.0; // pixels

    for i in 0..depths.len() {
        let (_, idx2) = matches[i];
        let (x, y) = features[idx2].point;

        // Find nearby points
        let mut nearby_depths = Vec::new();
        for j in 0..depths.len() {
            let (_, idx2_j) = matches[j];
            let (xj, yj) = features[idx2_j].point;
            let dist = ((x - xj).powi(2) + (y - yj).powi(2)).sqrt();

            if dist < radius {
                nearby_depths.push(depths[j]);
            }
        }

        // Use median of nearby depths
        if nearby_depths.len() >= 3 {
            nearby_depths.sort_by(|a, b| a.partial_cmp(b).unwrap());
            smoothed[i] = nearby_depths[nearby_depths.len() / 2];
        }
    }

    smoothed
}

/// Create 3D points from features using estimated depths
pub fn create_points_from_depths(
    features: &[Feature],
    depths: &[f32],
    intrinsics: &CameraIntrinsics,
    image: &ImageBuffer<Rgb<u8>, Vec<u8>>,
) -> Vec<ColoredPoint> {
    let mut points = Vec::new();

    for (i, feature) in features.iter().enumerate() {
        if i >= depths.len() {
            break;
        }

        let (x, y) = feature.point;
        let depth = depths[i];

        // Back-project to 3D
        let x_3d = ((x as f64 - intrinsics.cx) / intrinsics.fx * depth as f64) as f32;
        let y_3d = ((y as f64 - intrinsics.cy) / intrinsics.fy * depth as f64) as f32;
        let z_3d = depth;

        // Get color
        let px = x.clamp(0.0, (intrinsics.width - 1) as f32) as u32;
        let py = y.clamp(0.0, (intrinsics.height - 1) as f32) as u32;
        let pixel = image.get_pixel(px, py);

        points.push(ColoredPoint {
            position: na::Point3::new(x_3d, y_3d, z_3d),
            color: [pixel[0], pixel[1], pixel[2]],
            confidence: 0.6,
        });
    }

    points
}
