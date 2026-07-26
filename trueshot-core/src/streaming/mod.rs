//! Unified Streaming Protocol
//!
//! Core streaming types and protocols used across all TrueShot modes:
//! - Point cloud streaming
//! - 3DGS/4DGS Gaussian streaming
//! - Mesh streaming with LOD
//! - Avatar animation streaming
//! - Reconstruction progress streaming
//!
//! Protocol features:
//! - Delta compression for bandwidth efficiency
//! - Adaptive quality based on client capability
//! - Binary and JSON message formats
//! - Frame sequencing and interpolation

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Stream Types
// ============================================================================

/// Type of data being streamed
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StreamType {
    /// Raw point cloud
    PointCloud,
    /// 3D Gaussian Splatting data
    Gaussian3D,
    /// 4D Gaussian Splatting (temporal)
    Gaussian4D,
    /// Mesh with vertices/indices
    Mesh,
    /// Avatar animation data
    Avatar,
    /// Camera feed (MJPEG/WebRTC)
    CameraFeed,
    /// Reconstruction progress
    Progress,
    /// Depth map
    DepthMap,
    /// Scene metadata
    SceneInfo,
}

/// Quality level for adaptive streaming
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StreamQuality {
    /// Minimal quality for low bandwidth
    Low,
    /// Reduced quality
    Medium,
    /// Full quality
    High,
    /// Maximum quality with extra features
    Ultra,
}

impl StreamQuality {
    /// Get update frequency multiplier
    pub fn update_multiplier(&self) -> f32 {
        match self {
            StreamQuality::Low => 0.25,
            StreamQuality::Medium => 0.5,
            StreamQuality::High => 1.0,
            StreamQuality::Ultra => 1.0,
        }
    }
    
    /// Get point/gaussian density factor
    pub fn density_factor(&self) -> f32 {
        match self {
            StreamQuality::Low => 0.1,
            StreamQuality::Medium => 0.3,
            StreamQuality::High => 1.0,
            StreamQuality::Ultra => 1.0,
        }
    }
    
    /// Get mesh LOD level
    pub fn mesh_lod(&self) -> u8 {
        match self {
            StreamQuality::Low => 3,
            StreamQuality::Medium => 2,
            StreamQuality::High => 1,
            StreamQuality::Ultra => 0,
        }
    }
}

// ============================================================================
// Stream Messages
// ============================================================================

/// Base stream message header
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamHeader {
    /// Message sequence number
    pub sequence: u64,
    /// Timestamp in milliseconds
    pub timestamp: u64,
    /// Stream type
    pub stream_type: StreamType,
    /// Quality level
    pub quality: StreamQuality,
    /// Is this a delta update?
    pub is_delta: bool,
    /// Reference sequence for delta
    pub delta_ref: Option<u64>,
}

/// Point cloud chunk
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PointCloudChunk {
    /// Positions (x, y, z) interleaved
    pub positions: Vec<f32>,
    /// Colors (r, g, b) interleaved, 0-1
    pub colors: Vec<f32>,
    /// Optional normals (nx, ny, nz) interleaved
    pub normals: Option<Vec<f32>>,
    /// Total point count in this chunk
    pub point_count: u32,
    /// Chunk index for multi-chunk streams
    pub chunk_index: u32,
    /// Total chunks
    pub total_chunks: u32,
}

/// Gaussian data chunk (for 3DGS/4DGS)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GaussianChunk {
    /// Positions (x, y, z) interleaved
    pub positions: Vec<f32>,
    /// Covariance (6 values per Gaussian: upper triangle)
    pub covariances: Vec<f32>,
    /// Spherical harmonics coefficients (flattened)
    pub sh_coeffs: Vec<f32>,
    /// Opacities
    pub opacities: Vec<f32>,
    /// For 4DGS: temporal offset
    pub temporal_offsets: Option<Vec<f32>>,
    /// For 4DGS: motion vectors
    pub motion_vectors: Option<Vec<f32>>,
    /// Count
    pub gaussian_count: u32,
    /// Chunk index
    pub chunk_index: u32,
    /// Total chunks
    pub total_chunks: u32,
}

