use image::{DynamicImage, GrayImage};
/// Image Quality Assurance (IQA)
/// Real-time rejection of bad frames

pub struct IQAChecker {
    thresholds: IqaThresholds,
}

impl Default for IQAChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl IQAChecker {
    pub fn new() -> Self {
        Self { thresholds: IqaThresholds::from_env() }
    }

    pub fn from_thresholds(thresholds: IqaThresholds) -> Self {
        Self { thresholds }
    }
    
    pub fn check(&self, img: &DynamicImage) -> IQAResult {
        let sharpness = crate::metrics::compute_sharpness(img);
        if sharpness < self.thresholds.min_sharpness {
            return IQAResult::Reject(format!(
                "Too blurry: {:.1} < {:.1}",
                sharpness, self.thresholds.min_sharpness
            ));
        }
        
        let gray = img.to_luma8();
        let mut sum = 0u64;
        for p in gray.pixels() { sum += p[0] as u64; }
        let mean = sum as f32 / (gray.width() * gray.height()) as f32;
        
        if mean < self.thresholds.min_brightness {
            return IQAResult::Reject("Underexposed".into());
        }
        if mean > self.thresholds.max_brightness {
            return IQAResult::Reject("Overexposed".into());
        }

        let piqe_score = compute_piqe(&gray);
        if piqe_score > self.thresholds.max_piqe_score {
            return IQAResult::Reject(format!(
                "Low perceptual quality (PIQE {:.1} > {:.1})",
                piqe_score, self.thresholds.max_piqe_score
            ));
        }
        
        IQAResult::Pass
    }
}

pub enum IQAResult {
    Pass,
    Reject(String),
}

#[derive(Debug, Clone)]
pub struct IqaThresholds {
    pub min_sharpness: f32,
    pub min_brightness: f32,
    pub max_brightness: f32,
    pub max_piqe_score: f32,
}

impl Default for IqaThresholds {
    fn default() -> Self {
        Self {
            min_sharpness: 100.0,
            min_brightness: 20.0,
            max_brightness: 240.0,
            max_piqe_score: 35.0,
        }
    }
}

impl IqaThresholds {
    pub fn from_env() -> Self {
        let mut thresholds = Self::default();
        if let Ok(val) = std::env::var("TRUESHOT_IQA_MIN_SHARPNESS") {
            if let Ok(parsed) = val.parse::<f32>() {
                thresholds.min_sharpness = parsed;
            }
        }
        if let Ok(val) = std::env::var("TRUESHOT_IQA_MIN_BRIGHTNESS") {
            if let Ok(parsed) = val.parse::<f32>() {
                thresholds.min_brightness = parsed;
            }
        }
        if let Ok(val) = std::env::var("TRUESHOT_IQA_MAX_BRIGHTNESS") {
            if let Ok(parsed) = val.parse::<f32>() {
                thresholds.max_brightness = parsed;
            }
        }
        if let Ok(val) = std::env::var("TRUESHOT_IQA_MAX_PIQE") {
            if let Ok(parsed) = val.parse::<f32>() {
                thresholds.max_piqe_score = parsed;
            }
        }
        thresholds
    }
}

fn compute_piqe(gray: &GrayImage) -> f32 {
    let (width, height) = gray.dimensions();
    if width < 16 || height < 16 {
        return 100.0;
    }

    let block = 8u32;
    let mut boundary_diff = 0.0f32;
    let mut interior_diff = 0.0f32;
    let mut boundary_count = 0.0f32;
    let mut interior_count = 0.0f32;

    for y in 0..height {
        for x in 1..width {
            let a = gray.get_pixel(x - 1, y)[0] as f32;
            let b = gray.get_pixel(x, y)[0] as f32;
            let diff = (a - b).abs();
            if x % block == 0 {
                boundary_diff += diff;
                boundary_count += 1.0;
            } else {
                interior_diff += diff;
                interior_count += 1.0;
            }
        }
    }

    for x in 0..width {
        for y in 1..height {
            let a = gray.get_pixel(x, y - 1)[0] as f32;
            let b = gray.get_pixel(x, y)[0] as f32;
            let diff = (a - b).abs();
            if y % block == 0 {
                boundary_diff += diff;
                boundary_count += 1.0;
            } else {
                interior_diff += diff;
                interior_count += 1.0;
            }
        }
    }

    let boundary_avg = boundary_diff / boundary_count.max(1.0);
    let interior_avg = interior_diff / interior_count.max(1.0);
    let blockiness = (boundary_avg / (interior_avg + 1e-3)).min(5.0);

    let mut sum = 0.0f32;
    let mut sum_sq = 0.0f32;
    let mut count = 0.0f32;
    for p in gray.pixels() {
        let v = p[0] as f32 / 255.0;
        sum += v;
        sum_sq += v * v;
        count += 1.0;
    }
    let mean = sum / count.max(1.0);
    let variance = (sum_sq / count.max(1.0)) - mean * mean;
    let contrast = variance.max(0.0).sqrt().min(1.0);

    let noise = estimate_noise(gray).min(1.0);

    let blockiness_norm = (blockiness / 3.0).min(1.0);
    let noise_norm = (noise / 0.15).min(1.0);
    let contrast_norm = contrast;

    let score = 100.0 * (0.5 * blockiness_norm + 0.3 * noise_norm + 0.2 * (1.0 - contrast_norm));
    score.clamp(0.0, 100.0)
}

fn estimate_noise(gray: &GrayImage) -> f32 {
    let (width, height) = gray.dimensions();
    if width < 3 || height < 3 {
        return 1.0;
    }

    let mut residuals = Vec::with_capacity((width * height) as usize);
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let mut sum = 0u32;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let px = (x as i32 + dx) as u32;
                    let py = (y as i32 + dy) as u32;
                    sum += gray.get_pixel(px, py)[0] as u32;
                }
            }
            let avg = sum as f32 / 9.0;
            let val = gray.get_pixel(x, y)[0] as f32;
            residuals.push((val - avg).abs() / 255.0);
        }
    }

    if residuals.is_empty() {
        return 0.0;
    }

    residuals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = residuals[residuals.len() / 2];
    (mid / 0.6745).min(1.0)
}
