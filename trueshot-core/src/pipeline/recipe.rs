use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct PipelineRecipe {
    pub name: String,
    pub steps: Vec<PipelineStep>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PipelineStep {
    Capture {
        angles: Vec<f32>,
        exposure: u32,
    },
    Process {
        method: String, // "native", "heatmap"
    },
    Export {
        format: String,
    },
}

impl PipelineRecipe {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let recipe = serde_yaml::from_str(&content)?;
        Ok(recipe)
    }
}
