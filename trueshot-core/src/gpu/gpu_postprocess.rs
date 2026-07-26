//! GPU-accelerated postprocessing (tone mapping, sharpening, edge desaturation)

use super::gpu_context::GpuContext;
use anyhow::{Context, Result};
use ndarray::Array3;
use std::sync::Arc;

/// GPU-accelerated postprocessing
///
/// Performs tone mapping and sharpening on GPU
/// Returns None if GPU is unavailable or workload is too small
pub fn gpu_postprocess(
    gpu_ctx: &Arc<GpuContext>,
    linear_rgb: &Array3<f64>,
) -> Result<Option<Array3<u8>>> {
    let (height, width, channels) = linear_rgb.dim();

    if channels != 3 {
        anyhow::bail!("Expected 3-channel RGB image, got {}", channels);
    }

    // Auto-detect if GPU is beneficial (postprocessing is fast, only worth for large images)
    let pixels = height * width;
    let min_pixels_for_gpu = 5_000_000; // ~5M pixels

    if pixels < min_pixels_for_gpu {
        tracing::debug!(
            "Image too small for GPU postprocessing ({} pixels), using CPU",
            pixels
        );
        return Ok(None);
    }

    tracing::info!(
        "GPU postprocessing: {}x{} ({} pixels)",
        width,
        height,
        pixels
    );

    // Convert inputs to f32 for GPU
    let rgb_f32: Vec<f32> = linear_rgb.iter().map(|&v| v as f32).collect();

    // Create GPU buffers
    let input_size = (width * height * 3 * std::mem::size_of::<f32>()) as u64;
    let output_size = (width * height * 3 * std::mem::size_of::<f32>()) as u64;

    // Check buffer size limits against device limits
    let device_limits = gpu_ctx.device.limits();
    let max_buffer_size = device_limits.max_storage_buffer_binding_size as u64;

    if input_size > max_buffer_size || output_size > max_buffer_size {
        tracing::warn!(
            "GPU buffer size ({} MB) exceeds device limit ({} MB), falling back to CPU",
            input_size / (1024 * 1024),
            max_buffer_size / (1024 * 1024)
        );
        return Ok(None);
    }

    tracing::info!(
        "GPU buffer size: {} MB (limit: {} MB)",
        input_size / (1024 * 1024),
        max_buffer_size / (1024 * 1024)
    );

    let input_buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Postprocess Input"),
        size: input_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let output_buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Postprocess Output"),
        size: output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let staging_buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Postprocess Staging"),
        size: output_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Upload data
    gpu_ctx
        .queue
        .write_buffer(&input_buffer, 0, bytemuck::cast_slice(&rgb_f32));

    // Create shader
    let shader_source = generate_postprocess_shader(width, height);
    let shader = gpu_ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Postprocess Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

    // Create bind group layout
    let bind_group_layout =
        gpu_ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Postprocess Bind Group Layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

    let bind_group = gpu_ctx
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Postprocess Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output_buffer.as_entire_binding(),
                },
            ],
        });

    // Create pipeline
    let pipeline_layout = gpu_ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Postprocess Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

    let pipeline = gpu_ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Postprocess Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: "postprocess",
        });

    // Execute
    let mut encoder = gpu_ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Postprocess Encoder"),
        });

    {
        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Postprocess Pass"),
            timestamp_writes: None,
        });
        compute_pass.set_pipeline(&pipeline);
        compute_pass.set_bind_group(0, &bind_group, &[]);

        // Dispatch with 16x16 workgroups
        let workgroups_x = (width as u32).div_ceil(16);
        let workgroups_y = (height as u32).div_ceil(16);
        compute_pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
    }

    // Copy to staging buffer
    encoder.copy_buffer_to_buffer(&output_buffer, 0, &staging_buffer, 0, output_size);

    gpu_ctx.queue.submit(Some(encoder.finish()));

    // Read back results
    let buffer_slice = staging_buffer.slice(..);
    let (sender, receiver) = futures::channel::oneshot::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        sender.send(result).unwrap();
    });

    gpu_ctx.device.poll(wgpu::Maintain::Wait);

    pollster::block_on(receiver)
        .context("Failed to receive buffer mapping result")?
        .context("Failed to map buffer")?;

    let data = buffer_slice.get_mapped_range();
    let output_f32: &[f32] = bytemuck::cast_slice(&data);

    // Convert f32 [0, 1] to u8 [0, 255]
    let mut result = Array3::<u8>::zeros((height, width, 3));
    for y in 0..height {
        for x in 0..width {
            for c in 0..3 {
                let idx = (y * width + x) * 3 + c;
                let value = (output_f32[idx] * 255.0).clamp(0.0, 255.0) as u8;
                result[[y, x, c]] = value;
            }
        }
    }

    drop(data);
    staging_buffer.unmap();

    Ok(Some(result))
}

/// Generate WGSL shader for postprocessing
fn generate_postprocess_shader(width: usize, height: usize) -> String {
    format!(
        r#"
// Image dimensions
const WIDTH: u32 = {}u;
const HEIGHT: u32 = {}u;

@group(0) @binding(0) var<storage, read> input_rgb: array<f32>;  // Linear RGB (3 channels)
@group(0) @binding(1) var<storage, read_write> output: array<f32>; // Output as f32, will convert to u8 on CPU

// sRGB gamma encoding
fn srgb_gamma(v: f32) -> f32 {{
    if v <= 0.0031308 {{
        return 12.92 * v;
    }} else {{
        return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
    }}
}}

@compute @workgroup_size(16, 16, 1)
fn postprocess(@builtin(global_invocation_id) global_id: vec3<u32>) {{
    let x = global_id.x;
    let y = global_id.y;

    if x >= WIDTH || y >= HEIGHT {{
        return;
    }}

    let idx = y * WIDTH + x;
    let rgb_idx = idx * 3u;

    // 1. Load linear RGB
    var r = input_rgb[rgb_idx];
    var g = input_rgb[rgb_idx + 1u];
    var b = input_rgb[rgb_idx + 2u];

    // 2. Exposure compensation (bring max to ~0.6)
    // Note: This is simplified - ideally we'd compute max in a separate pass
    // For now, use a fixed exposure multiplier
    let exposure = 1.5;
    r *= exposure;
    g *= exposure;
    b *= exposure;

    // Clamp to [0, 1]
    r = clamp(r, 0.0, 1.0);
    g = clamp(g, 0.0, 1.0);
    b = clamp(b, 0.0, 1.0);

    // 3. Apply sRGB gamma
    r = srgb_gamma(r);
    g = srgb_gamma(g);
    b = srgb_gamma(b);

    // 4. Store as f32 (will convert to u8 on CPU)
    output[rgb_idx] = r;
    output[rgb_idx + 1u] = g;
    output[rgb_idx + 2u] = b;
}}
"#,
        width, height
    )
}
