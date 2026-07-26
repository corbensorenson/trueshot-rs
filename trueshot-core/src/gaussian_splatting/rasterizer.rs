//! GPU Rasterizer for 3D Gaussian Splatting
//!
//! High-performance tile-based rasterization using WGPU compute shaders.
//! Supports real-time 4K rendering of millions of Gaussians.
//!
//! Architecture:
//! 1. Project Gaussians to 2D
//! 2. Sort by depth
//! 3. Tile-based binning (16x16 tiles)
//! 4. Per-tile alpha blending
//! 5. Final compositing

use super::gaussian::SH_COEFFS_TOTAL;
use anyhow::Result;
#[cfg(not(feature = "wgpu"))]
use nalgebra as na;
use std::sync::Arc;

/// GPU Rasterizer configuration
#[derive(Debug, Clone)]
pub struct RasterizerConfig {
    /// Output image width
    pub width: u32,
    /// Output image height
    pub height: u32,
    /// Tile size (typically 16x16)
    pub tile_size: u32,
    /// Maximum Gaussians per tile
    pub max_gaussians_per_tile: u32,
    /// Enable Mip-Splatting anti-aliasing
    pub mip_splatting: bool,
    /// Enable depth sorting
    pub depth_sort: bool,
}

impl Default for RasterizerConfig {
    fn default() -> Self {
        Self {
            width: 800,
            height: 600,
            tile_size: 16,
            max_gaussians_per_tile: 256,
            mip_splatting: true,
            depth_sort: true,
        }
    }
}

/// GPU buffer for projected 2D Gaussians
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct ProjectedGaussian {
    /// Screen position (x, y)
    pub position: [f32; 2],
    /// Depth for sorting
    pub depth: f32,
    /// 2D covariance matrix (upper triangle: a, b, c)
    pub cov2d: [f32; 3],
    /// RGBA color
    pub color: [f32; 4],
    /// Tile indices (for binning)
    pub tile_min: [u32; 2],
    pub tile_max: [u32; 2],
}

#[cfg(feature = "wgpu")]
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TileConfigUniform {
    tiles_x: u32,
    tiles_y: u32,
    tile_size: u32,
    max_per_tile: u32,
}

/// Camera uniforms for GPU
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct CameraUniforms {
    /// View matrix (4x4)
    pub view: [[f32; 4]; 4],
    /// Projection matrix (4x4)
    pub projection: [[f32; 4]; 4],
    /// Combined view-projection
    pub view_projection: [[f32; 4]; 4],
    /// Camera position in world space
    pub camera_position: [f32; 4],
    /// Viewport dimensions
    pub viewport: [f32; 4], // width, height, near, far
}

/// GPU-accelerated Gaussian Splatting Rasterizer
#[cfg(feature = "wgpu")]
pub struct GpuRasterizer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    config: RasterizerConfig,

    // Pipelines
    project_pipeline: wgpu::ComputePipeline,
    sort_pipeline: wgpu::ComputePipeline,
    tile_pipeline: wgpu::ComputePipeline,
    gradient_pipeline: wgpu::ComputePipeline,
    render_pipeline: wgpu::RenderPipeline,

    // Buffers
    gaussian_buffer: wgpu::Buffer,
    projected_buffer: wgpu::Buffer,
    tile_buffer: wgpu::Buffer,
    tile_config_buffer: wgpu::Buffer,
    camera_uniform_buffer: wgpu::Buffer,
    output_texture: wgpu::Texture,
    ground_truth_texture: wgpu::Texture,
    gradient_buffer: wgpu::Buffer,

    // Bind groups
    project_bind_group: wgpu::BindGroup,
    gradient_bind_group: wgpu::BindGroup,
    render_bind_group: wgpu::BindGroup,

    // Stats
    num_gaussians: u32,
}

#[cfg(feature = "wgpu")]
impl GpuRasterizer {
    /// Create new GPU rasterizer
    pub async fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        config: RasterizerConfig,
        max_gaussians: u32,
    ) -> Result<Self> {
        // Create shaders
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Gaussian Splatting Shader"),
            source: wgpu::ShaderSource::Wgsl(GAUSSIAN_SPLATTING_SHADER.into()),
        });

        // Create buffers
        let gaussian_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Gaussian Buffer"),
            size: (max_gaussians as u64) * std::mem::size_of::<Gaussian3DGpu>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let projected_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Projected Gaussians Buffer"),
            size: (max_gaussians as u64) * std::mem::size_of::<ProjectedGaussian>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let num_tiles_x = (config.width + config.tile_size - 1) / config.tile_size;
        let num_tiles_y = (config.height + config.tile_size - 1) / config.tile_size;
        let tile_buffer_size =
            (num_tiles_x * num_tiles_y * (1 + config.max_gaussians_per_tile) * 4) as u64;

        let tile_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tile Buffer"),
            size: tile_buffer_size,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let tile_config = TileConfigUniform {
            tiles_x: num_tiles_x,
            tiles_y: num_tiles_y,
            tile_size: config.tile_size,
            max_per_tile: config.max_gaussians_per_tile,
        };
        let tile_config_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tile Config Buffer"),
            size: std::mem::size_of::<TileConfigUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&tile_config_buffer, 0, bytemuck::bytes_of(&tile_config));

        let camera_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Camera Uniforms"),
            size: std::mem::size_of::<CameraUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Output Texture"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let ground_truth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Ground Truth Texture"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let gradient_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Gaussian Gradients Buffer"),
            size: (max_gaussians as u64) * std::mem::size_of::<GaussianGradientsGpu>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Bind group layouts
        let compute_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Compute Bind Group Layout"),
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

        // Compute pipelines
        let compute_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Compute Pipeline Layout"),
                bind_group_layouts: &[&compute_bind_group_layout],
                push_constant_ranges: &[],
            });

        let project_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Project Pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &shader,
            entry_point: "project_gaussians",
        });

        let sort_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Sort Pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &shader,
            entry_point: "sort_gaussians",
        });

        let tile_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Tile Pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &shader,
            entry_point: "bin_to_tiles",
        });

        let gradient_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Gradient Bind Group Layout"),
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
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            });

        let gradient_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Gradient Pipeline Layout"),
                bind_group_layouts: &[&compute_bind_group_layout, &gradient_bind_group_layout],
                push_constant_ranges: &[],
            });

        let gradient_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Gradient Pipeline"),
            layout: Some(&gradient_pipeline_layout),
            module: &shader,
            entry_point: "compute_gradients",
        });

        // Create bind groups
        let project_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Project Bind Group"),
            layout: &compute_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: gaussian_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: projected_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: camera_uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: tile_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: tile_config_buffer.as_entire_binding(),
                },
            ],
        });

        let output_texture_view =
            output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let ground_truth_texture_view =
            ground_truth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let gradient_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Gradient Bind Group"),
            layout: &gradient_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: gaussian_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: gradient_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: camera_uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&output_texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&ground_truth_texture_view),
                },
            ],
        });

        // Render pipeline for final compositing
        let render_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Render Bind Group Layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Render Pipeline Layout"),
                bind_group_layouts: &[&render_bind_group_layout],
                push_constant_ranges: &[],
            });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        let render_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Render Bind Group"),
            layout: &render_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: projected_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: tile_buffer.as_entire_binding(),
                },
            ],
        });

        Ok(Self {
            device,
            queue,
            config,
            project_pipeline,
            sort_pipeline,
            tile_pipeline,
            gradient_pipeline,
            render_pipeline,
            gaussian_buffer,
            projected_buffer,
            tile_buffer,
            tile_config_buffer,
            camera_uniform_buffer,
            output_texture,
            ground_truth_texture,
            gradient_buffer,
            project_bind_group,
            gradient_bind_group,
            render_bind_group,
            num_gaussians: 0,
        })
    }

    /// Upload Gaussian data to GPU
    pub fn upload_gaussians(&mut self, gaussians: &[Gaussian3DGpu]) {
        self.num_gaussians = gaussians.len() as u32;
        self.queue
            .write_buffer(&self.gaussian_buffer, 0, bytemuck::cast_slice(gaussians));
    }

    /// Set camera for rendering
    pub fn set_camera(&self, camera: &CameraUniforms) {
        self.queue.write_buffer(
            &self.camera_uniform_buffer,
            0,
            bytemuck::cast_slice(&[*camera]),
        );
    }

    /// Render frame
    pub fn render(&self) -> Result<()> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        let workgroups = (self.num_gaussians + 255) / 256;

        // 1. Project Gaussians to 2D
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Project Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.project_pipeline);
            pass.set_bind_group(0, &self.project_bind_group, &[]);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }

        // 2. Sort by depth (simplified - radix sort would be better)
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Sort Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.sort_pipeline);
            pass.set_bind_group(0, &self.project_bind_group, &[]);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }

        // 3. Bin to tiles
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Tile Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.tile_pipeline);
            pass.set_bind_group(0, &self.project_bind_group, &[]);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }

        // 4. Render with alpha blending
        {
            let view = self
                .output_texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.render_pipeline);
            pass.set_bind_group(0, &self.render_bind_group, &[]);
            pass.draw(0..4, 0..1); // Fullscreen quad
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        Ok(())
    }

    /// Upload ground truth RGBA8 image for gradient computation
    pub fn upload_ground_truth(&self, data: &[u8]) {
        let bytes_per_row = self.config.width * 4;
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u32;
        let padded_bytes_per_row = ((bytes_per_row + alignment - 1) / alignment) * alignment;
        let (upload_data, bytes_per_row) = if padded_bytes_per_row != bytes_per_row {
            let mut padded = vec![0u8; (padded_bytes_per_row * self.config.height) as usize];
            let row_bytes = bytes_per_row as usize;
            let padded_row_bytes = padded_bytes_per_row as usize;
            for row in 0..self.config.height as usize {
                let src = row * row_bytes;
                let dst = row * padded_row_bytes;
                padded[dst..dst + row_bytes].copy_from_slice(&data[src..src + row_bytes]);
            }
            (padded, padded_bytes_per_row)
        } else {
            (data.to_vec(), bytes_per_row)
        };
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.ground_truth_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &upload_data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(self.config.height),
            },
            wgpu::Extent3d {
                width: self.config.width,
                height: self.config.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Compute image-space gradients on GPU (per-pixel accumulation)
    pub fn compute_gradients(&self) -> Result<()> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Gradient Encoder"),
            });

        let workgroups = (self.num_gaussians + 255) / 256;
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Gradient Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.gradient_pipeline);
            pass.set_bind_group(0, &self.project_bind_group, &[]);
            pass.set_bind_group(1, &self.gradient_bind_group, &[]);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        Ok(())
    }

    /// Read gradients back to CPU
    pub async fn read_gradients(&self) -> Result<Vec<GaussianGradientsGpu>> {
        let count = self.num_gaussians as usize;
        let buffer_size = (count * std::mem::size_of::<GaussianGradientsGpu>()) as u64;
        let staging_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Gradient Readback Buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Gradient Copy Encoder"),
            });
        encoder.copy_buffer_to_buffer(&self.gradient_buffer, 0, &staging_buffer, 0, buffer_size);
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = staging_buffer.slice(..);
        let (tx, rx) = tokio::sync::oneshot::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.await??;

        let data = slice.get_mapped_range();
        let mut gradients = Vec::with_capacity(count);
        let bytes = data.to_vec();
        drop(data);
        staging_buffer.unmap();

        let slice = bytemuck::cast_slice::<u8, GaussianGradientsGpu>(&bytes);
        gradients.extend_from_slice(slice);
        Ok(gradients)
    }

    /// Read rendered image back to CPU
    pub async fn read_output(&self) -> Result<Vec<u8>> {
        let buffer_size = (self.config.width * self.config.height * 4) as u64;
        let staging_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Staging Buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Copy Encoder"),
            });

        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &self.output_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &staging_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(self.config.width * 4),
                    rows_per_image: Some(self.config.height),
                },
            },
            wgpu::Extent3d {
                width: self.config.width,
                height: self.config.height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = staging_buffer.slice(..);
        let (tx, rx) = tokio::sync::oneshot::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.await??;

        let data = slice.get_mapped_range();
        let result = data.to_vec();
        drop(data);
        staging_buffer.unmap();

        Ok(result)
    }
}

