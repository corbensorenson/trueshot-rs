use anyhow::{Context, Result};
use kdtree::distance::squared_euclidean;
use kdtree::KdTree;
use nalgebra as na;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

const DEFAULT_SAMPLE_LIMIT: usize = 5000;
const DEFAULT_FSCORE_THRESHOLD: f64 = 0.01;
const DEFAULT_NORMAL_NEIGHBORS: usize = 16;
type PointKdTree = KdTree<f64, usize, [f64; 3]>;

#[derive(Debug, Clone)]
pub struct PointCloud {
    pub points: Vec<na::Point3<f64>>,
    pub normals: Option<Vec<na::Vector3<f64>>>,
}

#[derive(Debug, Clone)]
pub struct GeometryMetrics {
    pub chamfer: Option<f64>,
    pub hausdorff: Option<f64>,
    pub fscore: Option<f64>,
    pub precision: Option<f64>,
    pub recall: Option<f64>,
    pub normal_consistency: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct GeometryMetricsOptions {
    pub sample_limit: usize,
    pub fscore_threshold: f64,
    pub normal_k: usize,
}

impl Default for GeometryMetricsOptions {
    fn default() -> Self {
        Self {
            sample_limit: DEFAULT_SAMPLE_LIMIT,
            fscore_threshold: DEFAULT_FSCORE_THRESHOLD,
            normal_k: DEFAULT_NORMAL_NEIGHBORS,
        }
    }
}

pub fn load_point_cloud(path: &Path) -> Result<Vec<na::Point3<f64>>> {
    Ok(load_point_cloud_with_normals(path)?.points)
}

pub fn load_point_cloud_with_normals(path: &Path) -> Result<PointCloud> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "ply" => load_ply_ascii_cloud(path),
        "obj" => load_obj_cloud(path),
        _ => anyhow::bail!("Unsupported mesh format for Chamfer: {}", path.display()),
    }
}

pub fn chamfer_distance(points_a: &[na::Point3<f64>], points_b: &[na::Point3<f64>]) -> Option<f64> {
    if points_a.is_empty() || points_b.is_empty() {
        return None;
    }

    let sample_a = downsample(points_a, DEFAULT_SAMPLE_LIMIT);
    let sample_b = downsample(points_b, DEFAULT_SAMPLE_LIMIT);

    let (mean_a, _, _) = mean_min_distance_kdtree(&sample_a, &sample_b, DEFAULT_FSCORE_THRESHOLD)?;
    let (mean_b, _, _) = mean_min_distance_kdtree(&sample_b, &sample_a, DEFAULT_FSCORE_THRESHOLD)?;
    Some((mean_a + mean_b) * 0.5)
}

pub fn compute_geometry_metrics(
    pred: &PointCloud,
    gt: &PointCloud,
    options: &GeometryMetricsOptions,
) -> GeometryMetrics {
    if pred.points.is_empty() || gt.points.is_empty() {
        return GeometryMetrics {
            chamfer: None,
            hausdorff: None,
            fscore: None,
            precision: None,
            recall: None,
            normal_consistency: None,
        };
    }

    let pred_sample = sample_cloud(pred, options.sample_limit);
    let gt_sample = sample_cloud(gt, options.sample_limit);

    let (mean_pred, max_pred, within_pred) = mean_min_distance_kdtree(
        &pred_sample.points,
        &gt_sample.points,
        options.fscore_threshold,
    )
    .unwrap_or((0.0, 0.0, 0));
    let (mean_gt, max_gt, within_gt) = mean_min_distance_kdtree(
        &gt_sample.points,
        &pred_sample.points,
        options.fscore_threshold,
    )
    .unwrap_or((0.0, 0.0, 0));

    let chamfer = Some((mean_pred + mean_gt) * 0.5);
    let hausdorff = Some(max_pred.max(max_gt));

    let precision = if pred_sample.points.is_empty() {
        None
    } else {
        Some(within_pred as f64 / pred_sample.points.len() as f64)
    };
    let recall = if gt_sample.points.is_empty() {
        None
    } else {
        Some(within_gt as f64 / gt_sample.points.len() as f64)
    };
    let fscore = match (precision, recall) {
        (Some(p), Some(r)) if (p + r) > 0.0 => Some(2.0 * p * r / (p + r)),
        _ => None,
    };

    let normal_consistency =
        compute_normal_consistency(&pred_sample, &gt_sample, options.normal_k.max(3));

    GeometryMetrics {
        chamfer,
        hausdorff,
        fscore,
        precision,
        recall,
        normal_consistency,
    }
}

