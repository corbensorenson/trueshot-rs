//! Streaming module
//! 
//! Real-time video and data streaming endpoints:
//! - MJPEG streaming for camera feeds
//! - WebRTC for low-latency video
//! - LiveHybrid WebSocket for hybrid scene streaming

pub mod mjpeg;
pub mod webrtc;
pub mod livehybrid_ws;

pub use livehybrid_ws::{
    StreamingSessionManager,
    StreamingConfig,
    StreamingRouterConfig,
    StreamQuality,
    create_streaming_router,
    create_streaming_router_with_config,
};