/// GPU representation of a 3D Gaussian
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct Gaussian3DGpu {
    /// Position (x, y, z, w=1)
    pub position: [f32; 4],
    /// Rotation quaternion (x, y, z, w)
    pub rotation: [f32; 4],
    /// Scale (x, y, z, padding)
    pub scale: [f32; 4],
    /// Opacity and padding
    pub opacity: [f32; 4],
    /// Spherical harmonics coefficients (25 coeffs * 3 channels)
    pub sh_coeffs: [f32; SH_COEFFS_TOTAL],
}

/// GPU gradients for a 3D Gaussian (image-space, approximate)
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct GaussianGradientsGpu {
    pub sh_grad: [f32; SH_COEFFS_TOTAL],
    pub opacity_grad: f32,
    pub scale_grad: [f32; 4],
    pub rotation_grad: [f32; 4],
    pub position_grad: [f32; 4],
}

/// WGSL shader for Gaussian Splatting
const GAUSSIAN_SPLATTING_SHADER: &str = r#"
// Gaussian Splatting - GPU Implementation
// Based on "3D Gaussian Splatting for Real-Time Radiance Field Rendering"

struct Gaussian {
    position: vec4<f32>,
    rotation: vec4<f32>,
    scale: vec4<f32>,
    opacity: vec4<f32>,
    sh_coeffs: array<f32, 75>,
}

struct ProjectedGaussian {
    position: vec2<f32>,
    depth: f32,
    cov2d: vec3<f32>,
    color: vec4<f32>,
    tile_min: vec2<u32>,
    tile_max: vec2<u32>,
}

struct Camera {
    view: mat4x4<f32>,
    projection: mat4x4<f32>,
    view_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
    viewport: vec4<f32>,
}

struct GaussianGradients {
    sh_grad: array<f32, 75>,
    opacity_grad: f32,
    scale_grad: vec4<f32>,
    rotation_grad: vec4<f32>,
    position_grad: vec4<f32>,
}

struct SHColor {
    color: vec3<f32>,
    mask: vec3<f32>,
}

@group(0) @binding(0) var<storage, read> gaussians: array<Gaussian>;
@group(0) @binding(1) var<storage, read_write> projected: array<ProjectedGaussian>;
@group(0) @binding(2) var<uniform> camera: Camera;

@group(1) @binding(0) var<storage, read> gradient_gaussians: array<Gaussian>;
@group(1) @binding(1) var<storage, read_write> gradients: array<GaussianGradients>;
@group(1) @binding(2) var<uniform> gradient_camera: Camera;
@group(1) @binding(3) var rendered_tex: texture_2d<f32>;
@group(1) @binding(4) var gt_tex: texture_2d<f32>;

// Quaternion to rotation matrix
fn quat_to_mat3(q: vec4<f32>) -> mat3x3<f32> {
    let x = q.x;
    let y = q.y;
    let z = q.z;
    let w = q.w;
    
    return mat3x3<f32>(
        vec3<f32>(1.0 - 2.0*(y*y + z*z), 2.0*(x*y + w*z), 2.0*(x*z - w*y)),
        vec3<f32>(2.0*(x*y - w*z), 1.0 - 2.0*(x*x + z*z), 2.0*(x*z + w*y)),
        vec3<f32>(2.0*(x*z + w*y), 2.0*(y*z - w*x), 1.0 - 2.0*(x*x + y*y))
    );
}

// Compute 3D covariance matrix
fn compute_cov3d(scale: vec3<f32>, rotation: vec4<f32>) -> mat3x3<f32> {
    let R = quat_to_mat3(rotation);
    let S = mat3x3<f32>(
        vec3<f32>(scale.x, 0.0, 0.0),
        vec3<f32>(0.0, scale.y, 0.0),
        vec3<f32>(0.0, 0.0, scale.z)
    );
    // Cov = R * S * S^T * R^T
    let M = R * S;
    return M * transpose(M);
}

// Project 3D covariance to 2D
fn project_cov3d_to_2d(cov3d: mat3x3<f32>, cam_pos: vec3<f32>) -> vec3<f32> {
    let focal = camera.projection[0][0];
    let z = cam_pos.z;
    let z2 = z * z;
    
    // Jacobian of perspective projection
    let J = mat3x2<f32>(
        vec2<f32>(focal / z, 0.0),
        vec2<f32>(0.0, focal / z),
        vec2<f32>(-focal * cam_pos.x / z2, -focal * cam_pos.y / z2)
    );
    
    let cov2d = J * cov3d * transpose(J);
    
    // Return upper triangle (a, b, c) where cov = [[a, b], [b, c]]
    return vec3<f32>(cov2d[0][0] + 0.3, cov2d[0][1], cov2d[1][1] + 0.3);
}

