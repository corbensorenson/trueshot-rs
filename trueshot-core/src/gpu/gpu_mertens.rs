//! GPU-accelerated Mertens weight computation
//!
//! Computes contrast, saturation, and exposedness weights in parallel on GPU.
//! This is highly parallel and benefits significantly from GPU acceleration.

use super::gpu_context::GpuContext;
use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use ndarray::Array3;
use std::sync::Arc;

/// Mertens quality weights (same as CPU version)
pub struct MertensWeights {
    pub contrast: Vec<f64>,
    pub saturation: Vec<f64>,
    pub exposedness: Vec<f64>,
}

/// Compute Mertens weights on GPU
///
/// Returns None if GPU is unavailable, caller should fall back to CPU
pub fn gpu_compute_mertens_weights(
    gpu_ctx: &Arc<GpuContext>,
    images: &Array3<f64>,
    _exposures: &[f64],
    exposure_sigma: f64,
) -> Result<Option<MertensWeights>> {
    let (height, width, num_images) = images.dim();
    if num_images == 0 {
        return Ok(Some(MertensWeights {
            contrast: Vec::new(),
            saturation: Vec::new(),
            exposedness: Vec::new(),
        }));
    }

    let images_f32 = flatten_images_n_major(images)?;
    let output_len = num_images * 3;

    let input_buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Mertens Images"),
        size: (images_f32.len() * std::mem::size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let output_buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Mertens Output"),
        size: (output_len * std::mem::size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let readback_buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Mertens Readback"),
        size: (output_len * std::mem::size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let params = MertensParams {
        width: width as u32,
        height: height as u32,
        num_images: num_images as u32,
        step: 4,
        sigma: exposure_sigma as f32,
        _pad: [0u32; 3],
    };

    let params_buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Mertens Params"),
        size: std::mem::size_of::<MertensParams>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    gpu_ctx
        .queue
        .write_buffer(&input_buffer, 0, bytemuck::cast_slice(&images_f32));
    gpu_ctx
        .queue
        .write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

    let shader = gpu_ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Mertens Shader"),
            source: wgpu::ShaderSource::Wgsl(MERTENS_SHADER.into()),
        });

    let bind_group_layout =
        gpu_ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Mertens Bind Group Layout"),
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
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

    let pipeline_layout = gpu_ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Mertens Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

    let pipeline = gpu_ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Mertens Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: "compute_mertens",
        });

    let bind_group = gpu_ctx
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Mertens Bind Group"),
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
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

    let mut encoder = gpu_ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Mertens Encoder"),
        });

    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Mertens Pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(num_images as u32, 1, 1);
    }

    encoder.copy_buffer_to_buffer(
        &output_buffer,
        0,
        &readback_buffer,
        0,
        (output_len * std::mem::size_of::<f32>()) as u64,
    );

    gpu_ctx.queue.submit(Some(encoder.finish()));

    let buffer_slice = readback_buffer.slice(..);
    let (sender, receiver) = futures::channel::oneshot::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        sender.send(result).ok();
    });

    gpu_ctx.device.poll(wgpu::Maintain::Wait);
    pollster::block_on(receiver)
        .context("Failed to receive Mertens buffer mapping result")?
        .context("Failed to map Mertens buffer")?;

    let data = buffer_slice.get_mapped_range();
    let out_data: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
    drop(data);
    readback_buffer.unmap();

    let mut contrast = Vec::with_capacity(num_images);
    let mut saturation = Vec::with_capacity(num_images);
    let mut exposedness = Vec::with_capacity(num_images);

    for idx in 0..num_images {
        let base = idx * 3;
        contrast.push(out_data[base] as f64);
        saturation.push(out_data[base + 1] as f64);
        exposedness.push(out_data[base + 2] as f64);
    }

    Ok(Some(MertensWeights {
        contrast,
        saturation,
        exposedness,
    }))
}

