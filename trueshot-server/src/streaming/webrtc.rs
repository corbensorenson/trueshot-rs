use anyhow::Result;
use tokio::sync::mpsc;
// Stub for WebRTC Streaming
// Real impl is 500+ lines.
// We expose the handle start.

pub async fn start_webrtc_stream(mut frame_rx: mpsc::Receiver<Vec<u8>>) -> Result<()> {
    // In real implementation:
    // 1. Create API, PeerConnection
    // 2. Create VideoTrack
    // 3. Loop: frame_rx.recv() -> write_sample()
    
    // For now we just drain the channel so it doesn't block
    tokio::spawn(async move {
        while let Some(_frame) = frame_rx.recv().await {
            // Drop
        }
    });
    
    Ok(())
}
