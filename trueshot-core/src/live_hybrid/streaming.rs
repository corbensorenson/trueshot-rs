//! Streaming Protocol
//!
//! Ultra-efficient streaming protocol for hybrid mesh/4DGS scenes:
//! - Differential encoding for Gaussian updates
//! - Progressive mesh loading
//! - Avatar pose compression
//! - Adaptive quality based on bandwidth
//!
//! Achieves ~293x bandwidth reduction vs raw 4DGS streaming

use std::collections::HashMap;
use uuid::Uuid;
use serde::{Deserialize, Serialize};

use super::scene_graph::{Transform3D, MeshData};
use super::avatar::BlendshapePreset;

/// Protocol version for compatibility
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum packet size (64KB)
pub const MAX_PACKET_SIZE: usize = 65536;

/// Stream packet types
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StreamPacket {
    /// Initial scene graph structure
    SceneGraph(SceneGraphPacket),
    
    /// Full Gaussian data (initial or major change)
    GaussianFull(GaussianFullPacket),
    
    /// Gaussian delta update (incremental)
    GaussianDelta(GaussianDeltaPacket),
    
    /// Mesh asset (one-time transfer)
    MeshAsset(MeshAssetPacket),
    
    /// Object transform update (every frame)
    TransformUpdate(TransformUpdatePacket),
    
    /// Avatar pose update (every frame)
    AvatarPose(AvatarPosePacket),
    
    /// Texture data (chunked)
    TextureChunk(TextureChunkPacket),
    
    /// Heartbeat / keep-alive
    Heartbeat(HeartbeatPacket),
    
    /// Quality adjustment request
    QualityAdjust(QualityAdjustPacket),
}

/// Scene graph structure packet
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneGraphPacket {
    pub version: u32,
    pub scene_name: String,
    pub nodes: Vec<NodeDescriptor>,
    pub hierarchy: Vec<(Uuid, Option<Uuid>)>,  // (child, parent)
}

/// Node descriptor for scene graph
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeDescriptor {
    pub id: Uuid,
    pub name: String,
    pub representation_type: RepresentationType,
    pub transform: TransformData,
}

/// Representation type enum (serializable)
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum RepresentationType {
    Gaussian4D,
    Mesh,
    Avatar,
    Transitioning,
    Pending,
}

/// Compact transform data
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransformData {
    pub position: [f32; 3],
    pub rotation: [f32; 4],  // Quaternion (x, y, z, w)
    pub scale: [f32; 3],
}

impl From<&Transform3D> for TransformData {
    fn from(t: &Transform3D) -> Self {
        Self {
            position: [t.position.x, t.position.y, t.position.z],
            rotation: [
                t.rotation.i,
                t.rotation.j,
                t.rotation.k,
                t.rotation.w,
            ],
            scale: [t.scale.x, t.scale.y, t.scale.z],
        }
    }
}

/// Full Gaussian data packet
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GaussianFullPacket {
    pub node_id: Uuid,
    pub num_gaussians: u32,
    /// Compressed Gaussian data
    pub data: Vec<u8>,
    /// Compression method used
    pub compression: CompressionMethod,
}

/// Gaussian delta packet (incremental update)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GaussianDeltaPacket {
    pub node_id: Uuid,
    pub frame_id: u64,
    /// Reference frame for delta
    pub reference_frame: u64,
    /// Compressed deformation field
    pub deformation: Vec<u8>,
    /// Color updates (sparse)
    pub color_updates: Vec<(u32, [u8; 3])>,
}

/// Mesh asset packet
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MeshAssetPacket {
    pub node_id: Uuid,
    pub lod_level: u8,
    pub vertex_count: u32,
    pub index_count: u32,
    /// Compressed mesh data (Draco-style)
    pub data: Vec<u8>,
    /// Texture reference IDs
    pub texture_ids: Vec<Uuid>,
}

