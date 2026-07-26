use crate::reconstruction::multicam_sfm::CameraIntrinsics;
use anyhow::{Context, Result};
use exif::{Reader, Tag, Value};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const FULL_FRAME_DIAGONAL_MM: f64 = 43.266615305567875; // sqrt(36^2 + 24^2)

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum IntrinsicsSource {
    FocalPlane,
    FocalLength35mm,
    Heuristic,
    Calibration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntrinsicsReport {
    pub source: IntrinsicsSource,
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub width: u32,
    pub height: u32,
    pub focal_length_mm: Option<f64>,
    pub focal_length_35mm: Option<f64>,
    #[serde(default)]
    pub rms_error: Option<f64>,
}

pub fn estimate_intrinsics(path: &Path) -> Result<CameraIntrinsics> {
    let (intrinsics, _report) = estimate_intrinsics_with_report(path)?;
    Ok(intrinsics)
}

pub fn estimate_intrinsics_with_report(
    path: &Path,
) -> Result<(CameraIntrinsics, IntrinsicsReport)> {
    if let Some((intrinsics, report)) = load_calibration_override(path)? {
        return Ok((intrinsics, report));
    }
    let exif = read_exif(path).ok();

    let (width, height) = match image::image_dimensions(path) {
        Ok(dim) => dim,
        Err(_) => {
            if let Some(exif) = &exif {
                if let Some((w, h)) = exif_pixel_dimensions(exif) {
                    (w, h)
                } else {
                    anyhow::bail!("Failed to read image dimensions for {}", path.display());
                }
            } else {
                anyhow::bail!("Failed to read image dimensions for {}", path.display());
            }
        }
    };

    let mut fx: Option<f64> = None;
    let mut fy: Option<f64> = None;
    let mut source = IntrinsicsSource::Heuristic;

    let focal_length_mm = exif.as_ref().and_then(focal_length_mm);
    let focal_length_35mm = exif.as_ref().and_then(focal_length_35mm);

    if let Some(exif) = &exif {
        if let Some((fx_px, fy_px)) = focal_from_focal_plane(exif) {
            fx = Some(fx_px);
            fy = Some(fy_px);
            source = IntrinsicsSource::FocalPlane;
        } else if let Some(f_px) = focal_from_35mm_equiv(exif, width, height) {
            fx = Some(f_px);
            fy = Some(f_px);
            source = IntrinsicsSource::FocalLength35mm;
        }
    }

    if fx.is_none() || fy.is_none() {
        let focal = (width.max(height) as f64) * 1.2; // Fallback heuristic
        fx = Some(focal);
        fy = Some(focal);
        source = IntrinsicsSource::Heuristic;
    }

    let intrinsics = CameraIntrinsics {
        fx: fx.unwrap(),
        fy: fy.unwrap(),
        cx: width as f64 / 2.0,
        cy: height as f64 / 2.0,
        width,
        height,
        distortion: Vec::new(),
        distortion_model: trueshot_sfm::DistortionModel::None,
    };

    let report = IntrinsicsReport {
        source,
        fx: intrinsics.fx,
        fy: intrinsics.fy,
        cx: intrinsics.cx,
        cy: intrinsics.cy,
        width,
        height,
        focal_length_mm,
        focal_length_35mm,
        rms_error: None,
    };

    Ok((intrinsics, report))
}

#[derive(serde::Deserialize)]
struct CalibrationFile {
    camera_matrix: Vec<f64>,
    dist_coeffs: Vec<f64>,
    rms_error: Option<f64>,
    width: Option<i32>,
    height: Option<i32>,
}

fn load_calibration_override(path: &Path) -> Result<Option<(CameraIntrinsics, IntrinsicsReport)>> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let camera_specific = camera_specific_calibration(path, dir);
    let candidates = [
        camera_specific,
        dir.join("calibration.json"),
        dir.join(".trueshot").join("intrinsics.json"),
        dir.join(".trueshot").join("calibration.json"),
    ];

    for candidate in candidates.iter() {
        if candidate.as_os_str().is_empty() {
            continue;
        }
        if !candidate.exists() {
            continue;
        }
        let raw = std::fs::read_to_string(candidate)?;
        let parsed: CalibrationFile = serde_json::from_str(&raw)?;
        if parsed.camera_matrix.len() < 9 {
            continue;
        }
        let fx = parsed.camera_matrix[0];
        let fy = parsed.camera_matrix[4];
        let cx = parsed.camera_matrix[2];
        let cy = parsed.camera_matrix[5];
        let (width, height) = match (parsed.width, parsed.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (w as u32, h as u32),
            _ => image::image_dimensions(path).unwrap_or((0, 0)),
        };
        if width == 0 || height == 0 {
            anyhow::bail!("Invalid calibration dimensions for {}", candidate.display());
        }
        let intrinsics = CameraIntrinsics {
            fx,
            fy,
            cx,
            cy,
            width,
            height,
            distortion: parsed.dist_coeffs.clone(),
            distortion_model: if parsed.dist_coeffs.is_empty() {
                trueshot_sfm::DistortionModel::None
            } else {
                trueshot_sfm::DistortionModel::BrownConrady
            },
        };
        let report = IntrinsicsReport {
            source: IntrinsicsSource::Calibration,
            fx,
            fy,
            cx,
            cy,
            width,
            height,
            focal_length_mm: None,
            focal_length_35mm: None,
            rms_error: parsed.rms_error,
        };
        return Ok(Some((intrinsics, report)));
    }

    Ok(None)
}

