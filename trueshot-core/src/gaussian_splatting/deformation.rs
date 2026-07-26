//! Neural Deformation Network for 4D Gaussian Splatting
//!
//! State-of-the-art deformation prediction using lightweight MLPs.
//! Based on 4D-GS (CVPR 2024) and Deformable 3D Gaussians approaches.
//!
//! Key features:
//! - Multi-head Gaussian deformation decoder
//! - HexPlane-inspired spatio-temporal feature encoding
//! - Position, rotation, and scale deformation prediction
//! - Deformation regularization for motion consistency

use nalgebra as na;
use rayon::prelude::*;

/// Configuration for the deformation network
#[derive(Clone, Debug)]
pub struct DeformationConfig {
    /// Hidden layer dimensions
    pub hidden_dims: Vec<usize>,
    /// Number of output heads (position, rotation, scale)
    pub num_heads: usize,
    /// Feature encoding resolution
    pub encoding_resolution: usize,
    /// Number of frequency bands for positional encoding
    pub num_frequency_bands: usize,
    /// Regularization weight for deformation smoothness
    pub regularization_weight: f32,
    /// Learning rate for deformation network
    pub learning_rate: f32,
    /// Use multi-resolution encoding
    pub use_multi_resolution: bool,
}

impl Default for DeformationConfig {
    fn default() -> Self {
        Self {
            hidden_dims: vec![256, 256, 128],
            num_heads: 3, // position, rotation, scale
            encoding_resolution: 128,
            num_frequency_bands: 6,
            regularization_weight: 0.1,
            learning_rate: 1e-4,
            use_multi_resolution: true,
        }
    }
}

/// Multi-resolution HexPlane features for spatio-temporal encoding
/// Based on K-Planes and HexPlane representations
#[derive(Clone, Debug)]
pub struct HexPlaneEncoder {
    /// Resolution levels for multi-resolution encoding
    resolutions: Vec<usize>,
    /// Feature dimension per plane
    feature_dim: usize,
    /// Plane features: XY, XZ, YZ, XT, YT, ZT
    planes: Vec<PlaneFeatures>,
}

#[derive(Clone, Debug)]
struct PlaneFeatures {
    resolution: usize,
    features: Vec<f32>, // [resolution, resolution, feature_dim]
}

impl HexPlaneEncoder {
    /// Create new HexPlane encoder with multi-resolution support
    pub fn new(resolutions: &[usize], feature_dim: usize) -> Self {
        let planes: Vec<PlaneFeatures> = resolutions
            .iter()
            .flat_map(|&res| {
                (0..6).map(move |_| PlaneFeatures {
                    resolution: res,
                    features: vec![0.0; res * res * feature_dim],
                })
            })
            .collect();

        Self {
            resolutions: resolutions.to_vec(),
            feature_dim,
            planes,
        }
    }

    /// Query features at spatio-temporal location
    pub fn query(&self, x: f32, y: f32, z: f32, t: f32) -> Vec<f32> {
        let mut features = Vec::with_capacity(self.feature_dim * 6 * self.resolutions.len());

        // For each resolution level
        for (level, &res) in self.resolutions.iter().enumerate() {
            let plane_offset = level * 6;

            // Bilinear interpolation coordinates
            let coords = [
                (x, y), // XY plane
                (x, z), // XZ plane
                (y, z), // YZ plane
                (x, t), // XT plane
                (y, t), // YT plane
                (z, t), // ZT plane
            ];

            for (plane_idx, (u, v)) in coords.iter().enumerate() {
                let plane = &self.planes[plane_offset + plane_idx];
                let sample = self.bilinear_sample(plane, *u, *v, res);
                features.extend(sample);
            }
        }

        features
    }

