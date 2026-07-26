use super::{ScanTask, DirectorContext};
use crate::events::{SystemEvent, LogLevel};
use crate::director::DirectorState;
use anyhow::Result;

pub struct HomeTurntableTask;

#[async_trait::async_trait]
impl ScanTask for HomeTurntableTask {
    fn name(&self) -> &'static str { "HomeTurntable" }
    
    async fn on_enter(&self, ctx: &DirectorContext) -> Result<bool> {
        ctx.bus.publish(SystemEvent::SystemMessage("Homing Turntable...".into(), LogLevel::Info));
        
        let mut t = ctx.turntable.lock().await;
        if let Err(e) = t.home().await {
             ctx.bus.publish(SystemEvent::SystemMessage(format!("Homing Error: {}", e), LogLevel::Error));
             let mut state = ctx.state.lock().await;
             *state = DirectorState::Error(format!("Homing Failed: {}", e));
             return Ok(false); 
        }
        Ok(true) 
    }
}

pub struct VerifyHardwareTask;

#[async_trait::async_trait]
impl ScanTask for VerifyHardwareTask {
    fn name(&self) -> &'static str { "VerifyHardware" }

    async fn on_enter(&self, ctx: &DirectorContext) -> Result<bool> {
        ctx.bus.publish(SystemEvent::SystemMessage("Verifying Hardware...".into(), LogLevel::Info));
        
        let mgr = ctx.cameras.lock().await;
        let connected = mgr.cameras.len();
        drop(mgr);
        
        if connected == 0 {
            ctx.bus.publish(SystemEvent::SystemMessage("No cameras detected! Check connections.".into(), LogLevel::Error));
            let mut state = ctx.state.lock().await;
            *state = DirectorState::Error("No cameras detected".into());
            Ok(false)
        } else {
            ctx.bus.publish(SystemEvent::SystemMessage(format!("Verified {} cameras.", connected), LogLevel::Success));
            Ok(true)
        }
    }
}

pub struct CheckExposureTask;

#[async_trait::async_trait]
impl ScanTask for CheckExposureTask {
    fn name(&self) -> &'static str { "CheckExposure" }
    
    async fn on_enter(&self, ctx: &DirectorContext) -> Result<bool> {
        ctx.bus.publish(SystemEvent::SystemMessage("Checking Exposure...".into(), LogLevel::Info));
        Ok(false) 
    }
    
    async fn on_frame(&self, ctx: &DirectorContext, frame: &image::ImageBuffer<image::Rgb<u8>, Vec<u8>>) -> Result<bool> {
       let (w, h) = frame.dimensions();
       let mut lum_sum = 0.0;
       let mut clipped = 0;
       for p in frame.pixels() {
           let l = 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
           lum_sum += l;
           if l > 250.0 { clipped += 1; }
       }
       let avg_lum = lum_sum / (w * h) as f32;
       let clipped_pct = clipped as f32 / (w * h) as f32;
       
       if avg_lum < 20.0 { 
            ctx.bus.publish(SystemEvent::SystemMessage("Scene too dark! Increase lighting.".into(), LogLevel::Error));
            let mut state = ctx.state.lock().await;
            *state = DirectorState::Error("Exposure Check Failed: Too Dark".into());
            return Ok(false);
       } else if clipped_pct > 0.10 { 
            ctx.bus.publish(SystemEvent::SystemMessage("Scene Overexposed! Reduce lighting.".into(), LogLevel::Error));
            let mut state = ctx.state.lock().await;
            *state = DirectorState::Error("Exposure Check Failed: Overexposed".into());
             return Ok(false);
       } else {
            ctx.bus.publish(SystemEvent::SystemMessage("Exposure OK.".into(), LogLevel::Success));
            return Ok(true); // Complete
       }
    }
}

pub struct CheckCenteringTask;

#[async_trait::async_trait]
impl ScanTask for CheckCenteringTask {
    fn name(&self) -> &'static str { "CheckCentering" }

    async fn on_enter(&self, ctx: &DirectorContext) -> Result<bool> {
        ctx.bus.publish(SystemEvent::SystemMessage("Checking Object Centering...".into(), LogLevel::Info));
        Ok(false)
    }

    async fn on_frame(&self, ctx: &DirectorContext, frame: &image::ImageBuffer<image::Rgb<u8>, Vec<u8>>) -> Result<bool> {
       let bg_guard = ctx.background_reference.lock().await;
       
       if let Some(bg) = bg_guard.as_ref() {
            let (w, h) = frame.dimensions();
            if bg.dimensions() != (w, h) {
                // Dim mismatch - advance (bypass)
                return Ok(true);
            }
            
            // Simple diff
            let mut x_sum = 0.0;
            let mut y_sum = 0.0;
            let mut mass = 0.0;
            
            // Subsampling for speed
            for y in (0..h).step_by(4) {
                for x in (0..w).step_by(4) {
                    let p1 = frame.get_pixel(x, y);
                    let p2 = bg.get_pixel(x, y);
                    let d = (p1[0] as i16 - p2[0] as i16).abs() + 
                            (p1[1] as i16 - p2[1] as i16).abs() +
                            (p1[2] as i16 - p2[2] as i16).abs();
                    
                    if d > 100 { 
                        x_sum += x as f32;
                        y_sum += y as f32;
                        mass += 1.0;
                    }
                }
            }
            
            if mass < 100.0 {
                ctx.bus.publish(SystemEvent::SystemMessage("No Object Detected!".into(), LogLevel::Warning));
            } else {
                let cx = x_sum / mass;
                let cy = y_sum / mass;
                
                let center_x = w as f32 / 2.0;
                let center_y = h as f32 / 2.0;
                
                let dist = ((cx - center_x).powi(2) + (cy - center_y).powi(2)).sqrt();
                let max_dist = w.min(h) as f32 * 0.25; 
                
                if dist > max_dist {
                     ctx.bus.publish(SystemEvent::SystemMessage("Object Off-Center! Please center it.".into(), LogLevel::Error));
                     let mut state = ctx.state.lock().await;
                     *state = DirectorState::Error("Centering Check Failed".into());
                     return Ok(false);
                }
            }
            ctx.bus.publish(SystemEvent::SystemMessage("Centering OK.".into(), LogLevel::Success));
            Ok(true)
       } else {
           ctx.bus.publish(SystemEvent::SystemMessage("Skipping Centering Check (No BG)".into(), LogLevel::Warning));
           Ok(true)
       }
    }
}
