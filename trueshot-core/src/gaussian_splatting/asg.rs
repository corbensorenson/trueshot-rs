//! Anisotropic Spherical Gaussians (ASG) for View-Dependent Appearance
//! 
//! NeurIPS 2024: Spec-Gaussian implementation.
//! Better representation of specular and reflective surfaces.
//! 
//! Reference: "Spec-Gaussian: Anisotropic View-Dependent Appearance for 3D Gaussian Splatting"

use nalgebra as na;
use super::gaussian::SH_COEFFS_PER_CHANNEL;

/// Anisotropic Spherical Gaussian lobe
/// 
/// Represents a view-dependent color component (specular highlights, reflections)
#[derive(Debug, Clone)]
pub struct AnisotropicSG {
    /// Lobe center direction (normalized)
    pub axis: na::Vector3<f32>,
    /// Tangent direction for anisotropy
    pub tangent: na::Vector3<f32>,
    /// Sharpness along axis direction
    pub sharpness: f32,
    /// Anisotropy ratio (1.0 = isotropic)
    pub anisotropy: f32,
    /// RGB amplitude
    pub amplitude: [f32; 3],
}

impl AnisotropicSG {
    /// Create new ASG lobe
    pub fn new(axis: na::Vector3<f32>, sharpness: f32, amplitude: [f32; 3]) -> Self {
        // Compute tangent perpendicular to axis
        let up = if axis.y.abs() < 0.9 {
            na::Vector3::new(0.0, 1.0, 0.0)
        } else {
            na::Vector3::new(1.0, 0.0, 0.0)
        };
        let tangent = axis.cross(&up).normalize();

        Self {
            axis: axis.normalize(),
            tangent,
            sharpness,
            anisotropy: 1.0,
            amplitude,
        }
    }

    /// Create anisotropic ASG with specific tangent
    pub fn anisotropic(
        axis: na::Vector3<f32>,
        tangent: na::Vector3<f32>,
        sharpness: f32,
        anisotropy: f32,
        amplitude: [f32; 3],
    ) -> Self {
        Self {
            axis: axis.normalize(),
            tangent: tangent.normalize(),
            sharpness,
            anisotropy,
            amplitude,
        }
    }

    /// Evaluate ASG for given viewing direction
    /// 
    /// Returns RGB contribution from this lobe
    pub fn evaluate(&self, view_dir: &na::Vector3<f32>) -> [f32; 3] {
        let v = view_dir.normalize();
        
        // Dot product with lobe axis
        let cos_theta = self.axis.dot(&v);
        
        if cos_theta <= 0.0 {
            return [0.0, 0.0, 0.0];
        }

        // For anisotropic: modify exponential based on tangent direction
        let aniso_term = if (self.anisotropy - 1.0).abs() > 0.01 {
            let bitangent = self.axis.cross(&self.tangent);
            let proj_tangent = v.dot(&self.tangent);
            let proj_bitangent = v.dot(&bitangent);
            
            // Anisotropic falloff
            let aniso_factor = proj_tangent.powi(2) * self.anisotropy 
                             + proj_bitangent.powi(2) / self.anisotropy;
            (-self.sharpness * aniso_factor * (1.0 - cos_theta)).exp()
        } else {
            // Isotropic falloff
            (-self.sharpness * (1.0 - cos_theta)).exp()
        };

        [
            self.amplitude[0] * aniso_term,
            self.amplitude[1] * aniso_term,
            self.amplitude[2] * aniso_term,
        ]
    }

    /// Gradient of ASG evaluation w.r.t. view direction
    /// Used for backpropagation during training
    pub fn gradient(&self, view_dir: &na::Vector3<f32>) -> na::Matrix3<f32> {
        let v = view_dir.normalize();
        let cos_theta = self.axis.dot(&v);
        
        if cos_theta <= 0.0 {
            return na::Matrix3::zeros();
        }

        let exp_term = (-self.sharpness * (1.0 - cos_theta)).exp();
        
        // d(exp(-s*(1-d)))/dv = s * exp(-s*(1-d)) * axis
        let d_exp = self.sharpness * exp_term;
        
        // Gradient for each color channel
        let grad_col = |amp: f32| -> na::Vector3<f32> {
            amp * d_exp * self.axis
        };

        na::Matrix3::from_columns(&[
            grad_col(self.amplitude[0]),
            grad_col(self.amplitude[1]),
            grad_col(self.amplitude[2]),
        ])
    }
}