    /// Bilinear sampling from a feature plane
    fn bilinear_sample(&self, plane: &PlaneFeatures, u: f32, v: f32, res: usize) -> Vec<f32> {
        // Normalize to [0, res-1]
        let u_scaled = (u + 1.0) * 0.5 * (res - 1) as f32;
        let v_scaled = (v + 1.0) * 0.5 * (res - 1) as f32;

        let u0 = (u_scaled.floor() as usize).min(res - 1);
        let v0 = (v_scaled.floor() as usize).min(res - 1);
        let u1 = (u0 + 1).min(res - 1);
        let v1 = (v0 + 1).min(res - 1);

        let wu = u_scaled - u0 as f32;
        let wv = v_scaled - v0 as f32;

        let mut result = vec![0.0; self.feature_dim];

        for f in 0..self.feature_dim {
            let f00 = plane.features[(v0 * res + u0) * self.feature_dim + f];
            let f01 = plane.features[(v0 * res + u1) * self.feature_dim + f];
            let f10 = plane.features[(v1 * res + u0) * self.feature_dim + f];
            let f11 = plane.features[(v1 * res + u1) * self.feature_dim + f];

            result[f] = (1.0 - wu) * (1.0 - wv) * f00
                + wu * (1.0 - wv) * f01
                + (1.0 - wu) * wv * f10
                + wu * wv * f11;
        }

        result
    }

    /// Update plane features (gradient descent)
    pub fn update(&mut self, gradients: &[f32], learning_rate: f32) {
        for (feat, &grad) in self
            .planes
            .iter_mut()
            .flat_map(|p| p.features.iter_mut())
            .zip(gradients.iter())
        {
            *feat -= learning_rate * grad;
        }
    }
}

/// Lightweight MLP for deformation prediction
#[derive(Clone, Debug)]
pub struct DeformationMLP {
    /// Layer weights
    weights: Vec<LayerWeights>,
    /// Configuration
    config: DeformationConfig,
}

#[derive(Clone, Debug)]
struct LayerWeights {
    weight: Vec<f32>, // [out_dim, in_dim]
    bias: Vec<f32>,   // [out_dim]
    in_dim: usize,
    out_dim: usize,
}

impl LayerWeights {
    fn new(in_dim: usize, out_dim: usize) -> Self {
        // Xavier initialization
        let scale = (6.0 / (in_dim + out_dim) as f32).sqrt();
        let mut rng = rand::thread_rng();
        use rand::Rng;

        Self {
            weight: (0..out_dim * in_dim)
                .map(|_| rng.gen_range(-scale..scale))
                .collect(),
            bias: vec![0.0; out_dim],
            in_dim,
            out_dim,
        }
    }

    fn forward(&self, input: &[f32]) -> Vec<f32> {
        let mut output = self.bias.clone();
        for o in 0..self.out_dim {
            for i in 0..self.in_dim {
                output[o] += self.weight[o * self.in_dim + i] * input[i];
            }
        }
        output
    }
}

impl DeformationMLP {
    /// Create new deformation MLP
    pub fn new(input_dim: usize, config: DeformationConfig) -> Self {
        let mut weights = Vec::new();

        // Build layers
        let mut current_dim = input_dim;
        for &hidden_dim in &config.hidden_dims {
            weights.push(LayerWeights::new(current_dim, hidden_dim));
            current_dim = hidden_dim;
        }

        // Output heads: position (3), rotation (4 quaternion), scale (3)
        let output_dim = 3 + 4 + 3;
        weights.push(LayerWeights::new(current_dim, output_dim));

        Self { weights, config }
    }

    /// Forward pass with ReLU activations
    pub fn forward(&self, input: &[f32]) -> DeformationOutput {
        let mut x = input.to_vec();

        // Hidden layers with ReLU
        for layer in &self.weights[..self.weights.len() - 1] {
            x = layer.forward(&x);
            // ReLU
            for v in &mut x {
                *v = v.max(0.0);
            }
        }

        // Output layer (no activation)
        let output = self.weights.last().unwrap().forward(&x);

        // Parse output into deformation components
        let raw_rotation = na::Quaternion::new(output[6], output[3], output[4], output[5]);
        let rotation_delta = if raw_rotation.norm_squared() > 1e-12 {
            na::UnitQuaternion::new_normalize(raw_rotation)
        } else {
            // Zero input should initialize to identity, not an invalid quaternion.
            na::UnitQuaternion::identity()
        };

        DeformationOutput {
            position_delta: na::Vector3::new(output[0], output[1], output[2]),
            rotation_delta,
            scale_delta: na::Vector3::new(output[7], output[8], output[9]),
        }
    }
}

