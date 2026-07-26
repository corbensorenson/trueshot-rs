///! Object detection for ROI-based selective loading
///!
///! Extracts preview from NEF, applies Otsu thresholding, finds connected components,
///! and calculates bounding box for selective loading.

use anyhow::{Context, Result};
use crate::nef::preview::PreviewExtractor;
use crate::nef::parser::Z9NefParser;
use crate::types::Rect;

/// Bounding box for object detection (used internally by NEF parser)
#[derive(Debug, Clone, Copy)]
pub struct BoundingBox {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl BoundingBox {
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }
    
    /// Calculate the area of this bounding box
    pub fn area(&self) -> u32 {
        self.width * self.height
    }
    
    /// Check if a point is contained within this bounding box
    pub fn contains_point(&self, px: u32, py: u32) -> bool {
        px >= self.x && px < self.x + self.width &&
        py >= self.y && py < self.y + self.height
    }
    
    /// Calculate the intersection of two bounding boxes
    pub fn intersection(&self, other: &BoundingBox) -> Option<BoundingBox> {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x + self.width).min(other.x + other.width);
        let y2 = (self.y + self.height).min(other.y + other.height);
        
        if x2 > x1 && y2 > y1 {
            Some(BoundingBox {
                x: x1,
                y: y1,
                width: x2 - x1,
                height: y2 - y1,
            })
        } else {
            None
        }
    }
    
    /// Scale the bounding box by a factor
    pub fn scale(&self, factor: f32) -> BoundingBox {
        BoundingBox {
            x: (self.x as f32 * factor) as u32,
            y: (self.y as f32 * factor) as u32,
            width: (self.width as f32 * factor) as u32,
            height: (self.height as f32 * factor) as u32,
        }
    }
}

