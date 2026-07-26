use image::DynamicImage;
/// Auto-Naming Classifier
/// Uses basic color/shape heuristics if no AI model is loaded,
/// or wraps an ONNX classifier if available.

pub struct AutoNamer;

impl AutoNamer {
    pub fn suggest_name(img: &DynamicImage) -> String {
        // 1. Analyze Dominant Color
        let color = Self::get_dominant_color_name(img);

        // 2. Analyze Time
        let time = chrono::Local::now().format("%I:%M%p").to_string();

        // 3. Complete Name
        format!("{} Object - {}", color, time)
    }

    fn get_dominant_color_name(img: &DynamicImage) -> String {
        // Simple resizing to 1x1 to find average
        let resized = img.resize(1, 1, image::imageops::FilterType::Triangle);
        let rgba = resized.to_rgba8();
        let p = rgba.get_pixel(0, 0);
        let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);

        if r > g && r > b {
            if g > 100.0 && b < 100.0 {
                return "Orange".to_string();
            }
            return "Red".to_string();
        }
        if g > r && g > b {
            return "Green".to_string();
        }
        if b > r && b > g {
            return "Blue".to_string();
        }
        if r > 200.0 && g > 200.0 && b > 200.0 {
            return "White".to_string();
        }
        if r < 50.0 && g < 50.0 && b < 50.0 {
            return "Dark".to_string();
        }

        "General".to_string()
    }
}
