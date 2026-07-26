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

pub mod scene_graph;
pub mod motion_score;
pub mod segmentation;
pub mod unified_renderer;
pub mod transitions;
pub mod meshification;
pub mod gpu_meshification;
pub mod avatar;
pub mod facial_landmarks;
pub mod streaming;
pub mod pipeline;

// Re-exports
pub use scene_graph::{HybridScene, HybridSceneNode, ObjectRepresentation, MeshData, Vertex};
pub use motion_score::{MotionScorer, MotionClassification};
pub use segmentation::{ObjectSegmenter, SegmentedObject, TrackedObject, BoundingBox3D};
pub use unified_renderer::{UnifiedRenderer, UnifiedCamera, UnifiedFrame};
pub use transitions::TransitionManager;
pub use meshification::{MeshificationPipeline, MeshificationConfig, MeshificationResult};
pub use gpu_meshification::GpuMeshifier;
pub use avatar::{Avatar, AvatarTracker, BoundAvatar, Skeleton, BoneName, BlendshapePreset};
pub use facial_landmarks::{FacialLandmarkDetector, FaceDetection, ARKitBlendshape};
pub use streaming::{StreamEncoder, StreamDecoder, StreamPacket};
pub use pipeline::{LiveHybridPipeline, LiveHybridConfig, PipelineStats};
