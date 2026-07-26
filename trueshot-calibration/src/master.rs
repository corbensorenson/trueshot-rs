use anyhow::Result;
use std::path::PathBuf;

use crate::lens::{self, CameraIntrinsics};

/// Master Calibration Wizard
/// Orchestrates Lens -> Extrinsics -> Color steps

#[derive(Debug, Default)]
pub struct MasterCalibration {
    step: u8,
    pub lens_intrinsics: Option<CameraIntrinsics>,
    pub extrinsics: Vec<CameraExtrinsics>,
    pub color_profile: Option<ColorCalibration>,
}

impl MasterCalibration {
    pub fn new() -> Self {
        Self {
            step: 0,
            lens_intrinsics: None,
            extrinsics: Vec::new(),
            color_profile: None,
        }
    }
    
    // Step 1: Lens Distrotion (Checkerboard close up)
    pub fn run_lens_step(
        &mut self,
        image_paths: &[PathBuf],
        rows: i32,
        cols: i32,
        square_size_mm: f32,
    ) -> Result<CameraIntrinsics> {
        let intrinsics = lens::calibrate_checkerboard(image_paths, rows, cols, square_size_mm)?;
        self.lens_intrinsics = Some(intrinsics.clone());
        self.step = self.step.max(1);
        Ok(intrinsics)
    }
    
    // Step 2: Extrinsics (Checkerboard at distance)
    pub fn run_extrinsics_step(
        &mut self,
        image_paths: &[PathBuf],
        rows: i32,
        cols: i32,
        square_size_mm: f32,
    ) -> Result<Vec<CameraExtrinsics>> {
        let intrinsics = match &self.lens_intrinsics {
            Some(intr) => intr.clone(),
            None => lens::calibrate_checkerboard(image_paths, rows, cols, square_size_mm)?,
        };

        let extrinsics = solve_extrinsics(image_paths, &intrinsics, rows, cols, square_size_mm)?;
        self.extrinsics = extrinsics.clone();
        self.step = self.step.max(2);
        Ok(extrinsics)
    }
    
    // Step 3: Color (Color Card)
    pub fn run_color_step(&mut self, image_paths: &[PathBuf]) -> Result<ColorCalibration> {
        if image_paths.is_empty() {
            anyhow::bail!("No images provided for color calibration");
        }
        let mut accum = [0.0f64; 3];
        let mut count = 0u64;
        for path in image_paths {
            let img = image::open(path)?;
            let rgb = img.to_rgb8();
            for pixel in rgb.pixels() {
                accum[0] += pixel[0] as f64;
                accum[1] += pixel[1] as f64;
                accum[2] += pixel[2] as f64;
                count += 1;
            }
        }
        if count == 0 {
            anyhow::bail!("No pixels available for color calibration");
        }
        let mean = [
            (accum[0] / count as f64) as f32,
            (accum[1] / count as f64) as f32,
            (accum[2] / count as f64) as f32,
        ];
        let target = (mean[0] + mean[1] + mean[2]) / 3.0;
        let gains = [
            if mean[0] > 0.0 { target / mean[0] } else { 1.0 },
            if mean[1] > 0.0 { target / mean[1] } else { 1.0 },
            if mean[2] > 0.0 { target / mean[2] } else { 1.0 },
        ];

        let profile = ColorCalibration { gains, mean_rgb: mean };
        self.color_profile = Some(profile.clone());
        self.step = self.step.max(3);
        Ok(profile)
    }
}

#[derive(Debug, Clone)]
pub struct CameraExtrinsics {
    pub rvec: [f64; 3],
    pub tvec: [f64; 3],
    pub reprojection_error: f64,
}

#[derive(Debug, Clone)]
pub struct ColorCalibration {
    pub gains: [f32; 3],
    pub mean_rgb: [f32; 3],
}

