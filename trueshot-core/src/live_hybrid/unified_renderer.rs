//! Unified Renderer
//!
//! Renders hybrid scenes containing:
//! - 4D Gaussian Splatting objects
//! - Textured meshes
//! - Animated avatars
//!
//! Uses depth compositing to correctly blend all representation types.

use nalgebra as na;

use super::scene_graph::{HybridScene, MeshData, ObjectRepresentation, Vertex};
use super::transitions::{TransitionBlend, TransitionManager};
use crate::gaussian_splatting::rasterizer_4d::{
    GpuRasterizer4D, Raster4DConfig, RasterCamera, RenderedFrame4D,
};

/// Camera for unified rendering
#[derive(Clone, Debug)]
pub struct UnifiedCamera {
    /// View matrix (world to camera)
    pub view: na::Matrix4<f32>,
    /// Projection matrix
    pub projection: na::Matrix4<f32>,
    /// Output width
    pub width: u32,
    /// Output height
    pub height: u32,
}

impl UnifiedCamera {
    pub fn perspective(
        fov_y: f32,
        aspect: f32,
        near: f32,
        far: f32,
        position: na::Point3<f32>,
        target: na::Point3<f32>,
    ) -> Self {
        let projection = na::Perspective3::new(aspect, fov_y, near, far).to_homogeneous();
        let view =
            na::Isometry3::look_at_rh(&position, &target, &na::Vector3::y()).to_homogeneous();

        Self {
            view,
            projection,
            width: (1920.0 * aspect) as u32,
            height: 1080,
        }
    }

    pub fn view_projection(&self) -> na::Matrix4<f32> {
        self.projection * self.view
    }
}

/// Rendered frame from unified renderer
#[derive(Clone)]
pub struct UnifiedFrame {
    /// Final composited color buffer
    pub color: Vec<[f32; 4]>,
    /// Depth buffer
    pub depth: Vec<f32>,
    /// Width
    pub width: u32,
    /// Height
    pub height: u32,
    /// Statistics
    pub stats: RenderStats,
}

/// Render statistics
#[derive(Clone, Debug, Default)]
pub struct RenderStats {
    pub gaussian_objects: usize,
    pub mesh_objects: usize,
    pub avatar_objects: usize,
    pub transitioning_objects: usize,
    pub total_gaussians: usize,
    pub total_triangles: usize,
    pub render_time_ms: f32,
}

/// Unified renderer for hybrid scenes
pub struct UnifiedRenderer {
    /// 4DGS rasterizer
    gaussian_rasterizer: GpuRasterizer4D,
    /// Transition manager
    transitions: TransitionManager,
    /// Configuration
    config: UnifiedRendererConfig,
}

/// Configuration for unified renderer
#[derive(Clone, Debug)]
pub struct UnifiedRendererConfig {
    /// Enable depth compositing
    pub depth_compositing: bool,
    /// Enable transition animations
    pub enable_transitions: bool,
    /// Background color
    pub background_color: [f32; 4],
}

impl Default for UnifiedRendererConfig {
    fn default() -> Self {
        Self {
            depth_compositing: true,
            enable_transitions: true,
            background_color: [0.1, 0.1, 0.1, 1.0],
        }
    }
}

impl UnifiedRenderer {
    pub fn new(config: UnifiedRendererConfig) -> Self {
        Self {
            gaussian_rasterizer: GpuRasterizer4D::new(Raster4DConfig::default()),
            transitions: TransitionManager::new(),
            config,
        }
    }