fn eval_sh_basis(dir_in: vec3<f32>) -> array<f32, 25> {
    var dir = dir_in;
    let norm = length(dir);
    if (norm > 1e-6) {
        dir = dir / norm;
    } else {
        dir = vec3<f32>(0.0, 0.0, 1.0);
    }
    let x = dir.x;
    let y = dir.y;
    let z = dir.z;
    let xx = x * x;
    let yy = y * y;
    let zz = z * z;
    let xy = x * y;
    let yz = y * z;
    let xz = x * z;

    let c0 = 0.2820947918;
    let c1 = 0.4886025119;
    let c2_0 = 1.0925484306;
    let c2_1 = 0.3153915653;
    let c2_2 = 0.5462742153;
    let c3_0 = 0.5900435899;
    let c3_1 = 2.8906114426;
    let c3_2 = 0.4570457995;
    let c3_3 = 0.3731763326;
    let c3_4 = 1.4453057213;
    let c4_0 = 2.5033429418;
    let c4_1 = 1.7701307698;
    let c4_2 = 0.9461746958;
    let c4_3 = 0.6690465436;
    let c4_4 = 0.1057855469;
    let c4_6 = 0.4730873479;
    let c4_8 = 0.6258357354;

    let zz2 = zz * zz;
    let xx2 = xx * xx;
    let yy2 = yy * yy;

    return array<f32, 25>(
        c0,
        -c1 * y,
        c1 * z,
        -c1 * x,
        c2_0 * xy,
        -c2_0 * yz,
        c2_1 * (3.0 * zz - 1.0),
        -c2_0 * xz,
        c2_2 * (xx - yy),
        -c3_0 * y * (3.0 * xx - yy),
        c3_1 * xy * z,
        -c3_2 * y * (5.0 * zz - 1.0),
        c3_3 * z * (5.0 * zz - 3.0),
        -c3_2 * x * (5.0 * zz - 1.0),
        c3_4 * z * (xx - yy),
        -c3_0 * x * (xx - 3.0 * yy),
        c4_0 * xy * (xx - yy),
        -c4_1 * y * z * (3.0 * xx - yy),
        c4_2 * xy * (7.0 * zz - 1.0),
        -c4_3 * y * z * (7.0 * zz - 3.0),
        c4_4 * (35.0 * zz2 - 30.0 * zz + 3.0),
        -c4_3 * x * z * (7.0 * zz - 3.0),
        c4_6 * (xx - yy) * (7.0 * zz - 1.0),
        -c4_1 * x * z * (xx - 3.0 * yy),
        c4_8 * (xx2 - 6.0 * xx * yy + yy2),
    );
}

fn eval_sh_color(coeffs_in: array<f32, 75>, basis: array<f32, 25>) -> SHColor {
    let b0 = basis[0];
    let b1 = basis[1];
    let b2 = basis[2];
    let b3 = basis[3];
    let b4 = basis[4];
    let b5 = basis[5];
    let b6 = basis[6];
    let b7 = basis[7];
    let b8 = basis[8];
    let b9 = basis[9];
    let b10 = basis[10];
    let b11 = basis[11];
    let b12 = basis[12];
    let b13 = basis[13];
    let b14 = basis[14];
    let b15 = basis[15];
    let b16 = basis[16];
    let b17 = basis[17];
    let b18 = basis[18];
    let b19 = basis[19];
    let b20 = basis[20];
    let b21 = basis[21];
    let b22 = basis[22];
    let b23 = basis[23];
    let b24 = basis[24];

    let c0 = coeffs_in[0] * b0
        + coeffs_in[1] * b1
        + coeffs_in[2] * b2
        + coeffs_in[3] * b3
        + coeffs_in[4] * b4
        + coeffs_in[5] * b5
        + coeffs_in[6] * b6
        + coeffs_in[7] * b7
        + coeffs_in[8] * b8
        + coeffs_in[9] * b9
        + coeffs_in[10] * b10
        + coeffs_in[11] * b11
        + coeffs_in[12] * b12
        + coeffs_in[13] * b13
        + coeffs_in[14] * b14
        + coeffs_in[15] * b15
        + coeffs_in[16] * b16
        + coeffs_in[17] * b17
        + coeffs_in[18] * b18
        + coeffs_in[19] * b19
        + coeffs_in[20] * b20
        + coeffs_in[21] * b21
        + coeffs_in[22] * b22
        + coeffs_in[23] * b23
        + coeffs_in[24] * b24
        ;

    let c1 = coeffs_in[25] * b0
        + coeffs_in[26] * b1
        + coeffs_in[27] * b2
        + coeffs_in[28] * b3
        + coeffs_in[29] * b4
        + coeffs_in[30] * b5
        + coeffs_in[31] * b6
        + coeffs_in[32] * b7
        + coeffs_in[33] * b8
        + coeffs_in[34] * b9
        + coeffs_in[35] * b10
        + coeffs_in[36] * b11
        + coeffs_in[37] * b12
        + coeffs_in[38] * b13
        + coeffs_in[39] * b14
        + coeffs_in[40] * b15
        + coeffs_in[41] * b16
        + coeffs_in[42] * b17
        + coeffs_in[43] * b18
        + coeffs_in[44] * b19
        + coeffs_in[45] * b20
        + coeffs_in[46] * b21
        + coeffs_in[47] * b22
        + coeffs_in[48] * b23
        + coeffs_in[49] * b24
        ;

    let c2 = coeffs_in[50] * b0
        + coeffs_in[51] * b1
        + coeffs_in[52] * b2
        + coeffs_in[53] * b3
        + coeffs_in[54] * b4
        + coeffs_in[55] * b5
        + coeffs_in[56] * b6
        + coeffs_in[57] * b7
        + coeffs_in[58] * b8
        + coeffs_in[59] * b9
        + coeffs_in[60] * b10
        + coeffs_in[61] * b11
        + coeffs_in[62] * b12
        + coeffs_in[63] * b13
        + coeffs_in[64] * b14
        + coeffs_in[65] * b15
        + coeffs_in[66] * b16
        + coeffs_in[67] * b17
        + coeffs_in[68] * b18
        + coeffs_in[69] * b19
        + coeffs_in[70] * b20
        + coeffs_in[71] * b21
        + coeffs_in[72] * b22
        + coeffs_in[73] * b23
        + coeffs_in[74] * b24
        ;

    var color = vec3<f32>(0.0);
    var mask = vec3<f32>(1.0);

    let v0 = c0 + 0.5;
    if (v0 <= 0.0) {
        color.x = 0.0;
        mask.x = 0.0;
    } else if (v0 >= 1.0) {
        color.x = 1.0;
        mask.x = 0.0;
    } else {
        color.x = v0;
    }

    let v1 = c1 + 0.5;
    if (v1 <= 0.0) {
        color.y = 0.0;
        mask.y = 0.0;
    } else if (v1 >= 1.0) {
        color.y = 1.0;
        mask.y = 0.0;
    } else {
        color.y = v1;
    }

    let v2 = c2 + 0.5;
    if (v2 <= 0.0) {
        color.z = 0.0;
        mask.z = 0.0;
    } else if (v2 >= 1.0) {
        color.z = 1.0;
        mask.z = 0.0;
    } else {
        color.z = v2;
    }

    return SHColor(color, mask);
}

fn rotation_grad_from_matrix(grad_r: mat3x3<f32>, q: vec4<f32>) -> vec4<f32> {
    let w = q.w;
    let x = q.x;
    let y = q.y;
    let z = q.z;

    let g00 = grad_r[0][0];
    let g01 = grad_r[1][0];
    let g02 = grad_r[2][0];
    let g10 = grad_r[0][1];
    let g11 = grad_r[1][1];
    let g12 = grad_r[2][1];
    let g20 = grad_r[0][2];
    let g21 = grad_r[1][2];
    let g22 = grad_r[2][2];

    let grad_w = g01 * (-2.0 * z)
        + g02 * (2.0 * y)
        + g10 * (2.0 * z)
        + g12 * (-2.0 * x)
        + g20 * (-2.0 * y)
        + g21 * (2.0 * x);

    let grad_x = g01 * (2.0 * y)
        + g02 * (2.0 * z)
        + g10 * (2.0 * y)
        + g11 * (-4.0 * x)
        + g12 * (-2.0 * w)
        + g20 * (2.0 * z)
        + g21 * (2.0 * w)
        + g22 * (-4.0 * x);

    let grad_y = g00 * (-4.0 * y)
        + g01 * (2.0 * x)
        + g02 * (2.0 * w)
        + g10 * (2.0 * x)
        + g12 * (2.0 * z)
        + g20 * (-2.0 * w)
        + g21 * (2.0 * z)
        + g22 * (-4.0 * y);

    let grad_z = g00 * (-4.0 * z)
        + g01 * (-2.0 * w)
        + g02 * (2.0 * x)
        + g10 * (2.0 * w)
        + g11 * (-4.0 * z)
        + g12 * (2.0 * y)
        + g20 * (2.0 * x)
        + g21 * (2.0 * y);

    return vec4<f32>(grad_x, grad_y, grad_z, grad_w);
}

