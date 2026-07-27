//! Bounded multi-pass AHD demosaicing on Apple Metal through wgpu.
//!
//! The GPU uses the same directional interpolation, integer Lab lookup,
//! homogeneity thresholds, and source-sample preservation contract as the CPU
//! implementation. Haloed row bands bound unified-memory use for full-frame
//! RAW input and make every emitted interior pixel independent of band edges.

use super::gpu_context::GpuContext;
use crate::demosaic_ahd::ahd_normalization_scale;
use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use ndarray::Array3;
use std::sync::Arc;

const WORKGROUP_EDGE: u32 = 16;
const BAND_OUTPUT_ROWS: usize = 512;
const BAND_HALO: usize = 6;
const MIN_GPU_PIXELS: usize = 1_000_000;
const OUTPUT_BORDER: usize = 5;
const CBR_T_ENTRIES: usize = 65_536;
pub const METAL_AHD_NORMALIZED_PARITY_TOLERANCE: f32 = 2e-3;
pub const METAL_AHD_RELEASE_QUALIFICATION_TOLERANCE: f32 = 1e-3;

#[derive(Debug)]
pub struct GpuAhdOutput {
    pub image: Array3<f32>,
    pub bands: usize,
    pub adapter: String,
}

pub struct GpuAhdEngine {
    context: Arc<GpuContext>,
    bind_group_layout: wgpu::BindGroupLayout,
    green_pipeline: wgpu::ComputePipeline,
    rb_lab_pipeline: wgpu::ComputePipeline,
    homogeneity_pipeline: wgpu::ComputePipeline,
    combine_pipeline: wgpu::ComputePipeline,
    cbrt_buffer: wgpu::Buffer,
}

impl GpuAhdEngine {
    /// Build the qualified Metal pipeline. Other backends remain on CPU until
    /// they have their own retained parity and performance evidence.
    pub fn new(context: Arc<GpuContext>) -> Result<Option<Self>> {
        if context.adapter_info.backend != wgpu::Backend::Metal {
            tracing::info!(
                "AHD GPU path requires qualified Metal; {:?} uses CPU",
                context.adapter_info.backend
            );
            return Ok(None);
        }

        let device = &context.device;
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("TrueShot bounded AHD shader"),
            source: wgpu::ShaderSource::Wgsl(AHD_SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("TrueShot AHD bind group layout"),
            entries: &[
                storage_layout_entry(0, true),
                storage_layout_entry(1, false),
                storage_layout_entry(2, false),
                storage_layout_entry(3, false),
                storage_layout_entry(4, false),
                uniform_layout_entry(5),
                storage_layout_entry(6, true),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("TrueShot AHD pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let create_pipeline = |label: &'static str, entry_point: &'static str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point,
            })
        };
        let green_pipeline = create_pipeline("TrueShot AHD green", "interpolate_green");
        let rb_lab_pipeline = create_pipeline("TrueShot AHD RGB Lab", "interpolate_rb_lab");
        let homogeneity_pipeline = create_pipeline("TrueShot AHD homogeneity", "build_homogeneity");
        let combine_pipeline = create_pipeline("TrueShot AHD combine", "combine_directions");
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            anyhow::bail!("Metal AHD pipeline validation failed: {error}");
        }

        let cbrt = exact_cbrt_lut();
        let cbrt_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("TrueShot AHD Lab cbrt LUT"),
            size: byte_len::<i32>(cbrt.len())?,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context
            .queue
            .write_buffer(&cbrt_buffer, 0, bytemuck::cast_slice(&cbrt));

