use crate::reconstruction::{ColoredPoint, ReconstructionStats};
use std::collections::HashMap;

/// Direction to rotate object
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RotationDirection {
    Left,      // Rotate left (counterclockwise)
    Right,     // Rotate right (clockwise)
    Up,        // Tilt up
    Down,      // Tilt down
    Flip,      // Flip over to show bottom
    Any,       // Any rotation is fine
}

impl RotationDirection {
    pub fn to_arrow(&self) -> &'static str {
        match self {
            Self::Left => "⬅️",
            Self::Right => "➡️",
            Self::Up => "⬆️",
            Self::Down => "⬇️",
            Self::Flip => "🔄",
            Self::Any => "🔁",
        }
    }

    pub fn to_description(&self) -> &'static str {
        match self {
            Self::Left => "Rotate LEFT",
            Self::Right => "Rotate RIGHT",
            Self::Up => "Tilt UP",
            Self::Down => "Tilt DOWN",
            Self::Flip => "FLIP OVER to show bottom",
            Self::Any => "Rotate slowly",
        }
    }
}

/// Guidance message to display to user
#[derive(Debug, Clone, PartialEq)]
pub enum GuidanceMessage {
    StartScanning,
    RotateObject,
    RotateInDirection(RotationDirection), // NEW: Specific rotation direction
    RotateToShowCavity,
    MoveCloser,
    MoveFurther,
    GoodCoverage,
    ReadyToExport,
    Custom(String), // Custom message
}

impl GuidanceMessage {
    pub fn to_string(&self) -> String {
        match self {
            Self::StartScanning => "Click 'Start New Scan' to begin".to_string(),
            Self::RotateObject => "Slowly rotate the object".to_string(),
            Self::RotateInDirection(dir) => format!("{} {} to scan missing areas", dir.to_arrow(), dir.to_description()),
            Self::RotateToShowCavity => "Rotate to show hidden areas".to_string(),
            Self::MoveCloser => "Move cameras closer for more detail".to_string(),
            Self::MoveFurther => "Move cameras back to see full object".to_string(),
            Self::GoodCoverage => "✅ Good coverage - continue scanning".to_string(),
            Self::ReadyToExport => "🎉 Scan complete - ready to export!".to_string(),
            Self::Custom(msg) => msg.clone(),
        }
    }
}

/// Analyzes reconstruction quality and provides guidance
pub struct GuidanceSystem {
    last_point_count: usize,
    frames_without_progress: usize,
    total_frames_processed: usize,
    last_coverage: Option<CoverageAnalysis>,
}

impl GuidanceSystem {
    pub fn new() -> Self {
        Self {
            last_point_count: 0,
            frames_without_progress: 0,
            total_frames_processed: 0,
            last_coverage: None,
        }
    }

    /// Analyze current reconstruction and provide guidance
    pub fn analyze(&mut self, stats: &ReconstructionStats, point_cloud: &[ColoredPoint]) -> GuidanceMessage {
        self.total_frames_processed += 1;
        
        // No data yet
        if stats.point_count == 0 {
            return GuidanceMessage::StartScanning;
        }
        
        // Check for progress
        let point_increase = stats.point_count.saturating_sub(self.last_point_count);
        
        if point_increase < 10 {
            self.frames_without_progress += 1;
        } else {
            self.frames_without_progress = 0;
        }
        
        self.last_point_count = stats.point_count;
        
        // Early stage - just starting
        if stats.point_count < 100 {
            return GuidanceMessage::RotateObject;
        }
        
        // Check coverage quality
        let coverage = self.analyze_coverage(point_cloud);
        self.last_coverage = Some(coverage.clone());

        // Not enough progress - might need to show different angle
        if self.frames_without_progress > 30 {
            if coverage.has_gaps {
                // Detect which direction has the most gaps
                let gap_direction = self.detect_gap_direction(point_cloud);
                return GuidanceMessage::RotateInDirection(gap_direction);
            } else {
                return GuidanceMessage::RotateInDirection(RotationDirection::Any);
            }
        }

        // Check point density
        if coverage.avg_density < 0.3 && stats.point_count < 1000 {
            return GuidanceMessage::MoveCloser;
        }

        // Good amount of data
        if stats.point_count > 5000 && coverage.uniformity > 0.6 {
            return GuidanceMessage::ReadyToExport;
        }

        // Default - keep scanning with directional guidance
        if coverage.uniformity > 0.4 {
            GuidanceMessage::GoodCoverage
        } else {
            // Suggest rotation direction based on coverage gaps
            let gap_direction = self.detect_gap_direction(point_cloud);
            GuidanceMessage::RotateInDirection(gap_direction)
        }
    }

    /// Analyze coverage quality of point cloud
    fn analyze_coverage(&self, points: &[ColoredPoint]) -> CoverageAnalysis {
        if points.is_empty() {
            return CoverageAnalysis::default();
        }
        
        // Divide space into voxels and count points per voxel
        let voxel_size = 0.05; // 5cm voxels
        let mut voxel_counts: HashMap<(i32, i32, i32), usize> = HashMap::new();
        
        for point in points {
            let voxel = (
                (point.position.x / voxel_size).floor() as i32,
                (point.position.y / voxel_size).floor() as i32,
                (point.position.z / voxel_size).floor() as i32,
            );
            *voxel_counts.entry(voxel).or_insert(0) += 1;
        }
        
        // Calculate statistics
        let total_voxels = voxel_counts.len();
        let occupied_voxels = voxel_counts.values().filter(|&&c| c > 0).count();
        
        let counts: Vec<usize> = voxel_counts.values().copied().collect();
        let avg_density = if !counts.is_empty() {
            counts.iter().sum::<usize>() as f32 / counts.len() as f32
        } else {
            0.0
        };
        
        // Check for gaps (voxels with very few points surrounded by occupied voxels)
        let has_gaps = self.detect_gaps(&voxel_counts);
        
        // Uniformity: how evenly distributed are the points
        let variance = if counts.len() > 1 {
            let mean = avg_density;
            let var = counts.iter()
                .map(|&c| (c as f32 - mean).powi(2))
                .sum::<f32>() / counts.len() as f32;
            var.sqrt()
        } else {
            0.0
        };
        
        let uniformity = if avg_density > 0.0 {
            (1.0 - (variance / avg_density).min(1.0)).max(0.0)
        } else {
            0.0
        };
        
        CoverageAnalysis {
            total_voxels,
            occupied_voxels,
            avg_density: avg_density / 100.0, // Normalize
            uniformity,
            has_gaps,
        }
    }

