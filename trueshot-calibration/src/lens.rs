use anyhow::Result;
#[cfg(feature = "opencv")]
use opencv::{
    calib3d,
    core::{self, Mat, Point2f, Point3f, Size, TermCriteria, Vector},
    imgcodecs, imgproc,
    prelude::*,
    types::VectorOfPoint3f,
};

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
    if image_paths.is_empty() {
        anyhow::bail!("No images provided for calibration");
    }

    let mut images = Vec::with_capacity(image_paths.len());
    for path in image_paths {
        images.push((
            path.display().to_string(),
            imgcodecs::imread(
                path.to_str()
                    .ok_or_else(|| anyhow::anyhow!("Image path is not valid UTF-8"))?,
                imgcodecs::IMREAD_GRAYSCALE,
            )?,
        ));
    }
    calibrate_checkerboard_mats(images, rows, cols, square_size_mm)
}

#[cfg(feature = "opencv")]
pub fn calibrate_checkerboard_encoded(
    encoded_images: &[Vec<u8>],
    rows: i32,
    cols: i32,
    square_size_mm: f32,
) -> Result<CameraIntrinsics> {
    if encoded_images.is_empty() {
        anyhow::bail!("No images provided for calibration");
    }

    let mut images = Vec::with_capacity(encoded_images.len());
    for (index, encoded) in encoded_images.iter().enumerate() {
        if encoded.is_empty() {
            anyhow::bail!("Calibration image {index} is empty");
        }
        let bytes = Vector::<u8>::from_slice(encoded);
        let image = imgcodecs::imdecode(&bytes, imgcodecs::IMREAD_GRAYSCALE)?;
        images.push((format!("descriptor frame {index}"), image));
    }
    calibrate_checkerboard_mats(images, rows, cols, square_size_mm)
}

#[cfg(feature = "opencv")]
fn calibrate_checkerboard_mats(
    images: Vec<(String, Mat)>,
    rows: i32,
    cols: i32,
    square_size_mm: f32,
) -> Result<CameraIntrinsics> {
    if rows < 3 || cols < 3 {
        anyhow::bail!("Checkerboard rows and columns must both be at least 3");
    }
    if !square_size_mm.is_finite() || square_size_mm <= 0.0 {
        anyhow::bail!("Checkerboard square size must be finite and positive");
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

    let mut object_points = Vector::<VectorOfPoint3f>::new();
    let mut image_points = Vector::<Vector<Point2f>>::new();
    let mut image_size = Size::default();

    for (label, img) in images {
        if img.empty() {
            tracing::warn!("Failed to decode calibration image: {}", label);
            continue;
        }
        let current_size = img.size()?;
        if image_size.width == 0 && image_size.height == 0 {
            image_size = current_size;
        } else if image_size != current_size {
            anyhow::bail!(
                "Calibration images have inconsistent dimensions: expected {}x{}, got {}x{} for {}",
                image_size.width,
                image_size.height,
                current_size.width,
                current_size.height,
                label
            );
        }

        let mut corners = Vector::<Point2f>::new();
        let found = calib3d::find_chessboard_corners(
            &img,
            pattern_size,
            &mut corners,
            calib3d::CALIB_CB_ADAPTIVE_THRESH + calib3d::CALIB_CB_NORMALIZE_IMAGE,
        )?;

        if found {
            imgproc::corner_sub_pix(
                &img,
                &mut corners,
                Size::new(11, 11),
                Size::new(-1, -1),
                TermCriteria::default()?,
            )?;

            image_points.push(corners);
            object_points.push(obj_points_vec.clone());
            tracing::info!("Found corners in {}", label);
        } else {
            tracing::warn!("No corners found in {}", label);
        }
    }

    if image_points.len() < 5 {
        anyhow::bail!(
            "Not enough valid images for calibration (need 5+, got {})",
            image_points.len()
        );
    }

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

    let mut cam_data = Vec::with_capacity(9);
    for row in 0..3 {
        for column in 0..3 {
            cam_data.push(*camera_matrix.at_2d::<f64>(row, column)?);
        }
    }

    let mut dist_data =
        Vec::with_capacity(dist_coeffs.rows() as usize * dist_coeffs.cols() as usize);
    for row in 0..dist_coeffs.rows() {
        for column in 0..dist_coeffs.cols() {
            dist_data.push(*dist_coeffs.at_2d::<f64>(row, column)?);
        }
    }

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

#[cfg(not(feature = "opencv"))]
pub fn calibrate_checkerboard_encoded(
    _encoded_images: &[Vec<u8>],
    _rows: i32,
    _cols: i32,
    _square_size_mm: f32,
) -> Result<CameraIntrinsics> {
    anyhow::bail!("OpenCV feature not enabled")
}