/// Output of deformation network
#[derive(Clone, Debug)]
pub struct DeformationOutput {
    /// Position change
    pub position_delta: na::Vector3<f32>,
    /// Rotation change (as quaternion)
    pub rotation_delta: na::UnitQuaternion<f32>,
    /// Scale change (log-space)
    pub scale_delta: na::Vector3<f32>,
}

/// Complete deformation field for 4DGS
pub struct DeformationField {
    /// HexPlane encoder for spatio-temporal features
    encoder: HexPlaneEncoder,
    /// Deformation MLP
    mlp: DeformationMLP,
    /// Configuration
    config: DeformationConfig,
    /// Positional encoding frequencies
    frequencies: Vec<f32>,
}

impl DeformationField {
    /// Create new deformation field
    pub fn new(config: DeformationConfig) -> Self {
        let resolutions = if config.use_multi_resolution {
            vec![16, 32, 64, config.encoding_resolution]
        } else {
            vec![config.encoding_resolution]
        };

        let feature_dim = 8; // Features per plane
        let encoder = HexPlaneEncoder::new(&resolutions, feature_dim);

        // Calculate input dimension for MLP
        // 6 planes * feature_dim * num_resolutions + positional encoding
        let encoding_dim = 6 * feature_dim * resolutions.len();
        let pos_encoding_dim = 4 * 2 * config.num_frequency_bands; // xyzt * sin/cos * bands
        let input_dim = encoding_dim + pos_encoding_dim;

        let mlp = DeformationMLP::new(input_dim, config.clone());

        let frequencies: Vec<f32> = (0..config.num_frequency_bands)
            .map(|i| 2.0f32.powi(i as i32))
            .collect();

        Self {
            encoder,
            mlp,
            config,
            frequencies,
        }
    }

    /// Query deformation at a 4D point
    pub fn query(&self, x: f32, y: f32, z: f32, t: f32) -> DeformationOutput {
        // Get HexPlane features
        let mut features = self.encoder.query(x, y, z, t);

        // Add positional encoding
        for &freq in &self.frequencies {
            for &coord in &[x, y, z, t] {
                features.push((coord * freq * std::f32::consts::PI).sin());
                features.push((coord * freq * std::f32::consts::PI).cos());
            }
        }

        // Forward through MLP
        self.mlp.forward(&features)
    }

    /// Batch query with parallelization
    pub fn query_batch(&self, points: &[(f32, f32, f32, f32)]) -> Vec<DeformationOutput> {
        points
            .par_iter()
            .map(|&(x, y, z, t)| self.query(x, y, z, t))
            .collect()
    }

    /// Apply deformation to a Gaussian
    pub fn deform_gaussian(
        &self,
        position: &na::Point3<f32>,
        rotation: &na::UnitQuaternion<f32>,
        scale: &na::Vector3<f32>,
        time: f32,
    ) -> (na::Point3<f32>, na::UnitQuaternion<f32>, na::Vector3<f32>) {
        let deformation = self.query(position.x, position.y, position.z, time);

        // Apply deformations
        let new_position = na::Point3::from(position.coords + deformation.position_delta);
        let new_rotation = rotation * deformation.rotation_delta;
        let new_scale = scale.component_mul(&deformation.scale_delta.map(|s| s.exp()));

        (new_position, new_rotation, new_scale)
    }

