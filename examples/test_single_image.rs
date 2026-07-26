//! Test loading a single NEF image to diagnose color issues
//! 
//! This bypasses the collapse pipeline to isolate where the blue/purple hue comes from.

use anyhow::Result;
use ndarray::Array3;
use pixelcollapse2::raw_io::load_bayer_frame;
use std::path::Path;

fn apply_white_balance(bayer: &Array3<f64>, cam_mul: &[f32; 4]) -> Array3<f64> {
    let (height, width, _) = bayer.dim();
    
    // Normalize by green channel
    let green_mul = cam_mul[1].max(cam_mul[3]);
    let wb_r = cam_mul[0] / green_mul;
    let wb_g1 = cam_mul[1] / green_mul;
    let wb_b = cam_mul[2] / green_mul;
    let wb_g2 = cam_mul[3] / green_mul;
    
    println!("Applying WB: R={:.3}, G1={:.3}, B={:.3}, G2={:.3}", wb_r, wb_g1, wb_b, wb_g2);
    
    let mut wb_bayer = bayer.clone();
    
    // Apply WB to each Bayer channel
    // Bayer array: Channel 0=R, Channel 1=G, Channel 2=B, Channel 3=unused
    for y in 0..height {
        for x in 0..width {
            wb_bayer[[y, x, 0]] *= wb_r as f64;   // R channel
            wb_bayer[[y, x, 1]] *= wb_g1 as f64;  // G channel
            wb_bayer[[y, x, 2]] *= wb_b as f64;   // B channel
        }
    }
    
    wb_bayer
}

