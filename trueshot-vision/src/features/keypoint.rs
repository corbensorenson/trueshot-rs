//! Keypoint representation for feature detection

/// A detected keypoint in an image
#[derive(Debug, Clone)]
pub struct Keypoint {
    /// X coordinate (sub-pixel)
    pub x: f32,
    /// Y coordinate (sub-pixel)
    pub y: f32,
    /// Corner response / score
    pub response: f32,
    /// Orientation in radians (for rotation invariance)
    pub angle: f32,
    /// Scale octave (for multi-scale detection)
    pub octave: i32,
    /// Feature size
    pub size: f32,
}

impl Keypoint {
    pub fn new(x: f32, y: f32, response: f32) -> Self {
        Self {
            x,
            y,
            response,
            angle: 0.0,
            octave: 0,
            size: 1.0,
        }
    }

    /// Distance to another keypoint
    pub fn distance_to(&self, other: &Keypoint) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
}
