use anyhow::Result;
use image::RgbImage;
use nalgebra as na;

use crate::gaussian_splatting::{Camera, GaussianCloud, GaussianSplatTrainer, TrainingConfig};

/// Gaussian Splatting Model (Native)
/// Wraps TrueShot's 3DGS implementation for inference and training hooks.
pub struct GaussianSplatModel {
    cloud: GaussianCloud,
}

impl GaussianSplatModel {
    pub fn from_points(points: &[(na::Point3<f32>, [u8; 3])]) -> Self {
        Self {
            cloud: GaussianCloud::from_points(points),
        }
    }

    pub fn render(&self, camera: &Camera) -> Result<RgbImage> {
        self.cloud.render(camera)
    }
}

impl From<GaussianCloud> for GaussianSplatModel {
    fn from(cloud: GaussianCloud) -> Self {
        Self { cloud }
    }
}

pub struct SplatTrainer {
    trainer: GaussianSplatTrainer,
    last_loss: Option<f32>,
}

impl SplatTrainer {
    pub fn new(
        points: &[(na::Point3<f32>, [u8; 3])],
        cameras: Vec<Camera>,
        config: Option<TrainingConfig>,
    ) -> Result<Self> {
        let config = config.unwrap_or_default();
        let trainer = GaussianSplatTrainer::new(points, cameras, config);
        Ok(Self {
            trainer,
            last_loss: None,
        })
    }

    pub fn train_step(&mut self) -> Result<f32> {
        let loss = self.trainer.step()?;
        self.last_loss = Some(loss);
        Ok(loss)
    }

    pub fn last_loss(&self) -> Option<f32> {
        self.last_loss
    }
}
