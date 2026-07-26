use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProject {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub root_path: PathBuf,
    pub settings: ProjectSettings,
    pub status: ScanStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSettings {
    pub keep_raw: bool,
    pub keep_intermediates: bool,
    pub export_formats: Vec<ExportFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExportFormat {
    Gltf,
    Usd,
    Obj,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ScanStatus {
    New,
    Scanning,
    Processing,
    Completed,
    Archived,
}

impl ScanProject {
    pub fn new(name: &str, base_dir: &Path) -> Result<Self> {
        let id = Uuid::new_v4();
        let root_path = base_dir.join(format!("{}_{}", name.replace(" ", "_"), id));

        let project = Self {
            id,
            name: name.to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            root_path,
            settings: ProjectSettings {
                keep_raw: true,
                keep_intermediates: false, // Save space by default
                export_formats: vec![ExportFormat::Gltf, ExportFormat::Usd],
            },
            status: ScanStatus::New,
        };

        project.init_structure()?;
        project.save_manifest()?;

        Ok(project)
    }

    fn init_structure(&self) -> Result<()> {
        std::fs::create_dir_all(&self.root_path)?;
        std::fs::create_dir_all(self.root_path.join("raw/images"))?; // Raw captures
        std::fs::create_dir_all(self.root_path.join("processed/sfm"))?; // SfM workspace
        std::fs::create_dir_all(self.root_path.join("output"))?; // Final models
        std::fs::create_dir_all(self.root_path.join("logs"))?;
        Ok(())
    }

    pub fn save_manifest(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(self.root_path.join("project.json"), json)
            .context("Failed to save project manifest")?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path.join("project.json"))?;
        let project: ScanProject = serde_json::from_str(&content)?;
        Ok(project)
    }

    // --- Asset Management ---

    pub fn add_image(&self, source: &Path) -> Result<PathBuf> {
        let dest_dir = self.root_path.join("raw/images");
        let filename = source.file_name().context("Invalid source filename")?;
        let dest = dest_dir.join(filename);
        std::fs::copy(source, &dest)?;
        Ok(dest)
    }

    pub fn clean_intermediates(&self) -> Result<()> {
        if !self.settings.keep_intermediates {
            let p = self.root_path.join("processed");
            if p.exists() {
                std::fs::remove_dir_all(p)?;
                // Recreate empty wrapper
                std::fs::create_dir_all(self.root_path.join("processed/sfm"))?;
            }
        }
        Ok(())
    }
}