    /// Compute regularization loss for smooth deformations
    pub fn regularization_loss(&self, points: &[(f32, f32, f32, f32)]) -> f32 {
        if points.len() < 2 {
            return 0.0;
        }

        let eps = 0.01;
        let mut loss = 0.0;

        // Sample subset for efficiency
        let sample_size = points.len().min(1000);
        let step = points.len() / sample_size;

        for i in (0..points.len()).step_by(step.max(1)) {
            let (x, y, z, t) = points[i];
            let d0 = self.query(x, y, z, t);

            // Temporal smoothness
            let d_t = self.query(x, y, z, t + eps);
            loss += (d0.position_delta - d_t.position_delta).norm_squared();

            // Spatial smoothness
            let d_x = self.query(x + eps, y, z, t);
            let d_y = self.query(x, y + eps, z, t);
            let d_z = self.query(x, y, z + eps, t);

            loss += (d0.position_delta - d_x.position_delta).norm_squared();
            loss += (d0.position_delta - d_y.position_delta).norm_squared();
            loss += (d0.position_delta - d_z.position_delta).norm_squared();
        }

        self.config.regularization_weight * loss / sample_size as f32
    }
}

/// Lifespan tracking for Gaussian pruning
/// Based on 4DGS-1K optimization
#[derive(Clone, Debug)]
pub struct GaussianLifespan {
    /// Gaussian ID
    pub id: usize,
    /// First frame where Gaussian was active
    pub birth_frame: u32,
    /// Last frame where Gaussian was active
    pub last_active_frame: u32,
    /// Cumulative opacity contribution
    pub cumulative_opacity: f32,
    /// Number of frames where opacity > threshold
    pub active_frame_count: u32,
    /// Maximum opacity achieved
    pub max_opacity: f32,
    /// Is this Gaussian marked for removal
    pub marked_for_removal: bool,
}

impl GaussianLifespan {
    pub fn new(id: usize, birth_frame: u32) -> Self {
        Self {
            id,
            birth_frame,
            last_active_frame: birth_frame,
            cumulative_opacity: 0.0,
            active_frame_count: 0,
            max_opacity: 0.0,
            marked_for_removal: false,
        }
    }

    /// Update with new observation
    pub fn update(&mut self, frame: u32, opacity: f32, threshold: f32) {
        if opacity > threshold {
            self.last_active_frame = frame;
            self.active_frame_count += 1;
            self.cumulative_opacity += opacity;
            self.max_opacity = self.max_opacity.max(opacity);
        }
    }

    /// Get lifespan in frames
    pub fn lifespan(&self) -> u32 {
        self.last_active_frame - self.birth_frame + 1
    }

    /// Check if Gaussian should be pruned
    pub fn should_prune(&self, current_frame: u32, config: &LifespanPruningConfig) -> bool {
        // Short-lived Gaussians
        if self.lifespan() < config.min_lifespan_frames {
            return true;
        }

        // Inactive for too long
        if current_frame - self.last_active_frame > config.max_inactive_frames {
            return true;
        }

        // Low contribution
        let avg_opacity = self.cumulative_opacity / self.active_frame_count.max(1) as f32;
        if avg_opacity < config.min_average_opacity {
            return true;
        }

        // Low activity ratio
        let total_frames = current_frame - self.birth_frame + 1;
        let activity_ratio = self.active_frame_count as f32 / total_frames as f32;
        if activity_ratio < config.min_activity_ratio {
            return true;
        }

        false
    }
}

/// Configuration for lifespan-based pruning
#[derive(Clone, Debug)]
pub struct LifespanPruningConfig {
    /// Minimum lifespan in frames before considering for pruning
    pub min_lifespan_frames: u32,
    /// Maximum frames without activity before pruning
    pub max_inactive_frames: u32,
    /// Minimum average opacity to keep
    pub min_average_opacity: f32,
    /// Minimum ratio of active frames to total frames
    pub min_activity_ratio: f32,
    /// Opacity threshold for considering a frame "active"
    pub activity_threshold: f32,
}

impl Default for LifespanPruningConfig {
    fn default() -> Self {
        Self {
            min_lifespan_frames: 10,
            max_inactive_frames: 30,
            min_average_opacity: 0.01,
            min_activity_ratio: 0.1,
            activity_threshold: 0.005,
        }
    }
}

/// Lifespan manager for all Gaussians
pub struct LifespanManager {
    lifespans: Vec<GaussianLifespan>,
    config: LifespanPruningConfig,
    current_frame: u32,
}

