/// Feedback Loop for Real-time Correction
pub struct Command {
    pub angle_delta: f32, // Degrees
    pub elevation_delta: f32, // Degrees
    pub take_photos: u8,
}

pub fn check_coverage_and_correct(density: &[u32], _grid_size: (u32, u32, u32)) -> Option<Command> {
    let threshold = 5; // Min points per voxel
    let mut bad_voxels = 0;
    
    for d in density {
        if *d < threshold { bad_voxels += 1; }
    }
    
    let ratio = bad_voxels as f32 / density.len() as f32;
    if ratio > 0.3 {
        // Assume the "back" is missing (heuristic)
        // In reality, we'd map voxel index to world angle.
        return Some(Command { angle_delta: 180.0, elevation_delta: 0.0, take_photos: 3 });
    }
    None
}