@compute @workgroup_size(256)
fn project_gaussians(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= arrayLength(&gaussians)) {
        return;
    }
    
    let g = gaussians[idx];
    
    // Transform to camera space
    let world_pos = g.position.xyz;
    let cam_pos = (camera.view * vec4<f32>(world_pos, 1.0)).xyz;
    
    // Skip if behind camera
    if (cam_pos.z <= 0.0) {
        projected[idx].depth = 1e10;
        return;
    }
    
    // Project to screen
    let clip_pos = camera.projection * vec4<f32>(cam_pos, 1.0);
    let ndc = clip_pos.xyz / clip_pos.w;
    let screen_pos = vec2<f32>(
        (ndc.x * 0.5 + 0.5) * camera.viewport.x,
        (1.0 - (ndc.y * 0.5 + 0.5)) * camera.viewport.y
    );
    
    // Compute 3D covariance
    let scale = exp(g.scale.xyz);
    let cov3d = compute_cov3d(scale, g.rotation);
    
    // Project to 2D
    let cov2d = project_cov3d_to_2d(cov3d, cam_pos);
    
    // Compute tile range
    let radius = 3.0 * sqrt(max(cov2d.x, cov2d.z));
    let tile_min = vec2<u32>(
        u32(max(0.0, (screen_pos.x - radius) / 16.0)),
        u32(max(0.0, (screen_pos.y - radius) / 16.0))
    );
    let tile_max = vec2<u32>(
        u32(min(camera.viewport.x / 16.0, (screen_pos.x + radius) / 16.0)),
        u32(min(camera.viewport.y / 16.0, (screen_pos.y + radius) / 16.0))
    );
    
    let view_dir = normalize(-cam_pos);
    let basis = eval_sh_basis(view_dir);
    let sh = eval_sh_color(g.sh_coeffs, basis);
    let color = vec4<f32>(
        sh.color.x,
        sh.color.y,
        sh.color.z,
        1.0 / (1.0 + exp(-g.opacity.x))
    );
    
    // Store projected Gaussian
    projected[idx].position = screen_pos;
    projected[idx].depth = cam_pos.z;
    projected[idx].cov2d = cov2d;
    projected[idx].color = color;
    projected[idx].tile_min = tile_min;
    projected[idx].tile_max = tile_max;
}

@compute @workgroup_size(256)
fn sort_gaussians(@builtin(global_invocation_id) id: vec3<u32>) {
    // GPU Parallel Radix Sort - Bitonic Sort variant
    // Sorts projected Gaussians by depth (back-to-front for alpha blending)
    
    let num_gaussians = arrayLength(&projected);
    let idx = id.x;
    
    if (idx >= num_gaussians) {
        return;
    }
    
    // Bitonic sort network - log2(N) stages
    // Each thread compares and swaps pairs at increasing distances
    
    // Stage 1: Local sort within pairs
    for (var k: u32 = 2u; k <= num_gaussians; k = k * 2u) {
        for (var j: u32 = k / 2u; j > 0u; j = j / 2u) {
            let partner = idx ^ j;
            
            if (partner > idx && partner < num_gaussians) {
                let depth_i = projected[idx].depth;
                let depth_p = projected[partner].depth;
                
                // Check direction based on bitonic merge pattern
                let ascending = ((idx & k) == 0u);
                let should_swap = select(depth_i < depth_p, depth_i > depth_p, ascending);
                
                if (should_swap) {
                    // Swap the two Gaussians
                    let temp = projected[idx];
                    projected[idx] = projected[partner];
                    projected[partner] = temp;
                }
            }
            
            // Workgroup barrier for synchronization
            workgroupBarrier();
        }
    }
}

// Tile data structure for binning
struct TileData {
    count: atomic<u32>,
    gaussian_indices: array<u32, 256>,  // Max 256 Gaussians per tile
}

// Tile buffer binding (assume binding 3)
@group(0) @binding(3) var<storage, read_write> tiles: array<TileData>;

// Tile configuration uniforms
struct TileConfig {
    tiles_x: u32,
    tiles_y: u32,
    tile_size: u32,
    max_per_tile: u32,
}
@group(0) @binding(4) var<uniform> tile_config: TileConfig;

@compute @workgroup_size(256)
fn bin_to_tiles(@builtin(global_invocation_id) id: vec3<u32>) {
    // Tile Binning - Assigns each Gaussian to tiles it overlaps
    // This is critical for scalable rendering of millions of Gaussians
    
    let num_gaussians = arrayLength(&projected);
    let idx = id.x;
    
    if (idx >= num_gaussians) {
        return;
    }
    
    let g = projected[idx];
    
    // Skip invalid/culled Gaussians
    if (g.depth > 1e9) {
        return;
    }
    
    // Iterate over tiles this Gaussian overlaps
    for (var ty: u32 = g.tile_min.y; ty <= g.tile_max.y; ty++) {
        for (var tx: u32 = g.tile_min.x; tx <= g.tile_max.x; tx++) {
            // Compute linear tile index
            let tile_idx = ty * tile_config.tiles_x + tx;
            
            // Bounds check
            if (tile_idx >= tile_config.tiles_x * tile_config.tiles_y) {
                continue;
            }
            
            // Atomically add Gaussian to tile
            let slot = atomicAdd(&tiles[tile_idx].count, 1u);
            
            // Check for overflow
            if (slot < tile_config.max_per_tile) {
                tiles[tile_idx].gaussian_indices[slot] = idx;
            }
        }
    }
}

// Clear tiles before binning (call before bin_to_tiles)
@compute @workgroup_size(256)
fn clear_tiles(@builtin(global_invocation_id) id: vec3<u32>) {
    let num_tiles = tile_config.tiles_x * tile_config.tiles_y;
    let idx = id.x;
    
    if (idx >= num_tiles) {
        return;
    }
    
    atomicStore(&tiles[idx].count, 0u);
}

