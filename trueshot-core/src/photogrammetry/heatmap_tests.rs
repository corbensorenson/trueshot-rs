
#[cfg(test)]
mod tests {
    use super::*;
    use nalgebra as na;
    use crate::photogrammetry::heatmap::{CoverageDensity, CoverageVoxelGrid};
    use crate::reconstruction::ColoredPoint;

    #[test]
    fn test_density_from_count() {
        assert_eq!(CoverageDensity::from_point_count(0, 100), CoverageDensity::None);
        assert_eq!(CoverageDensity::from_point_count(10, 100), CoverageDensity::VeryLow);
        assert_eq!(CoverageDensity::from_point_count(30, 100), CoverageDensity::Low);
        assert_eq!(CoverageDensity::from_point_count(50, 100), CoverageDensity::Medium);
        assert_eq!(CoverageDensity::from_point_count(70, 100), CoverageDensity::Good);
        assert_eq!(CoverageDensity::from_point_count(90, 100), CoverageDensity::Excellent);
    }

    #[test]
    fn test_voxel_grid_stats() {
        let mut grid = CoverageVoxelGrid::new(1.0);
        let points = vec![
            ColoredPoint {
                position: na::Point3::new(0.5, 0.5, 0.5),
                color: [255, 255, 255],
                confidence: 1.0,
            },
        ];
        
        grid.add_points(&points);
        let stats = grid.get_stats();
        
        // With 1 point and 1 voxel, density is 100% (Excellent)
        assert_eq!(stats.total_voxels, 1);
        assert_eq!(stats.excellent_count, 1);
        assert_eq!(stats.good_coverage_percent(), 100.0);
    }

    #[test]
    fn test_color_conversions() {
       assert_eq!(CoverageDensity::VeryLow.to_color(), [255, 0, 0]);
       assert_eq!(CoverageDensity::Excellent.to_color(), [0, 255, 0]);
    }
}
