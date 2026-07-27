#[cfg(all(target_os = "linux", feature = "linux"))]
use tokio_uring::{buf::IoBuf, fs::File};

/// Writes a complete buffer with Linux `io_uring` acceleration when enabled.
///
/// The `linux` feature is opt-in because some Linux kernels, containers, and
/// sandbox policies do not permit `io_uring`. Other builds use Tokio's
/// portable filesystem implementation.
#[cfg(all(target_os = "linux", feature = "linux"))]
pub async fn write_raw_direct(path: &str, data: &[u8]) -> std::io::Result<()> {
    let path = path.to_owned();
    let data = data.to_vec();

    tokio::task::spawn_blocking(move || {
        tokio_uring::start(async move {
            let file = File::create(path).await?;
            let data_len = data.len();
            let mut data = data;
            let mut written = 0;

            while written < data_len {
                let pending = data.slice(written..data_len);
                let (result, pending) = file.write_at(pending, written as u64).await;
                data = pending.into_inner();

                let count = result?;
                if count == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "io_uring write made no progress",
                    ));
                }
                written += count;
            }

            file.close().await
        })
    })
    .await
    .map_err(|error| std::io::Error::other(format!("io_uring writer task failed: {error}")))?
}

#[cfg(not(all(target_os = "linux", feature = "linux")))]
pub async fn write_raw_direct(path: &str, data: &[u8]) -> std::io::Result<()> {
    tokio::fs::write(path, data).await
}

#[cfg(test)]
mod tests {
    use super::write_raw_direct;

    #[test]
    fn writes_the_complete_buffer() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("raw.bin");
        let expected: Vec<u8> = (0..=255).cycle().take(128 * 1024 + 17).collect();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("Tokio runtime");

        runtime
            .block_on(write_raw_direct(
                path.to_str().expect("UTF-8 test path"),
                &expected,
            ))
            .expect("write complete buffer");

        assert_eq!(std::fs::read(path).expect("read output"), expected);
    }
}
