//! RAW Processing Engine
//!
//! Non-destructive RAW image processing with professional adjustments.
//! Integrates with Photo Editor frontend for Lightroom-style workflow.
//!
//! Features:
//! - Full adjustment stack (exposure, WB, HSL, curves, etc.)
//! - XMP sidecar compatibility
//! - GPU-accelerated preview (via WGPU compute)
//! - Preset system

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Complete set of non-destructive image adjustments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAdjustments {
    // Basic adjustments
    #[serde(default)]
    pub exposure: f32, // -5.0 to +5.0 EV
    #[serde(default)]
    pub contrast: i32, // -100 to +100
    #[serde(default)]
    pub highlights: i32, // -100 to +100
    #[serde(default)]
    pub shadows: i32, // -100 to +100
    #[serde(default)]
    pub whites: i32, // -100 to +100
    #[serde(default)]
    pub blacks: i32, // -100 to +100

    // White balance
    #[serde(default = "default_temperature")]
    pub temperature: u32, // 2000 to 50000 Kelvin
    #[serde(default)]
    pub tint: i32, // -150 to +150 (Green-Magenta)

    // Presence
    #[serde(default)]
    pub clarity: i32, // -100 to +100
    #[serde(default)]
    pub dehaze: i32, // -100 to +100
    #[serde(default)]
    pub vibrance: i32, // -100 to +100
    #[serde(default)]
    pub saturation: i32, // -100 to +100

    // Tone curve
    #[serde(default)]
    pub tone_curve: ToneCurve,

    // HSL adjustments
    #[serde(default)]
    pub hsl: HslAdjustments,

    // Detail
    #[serde(default = "default_sharpen")]
    pub sharpen_amount: u32, // 0 to 150
    #[serde(default = "default_sharpen_radius")]
    pub sharpen_radius: f32, // 0.5 to 3.0
    #[serde(default = "default_sharpen_detail")]
    pub sharpen_detail: u32, // 0 to 100
    #[serde(default)]
    pub sharpen_masking: u32, // 0 to 100

    // Noise reduction
    #[serde(default)]
    pub nr_luminance: u32, // 0 to 100
    #[serde(default = "default_nr_color")]
    pub nr_color: u32, // 0 to 100

    // Lens corrections
    #[serde(default = "default_true")]
    pub enable_profile: bool,
    #[serde(default)]
    pub distortion: i32, // -100 to +100
    #[serde(default)]
    pub vignette: i32, // -100 to +100
    #[serde(default)]
    pub chromatic_aberration: u32, // 0 to 100

    // Split toning
    #[serde(default)]
    pub split_toning: SplitToning,

    // Effects
    #[serde(default)]
    pub post_vignette_amount: i32, // -100 to +100
    #[serde(default = "default_grain")]
    pub grain_amount: u32, // 0 to 100
    #[serde(default = "default_grain_size")]
    pub grain_size: u32, // 1 to 100
}

fn default_temperature() -> u32 {
    5500
}
fn default_sharpen() -> u32 {
    40
}
fn default_sharpen_radius() -> f32 {
    1.0
}
fn default_sharpen_detail() -> u32 {
    25
}
fn default_nr_color() -> u32 {
    25
}
fn default_true() -> bool {
    true
}
fn default_grain() -> u32 {
    0
}
fn default_grain_size() -> u32 {
    25
}

impl Default for ImageAdjustments {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 0,
            highlights: 0,
            shadows: 0,
            whites: 0,
            blacks: 0,
            temperature: 5500,
            tint: 0,
            clarity: 0,
            dehaze: 0,
            vibrance: 0,
            saturation: 0,
            tone_curve: ToneCurve::default(),
            hsl: HslAdjustments::default(),
            sharpen_amount: 40,
            sharpen_radius: 1.0,
            sharpen_detail: 25,
            sharpen_masking: 0,
            nr_luminance: 0,
            nr_color: 25,
            enable_profile: true,
            distortion: 0,
            vignette: 0,
            chromatic_aberration: 0,
            split_toning: SplitToning::default(),
            post_vignette_amount: 0,
            grain_amount: 0,
            grain_size: 25,
        }
    }
}

/// Tone curve with RGB and per-channel control
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToneCurve {
    /// RGB combined curve points [[x, y], ...]
    pub rgb: Vec<[u8; 2]>,
    /// Red channel curve points
    pub red: Vec<[u8; 2]>,
    /// Green channel curve points
    pub green: Vec<[u8; 2]>,
    /// Blue channel curve points
    pub blue: Vec<[u8; 2]>,
}

impl ToneCurve {
    pub fn linear() -> Self {
        Self {
            rgb: vec![[0, 0], [255, 255]],
            red: vec![[0, 0], [255, 255]],
            green: vec![[0, 0], [255, 255]],
            blue: vec![[0, 0], [255, 255]],
        }
    }

