#[cfg(target_os = "linux")]
use tokio_uring::fs::File;

/// Direct IO Writer for NVMe SSDs
#[cfg(target_os = "linux")]
pub async fn write_raw_direct(path: &str, data: &[u8]) -> std::io::Result<()> {
    tokio_uring::start(async {
        let file = File::create(path).await?;
        let (res, _buf) = file.write_all_at(data, 0).await;
        res
    })
}

#[cfg(not(target_os = "linux"))]
pub async fn write_raw_direct(path: &str, data: &[u8]) -> std::io::Result<()> {
    // Fallback for macOS/Windows
    tokio::fs::write(path, data).await
}
