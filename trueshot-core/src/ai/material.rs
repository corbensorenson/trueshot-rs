use crate::ai::model_cache::ensure_cached_model;
use crate::ai::model_manifest::verify_model_manifest;
use crate::security::provenance::{set_model_fingerprint, ModelFingerprint};
use anyhow::Context;
use image::{DynamicImage, GrayImage, Luma};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;
use ort::value::ValueType;
use std::path::Path;

/// Spatially Varying BRDF Estimator
/// Produces roughness/metallic maps using a physically inspired heuristic.
pub struct MaterialEstimator {
    backend: MaterialBackend,
}

enum MaterialBackend {
    Heuristic,
    Onnx {
        session: Session,
        input_name: String,
        layout: ModelLayout,
        target_width: Option<u32>,
        target_height: Option<u32>,
    },
}

#[derive(Debug, Clone, Copy)]
enum ModelLayout {
    Nchw,
    Nhwc,
}

impl MaterialEstimator {
    pub fn new(model_path: &str) -> anyhow::Result<Self> {
        let path = model_path.trim();
        if path.is_empty() {
            return Ok(Self {
                backend: MaterialBackend::Heuristic,
            });
        }

        let model_path = Path::new(path);
        if !model_path.exists() {
            anyhow::bail!("Material model not found at {}", model_path.display());
        }
        let mut resolved_model_path = model_path.to_path_buf();
        let mut rollback_record = None;
        if let Some(info) = verify_model_manifest(model_path)? {
            set_model_fingerprint(ModelFingerprint {
                model_id: info.model_id.clone(),
                model_version: info.model_version.clone(),
                model_weights_hash: info.weights_sha256.clone(),
            });
            if let Some(cache) = ensure_cached_model(model_path, &info)? {
                resolved_model_path = cache.primary.model_path.clone();
                rollback_record = cache.rollback;
            }
        }

        let session = Session::builder()
            .context("Failed to build ONNX session")?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_intra_threads(num_cpus::get().max(1))?
            .with_inter_threads((num_cpus::get() / 2).max(1))?
            .commit_from_file(&resolved_model_path)
            .or_else(|err| {
                if let Some(rollback) = &rollback_record {
                    tracing::warn!(
                        "Primary model load failed ({}), attempting rollback model {}",
                        err,
                        rollback.model_path.display()
                    );
                    set_model_fingerprint(ModelFingerprint {
                        model_id: rollback.model_id.clone(),
                        model_version: rollback.model_version.clone(),
                        model_weights_hash: rollback.weights_sha256.clone(),
                    });
                    Session::builder()
                        .context("Failed to build ONNX session")?
                        .with_optimization_level(GraphOptimizationLevel::Level3)?
                        .with_intra_threads(num_cpus::get().max(1))?
                        .with_inter_threads((num_cpus::get() / 2).max(1))?
                        .commit_from_file(&rollback.model_path)
                        .context("Failed to load rollback ONNX material model")
                } else {
                    Err(err.into())
                }
            })?;

        let input = session.inputs.get(0).context("ONNX model has no inputs")?;
        let input_name = input.name.clone();
        let layout = infer_layout(&input.input_type);
        let (target_width, target_height) = infer_target_dimensions(&input.input_type, layout);

        Ok(Self {
            backend: MaterialBackend::Onnx {
                session,
                input_name,
                layout,
                target_width,
                target_height,
            },
        })
    }

    pub fn estimate(&mut self, img: &DynamicImage) -> anyhow::Result<(DynamicImage, DynamicImage)> {
        match &mut self.backend {
            MaterialBackend::Heuristic => Ok(heuristic_material_estimate(img)),
            MaterialBackend::Onnx {
                session,
                input_name,
                layout,
                target_width,
                target_height,
            } => {
                match run_onnx_material(
                    session,
                    input_name,
                    *layout,
                    *target_width,
                    *target_height,
                    img,
                ) {
                    Ok(result) => Ok(result),
                    Err(err) => {
                        tracing::warn!(
                            "ONNX material estimation failed: {err}; falling back to heuristic"
                        );
                        Ok(heuristic_material_estimate(img))
                    }
                }
            }
        }
    }
}

