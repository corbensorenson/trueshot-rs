//! Native Geometry Module - TrueShot's Own Implementation
//!
//! Provides geometric algorithms for camera pose estimation
//! without OpenCV dependency.

pub mod bundle_adjustment;
pub mod essential;
pub mod magsac;
pub mod ransac;

use nalgebra as na;

/// 8-point algorithm for fundamental matrix estimation
/// Input: matched 2D points (at least 8 pairs)
/// Output: 3x3 fundamental matrix
pub fn estimate_fundamental_8point(
    points1: &[(f64, f64)],
    points2: &[(f64, f64)],
) -> Option<na::Matrix3<f64>> {
    if points1.len() < 8 || points2.len() < 8 || points1.len() != points2.len() {
        return None;
    }

    let n = points1.len();

    // Normalize points for numerical stability
    let (norm_pts1, t1) = normalize_points(points1);
    let (norm_pts2, t2) = normalize_points(points2);

    // Build coefficient matrix A (n x 9)
    let mut a = na::DMatrix::<f64>::zeros(n, 9);

    for i in 0..n {
        let (x1, y1) = norm_pts1[i];
        let (x2, y2) = norm_pts2[i];

        a[(i, 0)] = x2 * x1;
        a[(i, 1)] = x2 * y1;
        a[(i, 2)] = x2;
        a[(i, 3)] = y2 * x1;
        a[(i, 4)] = y2 * y1;
        a[(i, 5)] = y2;
        a[(i, 6)] = x1;
        a[(i, 7)] = y1;
        a[(i, 8)] = 1.0;
    }

    // Solve using SVD
    let svd = na::SVD::new(a, true, true);
    let v = svd.v_t?;

    // F is last row of V^T (corresponding to smallest singular value)
    let f_vec = v.row(8);

    // Reshape to 3x3
    let mut f = na::Matrix3::new(
        f_vec[0], f_vec[1], f_vec[2], f_vec[3], f_vec[4], f_vec[5], f_vec[6], f_vec[7], f_vec[8],
    );

    // Enforce rank-2 constraint
    let svd_f = na::SVD::new(f, true, true);
    let u = svd_f.u?;
    let v_t = svd_f.v_t?;
    let mut s = svd_f.singular_values;
    s[2] = 0.0; // Force smallest singular value to 0

    let s_mat = na::Matrix3::from_diagonal(&s);
    f = u * s_mat * v_t;

    // Denormalize: F = T2^T * F * T1
    f = t2.transpose() * f * t1;

    Some(f)
}

/// Normalize points to have centroid at origin and avg distance sqrt(2)
fn normalize_points(points: &[(f64, f64)]) -> (Vec<(f64, f64)>, na::Matrix3<f64>) {
    let n = points.len() as f64;

    // Compute centroid
    let (cx, cy) = points
        .iter()
        .fold((0.0, 0.0), |(sx, sy), (x, y)| (sx + x, sy + y));
    let cx = cx / n;
    let cy = cy / n;

    // Compute mean distance from centroid
    let mean_dist: f64 = points
        .iter()
        .map(|(x, y)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt())
        .sum::<f64>()
        / n;

    // Scale factor
    let scale = (2.0_f64).sqrt() / mean_dist.max(1e-10);

    // Normalization transform
    let t = na::Matrix3::new(
        scale,
        0.0,
        -scale * cx,
        0.0,
        scale,
        -scale * cy,
        0.0,
        0.0,
        1.0,
    );

    // Normalize points
    let normalized: Vec<(f64, f64)> = points
        .iter()
        .map(|(x, y)| (scale * (x - cx), scale * (y - cy)))
        .collect();

    (normalized, t)
}

/// Compute essential matrix from fundamental matrix and camera intrinsics
pub fn fundamental_to_essential(f: &na::Matrix3<f64>, k: &na::Matrix3<f64>) -> na::Matrix3<f64> {
    k.transpose() * f * k
}

