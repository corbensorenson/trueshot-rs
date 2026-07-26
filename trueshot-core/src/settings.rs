use serde::Deserialize;
use config::{Config, ConfigError, File};
use std::sync::RwLock;

lazy_static::lazy_static! {
    pub static ref SETTINGS: RwLock<Settings> = RwLock::new(Settings::new().unwrap());
}

#[derive(Debug, Deserialize, Clone)]
pub struct Settings {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub vision: VisionConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StorageConfig {
    pub root_dir: String,
    pub archive_dir: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct VisionConfig {
    pub model_dir: String,
    pub use_gpu: bool,
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let s = Config::builder()
            // Start with default
            .set_default("server.host", "127.0.0.1")?
            .set_default("server.port", 3000)?
            .set_default("storage.root_dir", "./data")?
            .set_default("storage.archive_dir", "./archive")?
            .set_default("vision.model_dir", "./models")?
            .set_default("vision.use_gpu", true)?
            // Merge file
            .add_source(File::with_name("trueshot").required(false))
            // Merge env
            .add_source(config::Environment::with_prefix("TRUESHOT"))
            .build()?;

        s.try_deserialize()
    }
}