/// Detect object bounding box from NEF preview
///
/// Returns a Rect in full-resolution coordinates for selective loading.
pub fn detect_object_bbox(nef_path: &std::path::Path) -> Result<Rect> {
    tracing::info!("Detecting object bbox from preview: {:?}", nef_path);
    
    // Step 1: Extract preview JPEG
    let preview_start = std::time::Instant::now();
    let mut extractor = PreviewExtractor::new();
    let nef_path_str = nef_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("NEF path is not valid UTF-8"))?;
    let preview_jpeg = extractor.extract_preview_jpeg(nef_path_str)
        .context("Failed to extract preview JPEG")?;
    
    tracing::debug!("Preview extraction: {:.1}ms", preview_start.elapsed().as_secs_f64() * 1000.0);
    
    // Step 2: Decode JPEG to grayscale
    let img = image::load_from_memory(&preview_jpeg)
        .context("Failed to decode preview JPEG")?;
    let gray_img = img.to_luma8();
    let (preview_width, preview_height) = (gray_img.width() as usize, gray_img.height() as usize);
    if preview_width == 0 || preview_height == 0 {
        anyhow::bail!("Preview image has invalid dimensions: {}x{}", preview_width, preview_height);
    }
    let gray_data: Vec<u8> = gray_img.into_raw();

    if gray_data.is_empty() {
        anyhow::bail!("Preview image data is empty");
    }

    tracing::info!("Preview decoded: {}x{} ({} pixels)", preview_width, preview_height, gray_data.len());

    // Step 3: Apply Otsu thresholding (proven to work in original pixelcollapse)
    let threshold = calculate_otsu_threshold(&gray_data);

    let min_gray = gray_data.iter().min().copied().unwrap_or(0);
    let max_gray = gray_data.iter().max().copied().unwrap_or(0);
    tracing::info!("Otsu threshold: {} (min={}, max={})", threshold, min_gray, max_gray);

    // Step 4: Binary thresholding
    let binary: Vec<u8> = gray_data.iter()
        .map(|&pixel| if pixel > threshold { 255 } else { 0 })
        .collect();
    
    // Step 5: Find connected components
    let components = find_connected_components(&binary, preview_width, preview_height);
    tracing::info!("Found {} connected components", components.len());

    // Step 6: Find largest component
    let bbox_preview = find_largest_component_bbox(&components, preview_width, preview_height)
        .unwrap_or((0, 0, preview_width, preview_height));

    tracing::info!("Preview bbox: ({}, {}) {}x{} ({:.1}% of preview)",
                   bbox_preview.0, bbox_preview.1,
                   bbox_preview.2, bbox_preview.3,
                   (bbox_preview.2 * bbox_preview.3) as f64 / (preview_width * preview_height) as f64 * 100.0);

    // DEBUG: Save binary threshold image and bbox overlay
    let binary: Vec<u8> = gray_data.iter()
        .map(|&pixel| if pixel > threshold { 255 } else { 0 })
        .collect();

    if let Some(mut binary_img) = image::GrayImage::from_raw(preview_width as u32, preview_height as u32, binary.clone()) {
        // Draw bbox on binary image
        let (bx, by, bw, bh) = bbox_preview;
        for x in bx..(bx + bw).min(preview_width) {
            if by < preview_height {
                binary_img.put_pixel(x as u32, by as u32, image::Luma([255u8]));
            }
            if by + bh - 1 < preview_height {
                binary_img.put_pixel(x as u32, (by + bh - 1) as u32, image::Luma([255u8]));
            }
        }
        for y in by..(by + bh).min(preview_height) {
            if bx < preview_width {
                binary_img.put_pixel(bx as u32, y as u32, image::Luma([255u8]));
            }
            if bx + bw - 1 < preview_width {
                binary_img.put_pixel((bx + bw - 1) as u32, y as u32, image::Luma([255u8]));
            }
        }
    }

    // Step 7: Scale to full resolution using actual sensor metadata
    let (full_width, full_height) = get_full_resolution(nef_path)
        .unwrap_or_else(|| {
            tracing::warn!("Falling back to preview dimensions for scaling");
            (preview_width, preview_height)
        });
    
    let scale_x = full_width as f64 / preview_width as f64;
    let scale_y = full_height as f64 / preview_height as f64;
    
    let x = bbox_preview.0 as f64 * scale_x;
    let y = bbox_preview.1 as f64 * scale_y;
    let width = bbox_preview.2 as f64 * scale_x;
    let height = bbox_preview.3 as f64 * scale_y;

    tracing::info!("Scaled bbox (before padding): {}x{} at ({}, {})", width, height, x, y);

    // ASYMMETRIC padding: minimal on top/left/right, moderate on bottom
    // Bones are centered on black background, main issue is cutting off bottom
    let padding_top = 40.0;      // Fixed 40px top (user feedback: less top)
    let padding_left = (width * 0.05).max(50.0);     // 5% left, min 50px
    let padding_right = (width * 0.05).max(50.0);    // 5% right, min 50px
    let padding_bottom = 250.0;  // Fixed 250px bottom (user feedback: more bottom)

    // Add extra bottom padding if object is near bottom edge
    let edge_threshold = 0.15; // 15% from edge
    let near_bottom = ((y + height) / full_height as f64) > (1.0 - edge_threshold);

    let final_padding_top = padding_top;
    let final_padding_left = padding_left;
    let final_padding_right = padding_right;
    let final_padding_bottom = if near_bottom {
        padding_bottom * 1.5  // 50% extra if near bottom
    } else {
        padding_bottom
    };

    tracing::info!("ASYMMETRIC Padding: top={:.0}, left={:.0}, right={:.0}, bottom={:.0}",
                  final_padding_top, final_padding_left, final_padding_right, final_padding_bottom);

    let x_with_padding = (x - final_padding_left).max(0.0);
    let y_with_padding = (y - final_padding_top).max(0.0);
    let width_with_padding = (width + final_padding_left + final_padding_right).min(full_width as f64 - x_with_padding);
    let height_with_padding = (height + final_padding_top + final_padding_bottom).min(full_height as f64 - y_with_padding);

    // CRITICAL: Align ROI to Bayer pattern (2x2 grid)
    // X and Y must be EVEN to preserve RGGB pattern alignment
    // Width and height should also be even for clean demosaic
    let aligned_x = (x_with_padding as u32 / 2 * 2) as f64;  // Round down to even
    let aligned_y = (y_with_padding as u32 / 2 * 2) as f64;  // Round down to even

    // Adjust width/height to compensate for alignment shift and ensure even dimensions
    let width_adjustment = x_with_padding - aligned_x;
    let height_adjustment = y_with_padding - aligned_y;
    let aligned_width = ((width_with_padding + width_adjustment) as u32 / 2 * 2 + 2) as f64;  // Round up to even
    let aligned_height = ((height_with_padding + height_adjustment) as u32 / 2 * 2 + 2) as f64;  // Round up to even

    let final_rect = Rect {
        x: aligned_x,
        y: aligned_y,
        width: aligned_width.min(full_width as f64 - aligned_x),
        height: aligned_height.min(full_height as f64 - aligned_y),
    };

    tracing::info!("Bayer-aligned ROI: {}x{} at ({}, {}) - {:.1}% of full image",
                  final_rect.width, final_rect.height, final_rect.x, final_rect.y,
                  (final_rect.width * final_rect.height) as f64 / (full_width * full_height) as f64 * 100.0);
    
    Ok(final_rect)
}

