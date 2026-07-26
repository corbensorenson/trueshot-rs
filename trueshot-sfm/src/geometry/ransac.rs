//! RANSAC - Robust Estimation
//!
//! Random Sample Consensus for robust model fitting.
//! Used for fundamental/essential matrix estimation with outlier rejection.

use nalgebra as na;
use rand::prelude::*;

/// RANSAC configuration
#[derive(Clone, Debug)]
pub struct RansacConfig {
    /// Maximum iterations
    pub max_iterations: usize,
    /// Inlier threshold (pixels for reprojection, normalized for F/E)
    pub threshold: f64,
    /// Confidence level (0.99 = 99%)
    pub confidence: f64,
    /// Minimum inlier ratio to accept model
    pub min_inlier_ratio: f64,
}

impl Default for RansacConfig {
    fn default() -> Self {
        Self {
            max_iterations: 2000,
            threshold: 3.0,
            confidence: 0.99,
            min_inlier_ratio: 0.3,
        }
    }
}

/// RANSAC result
#[derive(Clone, Debug)]
pub struct RansacResult<T> {
    pub model: T,
    pub inliers: Vec<usize>,
    pub inlier_ratio: f64,
    pub iterations: usize,
}

/// Adaptive RANSAC iteration count based on inlier ratio
fn adaptive_iterations(inlier_ratio: f64, sample_size: usize, confidence: f64) -> usize {
    if inlier_ratio <= 0.0 || inlier_ratio >= 1.0 {
        return 1000;
    }
    
    let p_all_inliers = inlier_ratio.powi(sample_size as i32);
    let num = (1.0 - confidence).ln();
    let denom = (1.0 - p_all_inliers).ln();
    
    if denom.abs() < 1e-10 {
        return 1000;
    }
    
    (num / denom).ceil() as usize
}

/// RANSAC for Essential Matrix estimation (5-point algorithm)
pub fn ransac_essential(
    pts1: &[na::Point2<f64>],
    pts2: &[na::Point2<f64>],
    k1: &na::Matrix3<f64>,
    k2: &na::Matrix3<f64>,
    config: &RansacConfig,
) -> Option<RansacResult<na::Matrix3<f64>>> {
    let n = pts1.len().min(pts2.len());
    if n < 8 {
        return None;
    }
    
    // Normalize points
    let k1_inv = k1.try_inverse()?;
    let k2_inv = k2.try_inverse()?;
    
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
    
    let mut rng = rand::thread_rng();
    let mut best_result: Option<RansacResult<na::Matrix3<f64>>> = None;
    let mut max_iterations = config.max_iterations;
    
    let mut iter = 0usize;
    while iter < max_iterations {
        // Sample 8 points (using 8-point algorithm for simplicity)
        let indices: Vec<usize> = (0..n).choose_multiple(&mut rng, 8);
        
        let sample_pts1: Vec<na::Point2<f64>> = indices.iter().map(|&i| norm_pts1[i]).collect();
        let sample_pts2: Vec<na::Point2<f64>> = indices.iter().map(|&i| norm_pts2[i]).collect();
        
        // Estimate essential matrix
        let e = estimate_essential_normalized(&sample_pts1, &sample_pts2);
        
        // Count inliers using Sampson error
        let mut inliers = Vec::new();
        for i in 0..n {
            let err = sampson_error(&e, &norm_pts1[i], &norm_pts2[i]);
            if err < config.threshold {
                inliers.push(i);
            }
        }
        
        let inlier_ratio = inliers.len() as f64 / n as f64;
        
        if inlier_ratio > config.min_inlier_ratio
            && best_result.as_ref().map_or(true, |r| inliers.len() > r.inliers.len()) {
                best_result = Some(RansacResult {
                    model: e,
                    inliers: inliers.clone(),
                    inlier_ratio,
                    iterations: iter + 1,
                });
                
                // Adaptive iteration count
                max_iterations = adaptive_iterations(inlier_ratio, 8, config.confidence)
                    .min(config.max_iterations);
            }
        iter += 1;
    }
    
    // Refine with all inliers
    if let Some(mut result) = best_result {
        let inlier_pts1: Vec<na::Point2<f64>> = result.inliers.iter().map(|&i| norm_pts1[i]).collect();
        let inlier_pts2: Vec<na::Point2<f64>> = result.inliers.iter().map(|&i| norm_pts2[i]).collect();
        
        result.model = estimate_essential_normalized(&inlier_pts1, &inlier_pts2);
        return Some(result);
    }
    
    None
}

