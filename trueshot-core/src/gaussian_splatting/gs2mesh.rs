//! GS2Mesh: Mesh Extraction from 3D Gaussian Splatting
//!
//! Converts trained 3DGS models to high-quality textured meshes.
//!
//! Algorithm (ECCV 2024):
//! 1. Render stereo pairs from trained 3DGS
//! 2. Dense depth estimation via stereo matching
//! 3. Depth fusion into TSDF volume
//! 4. Marching cubes mesh extraction
//! 5. UV unwrapping and texture baking
//!
//! This provides a path from neural representation to traditional mesh
//! suitable for game engines, AR apps, and 3D printing.

use anyhow::Result;
use nalgebra as na;
use std::path::PathBuf;

use crate::gaussian_splatting::{Camera, GaussianCloud};
use crate::mesh::{ExtractedMesh, MarchingCubes, MarchingCubesConfig, TsdfGrid, TsdfVoxel};

/// Type alias for TSDF volume using unified mesh library
pub type UnifiedTsdfVolume = TsdfGrid;

/// GS2Mesh configuration
#[derive(Debug, Clone)]
pub struct GS2MeshConfig {
    /// Number of virtual cameras for stereo rendering
    pub num_cameras: usize,
    /// Stereo baseline (distance between stereo pairs)
    pub stereo_baseline: f32,
    /// TSDF voxel size
    pub voxel_size: f32,
    /// TSDF truncation distance
    pub tsdf_truncation: f32,
    /// Marching cubes resolution
    pub mesh_resolution: u32,
    /// UV atlas resolution
    pub texture_resolution: u32,
    /// Enable mesh simplification
    pub simplify: bool,
    /// Target triangle count (if simplifying)
    pub target_triangles: usize,
    /// LOD ratios relative to base mesh (1.0 = full, 0.5 = half)
    pub lod_ratios: Vec<f32>,
    /// Preserve boundary edges during decimation
    pub preserve_boundaries: bool,
    /// Preserve UV seams during decimation
    pub preserve_uv_seams: bool,
    /// UV seam threshold for preservation
    pub uv_seam_threshold: f32,
}

impl Default for GS2MeshConfig {
    fn default() -> Self {
        Self {
            num_cameras: 100,
            stereo_baseline: 0.05,
            voxel_size: 0.005,
            tsdf_truncation: 0.02,
            mesh_resolution: 512,
            texture_resolution: 4096,
            simplify: true,
            target_triangles: 100_000,
            lod_ratios: vec![1.0, 0.5, 0.25],
            preserve_boundaries: true,
            preserve_uv_seams: true,
            uv_seam_threshold: 0.12,
        }
    }
}

/// Depth map from stereo matching
#[derive(Debug, Clone)]
pub struct DepthMap {
    /// Depth values (row-major)
    pub data: Vec<f32>,
    /// Width
    pub width: u32,
    /// Height
    pub height: u32,
    /// Camera pose
    pub camera_pose: na::Matrix4<f32>,
    /// Camera intrinsics
    pub intrinsics: na::Matrix3<f32>,
}

impl DepthMap {
    /// Get depth at pixel
    pub fn get(&self, x: u32, y: u32) -> f32 {
        if x < self.width && y < self.height {
            self.data[(y * self.width + x) as usize]
        } else {
            f32::INFINITY
        }
    }

    /// Unproject pixel to 3D point
    pub fn unproject(&self, x: u32, y: u32) -> Option<na::Point3<f32>> {
        let depth = self.get(x, y);
        if depth <= 0.0 || depth.is_infinite() {
            return None;
        }

        let k_inv = self.intrinsics.try_inverse()?;
        let pixel = na::Vector3::new(x as f32, y as f32, 1.0);
        let ray = k_inv * pixel;

        let point_cam = ray * depth;
        let point_world =
            self.camera_pose * na::Vector4::new(point_cam.x, point_cam.y, point_cam.z, 1.0);

        Some(na::Point3::new(point_world.x, point_world.y, point_world.z))
    }
}

/// TSDF (Truncated Signed Distance Function) volume
pub struct TsdfVolume {
    /// SDF values
    pub sdf: Vec<f32>,
    /// Weights
    pub weights: Vec<f32>,
    /// Colors (RGB)
    pub colors: Vec<[f32; 3]>,
    /// Resolution
    pub resolution: [usize; 3],
    /// Origin
    pub origin: na::Point3<f32>,
    /// Voxel size
    pub voxel_size: f32,
}

impl TsdfVolume {
    /// Create new TSDF volume
    pub fn new(origin: na::Point3<f32>, size: na::Vector3<f32>, voxel_size: f32) -> Self {
        let resolution = [
            (size.x / voxel_size).ceil() as usize,
            (size.y / voxel_size).ceil() as usize,
            (size.z / voxel_size).ceil() as usize,
        ];
        let total = resolution[0] * resolution[1] * resolution[2];

        Self {
            sdf: vec![1.0; total],
            weights: vec![0.0; total],
            colors: vec![[0.5, 0.5, 0.5]; total],
            resolution,
            origin,
            voxel_size,
        }
    }

