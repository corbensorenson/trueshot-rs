use crate::reconstruction::livescan::PosePriors;
use crate::reconstruction::unified::UnifiedReconstruction;
use crate::scheduler::Job;
use crate::scheduler::RemoteJobPayload;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use tokio::sync::mpsc;

pub enum UnifiedJobType {
    GaussianSplatting,
    Photogrammetry,
}

pub struct UnifiedJob {
    pub workspace_path: PathBuf,
    pub livescan_path: Option<PathBuf>,
    pub dslr_path: Option<PathBuf>,
    pub job_type: UnifiedJobType,
}

impl UnifiedJob {
    pub fn new(workspace_path: PathBuf, job_type: UnifiedJobType) -> Self {
        Self {
            workspace_path,
            livescan_path: None,
            dslr_path: None,
            job_type,
        }
    }

    pub fn with_livescan(mut self, path: PathBuf) -> Self {
        self.livescan_path = Some(path);
        self
    }

    pub fn with_dslr(mut self, path: PathBuf) -> Self {
        self.dslr_path = Some(path);
        self
    }
}

#[async_trait]
impl Job for UnifiedJob {
    fn name(&self) -> &str {
        match self.job_type {
            UnifiedJobType::GaussianSplatting => "Unified 3DGS Reconstruction",
            UnifiedJobType::Photogrammetry => "Unified Photogrammetry",
        }
    }

    async fn execute(&self, progress_tx: mpsc::Sender<f32>) -> Result<()> {
        let _ = progress_tx.send(0.05).await;

        // Check paths
        let unified = UnifiedReconstruction::new(self.workspace_path.clone());

        // 1. Synchronize (if DSLR + LiveScan)
        let mut priors: Option<PosePriors> = None;
        if let (Some(dslr), Some(livescan)) = (&self.dslr_path, &self.livescan_path) {
            let _ = progress_tx.send(0.1).await;
            tracing::info!("🔄 Synchronizing DSLR images from {:?}...", dslr);

            // Run sync logic
            // Ideally `unified` methods should be async or we wrap in blocking?
            // `synchronize_dslr_images` is synchronous.
            let unified_clone = UnifiedReconstruction::new(self.workspace_path.clone()); // lightweight
            let dslr_path = dslr.clone();
            let livescan_path = livescan.clone();

            let reconstruction = tokio::task::spawn_blocking(move || {
                unified_clone.synchronize_dslr_images(&dslr_path, &livescan_path)
            })
            .await??;
            priors = Some(reconstruction);
        }

        let _ = progress_tx.send(0.2).await;

        let unified_clone = UnifiedReconstruction::new(self.workspace_path.clone());
        let livescan_clone = self.livescan_path.clone();
        let priors_clone = priors.clone();
        let job_type = match self.job_type {
            UnifiedJobType::GaussianSplatting => {
                crate::reconstruction::pipeline::ReconstructionType::GaussianSplatting
            }
            UnifiedJobType::Photogrammetry => {
                crate::reconstruction::pipeline::ReconstructionType::PhotogrammetryHighQuality
            }
        };

        // We need to call the pipeline manually because `UnifiedReconstruction::process_...` are specific helpers.
        // Or I can call `process_gaussian_splatting`.

        tokio::task::spawn_blocking(move || -> Result<()> {
            match job_type {
                crate::reconstruction::pipeline::ReconstructionType::GaussianSplatting => {
                    unified_clone.process_gaussian_splatting(livescan_clone, priors_clone)?;
                }
                _ => {
                    unified_clone.process_photogrammetry(
                        "high".to_string(),
                        livescan_clone,
                        priors_clone,
                    )?;
                }
            }
            Ok(())
        })
        .await??;

        let _ = progress_tx.send(1.0).await;
        Ok(())
    }

    fn remote_payload(&self) -> Option<RemoteJobPayload> {
        let kind = match self.job_type {
            UnifiedJobType::GaussianSplatting => "unified_gaussian_splatting",
            UnifiedJobType::Photogrammetry => "unified_photogrammetry",
        };

        Some(RemoteJobPayload {
            kind: kind.to_string(),
            name: self.name().to_string(),
            payload: json!({
                "workspace_path": self.workspace_path,
                "livescan_path": self.livescan_path,
                "dslr_path": self.dslr_path,
                "job_type": match self.job_type {
                    UnifiedJobType::GaussianSplatting => "gaussian_splatting",
                    UnifiedJobType::Photogrammetry => "photogrammetry",
                }
            }),
        })
    }
}
