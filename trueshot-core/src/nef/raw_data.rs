/// Raw data structures for NEF processing
///
/// This module defines the data structures used for RAW image data,
/// regions of interest, and warp transformations.
/// Region of Interest for selective loading
#[derive(Debug, Clone)]
pub struct Roi {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Roi {
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn full_image(width: u32, height: u32) -> Self {
        Self::new(0, 0, width, height)
    }

    pub fn is_valid(&self, image_width: u32, image_height: u32) -> bool {
        self.x + self.width <= image_width && self.y + self.height <= image_height
    }

    pub fn area(&self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

/// Warp transformation matrix for inline alignment
#[derive(Debug, Clone)]
pub struct WarpMatrix {
    pub matrix: [[f32; 3]; 3], // 3x3 homography matrix
}

impl WarpMatrix {
    pub fn identity() -> Self {
        Self {
            matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        }
    }

    pub fn translation(dx: f32, dy: f32) -> Self {
        Self {
            matrix: [[1.0, 0.0, dx], [0.0, 1.0, dy], [0.0, 0.0, 1.0]],
        }
    }

    pub fn transform_point(&self, x: f32, y: f32) -> (f32, f32) {
        let w = self.matrix[2][0] * x + self.matrix[2][1] * y + self.matrix[2][2];
        let new_x = (self.matrix[0][0] * x + self.matrix[0][1] * y + self.matrix[0][2]) / w;
        let new_y = (self.matrix[1][0] * x + self.matrix[1][1] * y + self.matrix[1][2]) / w;
        (new_x, new_y)
    }
}

/// Raw image buffer with CFA data
#[derive(Debug)]
pub struct RawBuffer {
    pub data: Vec<u16>,
    pub width: u32,
    pub height: u32,
    pub cfa_pattern: [u8; 4], // RGGB = [0, 1, 1, 2]
    pub bits_per_sample: u16,
}

impl RawBuffer {
    pub fn new(width: u32, height: u32, cfa_pattern: [u8; 4], bits_per_sample: u16) -> Self {
        let size = (width * height) as usize;
        Self {
            data: vec![0u16; size],
            width,
            height,
            cfa_pattern,
            bits_per_sample,
        }
    }

    pub fn get_pixel(&self, x: u32, y: u32) -> Option<u16> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let index = (y * self.width + x) as usize;
        self.data.get(index).copied()
    }

    pub fn set_pixel(&mut self, x: u32, y: u32, value: u16) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let index = (y * self.width + x) as usize;
        if let Some(pixel) = self.data.get_mut(index) {
            *pixel = value;
            true
        } else {
            false
        }
    }

    pub fn get_cfa_color(&self, x: u32, y: u32) -> u8 {
        // RGGB pattern: [0, 1, 1, 2] = [R, G, G, B]
        let pattern_x = (x % 2) as usize;
        let pattern_y = (y % 2) as usize;
        self.cfa_pattern[pattern_y * 2 + pattern_x]
    }

    pub fn crop(&self, roi: &Roi) -> Option<RawBuffer> {
        if !roi.is_valid(self.width, self.height) {
            return None;
        }

        let mut cropped = RawBuffer::new(
            roi.width,
            roi.height,
            self.cfa_pattern,
            self.bits_per_sample,
        );

        for y in 0..roi.height {
            for x in 0..roi.width {
                if let Some(value) = self.get_pixel(roi.x + x, roi.y + y) {
                    cropped.set_pixel(x, y, value);
                }
            }
        }

        Some(cropped)
    }

    pub fn apply_mask(&mut self, mask: &[u8], mask_width: u32, mask_height: u32) {
        let mask_width = mask_width.min(self.width);
        let mask_height = mask_height.min(self.height);

        for y in 0..mask_height {
            for x in 0..mask_width {
                let mask_idx = (y * mask_width + x) as usize;
                if mask_idx < mask.len() && mask[mask_idx] == 0 {
                    self.set_pixel(x, y, 0);
                }
            }
        }
    }

    pub fn apply_warp(
        &self,
        warp: &WarpMatrix,
        output_width: u32,
        output_height: u32,
    ) -> RawBuffer {
        let mut warped = RawBuffer::new(
            output_width,
            output_height,
            self.cfa_pattern,
            self.bits_per_sample,
        );

        for y in 0..output_height {
            for x in 0..output_width {
                let (src_x, src_y) = warp.transform_point(x as f32, y as f32);

                // Bilinear interpolation
                let x0 = src_x.floor() as u32;
                let y0 = src_y.floor() as u32;
                let x1 = x0 + 1;
                let y1 = y0 + 1;

                if x1 < self.width && y1 < self.height {
                    let dx = src_x - x0 as f32;
                    let dy = src_y - y0 as f32;

                    let p00 = self.get_pixel(x0, y0).unwrap_or(0) as f32;
                    let p01 = self.get_pixel(x0, y1).unwrap_or(0) as f32;
                    let p10 = self.get_pixel(x1, y0).unwrap_or(0) as f32;
                    let p11 = self.get_pixel(x1, y1).unwrap_or(0) as f32;

                    let interpolated = p00 * (1.0 - dx) * (1.0 - dy)
                        + p10 * dx * (1.0 - dy)
                        + p01 * (1.0 - dx) * dy
                        + p11 * dx * dy;

                    warped.set_pixel(x, y, interpolated as u16);
                }
            }
        }

        warped
    }

    pub fn size_bytes(&self) -> usize {
        self.data.len() * std::mem::size_of::<u16>()
    }
}