        Ok(Some(Self {
            context,
            bind_group_layout,
            green_pipeline,
            rb_lab_pipeline,
            homogeneity_pipeline,
            combine_pipeline,
            cbrt_buffer,
        }))
    }

    /// Demosaic sufficiently large normalized or HDR-linear RGGB Bayer input.
    ///
    /// `None` means the workload or device limits are unsuitable and the
    /// caller must retain the CPU path. Device or shader failures are errors so
    /// production can report the fallback rather than silently claiming Metal.
    pub fn demosaic(
        &self,
        image: &Array3<f32>,
        rgb_cam: &[[f32; 4]; 3],
    ) -> Result<Option<GpuAhdOutput>> {
        let (height, width, _) = image.dim();
        let pixels = height
            .checked_mul(width)
            .context("Metal AHD image dimensions overflow")?;
        if pixels < MIN_GPU_PIXELS {
            return Ok(None);
        }
        self.execute(image, rgb_cam).map(Some)
    }

    /// Exact upper bound for the temporary buffers retained while a group is
    /// demosaiced. The caller uses this for unified-memory admission before
    /// decoding starts.
    pub fn scratch_bytes(&self, width: usize, height: usize) -> Result<Option<u64>> {
        let pixels = width
            .checked_mul(height)
            .context("Metal AHD image dimensions overflow")?;
        if pixels < MIN_GPU_PIXELS || width <= OUTPUT_BORDER * 2 || height <= OUTPUT_BORDER * 2 {
            return Ok(None);
        }
        let output_rows = self.output_rows_for_width(width)?;
        let allocation_rows = output_rows
            .checked_add(BAND_HALO * 2)
            .context("Metal AHD row allocation overflow")?
            .min(height);
        let allocation_pixels = width
            .checked_mul(allocation_rows)
            .context("Metal AHD band allocation overflow")?;
        // Bayer f32; two directional RGB f32x4 and Lab i32x4 candidates;
        // two u32 homogeneity maps; output/readback f32x4; one reusable host
        // normalization band; uniform + exact LUT.
        let band_bytes = byte_len::<u8>(
            allocation_pixels
                .checked_mul(112)
                .context("Metal AHD scratch estimate overflow")?,
        )?;
        band_bytes
            .checked_add(std::mem::size_of::<AhdParams>() as u64)
            .and_then(|bytes| bytes.checked_add((CBR_T_ENTRIES * 4) as u64))
            .map(Some)
            .context("Metal AHD scratch estimate overflow")
    }

    fn output_rows_for_width(&self, width: usize) -> Result<usize> {
        let max_binding = u64::from(self.context.device.limits().max_storage_buffer_binding_size);
        let row_candidate_bytes =
            byte_len::<[f32; 4]>(width.checked_mul(2).context("AHD row size overflow")?)?;
        let maximum_band_rows = usize::try_from(max_binding / row_candidate_bytes)
            .unwrap_or(0)
            .saturating_sub(BAND_HALO * 2);
        let output_rows = BAND_OUTPUT_ROWS.min(maximum_band_rows);
        if output_rows < 16 {
            anyhow::bail!(
                "Metal AHD storage binding limit {max_binding} cannot hold a {width}-pixel row band"
            );
        }
        Ok(output_rows)
    }

    fn execute(&self, image: &Array3<f32>, rgb_cam: &[[f32; 4]; 3]) -> Result<GpuAhdOutput> {
        let (height, width, channels) = image.dim();
        if channels != 1 || height == 0 || width == 0 {
            anyhow::bail!("Metal AHD requires a non-empty single-channel Bayer image");
        }
        if width > u32::MAX as usize || height > u32::MAX as usize {
            anyhow::bail!("Metal AHD dimensions exceed shader coordinate limits");
        }
        let input = image
            .as_slice()
            .context("Metal AHD input must be contiguous")?;
        if input.iter().any(|value| !value.is_finite() || *value < 0.0) {
            anyhow::bail!("Metal AHD input must contain finite, non-negative values");
        }
        if width <= OUTPUT_BORDER * 2 || height <= OUTPUT_BORDER * 2 {
            anyhow::bail!("Metal AHD input is too small for the qualified support radius");
        }

        let range_scale = ahd_normalization_scale(input.iter().copied().fold(1.0f32, f32::max))?;
        let xyz_cam = xyz_camera_matrix(rgb_cam);

        let output_rows = self.output_rows_for_width(width)?;
        let allocation_rows = (output_rows + BAND_HALO * 2).min(height);
        let allocation_pixels = width
            .checked_mul(allocation_rows)
            .context("Metal AHD band allocation overflow")?;
        let mut normalized_band =
            (range_scale != 1.0).then(|| Vec::<f32>::with_capacity(allocation_pixels));
        let buffers = AhdBuffers::new(&self.context.device, allocation_pixels)?;
        let bind_group = self
            .context
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("TrueShot AHD bind group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffers.bayer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: buffers.candidates.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buffers.lab.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: buffers.homogeneity.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: buffers.output.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: buffers.params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: self.cbrt_buffer.as_entire_binding(),
                    },
                ],
            });

        let mut output = Array3::<f32>::zeros((height, width, 3));
        let output_slice = output
            .as_slice_mut()
            .context("Metal AHD output must be contiguous")?;
        let mut band_count = 0usize;
        let mut output_y0 = OUTPUT_BORDER;
        let output_end = height - OUTPUT_BORDER;
        while output_y0 < output_end {
            let output_y1 = (output_y0 + output_rows).min(output_end);
            let input_y0 = output_y0.saturating_sub(BAND_HALO);
            let input_y1 = (output_y1 + BAND_HALO).min(height);
            let band_height = input_y1 - input_y0;
            let band_pixels = width
                .checked_mul(band_height)
                .context("Metal AHD band size overflow")?;
            let input_start = input_y0
                .checked_mul(width)
                .context("Metal AHD input offset overflow")?;
            let input_end = input_y1
                .checked_mul(width)
                .context("Metal AHD input extent overflow")?;
            let params = AhdParams {
                width: width as u32,
                band_height: band_height as u32,
                sensor_y0: input_y0 as u32,
                full_height: height as u32,
                xyz_row_0: xyz_cam[0],
                xyz_row_1: xyz_cam[1],
                xyz_row_2: xyz_cam[2],
            };
            let input_band = &input[input_start..input_end];
            if let Some(staging) = normalized_band.as_mut() {
                staging.clear();
                staging.extend(input_band.iter().map(|value| *value / range_scale));
                self.context
                    .queue
                    .write_buffer(&buffers.bayer, 0, bytemuck::cast_slice(staging));
            } else {
                self.context.queue.write_buffer(
                    &buffers.bayer,
                    0,
                    bytemuck::cast_slice(input_band),
                );
            }
            self.context
                .queue
                .write_buffer(&buffers.params, 0, bytemuck::bytes_of(&params));

            let mut encoder =
                self.context
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("TrueShot bounded AHD encoder"),
                    });
            let dispatch_x = (width as u32).div_ceil(WORKGROUP_EDGE);
            let dispatch_y = (band_height as u32).div_ceil(WORKGROUP_EDGE);
            for (label, pipeline) in [
                ("AHD green", &self.green_pipeline),
                ("AHD RGB Lab", &self.rb_lab_pipeline),
                ("AHD homogeneity", &self.homogeneity_pipeline),
                ("AHD combine", &self.combine_pipeline),
            ] {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some(label),
                    timestamp_writes: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups(dispatch_x, dispatch_y, 1);
            }
            let output_bytes = byte_len::<[f32; 4]>(band_pixels)?;
            encoder.copy_buffer_to_buffer(&buffers.output, 0, &buffers.readback, 0, output_bytes);
            self.context.queue.submit(Some(encoder.finish()));

            let mapped = map_readback(
                &self.context.device,
                &buffers.readback,
                output_bytes,
                "Metal AHD",
            )?;
            let gpu_pixels: &[[f32; 4]] = bytemuck::cast_slice(&mapped);
            for sensor_y in output_y0..output_y1 {
                let local_y = sensor_y - input_y0;
                let source_row = &gpu_pixels[local_y * width..(local_y + 1) * width];
                let destination_start = (sensor_y * width + OUTPUT_BORDER) * 3;
                let destination_end = (sensor_y * width + width - OUTPUT_BORDER) * 3;
                let destination = &mut output_slice[destination_start..destination_end];
                for (pixel, target) in source_row[OUTPUT_BORDER..width - OUTPUT_BORDER]
                    .iter()
                    .zip(destination.chunks_exact_mut(3))
                {
                    target.copy_from_slice(&pixel[..3]);
                }
            }
            drop(mapped);
            buffers.readback.unmap();
            band_count += 1;
            output_y0 = output_y1;
        }

        if range_scale != 1.0 {
            output_slice
                .iter_mut()
                .for_each(|value| *value *= range_scale);
        }
        fill_bilinear_border(input, output_slice, width, height);
        Ok(GpuAhdOutput {
            image: output,
            bands: band_count,
            adapter: self.context.adapter_info.name.clone(),
        })
    }
}

