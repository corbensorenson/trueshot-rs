//! Geometry Module
//!
//! Feature matching, essential matrix estimation, and triangulation.

pub mod ransac;

pub use ransac::{RansacConfig, RansacResult, ransac_essential, ransac_homography};

use crate::{ImageData, CameraPose, Point3D};
use crate::distortion::undistort_normalized;
use crate::features::Descriptor;
use nalgebra as na;
use rayon::prelude::*;

// ============================================================================
// Types
// ============================================================================

/// Feature match between two images
#[derive(Clone, Debug)]
pub struct FeatureMatch {
    pub image1_id: usize,
    pub image2_id: usize,
    pub keypoint1_idx: usize,
    pub keypoint2_idx: usize,
    pub distance: f32,
}

/// Image pair with matches
#[derive(Clone, Debug)]
pub struct ImagePair {
    pub image1_id: usize,
    pub image2_id: usize,
    pub matches: Vec<FeatureMatch>,
    pub essential: Option<na::Matrix3<f64>>,
    pub relative_pose: Option<CameraPose>,
}

// ============================================================================
// Feature Matching
// ============================================================================

/// Match features between all image pairs
pub fn match_all_pairs(
    images: &[ImageData],
    ratio_threshold: f32,
    min_matches: usize,
) -> Vec<ImagePair> {
    let n = images.len();
    let mut pairs = Vec::new();
    
    // Generate all pairs
    let pair_indices: Vec<(usize, usize)> = (0..n)
        .flat_map(|i| ((i + 1)..n).map(move |j| (i, j)))
        .collect();
    
    // Match in parallel
    let matched_pairs: Vec<Option<ImagePair>> = pair_indices.par_iter()
        .map(|&(i, j)| {
            let matches = match_features(
                &images[i].descriptors,
                &images[j].descriptors,
                ratio_threshold,
            );
            
            if matches.len() >= min_matches {
                Some(ImagePair {
                    image1_id: i,
                    image2_id: j,
                    matches: matches.into_iter().map(|(k1, k2, d)| FeatureMatch {
                        image1_id: i,
                        image2_id: j,
                        keypoint1_idx: k1,
                        keypoint2_idx: k2,
                        distance: d,
                    }).collect(),
                    essential: None,
                    relative_pose: None,
                })
            } else {
                None
            }
        })
        .collect();
    
    pairs.extend(matched_pairs.into_iter().flatten());
    pairs
}

/// Match features using ratio test
fn match_features(
    desc1: &[Descriptor],
    desc2: &[Descriptor],
    ratio_threshold: f32,
) -> Vec<(usize, usize, f32)> {
    let mut matches = Vec::new();
    
    for (i, d1) in desc1.iter().enumerate() {
        let mut best_dist = f32::MAX;
        let mut second_dist = f32::MAX;
        let mut best_idx = 0;
        
        for (j, d2) in desc2.iter().enumerate() {
            let dist = if d1.data.len() == 32 {
                d1.hamming_distance(d2) as f32
            } else {
                d1.l2_distance(d2)
            };
            
            if dist < best_dist {
                second_dist = best_dist;
                best_dist = dist;
                best_idx = j;
            } else if dist < second_dist {
                second_dist = dist;
            }
        }
        
        // Lowe's ratio test
        if best_dist < ratio_threshold * second_dist {
            matches.push((i, best_idx, best_dist));
        }
    }
    
    matches
}

// ============================================================================
// Pose Estimation
// ============================================================================