/// 8-point essential matrix estimation (normalized coordinates)
fn estimate_essential_normalized(pts1: &[na::Point2<f64>], pts2: &[na::Point2<f64>]) -> na::Matrix3<f64> {
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
    
    // Last column of V is the solution (use ncols-1 for dynamic access)
    let last_col = v.ncols() - 1;
    let e = v.column(last_col);
    let e = na::Matrix3::new(
        e[0], e[1], e[2],
        e[3], e[4], e[5],
        e[6], e[7], e[8],
    );
    
    // Enforce rank-2 constraint with equal singular values
    let svd_e = na::SVD::new(e, true, true);
    let mut s = svd_e.singular_values;
    let avg = (s[0] + s[1]) / 2.0;
    s[0] = avg;
    s[1] = avg;
    s[2] = 0.0;
    
    let u = svd_e.u.unwrap();
    let vt = svd_e.v_t.unwrap();
    
    u * na::Matrix3::from_diagonal(&s) * vt
}

/// Sampson error for essential matrix
fn sampson_error(e: &na::Matrix3<f64>, p1: &na::Point2<f64>, p2: &na::Point2<f64>) -> f64 {
    let x1 = na::Vector3::new(p1.x, p1.y, 1.0);
    let x2 = na::Vector3::new(p2.x, p2.y, 1.0);
    
    let ex1 = e * x1;
    let etx2 = e.transpose() * x2;
    
    let x2t_ex1 = x2.dot(&ex1);
    
    let denom = ex1[0].powi(2) + ex1[1].powi(2) + etx2[0].powi(2) + etx2[1].powi(2);
    
    if denom < 1e-10 {
        return f64::MAX;
    }
    
    (x2t_ex1.powi(2) / denom).sqrt()
}

/// RANSAC for Homography estimation
pub fn ransac_homography(
    pts1: &[na::Point2<f64>],
    pts2: &[na::Point2<f64>],
    config: &RansacConfig,
) -> Option<RansacResult<na::Matrix3<f64>>> {
    let n = pts1.len().min(pts2.len());
    if n < 4 {
        return None;
    }
    
    let mut rng = rand::thread_rng();
    let mut best_result: Option<RansacResult<na::Matrix3<f64>>> = None;
    let mut max_iterations = config.max_iterations;
    
    let mut iter = 0usize;
    while iter < max_iterations {
        // Sample 4 points
        let indices: Vec<usize> = (0..n).choose_multiple(&mut rng, 4);
        
        let sample_pts1: Vec<na::Point2<f64>> = indices.iter().map(|&i| pts1[i]).collect();
        let sample_pts2: Vec<na::Point2<f64>> = indices.iter().map(|&i| pts2[i]).collect();
        
        // Estimate homography using DLT
        if let Some(h) = estimate_homography_dlt(&sample_pts1, &sample_pts2) {
            // Count inliers
            let mut inliers = Vec::new();
            for i in 0..n {
                let p1h = na::Vector3::new(pts1[i].x, pts1[i].y, 1.0);
                let p2_pred = h * p1h;
                
                if p2_pred.z.abs() > 1e-10 {
                    let p2_proj = na::Point2::new(p2_pred.x / p2_pred.z, p2_pred.y / p2_pred.z);
                    let err = (p2_proj - pts2[i]).norm();
                    
                    if err < config.threshold {
                        inliers.push(i);
                    }
                }
            }
            
            let inlier_ratio = inliers.len() as f64 / n as f64;
            
            if inlier_ratio > config.min_inlier_ratio
                && best_result.as_ref().map_or(true, |r| inliers.len() > r.inliers.len()) {
                    best_result = Some(RansacResult {
                        model: h,
                        inliers,
                        inlier_ratio,
                        iterations: iter + 1,
                    });
                    
                    max_iterations = adaptive_iterations(inlier_ratio, 4, config.confidence)
                        .min(config.max_iterations);
                }
        }
        iter += 1;
    }
    
    best_result
}

