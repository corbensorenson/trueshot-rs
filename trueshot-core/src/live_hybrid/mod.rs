//! TrueShot LiveHybrid: Adaptive Mesh/4DGS Streaming
//!
//! A hybrid real-time scene capture system that combines:
//! - 4D Gaussian Splatting for dynamic objects
//! - Progressive mesh conversion for static objects
//! - Avatar binding for tracked humans
//!
//! Features:
//! - Motion scoring for adaptive representation selection
//! - Marching cubes surface extraction (CPU + GPU)
//! - VRM-standard skeletal animation
//! - 478-point facial landmark detection
//! - Ultra-efficient streaming protocol (293x bandwidth reduction)
//!
//! See the whitepaper for full architecture details.

pub mod avatar;
pub mod facial_landmarks;
pub mod gpu_meshification;
pub mod meshification;
pub mod motion_score;
pub mod pipeline;
pub mod scene_graph;
pub mod segmentation;
pub mod streaming;
pub mod transitions;
pub mod unified_renderer;

// Re-exports
pub use avatar::{Avatar, AvatarTracker, BlendshapePreset, BoneName, BoundAvatar, Skeleton};
pub use facial_landmarks::{ARKitBlendshape, FaceDetection, FacialLandmarkDetector};
pub use gpu_meshification::GpuMeshifier;
pub use meshification::{MeshificationConfig, MeshificationPipeline, MeshificationResult};
pub use motion_score::{MotionClassification, MotionScorer};
pub use pipeline::{LiveHybridConfig, LiveHybridPipeline, PipelineStats};
pub use scene_graph::{HybridScene, HybridSceneNode, MeshData, ObjectRepresentation, Vertex};
pub use segmentation::{BoundingBox3D, ObjectSegmenter, SegmentedObject, TrackedObject};
pub use streaming::{StreamDecoder, StreamEncoder, StreamPacket};
pub use transitions::TransitionManager;
pub use unified_renderer::{UnifiedCamera, UnifiedFrame, UnifiedRenderer};
