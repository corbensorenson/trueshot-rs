use crate::reconstruction::{ColoredPoint, Mesh};
use nalgebra as na;
use std::collections::HashMap;

/// Statistical outlier removal parameters
#[derive(Debug, Clone)]
pub struct OutlierRemovalParams {
    pub num_neighbors: usize,    // Number of neighbors to consider
    pub std_dev_multiplier: f32, // Standard deviation multiplier for threshold
}

impl Default for OutlierRemovalParams {
    fn default() -> Self {
        Self {
            num_neighbors: 20,
            std_dev_multiplier: 2.0,
        }
    }
}

/// Radius outlier removal parameters
#[derive(Debug, Clone)]
pub struct RadiusOutlierParams {
    pub radius: f32,          // Search radius
    pub min_neighbors: usize, // Minimum neighbors required
}

impl Default for RadiusOutlierParams {
    fn default() -> Self {
        Self {
            radius: 0.05, // 5cm
            min_neighbors: 5,
        }
    }
}

/// Mesh decimation parameters
#[derive(Debug, Clone)]
pub struct DecimationParams {
    pub target_reduction: f32, // 0-1, percentage to reduce
    pub preserve_boundaries: bool,
    pub preserve_topology: bool,
}

impl Default for DecimationParams {
    fn default() -> Self {
        Self {
            target_reduction: 0.5, // Reduce by 50%
            preserve_boundaries: true,
            preserve_topology: true,
        }
    }
}

/// Statistical outlier removal (SOR)
/// Removes points that are far from their neighbors
pub fn remove_statistical_outliers(
    points: &[ColoredPoint],
    params: &OutlierRemovalParams,
) -> Vec<ColoredPoint> {
    if points.len() < params.num_neighbors {
        return points.to_vec();
    }

    tracing::info!(
        "Removing statistical outliers from {} points...",
        points.len()
    );

    // Compute mean distance to k-nearest neighbors for each point
    let mut mean_distances = Vec::with_capacity(points.len());

    for (i, point) in points.iter().enumerate() {
        // Find k-nearest neighbors
        let mut distances: Vec<f32> = points
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, other)| na::distance(&point.position, &other.position))
            .collect();

        // Sort and take k nearest
        distances.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let k_nearest = &distances[..params.num_neighbors.min(distances.len())];

        // Compute mean distance
        let mean_dist = k_nearest.iter().sum::<f32>() / k_nearest.len() as f32;
        mean_distances.push(mean_dist);
    }

    // Compute global mean and standard deviation
    let global_mean = mean_distances.iter().sum::<f32>() / mean_distances.len() as f32;
    let variance = mean_distances
        .iter()
        .map(|d| (d - global_mean).powi(2))
        .sum::<f32>()
        / mean_distances.len() as f32;
    let std_dev = variance.sqrt();

    // Threshold for outlier detection
    let threshold = global_mean + params.std_dev_multiplier * std_dev;

    // Filter points
    let filtered: Vec<ColoredPoint> = points
        .iter()
        .zip(mean_distances.iter())
        .filter(|(_, &mean_dist)| mean_dist < threshold)
        .map(|(point, _)| *point)
        .collect();

    let removed = points.len() - filtered.len();
    tracing::info!(
        "Removed {} outliers ({:.1}%), kept {} points",
        removed,
        (removed as f32 / points.len() as f32) * 100.0,
        filtered.len()
    );

    filtered
}

/// Radius outlier removal (ROR)
/// Removes points with too few neighbors within a radius
pub fn remove_radius_outliers(
    points: &[ColoredPoint],
    params: &RadiusOutlierParams,
) -> Vec<ColoredPoint> {
    if points.is_empty() {
        return Vec::new();
    }

    tracing::info!("Removing radius outliers from {} points...", points.len());

    let radius_sq = params.radius * params.radius;

    // Count neighbors for each point
    let filtered: Vec<ColoredPoint> = points
        .iter()
        .filter(|point| {
            let neighbor_count = points
                .iter()
                .filter(|other| {
                    let dist_sq = na::distance_squared(&point.position, &other.position);
                    dist_sq > 0.0 && dist_sq < radius_sq
                })
                .count();

            neighbor_count >= params.min_neighbors
        })
        .copied()
        .collect();

    let removed = points.len() - filtered.len();
    tracing::info!(
        "Removed {} radius outliers ({:.1}%), kept {} points",
        removed,
        (removed as f32 / points.len() as f32) * 100.0,
        filtered.len()
    );

    filtered
}

/// Confidence-based filtering
/// Remove points with low confidence scores
pub fn filter_by_confidence(points: &[ColoredPoint], min_confidence: f32) -> Vec<ColoredPoint> {
    let filtered: Vec<ColoredPoint> = points
        .iter()
        .filter(|p| p.confidence >= min_confidence)
        .copied()
        .collect();

    let removed = points.len() - filtered.len();
    if removed > 0 {
        tracing::info!(
            "Removed {} low-confidence points ({:.1}%)",
            removed,
            (removed as f32 / points.len() as f32) * 100.0
        );
    }

    filtered
}