    /// Render a hybrid scene
    pub fn render(
        &mut self,
        scene: &HybridScene,
        camera: &UnifiedCamera,
        time: f32,
    ) -> UnifiedFrame {
        let start = std::time::Instant::now();

        let width = camera.width as usize;
        let height = camera.height as usize;

        // Initialize buffers
        let mut color_buffer = vec![self.config.background_color; width * height];
        let mut depth_buffer = vec![f32::MAX; width * height];

        let mut stats = RenderStats::default();

        // Update transitions
        if self.config.enable_transitions {
            let _completed = self.transitions.update();
        }

        // Render each node type
        for node in scene.nodes() {
            match &node.representation {
                ObjectRepresentation::Gaussian4D {
                    scene: gs_scene, ..
                } => {
                    stats.gaussian_objects += 1;
                    stats.total_gaussians += gs_scene.num_gaussians();

                    // Render Gaussians
                    let raster_camera = RasterCamera {
                        view: camera.view,
                        projection: camera.projection,
                        width: camera.width,
                        height: camera.height,
                    };
                    let frame = self.gaussian_rasterizer.render_with_camera(
                        gs_scene,
                        time,
                        &raster_camera,
                        Some(&node.transform.to_matrix()),
                    );
                    self.composite_gaussian_frame(
                        &frame,
                        &node.transform.to_matrix(),
                        &mut color_buffer,
                        &mut depth_buffer,
                        width,
                    );
                }

                ObjectRepresentation::Mesh { geometry, .. } => {
                    stats.mesh_objects += 1;
                    stats.total_triangles += geometry.indices.len() / 3;

                    // Render mesh (simplified - would use GPU rasterization)
                    self.render_mesh(
                        geometry,
                        &node.transform.to_matrix(),
                        camera,
                        &mut color_buffer,
                        &mut depth_buffer,
                        width,
                        height,
                    );
                }

                ObjectRepresentation::Avatar { geometry, .. } => {
                    stats.avatar_objects += 1;
                    stats.total_triangles += geometry.indices.len() / 3;
                    // Render avatar as a skinned mesh (fallback to static mesh)
                    self.render_mesh(
                        geometry,
                        &node.transform.to_matrix(),
                        camera,
                        &mut color_buffer,
                        &mut depth_buffer,
                        width,
                        height,
                    );
                }

                ObjectRepresentation::Transitioning { progress, .. } => {
                    stats.transitioning_objects += 1;

                    if self.config.enable_transitions {
                        let _blend = TransitionBlend::from_progress(*progress);
                        // Render both with blend (simplified)
                        // Full implementation would render both and alpha blend
                    }
                }

                ObjectRepresentation::Pending => {
                    // Skip pending objects
                }
            }
        }

        stats.render_time_ms = start.elapsed().as_secs_f32() * 1000.0;

        UnifiedFrame {
            color: color_buffer,
            depth: depth_buffer,
            width: camera.width,
            height: camera.height,
            stats,
        }
    }

    /// Composite Gaussian frame into main buffer
    fn composite_gaussian_frame(
        &self,
        frame: &RenderedFrame4D,
        _transform: &na::Matrix4<f32>,
        color_buffer: &mut [[f32; 4]],
        depth_buffer: &mut [f32],
        _width: usize,
    ) {
        // Simple compositing (in production, would use GPU)
        for (i, (color, &depth)) in frame.color.iter().zip(frame.depth.iter()).enumerate() {
            if i < color_buffer.len() && depth < depth_buffer[i] {
                let alpha = color[3];
                if alpha > 0.01 {
                    // Alpha blend
                    let dst = &mut color_buffer[i];
                    dst[0] = dst[0] * (1.0 - alpha) + color[0] * alpha;
                    dst[1] = dst[1] * (1.0 - alpha) + color[1] * alpha;
                    dst[2] = dst[2] * (1.0 - alpha) + color[2] * alpha;
                    dst[3] = dst[3] * (1.0 - alpha) + alpha;

                    if alpha > 0.5 {
                        depth_buffer[i] = depth;
                    }
                }
            }
        }
    }

    /// Render mesh to buffer (simplified software rasterization)
    fn render_mesh(
        &self,
        mesh: &MeshData,
        transform: &na::Matrix4<f32>,
        camera: &UnifiedCamera,
        color_buffer: &mut [[f32; 4]],
        depth_buffer: &mut [f32],
        width: usize,
        height: usize,
    ) {
        let mvp = camera.view_projection() * transform;

        // Software rasterization (barycentric)
        for tri in mesh.indices.chunks(3) {
            if tri.len() < 3 {
                continue;
            }

            // Get vertices
            let v0 = &mesh.vertices[tri[0] as usize];
            let v1 = &mesh.vertices[tri[1] as usize];
            let v2 = &mesh.vertices[tri[2] as usize];

            // Project vertices
            let p0 = self.project_vertex(v0, &mvp, width, height);
            let p1 = self.project_vertex(v1, &mvp, width, height);
            let p2 = self.project_vertex(v2, &mvp, width, height);

            if let (Some(p0), Some(p1), Some(p2)) = (p0, p1, p2) {
                self.rasterize_triangle(
                    p0,
                    p1,
                    p2,
                    v0.color,
                    v1.color,
                    v2.color,
                    color_buffer,
                    depth_buffer,
                    width,
                    height,
                );
            }
        }
    }

