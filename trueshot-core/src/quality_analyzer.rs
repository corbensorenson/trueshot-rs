
use anyhow::Result;
use ndarray::{Array2, Array3};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
// ... imports

/// Defect types...
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Defect {
    /// Edge banding artifacts (high gradient kurtosis)
    EdgeBanding,
    /// Background not fully removed (boundary irregularity)
    BackgroundLeak,
    /// Object erosion (contour area loss)
    ObjectErosion,
    /// Insufficient sharpness (low Laplacian variance)
    Blur,
    /// Overexposure (histogram clipping)
    Overexposure,
    /// Color cast (channel imbalance)
    ColorCast,
    /// Black dots/holes in foreground (zero or near-zero pixels)
    BlackDots,
    /// Raw exposure issues
    RawUnderexposed,
}

impl Defect {
    pub fn description(&self) -> &'static str {
        match self {
            Defect::EdgeBanding => "Edge banding artifacts detected",
            Defect::BackgroundLeak => "Background not fully removed",
            Defect::ObjectErosion => "Object boundary eroded",
            Defect::Blur => "Image not sufficiently sharp",
            Defect::Overexposure => "Overexposed regions detected",
            Defect::ColorCast => "Color cast detected",
            Defect::BlackDots => "Black dots/holes in foreground",
            Defect::RawUnderexposed => "Raw data underexposed",
        }
    }

    pub fn is_low_bad(&self) -> bool {
        matches!(self, Defect::ObjectErosion | Defect::Blur | Defect::RawUnderexposed)
    }
}

// ... ProcessingParams ...
#[derive(Debug, Clone)]
pub struct ProcessingParams {
    /// Sharpness threshold (median + N*std for Laplacian mask)
    pub sharp_theta: f64,
    /// Chan-Vese smoothness parameter for background removal
    pub chanvese_mu: f64,
    /// Collapse sparsity regularization (λ for wavelet/curvelet)
    pub collapse_lambda: f64,
    /// WFC propagation strength
    pub wfc_beta: f64,
    /// Otsu threshold multiplier for foreground detection
    pub otsu_multiplier: f64,
}

impl Default for ProcessingParams {
    fn default() -> Self {
        Self {
            sharp_theta: 0.5,
            chanvese_mu: 0.001,
            collapse_lambda: 1e-4,
            wfc_beta: 0.2,
            otsu_multiplier: 1.0,
        }
    }
}

pub struct Assessment {
    pub scores: HashMap<Defect, f64>,
    pub pass: bool,
    pub reasons: Vec<String>,
}

pub struct Analyzer {
    thresholds: HashMap<Defect, f64>,
    params: Arc<Mutex<ProcessingParams>>,
}

/// Raw Histogram Analysis
#[derive(Debug, Clone)]
pub struct RawHistogram {
    pub bins: Vec<u64>,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
}

impl Analyzer {
    pub fn new(params: Arc<Mutex<ProcessingParams>>) -> Self {
        let mut thresholds = HashMap::new();
        thresholds.insert(Defect::EdgeBanding, 3.0);
        thresholds.insert(Defect::BackgroundLeak, 0.1);
        thresholds.insert(Defect::ObjectErosion, 0.8);
        thresholds.insert(Defect::Blur, 100.0);
        thresholds.insert(Defect::Overexposure, 0.05);
        thresholds.insert(Defect::ColorCast, 15.0);
        thresholds.insert(Defect::BlackDots, 0.01);
        thresholds.insert(Defect::RawUnderexposed, 0.2); // >20% pixels too dark
        
        Self { thresholds, params }
    }
    