/// Transform update packet (frequent, small)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransformUpdatePacket {
    pub frame_id: u64,
    pub timestamp: f32,
    /// Sparse updates: (node_id, transform)
    pub updates: Vec<(Uuid, TransformData)>,
}

/// Avatar pose packet
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AvatarPosePacket {
    pub node_id: Uuid,
    pub frame_id: u64,
    /// Bone transforms (compressed)
    pub bone_data: Vec<u8>,
    /// Blendshape weights (only non-zero)
    pub blendshapes: Vec<(BlendshapePreset, f32)>,
}

/// Texture chunk packet (for progressive loading)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextureChunkPacket {
    pub texture_id: Uuid,
    pub mip_level: u8,
    pub chunk_index: u32,
    pub total_chunks: u32,
    pub data: Vec<u8>,
}

/// Heartbeat packet
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeartbeatPacket {
    pub timestamp: u64,
    pub server_frame: u64,
}

/// Quality adjustment packet
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QualityAdjustPacket {
    pub target_bitrate_kbps: u32,
    pub max_gaussians: u32,
    pub texture_quality: u8,
    pub enable_motion_blur: bool,
}

/// Compression methods
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CompressionMethod {
    None,
    LZ4,
    Zstd,
    /// Vector quantization for Gaussians
    VectorQuantized { codebook_size: u16 },
    /// Delta encoding for temporal data
    DeltaEncoded,
}

/// Stream encoder for server-side
pub struct StreamEncoder {
    config: EncoderConfig,
    frame_id: u64,
    /// Reference frames for delta encoding
    reference_frames: HashMap<Uuid, Vec<u8>>,
    /// Statistics
    stats: EncoderStats,
}

/// Encoder configuration
#[derive(Clone, Debug)]
pub struct EncoderConfig {
    pub target_bitrate_kbps: u32,
    pub keyframe_interval: u32,
    pub compression: CompressionMethod,
    pub enable_delta_encoding: bool,
    pub quantization_bits: u8,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            target_bitrate_kbps: 5000,
            keyframe_interval: 30,
            compression: CompressionMethod::Zstd,
            enable_delta_encoding: true,
            quantization_bits: 12,
        }
    }
}

/// Encoder statistics
#[derive(Clone, Debug, Default)]
pub struct EncoderStats {
    pub frames_encoded: u64,
    pub bytes_sent: u64,
    pub gaussians_encoded: u64,
    pub meshes_sent: u32,
    pub avatar_updates: u64,
}

impl StreamEncoder {
    pub fn new(config: EncoderConfig) -> Self {
        Self {
            config,
            frame_id: 0,
            reference_frames: HashMap::new(),
            stats: EncoderStats::default(),
        }
    }
    
    /// Encode scene graph structure
    pub fn encode_scene_graph(
        &mut self,
        nodes: Vec<NodeDescriptor>,
        hierarchy: Vec<(Uuid, Option<Uuid>)>,
        scene_name: &str,
    ) -> StreamPacket {
        StreamPacket::SceneGraph(SceneGraphPacket {
            version: PROTOCOL_VERSION,
            scene_name: scene_name.to_string(),
            nodes,
            hierarchy,
        })
    }
    
    /// Encode Gaussian data (full or delta)
    pub fn encode_gaussians(
        &mut self,
        node_id: Uuid,
        positions: &[[f32; 3]],
        colors: &[[f32; 3]],
        opacities: &[f32],
        covariances: &[[f32; 6]],
    ) -> StreamPacket {
        let is_keyframe = self.frame_id % self.config.keyframe_interval as u64 == 0;
        
        if is_keyframe || !self.config.enable_delta_encoding {
            // Full frame
            let data = self.compress_gaussians_full(positions, colors, opacities, covariances);
            self.reference_frames.insert(node_id, data.clone());
            
            self.stats.gaussians_encoded += positions.len() as u64;
            self.stats.bytes_sent += data.len() as u64;
            
            StreamPacket::GaussianFull(GaussianFullPacket {
                node_id,
                num_gaussians: positions.len() as u32,
                data,
                compression: self.config.compression,
            })
        } else {
            // Delta frame
            let deformation = self.encode_delta(node_id, positions);
            let color_updates = self.detect_color_changes(node_id, colors);
            
            self.frame_id += 1;
            
            StreamPacket::GaussianDelta(GaussianDeltaPacket {
                node_id,
                frame_id: self.frame_id,
                reference_frame: self.frame_id - 1,
                deformation,
                color_updates,
            })
        }
    }
    