    /// Project vertex to screen space
    fn project_vertex(
        &self,
        vertex: &Vertex,
        mvp: &na::Matrix4<f32>,
        width: usize,
        height: usize,
    ) -> Option<(f32, f32, f32)> {
        let pos = na::Vector4::new(
            vertex.position[0],
            vertex.position[1],
            vertex.position[2],
            1.0,
        );
        let clip = mvp * pos;

        if clip.w <= 0.0 {
            return None;
        }

        let ndc = na::Vector3::new(clip.x / clip.w, clip.y / clip.w, clip.z / clip.w);

        let screen_x = (ndc.x + 1.0) * 0.5 * width as f32;
        let screen_y = (1.0 - ndc.y) * 0.5 * height as f32;
        let depth = (ndc.z + 1.0) * 0.5;

        Some((screen_x, screen_y, depth))
    }

    fn rasterize_triangle(
        &self,
        p0: (f32, f32, f32),
        p1: (f32, f32, f32),
        p2: (f32, f32, f32),
        c0: [f32; 4],
        c1: [f32; 4],
        c2: [f32; 4],
        color_buffer: &mut [[f32; 4]],
        depth_buffer: &mut [f32],
        width: usize,
        height: usize,
    ) {
        let (x0, y0, z0) = p0;
        let (x1, y1, z1) = p1;
        let (x2, y2, z2) = p2;

        let min_x = x0.min(x1).min(x2).floor().max(0.0) as i32;
        let max_x = x0.max(x1).max(x2).ceil().min((width - 1) as f32) as i32;
        let min_y = y0.min(y1).min(y2).floor().max(0.0) as i32;
        let max_y = y0.max(y1).max(y2).ceil().min((height - 1) as f32) as i32;
        if min_x > max_x || min_y > max_y {
            return;
        }

        let area = edge_function(x0, y0, x1, y1, x2, y2);
        if area.abs() < 1e-6 {
            return;
        }
        let inv_area = 1.0 / area;

        let default_color = [0.7, 0.7, 0.7, 1.0];
        let c0 = if c0[3] <= 0.0 { default_color } else { c0 };
        let c1 = if c1[3] <= 0.0 { default_color } else { c1 };
        let c2 = if c2[3] <= 0.0 { default_color } else { c2 };

        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let sample_x = px as f32 + 0.5;
                let sample_y = py as f32 + 0.5;

                let w0 = edge_function(x1, y1, x2, y2, sample_x, sample_y);
                let w1 = edge_function(x2, y2, x0, y0, sample_x, sample_y);
                let w2 = edge_function(x0, y0, x1, y1, sample_x, sample_y);

                if (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0) {
                    let b0 = w0 * inv_area;
                    let b1 = w1 * inv_area;
                    let b2 = w2 * inv_area;

                    let depth = b0 * z0 + b1 * z1 + b2 * z2;
                    if !(0.0..=1.0).contains(&depth) {
                        continue;
                    }

                    let idx = (py as usize) * width + (px as usize);
                    if depth < depth_buffer[idx] {
                        let color = [
                            b0 * c0[0] + b1 * c1[0] + b2 * c2[0],
                            b0 * c0[1] + b1 * c1[1] + b2 * c2[1],
                            b0 * c0[2] + b1 * c1[2] + b2 * c2[2],
                            1.0,
                        ];
                        color_buffer[idx] = color;
                        depth_buffer[idx] = depth;
                    }
                }
            }
        }
    }

    /// Start a transition for an object
    pub fn start_transition(
        &mut self,
        object_id: uuid::Uuid,
        from: ObjectRepresentation,
        to: ObjectRepresentation,
    ) {
        self.transitions.start_transition(object_id, from, to);
    }

    /// Get transition manager for external control
    pub fn transitions(&self) -> &TransitionManager {
        &self.transitions
    }

    /// Get mutable transition manager
    pub fn transitions_mut(&mut self) -> &mut TransitionManager {
        &mut self.transitions
    }
}

fn edge_function(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (px - ax) * (by - ay) - (py - ay) * (bx - ax)
}

impl Default for UnifiedRenderer {
    fn default() -> Self {
        Self::new(UnifiedRendererConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_camera_creation() {
        let camera = UnifiedCamera::perspective(
            std::f32::consts::FRAC_PI_4,
            16.0 / 9.0,
            0.1,
            100.0,
            na::Point3::new(0.0, 0.0, 5.0),
            na::Point3::origin(),
        );

        let vp = camera.view_projection();
        assert!(!vp.is_identity(1e-6));
    }

    #[test]
    fn test_renderer_creation() {
        let renderer = UnifiedRenderer::default();
        assert_eq!(renderer.transitions.active_count(), 0);
    }
}