fn mean_min_distance_kdtree(
    src: &[na::Point3<f64>],
    dst: &[na::Point3<f64>],
    threshold: f64,
) -> Option<(f64, f64, usize)> {
    if src.is_empty() || dst.is_empty() {
        return None;
    }
    let tree = build_kdtree(dst);
    let mut sum = 0.0f64;
    let mut max = 0.0f64;
    let mut within = 0usize;
    for p in src {
        let dist = nearest_distance(&tree, p)?;
        if dist <= threshold {
            within += 1;
        }
        if dist > max {
            max = dist;
        }
        sum += dist;
    }
    Some((sum / src.len() as f64, max, within))
}

fn downsample(points: &[na::Point3<f64>], max_points: usize) -> Vec<na::Point3<f64>> {
    if points.len() <= max_points {
        return points.to_vec();
    }
    let step = (points.len() as f64 / max_points as f64).ceil() as usize;
    points.iter().step_by(step).cloned().collect()
}

fn sample_cloud(cloud: &PointCloud, max_points: usize) -> PointCloud {
    if cloud.points.len() <= max_points {
        return cloud.clone();
    }
    let step = (cloud.points.len() as f64 / max_points as f64).ceil() as usize;
    let points: Vec<na::Point3<f64>> = cloud.points.iter().step_by(step).cloned().collect();
    let normals = cloud
        .normals
        .as_ref()
        .map(|normals| normals.iter().step_by(step).cloned().collect::<Vec<_>>());
    PointCloud { points, normals }
}

fn load_ply_ascii_cloud(path: &Path) -> Result<PointCloud> {
    let file =
        File::open(path).with_context(|| format!("Failed to open PLY: {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut header = String::new();
    let mut vertex_count = 0usize;
    let mut is_ascii = false;
    let mut in_vertex = false;
    let mut properties: Vec<String> = Vec::new();

    loop {
        header.clear();
        if reader.read_line(&mut header)? == 0 {
            break;
        }
        let line = header.trim();
        if line.starts_with("format ") {
            if line.contains("ascii") {
                is_ascii = true;
            }
        } else if line.starts_with("element vertex ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let Some(count) = parts.last() {
                vertex_count = count.parse::<usize>().unwrap_or(0);
            }
            in_vertex = true;
        } else if line.starts_with("element ") {
            in_vertex = false;
        } else if in_vertex && line.starts_with("property ") {
            if let Some(name) = line.split_whitespace().last() {
                properties.push(name.to_string());
            }
        } else if line == "end_header" {
            break;
        }
    }

    if !is_ascii {
        anyhow::bail!("PLY must be ascii for Chamfer: {}", path.display());
    }
    if vertex_count == 0 {
        return Ok(PointCloud {
            points: Vec::new(),
            normals: None,
        });
    }

    let x_idx = properties.iter().position(|p| p == "x").unwrap_or(0);
    let y_idx = properties.iter().position(|p| p == "y").unwrap_or(1);
    let z_idx = properties.iter().position(|p| p == "z").unwrap_or(2);
    let nx_idx = properties.iter().position(|p| p == "nx");
    let ny_idx = properties.iter().position(|p| p == "ny");
    let nz_idx = properties.iter().position(|p| p == "nz");
    let normals_available = nx_idx.is_some() && ny_idx.is_some() && nz_idx.is_some();

    let mut points = Vec::with_capacity(vertex_count);
    let mut normals = if normals_available {
        Some(Vec::with_capacity(vertex_count))
    } else {
        None
    };
    let mut line = String::new();
    for _ in 0..vertex_count {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let x = parts
            .get(x_idx)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
        let y = parts
            .get(y_idx)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
        let z = parts
            .get(z_idx)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
        points.push(na::Point3::new(x, y, z));
        if let (Some(nx), Some(ny), Some(nz), Some(normals_vec)) =
            (nx_idx, ny_idx, nz_idx, normals.as_mut())
        {
            let nx = parts
                .get(nx)
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0);
            let ny = parts
                .get(ny)
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0);
            let nz = parts
                .get(nz)
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0);
            let mut normal = na::Vector3::new(nx, ny, nz);
            if normal.norm() > 0.0 {
                normal = normal.normalize();
            }
            normals_vec.push(normal);
        }
    }

    Ok(PointCloud { points, normals })
}

