use anyhow::Result;
#[cfg(feature = "opencv")]
use opencv::{
    calib3d,
    core::{self, Mat, Point2f, Point3f, Size, TermCriteria, Vector},
    imgproc,
    prelude::*,
    types::VectorOfPoint3f,
};
#[cfg(feature = "opencv")]
use std::path::Path;

#[derive(Debug, Clone)]
pub struct CameraIntrinsics {
    pub camera_matrix: Vec<f64>, // 3x3
    pub dist_coeffs: Vec<f64>,   // 5 or 8
    pub rms_error: f64,
    pub width: i32,
    pub height: i32,
}

#[cfg(feature = "opencv")]
pub fn calibrate_checkerboard(
    image_paths: &[std::path::PathBuf],
    rows: i32,
    cols: i32,
    square_size_mm: f32,
) -> Result<CameraIntrinsics> {
    // Check inputs
    if image_paths.is_empty() {
        anyhow::bail!("No images provided for calibration");
    }

    // 1. Prepare object points (3D world coordinates)
    // The checkerboard typically has (rows-1) * (cols-1) internal corners
    let pattern_size = Size::new(cols - 1, rows - 1);
    let mut obj_points_vec = VectorOfPoint3f::new();

    // Create the standard grid of points (Z=0)
    for i in 0..pattern_size.height {
        for j in 0..pattern_size.width {
            obj_points_vec.push(Point3f::new(
                j as f32 * square_size_mm,
                i as f32 * square_size_mm,
                0.0,
            ));
        }
    }

    let mut object_points = Vector::<VectorOfPoint3f>::new();
    let mut image_points = Vector::<Vector<Point2f>>::new();
    let mut image_size = Size::default();

    for path in image_paths {
        let img =
            opencv::imgcodecs::imread(path.to_str().unwrap(), opencv::imgcodecs::IMREAD_GRAYSCALE)?;
        if img.empty() {
            tracing::warn!("Failed to load image: {:?}", path);
            continue;
        }
        image_size = img.size()?;

        let mut corners = Vector::<Point2f>::new();
        // Detect corners
        let found = calib3d::find_chessboard_corners(
            &img,
            pattern_size,
            &mut corners,
            calib3d::CALIB_CB_ADAPTIVE_THRESH + calib3d::CALIB_CB_NORMALIZE_IMAGE,
        )?;

        if found {
            // Refine corners
            imgproc::corner_sub_pix(
                &img,
                &mut corners,
                Size::new(11, 11),
                Size::new(-1, -1),
                TermCriteria::default()?,
            )?;

            image_points.push(corners);
            object_points.push(obj_points_vec.clone());
            tracing::info!("Found corners in {:?}", path);
        } else {
            tracing::warn!("No corners found in {:?}", path);
        }
    }

    if image_points.len() < 5 {
        anyhow::bail!(
            "Not enough valid images for calibration (need 5+, got {})",
            image_points.len()
        );
    }

    // Run calibration
    let mut camera_matrix = Mat::eye(3, 3, core::CV_64F)?.to_mat()?;
    let mut dist_coeffs = Mat::zeros(8, 1, core::CV_64F)?.to_mat()?;
    let mut rvecs = Vector::<Mat>::new();
    let mut tvecs = Vector::<Mat>::new();

    let rms = calib3d::calibrate_camera(
        &object_points,
        &image_points,
        image_size,
        &mut camera_matrix,
        &mut dist_coeffs,
        &mut rvecs,
        &mut tvecs,
        0,
        TermCriteria::default()?,
    )?;

    tracing::info!("Calibration successful! RMS error: {:.4}", rms);

    // Extract data
    let mut cam_data = vec![0.0; 9];
    camera_matrix.copy_to(&mut Mat::from_slice_mut(&mut cam_data)?)?;

    let mut dist_data = vec![0.0; dist_coeffs.rows() as usize * dist_coeffs.cols() as usize];
    dist_coeffs.copy_to(&mut Mat::from_slice_mut(&mut dist_data)?)?;

    Ok(CameraIntrinsics {
        camera_matrix: cam_data,
        dist_coeffs: dist_data,
        rms_error: rms,
        width: image_size.width,
        height: image_size.height,
    })
}

#[cfg(not(feature = "opencv"))]
pub fn calibrate_checkerboard(
    _image_paths: &[std::path::PathBuf],
    _rows: i32,
    _cols: i32,
    _square_size_mm: f32,
) -> Result<CameraIntrinsics> {
    anyhow::bail!("OpenCV feature not enabled")
}