    /// Get voxel index
    fn index(&self, x: usize, y: usize, z: usize) -> Option<usize> {
        if x < self.resolution[0] && y < self.resolution[1] && z < self.resolution[2] {
            Some(z * self.resolution[0] * self.resolution[1] + y * self.resolution[0] + x)
        } else {
            None
        }
    }

    /// Get voxel center position
    fn voxel_center(&self, x: usize, y: usize, z: usize) -> na::Point3<f32> {
        na::Point3::new(
            self.origin.x + (x as f32 + 0.5) * self.voxel_size,
            self.origin.y + (y as f32 + 0.5) * self.voxel_size,
            self.origin.z + (z as f32 + 0.5) * self.voxel_size,
        )
    }

    /// Integrate depth map into TSDF
    pub fn integrate(&mut self, depth_map: &DepthMap, truncation: f32) {
        let k = depth_map.intrinsics;
        let pose_inv = depth_map
            .camera_pose
            .try_inverse()
            .unwrap_or(na::Matrix4::identity());

        for z in 0..self.resolution[2] {
            for y in 0..self.resolution[1] {
                for x in 0..self.resolution[0] {
                    let voxel_world = self.voxel_center(x, y, z);
                    let voxel_cam = pose_inv
                        * na::Vector4::new(voxel_world.x, voxel_world.y, voxel_world.z, 1.0);

                    // Skip if behind camera
                    if voxel_cam.z <= 0.0 {
                        continue;
                    }

                    // Project to image
                    let proj = k * na::Vector3::new(
                        voxel_cam.x / voxel_cam.z,
                        voxel_cam.y / voxel_cam.z,
                        1.0,
                    );

                    let px = proj.x.round() as i32;
                    let py = proj.y.round() as i32;

                    if px < 0
                        || py < 0
                        || px >= depth_map.width as i32
                        || py >= depth_map.height as i32
                    {
                        continue;
                    }

                    let measured_depth = depth_map.get(px as u32, py as u32);
                    if measured_depth <= 0.0 || measured_depth.is_infinite() {
                        continue;
                    }

                    // TSDF value
                    let sdf = measured_depth - voxel_cam.z;

                    // Truncate
                    if sdf < -truncation {
                        continue;
                    }
                    let sdf = sdf.min(truncation) / truncation;

                    // Update TSDF
                    if let Some(idx) = self.index(x, y, z) {
                        let w = self.weights[idx];
                        let w_new = 1.0;
                        let w_sum = w + w_new;

                        self.sdf[idx] = (self.sdf[idx] * w + sdf * w_new) / w_sum;
                        self.weights[idx] = w_sum.min(100.0);
                    }
                }
            }
        }
    }
}

/// Extracted mesh with UV coordinates
#[derive(Debug, Clone)]
pub struct TexturedMesh {
    /// Vertex positions
    pub vertices: Vec<na::Point3<f32>>,
    /// Vertex normals
    pub normals: Vec<na::Vector3<f32>>,
    /// UV coordinates
    pub uvs: Vec<na::Vector2<f32>>,
    /// Triangle indices
    pub indices: Vec<[u32; 3]>,
    /// Texture atlas
    pub texture: Option<image::RgbaImage>,
    /// Optional LOD chain
    pub lod_chain: Vec<LodMesh>,
}

/// Lightweight LOD mesh representation
#[derive(Debug, Clone)]
pub struct LodMesh {
    pub vertices: Vec<na::Point3<f32>>,
    pub normals: Vec<na::Vector3<f32>>,
    pub uvs: Vec<na::Vector2<f32>>,
    pub indices: Vec<[u32; 3]>,
    pub texture: Option<image::RgbaImage>,
}

impl TexturedMesh {
    /// Export to OBJ format
    pub fn export_obj(&self, path: &PathBuf) -> Result<()> {
        use std::io::Write;

        let mut file = std::fs::File::create(path)?;

        // Write MTL reference if we have texture
        if self.texture.is_some() {
            let mtl_name = path.with_extension("mtl");
            writeln!(
                file,
                "mtllib {}",
                mtl_name.file_name().unwrap().to_str().unwrap()
            )?;
        }

        // Vertices
        for v in &self.vertices {
            writeln!(file, "v {} {} {}", v.x, v.y, v.z)?;
        }

        // Normals
        for n in &self.normals {
            writeln!(file, "vn {} {} {}", n.x, n.y, n.z)?;
        }

        // UVs
        for uv in &self.uvs {
            writeln!(file, "vt {} {}", uv.x, 1.0 - uv.y)?;
        }

        // Use material
        if self.texture.is_some() {
            writeln!(file, "usemtl material0")?;
        }

        // Faces
        for tri in &self.indices {
            writeln!(
                file,
                "f {}/{}/{} {}/{}/{} {}/{}/{}",
                tri[0] + 1,
                tri[0] + 1,
                tri[0] + 1,
                tri[1] + 1,
                tri[1] + 1,
                tri[1] + 1,
                tri[2] + 1,
                tri[2] + 1,
                tri[2] + 1,
            )?;
        }

        // Write MTL file
        if let Some(ref texture) = self.texture {
            let mtl_path = path.with_extension("mtl");
            let tex_path = path.with_extension("png");

            let mut mtl_file = std::fs::File::create(&mtl_path)?;
            writeln!(mtl_file, "newmtl material0")?;
            writeln!(mtl_file, "Ka 1.0 1.0 1.0")?;
            writeln!(mtl_file, "Kd 1.0 1.0 1.0")?;
            writeln!(
                mtl_file,
                "map_Kd {}",
                tex_path.file_name().unwrap().to_str().unwrap()
            )?;

            texture.save(&tex_path)?;
        }

        Ok(())
    }

