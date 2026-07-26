//! Essential Matrix Estimation
//!
//! 5-point and 8-point algorithms for essential matrix computation.

use nalgebra as na;

/// Compute essential matrix directly from calibrated points
/// Input: normalized (calibrated) point correspondences
pub fn estimate_essential_5point(
    _points1: &[(f64, f64)],
    _points2: &[(f64, f64)],
) -> Vec<na::Matrix3<f64>> {
    // 5-point algorithm is complex (Nister, 2004)
    // For now, fall back to 8-point with more data
    // Full implementation requires solving a 10th degree polynomial

    // Placeholder: return empty for now, use 8-point instead
    Vec::new()
}

/// Compute essential matrix from calibrated correspondences using 8-point
pub fn estimate_essential_8point(
    points1: &[(f64, f64)],
    points2: &[(f64, f64)],
    k: &na::Matrix3<f64>,
) -> Option<na::Matrix3<f64>> {
    // Convert to image coordinates for fundamental matrix
    let pts1_img: Vec<(f64, f64)> = points1
        .iter()
        .map(|(x, y)| {
            let p = k * na::Vector3::new(*x, *y, 1.0);
            (p.x / p.z, p.y / p.z)
        })
        .collect();

    let pts2_img: Vec<(f64, f64)> = points2
        .iter()
        .map(|(x, y)| {
            let p = k * na::Vector3::new(*x, *y, 1.0);
            (p.x / p.z, p.y / p.z)
        })
        .collect();

    // Estimate fundamental matrix
    let f = super::estimate_fundamental_8point(&pts1_img, &pts2_img)?;

    // Convert to essential: E = K^T * F * K
    Some(super::fundamental_to_essential(&f, k))
}

/// Triangulate a 3D point from two views
pub fn triangulate_point(
    pt1: (f64, f64),
    pt2: (f64, f64),
    p1: &na::Matrix3x4<f64>,
    p2: &na::Matrix3x4<f64>,
) -> Option<na::Point3<f64>> {
    // DLT triangulation
    let mut a = na::Matrix4::<f64>::zeros();

    a.row_mut(0).copy_from(&(pt1.0 * p1.row(2) - p1.row(0)));
    a.row_mut(1).copy_from(&(pt1.1 * p1.row(2) - p1.row(1)));
    a.row_mut(2).copy_from(&(pt2.0 * p2.row(2) - p2.row(0)));
    a.row_mut(3).copy_from(&(pt2.1 * p2.row(2) - p2.row(1)));

    // SVD
    let svd = na::SVD::new(a, true, true);
    let v_t = svd.v_t?;
    let solution = v_t.row(3);

    let w = solution[3];
    if w.abs() < 1e-10 {
        return None;
    }

    Some(na::Point3::new(
        solution[0] / w,
        solution[1] / w,
        solution[2] / w,
    ))
}

/// Create a projection matrix from world-to-camera rotation and translation.
pub fn projection_matrix(
    k: &na::Matrix3<f64>,
    r: &na::Matrix3<f64>,
    t: &na::Vector3<f64>,
) -> na::Matrix3x4<f64> {
    let mut rt = na::Matrix3x4::zeros();
    rt.fixed_view_mut::<3, 3>(0, 0).copy_from(r);
    rt.column_mut(3).copy_from(t);

    k * rt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_triangulation() {
        // Simple test case
        let k = na::Matrix3::new(500.0, 0.0, 320.0, 0.0, 500.0, 240.0, 0.0, 0.0, 1.0);

        let r = na::Matrix3::identity();
        let t1 = na::Vector3::zeros();
        // A camera center 10 cm to the right has world-to-camera t = -R * C.
        let t2 = na::Vector3::new(-0.1, 0.0, 0.0);

        let p1 = projection_matrix(&k, &r, &t1);
        let p2 = projection_matrix(&k, &r, &t2);

        // A point at (0, 0, 1) projects leftward in the second camera.
        let pt1 = (320.0, 240.0);
        let pt2 = (270.0, 240.0);

        let result = triangulate_point(pt1, pt2, &p1, &p2);
        assert!(result.is_some());

        let point = result.unwrap();
        assert!(point.z > 0.0, "Point should be in front of camera");
        assert!((point - na::Point3::new(0.0, 0.0, 1.0)).norm() < 1e-9);
    }
}