/// Collection of ASG lobes for a single Gaussian
#[derive(Debug, Clone)]
pub struct ASGBank {
    /// Multiple ASG lobes for complex specular patterns
    pub lobes: Vec<AnisotropicSG>,
}

impl ASGBank {
    /// Create empty bank
    pub fn new() -> Self {
        Self { lobes: Vec::new() }
    }

    /// Create bank from reflection direction (for initialization)
    pub fn from_reflection(
        normal: &na::Vector3<f32>,
        roughness: f32,
        specular_color: [f32; 3],
    ) -> Self {
        // Create a single lobe in reflection direction
        let sharpness = 1.0 / (roughness.powi(2) + 0.01);
        
        let mut bank = Self::new();
        bank.lobes.push(AnisotropicSG::new(
            *normal,
            sharpness,
            specular_color,
        ));
        
        bank
    }

    /// Evaluate all lobes for given viewing direction
    pub fn evaluate(&self, view_dir: &na::Vector3<f32>) -> [f32; 3] {
        let mut total = [0.0f32; 3];
        
        for lobe in &self.lobes {
            let contribution = lobe.evaluate(view_dir);
            total[0] += contribution[0];
            total[1] += contribution[1];
            total[2] += contribution[2];
        }
        
        // Clamp to valid range
        [
            total[0].clamp(0.0, 1.0),
            total[1].clamp(0.0, 1.0),
            total[2].clamp(0.0, 1.0),
        ]
    }

    /// Total number of learnable parameters
    pub fn num_params(&self) -> usize {
        // Each lobe: axis(3) + tangent(3) + sharpness(1) + anisotropy(1) + amplitude(3) = 11
        self.lobes.len() * 11
    }
}

impl Default for ASGBank {
    fn default() -> Self {
        Self::new()
    }
}

/// Extended Gaussian with Spherical Harmonics + ASG
/// 
/// Combines diffuse (SH) and specular (ASG) components
#[derive(Debug, Clone)]
pub struct SpecularGaussian {
    /// Position in world space
    pub position: na::Point3<f32>,
    /// Rotation quaternion
    pub rotation: na::Vector4<f32>,
    /// Log-space scale
    pub scale: na::Vector3<f32>,
    /// Opacity (before sigmoid)
    pub opacity: f32,
    /// Spherical harmonics for diffuse color
    pub sh_coeffs: Vec<f32>,
    /// ASG lobes for specular highlights
    pub asg_bank: ASGBank,
}

impl SpecularGaussian {
    /// Compute color for given viewing direction
    pub fn color(&self, view_dir: &na::Vector3<f32>, normal: &na::Vector3<f32>) -> [f32; 3] {
        // Diffuse from SH (DC term only for now)
        let c0 = 0.28209479f32;
        let diffuse = [
            (self.sh_coeffs.first().unwrap_or(&0.0) * c0 + 0.5).clamp(0.0, 1.0),
            (self.sh_coeffs.get(SH_COEFFS_PER_CHANNEL).unwrap_or(&0.0) * c0 + 0.5).clamp(0.0, 1.0),
            (self.sh_coeffs.get(SH_COEFFS_PER_CHANNEL * 2).unwrap_or(&0.0) * c0 + 0.5).clamp(0.0, 1.0),
        ];

        // Specular from ASG
        let specular = self.asg_bank.evaluate(view_dir);

        // Combine (simple additive for now)
        [
            (diffuse[0] + specular[0]).clamp(0.0, 1.0),
            (diffuse[1] + specular[1]).clamp(0.0, 1.0),
            (diffuse[2] + specular[2]).clamp(0.0, 1.0),
        ]
    }