    /// Detect gaps in coverage (simple heuristic)
    fn detect_gaps(&self, voxel_counts: &HashMap<(i32, i32, i32), usize>) -> bool {
        // Check if there are neighboring voxels with very different counts
        for (voxel, &count) in voxel_counts.iter() {
            if count < 2 {
                continue; // Skip sparse voxels
            }

            // Check 6-connected neighbors
            let neighbors = [
                (voxel.0 + 1, voxel.1, voxel.2),
                (voxel.0 - 1, voxel.1, voxel.2),
                (voxel.0, voxel.1 + 1, voxel.2),
                (voxel.0, voxel.1 - 1, voxel.2),
                (voxel.0, voxel.1, voxel.2 + 1),
                (voxel.0, voxel.1, voxel.2 - 1),
            ];

            let empty_neighbors = neighbors.iter()
                .filter(|n| !voxel_counts.contains_key(n))
                .count();

            // If more than half neighbors are empty, we have a gap
            if empty_neighbors > 3 {
                return true;
            }
        }

        false
    }

    /// Detect which direction has the most coverage gaps
    /// Returns suggested rotation direction to fill gaps
    fn detect_gap_direction(&self, points: &[ColoredPoint]) -> RotationDirection {
        if points.is_empty() {
            return RotationDirection::Any;
        }

        // Analyze point distribution in different directions
        // Count points in each hemisphere
        let mut left_count = 0;   // Negative X
        let mut right_count = 0;  // Positive X
        let mut top_count = 0;    // Positive Y
        let mut bottom_count = 0; // Negative Y
        let mut front_count = 0;  // Positive Z
        let mut back_count = 0;   // Negative Z

        for point in points {
            if point.position.x < 0.0 { left_count += 1; } else { right_count += 1; }
            if point.position.y < 0.0 { bottom_count += 1; } else { top_count += 1; }
            if point.position.z < 0.0 { back_count += 1; } else { front_count += 1; }
        }

        // Find the direction with the least coverage
        let total = points.len() as f32;
        let left_ratio = left_count as f32 / total;
        let right_ratio = right_count as f32 / total;
        let top_ratio = top_count as f32 / total;
        let bottom_ratio = bottom_count as f32 / total;

        // If bottom is severely under-represented, suggest flipping
        if bottom_ratio < 0.15 && top_ratio > 0.4 {
            return RotationDirection::Flip;
        }

        // Find the most under-represented horizontal direction
        let min_horizontal = left_ratio.min(right_ratio);
        let min_vertical = top_ratio.min(bottom_ratio);

        // Prioritize horizontal rotation (more natural)
        if min_horizontal < 0.25 {
            if left_ratio < right_ratio {
                RotationDirection::Right // Rotate right to show left side
            } else {
                RotationDirection::Left // Rotate left to show right side
            }
        } else if min_vertical < 0.25 {
            if top_ratio < bottom_ratio {
                RotationDirection::Down // Tilt down to show top
            } else {
                RotationDirection::Up // Tilt up to show bottom
            }
        } else {
            RotationDirection::Any
        }
    }

    /// Reset guidance state (when starting new scan)
    pub fn reset(&mut self) {
        self.last_point_count = 0;
        self.frames_without_progress = 0;
        self.total_frames_processed = 0;
        self.last_coverage = None;
    }

    /// Get coverage statistics for display
    pub fn get_coverage_stats(&self) -> Option<CoverageStats> {
        self.last_coverage.as_ref().map(|cov| CoverageStats {
            coverage_percent: (cov.occupied_voxels as f32 / cov.total_voxels.max(1) as f32 * 100.0).min(100.0),
            uniformity_percent: (cov.uniformity * 100.0).min(100.0),
            has_gaps: cov.has_gaps,
            total_voxels: cov.total_voxels,
        })
    }

    /// Get progress percentage (0-100)
    pub fn get_progress(&self) -> f32 {
        // Simple heuristic based on point count
        let target_points = 5000.0;
        ((self.last_point_count as f32 / target_points) * 100.0).min(100.0)
    }
}

#[derive(Debug, Clone)]
struct CoverageAnalysis {
    total_voxels: usize,
    occupied_voxels: usize,
    avg_density: f32,
    uniformity: f32,
    has_gaps: bool,
}

impl Default for CoverageAnalysis {
    fn default() -> Self {
        Self {
            total_voxels: 0,
            occupied_voxels: 0,
            avg_density: 0.0,
            uniformity: 0.0,
            has_gaps: false,
        }
    }
}

/// Public coverage statistics for UI display
#[derive(Debug, Clone)]
pub struct CoverageStats {
    pub coverage_percent: f32,    // 0-100%
    pub uniformity_percent: f32,  // 0-100%
    pub has_gaps: bool,
    pub total_voxels: usize,
}

impl Default for GuidanceSystem {
    fn default() -> Self {
        Self::new()
    }
}
