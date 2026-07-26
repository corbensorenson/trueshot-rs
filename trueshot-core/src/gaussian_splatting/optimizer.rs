//! Adam Optimizer for 3D Gaussian Splatting
//! 
//! Implements the Adam optimization algorithm with per-parameter learning rates.

use super::{GaussianCloud, GaussianGradients, SH_COEFFS_TOTAL};
use nalgebra as na;

/// Adam optimizer state
pub struct AdamOptimizer {
    /// First moment (mean of gradients)
    m_position: Vec<na::Vector3<f32>>,
    m_rotation: Vec<na::Vector4<f32>>,
    m_scale: Vec<na::Vector3<f32>>,
    m_opacity: Vec<f32>,
    m_sh: Vec<Vec<f32>>,
    
    /// Second moment (variance of gradients)
    v_position: Vec<na::Vector3<f32>>,
    v_rotation: Vec<na::Vector4<f32>>,
    v_scale: Vec<na::Vector3<f32>>,
    v_opacity: Vec<f32>,
    v_sh: Vec<Vec<f32>>,
    
    /// Learning rates
    lr_position: f32,
    lr_rotation: f32,
    lr_scale: f32,
    lr_opacity: f32,
    lr_sh: f32,
    
    /// Adam hyperparameters
    beta1: f32,
    beta2: f32,
    epsilon: f32,
    
    /// Timestep
    t: u32,
}

impl AdamOptimizer {
    pub fn new(n: usize, lr_position: f32, lr_color: f32) -> Self {
        Self {
            m_position: vec![na::Vector3::zeros(); n],
            m_rotation: vec![na::Vector4::zeros(); n],
            m_scale: vec![na::Vector3::zeros(); n],
            m_opacity: vec![0.0; n],
            m_sh: vec![vec![0.0; SH_COEFFS_TOTAL]; n],
            
            v_position: vec![na::Vector3::zeros(); n],
            v_rotation: vec![na::Vector4::zeros(); n],
            v_scale: vec![na::Vector3::zeros(); n],
            v_opacity: vec![0.0; n],
            v_sh: vec![vec![0.0; SH_COEFFS_TOTAL]; n],
            
            lr_position,
            lr_rotation: lr_position * 0.1,
            lr_scale: lr_position * 5.0,
            lr_opacity: lr_position * 50.0,
            lr_sh: lr_color,
            
            beta1: 0.9,
            beta2: 0.999,
            epsilon: 1e-8,
            
            t: 0,
        }
    }

    /// Resize optimizer for new Gaussian count
    pub fn resize(&mut self, n: usize) {
        let current = self.m_position.len();
        
        if n > current {
            // Add new elements
            let diff = n - current;
            self.m_position.extend(vec![na::Vector3::zeros(); diff]);
            self.m_rotation.extend(vec![na::Vector4::zeros(); diff]);
            self.m_scale.extend(vec![na::Vector3::zeros(); diff]);
            self.m_opacity.extend(vec![0.0; diff]);
            self.m_sh.extend(vec![vec![0.0; SH_COEFFS_TOTAL]; diff]);
            
            self.v_position.extend(vec![na::Vector3::zeros(); diff]);
            self.v_rotation.extend(vec![na::Vector4::zeros(); diff]);
            self.v_scale.extend(vec![na::Vector3::zeros(); diff]);
            self.v_opacity.extend(vec![0.0; diff]);
            self.v_sh.extend(vec![vec![0.0; SH_COEFFS_TOTAL]; diff]);
        } else if n < current {
            // Truncate
            self.m_position.truncate(n);
            self.m_rotation.truncate(n);
            self.m_scale.truncate(n);
            self.m_opacity.truncate(n);
            self.m_sh.truncate(n);
            
            self.v_position.truncate(n);
            self.v_rotation.truncate(n);
            self.v_scale.truncate(n);
            self.v_opacity.truncate(n);
            self.v_sh.truncate(n);
        }
    }