    // ... assess ...
    pub fn assess(
        &self,
        rgb: &Array3<u8>,
        _depth: &Array2<f32>,
        mask: &Array2<u8>,
    ) -> Result<Assessment> {
        // [Existing logic stays same, I will rewrite it to be safe]
        let mut scores = HashMap::new();
        let mut reasons = Vec::new();
        
        // 1. Edge Banding
        let edge_score = self.detect_edge_banding(rgb)?;
        scores.insert(Defect::EdgeBanding, edge_score);
        if edge_score > self.thresholds[&Defect::EdgeBanding] {
             reasons.push(format!("{} (score={:.2})", Defect::EdgeBanding.description(), edge_score));
        }

        // 2. Background Leak
        let bg_score = self.detect_background_leak(mask)?;
        scores.insert(Defect::BackgroundLeak, bg_score);
        if bg_score > self.thresholds[&Defect::BackgroundLeak] {
            reasons.push(format!("{} (score={:.2})", Defect::BackgroundLeak.description(), bg_score));
        }

        // 3. Object Erosion
        let erosion_score = self.detect_object_erosion(mask)?;
        scores.insert(Defect::ObjectErosion, erosion_score);
        if erosion_score < self.thresholds[&Defect::ObjectErosion] {
            reasons.push(format!("{} (ratio={:.2})", Defect::ObjectErosion.description(), erosion_score));
        }

        // 4. Blur
        let blur_score = self.detect_blur(rgb)?;
        scores.insert(Defect::Blur, blur_score);
        if blur_score < self.thresholds[&Defect::Blur] {
            reasons.push(format!("{} (var={:.2})", Defect::Blur.description(), blur_score));
        }

        // 5. Overexposure
        let overexp = self.detect_overexposure(rgb)?;
        scores.insert(Defect::Overexposure, overexp);
        if overexp > self.thresholds[&Defect::Overexposure] {
            reasons.push(format!("{} (ratio={:.2})", Defect::Overexposure.description(), overexp));
        }

        // 6. Color Cast
        let cast = self.detect_color_cast(rgb)?;
        scores.insert(Defect::ColorCast, cast);
        if cast > self.thresholds[&Defect::ColorCast] {
            reasons.push(format!("{} (score={:.2})", Defect::ColorCast.description(), cast));
        }

        // 7. Black Dots
        let black = self.detect_black_dots(rgb, mask)?;
        scores.insert(Defect::BlackDots, black);
        if black > self.thresholds[&Defect::BlackDots] {
            reasons.push(format!("{} (ratio={:.4})", Defect::BlackDots.description(), black));
        }

        let pass = reasons.is_empty();
        Ok(Assessment { scores, pass, reasons })
    }

    pub fn thresholds(&self) -> HashMap<Defect, f64> {
        self.thresholds.clone()
    }

    /// Compute raw histogram for analysis (Feature Request 9)
    pub fn compute_raw_histogram(bayer: &Array2<f64>) -> RawHistogram {
        let (h, w) = bayer.dim();
        let mut bins = vec![0u64; 256];
        let mut min_val = f64::MAX;
        let mut max_val = f64::MIN;
        let mut sum = 0.0;
        let count = (h * w) as f64;

        for v in bayer.iter() {
            let val = *v;
            if val < min_val { min_val = val; }
            if val > max_val { max_val = val; }
            sum += val;

            // Map 0.0-1.0 to 0-255
            let bin = (val * 255.0).clamp(0.0, 255.0) as usize;
            bins[bin] += 1;
        }

        RawHistogram {
            bins,
            min: min_val,
            max: max_val,
            mean: sum / count,
        }
    }

    // ... Helper detections methods from previous file ...
    fn detect_edge_banding(&self, rgb: &Array3<u8>) -> Result<f64> {
        let gray = rgb_to_gray(rgb);
        let grad_x = sobel_x(&gray);
        let grad_y = sobel_y(&gray);
        let kx = compute_kurtosis(&grad_x);
        let ky = compute_kurtosis(&grad_y);
        Ok(kx + ky)
    }

    fn detect_background_leak(&self, _mask: &Array2<u8>) -> Result<f64> {
        let (h, w) = _mask.dim();
        if h == 0 || w == 0 {
            return Ok(0.0);
        }
        let border = ((h.min(w) as f64) * 0.05).round() as usize;
        let border = border.max(2).min(h.min(w).saturating_sub(1));

        let mut border_fg = 0usize;
        let mut border_total = 0usize;

        for y in 0..h {
            for x in 0..w {
                if y < border || y + border >= h || x < border || x + border >= w {
                    border_total += 1;
                    if _mask[[y, x]] > 128 {
                        border_fg += 1;
                    }
                }
            }
        }

        if border_total == 0 {
            return Ok(0.0);
        }
        Ok(border_fg as f64 / border_total as f64)
    }

