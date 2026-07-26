pub mod material;
pub mod naming;
pub mod segmentation;
pub mod model_manifest;
// pub mod splatting;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use once_cell::sync::Lazy;

/// Central Model Registry
/// Manages loading and caching of AI models to prevent OOM
use ort::session::Session;

// Actually, let's look at the error again: "could not find `Session` in `ort`".
// So I must find where it is. Common path: `ort::session::Session`.
// But wait, `ort` 2.0 might be `ort::execution::Session`? No.
// Let's try `ort::session::Session`.

pub struct ModelRegistry {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    metadata: Mutex<HashMap<String, ModelMetadata>>,
}

#[derive(Debug, Clone)]
pub struct ModelMetadata {
    pub model_id: String,
    pub model_version: String,
    pub weights_sha256: String,
}

impl ModelRegistry {
    pub fn instance() -> &'static Self {
        static INSTANCE: Lazy<ModelRegistry> = Lazy::new(|| ModelRegistry {
            sessions: Mutex::new(HashMap::new()),
            metadata: Mutex::new(HashMap::new()),
        });
        &INSTANCE
    }
    
    // Stub for lazy loading logic
    pub fn get_session(&self, model_key: &str) -> Option<Arc<Session>> {
        self.sessions.lock().unwrap().get(model_key).cloned()
    }

    pub fn register_model_metadata(&self, model_key: &str, metadata: ModelMetadata) {
        self.metadata
            .lock()
            .unwrap()
            .insert(model_key.to_string(), metadata);
    }

    pub fn model_metadata(&self, model_key: &str) -> Option<ModelMetadata> {
        self.metadata.lock().unwrap().get(model_key).cloned()
    }
}
