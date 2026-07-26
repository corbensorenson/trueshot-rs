use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LensInfo {
    pub model: String,
    pub focal_length_mm: f32,
    pub distortion_k1: f32,
    pub distortion_k2: f32,
}

pub fn get_known_lenses() -> HashMap<String, LensInfo> {
    let mut db = HashMap::new();
    
    db.insert("iphone_13_wide".to_string(), LensInfo {
        model: "iPhone 13 Wide".to_string(),
        focal_length_mm: 26.0,
        distortion_k1: -0.05,
        distortion_k2: 0.0,
    });
    
    db.insert("nikon_z_24_70_24mm".to_string(), LensInfo {
        model: "Nikon Z 24-70mm @ 24mm".to_string(),
        focal_length_mm: 24.0,
        distortion_k1: -0.02,
        distortion_k2: 0.001,
    });
    
    // Add more...
    db
}