/// Estimate camera poses from matches
pub fn estimate_poses(
    images: &[ImageData],
    pairs: &[ImagePair],
) -> anyhow::Result<Vec<CameraPose>> {
    let n = images.len();
    let mut poses = vec![CameraPose::identity(); n];
    let mut registered = vec![false; n];
    
    if pairs.is_empty() {
        anyhow::bail!("No valid image pairs found");
    }
    
    // Start with first pair
    let first_pair = &pairs[0];
    registered[first_pair.image1_id] = true;
    
    // Estimate essential matrix for first pair
    let pts1: Vec<na::Point2<f64>> = first_pair.matches.iter()
        .map(|m| {
            let kp = &images[first_pair.image1_id].keypoints[m.keypoint1_idx];
            na::Point2::new(kp.x as f64, kp.y as f64)
        })
        .collect();
    
    let pts2: Vec<na::Point2<f64>> = first_pair.matches.iter()
        .map(|m| {
            let kp = &images[first_pair.image2_id].keypoints[m.keypoint2_idx];
            na::Point2::new(kp.x as f64, kp.y as f64)
        })
        .collect();
    
    let k1 = images[first_pair.image1_id].intrinsics.to_matrix();
    let k2 = images[first_pair.image2_id].intrinsics.to_matrix();
    
    // Normalize points
    let k1_inv = k1.try_inverse().unwrap_or(na::Matrix3::identity());
    let k2_inv = k2.try_inverse().unwrap_or(na::Matrix3::identity());
    
    let norm_pts1: Vec<na::Point2<f64>> = pts1.iter()
        .map(|p| {
            let h = k1_inv * na::Vector3::new(p.x, p.y, 1.0);
            na::Point2::new(h.x / h.z, h.y / h.z)
        })
        .collect();
    
    let norm_pts2: Vec<na::Point2<f64>> = pts2.iter()
        .map(|p| {
            let h = k2_inv * na::Vector3::new(p.x, p.y, 1.0);
            na::Point2::new(h.x / h.z, h.y / h.z)
        })
        .collect();
    
    // Estimate essential matrix using RANSAC + 8-point
    let e = estimate_essential_ransac(&norm_pts1, &norm_pts2);
    
    // Decompose essential matrix
    let (r, t) = decompose_essential(&e, &norm_pts1, &norm_pts2);
    
    let rotation_wc = na::UnitQuaternion::from_rotation_matrix(&r);
    let rotation_cw = rotation_wc.inverse();
    let translation_cw = -(rotation_cw * t);
    poses[first_pair.image2_id] = CameraPose {
        rotation: rotation_cw,
        translation: translation_cw,
    };
    registered[first_pair.image2_id] = true;
    
    // Register remaining images using PnP
    for pair in pairs.iter().skip(1) {
        if registered[pair.image1_id] && !registered[pair.image2_id] {
            // Use pair.image1 as reference
            if let Some(pose) = estimate_pose_pnp(images, pair, false, &poses) {
                poses[pair.image2_id] = pose;
                registered[pair.image2_id] = true;
            }
        } else if !registered[pair.image1_id] && registered[pair.image2_id] {
            if let Some(pose) = estimate_pose_pnp(images, pair, true, &poses) {
                poses[pair.image1_id] = pose;
                registered[pair.image1_id] = true;
            }
        }
    }
    
    tracing::info!("Registered {}/{} cameras", registered.iter().filter(|&&x| x).count(), n);
    
    Ok(poses)
}

/// 8-point algorithm for essential matrix
fn estimate_essential_8point(pts1: &[na::Point2<f64>], pts2: &[na::Point2<f64>]) -> na::Matrix3<f64> {
    let n = pts1.len().min(pts2.len());
    
    // Build constraint matrix
    let mut a = na::DMatrix::<f64>::zeros(n, 9);
    
    for i in 0..n {
        let (x1, y1) = (pts1[i].x, pts1[i].y);
        let (x2, y2) = (pts2[i].x, pts2[i].y);
        
        a[(i, 0)] = x1 * x2;
        a[(i, 1)] = x1 * y2;
        a[(i, 2)] = x1;
        a[(i, 3)] = y1 * x2;
        a[(i, 4)] = y1 * y2;
        a[(i, 5)] = y1;
        a[(i, 6)] = x2;
        a[(i, 7)] = y2;
        a[(i, 8)] = 1.0;
    }
    
    // SVD
    let svd = na::SVD::new(a, true, true);
    let v = svd.v_t.unwrap().transpose();
    
    // Last column of V is the solution
    let e = v.column(8);
    let e = na::Matrix3::new(
        e[0], e[1], e[2],
        e[3], e[4], e[5],
        e[6], e[7], e[8],
    );
    
    // Enforce rank-2 constraint
    let svd_e = na::SVD::new(e, true, true);
    let mut s = svd_e.singular_values;
    s[2] = 0.0;
    let avg = (s[0] + s[1]) / 2.0;
    s[0] = avg;
    s[1] = avg;
    
    let u = svd_e.u.unwrap();
    let vt = svd_e.v_t.unwrap();
    
    u * na::Matrix3::from_diagonal(&s) * vt
}

