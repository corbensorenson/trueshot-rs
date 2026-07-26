//! Scene Reconstruction Module
//!
//! Reconstructs 4DGS scenes from crowd-sourced, heterogeneous video sources:
//! - Multi-source video ingest (phones, professional, online)
//! - Audio-based temporal synchronization (fingerprinting + cross-correlation)
//! - Automatic video stabilization
//! - Camera pose estimation from wild footage
//! - Confidence/uncertainty mapping for reconstruction quality
//! - Spatial audio reconstruction from multiple sources

use nalgebra as na;
use anyhow::{Context, Result};
use rustfft::{FftPlanner, num_complex::Complex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use crate::reconstruction::Mesh;

// ============================================================================
// Video Source Types
// ============================================================================

/// Source video input for scene reconstruction
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VideoSource {
    /// Unique identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Source type
    pub source_type: VideoSourceType,
    /// File path or URL
    pub path: String,
    /// Video metadata
    pub metadata: VideoMetadata,
    /// Extracted audio track
    pub audio_track: Option<AudioTrack>,
    /// Temporal alignment info
    pub alignment: Option<TemporalAlignment>,
    /// Quality assessment
    pub quality: VideoQuality,
    /// Processing state
    pub state: SourceState,
    /// Optional per-frame motion vectors for stabilization/pose estimation
    #[serde(default)]
    pub motion_vectors: Option<Vec<MotionVector>>,
    /// Optional precomputed camera path (from VIO/SfM)
    #[serde(default)]
    pub camera_path: Option<CameraPath>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum VideoSourceType {
    /// User's own phone/camera footage
    Personal,
    /// Official broadcast/recording
    Official,
    /// Found online (YouTube, social media, etc.)
    Online,
    /// Security/surveillance footage
    Surveillance,
    /// Professional media coverage
    Professional,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VideoMetadata {
    /// Duration in seconds
    pub duration_secs: f64,
    /// Frame rate
    pub fps: f64,
    /// Resolution
    pub resolution: (u32, u32),
    /// Codec
    pub codec: String,
    /// Has audio
    pub has_audio: bool,
    /// Audio sample rate
    pub audio_sample_rate: Option<u32>,
    /// Recording timestamp (if known)
    pub recorded_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Device info (if available)
    pub device_info: Option<String>,
    /// GPS location (if available)
    pub gps_location: Option<(f64, f64)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioTrack {
    /// Extracted samples (mono, normalized)
    pub samples: Vec<f32>,
    /// Sample rate
    pub sample_rate: u32,
    /// Audio fingerprint for matching
    pub fingerprint: Option<AudioFingerprint>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioFingerprint {
    /// Chromaprint-style fingerprint hash
    pub hash: Vec<u32>,
    /// Spectral peaks for alignment
    pub peaks: Vec<SpectralPeak>,
    /// Hashed peak pairs with timestamps
    #[serde(default)]
    pub hashes: Vec<FingerprintHash>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FingerprintHash {
    pub hash: u32,
    pub time: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpectralPeak {
    /// Time offset in samples
    pub time: u64,
    /// Frequency bin
    pub freq: u32,
    /// Magnitude
    pub magnitude: f32,
}

/// Temporal alignment between source and reference timeline
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemporalAlignment {
    /// Offset in seconds (source starts at t=offset on master timeline)
    pub offset_secs: f64,
    /// Time stretch factor (1.0 = normal speed)
    pub stretch_factor: f64,
    /// Confidence in alignment (0-1)
    pub confidence: f32,
    /// Method used for alignment
    pub method: AlignmentMethod,
    /// Per-segment alignment corrections
    pub drift_corrections: Vec<DriftCorrection>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AlignmentMethod {
    /// Audio fingerprint matching
    AudioFingerprint,
    /// Cross-correlation of audio
    AudioCrossCorrelation,
    /// Visual event detection
    VisualEvents,
    /// Manual user alignment
    Manual,
    /// Metadata timestamp
    Metadata,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriftCorrection {
    /// Start time on master timeline
    pub from_time: f64,
    /// End time on master timeline
    pub to_time: f64,
    /// Additional offset adjustment
    pub offset_adjustment: f64,
}

/// Video quality assessment
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VideoQuality {
    /// Overall quality score (0-1)
    pub overall_score: f32,
    /// Resolution score
    pub resolution_score: f32,
    /// Stability score (0=very shaky, 1=stable)
    pub stability_score: f32,
    /// Focus/sharpness score
    pub sharpness_score: f32,
    /// Exposure quality
    pub exposure_score: f32,
    /// Motion blur severity
    pub motion_blur_score: f32,
    /// Compression artifact severity
    pub compression_score: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SourceState {
    Pending,
    Analyzing,
    Aligning,
    Stabilizing,
    Ready,
    Failed(String),
}

// ============================================================================
// Confidence/Uncertainty Mapping
// ============================================================================

/// Reconstruction confidence at a point in space-time
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfidenceField {
    /// Spatial resolution
    pub voxel_size: f32,
    /// Temporal resolution (seconds per slice)
    pub time_step: f32,
    /// Bounding box min
    pub bounds_min: na::Point3<f32>,
    /// Bounding box max
    pub bounds_max: na::Point3<f32>,
    /// Time range
    pub time_range: (f64, f64),
    /// Confidence values (flattened 4D grid: t, z, y, x)
    pub values: Vec<f32>,
    /// Grid dimensions (t, z, y, x)
    pub dimensions: (usize, usize, usize, usize),
}

impl ConfidenceField {
    pub fn new(
        bounds_min: na::Point3<f32>,
        bounds_max: na::Point3<f32>,
        time_range: (f64, f64),
        voxel_size: f32,
        time_step: f32,
    ) -> Self {
        let dims = (
            ((bounds_max.x - bounds_min.x) / voxel_size).ceil() as usize,
            ((bounds_max.y - bounds_min.y) / voxel_size).ceil() as usize,
            ((bounds_max.z - bounds_min.z) / voxel_size).ceil() as usize,
        );
        let t_dims = ((time_range.1 - time_range.0) / time_step as f64).ceil() as usize;
        
        let total = t_dims * dims.2 * dims.1 * dims.0;
        
        Self {
            voxel_size,
            time_step,
            bounds_min,
            bounds_max,
            time_range,
            values: vec![0.0; total],
            dimensions: (t_dims, dims.2, dims.1, dims.0),
        }
    }
    
    /// Get confidence at space-time point
    pub fn sample(&self, position: na::Point3<f32>, time: f64) -> f32 {
        let t_idx = ((time - self.time_range.0) / self.time_step as f64) as usize;
        let x_idx = ((position.x - self.bounds_min.x) / self.voxel_size) as usize;
        let y_idx = ((position.y - self.bounds_min.y) / self.voxel_size) as usize;
        let z_idx = ((position.z - self.bounds_min.z) / self.voxel_size) as usize;
        
        if t_idx >= self.dimensions.0 ||
           z_idx >= self.dimensions.1 ||
           y_idx >= self.dimensions.2 ||
           x_idx >= self.dimensions.3 {
            return 0.0;
        }
        
        let idx = ((t_idx * self.dimensions.1 + z_idx) * self.dimensions.2 + y_idx) 
                  * self.dimensions.3 + x_idx;
        self.values.get(idx).copied().unwrap_or(0.0)
    }
    
    /// Accumulate view coverage
    pub fn accumulate_view(
        &mut self,
        camera_pos: na::Point3<f32>,
        camera_dir: na::Vector3<f32>,
        fov: f32,
        time: f64,
        weight: f32,
    ) {
        let t_idx = ((time - self.time_range.0) / self.time_step as f64) as usize;
        if t_idx >= self.dimensions.0 {
            return;
        }
        
        // Simple frustum-based confidence accumulation
        let half_fov = fov / 2.0;
        let cos_half_fov = half_fov.cos();
        
        for z in 0..self.dimensions.1 {
            for y in 0..self.dimensions.2 {
                for x in 0..self.dimensions.3 {
                    let pos = na::Point3::new(
                        self.bounds_min.x + x as f32 * self.voxel_size,
                        self.bounds_min.y + y as f32 * self.voxel_size,
                        self.bounds_min.z + z as f32 * self.voxel_size,
                    );
                    
                    let dir_to_point = (pos - camera_pos).normalize();
                    let angle_cos = dir_to_point.dot(&camera_dir);
                    
                    if angle_cos > cos_half_fov {
                        let distance = (pos - camera_pos).norm();
                        let dist_weight = (1.0 / (1.0 + distance * 0.1)).min(1.0);
                        let view_weight = (angle_cos - cos_half_fov) / (1.0 - cos_half_fov);
                        
                        let idx = ((t_idx * self.dimensions.1 + z) * self.dimensions.2 + y) 
                                  * self.dimensions.3 + x;
                        if idx < self.values.len() {
                            self.values[idx] += weight * dist_weight * view_weight;
                        }
                    }
                }
            }
        }
    }
    
    /// Normalize confidence values
    pub fn normalize(&mut self) {
        let max_val = self.values.iter().cloned().fold(0.0f32, f32::max);
        if max_val > 0.0 {
            for v in &mut self.values {
                *v /= max_val;
            }
        }
    }
}

/// Uncertainty visualization mode
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ConfidenceVisualization {
    /// No confidence overlay
    None,
    /// Grayscale overlay (dark = uncertain)
    Grayscale,
    /// Heatmap (blue=high, red=low)
    Heatmap,
    /// Transparency (transparent = uncertain)
    Transparency,
    /// Wireframe in uncertain regions
    Wireframe,
}

// ============================================================================
// Floorplan + Measurements
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FloorPlanOptions {
    /// Grid cell size in meters
    pub cell_size_m: f32,
    /// Minimum points per cell to consider occupied
    pub min_points_per_cell: u32,
    /// Floor band thickness in meters
    pub floor_band_m: f32,
    /// Percentile used to estimate floor height
    pub floor_percentile: f32,
    /// Optional scale factor applied to mesh units (1.0 = no scaling)
    pub scale_factor: f32,
}

impl Default for FloorPlanOptions {
    fn default() -> Self {
        Self {
            cell_size_m: 0.05,
            min_points_per_cell: 3,
            floor_band_m: 0.08,
            floor_percentile: 0.05,
            scale_factor: 1.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FloorPlan {
    pub outline: Vec<[f32; 2]>,
    pub walls: Vec<WallSegment>,
    pub area_m2: f32,
    pub perimeter_m: f32,
    /// Bounds: min_x, min_z, max_x, max_z
    pub bounds: [f32; 4],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WallSegment {
    pub start: [f32; 2],
    pub end: [f32; 2],
    pub length_m: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Measurement {
    pub name: String,
    pub value: f32,
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FloorPlanOutput {
    pub floor_plan: FloorPlan,
    pub measurements: Vec<Measurement>,
    pub confidence: f32,
    #[serde(default)]
    pub geo_reference: Option<GeoReference>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FloorPlanExportPaths {
    pub geojson_path: PathBuf,
    pub csv_path: PathBuf,
    #[serde(default)]
    pub prj_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeoReference {
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub origin_alt: Option<f64>,
    pub crs: Option<String>,
}

pub fn extract_floorplan_from_mesh(mesh: &Mesh, options: &FloorPlanOptions) -> Result<FloorPlanOutput> {
    if mesh.vertices.is_empty() {
        anyhow::bail!("Cannot extract floorplan from empty mesh");
    }

    let mut ys: Vec<f32> = mesh.vertices.iter().map(|v| v.y).collect();
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let percentile_idx = ((ys.len() as f32) * options.floor_percentile).floor() as usize;
    let percentile_idx = percentile_idx.min(ys.len().saturating_sub(1));
    let floor_y = ys[percentile_idx];
    let max_y = *ys.last().unwrap_or(&floor_y);
    let height_range = (max_y - floor_y).max(1e-3);
    let band = options.floor_band_m.max(height_range * 0.05);
    let ceiling = floor_y + band;

    let scale = options.scale_factor.max(1e-6);
    let mut floor_points: Vec<na::Point2<f32>> = mesh
        .vertices
        .iter()
        .filter(|v| v.y <= ceiling)
        .map(|v| na::Point2::new(v.x * scale, v.z * scale))
        .collect();

    if floor_points.len() < 64 {
        let fallback_idx = ((ys.len() as f32) * 0.12).floor() as usize;
        let fallback_idx = fallback_idx.min(ys.len().saturating_sub(1));
        let fallback_y = ys[fallback_idx];
        let fallback_ceiling = (fallback_y + band * 1.5).min(max_y);
        floor_points = mesh
            .vertices
            .iter()
            .filter(|v| v.y <= fallback_ceiling)
            .map(|v| na::Point2::new(v.x * scale, v.z * scale))
            .collect();
    }

    if floor_points.len() < 16 {
        anyhow::bail!("Insufficient floor points to build floorplan");
    }

    let (min_x, max_x, min_z, max_z) = bounds_2d(&floor_points);
    let cell = options.cell_size_m.max(1e-4);
    let width = ((max_x - min_x) / cell).ceil().max(1.0) as usize;
    let height = ((max_z - min_z) / cell).ceil().max(1.0) as usize;
    let mut counts = vec![0u32; width * height];

    for p in &floor_points {
        let xi = ((p.x - min_x) / cell).floor() as isize;
        let zi = ((p.y - min_z) / cell).floor() as isize;
        if xi >= 0 && zi >= 0 {
            let xi = xi as usize;
            let zi = zi as usize;
            if xi < width && zi < height {
                counts[zi * width + xi] += 1;
            }
        }
    }

    let mut occupied = vec![false; width * height];
    let mut occupied_count = 0usize;
    for i in 0..counts.len() {
        if counts[i] >= options.min_points_per_cell {
            occupied[i] = true;
            occupied_count += 1;
        }
    }

    let mut boundary_points = Vec::new();
    for z in 0..height {
        for x in 0..width {
            if !occupied[z * width + x] {
                continue;
            }
            let mut is_boundary = false;
            for (dx, dz) in &[
                (-1i32, 0i32),
                (1, 0),
                (0, -1),
                (0, 1),
                (-1, -1),
                (-1, 1),
                (1, -1),
                (1, 1),
            ] {
                let nx = x as i32 + dx;
                let nz = z as i32 + dz;
                if nx < 0 || nz < 0 || nx >= width as i32 || nz >= height as i32 {
                    is_boundary = true;
                    break;
                }
                let nidx = nz as usize * width + nx as usize;
                if !occupied[nidx] {
                    is_boundary = true;
                    break;
                }
            }
            if is_boundary {
                let cx = min_x + (x as f32 + 0.5) * cell;
                let cz = min_z + (z as f32 + 0.5) * cell;
                boundary_points.push(na::Point2::new(cx, cz));
            }
        }
    }

    let hull = convex_hull_2d(&boundary_points);
    if hull.len() < 3 {
        anyhow::bail!("Failed to compute floorplan outline");
    }

    let outline: Vec<[f32; 2]> = hull.iter().map(|p| [p.x, p.y]).collect();
    let area_m2 = polygon_area(&hull).abs();
    let perimeter_m = polygon_perimeter(&hull);
    let bounds = [min_x, min_z, max_x, max_z];

    let walls = build_wall_segments(&hull);
    let measurements = build_measurements(area_m2, perimeter_m, min_x, max_x, min_z, max_z);
    let density = occupied_count as f32 / (width * height).max(1) as f32;
    let coverage = (boundary_points.len() as f32 / occupied_count.max(1) as f32).clamp(0.0, 1.0);
    let confidence = (density * 1.5 + coverage * 0.5).clamp(0.0, 1.0);

    Ok(FloorPlanOutput {
        floor_plan: FloorPlan {
            outline,
            walls,
            area_m2,
            perimeter_m,
            bounds,
        },
        measurements,
        confidence,
        geo_reference: None,
    })
}

pub fn export_floorplan_geojson(plan: &FloorPlanOutput, path: &Path) -> Result<()> {
    export_floorplan_geojson_with_reference(plan, plan.geo_reference.as_ref(), path)
}

pub fn export_floorplan_geojson_with_reference(
    plan: &FloorPlanOutput,
    geo_reference: Option<&GeoReference>,
    path: &Path,
) -> Result<()> {
    let mut coords = Vec::new();
    for p in &plan.floor_plan.outline {
        coords.push(format!("[{:.6},{:.6}]", p[0], p[1]));
    }
    if let Some(first) = plan.floor_plan.outline.first() {
        coords.push(format!("[{:.6},{:.6}]", first[0], first[1]));
    }
    let mut crs_block = String::new();
    if let Some(reference) = geo_reference {
        if let Some(crs) = reference.crs.as_ref() {
            crs_block = format!(
                r#",
  "crs": {{
    "type": "name",
    "properties": {{
      "name": "{}"
    }}
  }}"#,
                crs
            );
        }
    }
    let mut origin_block = String::new();
    if let Some(reference) = geo_reference {
        let alt = reference.origin_alt.unwrap_or(0.0);
        origin_block = format!(
            r#",
        "origin_lat": {:.8},
        "origin_lon": {:.8},
        "origin_alt": {:.4}"#,
            reference.origin_lat, reference.origin_lon, alt
        );
    }
    let geojson = format!(
        r#"{{
  "type": "FeatureCollection",
  "features": [
    {{
      "type": "Feature",
      "properties": {{
        "area_m2": {:.6},
        "perimeter_m": {:.6},
        "confidence": {:.4}{} 
      }},
      "geometry": {{
        "type": "Polygon",
        "coordinates": [[{}]]
      }}
    }}
  ]{}
}}"#,
        plan.floor_plan.area_m2,
        plan.floor_plan.perimeter_m,
        plan.confidence,
        origin_block,
        coords.join(","),
        crs_block
    );
    std::fs::write(path, geojson)
        .with_context(|| format!("Failed to write floorplan GeoJSON: {}", path.display()))?;
    Ok(())
}

pub fn export_floorplan_csv(measurements: &[Measurement], path: &Path) -> Result<()> {
    let mut out = String::from("name,value,unit\n");
    for m in measurements {
        out.push_str(&format!("{},{:.6},{}\n", m.name, m.value, m.unit));
    }
    std::fs::write(path, out)
        .with_context(|| format!("Failed to write floorplan CSV: {}", path.display()))?;
    Ok(())
}

pub fn export_floorplan_bundle(
    output_dir: &Path,
    plan: &FloorPlanOutput,
) -> Result<FloorPlanExportPaths> {
    export_floorplan_bundle_with_reference(output_dir, plan, plan.geo_reference.as_ref())
}

pub fn export_floorplan_bundle_with_reference(
    output_dir: &Path,
    plan: &FloorPlanOutput,
    geo_reference: Option<&GeoReference>,
) -> Result<FloorPlanExportPaths> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create floorplan output dir: {}", output_dir.display()))?;
    let geojson_path = output_dir.join("floorplan.geojson");
    let csv_path = output_dir.join("measurements.csv");
    export_floorplan_geojson_with_reference(plan, geo_reference, &geojson_path)?;
    export_floorplan_csv(&plan.measurements, &csv_path)?;
    let prj_path = if let Some(reference) = geo_reference {
        reference.crs.as_ref().map(|crs| {
            let path = output_dir.join("floorplan.prj");
            let _ = std::fs::write(&path, crs);
            path
        })
    } else {
        None
    };
    Ok(FloorPlanExportPaths { geojson_path, csv_path, prj_path })
}

#[cfg(test)]
mod floorplan_tests {
    use super::*;

    #[test]
    fn test_floorplan_extraction_square() {
        let mut vertices = Vec::new();
        let size = 4.0f32;
        let height = 2.5f32;
        for x in 0..40 {
            for z in 0..40 {
                let fx = (x as f32 / 39.0) * size;
                let fz = (z as f32 / 39.0) * size;
                vertices.push(na::Point3::new(fx, 0.0, fz));
            }
        }
        for i in 0..40 {
            let fx = (i as f32 / 39.0) * size;
            vertices.push(na::Point3::new(fx, height, 0.0));
            vertices.push(na::Point3::new(fx, height, size));
            vertices.push(na::Point3::new(0.0, height, fx));
            vertices.push(na::Point3::new(size, height, fx));
        }

        let mesh = Mesh {
            vertices,
            faces: Vec::new(),
            normals: Vec::new(),
            colors: Vec::new(),
            uvs: Vec::new(),
        };

        let plan = extract_floorplan_from_mesh(&mesh, &FloorPlanOptions::default()).unwrap();
        assert!(plan.floor_plan.area_m2 > 8.0);
        assert!(plan.floor_plan.perimeter_m > 8.0);
        assert!(plan.floor_plan.outline.len() >= 4);
    }
}

fn bounds_2d(points: &[na::Point2<f32>]) -> (f32, f32, f32, f32) {
    let mut min_x = points[0].x;
    let mut max_x = points[0].x;
    let mut min_z = points[0].y;
    let mut max_z = points[0].y;
    for p in points {
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_z = min_z.min(p.y);
        max_z = max_z.max(p.y);
    }
    (min_x, max_x, min_z, max_z)
}

fn convex_hull_2d(points: &[na::Point2<f32>]) -> Vec<na::Point2<f32>> {
    if points.len() <= 3 {
        return points.to_vec();
    }
    let mut pts = points.to_vec();
    pts.sort_by(|a, b| {
        a.x.partial_cmp(&b.x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut lower = Vec::new();
    for p in &pts {
        while lower.len() >= 2 {
            let a = lower[lower.len() - 2];
            let b = lower[lower.len() - 1];
            if cross_2d(a, b, *p) <= 0.0 {
                lower.pop();
            } else {
                break;
            }
        }
        lower.push(*p);
    }

    let mut upper = Vec::new();
    for p in pts.iter().rev() {
        while upper.len() >= 2 {
            let a = upper[upper.len() - 2];
            let b = upper[upper.len() - 1];
            if cross_2d(a, b, *p) <= 0.0 {
                upper.pop();
            } else {
                break;
            }
        }
        upper.push(*p);
    }

    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn cross_2d(a: na::Point2<f32>, b: na::Point2<f32>, c: na::Point2<f32>) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn polygon_area(points: &[na::Point2<f32>]) -> f32 {
    let mut area = 0.0;
    for i in 0..points.len() {
        let j = (i + 1) % points.len();
        area += points[i].x * points[j].y - points[j].x * points[i].y;
    }
    area * 0.5
}

fn polygon_perimeter(points: &[na::Point2<f32>]) -> f32 {
    let mut length = 0.0;
    for i in 0..points.len() {
        let j = (i + 1) % points.len();
        let dx = points[j].x - points[i].x;
        let dy = points[j].y - points[i].y;
        length += (dx * dx + dy * dy).sqrt();
    }
    length
}

fn build_wall_segments(points: &[na::Point2<f32>]) -> Vec<WallSegment> {
    let mut walls = Vec::new();
    for i in 0..points.len() {
        let j = (i + 1) % points.len();
        let start = [points[i].x, points[i].y];
        let end = [points[j].x, points[j].y];
        let dx = end[0] - start[0];
        let dy = end[1] - start[1];
        let length = (dx * dx + dy * dy).sqrt();
        walls.push(WallSegment { start, end, length_m: length });
    }
    walls
}

fn build_measurements(
    area_m2: f32,
    perimeter_m: f32,
    min_x: f32,
    max_x: f32,
    min_z: f32,
    max_z: f32,
) -> Vec<Measurement> {
    let width = max_x - min_x;
    let depth = max_z - min_z;
    vec![
        Measurement { name: "area".to_string(), value: area_m2, unit: "m2".to_string() },
        Measurement { name: "perimeter".to_string(), value: perimeter_m, unit: "m".to_string() },
        Measurement { name: "width".to_string(), value: width, unit: "m".to_string() },
        Measurement { name: "depth".to_string(), value: depth, unit: "m".to_string() },
    ]
}

// ============================================================================
// Scene Reconstruction Pipeline
// ============================================================================

/// Main scene reconstruction job
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneReconstructionJob {
    /// Job ID
    pub id: String,
    /// Job name
    pub name: String,
    /// Video sources
    pub sources: Vec<VideoSource>,
    /// Master timeline duration
    pub duration: f64,
    /// Reconstruction settings
    pub settings: ReconstructionSettings,
    /// Processing status
    pub status: ReconstructionStatus,
    /// Confidence field
    pub confidence: Option<ConfidenceField>,
    /// Output path
    pub output_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReconstructionSettings {
    /// Target frame rate for reconstruction
    pub target_fps: f64,
    /// Gaussian splatting quality level
    pub quality: QualityLevel,
    /// Enable video stabilization
    pub stabilize_videos: bool,
    /// Stabilization strength (0-1)
    pub stabilization_strength: f32,
    /// Enable audio-based sync
    pub audio_sync: bool,
    /// Enable visual event sync
    pub visual_sync: bool,
    /// Minimum source quality to include
    pub min_quality_threshold: f32,
    /// Build confidence field
    pub build_confidence: bool,
    /// Confidence visualization mode
    pub confidence_visualization: ConfidenceVisualization,
    /// Enable spatial audio reconstruction
    pub spatial_audio: bool,
    /// Voxel size for confidence grid (meters)
    pub confidence_voxel_size: f32,
}

impl Default for ReconstructionSettings {
    fn default() -> Self {
        Self {
            target_fps: 30.0,
            quality: QualityLevel::High,
            stabilize_videos: true,
            stabilization_strength: 0.7,
            audio_sync: true,
            visual_sync: true,
            min_quality_threshold: 0.3,
            build_confidence: true,
            confidence_visualization: ConfidenceVisualization::Heatmap,
            spatial_audio: true,
            confidence_voxel_size: 0.5,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum QualityLevel {
    Draft,
    Low,
    Medium,
    High,
    Ultra,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReconstructionStatus {
    pub phase: ReconstructionPhase,
    pub progress: f32,
    pub current_operation: String,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ReconstructionPhase {
    Pending,
    AnalyzingSources,
    SyncingTimelines,
    StabilizingVideos,
    EstimatingPoses,
    BuildingGaussians,
    BuildingConfidence,
    ReconstructingAudio,
    Finalizing,
    Complete,
    Failed,
}

// ============================================================================
// Audio Synchronization
// ============================================================================

/// Audio synchronizer using fingerprinting and cross-correlation
pub struct AudioSynchronizer {
    /// Reference audio track (master timeline)
    reference: Option<AudioTrack>,
    /// Sample rate for processing
    sample_rate: u32,
    /// FFT window size
    fft_size: usize,
    /// Hop size for STFT
    hop_size: usize,
}

impl AudioSynchronizer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            reference: None,
            sample_rate,
            fft_size: 2048,
            hop_size: 512,
        }
    }
    
    /// Set reference audio (from official source or best quality)
    pub fn set_reference(&mut self, track: AudioTrack) {
        let mut track = track;
        self.sample_rate = track.sample_rate;
        if track.fingerprint.is_none() {
            track.fingerprint = Some(self.compute_fingerprint(&track.samples));
        }
        self.reference = Some(track);
    }
    
    /// Compute audio fingerprint
    pub fn compute_fingerprint(&self, samples: &[f32]) -> AudioFingerprint {
        let peaks = self.extract_peaks(samples);
        let mut hashes = self.hash_peaks(&peaks);
        if hashes.is_empty() {
            if let Some(first) = peaks.first() {
                let hash = (first.freq.min(1023) << 12) | ((first.time as u32) & 0xFFF);
                hashes.push(FingerprintHash { hash, time: first.time });
            }
        }
        let hash = hashes.iter().map(|h| h.hash).collect();
        AudioFingerprint { hash, peaks, hashes }
    }
    
    /// Find temporal offset using cross-correlation
    pub fn find_offset(&self, source: &AudioTrack) -> Option<TemporalAlignment> {
        let reference = self.reference.as_ref()?;
        let mut ref_track = reference.clone();
        if ref_track.fingerprint.is_none() {
            ref_track.fingerprint = Some(self.compute_fingerprint(&ref_track.samples));
        }
        let mut src_track = source.clone();
        if src_track.fingerprint.is_none() {
            src_track.fingerprint = Some(self.compute_fingerprint(&src_track.samples));
        }

        let ref_fp = ref_track.fingerprint.as_ref()?;
        let src_fp = src_track.fingerprint.as_ref()?;

        let ref_index = build_hash_index(&ref_fp.hashes);
        let mut offset_hist: HashMap<i64, usize> = HashMap::new();
        let mut matched_pairs: Vec<(f64, f64)> = Vec::new();
        let ref_rate = ref_track.sample_rate as f64;
        let src_rate = src_track.sample_rate as f64;

        for entry in &src_fp.hashes {
            if let Some(ref_times) = ref_index.get(&entry.hash) {
                for &ref_time in ref_times.iter().take(6) {
                    let offset_secs = ref_time as f64 / ref_rate - entry.time as f64 / src_rate;
                    let offset_ms = (offset_secs * 1000.0).round() as i64;
                    *offset_hist.entry(offset_ms).or_insert(0) += 1;
                    matched_pairs.push((entry.time as f64 / src_rate, ref_time as f64 / ref_rate));
                }
            }
        }

        if offset_hist.is_empty() {
            let (offset_samples, correlation) = self.cross_correlate(&ref_track.samples, &src_track.samples);
            let offset_secs = offset_samples as f64 / self.sample_rate as f64;
            let confidence = (correlation / 2.0 + 0.5).clamp(0.0, 1.0);
            return Some(TemporalAlignment {
                offset_secs,
                stretch_factor: 1.0,
                confidence,
                method: AlignmentMethod::AudioCrossCorrelation,
                drift_corrections: Vec::new(),
            });
        }

        let (best_offset_ms, best_count) = offset_hist
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1))
            .unwrap_or((0, 0));

        let mut aligned_pairs: Vec<(f64, f64)> = Vec::new();
        let tolerance_ms = 40i64;
        for (src_time, ref_time) in matched_pairs {
            let offset_ms = ((ref_time - src_time) * 1000.0).round() as i64;
            if (offset_ms - best_offset_ms).abs() <= tolerance_ms {
                aligned_pairs.push((src_time, ref_time));
            }
        }

        let (stretch_factor, offset_correction) = estimate_drift(&aligned_pairs);
        let offset_secs = (best_offset_ms as f64 / 1000.0) + offset_correction;
        let confidence = (best_count as f32 / src_fp.hashes.len().max(1) as f32).clamp(0.0, 1.0);

        Some(TemporalAlignment {
            offset_secs,
            stretch_factor,
            confidence,
            method: AlignmentMethod::AudioFingerprint,
            drift_corrections: Vec::new(),
        })
    }
    
    /// Cross-correlation to find best offset
    fn cross_correlate(&self, a: &[f32], b: &[f32]) -> (i64, f32) {
        let max_lag = (a.len().min(b.len()) / 4) as i64;
        let mut best_lag = 0i64;
        let mut best_corr = 0.0f32;
        
        // Subsample for efficiency
        let step = 16;
        
        for lag in -max_lag..max_lag {
            let mut sum = 0.0f32;
            let mut count = 0;
            
            for i in (0..a.len().min(b.len())).step_by(step) {
                let a_idx = i as i64;
                let b_idx = (i as i64) + lag;
                
                if b_idx >= 0 && (b_idx as usize) < b.len() {
                    sum += a[a_idx as usize] * b[b_idx as usize];
                    count += 1;
                }
            }
            
            if count > 0 {
                let corr = sum / count as f32;
                if corr > best_corr {
                    best_corr = corr;
                    best_lag = lag;
                }
            }
        }
        
        (best_lag * step as i64, best_corr)
    }
}

impl AudioSynchronizer {
    fn extract_peaks(&self, samples: &[f32]) -> Vec<SpectralPeak> {
        let mut peaks = Vec::new();
        if samples.len() < self.fft_size {
            let energy: f32 = samples.iter().map(|s| s * s).sum();
            if energy > 1e-6 {
                peaks.push(SpectralPeak {
                    time: 0,
                    freq: 0,
                    magnitude: energy.sqrt(),
                });
            }
            return peaks;
        }
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(self.fft_size);
        let window: Vec<f32> = (0..self.fft_size)
            .map(|i| {
                let phase = 2.0 * std::f32::consts::PI * i as f32 / self.fft_size as f32;
                0.5 - 0.5 * phase.cos()
            })
            .collect();

        let num_chunks = (samples.len().saturating_sub(self.fft_size)) / self.hop_size;
        let mut buffer = vec![Complex::new(0.0f32, 0.0f32); self.fft_size];
        for chunk_idx in 0..num_chunks {
            let start = chunk_idx * self.hop_size;
            let chunk = &samples[start..start + self.fft_size];
            for (i, sample) in chunk.iter().enumerate() {
                buffer[i] = Complex::new(sample * window[i], 0.0);
            }
            fft.process(&mut buffer);
            let mut mags = Vec::with_capacity(self.fft_size / 2);
            for bin in 0..(self.fft_size / 2) {
                let c = buffer[bin];
                mags.push((c.re * c.re + c.im * c.im).sqrt());
            }

            let mut candidates: Vec<(usize, f32)> = Vec::new();
            for bin in 2..mags.len().saturating_sub(2) {
                let m = mags[bin];
                if m > mags[bin - 1] && m > mags[bin + 1] && m > mags[bin - 2] && m > mags[bin + 2] {
                    candidates.push((bin, m));
                }
            }
            candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            for (bin, mag) in candidates.into_iter().take(6) {
                peaks.push(SpectralPeak {
                    time: (start + self.fft_size / 2) as u64,
                    freq: bin as u32,
                    magnitude: mag,
                });
            }
        }
        if peaks.is_empty() {
            let energy: f32 = samples.iter().map(|s| s * s).sum();
            if energy > 1e-6 {
                peaks.push(SpectralPeak {
                    time: 0,
                    freq: 0,
                    magnitude: energy.sqrt(),
                });
            }
        }
        peaks
    }

    fn hash_peaks(&self, peaks: &[SpectralPeak]) -> Vec<FingerprintHash> {
        let mut hashes = Vec::new();
        let fan_out = 6usize;
        let min_dt = (self.hop_size / 2) as u64;
        let max_dt = (self.hop_size * 40) as u64;
        for (i, peak) in peaks.iter().enumerate() {
            for j in 1..=fan_out {
                let Some(next) = peaks.get(i + j) else { break };
                let dt = next.time.saturating_sub(peak.time);
                if dt < min_dt || dt > max_dt {
                    continue;
                }
                let f1 = (peak.freq.min(1023) as u32) & 0x3FF;
                let f2 = (next.freq.min(1023) as u32) & 0x3FF;
                let dt_bin = (dt.min(4095) as u32) & 0xFFF;
                let hash = (f1 << 22) | (f2 << 12) | dt_bin;
                hashes.push(FingerprintHash { hash, time: peak.time });
            }
        }
        hashes
    }
}

fn build_hash_index(hashes: &[FingerprintHash]) -> HashMap<u32, Vec<u64>> {
    let mut index = HashMap::new();
    for entry in hashes {
        index.entry(entry.hash).or_insert_with(Vec::new).push(entry.time);
    }
    index
}

fn estimate_drift(pairs: &[(f64, f64)]) -> (f64, f64) {
    if pairs.len() < 3 {
        return (1.0, 0.0);
    }
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    for (x, y) in pairs {
        sum_x += x;
        sum_y += y;
    }
    let mean_x = sum_x / pairs.len() as f64;
    let mean_y = sum_y / pairs.len() as f64;
    let mut num = 0.0;
    let mut den = 0.0;
    for (x, y) in pairs {
        num += (x - mean_x) * (y - mean_y);
        den += (x - mean_x) * (x - mean_x);
    }
    if den.abs() < 1e-9 {
        return (1.0, mean_y - mean_x);
    }
    let slope = num / den;
    let intercept = mean_y - slope * mean_x;
    (slope, intercept)
}

// ============================================================================
// Video Stabilization
// ============================================================================

/// Video stabilization using optical flow
pub struct VideoStabilizer {
    /// Smoothing window size
    window_size: usize,
    /// Stabilization strength (0-1)
    strength: f32,
}

impl VideoStabilizer {
    pub fn new(strength: f32) -> Self {
        Self {
            window_size: 30,
            strength: strength.clamp(0.0, 1.0),
        }
    }
    
    /// Estimate camera path from frame-to-frame motion
    pub fn estimate_camera_path(&self, motion_vectors: &[MotionVector]) -> CameraPath {
        // Accumulate motion to get camera path
        let mut path = Vec::with_capacity(motion_vectors.len());
        let mut cumulative = CameraTransform::identity();
        
        for mv in motion_vectors {
            cumulative = cumulative.apply(&mv.transform);
            path.push(cumulative.clone());
        }
        
        CameraPath { transforms: path }
    }
    
    /// Smooth camera path
    pub fn smooth_path(&self, path: &CameraPath) -> CameraPath {
        let n = path.transforms.len();
        let mut smoothed = Vec::with_capacity(n);
        
        for i in 0..n {
            let start = i.saturating_sub(self.window_size / 2);
            let end = (i + self.window_size / 2).min(n);
            
            // Average transforms in window
            let mut avg_tx = 0.0f32;
            let mut avg_ty = 0.0f32;
            let mut avg_angle = 0.0f32;
            
            for j in start..end {
                avg_tx += path.transforms[j].translation.x;
                avg_ty += path.transforms[j].translation.y;
                avg_angle += path.transforms[j].rotation;
            }
            
            let count = (end - start) as f32;
            avg_tx /= count;
            avg_ty /= count;
            avg_angle /= count;
            
            // Blend original with smoothed
            let orig = &path.transforms[i];
            let smooth = CameraTransform {
                translation: na::Vector2::new(
                    orig.translation.x * (1.0 - self.strength) + avg_tx * self.strength,
                    orig.translation.y * (1.0 - self.strength) + avg_ty * self.strength,
                ),
                rotation: orig.rotation * (1.0 - self.strength) + avg_angle * self.strength,
                scale: orig.scale,
            };
            
            smoothed.push(smooth);
        }
        
        CameraPath { transforms: smoothed }
    }
    
    /// Compute stabilization transforms
    pub fn compute_stabilization(&self, original: &CameraPath, smoothed: &CameraPath) -> Vec<CameraTransform> {
        original.transforms.iter()
            .zip(smoothed.transforms.iter())
            .map(|(orig, smooth)| {
                CameraTransform {
                    translation: smooth.translation - orig.translation,
                    rotation: smooth.rotation - orig.rotation,
                    scale: 1.0,
                }
            })
            .collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MotionVector {
    pub frame_idx: usize,
    pub transform: CameraTransform,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CameraTransform {
    pub translation: na::Vector2<f32>,
    pub rotation: f32,
    pub scale: f32,
}

impl CameraTransform {
    pub fn identity() -> Self {
        Self {
            translation: na::Vector2::zeros(),
            rotation: 0.0,
            scale: 1.0,
        }
    }
    
    pub fn apply(&self, other: &CameraTransform) -> Self {
        Self {
            translation: self.translation + other.translation,
            rotation: self.rotation + other.rotation,
            scale: self.scale * other.scale,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CameraPath {
    pub transforms: Vec<CameraTransform>,
}

fn sample_camera_transform(path: &CameraPath, index: usize, total: usize) -> CameraTransform {
    if path.transforms.is_empty() {
        return CameraTransform::identity();
    }
    if total <= 1 {
        return path.transforms[0].clone();
    }
    let t = index as f32 / (total - 1) as f32;
    let target = (t * (path.transforms.len() - 1) as f32).round() as usize;
    path.transforms.get(target).cloned().unwrap_or_else(CameraTransform::identity)
}

// ============================================================================
// Scene Reconstruction Manager
// ============================================================================

/// Main scene reconstruction manager
pub struct SceneReconstructor {
    /// Active jobs
    jobs: HashMap<String, SceneReconstructionJob>,
    /// Audio synchronizer
    audio_sync: AudioSynchronizer,
    /// Video stabilizer
    stabilizer: VideoStabilizer,
}

impl Default for SceneReconstructor {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneReconstructor {
    pub fn new() -> Self {
        Self {
            jobs: HashMap::new(),
            audio_sync: AudioSynchronizer::new(48000),
            stabilizer: VideoStabilizer::new(0.7),
        }
    }
    
    /// Create new reconstruction job
    pub fn create_job(&mut self, name: &str, settings: ReconstructionSettings) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        
        let job = SceneReconstructionJob {
            id: id.clone(),
            name: name.to_string(),
            sources: Vec::new(),
            duration: 0.0,
            settings,
            status: ReconstructionStatus {
                phase: ReconstructionPhase::Pending,
                progress: 0.0,
                current_operation: "Initializing".to_string(),
                errors: Vec::new(),
                warnings: Vec::new(),
            },
            confidence: None,
            output_path: None,
        };
        
        self.jobs.insert(id.clone(), job);
        id
    }
    
    /// Add video source to job
    pub fn add_source(&mut self, job_id: &str, source: VideoSource) -> Result<(), ReconstructionError> {
        let job = self.jobs.get_mut(job_id)
            .ok_or(ReconstructionError::JobNotFound)?;
        
        job.sources.push(source);
        Ok(())
    }
    
    /// Analyze and sync all sources
    pub fn sync_sources(&mut self, job_id: &str) -> Result<(), ReconstructionError> {
        let job = self.jobs.get_mut(job_id)
            .ok_or(ReconstructionError::JobNotFound)?;
        
        if job.sources.is_empty() {
            return Err(ReconstructionError::NoSources);
        }
        
        job.status.phase = ReconstructionPhase::SyncingTimelines;
        job.status.current_operation = "Synchronizing timelines".to_string();
        
        // Find best reference (prefer official sources)
        let ref_idx = job.sources.iter()
            .enumerate()
            .filter(|(_, s)| s.audio_track.is_some())
            .max_by(|(_, a), (_, b)| {
                let score_a = match a.source_type {
                    VideoSourceType::Official => 100,
                    VideoSourceType::Professional => 80,
                    _ => a.quality.overall_score as i32,
                };
                let score_b = match b.source_type {
                    VideoSourceType::Official => 100,
                    VideoSourceType::Professional => 80,
                    _ => b.quality.overall_score as i32,
                };
                score_a.cmp(&score_b)
            })
            .map(|(i, _)| i);
        
        if let Some(ref_idx) = ref_idx {
            if let Some(ref_audio) = &job.sources[ref_idx].audio_track {
                self.audio_sync.set_reference(ref_audio.clone());
            }
            
            // Align other sources
            for i in 0..job.sources.len() {
                if i == ref_idx {
                    job.sources[i].alignment = Some(TemporalAlignment {
                        offset_secs: 0.0,
                        stretch_factor: 1.0,
                        confidence: 1.0,
                        method: AlignmentMethod::Manual,
                        drift_corrections: Vec::new(),
                    });
                    continue;
                }
                
                if let Some(audio) = &job.sources[i].audio_track {
                    if let Some(alignment) = self.audio_sync.find_offset(audio) {
                        job.sources[i].alignment = Some(alignment);
                    }
                }
            }
        }
        
        // Compute total duration
        job.duration = job.sources.iter()
            .filter_map(|s| {
                let offset = s.alignment.as_ref()?.offset_secs;
                Some(offset + s.metadata.duration_secs)
            })
            .fold(0.0f64, f64::max);
        
        Ok(())
    }
    
    /// Build confidence field
    pub fn build_confidence(&mut self, job_id: &str) -> Result<(), ReconstructionError> {
        let job = self.jobs.get_mut(job_id)
            .ok_or(ReconstructionError::JobNotFound)?;
        
        job.status.phase = ReconstructionPhase::BuildingConfidence;
        job.status.current_operation = "Building confidence field".to_string();
        
        // Estimate scene bounds from GPS or default
        let bounds_min = na::Point3::new(-50.0, -10.0, -50.0);
        let bounds_max = na::Point3::new(50.0, 30.0, 50.0);
        
        let mut confidence = ConfidenceField::new(
            bounds_min,
            bounds_max,
            (0.0, job.duration),
            job.settings.confidence_voxel_size,
            1.0,  // 1 second time steps
        );
        
        // Accumulate views from each source
        for source in &job.sources {
            if source.state != SourceState::Ready {
                continue;
            }
            
            let offset = source.alignment.as_ref()
                .map(|a| a.offset_secs)
                .unwrap_or(0.0);
            
            let quality_weight = source.quality.overall_score;

            let num_samples = (source.metadata.duration_secs * source.metadata.fps / 30.0) as usize;
            if num_samples == 0 {
                continue;
            }

            let path = if let Some(path) = &source.camera_path {
                Some(path.clone())
            } else if let Some(vectors) = &source.motion_vectors {
                Some(self.stabilizer.estimate_camera_path(vectors))
            } else {
                None
            };

            for i in 0..num_samples {
                let t = i as f64 / num_samples as f64 * source.metadata.duration_secs;
                let global_t = offset + t;

                let transform = if let Some(path) = &path {
                    sample_camera_transform(path, i, num_samples)
                } else {
                    CameraTransform::identity()
                };

                let jitter_scale = (1.0 - source.quality.stability_score).clamp(0.1, 1.0);
                let yaw = transform.rotation;
                let camera_pos = na::Point3::new(
                    transform.translation.x * jitter_scale,
                    1.5,
                    transform.translation.y * jitter_scale,
                );
                let camera_dir = na::Vector3::new(yaw.sin(), 0.0, yaw.cos());
                
                confidence.accumulate_view(
                    camera_pos,
                    camera_dir,
                    1.2,  // ~70 degree FOV
                    global_t,
                    quality_weight,
                );
            }
        }
        
        confidence.normalize();
        job.confidence = Some(confidence);
        
        Ok(())
    }
    
    /// Get job by ID
    pub fn get_job(&self, job_id: &str) -> Option<&SceneReconstructionJob> {
        self.jobs.get(job_id)
    }
    
    /// List all jobs
    pub fn list_jobs(&self) -> Vec<&SceneReconstructionJob> {
        self.jobs.values().collect()
    }
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Clone, Debug)]
pub enum ReconstructionError {
    JobNotFound,
    NoSources,
    SyncFailed(String),
    StabilizationFailed(String),
    PoseEstimationFailed(String),
    GaussianBuildFailed(String),
    IoError(String),
}

impl std::fmt::Display for ReconstructionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JobNotFound => write!(f, "Job not found"),
            Self::NoSources => write!(f, "No sources provided"),
            Self::SyncFailed(msg) => write!(f, "Sync failed: {}", msg),
            Self::StabilizationFailed(msg) => write!(f, "Stabilization failed: {}", msg),
            Self::PoseEstimationFailed(msg) => write!(f, "Pose estimation failed: {}", msg),
            Self::GaussianBuildFailed(msg) => write!(f, "Gaussian build failed: {}", msg),
            Self::IoError(msg) => write!(f, "IO error: {}", msg),
        }
    }
}

impl std::error::Error for ReconstructionError {}

// ============================================================================
// Web API Types
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneReconstructionJobInfo {
    pub id: String,
    pub name: String,
    pub source_count: usize,
    pub duration: f64,
    pub phase: ReconstructionPhase,
    pub progress: f32,
}

impl From<&SceneReconstructionJob> for SceneReconstructionJobInfo {
    fn from(job: &SceneReconstructionJob) -> Self {
        Self {
            id: job.id.clone(),
            name: job.name.clone(),
            source_count: job.sources.len(),
            duration: job.duration,
            phase: job.status.phase.clone(),
            progress: job.status.progress,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_confidence_field() {
        let mut field = ConfidenceField::new(
            na::Point3::new(-10.0, -10.0, -10.0),
            na::Point3::new(10.0, 10.0, 10.0),
            (0.0, 10.0),
            1.0,
            1.0,
        );
        
        field.accumulate_view(
            na::Point3::new(0.0, 0.0, -5.0),
            na::Vector3::new(0.0, 0.0, 1.0),
            1.0,
            5.0,
            1.0,
        );
        
        field.normalize();
        
        // Should have non-zero confidence in front of camera
        let conf = field.sample(na::Point3::new(0.0, 0.0, 0.0), 5.0);
        assert!(conf > 0.0);
    }
    
    #[test]
    fn test_audio_fingerprint() {
        let sync = AudioSynchronizer::new(48000);
        let samples = vec![0.5f32; 48000];
        let fp = sync.compute_fingerprint(&samples);
        assert!(!fp.hash.is_empty());
    }
    
    #[test]
    fn test_video_stabilizer() {
        let stabilizer = VideoStabilizer::new(0.7);
        
        let motions: Vec<_> = (0..100).map(|i| MotionVector {
            frame_idx: i,
            transform: CameraTransform {
                translation: na::Vector2::new((i as f32 * 0.1).sin() * 5.0, 0.0),
                rotation: 0.0,
                scale: 1.0,
            },
        }).collect();
        
        let path = stabilizer.estimate_camera_path(&motions);
        let smoothed = stabilizer.smooth_path(&path);
        
        // Smoothed path should have less variation
        let orig_var: f32 = path.transforms.windows(2)
            .map(|w| (w[1].translation.x - w[0].translation.x).abs())
            .sum();
        let smooth_var: f32 = smoothed.transforms.windows(2)
            .map(|w| (w[1].translation.x - w[0].translation.x).abs())
            .sum();
        
        assert!(smooth_var < orig_var);
    }
}