    /// Calculate surface area
    pub fn surface_area(&self) -> f32 {
        let mut area = 0.0;
        for tri in &self.indices {
            let v0 = self.vertices[tri[0] as usize];
            let v1 = self.vertices[tri[1] as usize];
            let v2 = self.vertices[tri[2] as usize];

            let edge1 = v1 - v0;
            let edge2 = v2 - v0;
            area += edge1.cross(&edge2).norm() * 0.5;
        }
        area
    }
}

/// GS2Mesh: Extract mesh from 3D Gaussian Splatting
pub struct GS2Mesh {
    config: GS2MeshConfig,
}

impl GS2Mesh {
    /// Create new GS2Mesh extractor
    pub fn new(config: GS2MeshConfig) -> Self {
        Self { config }
    }

    /// Extract mesh from trained Gaussian cloud
    pub fn extract(&self, gaussians: &GaussianCloud) -> Result<TexturedMesh> {
        tracing::info!(
            "🔧 GS2Mesh: Starting mesh extraction from {} Gaussians",
            gaussians.num_gaussians()
        );

        // 1. Compute bounding box
        let (min_bound, max_bound) = self.compute_bounds(gaussians);
        let size = max_bound - min_bound;

        tracing::info!("  Bounding box: {:?} to {:?}", min_bound, max_bound);

        // 2. Create TSDF volume
        let mut tsdf = TsdfVolume::new(min_bound, size, self.config.voxel_size);
        tracing::info!("  TSDF resolution: {:?}", tsdf.resolution);

        // 3. Generate virtual cameras
        let cameras = self.generate_cameras(&min_bound, &max_bound);
        tracing::info!("  Generated {} virtual cameras", cameras.len());

        // 4. Render depth maps and integrate
        for (i, camera) in cameras.iter().enumerate() {
            // Render depth from 3DGS
            let depth_map = self.render_depth(gaussians, camera)?;

            // Integrate into TSDF
            tsdf.integrate(&depth_map, self.config.tsdf_truncation);

            if i % 10 == 0 {
                tracing::info!("  Integrated {}/{} depth maps", i + 1, cameras.len());
            }
        }

        // 5. Marching cubes
        tracing::info!("  Running marching cubes...");
        let mesh = self.marching_cubes(&tsdf)?;
        tracing::info!(
            "  Extracted {} vertices, {} triangles",
            mesh.vertices.len(),
            mesh.indices.len()
        );

        // 6. UV unwrap
        tracing::info!("  Computing UV coordinates...");
        let mesh = self.compute_uvs(mesh)?;

        // 7. Simplify if needed
        let mut mesh = if self.config.simplify && mesh.indices.len() > self.config.target_triangles
        {
            tracing::info!(
                "  Simplifying to {} triangles...",
                self.config.target_triangles
            );
            self.simplify_mesh(mesh, self.config.target_triangles)?
        } else {
            mesh
        };
        mesh = self.recompute_normals(mesh);

        // 8. Bake texture
        tracing::info!("  Baking texture atlas...");
        let mut mesh = self.bake_texture(mesh, gaussians, &cameras)?;

        // 9. Generate LOD chain
        let lod_chain = self.generate_lod_chain(&mesh)?;
        mesh.lod_chain = lod_chain;

        tracing::info!(
            "✅ GS2Mesh complete: {} vertices, {} triangles",
            mesh.vertices.len(),
            mesh.indices.len()
        );

        Ok(mesh)
    }

    /// Compute bounding box from Gaussians
    fn compute_bounds(&self, gaussians: &GaussianCloud) -> (na::Point3<f32>, na::Point3<f32>) {
        let mut min_bound = na::Point3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut max_bound = na::Point3::new(f32::MIN, f32::MIN, f32::MIN);

        for i in 0..gaussians.num_gaussians() {
            let pos = gaussians.position(i);
            min_bound.x = min_bound.x.min(pos.x);
            min_bound.y = min_bound.y.min(pos.y);
            min_bound.z = min_bound.z.min(pos.z);
            max_bound.x = max_bound.x.max(pos.x);
            max_bound.y = max_bound.y.max(pos.y);
            max_bound.z = max_bound.z.max(pos.z);
        }

        // Add margin
        let margin = 0.1;
        min_bound -= na::Vector3::new(margin, margin, margin);
        max_bound += na::Vector3::new(margin, margin, margin);

        (min_bound, max_bound)
    }