struct AhdBuffers {
    bayer: wgpu::Buffer,
    candidates: wgpu::Buffer,
    lab: wgpu::Buffer,
    homogeneity: wgpu::Buffer,
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
    params: wgpu::Buffer,
}

impl AhdBuffers {
    fn new(device: &wgpu::Device, pixels: usize) -> Result<Self> {
        let bayer_bytes = byte_len::<f32>(pixels)?;
        let directional_pixels = pixels
            .checked_mul(2)
            .context("Metal AHD directional allocation overflow")?;
        let candidate_bytes = byte_len::<[f32; 4]>(directional_pixels)?;
        let lab_bytes = byte_len::<[i32; 4]>(directional_pixels)?;
        let homogeneity_bytes = byte_len::<u32>(directional_pixels)?;
        let output_bytes = byte_len::<[f32; 4]>(pixels)?;
        let storage = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Ok(Self {
            bayer: storage("TrueShot AHD Bayer", bayer_bytes),
            candidates: storage("TrueShot AHD directional RGB", candidate_bytes),
            lab: storage("TrueShot AHD directional Lab", lab_bytes),
            homogeneity: storage("TrueShot AHD homogeneity", homogeneity_bytes),
            output: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("TrueShot AHD output"),
                size: output_bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("TrueShot AHD readback"),
                size: output_bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("TrueShot AHD params"),
                size: std::mem::size_of::<AhdParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct AhdParams {
    width: u32,
    band_height: u32,
    sensor_y0: u32,
    full_height: u32,
    xyz_row_0: [f32; 4],
    xyz_row_1: [f32; 4],
    xyz_row_2: [f32; 4],
}

fn storage_layout_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn byte_len<T>(count: usize) -> Result<u64> {
    count
        .checked_mul(std::mem::size_of::<T>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .context("GPU AHD buffer size overflow")
}

fn map_readback<'a>(
    device: &wgpu::Device,
    buffer: &'a wgpu::Buffer,
    bytes: u64,
    label: &str,
) -> Result<wgpu::BufferView<'a>> {
    let slice = buffer.slice(..bytes);
    let (sender, receiver) = futures::channel::oneshot::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device.poll(wgpu::Maintain::Wait);
    pollster::block_on(receiver)
        .with_context(|| format!("{label} mapping callback was dropped"))?
        .with_context(|| format!("{label} readback mapping failed"))?;
    Ok(slice.get_mapped_range())
}

fn exact_cbrt_lut() -> Vec<i32> {
    (0..CBR_T_ENTRIES)
        .map(|index| {
            let value = index as f32 / 65_535.0;
            let transformed = if value > 0.008_856 {
                value.powf(1.0 / 3.0)
            } else {
                7.787 * value + 16.0 / 116.0
            };
            (transformed * 32_767.0).round() as i32
        })
        .collect()
}

fn xyz_camera_matrix(rgb_cam: &[[f32; 4]; 3]) -> [[f32; 4]; 3] {
    const XYZ_RGB: [[f32; 3]; 3] = [
        [0.412_453, 0.357_580, 0.180_423],
        [0.212_671, 0.715_160, 0.072_169],
        [0.019_334, 0.119_193, 0.950_227],
    ];
    const D65_WHITE: [f32; 3] = [0.950_456, 1.0, 1.088_754];
    let mut output = [[0.0f32; 4]; 3];
    for row in 0..3 {
        for column in 0..4 {
            for channel in 0..3 {
                output[row][column] +=
                    XYZ_RGB[row][channel] * rgb_cam[channel][column] / D65_WHITE[row];
            }
        }
    }
    output
}

fn fill_bilinear_border(bayer: &[f32], output: &mut [f32], width: usize, height: usize) {
    for y in 0..height {
        for x in 0..width {
            if y >= OUTPUT_BORDER
                && y + OUTPUT_BORDER < height
                && x >= OUTPUT_BORDER
                && x + OUTPUT_BORDER < width
            {
                continue;
            }
            let rgb = bilinear_pixel(bayer, width, height, y, x);
            output[(y * width + x) * 3..(y * width + x + 1) * 3].copy_from_slice(&rgb);
        }
    }
}

fn bilinear_pixel(bayer: &[f32], width: usize, height: usize, row: usize, col: usize) -> [f32; 3] {
    let direct = cfa_rgb(row, col);
    let mut rgb = [0.0f32; 3];
    rgb[direct] = bayer[row * width + col];
    for (channel, value) in rgb.iter_mut().enumerate() {
        if channel == direct {
            continue;
        }
        let mut weighted_sum = 0.0f32;
        let mut weight_sum = 0.0f32;
        for radius in 1..=2isize {
            let y0 = (row as isize - radius).max(0);
            let y1 = (row as isize + radius).min(height as isize - 1);
            let x0 = (col as isize - radius).max(0);
            let x1 = (col as isize + radius).min(width as isize - 1);
            for source_y in y0..=y1 {
                for source_x in x0..=x1 {
                    if cfa_rgb(source_y as usize, source_x as usize) != channel {
                        continue;
                    }
                    let dy = source_y - row as isize;
                    let dx = source_x - col as isize;
                    let distance_squared = (dx * dx + dy * dy) as f32;
                    if distance_squared > 0.0 {
                        let weight = distance_squared.recip();
                        weighted_sum +=
                            weight * bayer[source_y as usize * width + source_x as usize];
                        weight_sum += weight;
                    }
                }
            }
            if weight_sum > 0.0 {
                break;
            }
        }
        *value = if weight_sum > 0.0 {
            weighted_sum / weight_sum
        } else {
            bayer[row * width + col]
        };
    }
    rgb
}

fn cfa_rgb(row: usize, col: usize) -> usize {
    match ((row & 1) << 1) | (col & 1) {
        0 => 0,
        1 | 2 => 1,
        3 => 2,
        _ => unreachable!(),
    }
}

const AHD_SHADER: &str = r#"
struct Params {
    width: u32,
    band_height: u32,
    sensor_y0: u32,
    full_height: u32,
    xyz_row_0: vec4<f32>,
    xyz_row_1: vec4<f32>,
    xyz_row_2: vec4<f32>,
}

@group(0) @binding(0) var<storage, read> bayer: array<f32>;
@group(0) @binding(1) var<storage, read_write> candidates: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> lab: array<vec4<i32>>;
@group(0) @binding(3) var<storage, read_write> homogeneity: array<u32>;
@group(0) @binding(4) var<storage, read_write> output: array<vec4<f32>>;
@group(0) @binding(5) var<uniform> params: Params;
@group(0) @binding(6) var<storage, read> cbrt_lut: array<i32>;

fn pixel_index(x: u32, y: u32) -> u32 {
    return y * params.width + x;
}

fn direction_index(direction: u32, x: u32, y: u32) -> u32 {
    return direction * params.width * params.band_height + pixel_index(x, y);
}

fn sensor_y(local_y: u32) -> u32 {
    return params.sensor_y0 + local_y;
}

fn cfa_rgb(x: u32, local_y: u32) -> u32 {
    let site = ((sensor_y(local_y) & 1u) << 1u) | (x & 1u);
    if site == 0u { return 0u; }
    if site == 3u { return 2u; }
    return 1u;
}

fn set_channel(value: vec4<f32>, channel: u32, sample: f32) -> vec4<f32> {
    var result = value;
    if channel == 0u { result.x = sample; }
    if channel == 1u { result.y = sample; }
    if channel == 2u { result.z = sample; }
    return result;
}

fn round_away(value: f32) -> i32 {
    if value < 0.0 { return i32(ceil(value - 0.5)); }
    return i32(floor(value + 0.5));
}

fn rgb_to_lab(rgb: vec3<f32>) -> vec4<i32> {
    let xyz = vec3<f32>(
        params.xyz_row_0.x * rgb.x + params.xyz_row_0.y * rgb.y + params.xyz_row_0.z * rgb.z,
        params.xyz_row_1.x * rgb.x + params.xyz_row_1.y * rgb.y + params.xyz_row_1.z * rgb.z,
        params.xyz_row_2.x * rgb.x + params.xyz_row_2.y * rgb.y + params.xyz_row_2.z * rgb.z
    );
    let indexes = vec3<u32>(clamp(xyz, vec3<f32>(0.0), vec3<f32>(1.0)) * 65535.0);
    let roots = vec3<f32>(
        f32(cbrt_lut[indexes.x]),
        f32(cbrt_lut[indexes.y]),
        f32(cbrt_lut[indexes.z])
    ) / 32767.0;
    return vec4<i32>(
        round_away((116.0 * roots.y - 16.0) * 64.0),
        round_away(500.0 * (roots.x - roots.y) * 64.0),
        round_away(200.0 * (roots.y - roots.z) * 64.0),
        0
    );
}

@compute @workgroup_size(16, 16, 1)
fn interpolate_green(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = id.x;
    let y = id.y;
    if x >= params.width || y >= params.band_height { return; }
    let index = pixel_index(x, y);
    let channel = cfa_rgb(x, y);
    let original = bayer[index];
    var horizontal = set_channel(vec4<f32>(0.0), channel, original);
    var vertical = horizontal;
    if channel == 1u {
        horizontal.y = original;
        vertical.y = original;
    } else {
        let dark = original < 0.02;
        if x > 1u && x + 2u < params.width {
            let left_1 = bayer[pixel_index(x - 1u, y)];
            let right_1 = bayer[pixel_index(x + 1u, y)];
            var green = 0.5 * (left_1 + right_1);
            if !dark {
                green = 0.25 * (
                    2.0 * (left_1 + original + right_1)
                    - bayer[pixel_index(x - 2u, y)]
                    - bayer[pixel_index(x + 2u, y)]
                );
            }
            horizontal.y = clamp(green, min(left_1, right_1), max(left_1, right_1));
        } else {
            horizontal.y = original;
        }
        let global_y = sensor_y(y);
        if y > 1u && y + 2u < params.band_height && global_y > 1u && global_y + 2u < params.full_height {
            let up_1 = bayer[pixel_index(x, y - 1u)];
            let down_1 = bayer[pixel_index(x, y + 1u)];
            var green = 0.5 * (up_1 + down_1);
            if !dark {
                green = 0.25 * (
                    2.0 * (up_1 + original + down_1)
                    - bayer[pixel_index(x, y - 2u)]
                    - bayer[pixel_index(x, y + 2u)]
                );
            }
            vertical.y = clamp(green, min(up_1, down_1), max(up_1, down_1));
        } else {
            vertical.y = original;
        }
    }
    candidates[direction_index(0u, x, y)] = horizontal;
    candidates[direction_index(1u, x, y)] = vertical;
}

@compute @workgroup_size(16, 16, 1)
fn interpolate_rb_lab(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = id.x;
    let y = id.y;
    if x >= params.width || y >= params.band_height { return; }
    let channel = cfa_rgb(x, y);
    let original = bayer[pixel_index(x, y)];
    let dark = original < 0.02;
    for (var direction = 0u; direction < 2u; direction += 1u) {
        let index = direction_index(direction, x, y);
        var rgb = candidates[index];
        if channel == 1u && x > 0u && x + 1u < params.width && y > 0u && y + 1u < params.band_height {
            let adjacent_channel = cfa_rgb(x, y + 1u);
            var horizontal = 0.5 * (
                bayer[pixel_index(x - 1u, y)] + bayer[pixel_index(x + 1u, y)]
            );
            var vertical = 0.5 * (
                bayer[pixel_index(x, y - 1u)] + bayer[pixel_index(x, y + 1u)]
            );
            if !dark {
                horizontal = rgb.y + 0.5 * (
                    bayer[pixel_index(x - 1u, y)] + bayer[pixel_index(x + 1u, y)]
                    - candidates[direction_index(direction, x - 1u, y)].y
                    - candidates[direction_index(direction, x + 1u, y)].y
                );
                vertical = rgb.y + 0.5 * (
                    bayer[pixel_index(x, y - 1u)] + bayer[pixel_index(x, y + 1u)]
                    - candidates[direction_index(direction, x, y - 1u)].y
                    - candidates[direction_index(direction, x, y + 1u)].y
                );
            }
            rgb = set_channel(rgb, 2u - adjacent_channel, clamp(horizontal, 0.0, 1.0));
            rgb = set_channel(rgb, adjacent_channel, clamp(vertical, 0.0, 1.0));
        } else if channel != 1u && x > 0u && x + 1u < params.width && y > 0u && y + 1u < params.band_height {
            var other = 0.25 * (
                bayer[pixel_index(x - 1u, y - 1u)]
                + bayer[pixel_index(x + 1u, y - 1u)]
                + bayer[pixel_index(x - 1u, y + 1u)]
                + bayer[pixel_index(x + 1u, y + 1u)]
            );
            if !dark {
                other = rgb.y + 0.25 * (
                    bayer[pixel_index(x - 1u, y - 1u)]
                    + bayer[pixel_index(x + 1u, y - 1u)]
                    + bayer[pixel_index(x - 1u, y + 1u)]
                    + bayer[pixel_index(x + 1u, y + 1u)]
                    - candidates[direction_index(direction, x - 1u, y - 1u)].y
                    - candidates[direction_index(direction, x + 1u, y - 1u)].y
                    - candidates[direction_index(direction, x - 1u, y + 1u)].y
                    - candidates[direction_index(direction, x + 1u, y + 1u)].y
                );
            }
            rgb = set_channel(rgb, 2u - channel, clamp(other, 0.0, 1.0));
        }
        rgb = set_channel(rgb, channel, original);
        candidates[index] = rgb;
        lab[index] = rgb_to_lab(rgb.xyz);
    }
}

fn squared_chroma_difference(first: vec4<i32>, second: vec4<i32>) -> u32 {
    let da = first.y - second.y;
    let db = first.z - second.z;
    return u32(da * da + db * db);
}

fn is_homogeneous(
    center: vec4<i32>,
    neighbor: vec4<i32>,
    lightness_epsilon: u32,
    chroma_epsilon: u32,
) -> u32 {
    let lightness = u32(abs(center.x - neighbor.x));
    let chroma = squared_chroma_difference(center, neighbor);
    return select(0u, 1u, lightness <= lightness_epsilon && chroma <= chroma_epsilon);
}

@compute @workgroup_size(16, 16, 1)
fn build_homogeneity(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = id.x;
    let y = id.y;
    if x >= params.width || y >= params.band_height { return; }
    for (var direction = 0u; direction < 2u; direction += 1u) {
        homogeneity[direction_index(direction, x, y)] = 0u;
    }
    if x < 2u || x + 2u >= params.width || y < 2u || y + 2u >= params.band_height { return; }

    let horizontal_center = lab[direction_index(0u, x, y)];
    let vertical_center = lab[direction_index(1u, x, y)];
    let horizontal_left = lab[direction_index(0u, x - 1u, y)];
    let horizontal_right = lab[direction_index(0u, x + 1u, y)];
    let vertical_up = lab[direction_index(1u, x, y - 1u)];
    let vertical_down = lab[direction_index(1u, x, y + 1u)];
    let lightness_epsilon = min(
        max(
            u32(abs(horizontal_center.x - horizontal_left.x)),
            u32(abs(horizontal_center.x - horizontal_right.x)),
        ),
        max(
            u32(abs(vertical_center.x - vertical_up.x)),
            u32(abs(vertical_center.x - vertical_down.x)),
        ),
    );
    let chroma_epsilon = min(
        max(
            squared_chroma_difference(horizontal_center, horizontal_left),
            squared_chroma_difference(horizontal_center, horizontal_right),
        ),
        max(
            squared_chroma_difference(vertical_center, vertical_up),
            squared_chroma_difference(vertical_center, vertical_down),
        ),
    );
    for (var direction = 0u; direction < 2u; direction += 1u) {
        let center = lab[direction_index(direction, x, y)];
        let count =
            is_homogeneous(
                center,
                lab[direction_index(direction, x - 1u, y)],
                lightness_epsilon,
                chroma_epsilon,
            )
            + is_homogeneous(
                center,
                lab[direction_index(direction, x + 1u, y)],
                lightness_epsilon,
                chroma_epsilon,
            )
            + is_homogeneous(
                center,
                lab[direction_index(direction, x, y - 1u)],
                lightness_epsilon,
                chroma_epsilon,
            )
            + is_homogeneous(
                center,
                lab[direction_index(direction, x, y + 1u)],
                lightness_epsilon,
                chroma_epsilon,
            );
        homogeneity[direction_index(direction, x, y)] = count;
    }
}

@compute @workgroup_size(16, 16, 1)
fn combine_directions(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = id.x;
    let y = id.y;
    if x >= params.width || y >= params.band_height { return; }
    if x < 3u || x + 3u >= params.width || y < 3u || y + 3u >= params.band_height {
        output[pixel_index(x, y)] = vec4<f32>(0.0);
        return;
    }
    var sums = vec2<u32>(0u);
    for (var dy = -1; dy <= 1; dy += 1) {
        for (var dx = -1; dx <= 1; dx += 1) {
            let sample_x = u32(i32(x) + dx);
            let sample_y = u32(i32(y) + dy);
            sums.x += homogeneity[direction_index(0u, sample_x, sample_y)];
            sums.y += homogeneity[direction_index(1u, sample_x, sample_y)];
        }
    }
    let horizontal = candidates[direction_index(0u, x, y)];
    let vertical = candidates[direction_index(1u, x, y)];
    var selected = 0.5 * (horizontal + vertical);
    if sums.x > sums.y + 1u { selected = horizontal; }
    if sums.y > sums.x + 1u { selected = vertical; }
    output[pixel_index(x, y)] = selected;
}
"#;

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::demosaic_ahd::ahd_demosaic_f32_owned;

    fn identity_camera_matrix() -> [[f32; 4]; 3] {
        [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ]
    }

    fn synthetic_bayer(width: usize, height: usize) -> Array3<f32> {
        Array3::from_shape_fn((height, width, 1), |(y, x, _)| {
            let base = if (x / 11 + y / 17) & 1 == 0 {
                0.13
            } else {
                0.79
            };
            let channel_scale = match cfa_rgb(y, x) {
                0 => 1.07,
                1 => 1.0,
                2 => 0.91,
                _ => unreachable!(),
            };
            (base * channel_scale + 0.04 * (x as f32 * 0.19).sin() + 0.03 * (y as f32 * 0.13).cos())
                .clamp(0.0, 1.0)
        })
    }

    #[test]
    fn metal_ahd_when_available_is_deterministic_and_preserves_measured_samples() {
        let Some(context) = GpuContext::new_blocking().unwrap() else {
            return;
        };
        let Some(engine) = GpuAhdEngine::new(Arc::new(context)).unwrap() else {
            return;
        };
        let bayer = synthetic_bayer(128, 96);
        let first = engine
            .execute(&bayer, &identity_camera_matrix())
            .unwrap()
            .image;
        let second = engine
            .execute(&bayer, &identity_camera_matrix())
            .unwrap()
            .image;
        assert_eq!(first, second);
        for y in 0..96 {
            for x in 0..128 {
                let channel = cfa_rgb(y, x);
                assert_eq!(first[[y, x, channel]], bayer[[y, x, 0]]);
            }
        }
    }

    #[test]
    fn metal_ahd_when_available_tracks_cpu_direction_selection() {
        let Some(context) = GpuContext::new_blocking().unwrap() else {
            return;
        };
        let Some(engine) = GpuAhdEngine::new(Arc::new(context)).unwrap() else {
            return;
        };
        let bayer = synthetic_bayer(192, 160);
        let cpu = ahd_demosaic_f32_owned(bayer.clone(), &identity_camera_matrix()).unwrap();
        let gpu = engine
            .execute(&bayer, &identity_camera_matrix())
            .unwrap()
            .image;
        let maximum_error = cpu
            .iter()
            .zip(gpu.iter())
            .map(|(left, right)| (left - right).abs())
            .fold(0.0f32, f32::max);
        let materially_different = cpu
            .iter()
            .zip(gpu.iter())
            .filter(|(left, right)| (*left - *right).abs() > METAL_AHD_NORMALIZED_PARITY_TOLERANCE)
            .count();
        assert!(
            maximum_error <= METAL_AHD_NORMALIZED_PARITY_TOLERANCE,
            "Metal AHD max error was {maximum_error}"
        );
        assert_eq!(materially_different, 0);
    }

    #[test]
    fn metal_ahd_when_available_matches_hdr_band_seams() {
        let Some(context) = GpuContext::new_blocking().unwrap() else {
            return;
        };
        let Some(engine) = GpuAhdEngine::new(Arc::new(context)).unwrap() else {
            return;
        };
        let width = 1536;
        let height = 700;
        let input_multiplier = 6.25;
        let mut bayer = synthetic_bayer(width, height);
        bayer.mapv_inplace(|value| value * input_multiplier);
        let range_scale =
            ahd_normalization_scale(bayer.iter().copied().fold(1.0f32, f32::max)).unwrap();
        let expected_scratch = engine.scratch_bytes(width, height).unwrap().unwrap();
        assert_eq!(
            expected_scratch,
            (width * (BAND_OUTPUT_ROWS + BAND_HALO * 2) * 112
                + std::mem::size_of::<AhdParams>()
                + CBR_T_ENTRIES * 4) as u64
        );

        let cpu = ahd_demosaic_f32_owned(bayer.clone(), &identity_camera_matrix()).unwrap();
        let gpu_output = engine.execute(&bayer, &identity_camera_matrix()).unwrap();
        assert_eq!(gpu_output.bands, 2);
        let maximum_error = cpu
            .iter()
            .zip(gpu_output.image.iter())
            .map(|(left, right)| (left - right).abs())
            .fold(0.0f32, f32::max);
        let first_material_difference =
            cpu.indexed_iter()
                .zip(gpu_output.image.iter())
                .find_map(|((index, left), right)| {
                    let error = (*left - *right).abs();
                    (error > METAL_AHD_NORMALIZED_PARITY_TOLERANCE * range_scale)
                        .then_some((index, *left, *right, error))
                });
        assert!(
            maximum_error <= METAL_AHD_NORMALIZED_PARITY_TOLERANCE * range_scale,
            "Metal AHD HDR seam max error was {maximum_error}; first={first_material_difference:?}"
        );
        for seam in [OUTPUT_BORDER + BAND_OUTPUT_ROWS] {
            for y in seam.saturating_sub(2)..=(seam + 2).min(height - 1) {
                for x in OUTPUT_BORDER..width - OUTPUT_BORDER {
                    for channel in 0..3 {
                        let error =
                            (cpu[[y, x, channel]] - gpu_output.image[[y, x, channel]]).abs();
                        assert!(
                            error <= METAL_AHD_NORMALIZED_PARITY_TOLERANCE * range_scale,
                            "seam ({x},{y},{channel}) error {error}"
                        );
                    }
                }
            }
        }
    }
}