/// Mesh chunk
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MeshChunk {
    /// Vertex positions (x, y, z) interleaved
    pub positions: Vec<f32>,
    /// Normals (nx, ny, nz) interleaved
    pub normals: Vec<f32>,
    /// UVs (u, v) interleaved
    pub uvs: Vec<f32>,
    /// Triangle indices
    pub indices: Vec<u32>,
    /// Vertex count
    pub vertex_count: u32,
    /// Triangle count
    pub triangle_count: u32,
    /// LOD level (0 = highest)
    pub lod_level: u8,
    /// Object ID for multi-object scenes
    pub object_id: Option<String>,
}

/// Avatar animation frame
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AvatarFrame {
    /// Bone rotations (quaternion x, y, z, w per bone)
    pub bone_rotations: HashMap<String, [f32; 4]>,
    /// Blendshape weights (52 ARKit values as Vec for serde compatibility)
    pub blendshapes: Vec<f32>,
    /// Root position offset
    pub root_position: [f32; 3],
    /// Root rotation (quaternion)
    pub root_rotation: [f32; 4],
    /// Frame number
    pub frame: u64,
    /// Delta from previous frame (compressed)
    pub is_keyframe: bool,
}

/// Reconstruction progress update  
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProgressUpdate {
    /// Overall progress 0-100
    pub percent: f32,
    /// Current stage name
    pub stage: String,
    /// Sub-progress within stage
    pub sub_percent: f32,
    /// Estimated time remaining (seconds)
    pub eta_seconds: Option<f32>,
    /// Current operation description
    pub message: String,
    /// Warning/error messages
    pub warnings: Vec<String>,
}

/// Scene information
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneInfo {
    /// Scene name
    pub name: String,
    /// Bounding box min
    pub bounds_min: [f32; 3],
    /// Bounding box max
    pub bounds_max: [f32; 3],
    /// Total point/gaussian count
    pub element_count: u64,
    /// Number of objects
    pub object_count: u32,
    /// Available stream types
    pub available_streams: Vec<StreamType>,
    /// Recommended quality
    pub recommended_quality: StreamQuality,
}

// ============================================================================
// Stream Packet (Wire Format)
// ============================================================================

/// Complete stream packet
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamPacket {
    /// Header
    pub header: StreamHeader,
    /// Payload (type determined by header.stream_type)
    pub payload: StreamPayload,
}

/// Stream payload variants
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StreamPayload {
    PointCloud(PointCloudChunk),
    Gaussian(GaussianChunk),
    Mesh(MeshChunk),
    Avatar(AvatarFrame),
    Progress(ProgressUpdate),
    Scene(SceneInfo),
    /// Raw binary data (for custom use)
    Raw(Vec<u8>),
    /// Heartbeat/ping
    Heartbeat,
}

// ============================================================================
// Client Capabilities
// ============================================================================

/// Client capability announcement
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// Supported stream types
    pub stream_types: Vec<StreamType>,
    /// Maximum quality supported
    pub max_quality: StreamQuality,
    /// Preferred quality
    pub preferred_quality: StreamQuality,
    /// Max points per update
    pub max_points_per_update: u32,
    /// Max gaussians per update
    pub max_gaussians_per_update: u32,
    /// Supports binary protocol
    pub binary_protocol: bool,
    /// Supports delta compression
    pub delta_compression: bool,
    /// Client version
    pub version: String,
    /// Client platform
    pub platform: String,
}

impl Default for ClientCapabilities {
    fn default() -> Self {
        Self {
            stream_types: vec![StreamType::PointCloud, StreamType::Mesh, StreamType::Progress],
            max_quality: StreamQuality::High,
            preferred_quality: StreamQuality::Medium,
            max_points_per_update: 100_000,
            max_gaussians_per_update: 50_000,
            binary_protocol: true,
            delta_compression: true,
            version: "1.0".to_string(),
            platform: "unknown".to_string(),
        }
    }
}

// ============================================================================
// Stream Configuration
// ============================================================================