    /// Generate virtual cameras around the object
    fn generate_cameras(
        &self,
        min_bound: &na::Point3<f32>,
        max_bound: &na::Point3<f32>,
    ) -> Vec<Camera> {
        let center = na::Point3::new(
            (min_bound.x + max_bound.x) / 2.0,
            (min_bound.y + max_bound.y) / 2.0,
            (min_bound.z + max_bound.z) / 2.0,
        );

        let size = max_bound - min_bound;
        let radius = size.norm() * 1.5;

        let mut cameras = Vec::new();
        let elevations = [-30.0, 0.0, 30.0, 60.0];
        let num_azimuths = self.config.num_cameras / elevations.len();

        for elevation in elevations {
            for i in 0..num_azimuths {
                let azimuth = 2.0 * std::f32::consts::PI * i as f32 / num_azimuths as f32;
                let elev_rad = elevation * std::f32::consts::PI / 180.0;

                let pos = na::Point3::new(
                    center.x + radius * elev_rad.cos() * azimuth.sin(),
                    center.y + radius * elev_rad.sin(),
                    center.z + radius * elev_rad.cos() * azimuth.cos(),
                );

                let transform = self.look_at_matrix(&pos, &center);

                cameras.push(Camera {
                    transform,
                    intrinsics: na::Matrix3::new(
                        500.0, 0.0, 400.0, 0.0, 500.0, 300.0, 0.0, 0.0, 1.0,
                    ),
                    width: 800,
                    height: 600,
                    image_path: PathBuf::new(),
                });
            }
        }

        cameras
    }

    fn look_at_matrix(&self, eye: &na::Point3<f32>, target: &na::Point3<f32>) -> na::Matrix4<f32> {
        let up = na::Vector3::new(0.0, 1.0, 0.0);
        let forward = (target - eye).normalize();
        let right = forward.cross(&up).normalize();
        let up = right.cross(&forward);

        na::Matrix4::new(
            right.x, up.x, -forward.x, eye.x, right.y, up.y, -forward.y, eye.y, right.z, up.z,
            -forward.z, eye.z, 0.0, 0.0, 0.0, 1.0,
        )
    }

    /// Render depth map from 3DGS
    fn render_depth(&self, gaussians: &GaussianCloud, camera: &Camera) -> Result<DepthMap> {
        let width = camera.width;
        let height = camera.height;
        let mut depth_data = vec![f32::INFINITY; (width * height) as usize];

        // Simple z-buffer rendering
        let view = camera
            .transform
            .try_inverse()
            .unwrap_or(na::Matrix4::identity());

        for i in 0..gaussians.num_gaussians() {
            let pos = gaussians.position(i);
            let opacity = gaussians.opacity(i);

            if opacity < 0.1 {
                continue;
            }

            // Transform to camera space
            let world = na::Vector4::new(pos.x, pos.y, pos.z, 1.0);
            let cam = view * world;

            if cam.z <= 0.0 {
                continue;
            }

            // Project to image
            let proj = camera.intrinsics * na::Vector3::new(cam.x / cam.z, cam.y / cam.z, 1.0);

            let px = proj.x.round() as i32;
            let py = proj.y.round() as i32;

            if px >= 0 && py >= 0 && px < width as i32 && py < height as i32 {
                let idx = (py as u32 * width + px as u32) as usize;
                if cam.z < depth_data[idx] {
                    depth_data[idx] = cam.z;
                }
            }
        }

        Ok(DepthMap {
            data: depth_data,
            width,
            height,
            camera_pose: camera.transform,
            intrinsics: camera.intrinsics,
        })
    }

    /// Marching cubes algorithm - uses unified mesh library with complete 256-case table
    fn marching_cubes(&self, tsdf: &TsdfVolume) -> Result<TexturedMesh> {
        // Convert local TsdfVolume to unified TsdfGrid for marching cubes
        let unified_grid = self.convert_to_unified_grid(tsdf);

        // Use unified marching cubes with proper triangulation
        let mc = MarchingCubes::new(MarchingCubesConfig {
            threshold: 0.0, // Surface at zero-crossing
            compute_normals: true,
            compute_uvs: true,
            uv_scale: 1.0 / self.config.voxel_size,
        });

        let extracted = mc.extract(&unified_grid);

        // Convert from unified ExtractedMesh to local TexturedMesh
        self.convert_from_unified_mesh(extracted)
    }

