use crate::scanning::QualityLevel;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LensProfile {
    pub focal_length_mm: f32, // e.g. 105.0
    pub aperture_f_stop: f32, // e.g. 8.0
    pub sensor_coc_mm: f32,   // e.g. 0.029 for Full Frame, 0.019 for APS-C
}

impl Default for LensProfile {
    fn default() -> Self {
        Self {
            focal_length_mm: 100.0,
            aperture_f_stop: 11.0,
            sensor_coc_mm: 0.029, // Standard Full Frame CoC
        }
    }
}

pub struct CalibrationResult {
    /// Optimal turntable step in degrees (e.g. 10.0)
    pub turntable_step_deg: f32,
    /// Number of focus steps required
    pub focus_steps: usize,
    /// Physical step size (Focus Motor steps or mm distance)
    pub focus_step_size_mm: f32,
    /// Near limit (mm from camera)
    pub near_limit_mm: f32,
    /// Far limit (mm from camera)
    pub far_limit_mm: f32,
}

pub struct AutoCalibrator {
    lens: LensProfile,
}

impl AutoCalibrator {
    pub fn new(lens: LensProfile) -> Self {
        Self { lens }
    }

    /// Calculate optimal scan parameters based on object geometry and quality level.
    pub fn calculate(
        &self,
        quality: QualityLevel,
        object_radius_mm: f32,
        camera_distance_mm: f32,
    ) -> CalibrationResult {
        // 1. Turntable Steps
        // Higher quality = more overlap = smaller steps.
        // Heuristic:
        let turntable_step_deg = match quality {
            QualityLevel::Preview => 20.0,
            QualityLevel::Standard => 10.0, // 36 views
            QualityLevel::High => 5.0,      // 72 views
            QualityLevel::Ultra => 2.0,     // 180 views
        };

        // 2. Focus Bracketing (Depth of Field)
        // DoF Total = 2 * N * c * D^2 / f^2 (approx for D >> f, but macro is different)
        // Full Macro DoF ~ 2 * N * c * ((m+1) / m^2) ... relative to magnification.
        // Let's use standard approximation for subject distance D.
        // Front DoF ~ (N * c * D^2) / (f^2 + N * c * D)
        // Rear DoF ~ (N * c * D^2) / (f^2 - N * c * D)
        
        let f = self.lens.focal_length_mm;
        let n = self.lens.aperture_f_stop;
        let c = self.lens.sensor_coc_mm;
        let d = camera_distance_mm;

        // Depth of Field (Total) ~ 2 * N * c * (D/f)^2
        // Only valid if D >> f. For macro (1:1), use (2 * N * c * (1+m)^2 ) / m^2 ?
        // Let's use a robust approximation:
        // Hyperfocal H = f^2 / (N * c)
        // Near = H * D / (H + D)
        // Far = H * D / (H - D)
        
        // But we want the "Safe Step Size" to ensure overlap.
        // Step Size <= DoF * OverlapFactor (0.7)
        
        // Let's compute a simplified "Slice Depth"
        // At distance D, with magnification M ~ f / (D - f)
        let mag = f / (d - f).max(1.0);
        
        // Macro DoF approx: 2 * N * c * ((1 + M) / M^2)
        // If M is small (< 0.1), use standard.
        
        let dof_mm = if mag > 0.1 {
            // Macro regime
            2.0 * n * c * ((1.0 + mag) / (mag * mag))
        } else {
            // Standard regime
             (2.0 * n * c * d * d) / (f * f)
        };
        
        // Safety / Overlap factor
        let overlap = match quality {
            QualityLevel::Preview => 1.5, // Gaps allowed? No, just huge steps.
            QualityLevel::Standard => 0.8, // 20% overlap
            QualityLevel::High => 0.6,    // 40% overlap
            QualityLevel::Ultra => 0.5,   // 50% overlap - super dense
        };
        
        let step_size_mm = dof_mm * overlap;

        // 3. Scan Range
        // Perfect Sphere Assumption
        let margin_mm = object_radius_mm * 0.2; // 20% margin
        let near = d - object_radius_mm - margin_mm;
        let far = d + object_radius_mm + margin_mm;
        
        let total_depth = far - near;
        let steps = (total_depth / step_size_mm).ceil() as usize;
        
        // Clamp steps reasonably
        let steps = steps.max(1).min(1000); 

        CalibrationResult {
            turntable_step_deg,
            focus_steps: steps,
            focus_step_size_mm: step_size_mm,
            near_limit_mm: near,
            far_limit_mm: far,
        }
    }
    pub fn calculate_exposure_brackets(
        &self,
        frame: &image::ImageBuffer<image::Rgb<u8>, Vec<u8>>,
    ) -> Vec<f32> {
        // Histogram Analysis
        let mut hist_low = 0;   // Shadows (e.g. < 10)
        let mut hist_high = 0;  // Highlights (e.g. > 245)
        let mut total = 0;
        
        for p in frame.pixels() {
            let l = (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) as u8;
            if l < 10 { hist_low += 1; }
            if l > 245 { hist_high += 1; }
            total += 1; 
        }
        
        let pct_low = hist_low as f32 / total as f32;
        let pct_high = hist_high as f32 / total as f32;
        
        let mut stops = vec![0.0]; // Always include base exposure
        
        if pct_low > 0.05 {
            // Significant shadows crushed -> Need Overexposure (longer shutter)
            stops.push(2.0); 
        }
        
        if pct_high > 0.02 {
            // Highlights clipped -> Need Underexposure (faster shutter)
            stops.push(-2.0);
        }
        
        // ensure sorted
        stops.sort_by(|a, b| a.partial_cmp(b).unwrap());
        stops
    }
}