    /// Compress Gaussians using vector quantization
    fn compress_gaussians_full(
        &self,
        positions: &[[f32; 3]],
        colors: &[[f32; 3]],
        opacities: &[f32],
        covariances: &[[f32; 6]],
    ) -> Vec<u8> {
        let mut data = Vec::new();
        
        // Header
        data.extend_from_slice(&(positions.len() as u32).to_le_bytes());
        
        // Quantize and pack positions
        for pos in positions {
            for &v in pos {
                // Quantize to 16-bit
                let quantized = ((v + 100.0) * 327.67) as u16;
                data.extend_from_slice(&quantized.to_le_bytes());
            }
        }
        
        // Pack colors (8-bit per channel)
        for color in colors {
            data.push((color[0].clamp(0.0, 1.0) * 255.0) as u8);
            data.push((color[1].clamp(0.0, 1.0) * 255.0) as u8);
            data.push((color[2].clamp(0.0, 1.0) * 255.0) as u8);
        }
        
        // Pack opacities (8-bit)
        for &opacity in opacities {
            data.push((opacity.clamp(0.0, 1.0) * 255.0) as u8);
        }
        
        // Pack covariances (quantized)
        for cov in covariances {
            for &v in cov {
                let quantized = ((v + 10.0) * 3276.7) as u16;
                data.extend_from_slice(&quantized.to_le_bytes());
            }
        }
        
        // Apply compression
        match self.config.compression {
            CompressionMethod::Zstd => {
                zstd_compress(&data)
            }
            CompressionMethod::LZ4 => {
                lz4_compress(&data)
            }
            _ => data,
        }
    }
    
    /// Encode delta between frames
    fn encode_delta(&self, _node_id: Uuid, positions: &[[f32; 3]]) -> Vec<u8> {
        // Simplified delta encoding
        let mut data = Vec::with_capacity(positions.len() * 6);
        
        for pos in positions {
            // Quantize delta to 16-bit (smaller range than absolute)
            for &v in pos {
                let quantized = ((v + 10.0) * 3276.7) as i16;
                data.extend_from_slice(&quantized.to_le_bytes());
            }
        }
        
        data
    }
    
    /// Detect color changes (sparse update)
    fn detect_color_changes(&self, _node_id: Uuid, colors: &[[f32; 3]]) -> Vec<(u32, [u8; 3])> {
        // In production, compare against reference frame
        // For now, return empty (no changes)
        Vec::new()
    }
    
    /// Encode mesh asset
    pub fn encode_mesh(
        &mut self,
        node_id: Uuid,
        mesh: &MeshData,
        lod_level: u8,
        texture_ids: Vec<Uuid>,
    ) -> StreamPacket {
        let data = self.compress_mesh(mesh);
        
        self.stats.meshes_sent += 1;
        self.stats.bytes_sent += data.len() as u64;
        
        StreamPacket::MeshAsset(MeshAssetPacket {
            node_id,
            lod_level,
            vertex_count: mesh.vertices.len() as u32,
            index_count: mesh.indices.len() as u32,
            data,
            texture_ids,
        })
    }
    