    fn detect_object_erosion(&self, mask: &Array2<u8>) -> Result<f64> {
        let (h, w) = mask.dim();
        if h == 0 || w == 0 {
            return Ok(0.0);
        }

        let mut area = 0usize;
        let mut boundary_points: Vec<(i32, i32)> = Vec::new();

        for y in 0..h {
            for x in 0..w {
                if mask[[y, x]] > 128 {
                    area += 1;
                    if is_boundary_pixel(mask, x, y) {
                        if (x + y) % 2 == 0 {
                            boundary_points.push((x as i32, y as i32));
                        }
                    }
                }
            }
        }

        if area == 0 {
            return Ok(0.0);
        }

        if boundary_points.len() < 3 {
            return Ok(1.0);
        }

        let hull = convex_hull(&mut boundary_points);
        let hull_area = polygon_area(&hull).abs();
        if hull_area <= 1e-6 {
            return Ok(1.0);
        }

        Ok(area as f64 / hull_area.max(1.0))
    }

    fn detect_blur(&self, rgb: &Array3<u8>) -> Result<f64> {
        let gray = rgb_to_gray(rgb);
        let lap = compute_laplacian(&gray);
        // variance
        let mean = lap.mean().unwrap_or(0.0);
        let var = lap.mapv(|v| (v - mean).powi(2)).sum() / lap.len() as f64;
        Ok(var)
    }

    fn detect_overexposure(&self, rgb: &Array3<u8>) -> Result<f64> {
        let clipped = rgb.iter().filter(|&&v| v >= 254).count();
        Ok(clipped as f64 / rgb.len() as f64)
    }

    fn detect_color_cast(&self, rgb: &Array3<u8>) -> Result<f64> {
         let r_mean = rgb.slice(ndarray::s![.., .., 0]).mapv(|v| v as f64).mean().unwrap_or(0.0);
         let g_mean = rgb.slice(ndarray::s![.., .., 1]).mapv(|v| v as f64).mean().unwrap_or(0.0);
         let b_mean = rgb.slice(ndarray::s![.., .., 2]).mapv(|v| v as f64).mean().unwrap_or(0.0);
         let dev = ((r_mean - g_mean).powi(2) + (b_mean - g_mean).powi(2)).sqrt();
         Ok(dev)
    }
    
    fn detect_black_dots(&self, rgb: &Array3<u8>, mask: &Array2<u8>) -> Result<f64> {
         let mut black = 0;
         let mut fg = 0;
         for ((y, x), &m) in mask.indexed_iter() {
             if m > 128 {
                 fg += 1;
                 let sum = rgb[[y, x, 0]] as u32 + rgb[[y, x, 1]] as u32 + rgb[[y, x, 2]] as u32;
                 if sum < 30 { black += 1; }
             }
         }
         if fg == 0 { return Ok(0.0); }
         Ok(black as f64 / fg as f64)
    }
}

// Helpers
fn rgb_to_gray(rgb: &Array3<u8>) -> Array2<f64> {
    let (h, w, _) = rgb.dim();
    let mut gray = Array2::zeros((h, w));
    // Convert RGB to grayscale
    for y in 0..h {
        for x in 0..w {
            let r = rgb[[y,x,0]] as f64;
            let g = rgb[[y,x,1]] as f64;
            let b = rgb[[y,x,2]] as f64;
            gray[[y,x]] = 0.299*r + 0.587*g + 0.114*b;
        }
    }
    gray
}