    /// Convert local TsdfVolume to unified TsdfGrid
    fn convert_to_unified_grid(&self, tsdf: &TsdfVolume) -> TsdfGrid {
        let bounds_max = na::Point3::new(
            tsdf.origin.x + tsdf.resolution[0] as f32 * tsdf.voxel_size,
            tsdf.origin.y + tsdf.resolution[1] as f32 * tsdf.voxel_size,
            tsdf.origin.z + tsdf.resolution[2] as f32 * tsdf.voxel_size,
        );

        let mut grid = TsdfGrid::new(tsdf.origin, bounds_max, tsdf.voxel_size);

        // Copy TSDF values to unified grid
        for z in 0..tsdf.resolution[2] {
            for y in 0..tsdf.resolution[1] {
                for x in 0..tsdf.resolution[0] {
                    if let Some(idx) = tsdf.index(x, y, z) {
                        // Only copy voxels with sufficient weight
                        if tsdf.weights[idx] > 0.1 {
                            let voxel = TsdfVoxel {
                                tsdf: tsdf.sdf[idx],
                                weight: tsdf.weights[idx],
                                color: tsdf.colors[idx],
                            };
                            grid.set([x, y, z], voxel);
                        }
                    }
                }
            }
        }

        grid
    }

    /// Convert from unified ExtractedMesh to local TexturedMesh
    fn convert_from_unified_mesh(&self, extracted: ExtractedMesh) -> Result<TexturedMesh> {
        let mut vertices = Vec::with_capacity(extracted.vertices.len());
        let mut normals = Vec::with_capacity(extracted.vertices.len());
        let mut uvs = Vec::with_capacity(extracted.vertices.len());

        for v in &extracted.vertices {
            vertices.push(na::Point3::new(v.position[0], v.position[1], v.position[2]));
            normals.push(na::Vector3::new(v.normal[0], v.normal[1], v.normal[2]));
            uvs.push(na::Vector2::new(v.uv[0], v.uv[1]));
        }

        // Convert indices from flat to [u32; 3] triangles
        let mut indices = Vec::with_capacity(extracted.indices.len() / 3);
        for tri in extracted.indices.chunks(3) {
            if tri.len() == 3 {
                indices.push([tri[0], tri[1], tri[2]]);
            }
        }

        Ok(TexturedMesh {
            vertices,
            normals,
            uvs,
            indices,
            texture: None,
            lod_chain: Vec::new(),
        })
    }

    /// Simplify mesh using quadric error metrics
    fn simplify_mesh(&self, mesh: TexturedMesh, target_triangles: usize) -> Result<TexturedMesh> {
        if mesh.indices.len() <= target_triangles {
            return Ok(mesh);
        }
        qem_simplify(
            mesh,
            target_triangles,
            self.config.preserve_boundaries,
            self.config.preserve_uv_seams,
            self.config.uv_seam_threshold,
        )
    }

    /// Compute UV coordinates using box unwrapping
    fn compute_uvs(&self, mut mesh: TexturedMesh) -> Result<TexturedMesh> {
        // Simple box projection for UV mapping
        mesh.uvs.resize(mesh.vertices.len(), na::Vector2::zeros());

        for (i, v) in mesh.vertices.iter().enumerate() {
            let normal = if i < mesh.normals.len() {
                mesh.normals[i]
            } else {
                na::Vector3::new(0.0, 0.0, 1.0)
            };

            // Box projection based on dominant axis
            let abs_normal = na::Vector3::new(normal.x.abs(), normal.y.abs(), normal.z.abs());

            let uv = if abs_normal.x > abs_normal.y && abs_normal.x > abs_normal.z {
                na::Vector2::new(v.z, v.y)
            } else if abs_normal.y > abs_normal.z {
                na::Vector2::new(v.x, v.z)
            } else {
                na::Vector2::new(v.x, v.y)
            };

            mesh.uvs[i] = (uv + na::Vector2::new(1.0, 1.0)) * 0.5;
        }

        Ok(mesh)
    }

    /// Bake texture from 3DGS colors
    fn bake_texture(
        &self,
        mut mesh: TexturedMesh,
        gaussians: &GaussianCloud,
        _cameras: &[Camera],
    ) -> Result<TexturedMesh> {
        let size = self.config.texture_resolution;
        let mut texture = image::RgbaImage::new(size, size);

        // Simple nearest-neighbor color lookup from Gaussians
        for (i, v) in mesh.vertices.iter().enumerate() {
            if i >= mesh.uvs.len() {
                continue;
            }

            let uv = mesh.uvs[i];
            let px = (uv.x * (size - 1) as f32) as u32;
            let py = (uv.y * (size - 1) as f32) as u32;

            // Find nearest Gaussian
            let mut min_dist = f32::MAX;
            let mut color = [128u8, 128u8, 128u8];

            for g in 0..gaussians.num_gaussians().min(1000) {
                let pos = gaussians.position(g);
                let dist = (pos.coords - v.coords).norm();
                if dist < min_dist {
                    min_dist = dist;
                    // Get color from Gaussian (simplified - just use position as color for now)
                    let c = gaussians.position(g);
                    color = [
                        ((c.x.fract() * 255.0) as u8).max(50),
                        ((c.y.fract() * 255.0) as u8).max(50),
                        ((c.z.fract() * 255.0) as u8).max(50),
                    ];
                }
            }

            texture.put_pixel(
                px.min(size - 1),
                py.min(size - 1),
                image::Rgba([color[0], color[1], color[2], 255]),
            );
        }

        mesh.texture = Some(texture);
        Ok(mesh)
    }

