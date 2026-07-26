
/// Physical Gauge Verification
/// Convert pixel measurements to physical units using known reference.

pub struct Gauge {
    pub pixels_per_mm: f32,
}

impl Gauge {
    /// Calibrate from two points and known physical distance
    pub fn calibrate(p1: (f32, f32), p2: (f32, f32), real_mm: f32) -> Self {
        let dx = p2.0 - p1.0;
        let dy = p2.1 - p1.1;
        let dist_px = (dx*dx + dy*dy).sqrt();
        
        Self {
            pixels_per_mm: dist_px / real_mm,
        }
    }
    
    /// Measure pixel distance and return physical mm
    pub fn measure(&self, p1: (f32, f32), p2: (f32, f32)) -> f32 {
        let dx = p2.0 - p1.0;
        let dy = p2.1 - p1.1;
        let dist_px = (dx*dx + dy*dy).sqrt();
        dist_px / self.pixels_per_mm
    }
}
