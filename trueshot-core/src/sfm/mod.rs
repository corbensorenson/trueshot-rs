//! Native Structure from Motion Pipeline
//!
//! TrueShot's own SfM implementation for commercial use.

use anyhow::Result;
use nalgebra as na;
use std::path::PathBuf;

/// Native SfM pipeline - no COLMAP required
pub struct NativeSfmPipeline {
    images: Vec<PathBuf>,
    intrinsics: CameraIntrinsics,
    poses: Vec<CameraPose>,
    points_3d: Vec<Point3D>,
}

/// Camera intrinsic parameters
#[derive(Debug, Clone)]
pub struct CameraIntrinsics {
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub width: u32,
    pub height: u32,
}

impl CameraIntrinsics {
    pub fn from_resolution(width: u32, height: u32) -> Self {
        // Assume ~50mm equivalent focal length
        let fx = width as f64 * 1.2;
        let fy = fx;
        let cx = width as f64 / 2.0;
        let cy = height as f64 / 2.0;

        Self {
            fx,
            fy,
            cx,
            cy,
            width,
            height,
        }
    }

    pub fn to_matrix(&self) -> na::Matrix3<f64> {
        na::Matrix3::new(self.fx, 0.0, self.cx, 0.0, self.fy, self.cy, 0.0, 0.0, 1.0)
    }
}

/// Estimated camera pose
#[derive(Debug, Clone)]
pub struct CameraPose {
    pub rotation: na::Matrix3<f64>,
    pub translation: na::Vector3<f64>,
    pub image_path: PathBuf,
}

impl CameraPose {
    pub fn identity(image_path: PathBuf) -> Self {
        Self {
            rotation: na::Matrix3::identity(),
            translation: na::Vector3::zeros(),
            image_path,
        }
    }

    pub fn to_matrix4(&self) -> na::Matrix4<f64> {
        let mut m = na::Matrix4::identity();
        m.fixed_view_mut::<3, 3>(0, 0).copy_from(&self.rotation);
        m.fixed_view_mut::<3, 1>(0, 3).copy_from(&self.translation);
        m
    }
}

/// Reconstructed 3D point
#[derive(Debug, Clone)]
pub struct Point3D {
    pub position: na::Point3<f64>,
    pub color: [u8; 3],
    pub error: f64,
}

impl NativeSfmPipeline {
    pub fn new(images: Vec<PathBuf>, intrinsics: CameraIntrinsics) -> Self {
        Self {
            images,
            intrinsics,
            poses: Vec::new(),
            points_3d: Vec::new(),
        }
    }

    /// Run incremental SfM pipeline
    pub fn run(&mut self) -> Result<()> {
        if self.images.len() < 2 {
            anyhow::bail!("Need at least 2 images for SfM");
        }

        tracing::info!("🔍 Starting native SfM with {} images", self.images.len());

        // Initialize with first image
        self.poses
            .push(CameraPose::identity(self.images[0].clone()));

        // Process remaining images incrementally
        for i in 1..self.images.len() {
            tracing::info!("Processing image {}/{}", i + 1, self.images.len());

            // Load images
            let img_prev = image::open(&self.images[i - 1])?;
            let img_curr = image::open(&self.images[i])?;

            // Convert to grayscale
            let gray_prev = img_prev.to_luma8();
            let gray_curr = img_curr.to_luma8();

            // Detect features (using native implementation)
            let extractor = trueshot_vision::features::NativeFeatureExtractor::new(2000);
            let features_prev = extractor.detect(&gray_prev);
            let features_curr = extractor.detect(&gray_curr);

            tracing::debug!(
                "Found {} and {} features",
                features_prev.len(),
                features_curr.len()
            );

            // Match features
            let matcher = trueshot_vision::matching::NativeMatcher::default();
            let matches = matcher.match_features(&features_prev, &features_curr);

            tracing::debug!("Found {} matches", matches.len());

            if matches.len() < 8 {
                tracing::warn!("Too few matches, skipping image {}", i);
                continue;
            }

            // Convert matches to point pairs
            let pts1: Vec<(f64, f64)> = matches
                .iter()
                .map(|m| {
                    let kp = &features_prev[m.idx1].keypoint;
                    (kp.x as f64, kp.y as f64)
                })
                .collect();

            let pts2: Vec<(f64, f64)> = matches
                .iter()
                .map(|m| {
                    let kp = &features_curr[m.idx2].keypoint;
                    (kp.x as f64, kp.y as f64)
                })
                .collect();

            // Estimate pose using native geometry
            let k = self.intrinsics.to_matrix();

            if let Some(f) = trueshot_vision::geometry::estimate_fundamental_8point(&pts1, &pts2) {
                let e = trueshot_vision::geometry::fundamental_to_essential(&f, &k);
                let solutions = trueshot_vision::geometry::decompose_essential(&e);

                if let Some((r, t)) =
                    trueshot_vision::geometry::select_correct_pose(&solutions, &pts1, &pts2, &k)
                {
                    self.poses.push(CameraPose {
                        rotation: r,
                        translation: t,
                        image_path: self.images[i].clone(),
                    });

                    // Triangulate points
                    self.triangulate_points(&pts1, &pts2, i - 1, i, &img_prev.to_rgb8());
                }
            }
        }

        tracing::info!(
            "✅ Native SfM complete: {} cameras, {} points",
            self.poses.len(),
            self.points_3d.len()
        );

        Ok(())
    }