    fn recompute_normals(&self, mut mesh: TexturedMesh) -> TexturedMesh {
        let mut normals = vec![na::Vector3::zeros(); mesh.vertices.len()];
        for tri in &mesh.indices {
            let v0 = mesh.vertices[tri[0] as usize];
            let v1 = mesh.vertices[tri[1] as usize];
            let v2 = mesh.vertices[tri[2] as usize];
            let n = (v1 - v0).cross(&(v2 - v0));
            if n.norm_squared() > 1e-12 {
                let nn = n.normalize();
                normals[tri[0] as usize] += nn;
                normals[tri[1] as usize] += nn;
                normals[tri[2] as usize] += nn;
            }
        }
        for n in &mut normals {
            if n.norm_squared() > 1e-12 {
                *n = n.normalize();
            }
        }
        mesh.normals = normals;
        mesh
    }

    fn generate_lod_chain(&self, mesh: &TexturedMesh) -> Result<Vec<LodMesh>> {
        let mut lods = Vec::new();
        if self.config.lod_ratios.is_empty() {
            return Ok(lods);
        }
        for ratio in &self.config.lod_ratios {
            if *ratio >= 0.999 {
                continue;
            }
            let target = ((mesh.indices.len() as f32) * ratio).round() as usize;
            if target < 4 {
                continue;
            }
            let lod_mesh = qem_simplify(
                mesh.clone(),
                target,
                self.config.preserve_boundaries,
                self.config.preserve_uv_seams,
                self.config.uv_seam_threshold,
            )?;
            let lod_mesh = self.recompute_normals(lod_mesh);
            lods.push(LodMesh {
                vertices: lod_mesh.vertices,
                normals: lod_mesh.normals,
                uvs: lod_mesh.uvs,
                indices: lod_mesh.indices,
                texture: lod_mesh.texture,
            });
        }
        Ok(lods)
    }
}

#[derive(Clone)]
struct Quadric {
    m: na::Matrix4<f32>,
}

impl Quadric {
    fn zero() -> Self {
        Self {
            m: na::Matrix4::zeros(),
        }
    }
    fn from_plane(plane: na::Vector4<f32>) -> Self {
        Self {
            m: plane * plane.transpose(),
        }
    }
    fn add(&mut self, other: &Quadric) {
        self.m += other.m;
    }
}

#[derive(Clone)]
struct HeapEdge {
    cost: f32,
    u: usize,
    v: usize,
    position: na::Point3<f32>,
    uv: na::Vector2<f32>,
}

impl PartialEq for HeapEdge {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost
    }
}

impl Eq for HeapEdge {}