    /// Evaluate curve at input value
    pub fn evaluate(&self, channel: &[[u8; 2]], input: u8) -> u8 {
        if channel.is_empty() {
            return input;
        }
        if channel.len() == 1 {
            return channel[0][1];
        }

        // Find surrounding points
        let mut left_idx = 0;
        for (i, point) in channel.iter().enumerate() {
            if point[0] <= input {
                left_idx = i;
            } else {
                break;
            }
        }

        let right_idx = (left_idx + 1).min(channel.len() - 1);

        if left_idx == right_idx {
            return channel[left_idx][1];
        }

        // Linear interpolation
        let left = channel[left_idx];
        let right = channel[right_idx];

        if right[0] == left[0] {
            return left[1];
        }

        let t = (input - left[0]) as f32 / (right[0] - left[0]) as f32;
        let result = left[1] as f32 + t * (right[1] as f32 - left[1] as f32);
        result.clamp(0.0, 255.0) as u8
    }
}

/// HSL adjustments for 8 color ranges
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HslAdjustments {
    /// Hue shift for: Red, Orange, Yellow, Green, Aqua, Blue, Purple, Magenta
    pub hue: [i32; 8],
    /// Saturation adjustment per color (-100 to +100)
    pub saturation: [i32; 8],
    /// Luminance adjustment per color (-100 to +100)
    pub luminance: [i32; 8],
}

/// Split toning for highlights and shadows
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SplitToning {
    pub highlight_hue: u16,       // 0 to 360
    pub highlight_saturation: u8, // 0 to 100
    pub shadow_hue: u16,          // 0 to 360
    pub shadow_saturation: u8,    // 0 to 100
    pub balance: i8,              // -100 to +100
}

/// Editing preset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub category: String,
    pub adjustments: ImageAdjustments,
    pub created_at: String,
    pub modified_at: String,
}

/// XMP sidecar file support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XmpSidecar {
    pub rating: Option<u8>,
    pub label: Option<String>,
    pub adjustments: ImageAdjustments,
}

impl XmpSidecar {
    /// Load from XMP file
    pub fn load(path: impl AsRef<Path>) -> crate::Result<Self> {
        let content =
            fs::read_to_string(path.as_ref()).map_err(|e| crate::Error::Io(e.to_string()))?;

        // Parse XMP (simplified - in production, use proper XMP parser)
        // For now, try JSON sidecar format
        if content.trim().starts_with('{') {
            serde_json::from_str(&content).map_err(|e| crate::Error::Processing(e.to_string()))
        } else {
            // Return default if not parseable
            Ok(Self {
                rating: None,
                label: None,
                adjustments: ImageAdjustments::default(),
            })
        }
    }

    /// Save to XMP file
    pub fn save(&self, path: impl AsRef<Path>) -> crate::Result<()> {
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| crate::Error::Processing(e.to_string()))?;
        fs::write(path.as_ref(), content).map_err(|e| crate::Error::Io(e.to_string()))?;
        Ok(())
    }

    /// Get XMP sidecar path for an image
    pub fn sidecar_path(image_path: impl AsRef<Path>) -> std::path::PathBuf {
        let path = image_path.as_ref();
        path.with_extension("xmp")
    }
}

/// RAW image processor
pub struct RawProcessor {
    /// Current adjustments
    adjustments: ImageAdjustments,
    /// Cached lookup tables
    lut_cache: Option<LutCache>,
}

struct LutCache {
    exposure_lut: [u8; 256],
    contrast_lut: [u8; 256],
    curve_lut: [[u8; 256]; 3], // RGB
}

impl RawProcessor {
    pub fn new() -> Self {
        Self {
            adjustments: ImageAdjustments::default(),
            lut_cache: None,
        }
    }

    pub fn with_adjustments(adjustments: ImageAdjustments) -> Self {
        Self {
            adjustments,
            lut_cache: None,
        }
    }

    /// Update adjustments
    pub fn set_adjustments(&mut self, adjustments: ImageAdjustments) {
        self.adjustments = adjustments;
        self.lut_cache = None; // Invalidate cache
    }

    /// Get current adjustments
    pub fn adjustments(&self) -> &ImageAdjustments {
        &self.adjustments
    }