fn heuristic_material_estimate(img: &DynamicImage) -> (DynamicImage, DynamicImage) {
    let rgb = img.to_rgb8();
    let (width, height) = rgb.dimensions();
    let len = (width * height) as usize;

    let mut luminance = vec![0.0f32; len];
    let mut saturation = vec![0.0f32; len];

    for (idx, pixel) in rgb.pixels().enumerate() {
        let [r, g, b] = pixel.0;
        let lr = srgb_to_linear(r as f32 / 255.0);
        let lg = srgb_to_linear(g as f32 / 255.0);
        let lb = srgb_to_linear(b as f32 / 255.0);

        let max = lr.max(lg).max(lb);
        let min = lr.min(lg).min(lb);
        let delta = (max - min).max(1e-6);
        let sat = if max > 0.0 { delta / max } else { 0.0 };
        let lum = 0.2126 * lr + 0.7152 * lg + 0.0722 * lb;

        luminance[idx] = lum;
        saturation[idx] = sat;
    }

    let grad = sobel_magnitude(&luminance, width as usize, height as usize);
    let max_grad = grad.iter().cloned().fold(0.0f32, f32::max).max(1e-6);

    let mut roughness = GrayImage::new(width, height);
    let mut metallic = GrayImage::new(width, height);

    for y in 0..height as usize {
        for x in 0..width as usize {
            let idx = y * width as usize + x;
            let lum = luminance[idx].clamp(0.0, 1.0);
            let sat = saturation[idx].clamp(0.0, 1.0);
            let detail = (grad[idx] / max_grad).clamp(0.0, 1.0);

            // Specular proxy: bright and low saturation
            let spec = (lum * (1.0 - sat)).clamp(0.0, 1.0);

            // Roughness: higher with surface detail and lower with strong specular highlights
            let rough = (0.25 + 0.55 * detail + 0.20 * (1.0 - spec)).clamp(0.0, 1.0);

            // Metallic: higher with saturated reflections and higher energy
            let metal = ((0.7 * sat + 0.3 * lum).powf(0.85)).clamp(0.0, 1.0);

            roughness.put_pixel(x as u32, y as u32, Luma([(rough * 255.0) as u8]));
            metallic.put_pixel(x as u32, y as u32, Luma([(metal * 255.0) as u8]));
        }
    }

    (
        DynamicImage::ImageLuma8(roughness),
        DynamicImage::ImageLuma8(metallic),
    )
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn sobel_magnitude(luma: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; luma.len()];
    if width < 3 || height < 3 {
        return out;
    }

    let idx = |x: usize, y: usize| y * width + x;
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let gx = -1.0 * luma[idx(x - 1, y - 1)] + 1.0 * luma[idx(x + 1, y - 1)]
                - 2.0 * luma[idx(x - 1, y)]
                + 2.0 * luma[idx(x + 1, y)]
                - 1.0 * luma[idx(x - 1, y + 1)]
                + 1.0 * luma[idx(x + 1, y + 1)];

            let gy = -1.0 * luma[idx(x - 1, y - 1)]
                - 2.0 * luma[idx(x, y - 1)]
                - 1.0 * luma[idx(x + 1, y - 1)]
                + 1.0 * luma[idx(x - 1, y + 1)]
                + 2.0 * luma[idx(x, y + 1)]
                + 1.0 * luma[idx(x + 1, y + 1)];

            out[idx(x, y)] = (gx * gx + gy * gy).sqrt();
        }
    }
    out
}

fn infer_layout(value_type: &ValueType) -> ModelLayout {
    match value_type {
        ValueType::Tensor { shape, .. } if shape.len() == 4 => {
            if shape[1] == 3 {
                ModelLayout::Nchw
            } else if shape[3] == 3 {
                ModelLayout::Nhwc
            } else {
                ModelLayout::Nchw
            }
        }
        _ => ModelLayout::Nchw,
    }
}

fn infer_target_dimensions(
    value_type: &ValueType,
    layout: ModelLayout,
) -> (Option<u32>, Option<u32>) {
    if let ValueType::Tensor { shape, .. } = value_type {
        if shape.len() == 4 {
            let (h, w) = match layout {
                ModelLayout::Nchw => (shape[2], shape[3]),
                ModelLayout::Nhwc => (shape[1], shape[2]),
            };
            let height = if h > 0 { Some(h as u32) } else { None };
            let width = if w > 0 { Some(w as u32) } else { None };
            return (width, height);
        }
    }
    (None, None)
}

