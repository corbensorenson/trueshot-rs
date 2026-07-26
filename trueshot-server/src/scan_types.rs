use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BackgroundStatus {
    pub captured: bool,
    pub timestamp: Option<String>,
    pub frame_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ObjectDetection {
    pub detected: bool,
    pub confidence: f32,
    pub bounding_box: Option<BoundingBox>,
    pub stable: bool,
    pub stable_duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BoundingBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ObjectAnalysis {
    pub size: SizeInfo,
    pub complexity: ComplexityInfo,
    pub surface: SurfaceInfo,
    pub has_underside_detail: bool,
    pub aspect_ratio: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SizeInfo {
    pub category: String,       // tiny, small, medium, large, xlarge
    pub dimensions: [f32; 3],   // width, height, depth in cm
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ComplexityInfo {
    pub category: String,   // simple, moderate, complex, intricate
    pub feature_count: u32,
    pub score: f32,         // 0.0 - 1.0
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SurfaceInfo {
    pub surface_type: String, // matte, glossy, metallic, mixed
    pub specular_ratio: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ScanPlan {
    pub quality_level: String,
    pub object_orientations: u32,
    pub camera_positions_per_orientation: u32,
    pub photos_per_rotation: u32,
    pub total_photos: u32,
    pub estimated_time_seconds: u32,
    pub steps: Vec<ScanStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ScanStep {
    pub step_type: String, // camera_position, object_orientation, capture
    pub instruction: String,
    pub camera_position: Option<u32>,
    pub object_orientation: Option<u32>,
    pub rotation_angle: Option<f32>,
    pub photo_index: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ComputePlanRequest {
    pub quality_level: String, // preview, standard, high, ultra
    pub analysis: ObjectAnalysis,
    #[serde(default)]
    pub preset: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ScanProgress {
    pub status: String, // idle, capturing, paused, complete, error, stopped
    pub current_step: u32,
    pub total_steps: u32,
    pub photos_captured: u32,
    pub elapsed_seconds: u64,
    pub current_instruction: String,
    pub error_message: Option<String>,
    #[serde(default)]
    pub step_integrity: Vec<StepIntegrity>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub quality: Option<QualityAssessment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CoverageStatus {
    pub orientation_index: u32,
    pub azimuth_bins: u32,
    pub elevation_bins: u32,
    pub counts: Vec<f32>,
    pub coverage_score: f32,
    pub coverage_density: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StepIntegrity {
    pub step_index: u32,
    pub expected_files: u32,
    pub verified_files: u32,
    pub ok: bool,
    pub hashes: Vec<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct QualityAssessment {
    pub score: f32,
    pub pass: bool,
    pub issues: Vec<String>,
    pub actions: Vec<String>,
    pub defects: Vec<QualityDefectScore>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct QualityHistoryEntry {
    pub captured_at: String,
    pub score: f32,
    pub pass: bool,
    pub issues: Vec<String>,
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct QualityDefectScore {
    pub defect: String,
    pub score: f64,
    pub threshold: f64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ScaleAnchorRequest {
    pub known_distance_m: f32,
    pub measured_units: f32,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub origin_lat: Option<f64>,
    #[serde(default)]
    pub origin_lon: Option<f64>,
    #[serde(default)]
    pub origin_alt: Option<f64>,
    #[serde(default)]
    pub crs: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ScaleAnchor {
    pub known_distance_m: f32,
    pub measured_units: f32,
    pub meters_per_unit: f32,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub origin_lat: Option<f64>,
    #[serde(default)]
    pub origin_lon: Option<f64>,
    #[serde(default)]
    pub origin_alt: Option<f64>,
    #[serde(default)]
    pub crs: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ScaleAnchorStatus {
    pub configured: bool,
    #[serde(default)]
    pub anchor: Option<ScaleAnchor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ExecuteStepRequest {
    pub step_index: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SDCardStatus {
    pub detected: bool,
    pub volume_name: Option<String>,
    pub image_count: u32,
    pub total_size_mb: u64,
}
