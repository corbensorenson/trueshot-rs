use image::{ImageBuffer, Rgb};
use std::collections::VecDeque;

/// Simple diffusion-based inpainting for filling holes (black pixels)
pub fn inpaint_simple(image: &mut ImageBuffer<Rgb<u8>, Vec<u8>>) {
    let (width, height) = image.dimensions();
    let mut queue = VecDeque::new();
    let mut visited = vec![false; (width * height) as usize];

    // Find hole pixels (assuming exact black [0,0,0] is hole)
    // and identify boundary pixels
    for y in 0..height {
        for x in 0..width {
            let pixel = image.get_pixel(x, y);
            if pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0 {
                // It's a hole
            } else {
                // It's valid, add to queue as seed
                queue.push_back((x, y));
                visited[(y * width + x) as usize] = true;
            }
        }
    }

    // BFS flood fill
    while let Some((x, y)) = queue.pop_front() {
        let p = *image.get_pixel(x, y);
        
        let neighbors = [
            (x.wrapping_sub(1), y), (x + 1, y),
            (x, y.wrapping_sub(1)), (x, y + 1)
        ];

        for &(nx, ny) in &neighbors {
            if nx < width && ny < height {
                let idx = (ny * width + nx) as usize;
                if !visited[idx] {
                    // Propagate color
                    image.put_pixel(nx, ny, p);
                    visited[idx] = true;
                    queue.push_back((nx, ny));
                }
            }
        }
    }
}