    /// Triangulate 3D points from matched image points
    fn triangulate_points(
        &mut self,
        pts1: &[(f64, f64)],
        pts2: &[(f64, f64)],
        idx1: usize,
        idx2: usize,
        image: &image::RgbImage,
    ) {
        let k = self.intrinsics.to_matrix();
        let pose1 = &self.poses[idx1];
        let pose2 = &self.poses[idx2];

        let p1 = trueshot_vision::geometry::essential::projection_matrix(
            &k,
            &pose1.rotation,
            &pose1.translation,
        );
        let p2 = trueshot_vision::geometry::essential::projection_matrix(
            &k,
            &pose2.rotation,
            &pose2.translation,
        );

        for (pt1, pt2) in pts1.iter().zip(pts2.iter()) {
            if let Some(point_3d) =
                trueshot_vision::geometry::essential::triangulate_point(*pt1, *pt2, &p1, &p2)
            {
                // Check depth is positive
                if point_3d.z > 0.0 && point_3d.z < 100.0 {
                    let error = reprojection_error(&point_3d, &p1, *pt1, &p2, *pt2);
                    if error > MAX_REPROJ_ERROR_PX {
                        continue;
                    }
                    // Get color from image
                    let px = (pt1.0 as u32).min(image.width() - 1);
                    let py = (pt1.1 as u32).min(image.height() - 1);
                    let color = image.get_pixel(px, py);

                    self.points_3d.push(Point3D {
                        position: point_3d,
                        color: [color[0], color[1], color[2]],
                        error,
                    });
                }
            }
        }
    }

    /// Get reconstructed camera poses
    pub fn poses(&self) -> &[CameraPose] {
        &self.poses
    }

    /// Get reconstructed 3D points
    pub fn points(&self) -> &[Point3D] {
        &self.points_3d
    }

    /// Export point cloud to PLY
    pub fn export_ply(&self, path: &PathBuf) -> Result<()> {
        use std::io::Write;

        let mut file = std::fs::File::create(path)?;

        writeln!(file, "ply")?;
        writeln!(file, "format ascii 1.0")?;
        writeln!(file, "element vertex {}", self.points_3d.len())?;
        writeln!(file, "property float x")?;
        writeln!(file, "property float y")?;
        writeln!(file, "property float z")?;
        writeln!(file, "property uchar red")?;
        writeln!(file, "property uchar green")?;
        writeln!(file, "property uchar blue")?;
        writeln!(file, "end_header")?;

        for pt in &self.points_3d {
            writeln!(
                file,
                "{} {} {} {} {} {}",
                pt.position.x, pt.position.y, pt.position.z, pt.color[0], pt.color[1], pt.color[2]
            )?;
        }

        Ok(())
    }
}

const MAX_REPROJ_ERROR_PX: f64 = 4.0;

fn reprojection_error(
    point: &na::Point3<f64>,
    p1: &na::Matrix3x4<f64>,
    obs1: (f64, f64),
    p2: &na::Matrix3x4<f64>,
    obs2: (f64, f64),
) -> f64 {
    let err1 = reprojection_error_single(point, p1, obs1);
    let err2 = reprojection_error_single(point, p2, obs2);
    (err1 + err2) * 0.5
}

fn reprojection_error_single(
    point: &na::Point3<f64>,
    p: &na::Matrix3x4<f64>,
    obs: (f64, f64),
) -> f64 {
    let hom = p * point.to_homogeneous();
    if hom[2].abs() < f64::EPSILON {
        return f64::MAX;
    }
    let x = hom[0] / hom[2];
    let y = hom[1] / hom[2];
    let dx = x - obs.0;
    let dy = y - obs.1;
    (dx * dx + dy * dy).sqrt()
}
