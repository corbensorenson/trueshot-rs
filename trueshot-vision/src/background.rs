use image::{ImageBuffer, Rgb, Rgba};
use nalgebra as na;
#[cfg(feature = "opencv")]
use opencv::{
    core::{self, Mat, Scalar, Size, Vector, BORDER_DEFAULT},
    imgproc,
    prelude::*,
};
use anyhow::{Result, Context};

/// Background subtraction methods
pub enum BackgroundMethod {
    /// Simple color difference with a clean plate
    DifferenceKeying {
        threshold: u8,
        blur_radius: i32,
    },
    /// Chakra Keying (Green/Blue screen)
    ChromaKey {
        key_color: [u8; 3],
        tolerance: f32,
    },
    /// MOG2 (Gaussian Mixture-based Background/Foreground Segmentation)
    MOG2 {
        history: i32,
        var_threshold: f64,
        detect_shadows: bool,
    },
}

pub struct BackgroundRemover {
    #[cfg(feature = "opencv")]
    bg_subtractor: Option<core::Ptr<opencv::video::BackgroundSubtractorMOG2>>,
    clean_plate: Option<ImageBuffer<Rgb<u8>, Vec<u8>>>,
}

impl BackgroundRemover {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "opencv")]
            bg_subtractor: None,
            clean_plate: None,
        }
    }

    /// Set a clean plate (empty background image) for difference keying
    pub fn set_clean_plate(&mut self, image: ImageBuffer<Rgb<u8>, Vec<u8>>) {
        self.clean_plate = Some(image);
    }

    /// Generate a mask for the given image
    pub fn generate_mask(
        &mut self,
        image: &ImageBuffer<Rgb<u8>, Vec<u8>>,
        method: BackgroundMethod,
    ) -> Result<ImageBuffer<image::Luma<u8>, Vec<u8>>> {
        match method {
            BackgroundMethod::DifferenceKeying { threshold, blur_radius } => {
                self.difference_keying(image, threshold, blur_radius)
            }
            BackgroundMethod::ChromaKey { key_color, tolerance } => {
                self.chroma_key(image, key_color, tolerance)
            }
            BackgroundMethod::MOG2 { history, var_threshold, detect_shadows } => {
                self.mog2_subtraction(image, history, var_threshold, detect_shadows)
            }
        }
    }

    /// Difference Keying: |Current - CleanPlate| > Threshold
    fn difference_keying(
        &self,
        image: &ImageBuffer<Rgb<u8>, Vec<u8>>,
        threshold: u8,
        blur_radius: i32,
    ) -> Result<ImageBuffer<image::Luma<u8>, Vec<u8>>> {
        let clean = self.clean_plate.as_ref().context("Clean plate not set")?;
        
        if image.dimensions() != clean.dimensions() {
            anyhow::bail!("Image dimensions mismatch with clean plate");
        }

        let (image_blurred, clean_blurred);
        let (image_ref, clean_ref) = if blur_radius > 0 {
            image_blurred = gaussian_blur_rgb(image, blur_radius);
            clean_blurred = gaussian_blur_rgb(clean, blur_radius);
            (&image_blurred, &clean_blurred)
        } else {
            (image, clean)
        };

        let (width, height) = image.dimensions();
        let mut mask = ImageBuffer::new(width, height);
        
        for (x, y, pixel) in image_ref.enumerate_pixels() {
            let clean_pixel = clean_ref.get_pixel(x, y);
            
            let diff = (pixel[0] as i16 - clean_pixel[0] as i16).abs() +
                       (pixel[1] as i16 - clean_pixel[1] as i16).abs() +
                       (pixel[2] as i16 - clean_pixel[2] as i16).abs();
            
            if diff > threshold as i16 {
                mask.put_pixel(x, y, image::Luma([255])); // Foreground
            } else {
                mask.put_pixel(x, y, image::Luma([0])); // Background
            }
        }
        
        Ok(mask)
    }

    /// Simple Chroma Keying
    fn chroma_key(
        &self,
        image: &ImageBuffer<Rgb<u8>, Vec<u8>>,
        key_color: [u8; 3],
        tolerance: f32,
    ) -> Result<ImageBuffer<image::Luma<u8>, Vec<u8>>> {
        let (width, height) = image.dimensions();
        let mut mask = ImageBuffer::new(width, height);
        
        let target = na::Vector3::new(key_color[0] as f32, key_color[1] as f32, key_color[2] as f32);
        let dist_threshold = tolerance * 441.6; // Max distance in RGB
        
        for (x, y, pixel) in image.enumerate_pixels() {
            let color = na::Vector3::new(pixel[0] as f32, pixel[1] as f32, pixel[2] as f32);
            let dist = (color - target).norm();
            
            if dist > dist_threshold {
                mask.put_pixel(x, y, image::Luma([255]));
            } else {
                mask.put_pixel(x, y, image::Luma([0]));
            }
        }
        
        Ok(mask)
    }

    #[cfg(feature = "opencv")]
    fn mog2_subtraction(
        &mut self,
        image: &ImageBuffer<Rgb<u8>, Vec<u8>>,
        history: i32,
        var_threshold: f64,
        detect_shadows: bool,
    ) -> Result<ImageBuffer<image::Luma<u8>, Vec<u8>>> {
        if self.bg_subtractor.is_none() {
             self.bg_subtractor = Some(
                opencv::video::create_background_subtractor_mog2(history, var_threshold, detect_shadows)?
            );
        }
        
        let (width, height) = image.dimensions();
        let mat = Mat::from_slice(image.as_raw())?;
        let mat = mat.reshape(3, height as i32)?; // 3 channels
        
        let mut fg_mask = Mat::default();
        if let Some(ref mut bg) = self.bg_subtractor {
            bg.apply(&mat, &mut fg_mask, -1.0)?;
        }
        
        // Convert back to ImageBuffer
        let size = fg_mask.size()?;
        let mut buffer = vec![0u8; (size.width * size.height) as usize];
        fg_mask.copy_to(&mut Mat::from_slice_mut(&mut buffer)?)?;
        
        Ok(ImageBuffer::from_raw(size.width as u32, size.height as u32, buffer)
           .context("Failed to create mask buffer")?)
    }

    #[cfg(not(feature = "opencv"))]
    fn mog2_subtraction(
        &mut self,
        _image: &ImageBuffer<Rgb<u8>, Vec<u8>>,
        _history: i32,
        _var_threshold: f64,
        _detect_shadows: bool,
    ) -> Result<ImageBuffer<image::Luma<u8>, Vec<u8>>> {
        anyhow::bail!("OpenCV feature not enabled")
    }
}