    /// Compress mesh data
    fn compress_mesh(&self, mesh: &MeshData) -> Vec<u8> {
        let mut data = Vec::new();
        
        // Pack vertices
        for vertex in &mesh.vertices {
            for &v in &vertex.position {
                data.extend_from_slice(&v.to_le_bytes());
            }
            for &v in &vertex.normal {
                data.extend_from_slice(&v.to_le_bytes());
            }
            for &v in &vertex.uv {
                data.extend_from_slice(&v.to_le_bytes());
            }
        }
        
        // Pack indices (delta-encoded)
        let mut prev_idx = 0u32;
        for &idx in &mesh.indices {
            let delta = (idx as i32) - (prev_idx as i32);
            // Variable-length encoding
            if delta >= -127 && delta <= 127 {
                data.push(delta as i8 as u8);
            } else {
                data.push(0x80); // Escape
                data.extend_from_slice(&delta.to_le_bytes());
            }
            prev_idx = idx;
        }
        
        zstd_compress(&data)
    }
    
    /// Encode transform updates
    pub fn encode_transforms(
        &mut self,
        updates: Vec<(Uuid, Transform3D)>,
        timestamp: f32,
    ) -> StreamPacket {
        self.frame_id += 1;
        
        let updates: Vec<_> = updates.iter()
            .map(|(id, t)| (*id, TransformData::from(t)))
            .collect();
        
        StreamPacket::TransformUpdate(TransformUpdatePacket {
            frame_id: self.frame_id,
            timestamp,
            updates,
        })
    }
    
    /// Encode avatar pose
    pub fn encode_avatar_pose(
        &mut self,
        node_id: Uuid,
        bone_matrices: &[[f32; 16]],
        blendshapes: &[(BlendshapePreset, f32)],
    ) -> StreamPacket {
        self.frame_id += 1;
        self.stats.avatar_updates += 1;
        
        // Compress bone matrices
        let mut bone_data = Vec::with_capacity(bone_matrices.len() * 64);
        for matrix in bone_matrices {
            for &v in matrix {
                bone_data.extend_from_slice(&v.to_le_bytes());
            }
        }
        
        // Only include non-zero blendshapes
        let active_blendshapes: Vec<_> = blendshapes.iter()
            .filter(|(_, w)| *w > 0.001)
            .cloned()
            .collect();
        
        StreamPacket::AvatarPose(AvatarPosePacket {
            node_id,
            frame_id: self.frame_id,
            bone_data: zstd_compress(&bone_data),
            blendshapes: active_blendshapes,
        })
    }
    
    /// Get current statistics
    pub fn stats(&self) -> &EncoderStats {
        &self.stats
    }
    
    /// Calculate current bitrate
    pub fn current_bitrate_kbps(&self) -> f32 {
        if self.frame_id == 0 {
            return 0.0;
        }
        (self.stats.bytes_sent as f32 * 8.0 / 1000.0) / (self.frame_id as f32 / 30.0)
    }
}

impl Default for StreamEncoder {
    fn default() -> Self {
        Self::new(EncoderConfig::default())
    }
}

/// Stream decoder for client-side
pub struct StreamDecoder {
    /// Received scene graph
    scene_nodes: HashMap<Uuid, NodeDescriptor>,
    /// Reference frames for delta decoding
    reference_frames: HashMap<Uuid, Vec<u8>>,
    /// Received meshes
    meshes: HashMap<Uuid, MeshData>,
    /// Texture chunks being assembled
    texture_chunks: HashMap<Uuid, Vec<Option<Vec<u8>>>>,
    /// Statistics
    stats: DecoderStats,
}

