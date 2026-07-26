use image::DynamicImage;
use anyhow::Result;
use std::path::PathBuf;

/// Complete Pipeline Interface
/// Wraps both Legacy and Hierarchical implementations
pub trait Pipeline {
    fn process_scan(&self, scan_id: &str) -> Result<()>;
    fn ingest_image(&self, img: DynamicImage) -> Result<()>;
}

pub struct StandardPipeline {
    root: PathBuf,
}

impl StandardPipeline {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl Pipeline for StandardPipeline {
    fn process_scan(&self, scan_id: &str) -> Result<()> {
        // ... Dispatch to native or heatmap modules
        Ok(())
    }
    
    fn ingest_image(&self, _img: DynamicImage) -> Result<()> {
        Ok(())
    }
}