/// Stream session configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamConfig {
    /// Session ID
    pub session_id: String,
    /// Active streams
    pub active_streams: Vec<StreamType>,
    /// Quality level
    pub quality: StreamQuality,
    /// Target frame rate
    pub target_fps: f32,
    /// Max bandwidth (bytes/sec, 0 = unlimited)
    pub max_bandwidth: u64,
    /// Enable compression
    pub compression: bool,
    /// Chunk size for large data
    pub chunk_size: u32,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            active_streams: vec![StreamType::Progress],
            quality: StreamQuality::Medium,
            target_fps: 30.0,
            max_bandwidth: 0,
            compression: true,
            chunk_size: 65536,
        }
    }
}

// ============================================================================
// Delta Encoding
// ============================================================================

/// Delta encoder for bandwidth efficiency
pub struct DeltaEncoder {
    last_positions: Vec<f32>,
    last_colors: Vec<f32>,
    last_sequence: u64,
    threshold: f32,
}

impl DeltaEncoder {
    pub fn new(threshold: f32) -> Self {
        Self {
            last_positions: Vec::new(),
            last_colors: Vec::new(),
            last_sequence: 0,
            threshold,
        }
    }
    
    /// Encode point cloud with delta compression
    pub fn encode_point_cloud(
        &mut self,
        positions: &[f32],
        colors: &[f32],
        sequence: u64,
    ) -> (Vec<u32>, Vec<f32>, Vec<f32>, bool) {
        // If no previous data or too old, send full update
        if self.last_positions.len() != positions.len() || sequence > self.last_sequence + 30 {
            self.last_positions = positions.to_vec();
            self.last_colors = colors.to_vec();
            self.last_sequence = sequence;
            return (Vec::new(), positions.to_vec(), colors.to_vec(), false);
        }
        
        // Find changed points
        let mut changed_indices = Vec::new();
        let mut changed_positions = Vec::new();
        let mut changed_colors = Vec::new();
        
        for i in 0..positions.len() / 3 {
            let pos_idx = i * 3;
            let dx = (positions[pos_idx] - self.last_positions[pos_idx]).abs();
            let dy = (positions[pos_idx + 1] - self.last_positions[pos_idx + 1]).abs();
            let dz = (positions[pos_idx + 2] - self.last_positions[pos_idx + 2]).abs();
            
            if dx > self.threshold || dy > self.threshold || dz > self.threshold {
                changed_indices.push(i as u32);
                changed_positions.extend_from_slice(&positions[pos_idx..pos_idx + 3]);
                changed_colors.extend_from_slice(&colors[pos_idx..pos_idx + 3]);
                
                self.last_positions[pos_idx] = positions[pos_idx];
                self.last_positions[pos_idx + 1] = positions[pos_idx + 1];
                self.last_positions[pos_idx + 2] = positions[pos_idx + 2];
                self.last_colors[pos_idx] = colors[pos_idx];
                self.last_colors[pos_idx + 1] = colors[pos_idx + 1];
                self.last_colors[pos_idx + 2] = colors[pos_idx + 2];
            }
        }
        
        self.last_sequence = sequence;
        
        (changed_indices, changed_positions, changed_colors, true)
    }
    
    /// Reset encoder state
    pub fn reset(&mut self) {
        self.last_positions.clear();
        self.last_colors.clear();
        self.last_sequence = 0;
    }
}

impl Default for DeltaEncoder {
    fn default() -> Self {
        Self::new(0.001)
    }
}

// ============================================================================
// Stream Builder (Utility)
// ============================================================================

/// Builder for stream packets
pub struct StreamPacketBuilder {
    sequence: u64,
    quality: StreamQuality,
}

impl StreamPacketBuilder {
    pub fn new(quality: StreamQuality) -> Self {
        Self { sequence: 0, quality }
    }
    
    /// Create point cloud packet
    pub fn point_cloud(&mut self, chunk: PointCloudChunk) -> StreamPacket {
        self.sequence += 1;
        StreamPacket {
            header: StreamHeader {
                sequence: self.sequence,
                timestamp: Self::now_ms(),
                stream_type: StreamType::PointCloud,
                quality: self.quality,
                is_delta: false,
                delta_ref: None,
            },
            payload: StreamPayload::PointCloud(chunk),
        }
    }
    
    /// Create gaussian packet
    pub fn gaussian(&mut self, chunk: GaussianChunk) -> StreamPacket {
        self.sequence += 1;
        StreamPacket {
            header: StreamHeader {
                sequence: self.sequence,
                timestamp: Self::now_ms(),
                stream_type: StreamType::Gaussian3D,
                quality: self.quality,
                is_delta: false,
                delta_ref: None,
            },
            payload: StreamPayload::Gaussian(chunk),
        }
    }
    