/// Decompose essential matrix into rotation and translation
/// Returns up to 4 possible solutions
pub fn decompose_essential(e: &na::Matrix3<f64>) -> Vec<(na::Matrix3<f64>, na::Vector3<f64>)> {
    let svd = na::SVD::new(*e, true, true);

    let u = match svd.u {
        Some(u) => u,
        None => return Vec::new(),
    };

    let v_t = match svd.v_t {
        Some(v) => v,
        None => return Vec::new(),
    };

    // W matrix for rotation
    let w = na::Matrix3::new(0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0);

    // Two possible rotations
    let r1 = u * w * v_t;
    let r2 = u * w.transpose() * v_t;

    // Fix rotation matrices to be proper (det = 1)
    let r1 = if r1.determinant() < 0.0 { -r1 } else { r1 };
    let r2 = if r2.determinant() < 0.0 { -r2 } else { r2 };

    // Translation is last column of U
    let t = u.column(2).into_owned();

    // Four possible solutions
    vec![(r1, t), (r1, -t), (r2, t), (r2, -t)]
}

/// Select the correct pose from 4 candidates using cheirality check
/// (points should be in front of both cameras)
pub fn select_correct_pose(
    solutions: &[(na::Matrix3<f64>, na::Vector3<f64>)],
    points1: &[(f64, f64)],
    points2: &[(f64, f64)],
    k: &na::Matrix3<f64>,
) -> Option<(na::Matrix3<f64>, na::Vector3<f64>)> {
    let mut best_solution = None;
    let mut max_infront = 0;

    for (r, t) in solutions {
        let mut infront_count = 0;

        // Check a sample of points
        let sample_size = points1.len().min(10);
        for i in 0..sample_size {
            if is_point_in_front(points1[i], points2[i], r, t, k) {
                infront_count += 1;
            }
        }

        if infront_count > max_infront {
            max_infront = infront_count;
            best_solution = Some((*r, *t));
        }
    }

    best_solution
}

/// Check if a 3D point is in front of both cameras
fn is_point_in_front(
    pt1: (f64, f64),
    pt2: (f64, f64),
    r: &na::Matrix3<f64>,
    t: &na::Vector3<f64>,
    k: &na::Matrix3<f64>,
) -> bool {
    let k_inv = k.try_inverse().unwrap_or(na::Matrix3::identity());
    let x1 = k_inv * na::Vector3::new(pt1.0, pt1.1, 1.0);
    let x2 = k_inv * na::Vector3::new(pt2.0, pt2.1, 1.0);

    let p1 = na::Matrix3x4::new(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0);
    let p2 = na::Matrix3x4::new(
        r[(0, 0)],
        r[(0, 1)],
        r[(0, 2)],
        t.x,
        r[(1, 0)],
        r[(1, 1)],
        r[(1, 2)],
        t.y,
        r[(2, 0)],
        r[(2, 1)],
        r[(2, 2)],
        t.z,
    );

    let mut a = na::Matrix4::zeros();
    a.row_mut(0).copy_from(&(x1.x * p1.row(2) - p1.row(0)));
    a.row_mut(1).copy_from(&(x1.y * p1.row(2) - p1.row(1)));
    a.row_mut(2).copy_from(&(x2.x * p2.row(2) - p2.row(0)));
    a.row_mut(3).copy_from(&(x2.y * p2.row(2) - p2.row(1)));

    let svd = na::SVD::new(a, true, true);
    let v_t = match svd.v_t {
        Some(v) => v,
        None => return false,
    };
    let x = v_t.row(3).transpose();
    if x[3].abs() < 1e-9 {
        return false;
    }
    let point = na::Vector3::new(x[0] / x[3], x[1] / x[3], x[2] / x[3]);
    let z1 = point.z;
    let z2 = (r * point + t).z;

    z1 > 0.0 && z2 > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalization() {
        let points = vec![
            (100.0, 100.0),
            (200.0, 100.0),
            (100.0, 200.0),
            (200.0, 200.0),
        ];

        let (normalized, _t) = normalize_points(&points);

        // Centroid should be near origin
        let (cx, cy) = normalized
            .iter()
            .fold((0.0, 0.0), |(sx, sy), (x, y)| (sx + x, sy + y));
        let cx = cx / 4.0;
        let cy = cy / 4.0;

        assert!(cx.abs() < 1e-10);
        assert!(cy.abs() < 1e-10);
    }

    #[test]
    fn test_decompose_essential() {
        // Create a simple essential matrix
        let r = na::Matrix3::identity();
        let t = na::Vector3::new(1.0, 0.0, 0.0);

        // E = [t]x * R
        let t_cross = na::Matrix3::new(0.0, -t.z, t.y, t.z, 0.0, -t.x, -t.y, t.x, 0.0);
        let e = t_cross * r;

        let solutions = decompose_essential(&e);

        // Should get 4 solutions
        assert_eq!(solutions.len(), 4);
    }
}