fn sobel_x(img: &Array2<f64>) -> Array2<f64> {
    let (h, w) = img.dim();
    let mut out = Array2::zeros((h, w));
    if h < 3 || w < 3 {
        return out;
    }
    let kernel = [[-1.0, 0.0, 1.0],
                  [-2.0, 0.0, 2.0],
                  [-1.0, 0.0, 1.0]];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let mut sum = 0.0;
            for ky in 0..3 {
                for kx in 0..3 {
                    let yy = y + ky - 1;
                    let xx = x + kx - 1;
                    sum += img[[yy, xx]] * kernel[ky][kx];
                }
            }
            out[[y, x]] = sum;
        }
    }
    out
}
fn sobel_y(img: &Array2<f64>) -> Array2<f64> {
    let (h, w) = img.dim();
    let mut out = Array2::zeros((h, w));
    if h < 3 || w < 3 {
        return out;
    }
    let kernel = [[-1.0, -2.0, -1.0],
                  [0.0, 0.0, 0.0],
                  [1.0, 2.0, 1.0]];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let mut sum = 0.0;
            for ky in 0..3 {
                for kx in 0..3 {
                    let yy = y + ky - 1;
                    let xx = x + kx - 1;
                    sum += img[[yy, xx]] * kernel[ky][kx];
                }
            }
            out[[y, x]] = sum;
        }
    }
    out
}

fn compute_kurtosis(arr: &Array2<f64>) -> f64 {
    let mut count = 0.0;
    let mut mean = 0.0;
    for v in arr.iter() {
        mean += *v;
        count += 1.0;
    }
    if count <= 1.0 {
        return 0.0;
    }
    mean /= count;

    let mut m2 = 0.0;
    let mut m4 = 0.0;
    for v in arr.iter() {
        let d = *v - mean;
        let d2 = d * d;
        m2 += d2;
        m4 += d2 * d2;
    }
    let var = m2 / count;
    if var <= 1e-12 {
        return 0.0;
    }
    (m4 / count) / (var * var)
}

fn compute_laplacian(img: &Array2<f64>) -> Array2<f64> {
    let (h, w) = img.dim();
    let mut out = Array2::zeros((h, w));
    if h < 3 || w < 3 {
        return out;
    }
    let kernel = [[0.0, 1.0, 0.0],
                  [1.0, -4.0, 1.0],
                  [0.0, 1.0, 0.0]];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let mut sum = 0.0;
            for ky in 0..3 {
                for kx in 0..3 {
                    let yy = y + ky - 1;
                    let xx = x + kx - 1;
                    sum += img[[yy, xx]] * kernel[ky][kx];
                }
            }
            out[[y, x]] = sum;
        }
    }
    out
}

fn is_boundary_pixel(mask: &Array2<u8>, x: usize, y: usize) -> bool {
    let (h, w) = mask.dim();
    if mask[[y, x]] <= 128 {
        return false;
    }
    let x0 = x.saturating_sub(1);
    let y0 = y.saturating_sub(1);
    let x1 = (x + 1).min(w - 1);
    let y1 = (y + 1).min(h - 1);
    for yy in y0..=y1 {
        for xx in x0..=x1 {
            if mask[[yy, xx]] <= 128 {
                return true;
            }
        }
    }
    false
}

fn convex_hull(points: &mut Vec<(i32, i32)>) -> Vec<(i32, i32)> {
    points.sort_unstable();
    points.dedup();
    if points.len() < 3 {
        return points.clone();
    }

    let mut lower: Vec<(i32, i32)> = Vec::new();
    for &p in points.iter() {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0 {
            lower.pop();
        }
        lower.push(p);
    }

    let mut upper: Vec<(i32, i32)> = Vec::new();
    for &p in points.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0 {
            upper.pop();
        }
        upper.push(p);
    }

    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn cross(o: (i32, i32), a: (i32, i32), b: (i32, i32)) -> i64 {
    let ax = (a.0 - o.0) as i64;
    let ay = (a.1 - o.1) as i64;
    let bx = (b.0 - o.0) as i64;
    let by = (b.1 - o.1) as i64;
    ax * by - ay * bx
}

fn polygon_area(points: &[(i32, i32)]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut sum = 0i64;
    for i in 0..points.len() {
        let (x1, y1) = points[i];
        let (x2, y2) = points[(i + 1) % points.len()];
        sum += (x1 as i64 * y2 as i64) - (x2 as i64 * y1 as i64);
    }
    (sum as f64).abs() * 0.5
}