fn gaussian_blur_rgb(
    image: &ImageBuffer<Rgb<u8>, Vec<u8>>,
    radius: i32,
) -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    if radius <= 0 {
        return image.clone();
    }

    let kernel = gaussian_kernel(radius);
    let (width, height) = image.dimensions();
    let w = width as usize;
    let h = height as usize;
    let mut tmp = vec![0f32; w * h * 3];

    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 3];
            for (k, weight) in kernel.iter().enumerate() {
                let offset = k as i32 - radius;
                let xi = (x as i32 + offset).clamp(0, (w - 1) as i32) as usize;
                let pixel = image.get_pixel(xi as u32, y as u32);
                acc[0] += pixel[0] as f32 * weight;
                acc[1] += pixel[1] as f32 * weight;
                acc[2] += pixel[2] as f32 * weight;
            }
            let base = (y * w + x) * 3;
            tmp[base] = acc[0];
            tmp[base + 1] = acc[1];
            tmp[base + 2] = acc[2];
        }
    }

    let mut out = ImageBuffer::new(width, height);
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 3];
            for (k, weight) in kernel.iter().enumerate() {
                let offset = k as i32 - radius;
                let yi = (y as i32 + offset).clamp(0, (h - 1) as i32) as usize;
                let base = (yi * w + x) * 3;
                acc[0] += tmp[base] * weight;
                acc[1] += tmp[base + 1] * weight;
                acc[2] += tmp[base + 2] * weight;
            }
            out.put_pixel(
                x as u32,
                y as u32,
                Rgb([acc[0].round().clamp(0.0, 255.0) as u8,
                     acc[1].round().clamp(0.0, 255.0) as u8,
                     acc[2].round().clamp(0.0, 255.0) as u8]),
            );
        }
    }

    out
}

fn gaussian_kernel(radius: i32) -> Vec<f32> {
    let radius = radius.max(1);
    let sigma = (radius as f32) / 2.0;
    let denom = 2.0 * sigma * sigma;
    let mut kernel = Vec::with_capacity((radius * 2 + 1) as usize);
    let mut sum = 0.0f32;
    for i in -radius..=radius {
        let v = (-((i * i) as f32) / denom).exp();
        kernel.push(v);
        sum += v;
    }
    if sum > 0.0 {
        for v in &mut kernel {
            *v /= sum;
        }
    }
    kernel
}