    /// Compute reflection direction for initialization
    pub fn reflect(view_dir: &na::Vector3<f32>, normal: &na::Vector3<f32>) -> na::Vector3<f32> {
        let d = 2.0 * view_dir.dot(normal);
        (d * normal - view_dir).normalize()
    }
}

/// Cook-Torrance BRDF parameters for realistic materials
#[derive(Debug, Clone)]
pub struct PBRMaterial {
    /// Base color (albedo)
    pub albedo: [f32; 3],
    /// Metallic factor [0, 1]
    pub metallic: f32,
    /// Roughness [0, 1]
    pub roughness: f32,
    /// Index of refraction
    pub ior: f32,
}

impl PBRMaterial {
    /// Convert PBR material to ASG representation
    pub fn to_asg_bank(&self, normal: &na::Vector3<f32>) -> ASGBank {
        let mut bank = ASGBank::new();

        // For metallic materials, specular color = albedo
        // For dielectrics, specular is white
        let spec_color = if self.metallic > 0.5 {
            self.albedo
        } else {
            [1.0, 1.0, 1.0]
        };

        // Roughness to sharpness conversion
        let sharpness = 2.0 / (self.roughness.powi(4) + 0.001);

        // Fresnel at normal incidence
        let f0 = ((self.ior - 1.0) / (self.ior + 1.0)).powi(2);
        let fresnel_intensity = f0 * (1.0 - self.metallic) + self.metallic;

        bank.lobes.push(AnisotropicSG::new(
            *normal,
            sharpness,
            [
                spec_color[0] * fresnel_intensity,
                spec_color[1] * fresnel_intensity,
                spec_color[2] * fresnel_intensity,
            ],
        ));

        bank
    }
}

impl Default for PBRMaterial {
    fn default() -> Self {
        Self {
            albedo: [0.5, 0.5, 0.5],
            metallic: 0.0,
            roughness: 0.5,
            ior: 1.5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asg_evaluation() {
        let asg = AnisotropicSG::new(
            na::Vector3::new(0.0, 0.0, 1.0), // pointing up
            10.0, // sharpness
            [1.0, 1.0, 1.0], // white
        );

        // Directly along axis should be maximum
        let along_axis = asg.evaluate(&na::Vector3::new(0.0, 0.0, 1.0));
        assert!(along_axis[0] > 0.9);

        // Perpendicular should be near zero
        let perpendicular = asg.evaluate(&na::Vector3::new(1.0, 0.0, 0.0));
        assert!(perpendicular[0] < 0.1);

        // Opposite direction should be zero
        let opposite = asg.evaluate(&na::Vector3::new(0.0, 0.0, -1.0));
        assert_eq!(opposite, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_asg_bank() {
        let mut bank = ASGBank::new();
        bank.lobes.push(AnisotropicSG::new(
            na::Vector3::new(0.0, 0.0, 1.0),
            5.0,
            [0.5, 0.0, 0.0],
        ));
        bank.lobes.push(AnisotropicSG::new(
            na::Vector3::new(0.0, 1.0, 0.0),
            5.0,
            [0.0, 0.5, 0.0],
        ));

        // Evaluate at 45 degree angle
        let view = na::Vector3::new(0.0, 1.0, 1.0).normalize();
        let color = bank.evaluate(&view);
        
        // Should have contributions from both lobes
        assert!(color[0] > 0.0);
        assert!(color[1] > 0.0);
    }

    #[test]
    fn test_pbr_to_asg() {
        let material = PBRMaterial {
            albedo: [1.0, 0.8, 0.6],
            metallic: 1.0, // Gold-like
            roughness: 0.2,
            ior: 1.5,
        };

        let normal = na::Vector3::new(0.0, 0.0, 1.0);
        let bank = material.to_asg_bank(&normal);

        assert!(!bank.lobes.is_empty());
    }
}
