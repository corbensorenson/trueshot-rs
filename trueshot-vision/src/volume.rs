/// 3D Capture Volume (Crop Box)
/// Defines the safe scanning area.
pub struct SafeVolume {
    min: [f32; 3],
    max: [f32; 3],
}

impl SafeVolume {
    pub fn new(size: f32) -> Self {
        let h = size / 2.0;
        Self {
            min: [-h, -h, -h],
            max: [h, h, h],
        }
    }

    pub fn contains(&self, p: [f32; 3]) -> bool {
        p[0] >= self.min[0]
            && p[0] <= self.max[0]
            && p[1] >= self.min[1]
            && p[1] <= self.max[1]
            && p[2] >= self.min[2]
            && p[2] <= self.max[2]
    }
}