/// Voxel grid downsampling
/// Reduce point density by averaging points in voxels
pub fn voxel_downsample(points: &[ColoredPoint], voxel_size: f32) -> Vec<ColoredPoint> {
    if points.is_empty() {
        return Vec::new();
    }

    tracing::info!(
        "Downsampling {} points with voxel size {:.4}m...",
        points.len(),
        voxel_size
    );

    // Group points by voxel
    let mut voxel_map: HashMap<(i32, i32, i32), Vec<ColoredPoint>> = HashMap::new();

    for point in points {
        let voxel = (
            (point.position.x / voxel_size).floor() as i32,
            (point.position.y / voxel_size).floor() as i32,
            (point.position.z / voxel_size).floor() as i32,
        );
        voxel_map.entry(voxel).or_default().push(*point);
    }

    // Average points in each voxel
    let downsampled: Vec<ColoredPoint> = voxel_map
        .values()
        .map(|voxel_points| {
            let n = voxel_points.len() as f32;

            let avg_pos = voxel_points
                .iter()
                .fold(na::Vector3::zeros(), |acc, p| acc + p.position.coords)
                / n;

            let avg_color = [
                (voxel_points.iter().map(|p| p.color[0] as f32).sum::<f32>() / n) as u8,
                (voxel_points.iter().map(|p| p.color[1] as f32).sum::<f32>() / n) as u8,
                (voxel_points.iter().map(|p| p.color[2] as f32).sum::<f32>() / n) as u8,
            ];

            let avg_confidence = voxel_points.iter().map(|p| p.confidence).sum::<f32>() / n;

            ColoredPoint {
                position: na::Point3::from(avg_pos),
                color: avg_color,
                confidence: avg_confidence,
            }
        })
        .collect();

    tracing::info!(
        "Downsampled to {} points ({:.1}% reduction)",
        downsampled.len(),
        (1.0 - downsampled.len() as f32 / points.len() as f32) * 100.0
    );

    downsampled
}

/// Laplacian smoothing for meshes
/// Smooth mesh while preserving overall shape
pub fn smooth_mesh(mesh: &mut Mesh, iterations: usize, lambda: f32) {
    if mesh.vertices.is_empty() {
        return;
    }

    tracing::info!("Smoothing mesh with {} iterations...", iterations);

    for _ in 0..iterations {
        // Build adjacency list
        let mut adjacency: HashMap<usize, Vec<usize>> = HashMap::new();

        for face in &mesh.faces {
            for i in 0..3 {
                let v1 = face.vertices[i];
                let v2 = face.vertices[(i + 1) % 3];
                adjacency.entry(v1).or_default().push(v2);
                adjacency.entry(v2).or_default().push(v1);
            }
        }

        // Compute new positions
        let mut new_vertices = mesh.vertices.clone();

        for (vertex_idx, neighbors) in adjacency.iter() {
            if neighbors.is_empty() {
                continue;
            }

            // Compute average of neighbors
            let mut avg = na::Vector3::zeros();
            for &neighbor_idx in neighbors {
                if neighbor_idx < mesh.vertices.len() {
                    avg += mesh.vertices[neighbor_idx].coords;
                }
            }
            avg /= neighbors.len() as f32;

            // Move vertex towards average (weighted by lambda)
            if *vertex_idx < new_vertices.len() {
                let current = mesh.vertices[*vertex_idx].coords;
                new_vertices[*vertex_idx] = na::Point3::from(current + lambda * (avg - current));
            }
        }

        mesh.vertices = new_vertices;
    }

    tracing::info!("Mesh smoothing complete");
}

/// Remove small disconnected components from mesh
pub fn remove_small_components(mesh: &mut Mesh, min_faces: usize) {
    if mesh.faces.is_empty() {
        return;
    }

    tracing::info!(
        "Removing small mesh components (min {} faces)...",
        min_faces
    );

    // Build face adjacency
    let mut visited = vec![false; mesh.faces.len()];
    let mut components = Vec::new();

    for start_idx in 0..mesh.faces.len() {
        if visited[start_idx] {
            continue;
        }

        // BFS to find connected component
        let mut component = Vec::new();
        let mut queue = vec![start_idx];
        visited[start_idx] = true;

        while let Some(face_idx) = queue.pop() {
            component.push(face_idx);

            // Find adjacent faces (sharing vertices)
            let face = &mesh.faces[face_idx];
            for other_idx in 0..mesh.faces.len() {
                if visited[other_idx] {
                    continue;
                }

                let other_face = &mesh.faces[other_idx];
                let shared_vertices = face
                    .vertices
                    .iter()
                    .filter(|v| other_face.vertices.contains(v))
                    .count();

                if shared_vertices >= 2 {
                    visited[other_idx] = true;
                    queue.push(other_idx);
                }
            }
        }

        components.push(component);
    }

    // Keep only large components
    let mut kept_faces = Vec::new();
    for component in components {
        if component.len() >= min_faces {
            for face_idx in component {
                kept_faces.push(mesh.faces[face_idx].clone());
            }
        }
    }

    let removed = mesh.faces.len() - kept_faces.len();
    mesh.faces = kept_faces;

    tracing::info!("Removed {} faces from small components", removed);
}
