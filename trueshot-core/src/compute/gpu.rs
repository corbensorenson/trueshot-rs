use crate::reconstruction::ColoredPoint;
use anyhow::Result;
use nalgebra as na;

pub struct GpuCompute {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
}

impl GpuCompute {
    pub async fn new() -> Result<Self> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .ok_or_else(|| anyhow::anyhow!("No GPU Adapter found"))?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await?;

        // Load Shader
        let shader = device.create_shader_module(wgpu::include_wgsl!("shaders/heatmap.wgsl"));

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Heatmap Pipeline"),
            layout: None,
            module: &shader,
            entry_point: "main",
        });

        Ok(Self {
            device,
            queue,
            pipeline,
        })
    }

    pub async fn compute_density(
        &self,
        points: &[ColoredPoint],
        voxel_size: f32,
    ) -> Result<Vec<u32>> {
        use wgpu::util::DeviceExt;

        if points.is_empty() {
            return Ok(vec![]);
        }

        // 1. Calculate Bounds & Grid
        let mut min = points[0].position.coords;
        let mut max = points[0].position.coords;
        for p in points {
            min = min.inf(&p.position.coords);
            max = max.sup(&p.position.coords);
        }
        // Expand slightly
        min -= na::Vector3::new(voxel_size, voxel_size, voxel_size);
        max += na::Vector3::new(voxel_size, voxel_size, voxel_size);

        let grid_size_x = ((max.x - min.x) / voxel_size).ceil() as u32;
        let grid_size_y = ((max.y - min.y) / voxel_size).ceil() as u32;
        let grid_size_z = ((max.z - min.z) / voxel_size).ceil() as u32;
        let total_voxels = (grid_size_x * grid_size_y * grid_size_z) as usize;

        // 2. Prepare Buffers
        // Input Points: vec4 (x, y, z, padding)
        let raw_points: Vec<f32> = points
            .iter()
            .flat_map(|p| vec![p.position.x, p.position.y, p.position.z, 0.0])
            .collect();
        let point_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Point Buffer"),
                contents: bytemuck::cast_slice(&raw_points),
                usage: wgpu::BufferUsages::STORAGE,
            });

        // Output Density: u32 per voxel
        let density_buffer_size = (total_voxels * 4) as wgpu::BufferAddress;
        let density_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Density Buffer"),
            size: density_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Shader expects:
        // vec3<f32> min;
        // vec3<f32> max;
        // vec3<u32> size;
        // f32 voxel_size;
        // Alignment rules are strict. Let's repack.
        #[repr(C)]
        #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
        struct Uniforms {
            min_x: f32,
            min_y: f32,
            min_z: f32,
            pad1: f32,
            max_x: f32,
            max_y: f32,
            max_z: f32,
            pad2: f32,
            grid_x: u32,
            grid_y: u32,
            grid_z: u32,
            voxel_size: f32,
        }
        let uniform_data = Uniforms {
            min_x: min.x,
            min_y: min.y,
            min_z: min.z,
            pad1: 0.0,
            max_x: max.x,
            max_y: max.y,
            max_z: max.z,
            pad2: 0.0,
            grid_x: grid_size_x,
            grid_y: grid_size_y,
            grid_z: grid_size_z,
            voxel_size,
        };

        let uniform_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Uniform Buffer"),
                contents: bytemuck::bytes_of(&uniform_data),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        // 3. Bind Group
        let bind_group_layout = self.pipeline.get_bind_group_layout(0);
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Heatmap Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: point_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: density_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform_buffer.as_entire_binding(),
                },
            ],
        });

        // 4. Dispatch
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Compute Encoder"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Compute Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let workgroups = (points.len() as u32).div_ceil(64);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }

        // 5. Readback
        let staging_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Staging Buffer"),
            size: density_buffer_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&density_buffer, 0, &staging_buffer, 0, density_buffer_size);

        self.queue.submit(Some(encoder.finish()));

        let slice = staging_buffer.slice(..);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        slice.map_async(wgpu::MapMode::Read, move |v| sender.send(v).unwrap());

        self.device.poll(wgpu::Maintain::Wait);
        receiver.await.unwrap().unwrap();

        let data = slice.get_mapped_range();
        let result: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging_buffer.unmap();

        Ok(result)
    }
}
