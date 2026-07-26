use tokio::task;

/// Async wrapper for Rayon
/// Allows running CPU-bound tasks without blocking the async runtime
pub async fn run_cpu_task<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    // Use tokio's spawn_blocking which uses a dedicated thread pool
    // For heavy parallelism, one might configure a custom Rayon pool here
    task::spawn_blocking(move || {
        f()
    }).await.expect("CPU Task Failed")
}