/// Decompose essential matrix into R, t
fn decompose_essential(
    e: &na::Matrix3<f64>,
    pts1: &[na::Point2<f64>],
    pts2: &[na::Point2<f64>],
) -> (na::Rotation3<f64>, na::Vector3<f64>) {
    let svd = na::SVD::new(*e, true, true);
    let u = svd.u.unwrap();
    let vt = svd.v_t.unwrap();
    
    let w = na::Matrix3::new(
        0.0, -1.0, 0.0,
        1.0, 0.0, 0.0,
        0.0, 0.0, 1.0,
    );
    
    // Four possible solutions
    let r1 = u * w * vt;
    let r2 = u * w.transpose() * vt;
    let t = u.column(2).into_owned();
    
    // Choose solution with most points in front of both cameras
    let solutions = [
        (r1, t),
        (r1, -t),
        (r2, t),
        (r2, -t),
    ];
    
    let mut best_solution = (na::Rotation3::identity(), na::Vector3::zeros());
    let mut best_count = 0;
    
    for (r, t) in solutions {
        let r = na::Rotation3::from_matrix_unchecked(r);
        let mut count = 0;
        
        for i in 0..pts1.len().min(pts2.len()) {
            let p1 = na::Vector3::new(pts1[i].x, pts1[i].y, 1.0);
            let p2 = na::Vector3::new(pts2[i].x, pts2[i].y, 1.0);
            
            // Simple triangulation check
            if check_cheirality(&r, &t, &p1, &p2) {
                count += 1;
            }
        }
        
        if count > best_count {
            best_count = count;
            best_solution = (r, t);
        }
    }
    
    best_solution
}

fn check_cheirality(
    r: &na::Rotation3<f64>,
    t: &na::Vector3<f64>,
    p1: &na::Vector3<f64>,
    p2: &na::Vector3<f64>,
) -> bool {
    // Simplified cheirality check
    let p2_cam1 = r.inverse() * (p2 - t);
    p1.z > 0.0 && p2_cam1.z > 0.0
}