#[cfg(feature = "opencv")]
fn solve_extrinsics(
    image_paths: &[PathBuf],
    intrinsics: &CameraIntrinsics,
    rows: i32,
    cols: i32,
    square_size_mm: f32,
) -> Result<Vec<CameraExtrinsics>> {
    use opencv::{
        calib3d,
        core::{self, Mat, Point2f, Point3f, Size, TermCriteria, Vector},
        imgcodecs, imgproc,
        prelude::*,
        types::VectorOfPoint3f,
    };

    if image_paths.is_empty() {
        anyhow::bail!("No images provided for extrinsics");
    }

    let pattern_size = Size::new(cols - 1, rows - 1);
    let mut obj_points_vec = VectorOfPoint3f::new();
    for i in 0..pattern_size.height {
        for j in 0..pattern_size.width {
            obj_points_vec.push(Point3f::new(
                j as f32 * square_size_mm,
                i as f32 * square_size_mm,
                0.0,
            ));
        }
    }

    let mut camera_matrix = Mat::from_slice(&intrinsics.camera_matrix)?;
    camera_matrix = camera_matrix.reshape(1, 3)?;
    let mut dist_coeffs = Mat::from_slice(&intrinsics.dist_coeffs)?;
    dist_coeffs = dist_coeffs.reshape(1, intrinsics.dist_coeffs.len() as i32)?;

    let mut results = Vec::new();

    for path in image_paths {
        let img = imgcodecs::imread(path.to_str().unwrap(), imgcodecs::IMREAD_GRAYSCALE)?;
        if img.empty() {
            continue;
        }

        let mut corners = Vector::<Point2f>::new();
        let found = calib3d::find_chessboard_corners(
            &img,
            pattern_size,
            &mut corners,
            calib3d::CALIB_CB_ADAPTIVE_THRESH + calib3d::CALIB_CB_NORMALIZE_IMAGE,
        )?;

        if !found {
            continue;
        }

        imgproc::corner_sub_pix(
            &img,
            &mut corners,
            Size::new(11, 11),
            Size::new(-1, -1),
            TermCriteria::default()?,
        )?;

        let mut rvec = Mat::default();
        let mut tvec = Mat::default();
        calib3d::solve_pnp(
            &obj_points_vec,
            &corners,
            &camera_matrix,
            &dist_coeffs,
            &mut rvec,
            &mut tvec,
            false,
            calib3d::SOLVEPNP_ITERATIVE,
        )?;

        let mut projected = Vector::<Point2f>::new();
        calib3d::project_points(
            &obj_points_vec,
            &rvec,
            &tvec,
            &camera_matrix,
            &dist_coeffs,
            &mut projected,
        )?;

        let mut err = 0.0f64;
        let n = corners.len().min(projected.len()) as f64;
        for i in 0..corners.len().min(projected.len()) {
            let c = corners.get(i)?;
            let p = projected.get(i)?;
            let dx = (c.x - p.x) as f64;
            let dy = (c.y - p.y) as f64;
            err += (dx * dx + dy * dy).sqrt();
        }
        let reproj = if n > 0.0 { err / n } else { 0.0 };

        let mut rvec_buf = vec![0.0f64; 3];
        let mut tvec_buf = vec![0.0f64; 3];
        rvec.copy_to(&mut Mat::from_slice_mut(&mut rvec_buf)?)?;
        tvec.copy_to(&mut Mat::from_slice_mut(&mut tvec_buf)?)?;

        results.push(CameraExtrinsics {
            rvec: [rvec_buf[0], rvec_buf[1], rvec_buf[2]],
            tvec: [tvec_buf[0], tvec_buf[1], tvec_buf[2]],
            reprojection_error: reproj,
        });
    }

    Ok(results)
}

#[cfg(not(feature = "opencv"))]
fn solve_extrinsics(
    _image_paths: &[PathBuf],
    _intrinsics: &CameraIntrinsics,
    _rows: i32,
    _cols: i32,
    _square_size_mm: f32,
) -> Result<Vec<CameraExtrinsics>> {
    anyhow::bail!("OpenCV feature not enabled for extrinsics calibration")
}
