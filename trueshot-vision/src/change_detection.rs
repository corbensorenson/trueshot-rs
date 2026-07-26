use image::{ImageBuffer, Rgb};

/// Detects stability of the scene to trigger actions
pub struct SceneChangeDetector {
    last_frame: Option<ImageBuffer<Rgb<u8>, Vec<u8>>>,
    diff_threshold: f32,
    stability_counter: u32,
}

impl Default for SceneChangeDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneChangeDetector {
    pub fn new() -> Self {
        Self {
            last_frame: None,
            diff_threshold: 0.05, // 5% pixel change
            stability_counter: 0,
        }
    }
    
    /// Returns true if scene is "Stable" (no movement for N frames)
    pub fn update(&mut self, frame: &ImageBuffer<Rgb<u8>, Vec<u8>>) -> bool {
        if let Some(last) = &self.last_frame {
            let diff = self.compute_diff_percent(last, frame, 10); // Check every 10th pixel for speed
            
            if diff < self.diff_threshold {
                self.stability_counter += 1;
            } else {
                self.stability_counter = 0;
            }
        } else {
            // First frame is unstable by definition
            self.stability_counter = 0;
        }
        
        // Update last frame (clone is expensive, in real impl use ring buffer)
        self.last_frame = Some(frame.clone());
        
        // Stable if > 30 frames (1 second at 30fps)
        self.stability_counter > 30
    }
    
    fn compute_diff_percent(&self, a: &ImageBuffer<Rgb<u8>, Vec<u8>>, b: &ImageBuffer<Rgb<u8>, Vec<u8>>, step: usize) -> f32 {
        let (w, h) = a.dimensions();
        if b.dimensions() != (w, h) { return 1.0; } // Dimensions changed = HUGE change
        
        let mut diff_sum = 0.0;
        let mut count = 0;
        
        for y in (0..h).step_by(step) {
            for x in (0..w).step_by(step) {
                let p1 = a.get_pixel(x, y);
                let p2 = b.get_pixel(x, y);
                
                let d = (p1[0] as i16 - p2[0] as i16).abs() as f32 +
                        (p1[1] as i16 - p2[1] as i16).abs() as f32 +
                        (p1[2] as i16 - p2[2] as i16).abs() as f32;
                
                // If pixel changed significantly
                if d > 30.0 {
                    diff_sum += 1.0;
                }
                count += 1;
            }
        }
        
        if count == 0 { return 0.0; }
        diff_sum / count as f32
    }
    
    pub fn reset(&mut self) {
        self.stability_counter = 0;
        self.last_frame = None;
    }
}
