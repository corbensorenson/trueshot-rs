/// Multi-Spectral Image Container (16-bit)
pub struct MultiSpectralImage {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub data: Vec<u16>,
}

impl MultiSpectralImage {
    pub fn new(width: u32, height: u32, channels: u8) -> Self {
        Self {
            width,
            height,
            channels,
            data: vec![0; (width * height * channels as u32) as usize],
        }
    }

    pub fn get_pixel(&self, x: u32, y: u32) -> &[u16] {
        let start = ((y * self.width + x) * self.channels as u32) as usize;
        &self.data[start..start + self.channels as usize]
    }
    
    pub fn set_pixel(&mut self, x: u32, y: u32, pixel: &[u16]) {
        let start = ((y * self.width + x) * self.channels as u32) as usize;
        let end = start + self.channels as usize;
        self.data[start..end].copy_from_slice(pixel);
    }
}
