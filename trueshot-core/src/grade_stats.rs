use anyhow::Result;
use ndarray::{Array2, Array3, Axis};

#[derive(Debug, Clone, Default)]
pub struct ImageStats {
    pub mean: f32,
    pub std_dev: f32,
    pub min: f32,
    pub max: f32,
    pub dynamic_range_stops: f32,
    pub noise_floor: f32,
    pub histogram: Vec<u32>, // 256 bins for 8-bit preview
}

/// Compute statistics for a Bayer frame (or any single channel float array)
pub fn compute_grade_stats(data: &Array2<f32>) -> Result<ImageStats> {
    let mut min = f32::MAX;
    let mut max = f32::MIN;
    let mut sum = 0.0;
    
    // 1-pass for min/max/sum
    // Note: Array2 iteration is efficient in ndarray
    for &val in data.iter() {
        if val < min { min = val; }
        if val > max { max = val; }
        sum += val;
    }
    
    let n = data.len() as f32;
    let mean = sum / n;
    
    // 2-pass for std_dev and histogram
    let mut sum_sq_diff = 0.0;
    let mut histogram = vec![0u32; 256];
    
    for &val in data.iter() {
        let diff = val - mean;
        sum_sq_diff += diff * diff;
        
        // Histogram (assume normalized 0-1 input, map to 0-255)
        let bin = ((val * 255.0).clamp(0.0, 255.0) as usize).min(255);
        histogram[bin] += 1;
    }
    
    let std_dev = (sum_sq_diff / n).sqrt();
    
    // Estimate dynamic range
    // Avoid log(0)
    let safe_min = if min < 1e-6 { 1e-6 } else { min };
    let dr_stops = (max / safe_min).log2();
    
    Ok(ImageStats {
        mean,
        std_dev,
        min,
        max,
        dynamic_range_stops: dr_stops,
        noise_floor: std_dev, // Simplistic noise floor estimate
        histogram,
    })
}

/// Analyze channel correlation (covariance) for color grading
pub fn compute_covariance(rgb: &Array3<f64>) -> Result<Array2<f64>> {
    let (h, w, c) = (rgb.len_of(Axis(0)), rgb.len_of(Axis(1)), rgb.len_of(Axis(2)));
    if c != 3 {
        anyhow::bail!("Input must be RGB");
    }
    
    // Flatten to (N, 3) matrix
    let n = h * w;
    let mut flat = Array2::<f64>::zeros((n, 3));
    
    let mut idx = 0;
    for y in 0..h {
        for x in 0..w {
            flat[[idx, 0]] = rgb[[y, x, 0]];
            flat[[idx, 1]] = rgb[[y, x, 1]];
            flat[[idx, 2]] = rgb[[y, x, 2]];
            idx += 1;
        }
    }
    
    // Compute Covariance Matrix (3x3)
    // Cov(X, Y) = E[(X - E[X])(Y - E[Y])]
    
    let mut means = [0.0; 3];
    for col in 0..3 {
        means[col] = flat.column(col).sum() / n as f64;
    }
    
    let mut cov = Array2::<f64>::zeros((3, 3));
    
    for i in 0..3 {
        for j in 0..3 {
            let mut sum_prod = 0.0;
            // Vectorized operation would be better, but loop is explicit
            for k in 0..n {
                sum_prod += (flat[[k, i]] - means[i]) * (flat[[k, j]] - means[j]);
            }
            cov[[i, j]] = sum_prod / (n - 1) as f64;
        }
    }
    
    Ok(cov)
}