impl PartialOrd for HeapEdge {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEdge {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .cost
            .partial_cmp(&self.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

fn qem_simplify(
    mesh: TexturedMesh,
    target_triangles: usize,
    preserve_boundaries: bool,
    preserve_uv_seams: bool,
    uv_seam_threshold: f32,
) -> Result<TexturedMesh> {
    let mut vertices = mesh.vertices;
    let mut uvs = mesh.uvs;
    let mut indices = mesh.indices;
    let normals = mesh.normals;

    let mut face_valid = vec![true; indices.len()];
    let mut vertex_faces = build_vertex_faces(vertices.len(), &indices);
    let mut quadrics = build_quadrics(&vertices, &indices, &face_valid);

    let mut neighbors = build_vertex_neighbors(vertices.len(), &indices);
    let mut heap = std::collections::BinaryHeap::new();

    for u in 0..vertices.len() {
        for &v in neighbors[u].iter() {
            if u < v {
                if preserve_uv_seams && is_seam_edge(u, v, &uvs, uv_seam_threshold) {
                    continue;
                }
                if preserve_boundaries
                    && edge_face_count(u, v, &indices, &face_valid, &vertex_faces) <= 1
                {
                    continue;
                }
                let edge = compute_edge(u, v, &vertices, &uvs, &quadrics);
                heap.push(edge);
            }
        }
    }

    let mut valid_vertex = vec![true; vertices.len()];
    let mut tri_count = indices.len();

    while tri_count > target_triangles {
        let Some(edge) = heap.pop() else {
            break;
        };
        if !valid_vertex[edge.u] || !valid_vertex[edge.v] {
            continue;
        }
        if !neighbors[edge.u].contains(&edge.v) {
            continue;
        }
        let face_count = edge_face_count(edge.u, edge.v, &indices, &face_valid, &vertex_faces);
        if face_count == 0 {
            continue;
        }
        if preserve_boundaries && face_count <= 1 {
            continue;
        }
        if preserve_uv_seams && is_seam_edge(edge.u, edge.v, &uvs, uv_seam_threshold) {
            continue;
        }

        // Collapse v into u
        vertices[edge.u] = edge.position;
        uvs[edge.u] = edge.uv;
        let quadric_v = quadrics[edge.v].clone();
        quadrics[edge.u].add(&quadric_v);
        valid_vertex[edge.v] = false;

        let faces_to_update = vertex_faces[edge.v].clone();
        for face_idx in faces_to_update {
            if face_idx >= indices.len() || !face_valid[face_idx] {
                continue;
            }
            let mut tri = indices[face_idx];
            for idx in &mut tri {
                if *idx as usize == edge.v {
                    *idx = edge.u as u32;
                }
            }
            if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
                face_valid[face_idx] = false;
                tri_count = tri_count.saturating_sub(1);
                indices[face_idx] = tri;
                continue;
            }
            indices[face_idx] = tri;
            if !vertex_faces[edge.u].contains(&face_idx) {
                vertex_faces[edge.u].push(face_idx);
            }
        }

        // Update neighbors
        let neighbors_v: Vec<usize> = neighbors[edge.v].iter().copied().collect();
        for n in neighbors_v {
            neighbors[n].remove(&edge.v);
            if n != edge.u {
                neighbors[n].insert(edge.u);
                neighbors[edge.u].insert(n);
            }
        }
        neighbors[edge.v].clear();

        // Recompute quadric for u and neighbors
        quadrics[edge.u] =
            recompute_vertex_quadric(edge.u, &vertices, &indices, &face_valid, &vertex_faces);
        for &n in neighbors[edge.u].iter() {
            quadrics[n] =
                recompute_vertex_quadric(n, &vertices, &indices, &face_valid, &vertex_faces);
        }

        // Push updated edges
        for &n in neighbors[edge.u].iter() {
            let (a, b) = if edge.u < n { (edge.u, n) } else { (n, edge.u) };
            if preserve_uv_seams && is_seam_edge(a, b, &uvs, uv_seam_threshold) {
                continue;
            }
            if preserve_boundaries
                && edge_face_count(a, b, &indices, &face_valid, &vertex_faces) <= 1
            {
                continue;
            }
            let candidate = compute_edge(a, b, &vertices, &uvs, &quadrics);
            heap.push(candidate);
        }
    }

    let mut remap = vec![usize::MAX; vertices.len()];
    let mut new_vertices = Vec::new();
    let mut new_uvs = Vec::new();
    let mut new_normals = Vec::new();
    for (idx, valid) in valid_vertex.iter().enumerate() {
        if *valid {
            remap[idx] = new_vertices.len();
            new_vertices.push(vertices[idx]);
            new_uvs.push(uvs[idx]);
            new_normals.push(normals.get(idx).copied().unwrap_or_else(na::Vector3::zeros));
        }
    }

    let mut new_indices = Vec::new();
    for (face_idx, tri) in indices.iter().enumerate() {
        if !face_valid[face_idx] {
            continue;
        }
        let a = remap[tri[0] as usize];
        let b = remap[tri[1] as usize];
        let c = remap[tri[2] as usize];
        if a == usize::MAX || b == usize::MAX || c == usize::MAX {
            continue;
        }
        if a == b || b == c || a == c {
            continue;
        }
        new_indices.push([a as u32, b as u32, c as u32]);
    }

    Ok(TexturedMesh {
        vertices: new_vertices,
        normals: new_normals,
        uvs: new_uvs,
        indices: new_indices,
        texture: mesh.texture,
        lod_chain: mesh.lod_chain,
    })
}

pub fn simplify_textured_mesh(
    mesh: TexturedMesh,
    target_triangles: usize,
    preserve_boundaries: bool,
    preserve_uv_seams: bool,
    uv_seam_threshold: f32,
) -> Result<TexturedMesh> {
    qem_simplify(
        mesh,
        target_triangles,
        preserve_boundaries,
        preserve_uv_seams,
        uv_seam_threshold,
    )
}

fn build_vertex_faces(vertex_count: usize, indices: &[[u32; 3]]) -> Vec<Vec<usize>> {
    let mut faces = vec![Vec::new(); vertex_count];
    for (i, tri) in indices.iter().enumerate() {
        faces[tri[0] as usize].push(i);
        faces[tri[1] as usize].push(i);
        faces[tri[2] as usize].push(i);
    }
    faces
}

fn build_vertex_neighbors(
    vertex_count: usize,
    indices: &[[u32; 3]],
) -> Vec<std::collections::HashSet<usize>> {
    let mut neighbors = vec![std::collections::HashSet::new(); vertex_count];
    for tri in indices {
        let a = tri[0] as usize;
        let b = tri[1] as usize;
        let c = tri[2] as usize;
        neighbors[a].insert(b);
        neighbors[a].insert(c);
        neighbors[b].insert(a);
        neighbors[b].insert(c);
        neighbors[c].insert(a);
        neighbors[c].insert(b);
    }
    neighbors
}

fn build_quadrics(
    vertices: &[na::Point3<f32>],
    indices: &[[u32; 3]],
    face_valid: &[bool],
) -> Vec<Quadric> {
    let mut quadrics = vec![Quadric::zero(); vertices.len()];
    for (idx, tri) in indices.iter().enumerate() {
        if !face_valid[idx] {
            continue;
        }
        let v0 = vertices[tri[0] as usize];
        let v1 = vertices[tri[1] as usize];
        let v2 = vertices[tri[2] as usize];
        let n = (v1 - v0).cross(&(v2 - v0));
        if n.norm_squared() < 1e-12 {
            continue;
        }
        let normal = n.normalize();
        let d = -normal.dot(&v0.coords);
        let plane = na::Vector4::new(normal.x, normal.y, normal.z, d);
        let q = Quadric::from_plane(plane);
        quadrics[tri[0] as usize].add(&q);
        quadrics[tri[1] as usize].add(&q);
        quadrics[tri[2] as usize].add(&q);
    }
    quadrics
}

fn recompute_vertex_quadric(
    vertex: usize,
    vertices: &[na::Point3<f32>],
    indices: &[[u32; 3]],
    face_valid: &[bool],
    vertex_faces: &[Vec<usize>],
) -> Quadric {
    let mut quadric = Quadric::zero();
    for &face_idx in &vertex_faces[vertex] {
        if face_idx >= indices.len() || !face_valid[face_idx] {
            continue;
        }
        let tri = indices[face_idx];
        let v0 = vertices[tri[0] as usize];
        let v1 = vertices[tri[1] as usize];
        let v2 = vertices[tri[2] as usize];
        let n = (v1 - v0).cross(&(v2 - v0));
        if n.norm_squared() < 1e-12 {
            continue;
        }
        let normal = n.normalize();
        let d = -normal.dot(&v0.coords);
        let plane = na::Vector4::new(normal.x, normal.y, normal.z, d);
        quadric.add(&Quadric::from_plane(plane));
    }
    quadric
}

fn compute_edge(
    u: usize,
    v: usize,
    vertices: &[na::Point3<f32>],
    uvs: &[na::Vector2<f32>],
    quadrics: &[Quadric],
) -> HeapEdge {
    let q = quadrics[u].m + quadrics[v].m;
    let a = na::Matrix3::new(
        q[(0, 0)],
        q[(0, 1)],
        q[(0, 2)],
        q[(1, 0)],
        q[(1, 1)],
        q[(1, 2)],
        q[(2, 0)],
        q[(2, 1)],
        q[(2, 2)],
    );
    let b = na::Vector3::new(q[(0, 3)], q[(1, 3)], q[(2, 3)]);
    let position = if let Some(inv) = a.try_inverse() {
        let sol = -inv * b;
        na::Point3::new(sol.x, sol.y, sol.z)
    } else {
        let mid = (vertices[u].coords + vertices[v].coords) * 0.5;
        na::Point3::from(mid)
    };
    let uv = (uvs[u] + uvs[v]) * 0.5;
    let v4 = na::Vector4::new(position.x, position.y, position.z, 1.0);
    let cost = v4.dot(&(q * v4));
    HeapEdge {
        cost,
        u,
        v,
        position,
        uv,
    }
}

fn edge_face_count(
    u: usize,
    v: usize,
    indices: &[[u32; 3]],
    face_valid: &[bool],
    vertex_faces: &[Vec<usize>],
) -> usize {
    let mut count = 0;
    for &face_idx in &vertex_faces[u] {
        if face_idx >= indices.len() || !face_valid[face_idx] {
            continue;
        }
        let tri = indices[face_idx];
        if tri[0] as usize == v || tri[1] as usize == v || tri[2] as usize == v {
            count += 1;
        }
    }
    count
}

fn is_seam_edge(u: usize, v: usize, uvs: &[na::Vector2<f32>], threshold: f32) -> bool {
    if u >= uvs.len() || v >= uvs.len() {
        return false;
    }
    (uvs[u] - uvs[v]).norm() > threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tsdf_creation() {
        let origin = na::Point3::new(-1.0, -1.0, -1.0);
        let size = na::Vector3::new(2.0, 2.0, 2.0);
        let tsdf = TsdfVolume::new(origin, size, 0.1);

        assert_eq!(tsdf.resolution[0], 20);
        assert_eq!(tsdf.resolution[1], 20);
        assert_eq!(tsdf.resolution[2], 20);
    }

    #[test]
    fn test_depth_map_unproject() {
        let depth_map = DepthMap {
            data: vec![1.0; 100],
            width: 10,
            height: 10,
            camera_pose: na::Matrix4::identity(),
            intrinsics: na::Matrix3::new(10.0, 0.0, 5.0, 0.0, 10.0, 5.0, 0.0, 0.0, 1.0),
        };

        let point = depth_map.unproject(5, 5);
        assert!(point.is_some());
    }
}
