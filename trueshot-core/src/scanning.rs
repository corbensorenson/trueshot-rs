//! Smart Scanning Logic (SOTA Item 2)
//!
//! Adaptive scanning strategy that analyzes point cloud density to identify
//! coverage gaps and automatically target them.
//! Ported and mathematicaly purified from `MultiCam3DScanner`.

pub mod profiles;
pub mod workflow;
pub mod session;
pub mod rig;
pub mod calibration;
pub mod tasks;

use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum QualityLevel {
    Preview,
    Standard,
    High,
    Ultra,
}

impl QualityLevel {
    pub fn step_size(&self) -> f32 {
        match self {
            QualityLevel::Preview => 20.0,
            QualityLevel::Standard => 10.0,
            QualityLevel::High => 5.0,
            QualityLevel::Ultra => 2.0,
        }
    }
}

pub struct SmartScanStrategy {
    visited_angles: HashSet<i32>,
    target_angles: Vec<f32>,
    quality: QualityLevel,
}

impl SmartScanStrategy {
    pub fn new(quality: QualityLevel) -> Self {
        Self {
            visited_angles: HashSet::new(),
            target_angles: Vec::new(),
            quality,
        }
    }

    /// Mark an angle as visited
    pub fn visit(&mut self, angle: f32) {
        self.visited_angles.insert(angle.round() as i32);
    }

    /// Check coverage and return next angle to scan
    pub fn next_angle(&mut self) -> Option<f32> {
        // 1. Priority: Explicit target angles (from gap analysis)
        if let Some(target) = self.target_angles.pop() {
            if !self.is_visited(target) {
                return Some(target);
            }
        }

        // 2. Secondary: Regular sweep
        let step = self.quality.step_size();
        let mut angle = 0.0;
        while angle < 360.0 {
            if !self.is_visited(angle) {
                return Some(angle);
            }
            angle += step;
        }

        None // Scan complete
    }

    fn is_visited(&self, angle: f32) -> bool {
        // Fuzzy match within 1 degree
        let a = angle.round() as i32;
        self.visited_angles.contains(&a) || 
        self.visited_angles.contains(&(a - 1)) || 
        self.visited_angles.contains(&(a + 1))
    }

    /// Analyze sparse point cloud for gaps
    /// Input: list of (x,y,z) coverage points normalized to unit sphere
    pub fn analyze_coverage(&mut self, points: &[(f64, f64, f64)]) {
        // SOTA: Spherical Histogram / Fibonacci Sphere binning
        // For simplicity in this v1: Sector analysis
        
        let sectors = 36; // 10 degree sectors
        let mut sector_counts = vec![0; sectors];
        
        for (x, _, z) in points {
            let angle = z.atan2(*x).to_degrees();
            let norm_angle = if angle < 0.0 { angle + 360.0 } else { angle };
            let sector = (norm_angle / 10.0).floor() as usize % sectors;
            sector_counts[sector] += 1;
        }

        // Identify sectors with low density relative to mean
        let total: usize = sector_counts.iter().sum();
        if total == 0 { return; }
        let mean = total as f64 / sectors as f64;
        let threshold = mean * 0.5;

        for (i, &count) in sector_counts.iter().enumerate() {
            if (count as f64) < threshold {
                let angle = i as f32 * 10.0 + 5.0; // Center of sector
                if !self.is_visited(angle) {
                    self.target_angles.push(angle);
                }
            }
        }
        
        // Sort targets to optimize travel time (minimal rotation)
        self.target_angles.sort_by(|a, b| b.partial_cmp(a).unwrap());
    }
}