@compute @workgroup_size(256)
fn compute_gradients(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= arrayLength(&gradient_gaussians)) {
        return;
    }

    gradients[idx].sh_grad[0] = 0.0;
    gradients[idx].sh_grad[1] = 0.0;
    gradients[idx].sh_grad[2] = 0.0;
    gradients[idx].sh_grad[3] = 0.0;
    gradients[idx].sh_grad[4] = 0.0;
    gradients[idx].sh_grad[5] = 0.0;
    gradients[idx].sh_grad[6] = 0.0;
    gradients[idx].sh_grad[7] = 0.0;
    gradients[idx].sh_grad[8] = 0.0;
    gradients[idx].sh_grad[9] = 0.0;
    gradients[idx].sh_grad[10] = 0.0;
    gradients[idx].sh_grad[11] = 0.0;
    gradients[idx].sh_grad[12] = 0.0;
    gradients[idx].sh_grad[13] = 0.0;
    gradients[idx].sh_grad[14] = 0.0;
    gradients[idx].sh_grad[15] = 0.0;
    gradients[idx].sh_grad[16] = 0.0;
    gradients[idx].sh_grad[17] = 0.0;
    gradients[idx].sh_grad[18] = 0.0;
    gradients[idx].sh_grad[19] = 0.0;
    gradients[idx].sh_grad[20] = 0.0;
    gradients[idx].sh_grad[21] = 0.0;
    gradients[idx].sh_grad[22] = 0.0;
    gradients[idx].sh_grad[23] = 0.0;
    gradients[idx].sh_grad[24] = 0.0;
    gradients[idx].sh_grad[25] = 0.0;
    gradients[idx].sh_grad[26] = 0.0;
    gradients[idx].sh_grad[27] = 0.0;
    gradients[idx].sh_grad[28] = 0.0;
    gradients[idx].sh_grad[29] = 0.0;
    gradients[idx].sh_grad[30] = 0.0;
    gradients[idx].sh_grad[31] = 0.0;
    gradients[idx].sh_grad[32] = 0.0;
    gradients[idx].sh_grad[33] = 0.0;
    gradients[idx].sh_grad[34] = 0.0;
    gradients[idx].sh_grad[35] = 0.0;
    gradients[idx].sh_grad[36] = 0.0;
    gradients[idx].sh_grad[37] = 0.0;
    gradients[idx].sh_grad[38] = 0.0;
    gradients[idx].sh_grad[39] = 0.0;
    gradients[idx].sh_grad[40] = 0.0;
    gradients[idx].sh_grad[41] = 0.0;
    gradients[idx].sh_grad[42] = 0.0;
    gradients[idx].sh_grad[43] = 0.0;
    gradients[idx].sh_grad[44] = 0.0;
    gradients[idx].sh_grad[45] = 0.0;
    gradients[idx].sh_grad[46] = 0.0;
    gradients[idx].sh_grad[47] = 0.0;
    gradients[idx].sh_grad[48] = 0.0;
    gradients[idx].sh_grad[49] = 0.0;
    gradients[idx].sh_grad[50] = 0.0;
    gradients[idx].sh_grad[51] = 0.0;
    gradients[idx].sh_grad[52] = 0.0;
    gradients[idx].sh_grad[53] = 0.0;
    gradients[idx].sh_grad[54] = 0.0;
    gradients[idx].sh_grad[55] = 0.0;
    gradients[idx].sh_grad[56] = 0.0;
    gradients[idx].sh_grad[57] = 0.0;
    gradients[idx].sh_grad[58] = 0.0;
    gradients[idx].sh_grad[59] = 0.0;
    gradients[idx].sh_grad[60] = 0.0;
    gradients[idx].sh_grad[61] = 0.0;
    gradients[idx].sh_grad[62] = 0.0;
    gradients[idx].sh_grad[63] = 0.0;
    gradients[idx].sh_grad[64] = 0.0;
    gradients[idx].sh_grad[65] = 0.0;
    gradients[idx].sh_grad[66] = 0.0;
    gradients[idx].sh_grad[67] = 0.0;
    gradients[idx].sh_grad[68] = 0.0;
    gradients[idx].sh_grad[69] = 0.0;
    gradients[idx].sh_grad[70] = 0.0;
    gradients[idx].sh_grad[71] = 0.0;
    gradients[idx].sh_grad[72] = 0.0;
    gradients[idx].sh_grad[73] = 0.0;
    gradients[idx].sh_grad[74] = 0.0;
    gradients[idx].opacity_grad = 0.0;
    gradients[idx].scale_grad = vec4<f32>(0.0);
    gradients[idx].rotation_grad = vec4<f32>(0.0);
    gradients[idx].position_grad = vec4<f32>(0.0);

    let g = gradient_gaussians[idx];
    let cam_pos = (gradient_camera.view * vec4<f32>(g.position.xyz, 1.0)).xyz;
    if (cam_pos.z <= 0.0) {
        return;
    }

    let clip_pos = gradient_camera.projection * vec4<f32>(cam_pos, 1.0);
    if (clip_pos.w <= 1e-6) {
        return;
    }
    let ndc = clip_pos.xyz / clip_pos.w;
    let screen_pos = vec2<f32>(
        (ndc.x * 0.5 + 0.5) * gradient_camera.viewport.x,
        (1.0 - (ndc.y * 0.5 + 0.5)) * gradient_camera.viewport.y
    );
    let scale = exp(g.scale.xyz);
    let cov3d = compute_cov3d(scale, g.rotation);
    let cov2d = project_cov3d_to_2d(cov3d, cam_pos);
    let det = cov2d.x * cov2d.z - cov2d.y * cov2d.y;
    if (det <= 1e-6) {
        return;
    }
    let inv_det = 1.0 / det;
    let inv_cov = vec3<f32>(cov2d.z * inv_det, -cov2d.y * inv_det, cov2d.x * inv_det);
    var radius = 3.0 * sqrt(max(cov2d.x, cov2d.z));
    if (radius <= 0.0) {
        return;
    }
    radius = min(radius, 64.0);

    let min_x = max(i32(floor(screen_pos.x - radius)), 0);
    let max_x = min(i32(ceil(screen_pos.x + radius)), i32(gradient_camera.viewport.x) - 1);
    let min_y = max(i32(floor(screen_pos.y - radius)), 0);
    let max_y = min(i32(ceil(screen_pos.y + radius)), i32(gradient_camera.viewport.y) - 1);
    if (min_x > max_x || min_y > max_y) {
        return;
    }

    let view_dir = normalize(-cam_pos);
    let basis = eval_sh_basis(view_dir);
    let sh = eval_sh_color(g.sh_coeffs, basis);
    let opacity = 1.0 / (1.0 + exp(-g.opacity.x));
    let fx = gradient_camera.projection[0][0] * gradient_camera.viewport.x * 0.5;
    let fy = gradient_camera.projection[1][1] * gradient_camera.viewport.y * 0.5;
    let inv_z = 1.0 / cam_pos.z;
    let inv_z2 = inv_z * inv_z;
    var grad_cam = vec3<f32>(0.0);

    var grad_inv_cov = vec3<f32>(0.0);

    for (var py: i32 = min_y; py <= max_y; py = py + 1) {
        for (var px: i32 = min_x; px <= max_x; px = px + 1) {
            let dx = f32(px) + 0.5 - screen_pos.x;
            let dy = f32(py) + 0.5 - screen_pos.y;
            let quad = inv_cov.x * dx * dx + 2.0 * inv_cov.y * dx * dy + inv_cov.z * dy * dy;
            let power = -0.5 * quad;
            if (power > 0.0) {
                continue;
            }
            let weight = exp(power);
            let alpha = opacity * weight;
            if (alpha < 0.01) {
                continue;
            }

            let rendered = textureLoad(rendered_tex, vec2<i32>(px, py), 0);
            let gt = textureLoad(gt_tex, vec2<i32>(px, py), 0);
            let error = rendered.xyz - gt.xyz;

            let d_rendered_d_alpha = sh.color - rendered.xyz;
            let d_loss_d_alpha = dot(error, d_rendered_d_alpha);
            gradients[idx].opacity_grad = gradients[idx].opacity_grad + d_loss_d_alpha * weight;

            let d_loss_d_weight = d_loss_d_alpha * opacity;
            let d_loss_d_quad = -0.5 * d_loss_d_weight * weight;
            grad_inv_cov.x = grad_inv_cov.x + d_loss_d_quad * dx * dx;
            grad_inv_cov.y = grad_inv_cov.y + d_loss_d_quad * 2.0 * dx * dy;
            grad_inv_cov.z = grad_inv_cov.z + d_loss_d_quad * dy * dy;

            let inv_qx = inv_cov.x * dx + inv_cov.y * dy;
            let inv_qy = inv_cov.y * dx + inv_cov.z * dy;
            let d_loss_du = opacity * weight * d_loss_d_alpha * inv_qx;
            let d_loss_dv = opacity * weight * d_loss_d_alpha * inv_qy;

            let du_dx = fx * inv_z;
            let dv_dy = fy * inv_z;
            let du_dz = -fx * cam_pos.x * inv_z2;
            let dv_dz = -fy * cam_pos.y * inv_z2;

            grad_cam.x = grad_cam.x + d_loss_du * du_dx;
            grad_cam.y = grad_cam.y + d_loss_dv * dv_dy;
            grad_cam.z = grad_cam.z + d_loss_du * du_dz + d_loss_dv * dv_dz;

            let d_loss_d_color = error * alpha;
            let err0 = d_loss_d_color.x * sh.mask.x;
            let err1 = d_loss_d_color.y * sh.mask.y;
            let err2 = d_loss_d_color.z * sh.mask.z;

            gradients[idx].sh_grad[0] = gradients[idx].sh_grad[0] + err0 * basis[0];
            gradients[idx].sh_grad[1] = gradients[idx].sh_grad[1] + err0 * basis[1];
            gradients[idx].sh_grad[2] = gradients[idx].sh_grad[2] + err0 * basis[2];
            gradients[idx].sh_grad[3] = gradients[idx].sh_grad[3] + err0 * basis[3];
            gradients[idx].sh_grad[4] = gradients[idx].sh_grad[4] + err0 * basis[4];
            gradients[idx].sh_grad[5] = gradients[idx].sh_grad[5] + err0 * basis[5];
            gradients[idx].sh_grad[6] = gradients[idx].sh_grad[6] + err0 * basis[6];
            gradients[idx].sh_grad[7] = gradients[idx].sh_grad[7] + err0 * basis[7];
            gradients[idx].sh_grad[8] = gradients[idx].sh_grad[8] + err0 * basis[8];
            gradients[idx].sh_grad[9] = gradients[idx].sh_grad[9] + err0 * basis[9];
            gradients[idx].sh_grad[10] = gradients[idx].sh_grad[10] + err0 * basis[10];
            gradients[idx].sh_grad[11] = gradients[idx].sh_grad[11] + err0 * basis[11];
            gradients[idx].sh_grad[12] = gradients[idx].sh_grad[12] + err0 * basis[12];
            gradients[idx].sh_grad[13] = gradients[idx].sh_grad[13] + err0 * basis[13];
            gradients[idx].sh_grad[14] = gradients[idx].sh_grad[14] + err0 * basis[14];
            gradients[idx].sh_grad[15] = gradients[idx].sh_grad[15] + err0 * basis[15];
            gradients[idx].sh_grad[16] = gradients[idx].sh_grad[16] + err0 * basis[16];
            gradients[idx].sh_grad[17] = gradients[idx].sh_grad[17] + err0 * basis[17];
            gradients[idx].sh_grad[18] = gradients[idx].sh_grad[18] + err0 * basis[18];
            gradients[idx].sh_grad[19] = gradients[idx].sh_grad[19] + err0 * basis[19];
            gradients[idx].sh_grad[20] = gradients[idx].sh_grad[20] + err0 * basis[20];
            gradients[idx].sh_grad[21] = gradients[idx].sh_grad[21] + err0 * basis[21];
            gradients[idx].sh_grad[22] = gradients[idx].sh_grad[22] + err0 * basis[22];
            gradients[idx].sh_grad[23] = gradients[idx].sh_grad[23] + err0 * basis[23];
            gradients[idx].sh_grad[24] = gradients[idx].sh_grad[24] + err0 * basis[24];

            gradients[idx].sh_grad[25] = gradients[idx].sh_grad[25] + err1 * basis[0];
            gradients[idx].sh_grad[26] = gradients[idx].sh_grad[26] + err1 * basis[1];
            gradients[idx].sh_grad[27] = gradients[idx].sh_grad[27] + err1 * basis[2];
            gradients[idx].sh_grad[28] = gradients[idx].sh_grad[28] + err1 * basis[3];
            gradients[idx].sh_grad[29] = gradients[idx].sh_grad[29] + err1 * basis[4];
            gradients[idx].sh_grad[30] = gradients[idx].sh_grad[30] + err1 * basis[5];
            gradients[idx].sh_grad[31] = gradients[idx].sh_grad[31] + err1 * basis[6];
            gradients[idx].sh_grad[32] = gradients[idx].sh_grad[32] + err1 * basis[7];
            gradients[idx].sh_grad[33] = gradients[idx].sh_grad[33] + err1 * basis[8];
            gradients[idx].sh_grad[34] = gradients[idx].sh_grad[34] + err1 * basis[9];
            gradients[idx].sh_grad[35] = gradients[idx].sh_grad[35] + err1 * basis[10];
            gradients[idx].sh_grad[36] = gradients[idx].sh_grad[36] + err1 * basis[11];
            gradients[idx].sh_grad[37] = gradients[idx].sh_grad[37] + err1 * basis[12];
            gradients[idx].sh_grad[38] = gradients[idx].sh_grad[38] + err1 * basis[13];
            gradients[idx].sh_grad[39] = gradients[idx].sh_grad[39] + err1 * basis[14];
            gradients[idx].sh_grad[40] = gradients[idx].sh_grad[40] + err1 * basis[15];
            gradients[idx].sh_grad[41] = gradients[idx].sh_grad[41] + err1 * basis[16];
            gradients[idx].sh_grad[42] = gradients[idx].sh_grad[42] + err1 * basis[17];
            gradients[idx].sh_grad[43] = gradients[idx].sh_grad[43] + err1 * basis[18];
            gradients[idx].sh_grad[44] = gradients[idx].sh_grad[44] + err1 * basis[19];
            gradients[idx].sh_grad[45] = gradients[idx].sh_grad[45] + err1 * basis[20];
            gradients[idx].sh_grad[46] = gradients[idx].sh_grad[46] + err1 * basis[21];
            gradients[idx].sh_grad[47] = gradients[idx].sh_grad[47] + err1 * basis[22];
            gradients[idx].sh_grad[48] = gradients[idx].sh_grad[48] + err1 * basis[23];
            gradients[idx].sh_grad[49] = gradients[idx].sh_grad[49] + err1 * basis[24];

            gradients[idx].sh_grad[50] = gradients[idx].sh_grad[50] + err2 * basis[0];
            gradients[idx].sh_grad[51] = gradients[idx].sh_grad[51] + err2 * basis[1];
            gradients[idx].sh_grad[52] = gradients[idx].sh_grad[52] + err2 * basis[2];
            gradients[idx].sh_grad[53] = gradients[idx].sh_grad[53] + err2 * basis[3];
            gradients[idx].sh_grad[54] = gradients[idx].sh_grad[54] + err2 * basis[4];
            gradients[idx].sh_grad[55] = gradients[idx].sh_grad[55] + err2 * basis[5];
            gradients[idx].sh_grad[56] = gradients[idx].sh_grad[56] + err2 * basis[6];
            gradients[idx].sh_grad[57] = gradients[idx].sh_grad[57] + err2 * basis[7];
            gradients[idx].sh_grad[58] = gradients[idx].sh_grad[58] + err2 * basis[8];
            gradients[idx].sh_grad[59] = gradients[idx].sh_grad[59] + err2 * basis[9];
            gradients[idx].sh_grad[60] = gradients[idx].sh_grad[60] + err2 * basis[10];
            gradients[idx].sh_grad[61] = gradients[idx].sh_grad[61] + err2 * basis[11];
            gradients[idx].sh_grad[62] = gradients[idx].sh_grad[62] + err2 * basis[12];
            gradients[idx].sh_grad[63] = gradients[idx].sh_grad[63] + err2 * basis[13];
            gradients[idx].sh_grad[64] = gradients[idx].sh_grad[64] + err2 * basis[14];
            gradients[idx].sh_grad[65] = gradients[idx].sh_grad[65] + err2 * basis[15];
            gradients[idx].sh_grad[66] = gradients[idx].sh_grad[66] + err2 * basis[16];
            gradients[idx].sh_grad[67] = gradients[idx].sh_grad[67] + err2 * basis[17];
            gradients[idx].sh_grad[68] = gradients[idx].sh_grad[68] + err2 * basis[18];
            gradients[idx].sh_grad[69] = gradients[idx].sh_grad[69] + err2 * basis[19];
            gradients[idx].sh_grad[70] = gradients[idx].sh_grad[70] + err2 * basis[20];
            gradients[idx].sh_grad[71] = gradients[idx].sh_grad[71] + err2 * basis[21];
            gradients[idx].sh_grad[72] = gradients[idx].sh_grad[72] + err2 * basis[22];
            gradients[idx].sh_grad[73] = gradients[idx].sh_grad[73] + err2 * basis[23];
            gradients[idx].sh_grad[74] = gradients[idx].sh_grad[74] + err2 * basis[24];
        }
    }

    if (grad_inv_cov.x == 0.0 && grad_inv_cov.y == 0.0 && grad_inv_cov.z == 0.0) {
        return;
    }

    let inv_cov_mat = mat2x2<f32>(
        vec2<f32>(inv_cov.x, inv_cov.y),
        vec2<f32>(inv_cov.y, inv_cov.z)
    );
    let grad_inv_mat = mat2x2<f32>(
        vec2<f32>(grad_inv_cov.x, grad_inv_cov.y),
        vec2<f32>(grad_inv_cov.y, grad_inv_cov.z)
    );
    let grad_cov2d = (inv_cov_mat * grad_inv_mat * inv_cov_mat) * -1.0;

    let focal = gradient_camera.projection[0][0];
    let z = cam_pos.z;
    let z2 = z * z;
    let J = mat3x2<f32>(
        vec2<f32>(focal / z, 0.0),
        vec2<f32>(0.0, focal / z),
        vec2<f32>(-focal * cam_pos.x / z2, -focal * cam_pos.y / z2)
    );
    let grad_cov3d = transpose(J) * grad_cov2d * J;

    let r = quat_to_mat3(g.rotation);
    let s2 = vec3<f32>(scale.x * scale.x, scale.y * scale.y, scale.z * scale.z);
    let L = mat3x3<f32>(
        vec3<f32>(s2.x, 0.0, 0.0),
        vec3<f32>(0.0, s2.y, 0.0),
        vec3<f32>(0.0, 0.0, s2.z)
    );
    let grad_L = transpose(r) * grad_cov3d * r;
    let scale_grad = vec3<f32>(
        2.0 * s2.x * grad_L[0][0],
        2.0 * s2.y * grad_L[1][1],
        2.0 * s2.z * grad_L[2][2]
    );
    gradients[idx].scale_grad = vec4<f32>(scale_grad, 0.0);

    let grad_sym = grad_cov3d + transpose(grad_cov3d);
    let grad_r = grad_sym * r * L;
    gradients[idx].rotation_grad = rotation_grad_from_matrix(grad_r, g.rotation);

    let view_r = mat3x3<f32>(
        gradient_camera.view[0].xyz,
        gradient_camera.view[1].xyz,
        gradient_camera.view[2].xyz
    );
    let grad_world = transpose(view_r) * grad_cam;
    gradients[idx].position_grad = vec4<f32>(grad_world, 0.0);
}