fn get_full_resolution(nef_path: &std::path::Path) -> Option<(usize, usize)> {
    let mut parser = Z9NefParser::new(nef_path);
    if parser.parse().is_err() {
        return None;
    }
    let metadata = parser.get_metadata().ok()?;
    Some((metadata.width as usize, metadata.height as usize))
}

/// Calculate Otsu's threshold for automatic binary thresholding
///
/// Finds the threshold that maximizes between-class variance.
fn calculate_otsu_threshold(gray: &[u8]) -> u8 {
    // Build histogram
    let mut histogram = [0u32; 256];
    for &pixel in gray {
        histogram[pixel as usize] += 1;
    }
    
    let total_pixels = gray.len() as f32;
    let mut sum = 0.0f32;
    for (i, &count) in histogram.iter().enumerate() {
        sum += i as f32 * count as f32;
    }
    
    let mut sum_background = 0.0f32;
    let mut weight_background = 0.0f32;
    let mut max_variance = 0.0f32;
    let mut threshold = 0u8;
    
    for (t, &count) in histogram.iter().enumerate() {
        weight_background += count as f32;
        if weight_background == 0.0 {
            continue;
        }
        
        let weight_foreground = total_pixels - weight_background;
        if weight_foreground == 0.0 {
            break;
        }
        
        sum_background += t as f32 * count as f32;
        let mean_background = sum_background / weight_background;
        let mean_foreground = (sum - sum_background) / weight_foreground;
        
        // Calculate between-class variance
        let variance = weight_background * weight_foreground * 
                      (mean_background - mean_foreground).powi(2);
        
        if variance > max_variance {
            max_variance = variance;
            threshold = t as u8;
        }
    }
    
    threshold
}

/// Component information for connected component analysis
#[derive(Debug, Clone)]
struct Component {
    min_x: usize,
    max_x: usize,
    min_y: usize,
    max_y: usize,
    area: usize,
}

/// Find connected components using flood fill
fn find_connected_components(binary: &[u8], width: usize, height: usize) -> Vec<Component> {
    let mut visited = vec![false; width * height];
    let mut components = Vec::new();
    
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if !visited[idx] && binary[idx] == 255 {
                let component = flood_fill(binary, &mut visited, x, y, width, height);
                if component.area > 0 {  // Keep all components, filter in scoring
                    components.push(component);
                }
            }
        }
    }
    
    components
}

/// Flood fill to find a single connected component
fn flood_fill(binary: &[u8], visited: &mut [bool], start_x: usize, start_y: usize, 
              width: usize, height: usize) -> Component {
    let mut stack = vec![(start_x, start_y)];
    let mut min_x = start_x;
    let mut max_x = start_x;
    let mut min_y = start_y;
    let mut max_y = start_y;
    let mut area = 0;
    
    while let Some((x, y)) = stack.pop() {
        let idx = y * width + x;
        
        if visited[idx] || binary[idx] != 255 {
            continue;
        }
        
        visited[idx] = true;
        area += 1;
        
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
        
        // Add neighbors
        if x > 0 { stack.push((x - 1, y)); }
        if x < width - 1 { stack.push((x + 1, y)); }
        if y > 0 { stack.push((x, y - 1)); }
        if y < height - 1 { stack.push((x, y + 1)); }
    }
    
    Component { min_x, max_x, min_y, max_y, area }
}