fn demosaic_bayer(bayer: &Array3<f64>) -> Array3<f64> {
    let (height, width, _) = bayer.dim();
    let mut rgb = Array3::<f64>::zeros((height, width, 3));
    
    println!("Demosaicing {}x{} Bayer to RGB", width, height);
    
    // Simple bilinear demosaic
    for y in 2..height - 2 {
        for x in 2..width - 2 {
            let row_even = y % 2 == 0;
            let col_even = x % 2 == 0;
            
            let (r, g, b) = match (row_even, col_even) {
                (true, true) => {
                    // R pixel - sum all channels to get value
                    let r_val = bayer[[y, x, 0]] + bayer[[y, x, 1]] + bayer[[y, x, 2]] + bayer[[y, x, 3]];
                    
                    // Interpolate G from 4 neighbors
                    let g_left = bayer[[y, x - 1, 0]] + bayer[[y, x - 1, 1]] + bayer[[y, x - 1, 2]] + bayer[[y, x - 1, 3]];
                    let g_right = bayer[[y, x + 1, 0]] + bayer[[y, x + 1, 1]] + bayer[[y, x + 1, 2]] + bayer[[y, x + 1, 3]];
                    let g_top = bayer[[y - 1, x, 0]] + bayer[[y - 1, x, 1]] + bayer[[y - 1, x, 2]] + bayer[[y - 1, x, 3]];
                    let g_bottom = bayer[[y + 1, x, 0]] + bayer[[y + 1, x, 1]] + bayer[[y + 1, x, 2]] + bayer[[y + 1, x, 3]];
                    let g_val = (g_left + g_right + g_top + g_bottom) / 4.0;
                    
                    // Interpolate B from 4 diagonal neighbors
                    let b_tl = bayer[[y - 1, x - 1, 0]] + bayer[[y - 1, x - 1, 1]] + bayer[[y - 1, x - 1, 2]] + bayer[[y - 1, x - 1, 3]];
                    let b_tr = bayer[[y - 1, x + 1, 0]] + bayer[[y - 1, x + 1, 1]] + bayer[[y - 1, x + 1, 2]] + bayer[[y - 1, x + 1, 3]];
                    let b_bl = bayer[[y + 1, x - 1, 0]] + bayer[[y + 1, x - 1, 1]] + bayer[[y + 1, x - 1, 2]] + bayer[[y + 1, x - 1, 3]];
                    let b_br = bayer[[y + 1, x + 1, 0]] + bayer[[y + 1, x + 1, 1]] + bayer[[y + 1, x + 1, 2]] + bayer[[y + 1, x + 1, 3]];
                    let b_val = (b_tl + b_tr + b_bl + b_br) / 4.0;
                    
                    (r_val, g_val, b_val)
                }
                (true, false) => {
                    // G1 pixel
                    let g_val = bayer[[y, x, 0]] + bayer[[y, x, 1]] + bayer[[y, x, 2]] + bayer[[y, x, 3]];
                    
                    // Interpolate R from left/right
                    let r_left = bayer[[y, x - 1, 0]] + bayer[[y, x - 1, 1]] + bayer[[y, x - 1, 2]] + bayer[[y, x - 1, 3]];
                    let r_right = bayer[[y, x + 1, 0]] + bayer[[y, x + 1, 1]] + bayer[[y, x + 1, 2]] + bayer[[y, x + 1, 3]];
                    let r_val = (r_left + r_right) / 2.0;
                    
                    // Interpolate B from top/bottom
                    let b_top = bayer[[y - 1, x, 0]] + bayer[[y - 1, x, 1]] + bayer[[y - 1, x, 2]] + bayer[[y - 1, x, 3]];
                    let b_bottom = bayer[[y + 1, x, 0]] + bayer[[y + 1, x, 1]] + bayer[[y + 1, x, 2]] + bayer[[y + 1, x, 3]];
                    let b_val = (b_top + b_bottom) / 2.0;
                    
                    (r_val, g_val, b_val)
                }
                (false, true) => {
                    // G2 pixel
                    let g_val = bayer[[y, x, 0]] + bayer[[y, x, 1]] + bayer[[y, x, 2]] + bayer[[y, x, 3]];
                    
                    // Interpolate R from top/bottom
                    let r_top = bayer[[y - 1, x, 0]] + bayer[[y - 1, x, 1]] + bayer[[y - 1, x, 2]] + bayer[[y - 1, x, 3]];
                    let r_bottom = bayer[[y + 1, x, 0]] + bayer[[y + 1, x, 1]] + bayer[[y + 1, x, 2]] + bayer[[y + 1, x, 3]];
                    let r_val = (r_top + r_bottom) / 2.0;
                    
                    // Interpolate B from left/right
                    let b_left = bayer[[y, x - 1, 0]] + bayer[[y, x - 1, 1]] + bayer[[y, x - 1, 2]] + bayer[[y, x - 1, 3]];
                    let b_right = bayer[[y, x + 1, 0]] + bayer[[y, x + 1, 1]] + bayer[[y, x + 1, 2]] + bayer[[y, x + 1, 3]];
                    let b_val = (b_left + b_right) / 2.0;
                    
                    (r_val, g_val, b_val)
                }
                (false, false) => {
                    // B pixel
                    let b_val = bayer[[y, x, 0]] + bayer[[y, x, 1]] + bayer[[y, x, 2]] + bayer[[y, x, 3]];
                    
                    // Interpolate G from 4 neighbors
                    let g_left = bayer[[y, x - 1, 0]] + bayer[[y, x - 1, 1]] + bayer[[y, x - 1, 2]] + bayer[[y, x - 1, 3]];
                    let g_right = bayer[[y, x + 1, 0]] + bayer[[y, x + 1, 1]] + bayer[[y, x + 1, 2]] + bayer[[y, x + 1, 3]];
                    let g_top = bayer[[y - 1, x, 0]] + bayer[[y - 1, x, 1]] + bayer[[y - 1, x, 2]] + bayer[[y - 1, x, 3]];
                    let g_bottom = bayer[[y + 1, x, 0]] + bayer[[y + 1, x, 1]] + bayer[[y + 1, x, 2]] + bayer[[y + 1, x, 3]];
                    let g_val = (g_left + g_right + g_top + g_bottom) / 4.0;
                    
                    // Interpolate R from 4 diagonal neighbors
                    let r_tl = bayer[[y - 1, x - 1, 0]] + bayer[[y - 1, x - 1, 1]] + bayer[[y - 1, x - 1, 2]] + bayer[[y - 1, x - 1, 3]];
                    let r_tr = bayer[[y - 1, x + 1, 0]] + bayer[[y - 1, x + 1, 1]] + bayer[[y - 1, x + 1, 2]] + bayer[[y - 1, x + 1, 3]];
                    let r_bl = bayer[[y + 1, x - 1, 0]] + bayer[[y + 1, x - 1, 1]] + bayer[[y + 1, x - 1, 2]] + bayer[[y + 1, x - 1, 3]];
                    let r_br = bayer[[y + 1, x + 1, 0]] + bayer[[y + 1, x + 1, 1]] + bayer[[y + 1, x + 1, 2]] + bayer[[y + 1, x + 1, 3]];
                    let r_val = (r_tl + r_tr + r_bl + r_br) / 4.0;
                    
                    (r_val, g_val, b_val)
                }
            };
            
            rgb[[y, x, 0]] = r;
            rgb[[y, x, 1]] = g;
            rgb[[y, x, 2]] = b;
        }
    }
    
    rgb
}