fn run_onnx_material(
    session: &mut Session,
    input_name: &str,
    layout: ModelLayout,
    target_width: Option<u32>,
    target_height: Option<u32>,
    img: &DynamicImage,
) -> anyhow::Result<(DynamicImage, DynamicImage)> {
    let rgb = img.to_rgb8();
    let (orig_w, orig_h) = rgb.dimensions();
    let width = target_width.unwrap_or(orig_w);
    let height = target_height.unwrap_or(orig_h);

    let resized = if width != orig_w || height != orig_h {
        image::imageops::resize(&rgb, width, height, image::imageops::FilterType::Lanczos3)
    } else {
        rgb
    };

    let (input_shape, input_data) = match layout {
        ModelLayout::Nchw => build_input_nchw(&resized),
        ModelLayout::Nhwc => build_input_nhwc(&resized),
    };

    let outputs = session.run(ort::inputs! {
        input_name => TensorRef::from_array_view((input_shape, input_data.as_slice()))?
    })?;

    let (mut roughness, mut metallic) = if outputs.len() >= 2 {
        let (shape_r, data_r) = outputs[0].try_extract_tensor::<f32>()?;
        let (shape_m, data_m) = outputs[1].try_extract_tensor::<f32>()?;
        (
            build_single_map(shape_r, data_r)?,
            build_single_map(shape_m, data_m)?,
        )
    } else {
        let (shape, data) = outputs[0].try_extract_tensor::<f32>()?;
        (
            build_map_from_output(shape, data, OutputChannel::Roughness)?,
            build_map_from_output(shape, data, OutputChannel::Metallic)?,
        )
    };

    if roughness.dimensions() != (orig_w, orig_h) {
        roughness = image::imageops::resize(
            &roughness,
            orig_w,
            orig_h,
            image::imageops::FilterType::Lanczos3,
        );
    }
    if metallic.dimensions() != (orig_w, orig_h) {
        metallic = image::imageops::resize(
            &metallic,
            orig_w,
            orig_h,
            image::imageops::FilterType::Lanczos3,
        );
    }

    Ok((
        DynamicImage::ImageLuma8(roughness),
        DynamicImage::ImageLuma8(metallic),
    ))
}

fn build_input_nchw(rgb: &image::RgbImage) -> (Vec<usize>, Vec<f32>) {
    let (width, height) = rgb.dimensions();
    let h = height as usize;
    let w = width as usize;
    let mut data = vec![0.0f32; 1 * 3 * h * w];
    for y in 0..h {
        for x in 0..w {
            let pixel = rgb.get_pixel(x as u32, y as u32);
            let r = pixel[0] as f32 / 255.0;
            let g = pixel[1] as f32 / 255.0;
            let b = pixel[2] as f32 / 255.0;
            let base = y * w + x;
            data[0 * h * w + base] = r;
            data[1 * h * w + base] = g;
            data[2 * h * w + base] = b;
        }
    }
    (vec![1, 3, h, w], data)
}

fn build_input_nhwc(rgb: &image::RgbImage) -> (Vec<usize>, Vec<f32>) {
    let (width, height) = rgb.dimensions();
    let h = height as usize;
    let w = width as usize;
    let mut data = vec![0.0f32; 1 * h * w * 3];
    for y in 0..h {
        for x in 0..w {
            let pixel = rgb.get_pixel(x as u32, y as u32);
            let r = pixel[0] as f32 / 255.0;
            let g = pixel[1] as f32 / 255.0;
            let b = pixel[2] as f32 / 255.0;
            let base = (y * w + x) * 3;
            data[base] = r;
            data[base + 1] = g;
            data[base + 2] = b;
        }
    }
    (vec![1, h, w, 3], data)
}

enum OutputChannel {
    Roughness,
    Metallic,
}