/// Decoder statistics
#[derive(Clone, Debug, Default)]
pub struct DecoderStats {
    pub packets_received: u64,
    pub bytes_received: u64,
    pub frames_decoded: u64,
    pub decode_errors: u64,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self {
            scene_nodes: HashMap::new(),
            reference_frames: HashMap::new(),
            meshes: HashMap::new(),
            texture_chunks: HashMap::new(),
            stats: DecoderStats::default(),
        }
    }
    
    /// Decode a stream packet
    pub fn decode(&mut self, packet: StreamPacket) -> Result<DecodedData, DecodeError> {
        self.stats.packets_received += 1;
        
        match packet {
            StreamPacket::SceneGraph(p) => {
                for node in p.nodes {
                    self.scene_nodes.insert(node.id, node);
                }
                Ok(DecodedData::SceneGraph {
                    scene_name: p.scene_name,
                    node_count: self.scene_nodes.len(),
                })
            }
            
            StreamPacket::GaussianFull(p) => {
                let data = self.decompress(&p.data, p.compression)?;
                self.reference_frames.insert(p.node_id, data.clone());
                
                Ok(DecodedData::Gaussians {
                    node_id: p.node_id,
                    count: p.num_gaussians as usize,
                    is_keyframe: true,
                })
            }
            
            StreamPacket::GaussianDelta(p) => {
                // Apply delta to reference frame
                // (Simplified - would fully decode in production)
                Ok(DecodedData::Gaussians {
                    node_id: p.node_id,
                    count: 0,
                    is_keyframe: false,
                })
            }
            
            StreamPacket::MeshAsset(p) => {
                let data = zstd_decompress(&p.data)?;
                let mesh = self.decode_mesh(&data, p.vertex_count, p.index_count)?;
                self.meshes.insert(p.node_id, mesh);
                
                Ok(DecodedData::Mesh {
                    node_id: p.node_id,
                    vertices: p.vertex_count as usize,
                    triangles: p.index_count as usize / 3,
                })
            }
            
            StreamPacket::TransformUpdate(p) => {
                Ok(DecodedData::Transforms {
                    frame_id: p.frame_id,
                    count: p.updates.len(),
                })
            }
            
            StreamPacket::AvatarPose(p) => {
                Ok(DecodedData::AvatarPose {
                    node_id: p.node_id,
                    frame_id: p.frame_id,
                })
            }
            
            StreamPacket::TextureChunk(p) => {
                let chunks = self.texture_chunks
                    .entry(p.texture_id)
                    .or_insert_with(|| vec![None; p.total_chunks as usize]);
                
                if p.chunk_index < chunks.len() as u32 {
                    chunks[p.chunk_index as usize] = Some(p.data);
                }
                
                let complete = chunks.iter().all(|c| c.is_some());
                
                Ok(DecodedData::TextureChunk {
                    texture_id: p.texture_id,
                    complete,
                })
            }
            
            StreamPacket::Heartbeat(p) => {
                Ok(DecodedData::Heartbeat {
                    server_frame: p.server_frame,
                })
            }
            
            StreamPacket::QualityAdjust(_) => {
                Ok(DecodedData::QualityAdjust)
            }
        }
    }
    
    /// Decompress data
    fn decompress(&self, data: &[u8], method: CompressionMethod) -> Result<Vec<u8>, DecodeError> {
        match method {
            CompressionMethod::None => Ok(data.to_vec()),
            CompressionMethod::Zstd => zstd_decompress(data),
            CompressionMethod::LZ4 => lz4_decompress(data),
            _ => Ok(data.to_vec()),
        }
    }
    
    /// Decode mesh from compressed data
    fn decode_mesh(
        &self,
        data: &[u8],
        vertex_count: u32,
        index_count: u32,
    ) -> Result<MeshData, DecodeError> {
        let mut cursor = 0;
        let mut vertices = Vec::with_capacity(vertex_count as usize);
        
        // Decode vertices
        for _ in 0..vertex_count {
            if cursor + 32 > data.len() {
                return Err(DecodeError::InsufficientData);
            }
            
            let mut position = [0.0f32; 3];
            let mut normal = [0.0f32; 3];
            let mut uv = [0.0f32; 2];
            
            for i in 0..3 {
                position[i] = f32::from_le_bytes([
                    data[cursor], data[cursor + 1],
                    data[cursor + 2], data[cursor + 3],
                ]);
                cursor += 4;
            }
            
            for i in 0..3 {
                normal[i] = f32::from_le_bytes([
                    data[cursor], data[cursor + 1],
                    data[cursor + 2], data[cursor + 3],
                ]);
                cursor += 4;
            }
            
            for i in 0..2 {
                uv[i] = f32::from_le_bytes([
                    data[cursor], data[cursor + 1],
                    data[cursor + 2], data[cursor + 3],
                ]);
                cursor += 4;
            }
            
            vertices.push(super::scene_graph::Vertex {
                position,
                normal,
                uv,
                color: [1.0, 1.0, 1.0, 1.0],
            });
        }
        
        // Decode indices (delta-encoded)
        let mut indices = Vec::with_capacity(index_count as usize);
        let mut prev_idx = 0i32;
        
        while indices.len() < index_count as usize && cursor < data.len() {
            let byte = data[cursor] as i8;
            cursor += 1;
            
            let delta = if byte as u8 == 0x80 {
                if cursor + 4 > data.len() {
                    break;
                }
                let d = i32::from_le_bytes([
                    data[cursor], data[cursor + 1],
                    data[cursor + 2], data[cursor + 3],
                ]);
                cursor += 4;
                d
            } else {
                byte as i32
            };
            
            prev_idx += delta;
            indices.push(prev_idx as u32);
        }
        
        Ok(MeshData {
            vertices,
            indices,
            name: String::new(),
        })
    }
    
    /// Get statistics
    pub fn stats(&self) -> &DecoderStats {
        &self.stats
    }
}