fn simple_tone_map(rgb: &Array3<f64>) -> Array3<f64> {
    let (height, width, _) = rgb.dim();
    
    // Find max value for normalization
    let max_val = rgb.iter().copied().fold(0.0, f64::max);
    println!("Max RGB value before tone map: {:.6}", max_val);
    
    // Simple linear scaling to [0, 1]
    let mut tone_mapped = rgb.clone();
    if max_val > 0.0 {
        for y in 0..height {
            for x in 0..width {
                for c in 0..3 {
                    tone_mapped[[y, x, c]] = (rgb[[y, x, c]] / max_val).min(1.0).max(0.0);
                }
            }
        }
    }
    
    tone_mapped
}

fn save_rgb_as_png(rgb: &Array3<f64>, path: &Path) -> Result<()> {
    let (height, width, _) = rgb.dim();
    
    // Convert to u8
    let mut img_buffer = image::RgbImage::new(width as u32, height as u32);
    
    // Compute mean RGB for diagnostics
    let mut r_sum = 0.0;
    let mut g_sum = 0.0;
    let mut b_sum = 0.0;
    let mut count = 0;
    
    for y in 0..height {
        for x in 0..width {
            let r = (rgb[[y, x, 0]] * 255.0).round().clamp(0.0, 255.0) as u8;
            let g = (rgb[[y, x, 1]] * 255.0).round().clamp(0.0, 255.0) as u8;
            let b = (rgb[[y, x, 2]] * 255.0).round().clamp(0.0, 255.0) as u8;
            
            img_buffer.put_pixel(x as u32, y as u32, image::Rgb([r, g, b]));
            
            r_sum += r as f64;
            g_sum += g as f64;
            b_sum += b as f64;
            count += 1;
        }
    }
    
    println!("Final u8 output: mean R={:.2}, G={:.2}, B={:.2}", 
             r_sum / count as f64, g_sum / count as f64, b_sum / count as f64);
    
    img_buffer.save(path)?;
    println!("Saved to {:?}", path);
    
    Ok(())
}

fn main() -> Result<()> {
    env_logger::init();
    
    // Load a single NEF file
    let input_path = Path::new("realTest/_Z9Z5338.NEF");
    println!("Loading {:?}", input_path);
    
    let frame = load_bayer_frame(input_path)?;
    println!("Loaded Bayer frame: {}x{}", frame.data.dim().1, frame.data.dim().0);
    println!("WB from metadata: R={:.3}, G={:.3}, B={:.3}, G2={:.3}", 
             frame.meta.cam_mul[0], frame.meta.cam_mul[1], frame.meta.cam_mul[2], frame.meta.cam_mul[3]);
    
    // Apply white balance
    let wb_bayer = apply_white_balance(&frame.data, &frame.meta.cam_mul);
    
    // Demosaic
    let rgb = demosaic_bayer(&wb_bayer);
    
    // Simple tone map
    let tone_mapped = simple_tone_map(&rgb);
    
    // Save
    let output_path = Path::new("test_single_image.png");
    save_rgb_as_png(&tone_mapped, output_path)?;
    
    println!("\nDone! Check test_single_image.png");
    
    Ok(())
}

