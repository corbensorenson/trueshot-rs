use std::sync::Arc;
use tokio::sync::Semaphore;

pub struct GpuScheduler {
    semaphore: Arc<Semaphore>,
}

impl GpuScheduler {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
        }
    }
    
    pub async fn run_job<F, T>(&self, job: F) -> T 
    where F: std::future::Future<Output = T> 
    {
        let _permit = self.semaphore.acquire().await.unwrap();
        job.await
    }
}