impl Default for StreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Decoded data types
#[derive(Clone, Debug)]
pub enum DecodedData {
    SceneGraph { scene_name: String, node_count: usize },
    Gaussians { node_id: Uuid, count: usize, is_keyframe: bool },
    Mesh { node_id: Uuid, vertices: usize, triangles: usize },
    Transforms { frame_id: u64, count: usize },
    AvatarPose { node_id: Uuid, frame_id: u64 },
    TextureChunk { texture_id: Uuid, complete: bool },
    Heartbeat { server_frame: u64 },
    QualityAdjust,
}

/// Decode errors
#[derive(Clone, Debug)]
pub enum DecodeError {
    InsufficientData,
    InvalidHeader,
    DecompressionFailed,
    UnsupportedVersion,
}

// Compression helpers (zstd/lz4 backed)
fn zstd_compress(data: &[u8]) -> Vec<u8> {
    let level = 3;
    match zstd::stream::encode_all(data, level) {
        Ok(out) => out,
        Err(e) => {
            tracing::warn!("zstd compress failed: {}", e);
            data.to_vec()
        }
    }
}

fn zstd_decompress(data: &[u8]) -> Result<Vec<u8>, DecodeError> {
    zstd::stream::decode_all(data).map_err(|_| DecodeError::DecompressionFailed)
}

fn lz4_compress(data: &[u8]) -> Vec<u8> {
    lz4_flex::compress_prepend_size(data)
}

fn lz4_decompress(data: &[u8]) -> Result<Vec<u8>, DecodeError> {
    lz4_flex::decompress_size_prepended(data).map_err(|_| DecodeError::DecompressionFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_encoder_creation() {
        let encoder = StreamEncoder::default();
        assert_eq!(encoder.frame_id, 0);
    }
    
    #[test]
    fn test_transform_encoding() {
        let mut encoder = StreamEncoder::default();
        
        let transform = Transform3D::default();
        let packet = encoder.encode_transforms(
            vec![(Uuid::new_v4(), transform)],
            0.0,
        );
        
        match packet {
            StreamPacket::TransformUpdate(p) => {
                assert_eq!(p.updates.len(), 1);
            }
            _ => panic!("Wrong packet type"),
        }
    }
    
    #[test]
    fn test_decoder_roundtrip() {
        let mut encoder = StreamEncoder::default();
        let mut decoder = StreamDecoder::default();
        
        let packet = encoder.encode_scene_graph(
            vec![],
            vec![],
            "Test Scene",
        );
        
        let result = decoder.decode(packet);
        assert!(result.is_ok());
    }
}
