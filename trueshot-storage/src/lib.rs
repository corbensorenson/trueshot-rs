pub mod direct;
pub mod estimator;
pub mod octree;
pub mod sidecar;
pub mod tiering;

use anyhow::Result;
use std::path::PathBuf;

/// Unified Asset Manager
pub trait AssetManager {
    fn store_raw(&self, data: &[u8], name: &str) -> Result<PathBuf>;
    fn retrieve_asset(&self, id: &str) -> Result<Vec<u8>>;
    fn archive_project(&self, project_id: &str) -> Result<()>;
}

pub struct LocalAssetManager {
    root: PathBuf,
}

impl LocalAssetManager {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl AssetManager for LocalAssetManager {
    fn store_raw(&self, data: &[u8], name: &str) -> Result<PathBuf> {
        let path = self.root.join("raw").join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, data)?;
        Ok(path)
    }
    fn retrieve_asset(&self, id: &str) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.root.join(id))?)
    }
    fn archive_project(&self, project_id: &str) -> Result<()> {
        Ok(tiering::tier_project(
            &self.root.join(project_id),
            &self.root.join("archive"),
        )?)
    }
}