impl LifespanManager {
    pub fn new(num_gaussians: usize, config: LifespanPruningConfig) -> Self {
        Self {
            lifespans: (0..num_gaussians)
                .map(|id| GaussianLifespan::new(id, 0))
                .collect(),
            config,
            current_frame: 0,
        }
    }

    /// Update lifespans with current frame data
    pub fn update(&mut self, opacities: &[f32]) {
        self.current_frame += 1;

        for (lifespan, &opacity) in self.lifespans.iter_mut().zip(opacities.iter()) {
            lifespan.update(self.current_frame, opacity, self.config.activity_threshold);
        }
    }

    /// Get indices of Gaussians to prune
    pub fn get_prune_indices(&self) -> Vec<usize> {
        self.lifespans
            .iter()
            .filter(|l| l.should_prune(self.current_frame, &self.config))
            .map(|l| l.id)
            .collect()
    }

    /// Add new Gaussians
    pub fn add_gaussians(&mut self, count: usize) {
        let start_id = self.lifespans.len();
        for i in 0..count {
            self.lifespans
                .push(GaussianLifespan::new(start_id + i, self.current_frame));
        }
    }

    /// Remove pruned Gaussians
    pub fn remove_gaussians(&mut self, indices: &[usize]) {
        // Remove in reverse order to maintain indices
        let mut sorted_indices = indices.to_vec();
        sorted_indices.sort_unstable();
        sorted_indices.reverse();

        for &idx in &sorted_indices {
            if idx < self.lifespans.len() {
                self.lifespans.remove(idx);
            }
        }

        // Re-index remaining
        for (new_idx, lifespan) in self.lifespans.iter_mut().enumerate() {
            lifespan.id = new_idx;
        }
    }

    /// Get statistics
    pub fn statistics(&self) -> LifespanStatistics {
        let total = self.lifespans.len();
        let active = self
            .lifespans
            .iter()
            .filter(|l| self.current_frame - l.last_active_frame <= 5)
            .count();
        let avg_lifespan = self
            .lifespans
            .iter()
            .map(|l| l.lifespan() as f64)
            .sum::<f64>()
            / total.max(1) as f64;

        LifespanStatistics {
            total_gaussians: total,
            active_gaussians: active,
            average_lifespan: avg_lifespan as f32,
            current_frame: self.current_frame,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LifespanStatistics {
    pub total_gaussians: usize,
    pub active_gaussians: usize,
    pub average_lifespan: f32,
    pub current_frame: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hexplane_encoder() {
        let encoder = HexPlaneEncoder::new(&[16, 32], 8);
        let features = encoder.query(0.0, 0.0, 0.0, 0.5);
        assert_eq!(features.len(), 6 * 8 * 2); // 6 planes * 8 features * 2 resolutions
    }

    #[test]
    fn test_deformation_mlp() {
        let config = DeformationConfig::default();
        let mlp = DeformationMLP::new(256, config);
        let input = vec![0.0; 256];
        let output = mlp.forward(&input);
        assert!((output.rotation_delta.norm() - 1.0).abs() < 0.1);
    }

    #[test]
    fn test_deformation_field() {
        let config = DeformationConfig::default();
        let field = DeformationField::new(config);
        let output = field.query(0.0, 0.0, 0.0, 0.5);
        assert!(output.position_delta.norm() < 100.0);
    }

    #[test]
    fn test_lifespan_pruning() {
        let config = LifespanPruningConfig::default();
        let mut lifespan = GaussianLifespan::new(0, 0);

        // Active for many frames
        for frame in 1..100 {
            lifespan.update(frame, 0.5, config.activity_threshold);
        }

        assert!(!lifespan.should_prune(100, &config));
    }

    #[test]
    fn test_lifespan_manager() {
        let config = LifespanPruningConfig::default();
        let mut manager = LifespanManager::new(100, config);

        // Simulate some frames
        for _ in 0..50 {
            let opacities: Vec<f32> = (0..100).map(|i| if i < 50 { 0.5 } else { 0.001 }).collect();
            manager.update(&opacities);
        }

        let prune_indices = manager.get_prune_indices();
        assert!(prune_indices.len() > 0); // Should prune low-opacity Gaussians
    }
}