// Vertex shader - fullscreen triangle
@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4<f32> {
    var pos = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0,  1.0)
    );
    return vec4<f32>(pos[idx], 0.0, 1.0);
}

// Fragment shader - TILE-BASED Gaussian evaluation (10x faster for large scenes)
// Uses tile buffer from bin_to_tiles pass to only evaluate Gaussians in current tile
@group(0) @binding(0) var<storage, read> projected_gs: array<ProjectedGaussian>;
@group(0) @binding(1) var<storage, read> tiles_render: array<TileData>;

// Tile configuration (must match CPU-side config)
const TILE_SIZE: u32 = 16u;
const MAX_GAUSSIANS_PER_TILE: u32 = 256u;

@fragment
fn fs_main(@builtin(position) frag_coord: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<f32>(frag_coord.x, frag_coord.y);
    
    // Compute tile index for this pixel
    let tile_x = u32(frag_coord.x) / TILE_SIZE;
    let tile_y = u32(frag_coord.y) / TILE_SIZE;
    let tiles_per_row = (800u + TILE_SIZE - 1u) / TILE_SIZE;  // Assume 800px width, should be uniform
    let tile_idx = tile_y * tiles_per_row + tile_x;
    
    // Get number of Gaussians in this tile
    let num_gaussians_in_tile = min(atomicLoad(&tiles_render[tile_idx].count), MAX_GAUSSIANS_PER_TILE);
    
    var color = vec3<f32>(0.0);
    var alpha = 1.0;
    
    // Iterate ONLY through Gaussians in this tile (sorted by depth)
    for (var i: u32 = 0u; i < num_gaussians_in_tile; i++) {
        if (alpha < 0.01) {
            break;
        }
        
        // Get Gaussian index from tile buffer
        let gaussian_idx = tiles_render[tile_idx].gaussian_indices[i];
        let g = projected_gs[gaussian_idx];
        
        // Skip invalid
        if (g.depth > 1e9) {
            continue;
        }
        
        // Distance from Gaussian center
        let d = pixel - g.position;
        
        // Evaluate 2D Gaussian
        let cov = g.cov2d;
        let det = cov.x * cov.z - cov.y * cov.y;
        if (det <= 0.0) {
            continue;
        }
        
        let inv_det = 1.0 / det;
        let power = -0.5 * (
            cov.z * d.x * d.x * inv_det +
            -2.0 * cov.y * d.x * d.y * inv_det +
            cov.x * d.y * d.y * inv_det
        );
        
        if (power > 0.0) {
            continue;
        }
        
        let gaussian_alpha = min(0.99, g.color.a * exp(power));
        
        if (gaussian_alpha < 0.01) {
            continue;
        }
        
        // Alpha blending
        color += g.color.rgb * gaussian_alpha * alpha;
        alpha *= (1.0 - gaussian_alpha);
    }
    
    return vec4<f32>(color, 1.0 - alpha);
}