// WGSL shader for Mertens weight computation (currently unused - GPU implementation not active)

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MertensParams {
    width: u32,
    height: u32,
    num_images: u32,
    step: u32,
    sigma: f32,
    _pad: [u32; 3],
}

fn flatten_images_n_major(images: &Array3<f64>) -> Result<Vec<f32>> {
    let (height, width, num_images) = images.dim();
    let mut data = Vec::with_capacity(height * width * num_images);
    for n in 0..num_images {
        for y in 0..height {
            for x in 0..width {
                data.push(images[[y, x, n]] as f32);
            }
        }
    }
    Ok(data)
}

const MERTENS_SHADER: &str = r#"
struct Params {
    width: u32,
    height: u32,
    num_images: u32,
    step: u32,
    sigma: f32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<storage, read> images: array<f32>;
@group(0) @binding(1) var<storage, read_write> out: array<f32>;
@group(0) @binding(2) var<uniform> params: Params;

var<workgroup> lap_sum_shared: array<f32, 256>;
var<workgroup> lap_count_shared: array<u32, 256>;
var<workgroup> intensity_sum_shared: array<f32, 256>;
var<workgroup> intensity_count_shared: array<u32, 256>;

@compute @workgroup_size(256)
fn compute_mertens(
    @builtin(workgroup_id) workgroup_id: vec3<u32>,
    @builtin(local_invocation_id) local_id: vec3<u32>
) {
    let image_idx = workgroup_id.x;
    if (image_idx >= params.num_images) {
        return;
    }

    let width = params.width;
    let height = params.height;
    let total = width * height;
    let step = params.step;
    let tid = local_id.x;
    let stride = 256u;

    var lap_sum: f32 = 0.0;
    var lap_count: u32 = 0u;
    var intensity_sum: f32 = 0.0;
    var intensity_count: u32 = 0u;

    var idx = tid;
    loop {
        if (idx >= total) { break; }
        let x = idx % width;
        let y = idx / width;
        let base = image_idx * total + idx;
        let center = images[base];

        if (x >= step && x + step < width && y >= step && y + step < height) {
            let left = images[image_idx * total + y * width + (x - step)];
            let right = images[image_idx * total + y * width + (x + step)];
            let up = images[image_idx * total + (y - step) * width + x];
            let down = images[image_idx * total + (y + step) * width + x];
            let lap = abs(4.0 * center - left - right - up - down);
            lap_sum += lap;
            lap_count += 1u;
        }

        if ((x % step) == 0u && (y % step) == 0u) {
            intensity_sum += center;
            intensity_count += 1u;
        }

        idx += stride;
    }

    lap_sum_shared[tid] = lap_sum;
    lap_count_shared[tid] = lap_count;
    intensity_sum_shared[tid] = intensity_sum;
    intensity_count_shared[tid] = intensity_count;
    workgroupBarrier();

    var offset = 128u;
    loop {
        if (tid < offset) {
            lap_sum_shared[tid] += lap_sum_shared[tid + offset];
            lap_count_shared[tid] += lap_count_shared[tid + offset];
            intensity_sum_shared[tid] += intensity_sum_shared[tid + offset];
            intensity_count_shared[tid] += intensity_count_shared[tid + offset];
        }
        workgroupBarrier();
        if (offset == 1u) { break; }
        offset = offset / 2u;
    }

    if (tid == 0u) {
        let lap_count_total = max(lap_count_shared[0], 1u);
        let contrast = lap_sum_shared[0] / f32(lap_count_total);

        let intensity_count_total = max(intensity_count_shared[0], 1u);
        let mean_intensity = intensity_sum_shared[0] / f32(intensity_count_total);

        let sigma = max(params.sigma, 1e-3);
        let exposedness = exp(-(mean_intensity - 0.5) * (mean_intensity - 0.5) / (2.0 * sigma * sigma));

        let base = image_idx * 3u;
        out[base] = contrast;
        out[base + 1u] = 1.0;
        out[base + 2u] = exposedness;
    }
}
"#;