    /// Build lookup tables for fast processing
    fn build_luts(&mut self) {
        let adj = &self.adjustments;

        // Exposure LUT
        let exposure_mult = 2.0_f32.powf(adj.exposure);
        let mut exposure_lut = [0u8; 256];
        for i in 0..256 {
            let v = (i as f32 * exposure_mult).clamp(0.0, 255.0);
            exposure_lut[i] = v as u8;
        }

        // Contrast LUT (S-curve)
        let contrast = adj.contrast as f32 / 100.0;
        let mut contrast_lut = [0u8; 256];
        for i in 0..256 {
            let v = i as f32 / 255.0;
            let v = (v - 0.5) * (1.0 + contrast) + 0.5;
            contrast_lut[i] = (v.clamp(0.0, 1.0) * 255.0) as u8;
        }

        // Curve LUTs
        let mut curve_lut = [[0u8; 256]; 3];
        for i in 0..256 {
            let input = i as u8;
            let rgb_out = adj.tone_curve.evaluate(&adj.tone_curve.rgb, input);
            curve_lut[0][i] = adj.tone_curve.evaluate(&adj.tone_curve.red, rgb_out);
            curve_lut[1][i] = adj.tone_curve.evaluate(&adj.tone_curve.green, rgb_out);
            curve_lut[2][i] = adj.tone_curve.evaluate(&adj.tone_curve.blue, rgb_out);
        }

        self.lut_cache = Some(LutCache {
            exposure_lut,
            contrast_lut,
            curve_lut,
        });
    }

    /// Process a single pixel through the adjustment pipeline
    pub fn process_pixel(&mut self, r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        if self.lut_cache.is_none() {
            self.build_luts();
        }

        let cache = self.lut_cache.as_ref().unwrap();
        let adj = &self.adjustments;

        // Apply exposure
        let mut r = cache.exposure_lut[r as usize];
        let mut g = cache.exposure_lut[g as usize];
        let mut b = cache.exposure_lut[b as usize];

        // Apply contrast
        r = cache.contrast_lut[r as usize];
        g = cache.contrast_lut[g as usize];
        b = cache.contrast_lut[b as usize];

        // Apply curves
        r = cache.curve_lut[0][r as usize];
        g = cache.curve_lut[1][g as usize];
        b = cache.curve_lut[2][b as usize];

        // Apply vibrance/saturation
        if adj.vibrance != 0 || adj.saturation != 0 {
            let (h, s, l) = rgb_to_hsl(r, g, b);

            // Vibrance affects less saturated colors more
            let vibrance_factor = 1.0 + (adj.vibrance as f32 / 100.0) * (1.0 - s);
            let saturation_factor = 1.0 + (adj.saturation as f32 / 100.0);

            let new_s = (s * vibrance_factor * saturation_factor).clamp(0.0, 1.0);
            let (nr, ng, nb) = hsl_to_rgb(h, new_s, l);
            r = nr;
            g = ng;
            b = nb;
        }

        (r, g, b)
    }

    /// Process an entire image buffer
    pub fn process_image(&mut self, pixels: &mut [[u8; 3]]) {
        for pixel in pixels.iter_mut() {
            let (r, g, b) = self.process_pixel(pixel[0], pixel[1], pixel[2]);
            pixel[0] = r;
            pixel[1] = g;
            pixel[2] = b;
        }
    }
}

impl Default for RawProcessor {
    fn default() -> Self {
        Self::new()
    }
}

// Color space conversion helpers

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = r as f32 / 255.0;
    let g = g as f32 / 255.0;
    let b = b as f32 / 255.0;

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;

    if (max - min).abs() < 0.0001 {
        return (0.0, 0.0, l);
    }

    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };

    let h = if (max - r).abs() < 0.0001 {
        let mut h = (g - b) / d;
        if g < b {
            h += 6.0;
        }
        h
    } else if (max - g).abs() < 0.0001 {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };

    (h / 6.0, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    if s.abs() < 0.0001 {
        let v = (l * 255.0) as u8;
        return (v, v, v);
    }

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;

    let r = hue_to_rgb(p, q, h + 1.0 / 3.0);
    let g = hue_to_rgb(p, q, h);
    let b = hue_to_rgb(p, q, h - 1.0 / 3.0);

    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }

    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_adjustments() {
        let adj = ImageAdjustments::default();
        assert_eq!(adj.exposure, 0.0);
        assert_eq!(adj.temperature, 5500);
    }

    #[test]
    fn test_tone_curve_linear() {
        let curve = ToneCurve::linear();
        assert_eq!(curve.evaluate(&curve.rgb, 0), 0);
        assert_eq!(curve.evaluate(&curve.rgb, 128), 128);
        assert_eq!(curve.evaluate(&curve.rgb, 255), 255);
    }

    #[test]
    fn test_hsl_roundtrip() {
        let (r, g, b) = (180, 100, 50);
        let (h, s, l) = rgb_to_hsl(r, g, b);
        let (r2, g2, b2) = hsl_to_rgb(h, s, l);
        assert!((r as i32 - r2 as i32).abs() <= 1);
        assert!((g as i32 - g2 as i32).abs() <= 1);
        assert!((b as i32 - b2 as i32).abs() <= 1);
    }

    #[test]
    fn test_processor() {
        let mut proc = RawProcessor::new();

        // Default adjustments should be nearly identity
        let (r, g, b) = proc.process_pixel(128, 128, 128);
        assert!((r as i32 - 128).abs() <= 5);
        assert!((g as i32 - 128).abs() <= 5);
        assert!((b as i32 - 128).abs() <= 5);
    }
}
