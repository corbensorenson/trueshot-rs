// Computer vision module - feature detection, matching, pose estimation
// Uses OpenCV for robust computer vision operations

use anyhow::{Context, Result};
use image::{ImageBuffer, Rgb};
use nalgebra as na;
use opencv::{
    calib3d,
    core::{self, no_array, DMatch, KeyPoint, Mat, Point2f, Vector, NORM_HAMMING},
    features2d::{self, BFMatcher, Feature2DTrait, ORB_ScoreType, ORB},
    imgproc::{self, COLOR_RGB2GRAY},
    prelude::*,
};

/// Camera intrinsic parameters
#[derive(Debug, Clone)]
pub struct CameraIntrinsics {
    pub fx: f64, // Focal length x
    pub fy: f64, // Focal length y
    pub cx: f64, // Principal point x
    pub cy: f64, // Principal point y
    pub width: u32,
    pub height: u32,
    pub distortion: Vec<f64>,
    pub distortion_model: DistortionModel,
    pub rolling_shutter: Option<RollingShutterModel>,
    pub camera_motion: Option<CameraMotion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistortionModel {
    None,
    BrownConrady,
    Fisheye,
}

impl Default for DistortionModel {
    fn default() -> Self {
        DistortionModel::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollingShutterDirection {
    TopToBottom,
    BottomToTop,
    LeftToRight,
    RightToLeft,
}

impl Default for RollingShutterDirection {
    fn default() -> Self {
        RollingShutterDirection::TopToBottom
    }
}

#[derive(Debug, Clone)]
pub struct RollingShutterModel {
    pub readout_time_ms: f64,
    pub direction: RollingShutterDirection,
}

impl RollingShutterModel {
    pub fn time_offset_seconds(&self, x: f64, y: f64, width: u32, height: u32) -> f64 {
        let readout = (self.readout_time_ms / 1000.0).max(0.0);
        if readout <= 0.0 || width == 0 || height == 0 {
            return 0.0;
        }
        let norm = match self.direction {
            RollingShutterDirection::TopToBottom | RollingShutterDirection::BottomToTop => {
                if height > 1 {
                    y / (height as f64 - 1.0)
                } else {
                    0.0
                }
            }
            RollingShutterDirection::LeftToRight | RollingShutterDirection::RightToLeft => {
                if width > 1 {
                    x / (width as f64 - 1.0)
                } else {
                    0.0
                }
            }
        };
        let centered = norm - 0.5;
        let signed = match self.direction {
            RollingShutterDirection::BottomToTop | RollingShutterDirection::RightToLeft => {
                -centered
            }
            _ => centered,
        };
        signed * readout
    }
}

#[derive(Debug, Clone)]
pub struct CameraMotion {
    pub angular_velocity: na::Vector3<f64>,
    pub linear_velocity: na::Vector3<f64>,
}

impl CameraIntrinsics {
    /// Create default intrinsics for a given resolution (rough estimate)
    pub fn default_for_resolution(width: u32, height: u32) -> Self {
        // Assume ~60 degree horizontal FOV for typical webcam
        let fx = width as f64 * 1.2;
        let fy = fx; // Assume square pixels
        let cx = width as f64 / 2.0;
        let cy = height as f64 / 2.0;

        Self {
            fx,
            fy,
            cx,
            cy,
            width,
            height,
            distortion: Vec::new(),
            distortion_model: DistortionModel::None,
            rolling_shutter: None,
            camera_motion: None,
        }
    }

    /// Convert to OpenCV camera matrix
    pub fn to_camera_matrix(&self) -> Result<Mat> {
        let data = [self.fx, 0.0, self.cx, 0.0, self.fy, self.cy, 0.0, 0.0, 1.0];
        Mat::from_slice_2d(&[&data[0..3], &data[3..6], &data[6..9]])
            .context("Failed to create camera matrix")
    }

    /// Convert to nalgebra camera matrix
    pub fn to_nalgebra_matrix(&self) -> na::Matrix3<f64> {
        na::Matrix3::new(self.fx, 0.0, self.cx, 0.0, self.fy, self.cy, 0.0, 0.0, 1.0)
    }

    pub fn to_opencv_dist_coeffs(&self) -> Result<Mat> {
        match self.distortion_model {
            DistortionModel::None => Ok(Mat::default()),
            DistortionModel::BrownConrady => {
                let k1 = self.distortion.get(0).copied().unwrap_or(0.0);
                let k2 = self.distortion.get(1).copied().unwrap_or(0.0);
                let p1 = self.distortion.get(2).copied().unwrap_or(0.0);
                let p2 = self.distortion.get(3).copied().unwrap_or(0.0);
                let k3 = self.distortion.get(4).copied().unwrap_or(0.0);
                let k4 = self.distortion.get(5).copied().unwrap_or(0.0);
                let k5 = self.distortion.get(6).copied().unwrap_or(0.0);
                let k6 = self.distortion.get(7).copied().unwrap_or(0.0);
                Mat::from_slice(&[k1, k2, p1, p2, k3, k4, k5, k6])
                    .context("Failed to create distortion coeffs")
            }
            DistortionModel::Fisheye => Ok(Mat::default()),
        }
    }

    pub fn distort_normalized(&self, x: f64, y: f64) -> (f64, f64) {
        match self.distortion_model {
            DistortionModel::None => (x, y),
            DistortionModel::BrownConrady => distort_brown_conrady(&self.distortion, x, y),
            DistortionModel::Fisheye => distort_fisheye(&self.distortion, x, y),
        }
    }

    pub fn undistort_normalized(&self, x: f64, y: f64) -> (f64, f64) {
        if self.distortion_model == DistortionModel::None || self.distortion.is_empty() {
            return (x, y);
        }
        let mut xu = x;
        let mut yu = y;
        for _ in 0..8 {
            let (xd, yd) = self.distort_normalized(xu, yu);
            xu += x - xd;
            yu += y - yd;
        }
        (xu, yu)
    }
}

fn distort_brown_conrady(coeffs: &[f64], x: f64, y: f64) -> (f64, f64) {
    let k1 = coeffs.get(0).copied().unwrap_or(0.0);
    let k2 = coeffs.get(1).copied().unwrap_or(0.0);
    let p1 = coeffs.get(2).copied().unwrap_or(0.0);
    let p2 = coeffs.get(3).copied().unwrap_or(0.0);
    let k3 = coeffs.get(4).copied().unwrap_or(0.0);
    let k4 = coeffs.get(5).copied().unwrap_or(0.0);
    let k5 = coeffs.get(6).copied().unwrap_or(0.0);
    let k6 = coeffs.get(7).copied().unwrap_or(0.0);

    let r2 = x * x + y * y;
    let r4 = r2 * r2;
    let r6 = r4 * r2;

    let radial_num = 1.0 + k1 * r2 + k2 * r4 + k3 * r6;
    let radial_den = 1.0 + k4 * r2 + k5 * r4 + k6 * r6;
    let radial = if radial_den.abs() > 1e-12 {
        radial_num / radial_den
    } else {
        radial_num
    };

    let x_tan = 2.0 * p1 * x * y + p2 * (r2 + 2.0 * x * x);
    let y_tan = p1 * (r2 + 2.0 * y * y) + 2.0 * p2 * x * y;

    (x * radial + x_tan, y * radial + y_tan)
}

fn distort_fisheye(coeffs: &[f64], x: f64, y: f64) -> (f64, f64) {
    let k1 = coeffs.get(0).copied().unwrap_or(0.0);
    let k2 = coeffs.get(1).copied().unwrap_or(0.0);
    let k3 = coeffs.get(2).copied().unwrap_or(0.0);
    let k4 = coeffs.get(3).copied().unwrap_or(0.0);

    let r = (x * x + y * y).sqrt();
    if r < 1e-12 {
        return (x, y);
    }

    let theta = r.atan();
    let theta2 = theta * theta;
    let theta4 = theta2 * theta2;
    let theta6 = theta4 * theta2;
    let theta8 = theta4 * theta4;
    let theta_d = theta * (1.0 + k1 * theta2 + k2 * theta4 + k3 * theta6 + k4 * theta8);
    let scale = theta_d / r;
    (x * scale, y * scale)
}

/// Camera pose (rotation and translation)
#[derive(Debug, Clone)]
pub struct CameraPose {
    pub rotation: na::Matrix3<f64>,
    pub translation: na::Vector3<f64>,
}

impl CameraPose {
    pub fn identity() -> Self {
        Self {
            rotation: na::Matrix3::identity(),
            translation: na::Vector3::zeros(),
        }
    }
}

/// Detected feature point
#[derive(Debug, Clone)]
pub struct Feature {
    pub point: (f32, f32),
    pub descriptor: Vec<u8>,
}

/// Feature matcher for multi-view reconstruction
pub struct FeatureMatcher {
    detector: core::Ptr<ORB>,
}

impl FeatureMatcher {
    pub fn new() -> Result<Self> {
        // Create ORB detector (patent-free, fast)
        let detector = ORB::create(
            1000, // Max features
            1.2,  // Scale factor
            8,    // Pyramid levels
            31,   // Edge threshold
            0,    // First level
            2,    // WTA_K
            ORB_ScoreType::HARRIS_SCORE,
            31, // Patch size
            20, // Fast threshold
        )?;

        Ok(Self { detector })
    }

    /// Detect features in an image
    /// If roi is provided (x, y, width, height), only detect features in that region
    pub fn detect_features(
        &mut self,
        image: &ImageBuffer<Rgb<u8>, Vec<u8>>,
    ) -> Result<Vec<Feature>> {
        self.detect_features_with_roi(image, None)
    }

    /// Detect features in an image with optional ROI (Region of Interest)
    /// roi: (x, y, width, height) - only detect features in this region for performance
    pub fn detect_features_with_roi(
        &mut self,
        image: &ImageBuffer<Rgb<u8>, Vec<u8>>,
        roi: Option<(i32, i32, i32, i32)>,
    ) -> Result<Vec<Feature>> {
        // Convert image to OpenCV Mat
        let mat = self.image_to_mat(image)?;

        // Convert to grayscale
        let mut gray = Mat::default();
        imgproc::cvt_color(
            &mat,
            &mut gray,
            COLOR_RGB2GRAY,
            0,
            core::AlgorithmHint::ALGO_HINT_DEFAULT,
        )?;

        // If ROI is provided, crop the image to that region
        let (gray_roi, roi_offset) = if let Some((x, y, w, h)) = roi {
            // Clamp ROI to image bounds
            let img_width = image.width() as i32;
            let img_height = image.height() as i32;
            let x = x.max(0).min(img_width - 1);
            let y = y.max(0).min(img_height - 1);
            let w = w.min(img_width - x);
            let h = h.min(img_height - y);

            if w > 0 && h > 0 {
                let rect = opencv::core::Rect::new(x, y, w, h);
                let cropped = Mat::roi(&gray, rect)?.try_clone()?; // Clone to get owned Mat
                log::debug!("🎯 Using ROI for feature detection: ({}, {}, {}, {}) - processing {}% of image",
                    x, y, w, h, (w * h * 100) / (img_width * img_height));
                (cropped, (x, y))
            } else {
                log::warn!("Invalid ROI, using full image");
                (gray, (0, 0))
            }
        } else {
            (gray, (0, 0))
        };

        // Detect keypoints and compute descriptors
        let mut keypoints = Vector::<KeyPoint>::new();
        let mut descriptors = Mat::default();

        self.detector.detect_and_compute(
            &gray_roi,
            &no_array(),
            &mut keypoints,
            &mut descriptors,
            false,
        )?;

        // Convert to our Feature struct
        let mut features = Vec::new();
        for i in 0..keypoints.len() {
            let kp = keypoints.get(i)?;
            let pt = kp.pt();

            // Extract descriptor
            let mut desc = Vec::new();
            if !descriptors.empty() && i < descriptors.rows() as usize {
                for j in 0..descriptors.cols() {
                    desc.push(*descriptors.at_2d::<u8>(i as i32, j)?);
                }
            }

            // Adjust feature coordinates back to full image space if ROI was used
            features.push(Feature {
                point: (pt.x + roi_offset.0 as f32, pt.y + roi_offset.1 as f32),
                descriptor: desc,
            });
        }

        log::debug!(
            "Detected {} features{}",
            features.len(),
            if roi.is_some() { " (in ROI)" } else { "" }
        );
        Ok(features)
    }

    /// Match features between two images
    pub fn match_features(
        &self,
        features1: &[Feature],
        features2: &[Feature],
    ) -> Result<Vec<(usize, usize)>> {
        if features1.is_empty() || features2.is_empty() {
            return Ok(Vec::new());
        }

        // Create descriptor matrices
        let desc1 = self.features_to_mat(features1)?;
        let desc2 = self.features_to_mat(features2)?;

        // Use BFMatcher with Hamming distance (for ORB)
        let matcher = BFMatcher::create(NORM_HAMMING, true)?;

        let mut matches = Vector::<DMatch>::new();
        matcher.train_match(&desc1, &desc2, &mut matches, &no_array())?;

        // Filter matches by distance
        let mut good_matches = Vec::new();
        let mut distances: Vec<f32> = matches.iter().map(|m| m.distance).collect();
        distances.sort_by(|a, b| a.partial_cmp(b).unwrap());

        if distances.is_empty() {
            return Ok(Vec::new());
        }

        // Use median distance for filtering
        let median_dist = distances[distances.len() / 2];
        let threshold = median_dist * 1.5;

        for m in matches.iter() {
            if m.distance < threshold {
                good_matches.push((m.query_idx as usize, m.train_idx as usize));
            }
        }

        log::debug!(
            "Matched {} features (from {} total matches)",
            good_matches.len(),
            matches.len()
        );
        Ok(good_matches)
    }

    /// Convert image to OpenCV Mat
    fn image_to_mat(&self, image: &ImageBuffer<Rgb<u8>, Vec<u8>>) -> Result<Mat> {
        let (width, height) = image.dimensions();
        let data = image.as_raw();

        // Create Mat from raw RGB data (3 channels, 8-bit unsigned)
        let mat = unsafe {
            Mat::new_rows_cols_with_data_unsafe(
                height as i32,
                width as i32,
                opencv::core::CV_8UC3,
                data.as_ptr() as *mut std::ffi::c_void,
                opencv::core::Mat_AUTO_STEP,
            )
            .context("Failed to create Mat from image")?
        };

        // Clone to get an owned Mat
        mat.try_clone().context("Failed to clone Mat")
    }

    /// Convert features to OpenCV Mat for matching
    fn features_to_mat(&self, features: &[Feature]) -> Result<Mat> {
        if features.is_empty() {
            anyhow::bail!("Empty features");
        }

        let desc_len = features[0].descriptor.len();
        let mut data = Vec::with_capacity(features.len() * desc_len);

        for f in features {
            data.extend_from_slice(&f.descriptor);
        }

        let mat = unsafe {
            Mat::new_rows_cols_with_data(features.len() as i32, desc_len as i32, &data[..])
                .context("Failed to create descriptor matrix")?
        };

        mat.try_clone().context("Failed to clone descriptor matrix")
    }
}

/// Estimate relative pose between two cameras using matched features
pub fn estimate_pose(
    features1: &[Feature],
    features2: &[Feature],
    matches: &[(usize, usize)],
    intrinsics: &CameraIntrinsics,
) -> Result<CameraPose> {
    if matches.len() < 8 {
        return Ok(CameraPose::identity());
    }

    // Extract matched points
    let mut points1 = Vector::<Point2f>::new();
    let mut points2 = Vector::<Point2f>::new();

    for &(idx1, idx2) in matches {
        let p1 = features1[idx1].point;
        let p2 = features2[idx2].point;
        points1.push(Point2f::new(p1.0, p1.1));
        points2.push(Point2f::new(p2.0, p2.1));
    }

    // Compute essential matrix
    let camera_matrix = intrinsics.to_camera_matrix()?;
    let mut mask = Mat::default();

    let essential_mat = calib3d::find_essential_mat(
        &points1,
        &points2,
        &camera_matrix,
        calib3d::RANSAC,
        0.999,
        1.0,
        1000, // max iterations
        &mut mask,
    )?;

    // Recover pose from essential matrix
    let mut r = Mat::default();
    let mut t = Mat::default();
    let mut _mask2 = Mat::default();

    calib3d::recover_pose(
        &essential_mat,
        &points1,
        &points2,
        &mut t,
        &mut r,
        intrinsics.fx,
        core::Point2d::new(intrinsics.cx, intrinsics.cy),
        &mut _mask2,
    )?;

    // Convert to nalgebra
    let mut rotation = na::Matrix3::identity();
    let mut translation = na::Vector3::zeros();

    for i in 0..3 {
        for j in 0..3 {
            rotation[(i, j)] = *r.at_2d::<f64>(i as i32, j as i32)?;
        }
        translation[i] = *t.at_2d::<f64>(i as i32, 0)?;
    }

    Ok(CameraPose {
        rotation,
        translation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_camera_intrinsics() {
        let intrinsics = CameraIntrinsics::default_for_resolution(640, 480);
        assert_eq!(intrinsics.width, 640);
        assert_eq!(intrinsics.height, 480);
        assert!(intrinsics.fx > 0.0);
        assert!(intrinsics.fy > 0.0);
    }
}