/// Find the best component using scoring (from original pixelcollapse)
fn find_largest_component_bbox(components: &[Component], width: usize, height: usize)
    -> Option<(usize, usize, usize, usize)> {

    if components.is_empty() {
        return None;
    }

    let total_pixels = (width * height) as f32;
    let mut best_component = None;
    let mut best_score = 0.0f32;

    tracing::info!("Evaluating {} components for bone detection", components.len());

    for (i, component) in components.iter().enumerate() {
        let bbox_width = component.max_x - component.min_x + 1;
        let bbox_height = component.max_y - component.min_y + 1;
        let area_ratio = component.area as f32 / total_pixels;

        tracing::debug!("Component {}: area={} pixels ({:.1}%), bbox=({},{}) {}x{}",
                       i, component.area, area_ratio * 100.0,
                       component.min_x, component.min_y, bbox_width, bbox_height);

        // Very permissive filter for bone detection (0.1% to 60%)
        // Bones are centered on black background, so we can be very permissive
        if !(0.001..=0.6).contains(&area_ratio) {
            tracing::debug!("Component {} rejected: area ratio {:.1}% outside range [0.1%, 60%]",
                           i, area_ratio * 100.0);
            continue;
        }

        // NO minimum pixel size - some bones are small!

        // Calculate center position (prefer center objects)
        let center_x = (component.min_x + component.max_x) as f32 / 2.0;
        let center_y = (component.min_y + component.max_y) as f32 / 2.0;
        let image_center_x = width as f32 / 2.0;
        let image_center_y = height as f32 / 2.0;

        let center_distance = ((center_x - image_center_x).powi(2) +
                              (center_y - image_center_y).powi(2)).sqrt();
        let max_distance = (width as f32 + height as f32) / 4.0;
        let center_score = 1.0 - (center_distance / max_distance).min(1.0);

        // Calculate shape score (more permissive aspect ratios for bones: 0.2 to 5.0)
        let aspect_ratio = bbox_width as f32 / bbox_height as f32;
        let shape_score = if aspect_ratio > 0.2 && aspect_ratio < 5.0 { 1.0 } else { 0.7 };

        // Size score (prefer medium-sized objects, but be more permissive)
        let size_score = if area_ratio > 0.01 && area_ratio < 0.3 { 1.0 } else { 0.8 };

        // Combined score
        let total_score = size_score * 0.4 + center_score * 0.4 + shape_score * 0.2;

        tracing::debug!("Component {} score: {:.3} (size={:.2}, center={:.2}, shape={:.2})",
                       i, total_score, size_score, center_score, shape_score);

        if total_score > best_score {
            best_score = total_score;
            best_component = Some(component);
        }
    }

    if let Some(component) = best_component {
        tracing::info!("Selected component: area={}, coverage={:.3}, score={:.3}",
                      component.area, component.area as f32 / total_pixels, best_score);

        Some((component.min_x, component.min_y,
              component.max_x - component.min_x + 1,
              component.max_y - component.min_y + 1))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_otsu_threshold() {
        // Bimodal distribution: 100 pixels at 50, 100 pixels at 200
        let mut gray = vec![50u8; 100];
        gray.extend(vec![200u8; 100]);
        
        let threshold = calculate_otsu_threshold(&gray);
        
        // Threshold should be between 50 and 200
        assert!(threshold > 50 && threshold < 200);
    }
    
    #[test]
    fn test_connected_components() {
        // 10x10 image with a 5x5 square in the center
        let mut binary = vec![0u8; 100];
        for y in 2..7 {
            for x in 2..7 {
                binary[y * 10 + x] = 255;
            }
        }
        
        let components = find_connected_components(&binary, 10, 10);
        
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].area, 25);
        assert_eq!(components[0].min_x, 2);
        assert_eq!(components[0].max_x, 6);
    }
}