fn camera_specific_calibration(path: &Path, dir: &Path) -> PathBuf {
    let file_name = match path.file_name().and_then(|f| f.to_str()) {
        Some(name) => name,
        None => return PathBuf::new(),
    };
    let camera_id = if let Some((id, _)) = file_name.split_once("__") {
        if id.is_empty() {
            return PathBuf::new();
        }
        id
    } else if let Some((id, _)) = file_name.split_once('_') {
        if id.is_empty() {
            return PathBuf::new();
        }
        id
    } else {
        return PathBuf::new();
    };
    dir.join(format!("calibration_{}.json", camera_id))
}

fn read_exif(path: &Path) -> Result<exif::Exif> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open image {}", path.display()))?;
    let mut bufreader = std::io::BufReader::new(file);
    Reader::new()
        .read_from_container(&mut bufreader)
        .with_context(|| format!("Failed to parse EXIF for {}", path.display()))
}

fn exif_pixel_dimensions(exif: &exif::Exif) -> Option<(u32, u32)> {
    let width = exif
        .get_field(Tag::PixelXDimension, exif::In::PRIMARY)
        .and_then(|f| exif_first_u32(&f.value));
    let height = exif
        .get_field(Tag::PixelYDimension, exif::In::PRIMARY)
        .and_then(|f| exif_first_u32(&f.value));
    match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
        _ => None,
    }
}

fn focal_from_focal_plane(exif: &exif::Exif) -> Option<(f64, f64)> {
    let focal_mm = exif
        .get_field(Tag::FocalLength, exif::In::PRIMARY)
        .and_then(|f| exif_first_f64(&f.value))?;

    let x_res = exif
        .get_field(Tag::FocalPlaneXResolution, exif::In::PRIMARY)
        .and_then(|f| exif_first_f64(&f.value))?;
    let y_res = exif
        .get_field(Tag::FocalPlaneYResolution, exif::In::PRIMARY)
        .and_then(|f| exif_first_f64(&f.value))?;

    let unit = exif
        .get_field(Tag::FocalPlaneResolutionUnit, exif::In::PRIMARY)
        .and_then(|f| exif_first_u32(&f.value))
        .unwrap_or(2); // Default to inches

    let mm_per_unit = match unit {
        2 => 25.4, // inches
        3 => 10.0, // cm
        _ => 25.4,
    };

    let fx = focal_mm * x_res / mm_per_unit;
    let fy = focal_mm * y_res / mm_per_unit;

    if fx.is_finite() && fy.is_finite() && fx > 0.0 && fy > 0.0 {
        Some((fx, fy))
    } else {
        None
    }
}

fn focal_from_35mm_equiv(exif: &exif::Exif, width: u32, height: u32) -> Option<f64> {
    let f_equiv = exif
        .get_field(Tag::FocalLengthIn35mmFilm, exif::In::PRIMARY)
        .and_then(|f| exif_first_f64(&f.value))?;

    if f_equiv <= 0.0 {
        return None;
    }

    let diag_px = ((width as f64).powi(2) + (height as f64).powi(2)).sqrt();
    let diag_fov = 2.0 * (FULL_FRAME_DIAGONAL_MM / (2.0 * f_equiv)).atan();
    let f_px = (diag_px / 2.0) / (diag_fov / 2.0).tan();
    if f_px.is_finite() && f_px > 0.0 {
        Some(f_px)
    } else {
        None
    }
}

fn focal_length_mm(exif: &exif::Exif) -> Option<f64> {
    exif.get_field(Tag::FocalLength, exif::In::PRIMARY)
        .and_then(|f| exif_first_f64(&f.value))
}

fn focal_length_35mm(exif: &exif::Exif) -> Option<f64> {
    exif.get_field(Tag::FocalLengthIn35mmFilm, exif::In::PRIMARY)
        .and_then(|f| exif_first_f64(&f.value))
}

fn exif_first_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Rational(ref v) if !v.is_empty() => {
            let r = v[0];
            if r.denom != 0 {
                Some(r.num as f64 / r.denom as f64)
            } else {
                None
            }
        }
        Value::SRational(ref v) if !v.is_empty() => {
            let r = v[0];
            if r.denom != 0 {
                Some(r.num as f64 / r.denom as f64)
            } else {
                None
            }
        }
        Value::Short(ref v) if !v.is_empty() => Some(v[0] as f64),
        Value::Long(ref v) if !v.is_empty() => Some(v[0] as f64),
        _ => None,
    }
}

fn exif_first_u32(value: &Value) -> Option<u32> {
    match value {
        Value::Short(ref v) if !v.is_empty() => Some(v[0] as u32),
        Value::Long(ref v) if !v.is_empty() => Some(v[0] as u32),
        Value::Rational(ref v) if !v.is_empty() => {
            let r = v[0];
            if r.denom != 0 {
                Some((r.num as f64 / r.denom as f64).round() as u32)
            } else {
                None
            }
        }
        Value::SRational(ref v) if !v.is_empty() => {
            let r = v[0];
            if r.denom != 0 {
                Some((r.num as f64 / r.denom as f64).round() as u32)
            } else {
                None
            }
        }
        _ => None,
    }
}