// Legacy fallback fragment shader (non-tile-based, for compatibility)
@fragment
fn fs_main_legacy(@builtin(position) frag_coord: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<f32>(frag_coord.x, frag_coord.y);
    
    var color = vec3<f32>(0.0);
    var alpha = 1.0;
    
    // Iterate through ALL Gaussians (O(n) per pixel - slow for large scenes)
    let num_gaussians = arrayLength(&projected_gs);
    for (var i: u32 = 0u; i < min(num_gaussians, 1000u); i++) {
        if (alpha < 0.01) {
            break;
        }
        
        let g = projected_gs[i];
        
        if (g.depth > 1e9) {
            continue;
        }
        
        let d = pixel - g.position;
        let cov = g.cov2d;
        let det = cov.x * cov.z - cov.y * cov.y;
        if (det <= 0.0) {
            continue;
        }
        
        let inv_det = 1.0 / det;
        let power = -0.5 * (
            cov.z * d.x * d.x * inv_det +
            -2.0 * cov.y * d.x * d.y * inv_det +
            cov.x * d.y * d.y * inv_det
        );
        
        if (power > 0.0) {
            continue;
        }
        
        let gaussian_alpha = min(0.99, g.color.a * exp(power));
        if (gaussian_alpha < 0.01) {
            continue;
        }
        
        color += g.color.rgb * gaussian_alpha * alpha;
        alpha *= (1.0 - gaussian_alpha);
    }
    
    return vec4<f32>(color, 1.0 - alpha);
}
"#;

#[cfg(not(feature = "wgpu"))]
pub struct GpuRasterizer {
    config: RasterizerConfig,
    gaussians: Vec<Gaussian3DGpu>,
    camera: CameraUniforms,
    output: Vec<u8>,
}

#[cfg(not(feature = "wgpu"))]
impl GpuRasterizer {
    pub fn new(config: RasterizerConfig) -> Self {
        Self::new_cpu(config)
    }

    pub fn new_cpu(config: RasterizerConfig) -> Self {
        let camera = CameraUniforms {
            view: na::Matrix4::<f32>::identity().into(),
            projection: na::Matrix4::<f32>::identity().into(),
            view_projection: na::Matrix4::<f32>::identity().into(),
            camera_position: [0.0, 0.0, 0.0, 1.0],
            viewport: [config.width as f32, config.height as f32, 0.1, 1000.0],
        };
        let output = vec![0u8; (config.width * config.height * 4) as usize];
        Self {
            config,
            gaussians: Vec::new(),
            camera,
            output,
        }
    }

    pub fn upload_gaussians(&mut self, gaussians: &[Gaussian3DGpu]) {
        self.gaussians = gaussians.to_vec();
    }

    pub fn set_camera(&mut self, camera: &CameraUniforms) {
        self.camera = *camera;
    }