fn build_map_from_output(
    shape: &ort::tensor::Shape,
    data: &[f32],
    channel: OutputChannel,
) -> anyhow::Result<GrayImage> {
    let dims: Vec<usize> = shape
        .iter()
        .map(|d| {
            if *d <= 0 {
                anyhow::bail!("Model output has dynamic or invalid shape");
            }
            Ok(*d as usize)
        })
        .collect::<Result<Vec<_>, anyhow::Error>>()?;

    match dims.as_slice() {
        [1, 2, h, w] => Ok(extract_nchw_map(*h, *w, data, channel)),
        [1, h, w, 2] => Ok(extract_nhwc_map(*h, *w, data, channel)),
        [2, h, w] => Ok(extract_chw_map(*h, *w, data, channel)),
        [h, w, 2] => Ok(extract_hwc_map(*h, *w, data, channel)),
        _ => anyhow::bail!("Unsupported model output shape: {:?}", dims),
    }
}

fn build_single_map(shape: &ort::tensor::Shape, data: &[f32]) -> anyhow::Result<GrayImage> {
    let dims: Vec<usize> = shape
        .iter()
        .map(|d| {
            if *d <= 0 {
                anyhow::bail!("Model output has dynamic or invalid shape");
            }
            Ok(*d as usize)
        })
        .collect::<Result<Vec<_>, anyhow::Error>>()?;

    match dims.as_slice() {
        [1, 1, h, w] => Ok(extract_single_nchw(*h, *w, data)),
        [1, h, w, 1] => Ok(extract_single_nhwc(*h, *w, data)),
        [1, h, w] => Ok(extract_single_hw(*h, *w, data)),
        [h, w] => Ok(extract_single_hw(*h, *w, data)),
        _ => anyhow::bail!("Unsupported single-channel output shape: {:?}", dims),
    }
}

fn extract_nchw_map(h: usize, w: usize, data: &[f32], channel: OutputChannel) -> GrayImage {
    let channel_idx = match channel {
        OutputChannel::Roughness => 0usize,
        OutputChannel::Metallic => 1usize,
    };
    let mut img = GrayImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let idx = ((channel_idx * h + y) * w + x) as usize;
            img.put_pixel(x as u32, y as u32, Luma([to_u8(data[idx])]));
        }
    }
    img
}

fn extract_nhwc_map(h: usize, w: usize, data: &[f32], channel: OutputChannel) -> GrayImage {
    let channel_idx = match channel {
        OutputChannel::Roughness => 0usize,
        OutputChannel::Metallic => 1usize,
    };
    let mut img = GrayImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let idx = ((y * w + x) * 2 + channel_idx) as usize;
            img.put_pixel(x as u32, y as u32, Luma([to_u8(data[idx])]));
        }
    }
    img
}

fn extract_chw_map(h: usize, w: usize, data: &[f32], channel: OutputChannel) -> GrayImage {
    let channel_idx = match channel {
        OutputChannel::Roughness => 0usize,
        OutputChannel::Metallic => 1usize,
    };
    let mut img = GrayImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let idx = ((channel_idx * h + y) * w + x) as usize;
            img.put_pixel(x as u32, y as u32, Luma([to_u8(data[idx])]));
        }
    }
    img
}

fn extract_hwc_map(h: usize, w: usize, data: &[f32], channel: OutputChannel) -> GrayImage {
    let channel_idx = match channel {
        OutputChannel::Roughness => 0usize,
        OutputChannel::Metallic => 1usize,
    };
    let mut img = GrayImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let idx = ((y * w + x) * 2 + channel_idx) as usize;
            img.put_pixel(x as u32, y as u32, Luma([to_u8(data[idx])]));
        }
    }
    img
}

fn extract_single_nchw(h: usize, w: usize, data: &[f32]) -> GrayImage {
    let mut img = GrayImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            img.put_pixel(x as u32, y as u32, Luma([to_u8(data[idx])]));
        }
    }
    img
}

fn extract_single_nhwc(h: usize, w: usize, data: &[f32]) -> GrayImage {
    let mut img = GrayImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            img.put_pixel(x as u32, y as u32, Luma([to_u8(data[idx])]));
        }
    }
    img
}

fn extract_single_hw(h: usize, w: usize, data: &[f32]) -> GrayImage {
    let mut img = GrayImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            img.put_pixel(x as u32, y as u32, Luma([to_u8(data[idx])]));
        }
    }
    img
}

fn to_u8(value: f32) -> u8 {
    let clamped = value.clamp(0.0, 1.0);
    (clamped * 255.0).round().clamp(0.0, 255.0) as u8
}