    /// Perform one optimization step
    pub fn step(&mut self, gaussians: &mut GaussianCloud, gradients: &GaussianGradients) {
        self.t += 1;
        let t = self.t as f32;
        
        // Bias correction terms
        let bias_correction1 = 1.0 - self.beta1.powi(self.t as i32);
        let bias_correction2 = 1.0 - self.beta2.powi(self.t as i32);

        let n = gaussians.num_gaussians().min(gradients.position_grad.len());

        for i in 0..n {
            // Position update
            let g = gradients.position_grad[i];
            self.m_position[i] = self.beta1 * self.m_position[i] + (1.0 - self.beta1) * g;
            self.v_position[i] = self.beta2 * self.v_position[i] 
                + (1.0 - self.beta2) * g.component_mul(&g);
            
            let m_hat = self.m_position[i] / bias_correction1;
            let v_hat = self.v_position[i] / bias_correction2;
            
            let update = m_hat.component_div(&(v_hat.map(|x| x.sqrt()) + na::Vector3::repeat(self.epsilon)));
            gaussians.update_position(i, -self.lr_position * update);

            // Rotation update
            let g = gradients.rotation_grad[i];
            self.m_rotation[i] = self.beta1 * self.m_rotation[i] + (1.0 - self.beta1) * g;
            self.v_rotation[i] = self.beta2 * self.v_rotation[i] 
                + (1.0 - self.beta2) * g.component_mul(&g);
            
            let m_hat = self.m_rotation[i] / bias_correction1;
            let v_hat = self.v_rotation[i] / bias_correction2;
            
            let update = m_hat.component_div(&(v_hat.map(|x| x.sqrt()) + na::Vector4::repeat(self.epsilon)));
            gaussians.update_rotation(i, -self.lr_rotation * update);

            // Scale update
            let g = gradients.scale_grad[i];
            self.m_scale[i] = self.beta1 * self.m_scale[i] + (1.0 - self.beta1) * g;
            self.v_scale[i] = self.beta2 * self.v_scale[i] 
                + (1.0 - self.beta2) * g.component_mul(&g);
            
            let m_hat = self.m_scale[i] / bias_correction1;
            let v_hat = self.v_scale[i] / bias_correction2;
            
            let update = m_hat.component_div(&(v_hat.map(|x| x.sqrt()) + na::Vector3::repeat(self.epsilon)));
            gaussians.update_scale(i, -self.lr_scale * update);

            // Opacity update
            let g = gradients.opacity_grad[i];
            self.m_opacity[i] = self.beta1 * self.m_opacity[i] + (1.0 - self.beta1) * g;
            self.v_opacity[i] = self.beta2 * self.v_opacity[i] 
                + (1.0 - self.beta2) * g * g;
            
            let m_hat = self.m_opacity[i] / bias_correction1;
            let v_hat = self.v_opacity[i] / bias_correction2;
            
            let update = m_hat / (v_hat.sqrt() + self.epsilon);
            gaussians.update_opacity(i, -self.lr_opacity * update);

            // SH update (DC + higher terms)
            let grad_sh = &gradients.sh_grad[i];
            if grad_sh.len() == self.m_sh[i].len() {
                let mut delta_sh = vec![0.0f32; grad_sh.len()];
                for k in 0..grad_sh.len() {
                    let gk = grad_sh[k];
                    self.m_sh[i][k] = self.beta1 * self.m_sh[i][k] + (1.0 - self.beta1) * gk;
                    self.v_sh[i][k] = self.beta2 * self.v_sh[i][k] + (1.0 - self.beta2) * gk * gk;
                    let m_hat = self.m_sh[i][k] / bias_correction1;
                    let v_hat = self.v_sh[i][k] / bias_correction2;
                    let update = m_hat / (v_hat.sqrt() + self.epsilon);
                    delta_sh[k] = -self.lr_sh * update;
                }
                gaussians.update_sh_coeffs(i, &delta_sh);
            }
        }
    }

    /// Update learning rates (for learning rate scheduling)
    pub fn set_learning_rates(&mut self, lr_position: f32, lr_color: f32) {
        self.lr_position = lr_position;
        self.lr_rotation = lr_position * 0.1;
        self.lr_scale = lr_position * 5.0;
        self.lr_opacity = lr_position * 50.0;
        self.lr_sh = lr_color;
    }
}