fn load_obj_cloud(path: &Path) -> Result<PointCloud> {
    let file =
        File::open(path).with_context(|| format!("Failed to open OBJ: {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut points = Vec::new();

    for line in reader.lines().flatten() {
        let line = line.trim();
        if line.starts_with("v ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 4 {
                continue;
            }
            let x = parts[1].parse::<f64>().unwrap_or(0.0);
            let y = parts[2].parse::<f64>().unwrap_or(0.0);
            let z = parts[3].parse::<f64>().unwrap_or(0.0);
            points.push(na::Point3::new(x, y, z));
        }
    }

    Ok(PointCloud {
        points,
        normals: None,
    })
}

fn build_kdtree(points: &[na::Point3<f64>]) -> PointKdTree {
    let mut tree = KdTree::new(3);
    for (idx, p) in points.iter().enumerate() {
        let _ = tree.add([p.x, p.y, p.z], idx);
    }
    tree
}

fn nearest_distance(tree: &PointKdTree, point: &na::Point3<f64>) -> Option<f64> {
    let query = [point.x, point.y, point.z];
    let nearest = tree.nearest(&query, 1, &squared_euclidean).ok()?;
    nearest.first().map(|(dist, _)| dist.sqrt())
}

fn nearest_index(tree: &PointKdTree, point: &na::Point3<f64>) -> Option<usize> {
    let query = [point.x, point.y, point.z];
    let nearest = tree.nearest(&query, 1, &squared_euclidean).ok()?;
    nearest.first().map(|(_, idx)| **idx)
}

fn compute_normal_consistency(pred: &PointCloud, gt: &PointCloud, normal_k: usize) -> Option<f64> {
    if pred.points.is_empty() || gt.points.is_empty() {
        return None;
    }
    let pred_normals = ensure_normals(pred, normal_k);
    let gt_normals = ensure_normals(gt, normal_k);
    let (pred_normals, gt_normals) = match (pred_normals, gt_normals) {
        (Some(p), Some(g)) => (p, g),
        _ => return None,
    };

    let gt_tree = build_kdtree(&gt.points);
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for (idx, point) in pred.points.iter().enumerate() {
        if let Some(gt_idx) = nearest_index(&gt_tree, point) {
            if let (Some(n_pred), Some(n_gt)) = (pred_normals.get(idx), gt_normals.get(gt_idx)) {
                let dot = n_pred.dot(n_gt).abs();
                sum += dot;
                count += 1;
            }
        }
    }
    if count == 0 {
        None
    } else {
        Some(sum / count as f64)
    }
}

fn ensure_normals(cloud: &PointCloud, normal_k: usize) -> Option<Vec<na::Vector3<f64>>> {
    if let Some(normals) = cloud.normals.as_ref() {
        if normals.len() == cloud.points.len() {
            return Some(normals.clone());
        }
    }
    Some(estimate_normals(&cloud.points, normal_k))
}

fn estimate_normals(points: &[na::Point3<f64>], normal_k: usize) -> Vec<na::Vector3<f64>> {
    if points.is_empty() {
        return Vec::new();
    }
    let k = normal_k.min(points.len()).max(3);
    let tree = build_kdtree(points);
    let mut normals = Vec::with_capacity(points.len());
    for point in points {
        let neighbors = tree
            .nearest(&[point.x, point.y, point.z], k, &squared_euclidean)
            .ok()
            .unwrap_or_default();
        if neighbors.len() < 3 {
            normals.push(na::Vector3::new(0.0, 0.0, 1.0));
            continue;
        }
        let mut centroid = na::Vector3::zeros();
        for (_, idx) in &neighbors {
            let p = &points[**idx];
            centroid.x += p.x;
            centroid.y += p.y;
            centroid.z += p.z;
        }
        centroid /= neighbors.len() as f64;
        let mut cov = na::Matrix3::zeros();
        for (_, idx) in &neighbors {
            let p = &points[**idx];
            let v = na::Vector3::new(p.x - centroid.x, p.y - centroid.y, p.z - centroid.z);
            cov += v * v.transpose();
        }
        let eig = na::SymmetricEigen::new(cov);
        let mut min_idx = 0usize;
        let mut min_val = eig.eigenvalues[0];
        for i in 1..3 {
            if eig.eigenvalues[i] < min_val {
                min_val = eig.eigenvalues[i];
                min_idx = i;
            }
        }
        let mut normal = eig.eigenvectors.column(min_idx).into_owned();
        if normal.norm() > 0.0 {
            normal = normal.normalize();
        } else {
            normal = na::Vector3::new(0.0, 0.0, 1.0);
        }
        normals.push(normal);
    }
    normals
}