    /// Create mesh packet
    pub fn mesh(&mut self, chunk: MeshChunk) -> StreamPacket {
        self.sequence += 1;
        StreamPacket {
            header: StreamHeader {
                sequence: self.sequence,
                timestamp: Self::now_ms(),
                stream_type: StreamType::Mesh,
                quality: self.quality,
                is_delta: false,
                delta_ref: None,
            },
            payload: StreamPayload::Mesh(chunk),
        }
    }
    
    /// Create avatar frame packet
    pub fn avatar(&mut self, frame: AvatarFrame) -> StreamPacket {
        self.sequence += 1;
        StreamPacket {
            header: StreamHeader {
                sequence: self.sequence,
                timestamp: Self::now_ms(),
                stream_type: StreamType::Avatar,
                quality: self.quality,
                is_delta: !frame.is_keyframe,
                delta_ref: if frame.is_keyframe { None } else { Some(self.sequence - 1) },
            },
            payload: StreamPayload::Avatar(frame),
        }
    }
    
    /// Create progress packet
    pub fn progress(&mut self, update: ProgressUpdate) -> StreamPacket {
        self.sequence += 1;
        StreamPacket {
            header: StreamHeader {
                sequence: self.sequence,
                timestamp: Self::now_ms(),
                stream_type: StreamType::Progress,
                quality: self.quality,
                is_delta: false,
                delta_ref: None,
            },
            payload: StreamPayload::Progress(update),
        }
    }
    
    /// Create scene info packet
    pub fn scene_info(&mut self, info: SceneInfo) -> StreamPacket {
        self.sequence += 1;
        StreamPacket {
            header: StreamHeader {
                sequence: self.sequence,
                timestamp: Self::now_ms(),
                stream_type: StreamType::SceneInfo,
                quality: self.quality,
                is_delta: false,
                delta_ref: None,
            },
            payload: StreamPayload::Scene(info),
        }
    }
    
    /// Create heartbeat
    pub fn heartbeat(&mut self) -> StreamPacket {
        self.sequence += 1;
        StreamPacket {
            header: StreamHeader {
                sequence: self.sequence,
                timestamp: Self::now_ms(),
                stream_type: StreamType::SceneInfo,
                quality: self.quality,
                is_delta: false,
                delta_ref: None,
            },
            payload: StreamPayload::Heartbeat,
        }
    }
    
    fn now_ms() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

impl Default for StreamPacketBuilder {
    fn default() -> Self {
        Self::new(StreamQuality::Medium)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_quality_factors() {
        assert_eq!(StreamQuality::Low.density_factor(), 0.1);
        assert_eq!(StreamQuality::High.density_factor(), 1.0);
        assert_eq!(StreamQuality::Low.mesh_lod(), 3);
        assert_eq!(StreamQuality::Ultra.mesh_lod(), 0);
    }
    
    #[test]
    fn test_packet_builder() {
        let mut builder = StreamPacketBuilder::new(StreamQuality::High);
        
        let packet = builder.progress(ProgressUpdate {
            percent: 50.0,
            stage: "Processing".to_string(),
            sub_percent: 25.0,
            eta_seconds: Some(120.0),
            message: "Extracting features".to_string(),
            warnings: vec![],
        });
        
        assert_eq!(packet.header.sequence, 1);
        assert_eq!(packet.header.stream_type, StreamType::Progress);
    }
    
    #[test]
    fn test_delta_encoder() {
        let mut encoder = DeltaEncoder::new(0.01);
        
        let positions = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let colors = vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        
        // First call - full update
        let (indices, pos, col, is_delta) = encoder.encode_point_cloud(&positions, &colors, 1);
        assert!(!is_delta);
        assert!(indices.is_empty());
        assert_eq!(pos.len(), 6);
        
        // Second call with same data - no changes
        let (indices, pos, col, is_delta) = encoder.encode_point_cloud(&positions, &colors, 2);
        assert!(is_delta);
        assert!(indices.is_empty());
        assert!(pos.is_empty());
    }
}
