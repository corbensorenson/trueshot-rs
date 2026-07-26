use serde::{Deserialize, Serialize};
use config::{Config, File, ConfigError};
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub paths: PathConfig,
    pub photogrammetry: PhotogrammetryConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PathConfig {
    pub data_dir: PathBuf,
    pub temp_dir: PathBuf,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PhotogrammetryConfig {
    pub use_gpu: bool,
}

impl AppConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let builder = Config::builder()
            .set_default("server.host", "127.0.0.1")?
            .set_default("server.port", 3000)?
            .set_default("paths.data_dir", "./data")?
            .set_default("paths.temp_dir", "./tmp")?
            .set_default("photogrammetry.use_gpu", true)?
            .add_source(File::with_name("config").required(false))
            .add_source(config::Environment::with_prefix("TRUESHOT"));

        builder.build()?.try_deserialize()
    }
}
