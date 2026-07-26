use crate::scanning::QualityLevel;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ShutterSpeed {
    Auto,
    Bulb,
    Fraction(u32, u32), // numerator, denominator (e.g. 1, 60)
    Seconds(f32),       // e.g. 2.5
    Custom(String),     // Fallback
}

impl std::fmt::Display for ShutterSpeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShutterSpeed::Auto => write!(f, "auto"),
            ShutterSpeed::Bulb => write!(f, "bulb"),
            ShutterSpeed::Fraction(n, d) => write!(f, "{}/{}", n, d),
            ShutterSpeed::Seconds(s) => write!(f, "{}s", s),
            ShutterSpeed::Custom(s) => write!(f, "{}", s),
        }
    }
}

/// A preset configuration for a specific type of object scan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProfile {
    pub name: String,
    pub description: String,
    pub quality: QualityLevel,

    // Camera Settings
    pub iso: Option<u32>,
    pub shutter_speed: Option<ShutterSpeed>, // e.g., "1/60"
    pub aperture: Option<f32>,
    pub white_balance: Option<String>, // "Auto", "Daylight", or kelvin "5500K"

    // Turntable Settings
    pub turntable_speed: u32, // 1-3 typically
    pub acceleration: u32,

    // Processing Settings
    pub hdr_enabled: bool,
    pub focus_stacking_enabled: bool,
    pub use_cross_polarization: bool,
}

impl Default for ScanProfile {
    fn default() -> Self {
        Self {
            name: "Default Object".to_string(),
            description: "Standard scan settings".to_string(),
            quality: QualityLevel::Standard,
            iso: Some(100),
            shutter_speed: Some(ShutterSpeed::Fraction(1, 60)),
            aperture: Some(8.0),
            white_balance: Some("Daylight".to_string()),
            turntable_speed: 1,
            acceleration: 10,
            hdr_enabled: false,
            focus_stacking_enabled: false,
            use_cross_polarization: false,
        }
    }
}

pub struct ProfileManager {
    profiles: HashMap<String, ScanProfile>,
}

impl Default for ProfileManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfileManager {
    pub fn new() -> Self {
        let mut pm = Self {
            profiles: HashMap::new(),
        };
        // Add built-ins
        pm.add_profile(ScanProfile::default());
        pm.add_profile(ScanProfile {
            name: "Shiny Metal".to_string(),
            description: "Scan settings optimized for reflective objects (HDR + Slow)".to_string(),
            quality: QualityLevel::High,
            iso: Some(64),
            shutter_speed: None, // Auto
            aperture: Some(11.0),
            white_balance: None,
            turntable_speed: 1,
            acceleration: 5,
            hdr_enabled: true,
            focus_stacking_enabled: false,
            use_cross_polarization: true,
        });
        pm
    }

    pub fn load_from_dir(&mut self, dir: &Path) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                let content = fs::read_to_string(&path)?;
                let profile: ScanProfile = serde_json::from_str(&content)
                    .with_context(|| format!("Failed to parse profile {:?}", path))?;
                self.profiles.insert(profile.name.clone(), profile);
            }
        }
        Ok(())
    }

    pub fn save_profile(&self, profile: &ScanProfile, dir: &Path) -> Result<()> {
        fs::create_dir_all(dir)?;
        let filename = format!("{}.json", profile.name.replace(" ", "_").to_lowercase());
        let path = dir.join(filename);
        let json = serde_json::to_string_pretty(profile)?;
        fs::write(path, json)?;
        Ok(())
    }

    fn add_profile(&mut self, profile: ScanProfile) {
        self.profiles.insert(profile.name.clone(), profile);
    }

    pub fn get_profile(&self, name: &str) -> Option<&ScanProfile> {
        self.profiles.get(name)
    }

    pub fn list_profiles(&self) -> Vec<&ScanProfile> {
        self.profiles.values().collect()
    }
}
