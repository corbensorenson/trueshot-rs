use anyhow::Context;
use image::{DynamicImage, GrayImage};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;
use ort::value::ValueType;
use std::collections::VecDeque;
use std::path::Path;

use crate::ai::model_cache::ensure_cached_model;
use crate::ai::model_manifest::verify_model_manifest;
use crate::security::provenance::{set_model_fingerprint, ModelFingerprint};

/// Semantic Segmentation using SAM (via ONNX)
pub struct SegmentationEngine {
    backend: SegmentationBackend,
}

enum SegmentationBackend {
    Heuristic,
    Onnx {
        session: Session,
        input_name: String,
        layout: ModelLayout,
        target_width: Option<u32>,
        target_height: Option<u32>,
        allow_fallback: bool,
    },
}

impl SegmentationEngine {
    pub fn new(model_path: &str) -> anyhow::Result<Self> {
        let path = model_path.trim();
        if path.is_empty() {
            return Ok(Self { backend: SegmentationBackend::Heuristic });
        }

        let model_path = Path::new(path);
        if !model_path.exists() {
            anyhow::bail!("Segmentation model not found at {}", model_path.display());
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

        let allow_fallback = allow_fallback();
        let session = Session::builder()
            .context("Failed to build ONNX session")?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_intra_threads(num_cpus::get().max(1))?
            .with_inter_threads((num_cpus::get() / 2).max(1))?
            .commit_from_file(&resolved_model_path)
            .or_else(|err| {
                if let Some(rollback) = &rollback_record {
                    tracing::warn!(
                        "Primary segmentation model load failed ({}), attempting rollback model {}",
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
                        .context("Failed to load rollback segmentation model")
                } else {
                    Err(err.into())
                }
            })?;

        let input = session
            .inputs
            .get(0)
            .context("ONNX model has no inputs")?;
        let input_name = input.name.clone();
        let layout = infer_layout(&input.input_type);
        let (target_width, target_height) = infer_target_dimensions(&input.input_type, layout);

        Ok(Self {
            backend: SegmentationBackend::Onnx {
                session,
                input_name,
                layout,
                target_width,
                target_height,
                allow_fallback,
            },
        })
    }

    pub fn segment(&mut self, img: &DynamicImage) -> anyhow::Result<DynamicImage> {
        match &mut self.backend {
            SegmentationBackend::Heuristic => Ok(DynamicImage::ImageLuma8(heuristic_mask(img))),
            SegmentationBackend::Onnx {
                session,
                input_name,
                layout,
                target_width,
                target_height,
                allow_fallback,
            } => {
                match run_onnx_segmentation(
                    session,
                    input_name,
                    *layout,
                    *target_width,
                    *target_height,
                    img,
                ) {
                    Ok(mask) => Ok(DynamicImage::ImageLuma8(mask)),
                    Err(err) => {
                        if *allow_fallback {
                            tracing::warn!(
                                "Segmentation model failed: {err}; falling back to heuristic mask"
                            );
                            Ok(DynamicImage::ImageLuma8(heuristic_mask(img)))
                        } else {
                            Err(err)
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ModelLayout {
    Nchw,
    Nhwc,
}

fn run_onnx_segmentation(
    session: &mut Session,
    input_name: &str,
    layout: ModelLayout,
    target_width: Option<u32>,
    target_height: Option<u32>,
    img: &DynamicImage,
) -> anyhow::Result<GrayImage> {
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

    if outputs.len() == 0 {
        anyhow::bail!("Segmentation model returned no outputs");
    }

    let (shape, data) = outputs[0].try_extract_tensor::<f32>()?;
    let mut mask = build_mask_from_output(shape, data)?;

    if mask.dimensions() != (orig_w, orig_h) {
        mask = image::imageops::resize(&mask, orig_w, orig_h, image::imageops::FilterType::Lanczos3);
    }

    Ok(mask)
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

fn build_mask_from_output(shape: &ort::tensor::Shape, data: &[f32]) -> anyhow::Result<GrayImage> {
    let dims: Vec<usize> = shape
        .iter()
        .map(|d| {
            if *d <= 0 {
                anyhow::bail!("Segmentation output has dynamic or invalid shape");
            }
            Ok(*d as usize)
        })
        .collect::<Result<Vec<_>, anyhow::Error>>()?;

    let (channels, height, width, channel_last) = match dims.len() {
        4 => {
            if dims[1] <= 4 {
                (dims[1], dims[2], dims[3], false)
            } else {
                (dims[3], dims[1], dims[2], true)
            }
        }
        3 => {
            if dims[0] <= 4 {
                (dims[0], dims[1], dims[2], false)
            } else if dims[2] <= 4 {
                (dims[2], dims[0], dims[1], true)
            } else {
                (1, dims[1], dims[2], false)
            }
        }
        2 => (1, dims[0], dims[1], false),
        _ => anyhow::bail!("Unsupported segmentation output shape"),
    };

    if height == 0 || width == 0 {
        anyhow::bail!("Segmentation output has zero dimensions");
    }

    let mut mask = GrayImage::new(width as u32, height as u32);

    if channels > 1 {
        for y in 0..height {
            for x in 0..width {
                let mut best_class = 0usize;
                let mut best_value = f32::MIN;
                for c in 0..channels {
                    let idx = if channel_last {
                        (y * width + x) * channels + c
                    } else {
                        c * height * width + (y * width + x)
                    };
                    if idx >= data.len() {
                        continue;
                    }
                    let value = data[idx];
                    if value > best_value {
                        best_value = value;
                        best_class = c;
                    }
                }
                let out = if best_class == 0 { 0u8 } else { 255u8 };
                mask.put_pixel(x as u32, y as u32, image::Luma([out]));
            }
        }
        return Ok(mask);
    }

    let mut min_val = f32::INFINITY;
    let mut max_val = f32::NEG_INFINITY;
    for &value in data.iter().take(height * width) {
        min_val = min_val.min(value);
        max_val = max_val.max(value);
    }

    let use_sigmoid = min_val < 0.0 || max_val > 1.0;
    let mut values = vec![0.0f32; height * width];
    for idx in 0..values.len() {
        let raw = data.get(idx).copied().unwrap_or(0.0);
        values[idx] = if use_sigmoid { 1.0 / (1.0 + (-raw).exp()) } else { raw };
    }

    let threshold = otsu_threshold(&values);
    for y in 0..height {
        for x in 0..width {
            let v = values[y * width + x];
            let out = if v >= threshold { 255u8 } else { 0u8 };
            mask.put_pixel(x as u32, y as u32, image::Luma([out]));
        }
    }

    let mut binary = mask.into_raw();
    binary_close(&mut binary, width, height);
    keep_largest_component(&mut binary, width, height);
    Ok(GrayImage::from_raw(width as u32, height as u32, binary).unwrap())
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

fn infer_target_dimensions(value_type: &ValueType, layout: ModelLayout) -> (Option<u32>, Option<u32>) {
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

fn heuristic_mask(img: &DynamicImage) -> GrayImage {
    let rgb = img.to_rgb8();
    let (width, height) = rgb.dimensions();
    let w = width as usize;
    let h = height as usize;
    let len = w * h;

    let mut mean = [0.0f32; 3];
    for pixel in rgb.pixels() {
        mean[0] += pixel[0] as f32;
        mean[1] += pixel[1] as f32;
        mean[2] += pixel[2] as f32;
    }
    mean[0] /= len as f32;
    mean[1] /= len as f32;
    mean[2] /= len as f32;

    let mut contrast = vec![0.0f32; len];
    let mut saturation = vec![0.0f32; len];
    let mut luma = vec![0.0f32; len];

    for (idx, pixel) in rgb.pixels().enumerate() {
        let r = pixel[0] as f32;
        let g = pixel[1] as f32;
        let b = pixel[2] as f32;
        contrast[idx] = ((r - mean[0]).abs() + (g - mean[1]).abs() + (b - mean[2]).abs()) / 3.0;

        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
        saturation[idx] = sat;
        luma[idx] = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    }

    let edges = sobel_magnitude(&luma, w, h);
    normalize_in_place(&mut contrast);
    normalize_in_place(&mut saturation);
    let mut edges = edges;
    normalize_in_place(&mut edges);

    let mut saliency = vec![0.0f32; len];
    for i in 0..len {
        saliency[i] = 0.5 * contrast[i] + 0.3 * edges[i] + 0.2 * saturation[i];
    }
    let saliency = box_blur(&saliency, w, h);
    let threshold = otsu_threshold(&saliency);

    let mut mask = vec![0u8; len];
    for i in 0..len {
        mask[i] = if saliency[i] >= threshold { 255 } else { 0 };
    }
    binary_close(&mut mask, w, h);
    keep_largest_component(&mut mask, w, h);
    GrayImage::from_raw(width, height, mask).unwrap()
}

fn sobel_magnitude(luma: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; luma.len()];
    if width < 3 || height < 3 {
        return out;
    }
    let idx = |x: usize, y: usize| y * width + x;
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let gx = -1.0 * luma[idx(x - 1, y - 1)]
                + 1.0 * luma[idx(x + 1, y - 1)]
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

fn normalize_in_place(values: &mut [f32]) {
    let mut min_val = f32::INFINITY;
    let mut max_val = f32::NEG_INFINITY;
    for &v in values.iter() {
        min_val = min_val.min(v);
        max_val = max_val.max(v);
    }
    let denom = (max_val - min_val).max(1e-6);
    for v in values.iter_mut() {
        *v = (*v - min_val) / denom;
    }
}

fn box_blur(values: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; values.len()];
    let idx = |x: usize, y: usize| y * width + x;
    for y in 0..height {
        for x in 0..width {
            let mut acc: f32 = 0.0;
            let mut count: f32 = 0.0;
            for dy in [-1isize, 0, 1] {
                for dx in [-1isize, 0, 1] {
                    let nx = x as isize + dx;
                    let ny = y as isize + dy;
                    if nx >= 0 && ny >= 0 && nx < width as isize && ny < height as isize {
                        acc += values[idx(nx as usize, ny as usize)];
                        count += 1.0;
                    }
                }
            }
            out[idx(x, y)] = acc / count.max(1.0);
        }
    }
    out
}

fn otsu_threshold(values: &[f32]) -> f32 {
    let bins = 256usize;
    let mut hist = vec![0u32; bins];
    for &value in values {
        let v = value.clamp(0.0, 1.0);
        let idx = (v * (bins as f32 - 1.0)).round() as usize;
        hist[idx] += 1;
    }
    let total: f32 = values.len() as f32;
    let mut sum_total = 0.0f32;
    for (i, count) in hist.iter().enumerate() {
        sum_total += (i as f32) * (*count as f32);
    }

    let mut sum_b = 0.0f32;
    let mut w_b = 0.0f32;
    let mut max_var = -1.0f32;
    let mut threshold = 0usize;

    for (i, count) in hist.iter().enumerate() {
        w_b += *count as f32;
        if w_b <= 0.0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f <= 0.0 {
            break;
        }
        sum_b += (i as f32) * (*count as f32);
        let m_b = sum_b / w_b;
        let m_f = (sum_total - sum_b) / w_f;
        let var_between = w_b * w_f * (m_b - m_f) * (m_b - m_f);
        if var_between > max_var {
            max_var = var_between;
            threshold = i;
        }
    }
    threshold as f32 / (bins as f32 - 1.0)
}

fn binary_close(mask: &mut [u8], width: usize, height: usize) {
    let mut temp = mask.to_vec();
    dilate(&mut temp, width, height);
    erode(&mut temp, width, height);
    mask.copy_from_slice(&temp);
}

fn dilate(mask: &mut [u8], width: usize, height: usize) {
    let original = mask.to_vec();
    let idx = |x: usize, y: usize| y * width + x;
    for y in 0..height {
        for x in 0..width {
            let mut max_val = 0u8;
            for dy in [-1isize, 0, 1] {
                for dx in [-1isize, 0, 1] {
                    let nx = x as isize + dx;
                    let ny = y as isize + dy;
                    if nx >= 0 && ny >= 0 && nx < width as isize && ny < height as isize {
                        let v = original[idx(nx as usize, ny as usize)];
                        if v > max_val {
                            max_val = v;
                        }
                    }
                }
            }
            mask[idx(x, y)] = max_val;
        }
    }
}

fn erode(mask: &mut [u8], width: usize, height: usize) {
    let original = mask.to_vec();
    let idx = |x: usize, y: usize| y * width + x;
    for y in 0..height {
        for x in 0..width {
            let mut min_val = 255u8;
            for dy in [-1isize, 0, 1] {
                for dx in [-1isize, 0, 1] {
                    let nx = x as isize + dx;
                    let ny = y as isize + dy;
                    if nx >= 0 && ny >= 0 && nx < width as isize && ny < height as isize {
                        let v = original[idx(nx as usize, ny as usize)];
                        if v < min_val {
                            min_val = v;
                        }
                    }
                }
            }
            mask[idx(x, y)] = min_val;
        }
    }
}

fn keep_largest_component(mask: &mut [u8], width: usize, height: usize) {
    let mut visited = vec![false; mask.len()];
    let mut largest = Vec::new();
    let idx = |x: usize, y: usize| y * width + x;

    for y in 0..height {
        for x in 0..width {
            let start = idx(x, y);
            if mask[start] == 0 || visited[start] {
                continue;
            }
            let mut queue = VecDeque::new();
            let mut component = Vec::new();
            queue.push_back((x, y));
            visited[start] = true;
            while let Some((cx, cy)) = queue.pop_front() {
                component.push(idx(cx, cy));
                for (dx, dy) in [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
                    let nx = cx as isize + dx;
                    let ny = cy as isize + dy;
                    if nx >= 0 && ny >= 0 && nx < width as isize && ny < height as isize {
                        let ni = idx(nx as usize, ny as usize);
                        if !visited[ni] && mask[ni] > 0 {
                            visited[ni] = true;
                            queue.push_back((nx as usize, ny as usize));
                        }
                    }
                }
            }
            if component.len() > largest.len() {
                largest = component;
            }
        }
    }

    if largest.is_empty() {
        return;
    }
    for value in mask.iter_mut() {
        *value = 0;
    }
    for idx in largest {
        mask[idx] = 255;
    }
}

fn allow_fallback() -> bool {
    if is_production() {
        return env_flag("TRUESHOT_SEGMENTATION_ALLOW_FALLBACK");
    }
    true
}

fn is_production() -> bool {
    std::env::var("TRUESHOT_ENV")
        .map(|env| env == "production")
        .unwrap_or(false)
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_mask_returns_binary_image() {
        let mut img = image::RgbImage::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                let value = if x < 16 { 240 } else { 10 };
                img.put_pixel(x, y, image::Rgb([value, value, value]));
            }
        }
        let mask = heuristic_mask(&DynamicImage::ImageRgb8(img));
        assert_eq!(mask.dimensions(), (32, 32));
        let unique: std::collections::HashSet<_> = mask.pixels().map(|p| p[0]).collect();
        assert!(unique.contains(&0));
        assert!(unique.contains(&255));
    }
}
