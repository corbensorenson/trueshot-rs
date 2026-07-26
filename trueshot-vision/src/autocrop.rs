
/// Bounding Box Calculation from Alpha Mask
pub fn calculate_bounds_from_mask(mask: &[u8], width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0;
    let mut max_y = 0;
    
    let mut found = false;

    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) as usize;
            if mask[idx] > 128 { // Threshold
                if x < min_x { min_x = x; }
                if x > max_x { max_x = x; }
                if y < min_y { min_y = y; }
                if y > max_y { max_y = y; }
                found = true;
            }
        }
    }

    if !found { return None; }
    
    // Add padding
    let pad = 10;
    min_x = min_x.saturating_sub(pad);
    min_y = min_y.saturating_sub(pad);
    max_x = (max_x + pad).min(width - 1);
    max_y = (max_y + pad).min(height - 1);
    
    Some((min_x, min_y, max_x - min_x, max_y - min_y))
}