/// DLT homography estimation
fn estimate_homography_dlt(pts1: &[na::Point2<f64>], pts2: &[na::Point2<f64>]) -> Option<na::Matrix3<f64>> {
    let n = pts1.len();
    if n < 4 {
        return None;
    }
    
    // Normalize points for numerical stability
    let (norm_pts1, t1) = normalize_points(pts1);
    let (norm_pts2, t2) = normalize_points(pts2);
    
    // Build DLT matrix
    let mut a = na::DMatrix::<f64>::zeros(2 * n, 9);
    
    for i in 0..n {
        let (x1, y1) = (norm_pts1[i].x, norm_pts1[i].y);
        let (x2, y2) = (norm_pts2[i].x, norm_pts2[i].y);
        
        a[(2*i, 0)] = -x1;
        a[(2*i, 1)] = -y1;
        a[(2*i, 2)] = -1.0;
        a[(2*i, 6)] = x2 * x1;
        a[(2*i, 7)] = x2 * y1;
        a[(2*i, 8)] = x2;
        
        a[(2*i+1, 3)] = -x1;
        a[(2*i+1, 4)] = -y1;
        a[(2*i+1, 5)] = -1.0;
        a[(2*i+1, 6)] = y2 * x1;
        a[(2*i+1, 7)] = y2 * y1;
        a[(2*i+1, 8)] = y2;
    }
    
    let svd = na::SVD::new(a, true, true);
    let v = svd.v_t?.transpose();
    let last_col = v.ncols() - 1;
    let h = v.column(last_col);
    
    let h_norm = na::Matrix3::new(
        h[0], h[1], h[2],
        h[3], h[4], h[5],
        h[6], h[7], h[8],
    );
    
    // Denormalize
    let t2_inv = t2.try_inverse()?;
    let h = t2_inv * h_norm * t1;
    
    // Normalize so H[2,2] = 1
    let scale = h[(2, 2)];
    if scale.abs() < 1e-10 {
        return None;
    }
    
    Some(h / scale)
}

/// Normalize 2D points for numerical stability
fn normalize_points(pts: &[na::Point2<f64>]) -> (Vec<na::Point2<f64>>, na::Matrix3<f64>) {
    let n = pts.len() as f64;
    
    // Centroid
    let cx: f64 = pts.iter().map(|p| p.x).sum::<f64>() / n;
    let cy: f64 = pts.iter().map(|p| p.y).sum::<f64>() / n;
    
    // Average distance from centroid
    let avg_dist: f64 = pts.iter()
        .map(|p| ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt())
        .sum::<f64>() / n;
    
    let scale = if avg_dist > 1e-10 { std::f64::consts::SQRT_2 / avg_dist } else { 1.0 };
    
    // Transformation matrix
    let t = na::Matrix3::new(
        scale, 0.0, -scale * cx,
        0.0, scale, -scale * cy,
        0.0, 0.0, 1.0,
    );
    
    let normalized: Vec<na::Point2<f64>> = pts.iter()
        .map(|p| na::Point2::new(scale * (p.x - cx), scale * (p.y - cy)))
        .collect();
    
    (normalized, t)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_normalize_points() {
        let pts = vec![
            na::Point2::new(100.0, 100.0),
            na::Point2::new(200.0, 100.0),
            na::Point2::new(100.0, 200.0),
            na::Point2::new(200.0, 200.0),
        ];
        
        let (norm, T) = normalize_points(&pts);
        
        // Check centroid is at origin
        let cx: f64 = norm.iter().map(|p| p.x).sum::<f64>() / 4.0;
        let cy: f64 = norm.iter().map(|p| p.y).sum::<f64>() / 4.0;
        assert!(cx.abs() < 1e-10);
        assert!(cy.abs() < 1e-10);
    }
}