fn estimate_pose_pnp(
    images: &[ImageData],
    pair: &ImagePair,
    reverse: bool,
    poses: &[CameraPose],
) -> Option<CameraPose> {
    let (ref_id, new_id) = if reverse {
        (pair.image2_id, pair.image1_id)
    } else {
        (pair.image1_id, pair.image2_id)
    };
    if pair.matches.len() < 8 {
        return None;
    }

    let k_ref = images[ref_id].intrinsics.to_matrix();
    let k_new = images[new_id].intrinsics.to_matrix();
    let k_ref_inv = k_ref.try_inverse().unwrap_or(na::Matrix3::identity());
    let k_new_inv = k_new.try_inverse().unwrap_or(na::Matrix3::identity());

    let mut pts_ref = Vec::with_capacity(pair.matches.len());
    let mut pts_new = Vec::with_capacity(pair.matches.len());
    for m in &pair.matches {
        let (kp_ref, kp_new) = if reverse {
            (
                &images[pair.image2_id].keypoints[m.keypoint2_idx],
                &images[pair.image1_id].keypoints[m.keypoint1_idx],
            )
        } else {
            (
                &images[pair.image1_id].keypoints[m.keypoint1_idx],
                &images[pair.image2_id].keypoints[m.keypoint2_idx],
            )
        };
        let h_ref = k_ref_inv * na::Vector3::new(kp_ref.x as f64, kp_ref.y as f64, 1.0);
        let h_new = k_new_inv * na::Vector3::new(kp_new.x as f64, kp_new.y as f64, 1.0);
        let (x_ref, y_ref) = undistort_normalized(
            images[ref_id].intrinsics.distortion_model,
            &images[ref_id].intrinsics.distortion,
            h_ref.x / h_ref.z,
            h_ref.y / h_ref.z,
        );
        let (x_new, y_new) = undistort_normalized(
            images[new_id].intrinsics.distortion_model,
            &images[new_id].intrinsics.distortion,
            h_new.x / h_new.z,
            h_new.y / h_new.z,
        );
        pts_ref.push(na::Point2::new(x_ref, y_ref));
        pts_new.push(na::Point2::new(x_new, y_new));
    }

    let e = estimate_essential_ransac(&pts_ref, &pts_new);
    let (r, t) = decompose_essential(&e, &pts_ref, &pts_new);

    let rotation_rel = na::UnitQuaternion::from_rotation_matrix(&r).inverse();
    let translation_rel = -(rotation_rel * t);

    let ref_pose = &poses[ref_id];
    let rotation_world = ref_pose.rotation * rotation_rel;
    let translation_world = ref_pose.rotation * translation_rel + ref_pose.translation;

    Some(CameraPose {
        rotation: rotation_world,
        translation: translation_world,
    })
}

fn estimate_essential_ransac(pts1: &[na::Point2<f64>], pts2: &[na::Point2<f64>]) -> na::Matrix3<f64> {
    let n = pts1.len().min(pts2.len());
    if n < 8 {
        return estimate_essential_8point(pts1, pts2);
    }

    let mut rng = rand::thread_rng();
    let max_iters = (2000usize).min(n * 200);
    let threshold = 1e-3;

    let mut best_inliers: Vec<usize> = Vec::new();
    let mut best_e = estimate_essential_8point(pts1, pts2);

    for _ in 0..max_iters {
        let sample = rand::seq::index::sample(&mut rng, n, 8);
        let mut s1 = Vec::with_capacity(8);
        let mut s2 = Vec::with_capacity(8);
        for idx in sample.iter() {
            s1.push(pts1[idx]);
            s2.push(pts2[idx]);
        }
        let e = estimate_essential_8point(&s1, &s2);
        let mut inliers = Vec::new();
        for i in 0..n {
            if sampson_error(&e, &pts1[i], &pts2[i]) < threshold {
                inliers.push(i);
            }
        }
        if inliers.len() > best_inliers.len() {
            best_inliers = inliers;
            best_e = e;
            if best_inliers.len() > (n as f64 * 0.85) as usize {
                break;
            }
        }
    }

    if best_inliers.len() >= 8 {
        let mut in1 = Vec::with_capacity(best_inliers.len());
        let mut in2 = Vec::with_capacity(best_inliers.len());
        for &idx in &best_inliers {
            in1.push(pts1[idx]);
            in2.push(pts2[idx]);
        }
        estimate_essential_8point(&in1, &in2)
    } else {
        best_e
    }
}

fn sampson_error(e: &na::Matrix3<f64>, p1: &na::Point2<f64>, p2: &na::Point2<f64>) -> f64 {
    let x1 = na::Vector3::new(p1.x, p1.y, 1.0);
    let x2 = na::Vector3::new(p2.x, p2.y, 1.0);
    let ex1 = e * x1;
    let etx2 = e.transpose() * x2;
    let denom = ex1.x.powi(2) + ex1.y.powi(2) + etx2.x.powi(2) + etx2.y.powi(2);
    let num = x2.dot(&ex1);
    if denom.abs() < 1e-12 {
        return f64::MAX;
    }
    (num * num) / denom
}

