//! GPU-accelerated pixel collapse operations
//!
//! Implements weighted pixel collapse on GPU for hierarchical grading.

use super::gpu_context::GpuContext;
use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use ndarray::Array2;
use std::sync::Arc;

/// Collapse pixels on GPU using weighted average
///
/// Returns None if GPU is unavailable, caller should fall back to CPU
pub fn gpu_collapse_pixels(
    gpu_ctx: &Arc<GpuContext>,
    pixel_coords: &[(usize, usize)],
    image_weights: &[f64],
    images: &ndarray::Array3<f64>,
) -> Result<Option<Array2<f64>>> {
    if pixel_coords.is_empty() {
        let (height, width, _) = images.dim();
        return Ok(Some(Array2::zeros((height, width))));
    }

    let (height, width, num_images) = images.dim();
    if image_weights.len() != num_images {
        anyhow::bail!(
            "Image weights length {} does not match number of images {}",
            image_weights.len(),
            num_images
        );
    }

    let total_pixels = height
        .checked_mul(width)
        .context("Image dimensions overflow")?;
    let num_pixels = pixel_coords.len();

    let images_f32 = flatten_images_n_major(images)?;
    let weights_f32: Vec<f32> = image_weights.iter().map(|&w| w as f32).collect();
    let coords_u32: Vec<u32> = pixel_coords
        .iter()
        .flat_map(|&(y, x)| vec![y as u32, x as u32])
        .collect();

    let input_buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Collapse Images"),
        size: (images_f32.len() * std::mem::size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let weights_buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Collapse Weights"),
        size: (weights_f32.len() * std::mem::size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let coords_buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Collapse Coords"),
        size: (coords_u32.len() * std::mem::size_of::<u32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let output_buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Collapse Output"),
        size: (num_pixels * std::mem::size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let readback_buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Collapse Readback"),
        size: (num_pixels * std::mem::size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let params = CollapseParams {
        width: width as u32,
        height: height as u32,
        num_images: num_images as u32,
        num_pixels: num_pixels as u32,
    };

    let params_buffer = gpu_ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Collapse Params"),
        size: std::mem::size_of::<CollapseParams>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    gpu_ctx.queue.write_buffer(&input_buffer, 0, bytemuck::cast_slice(&images_f32));
    gpu_ctx.queue.write_buffer(&weights_buffer, 0, bytemuck::cast_slice(&weights_f32));
    gpu_ctx.queue.write_buffer(&coords_buffer, 0, bytemuck::cast_slice(&coords_u32));
    gpu_ctx.queue.write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

    let shader = gpu_ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Collapse Shader"),
        source: wgpu::ShaderSource::Wgsl(COLLAPSE_SHADER.into()),
    });

    let bind_group_layout = gpu_ctx.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Collapse Bind Group Layout"),
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
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 4,
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

    let pipeline_layout = gpu_ctx.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Collapse Pipeline Layout"),
        bind_group_layouts: &[&bind_group_layout],
        push_constant_ranges: &[],
    });

    let pipeline = gpu_ctx.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("Collapse Pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: "collapse",
    });

    let bind_group = gpu_ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Collapse Bind Group"),
        layout: &bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: weights_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: coords_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: output_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: params_buffer.as_entire_binding(),
            },
        ],
    });

    let mut encoder = gpu_ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Collapse Encoder"),
    });

    let workgroup_size = WORKGROUP_SIZE as u32;
    let dispatch_x = ((num_pixels as u32) + workgroup_size - 1) / workgroup_size;

    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Collapse Pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(dispatch_x, 1, 1);
    }

    encoder.copy_buffer_to_buffer(
        &output_buffer,
        0,
        &readback_buffer,
        0,
        (num_pixels * std::mem::size_of::<f32>()) as u64,
    );

    gpu_ctx.queue.submit(Some(encoder.finish()));

    let buffer_slice = readback_buffer.slice(..);
    let (sender, receiver) = futures::channel::oneshot::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        sender.send(result).ok();
    });

    gpu_ctx.device.poll(wgpu::Maintain::Wait);
    pollster::block_on(receiver)
        .context("Failed to receive collapse buffer mapping result")?
        .context("Failed to map collapse buffer")?;

    let data = buffer_slice.get_mapped_range();
    let collapsed: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
    drop(data);
    readback_buffer.unmap();

    let mut result = Array2::<f64>::zeros((height, width));
    for (idx, &(y, x)) in pixel_coords.iter().enumerate() {
        if y < height && x < width {
            result[[y, x]] = collapsed[idx] as f64;
        }
    }

    Ok(Some(result))
}

const WORKGROUP_SIZE: usize = 256;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CollapseParams {
    width: u32,
    height: u32,
    num_images: u32,
    num_pixels: u32,
}

fn flatten_images_n_major(images: &ndarray::Array3<f64>) -> Result<Vec<f32>> {
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

const COLLAPSE_SHADER: &str = r#"
struct Params {
    width: u32,
    height: u32,
    num_images: u32,
    num_pixels: u32,
}

@group(0) @binding(0) var<storage, read> images: array<f32>;
@group(0) @binding(1) var<storage, read> weights: array<f32>;
@group(0) @binding(2) var<storage, read> coords: array<u32>;
@group(0) @binding(3) var<storage, read_write> out: array<f32>;
@group(0) @binding(4) var<uniform> params: Params;

@compute @workgroup_size(256)
fn collapse(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= params.num_pixels) {
        return;
    }

    let coord_idx = idx * 2u;
    let y = coords[coord_idx];
    let x = coords[coord_idx + 1u];
    if (x >= params.width || y >= params.height) {
        out[idx] = 0.0;
        return;
    }

    let base = y * params.width + x;
    let image_stride = params.width * params.height;
    var sum: f32 = 0.0;
    var wsum: f32 = 0.0;

    for (var n: u32 = 0u; n < params.num_images; n = n + 1u) {
        let image_idx = n * image_stride + base;
        let w = weights[n];
        sum += w * images[image_idx];
        wsum += w;
    }

    if (wsum > 1e-8) {
        out[idx] = sum / wsum;
    } else {
        out[idx] = 0.0;
    }
}
"#;