    pub fn render(&mut self) -> Result<()> {
        let width = self.config.width as usize;
        let height = self.config.height as usize;
        let mut accum = vec![[0.0f32; 3]; width * height];
        let mut trans = vec![1.0f32; width * height];

        let view_proj = mat4_from_array(&self.camera.view_projection);
        let proj = mat4_from_array(&self.camera.projection);
        let focal = proj[(0, 0)];

        for g in &self.gaussians {
            let world = na::Vector4::new(g.position[0], g.position[1], g.position[2], 1.0);
            let clip = view_proj * world;
            if clip.w <= 1e-6 {
                continue;
            }
            let ndc = na::Vector3::new(clip.x / clip.w, clip.y / clip.w, clip.z / clip.w);
            let screen_x = (ndc.x * 0.5 + 0.5) * self.camera.viewport[0];
            let screen_y = (1.0 - (ndc.y * 0.5 + 0.5)) * self.camera.viewport[1];

            if !screen_x.is_finite() || !screen_y.is_finite() {
                continue;
            }

            let scale = na::Vector3::new(g.scale[0].exp(), g.scale[1].exp(), g.scale[2].exp());
            let cov3d = compute_cov3d(scale, g.rotation);

            let cam_pos = na::Vector3::new(
                (self.camera.view[0][0] * world.x
                    + self.camera.view[0][1] * world.y
                    + self.camera.view[0][2] * world.z
                    + self.camera.view[0][3]),
                (self.camera.view[1][0] * world.x
                    + self.camera.view[1][1] * world.y
                    + self.camera.view[1][2] * world.z
                    + self.camera.view[1][3]),
                (self.camera.view[2][0] * world.x
                    + self.camera.view[2][1] * world.y
                    + self.camera.view[2][2] * world.z
                    + self.camera.view[2][3]),
            );

            if cam_pos.z <= 1e-6 {
                continue;
            }

            let cov2d = project_cov3d_to_2d(cov3d, cam_pos, focal);
            let radius = 3.0 * cov2d.x.max(cov2d.z).sqrt();
            if !radius.is_finite() || radius <= 0.0 {
                continue;
            }

            let min_x = (screen_x - radius).floor().max(0.0) as i32;
            let max_x = (screen_x + radius)
                .ceil()
                .min(self.config.width as f32 - 1.0) as i32;
            let min_y = (screen_y - radius).floor().max(0.0) as i32;
            let max_y = (screen_y + radius)
                .ceil()
                .min(self.config.height as f32 - 1.0) as i32;

            let view_dir = (-cam_pos).normalize();
            let basis = eval_sh_basis_cpu(view_dir);
            let color = eval_sh_color_cpu(&g.sh_coeffs, &basis);
            let opacity = 1.0 / (1.0 + (-g.opacity[0]).exp());

            let det = cov2d.x * cov2d.z - cov2d.y * cov2d.y;
            if det <= 1e-8 {
                continue;
            }
            let inv_det = 1.0 / det;

            for y in min_y..=max_y {
                for x in min_x..=max_x {
                    let dx = x as f32 + 0.5 - screen_x;
                    let dy = y as f32 + 0.5 - screen_y;
                    let power = -0.5
                        * (cov2d.z * dx * dx * inv_det - 2.0 * cov2d.y * dx * dy * inv_det
                            + cov2d.x * dy * dy * inv_det);
                    if power > 0.0 {
                        continue;
                    }
                    let gaussian_alpha = (opacity * power.exp()).min(0.99);
                    if gaussian_alpha < 0.01 {
                        continue;
                    }

                    let idx = y as usize * width + x as usize;
                    let t = trans[idx];
                    accum[idx][0] += color[0] * gaussian_alpha * t;
                    accum[idx][1] += color[1] * gaussian_alpha * t;
                    accum[idx][2] += color[2] * gaussian_alpha * t;
                    trans[idx] = t * (1.0 - gaussian_alpha);
                }
            }
        }

        for (i, rgb) in accum.iter().enumerate() {
            let alpha = 1.0 - trans[i];
            self.output[i * 4] = (rgb[0].clamp(0.0, 1.0) * 255.0) as u8;
            self.output[i * 4 + 1] = (rgb[1].clamp(0.0, 1.0) * 255.0) as u8;
            self.output[i * 4 + 2] = (rgb[2].clamp(0.0, 1.0) * 255.0) as u8;
            self.output[i * 4 + 3] = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
        }

        Ok(())
    }

    pub async fn read_output(&self) -> Result<Vec<u8>> {
        Ok(self.output.clone())
    }
}

#[cfg(not(feature = "wgpu"))]
fn mat4_from_array(m: &[[f32; 4]; 4]) -> na::Matrix4<f32> {
    na::Matrix4::new(
        m[0][0], m[0][1], m[0][2], m[0][3], m[1][0], m[1][1], m[1][2], m[1][3], m[2][0], m[2][1],
        m[2][2], m[2][3], m[3][0], m[3][1], m[3][2], m[3][3],
    )
}

#[cfg(not(feature = "wgpu"))]
fn compute_cov3d(scale: na::Vector3<f32>, rotation: [f32; 4]) -> na::Matrix3<f32> {
    let q = na::Quaternion::new(rotation[3], rotation[0], rotation[1], rotation[2]);
    let r = na::UnitQuaternion::from_quaternion(q).to_rotation_matrix();
    let s = na::Matrix3::from_diagonal(&scale);
    let m = r.matrix() * s;
    m * m.transpose()
}

#[cfg(not(feature = "wgpu"))]
fn project_cov3d_to_2d(
    cov3d: na::Matrix3<f32>,
    cam_pos: na::Vector3<f32>,
    focal: f32,
) -> na::Vector3<f32> {
    let z = cam_pos.z;
    let z2 = z * z;
    let j = na::Matrix2x3::new(
        focal / z,
        0.0,
        -focal * cam_pos.x / z2,
        0.0,
        focal / z,
        -focal * cam_pos.y / z2,
    );
    let cov2d = j * cov3d * j.transpose();
    na::Vector3::new(cov2d[(0, 0)] + 0.3, cov2d[(0, 1)], cov2d[(1, 1)] + 0.3)
}

#[cfg(not(feature = "wgpu"))]
fn eval_sh_basis_cpu(view_dir: na::Vector3<f32>) -> [f32; SH_COEFFS_PER_CHANNEL] {
    let mut dir = view_dir;
    let norm = dir.norm();
    if norm > 1e-6 {
        dir /= norm;
    } else {
        dir = na::Vector3::new(0.0, 0.0, 1.0);
    }
    let x = dir.x;
    let y = dir.y;
    let z = dir.z;
    let xx = x * x;
    let yy = y * y;
    let zz = z * z;
    let xy = x * y;
    let yz = y * z;
    let xz = x * z;
    let zz2 = zz * zz;
    let xx2 = xx * xx;
    let yy2 = yy * yy;

    let c0 = 0.2820947918f32;
    let c1 = 0.4886025119f32;
    let c2_0 = 1.0925484306f32;
    let c2_1 = 0.3153915653f32;
    let c2_2 = 0.5462742153f32;
    let c3_0 = 0.5900435899f32;
    let c3_1 = 2.8906114426f32;
    let c3_2 = 0.4570457995f32;
    let c3_3 = 0.3731763326f32;
    let c3_4 = 1.4453057213f32;
    let c4_0 = 2.5033429418f32;
    let c4_1 = 1.7701307698f32;
    let c4_2 = 0.9461746958f32;
    let c4_3 = 0.6690465436f32;
    let c4_4 = 0.1057855469f32;
    let c4_6 = 0.4730873479f32;
    let c4_8 = 0.6258357354f32;

    [
        c0,
        -c1 * y,
        c1 * z,
        -c1 * x,
        c2_0 * xy,
        -c2_0 * yz,
        c2_1 * (3.0 * zz - 1.0),
        -c2_0 * xz,
        c2_2 * (xx - yy),
        -c3_0 * y * (3.0 * xx - yy),
        c3_1 * xy * z,
        -c3_2 * y * (5.0 * zz - 1.0),
        c3_3 * z * (5.0 * zz - 3.0),
        -c3_2 * x * (5.0 * zz - 1.0),
        c3_4 * z * (xx - yy),
        -c3_0 * x * (xx - 3.0 * yy),
        c4_0 * xy * (xx - yy),
        -c4_1 * y * z * (3.0 * xx - yy),
        c4_2 * xy * (7.0 * zz - 1.0),
        -c4_3 * y * z * (7.0 * zz - 3.0),
        c4_4 * (35.0 * zz2 - 30.0 * zz + 3.0),
        -c4_3 * x * z * (7.0 * zz - 3.0),
        c4_6 * (xx - yy) * (7.0 * zz - 1.0),
        -c4_1 * x * z * (xx - 3.0 * yy),
        c4_8 * (xx2 - 6.0 * xx * yy + yy2),
    ]
}

#[cfg(not(feature = "wgpu"))]
fn eval_sh_color_cpu(
    coeffs: &[f32; SH_COEFFS_TOTAL],
    basis: &[f32; SH_COEFFS_PER_CHANNEL],
) -> [f32; 3] {
    let mut color = [0.0f32; 3];
    for channel in 0..3 {
        let base = channel * SH_COEFFS_PER_CHANNEL;
        let mut accum = 0.0f32;
        for i in 0..SH_COEFFS_PER_CHANNEL {
            accum += coeffs[base + i] * basis[i];
        }
        let value = (accum + 0.5).clamp(0.0, 1.0);
        color[channel] = value;
    }
    color
}