// ============================================================================
// Triangulation
// ============================================================================

/// Triangulate 3D points from matches
pub fn triangulate_points(
    images: &[ImageData],
    pairs: &[ImagePair],
    poses: &[CameraPose],
) -> anyhow::Result<Vec<Point3D>> {
    let mut points = Vec::new();
    
    for pair in pairs {
        let p1 = compute_projection_matrix(&images[pair.image1_id].intrinsics, &poses[pair.image1_id]);
        let p2 = compute_projection_matrix(&images[pair.image2_id].intrinsics, &poses[pair.image2_id]);
        
        for m in &pair.matches {
            let kp1 = &images[pair.image1_id].keypoints[m.keypoint1_idx];
            let kp2 = &images[pair.image2_id].keypoints[m.keypoint2_idx];
            
            let pt1 = na::Point2::new(kp1.x as f64, kp1.y as f64);
            let pt2 = na::Point2::new(kp2.x as f64, kp2.y as f64);
            
            if let Some(pt3d) = triangulate_point(&p1, &p2, &pt1, &pt2) {
                // Get color from image (simplified - use center of keypoint)
                let color = [128u8, 128, 128]; // Placeholder
                
                points.push(Point3D {
                    position: pt3d,
                    color,
                    error: m.distance as f64,
                    track: vec![
                        (pair.image1_id, m.keypoint1_idx),
                        (pair.image2_id, m.keypoint2_idx),
                    ],
                });
            }
        }
    }
    
    Ok(points)
}

fn compute_projection_matrix(
    intrinsics: &crate::CameraIntrinsics,
    pose: &CameraPose,
) -> na::Matrix3x4<f64> {
    let k = intrinsics.to_matrix();
    let r = pose.world_to_camera_rotation();
    let t = pose.world_to_camera_translation();
    
    let mut rt = na::Matrix3x4::zeros();
    rt.fixed_view_mut::<3, 3>(0, 0).copy_from(r.matrix());
    rt.fixed_view_mut::<3, 1>(0, 3).copy_from(&t);
    
    k * rt
}

fn triangulate_point(
    p1: &na::Matrix3x4<f64>,
    p2: &na::Matrix3x4<f64>,
    pt1: &na::Point2<f64>,
    pt2: &na::Point2<f64>,
) -> Option<na::Point3<f64>> {
    // DLT triangulation
    let mut a = na::Matrix4::<f64>::zeros();
    
    a.row_mut(0).copy_from(&(pt1.x * p1.row(2) - p1.row(0)));
    a.row_mut(1).copy_from(&(pt1.y * p1.row(2) - p1.row(1)));
    a.row_mut(2).copy_from(&(pt2.x * p2.row(2) - p2.row(0)));
    a.row_mut(3).copy_from(&(pt2.y * p2.row(2) - p2.row(1)));
    
    let svd = na::SVD::new(a, true, true);
    let v = svd.v_t?.transpose();
    
    let x = v.column(3);
    
    if x[3].abs() < 1e-10 {
        return None;
    }
    
    Some(na::Point3::new(x[0] / x[3], x[1] / x[3], x[2] / x[3]))
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_essential_matrix() {
        // Create synthetic correspondences (need >= 9 points for 8-point algorithm)
        let pts1: Vec<na::Point2<f64>> = (0..15)
            .map(|i| {
                let x = (i % 5) as f64 * 0.2;
                let y = (i / 5) as f64 * 0.3;
                na::Point2::new(x, y)
            })
            .collect();
        
        let pts2: Vec<na::Point2<f64>> = pts1.iter()
            .map(|p| na::Point2::new(p.x + 0.1, p.y + 0.05))
            .collect();
        
        let e = estimate_essential_8point(&pts1, &pts2);
        
        // E should be 3x3
        assert_eq!(e.nrows(), 3);
        assert_eq!(e.ncols(), 3);
    }
}
