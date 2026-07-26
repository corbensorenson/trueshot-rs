//! Bayer data cache for faster repeated access
//!
//! Caches decompressed Bayer data to avoid re-parsing NEF files.
//! Useful for streaming mode where frames are loaded multiple times.

use anyhow::Result;
use ndarray::Array3;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Cached Bayer frame data
#[derive(Clone)]
struct CachedBayer {
    data: Array3<f64>,
    size_bytes: usize,
}

/// Thread-safe Bayer data cache
pub struct BayerCache {
    cache: Arc<Mutex<HashMap<PathBuf, CachedBayer>>>,
    max_size_bytes: usize,
    current_size_bytes: Arc<Mutex<usize>>,
}

impl BayerCache {
    /// Create a new cache with specified maximum size in bytes
    pub fn new(max_size_mb: usize) -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            max_size_bytes: max_size_mb * 1024 * 1024,
            current_size_bytes: Arc::new(Mutex::new(0)),
        }
    }

    /// Get cached Bayer data if available
    pub fn get(&self, path: &PathBuf) -> Option<Array3<f64>> {
        let cache = self.cache.lock().unwrap();
        cache.get(path).map(|cached| cached.data.clone())
    }

    /// Store Bayer data in cache
    pub fn put(&self, path: PathBuf, data: Array3<f64>) -> Result<()> {
        let size_bytes = data.len() * std::mem::size_of::<f64>();

        // Check if we need to evict entries
        {
            let mut current_size = self.current_size_bytes.lock().unwrap();
            let mut cache = self.cache.lock().unwrap();

            // Simple LRU: if cache is full, clear it entirely
            // (More sophisticated LRU would track access times)
            if *current_size + size_bytes > self.max_size_bytes {
                tracing::info!(
                    "Bayer cache full ({} MB), clearing cache",
                    *current_size / 1024 / 1024
                );
                cache.clear();
                *current_size = 0;
            }

            // Add new entry
            cache.insert(
                path.clone(),
                CachedBayer {
                    data: data.clone(),
                    size_bytes,
                },
            );
            *current_size += size_bytes;

            tracing::debug!(
                "Cached Bayer data for {:?} ({} MB, total cache: {} MB)",
                path.file_name().unwrap_or_default(),
                size_bytes / 1024 / 1024,
                *current_size / 1024 / 1024
            );
        }

        Ok(())
    }

    /// Clear all cached data
    pub fn clear(&self) {
        let mut cache = self.cache.lock().unwrap();
        let mut current_size = self.current_size_bytes.lock().unwrap();
        cache.clear();
        *current_size = 0;
        tracing::info!("Bayer cache cleared");
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let cache = self.cache.lock().unwrap();
        let current_size = self.current_size_bytes.lock().unwrap();

        CacheStats {
            num_entries: cache.len(),
            size_bytes: *current_size,
            max_size_bytes: self.max_size_bytes,
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub num_entries: usize,
    pub size_bytes: usize,
    pub max_size_bytes: usize,
}

impl CacheStats {
    pub fn size_mb(&self) -> f64 {
        self.size_bytes as f64 / 1024.0 / 1024.0
    }

    pub fn max_size_mb(&self) -> f64 {
        self.max_size_bytes as f64 / 1024.0 / 1024.0
    }

    pub fn usage_percent(&self) -> f64 {
        if self.max_size_bytes == 0 {
            0.0
        } else {
            (self.size_bytes as f64 / self.max_size_bytes as f64) * 100.0
        }
    }
}

// Global cache instance
lazy_static::lazy_static! {
    static ref GLOBAL_BAYER_CACHE: BayerCache = BayerCache::new(2048); // 2 GB cache
}

/// Get the global Bayer cache
pub fn get_bayer_cache() -> &'static BayerCache {
    &GLOBAL_BAYER_CACHE
}

/// Load Bayer frame with caching
pub fn load_bayer_with_cache<F>(path: &PathBuf, loader: F) -> Result<Array3<f64>>
where
    F: FnOnce() -> Result<Array3<f64>>,
{
    let cache = get_bayer_cache();

    // Try cache first
    if let Some(cached) = cache.get(path) {
        tracing::debug!(
            "Bayer cache HIT for {:?}",
            path.file_name().unwrap_or_default()
        );
        return Ok(cached);
    }

    // Cache miss - load and cache
    tracing::debug!(
        "Bayer cache MISS for {:?}",
        path.file_name().unwrap_or_default()
    );
    let data = loader()?;
    cache.put(path.clone(), data.clone())?;

    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_basic() {
        let cache = BayerCache::new(100); // 100 MB

        let path = PathBuf::from("test.nef");
        let data = Array3::<f64>::zeros((100, 100, 1));

        // Put and get
        cache.put(path.clone(), data.clone()).unwrap();
        let retrieved = cache.get(&path).unwrap();

        assert_eq!(data.dim(), retrieved.dim());
    }

    #[test]
    fn test_cache_eviction() {
        let cache = BayerCache::new(1); // 1 MB - very small

        // Create large data that will exceed cache
        let data1 = Array3::<f64>::zeros((1000, 1000, 1)); // ~8 MB
        let data2 = Array3::<f64>::zeros((1000, 1000, 1)); // ~8 MB

        let path1 = PathBuf::from("test1.nef");
        let path2 = PathBuf::from("test2.nef");

        cache.put(path1.clone(), data1).unwrap();
        assert!(cache.get(&path1).is_some());

        // Adding data2 should evict data1
        cache.put(path2.clone(), data2).unwrap();
        assert!(cache.get(&path1).is_none()); // Evicted
        assert!(cache.get(&path2).is_some()); // Present
    }

    #[test]
    fn test_cache_stats() {
        let cache = BayerCache::new(100);

        let stats = cache.stats();
        assert_eq!(stats.num_entries, 0);
        assert_eq!(stats.size_bytes, 0);

        let data = Array3::<f64>::zeros((100, 100, 1));
        cache.put(PathBuf::from("test.nef"), data).unwrap();

        let stats = cache.stats();
        assert_eq!(stats.num_entries, 1);
        assert!(stats.size_bytes > 0);
    }
}
