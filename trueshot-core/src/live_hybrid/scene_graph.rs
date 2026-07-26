//! Hybrid Scene Graph
//!
//! Manages a scene containing mixed representations:
//! - 4D Gaussian Splatting for dynamic objects
//! - Textured meshes for static objects
//! - Animated avatars for tracked humans

use std::collections::HashMap;
use std::time::Instant;
use uuid::Uuid;
use nalgebra as na;
use serde::{Deserialize, Serialize};

use crate::gaussian_splatting::gaussian_4d::Dynamic4DScene;

/// Vertex data for mesh representation
#[derive(Clone, Debug, Default)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// Mesh geometry data
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub name: String,
}

impl MeshData {
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }
    
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Transform in 3D space
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transform3D {
    pub position: na::Vector3<f32>,
    pub rotation: na::UnitQuaternion<f32>,
    pub scale: na::Vector3<f32>,
}

impl Default for Transform3D {
    fn default() -> Self {
        Self {
            position: na::Vector3::zeros(),
            rotation: na::UnitQuaternion::identity(),
            scale: na::Vector3::new(1.0, 1.0, 1.0),
        }
    }
}

impl Transform3D {
    pub fn to_matrix(&self) -> na::Matrix4<f32> {
        let translation = na::Translation3::from(self.position);
        let rotation = self.rotation.to_homogeneous();
        let scale = na::Matrix4::new_nonuniform_scaling(&self.scale);
        translation.to_homogeneous() * rotation * scale
    }
}

/// A node in the hybrid scene graph
#[derive(Clone, Debug)]
pub struct HybridSceneNode {
    /// Unique identifier
    pub id: Uuid,
    /// Human-readable name
    pub name: String,
    /// World transform
    pub transform: Transform3D,
    /// Current motion score (0.0 = static, 1.0 = highly dynamic)
    pub motion_score: f32,
    /// Motion score history for smoothing
    pub motion_history: Vec<f32>,
    /// Current representation
    pub representation: ObjectRepresentation,
    /// Last time this node was updated
    pub last_updated: Instant,
    /// Parent node ID (None = root)
    pub parent: Option<Uuid>,
    /// Child node IDs
    pub children: Vec<Uuid>,
}

impl HybridSceneNode {
    pub fn new(name: &str, representation: ObjectRepresentation) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.to_string(),
            transform: Transform3D::default(),
            motion_score: 0.5, // Start neutral
            motion_history: Vec::with_capacity(30),
            representation,
            last_updated: Instant::now(),
            parent: None,
            children: Vec::new(),
        }
    }
    
    /// Update motion score with smoothing
    pub fn update_motion_score(&mut self, new_score: f32) {
        self.motion_history.push(new_score);
        if self.motion_history.len() > 30 {
            self.motion_history.remove(0);
        }
        
        // Exponential moving average
        let alpha = 0.3;
        self.motion_score = self.motion_score * (1.0 - alpha) + new_score * alpha;
        self.last_updated = Instant::now();
    }
    
    /// Get average motion score over history
    pub fn average_motion_score(&self) -> f32 {
        if self.motion_history.is_empty() {
            return self.motion_score;
        }
        self.motion_history.iter().sum::<f32>() / self.motion_history.len() as f32
    }
    
    /// Check if object has been stable (low motion) for a duration
    pub fn is_stable_for(&self, frames: usize) -> bool {
        if self.motion_history.len() < frames {
            return false;
        }
        let recent: Vec<_> = self.motion_history.iter().rev().take(frames).collect();
        recent.iter().all(|&&s| s < 0.1)
    }
}

/// Object representation in the hybrid scene
#[derive(Clone, Debug)]
pub enum ObjectRepresentation {
    /// Full 4D Gaussian cloud for dynamic objects
    Gaussian4D {
        scene: Dynamic4DScene,
        /// Indices of Gaussians belonging to this object
        gaussian_indices: Vec<usize>,
    },
    
    /// Textured mesh for static objects
    Mesh {
        geometry: MeshData,
        texture_id: Option<Uuid>,
        lod_levels: Vec<MeshLOD>,
    },
    
    /// Avatar with skeletal animation
    Avatar {
        avatar_id: Uuid,
        geometry: MeshData,
        bone_transforms: Vec<na::Matrix4<f32>>,
        blendshape_weights: Vec<f32>,
    },
    
    /// Transitioning between representations
    Transitioning {
        from: Box<ObjectRepresentation>,
        to: Box<ObjectRepresentation>,
        progress: f32,
        start_time: Instant,
    },
    
    /// Placeholder for objects being processed
    Pending,
}

/// Level of Detail for mesh
#[derive(Clone, Debug)]
pub struct MeshLOD {
    pub level: u8,
    pub geometry: MeshData,
    pub distance_threshold: f32,
}

/// The complete hybrid scene
#[derive(Clone)]
pub struct HybridScene {
    /// All nodes in the scene
    nodes: HashMap<Uuid, HybridSceneNode>,
    /// Root node IDs
    roots: Vec<Uuid>,
    /// Scene metadata
    pub name: String,
    pub created_at: Instant,
    /// Global 4DGS scene (source of truth for Gaussians)
    pub source_4dgs: Option<Dynamic4DScene>,
}

impl Default for HybridScene {
    fn default() -> Self {
        Self::new("Untitled Scene")
    }
}

impl HybridScene {
    pub fn new(name: &str) -> Self {
        Self {
            nodes: HashMap::new(),
            roots: Vec::new(),
            name: name.to_string(),
            created_at: Instant::now(),
            source_4dgs: None,
        }
    }
    
    /// Add a node to the scene
    pub fn add_node(&mut self, node: HybridSceneNode) -> Uuid {
        let id = node.id;
        if node.parent.is_none() {
            self.roots.push(id);
        }
        self.nodes.insert(id, node);
        id
    }
    
    /// Get a node by ID
    pub fn get_node(&self, id: Uuid) -> Option<&HybridSceneNode> {
        self.nodes.get(&id)
    }
    
    /// Get a mutable node by ID
    pub fn get_node_mut(&mut self, id: Uuid) -> Option<&mut HybridSceneNode> {
        self.nodes.get_mut(&id)
    }
    
    /// Remove a node from the scene
    pub fn remove_node(&mut self, id: Uuid) -> Option<HybridSceneNode> {
        if let Some(node) = self.nodes.remove(&id) {
            self.roots.retain(|&r| r != id);
            return Some(node);
        }
        None
    }
    
    /// Update a node's representation
    pub fn update_representation(&mut self, id: Uuid, repr: ObjectRepresentation) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.representation = repr;
            node.last_updated = Instant::now();
        }
    }
    
    /// Get all nodes
    pub fn nodes(&self) -> impl Iterator<Item = &HybridSceneNode> {
        self.nodes.values()
    }
    
    /// Get mutable reference to all nodes
    pub fn nodes_mut(&mut self) -> impl Iterator<Item = &mut HybridSceneNode> {
        self.nodes.values_mut()
    }
    
    /// Get nodes by representation type
    pub fn get_gaussian_nodes(&self) -> Vec<&HybridSceneNode> {
        self.nodes.values()
            .filter(|n| matches!(n.representation, ObjectRepresentation::Gaussian4D { .. }))
            .collect()
    }
    
    pub fn get_mesh_nodes(&self) -> Vec<&HybridSceneNode> {
        self.nodes.values()
            .filter(|n| matches!(n.representation, ObjectRepresentation::Mesh { .. }))
            .collect()
    }
    
    pub fn get_avatar_nodes(&self) -> Vec<&HybridSceneNode> {
        self.nodes.values()
            .filter(|n| matches!(n.representation, ObjectRepresentation::Avatar { .. }))
            .collect()
    }
    
    /// Get nodes that are candidates for meshification
    pub fn get_meshification_candidates(&self, stability_frames: usize) -> Vec<Uuid> {
        self.nodes.values()
            .filter(|n| {
                matches!(n.representation, ObjectRepresentation::Gaussian4D { .. })
                    && n.is_stable_for(stability_frames)
            })
            .map(|n| n.id)
            .collect()
    }
    
    /// Count nodes by type
    pub fn stats(&self) -> SceneStats {
        let mut stats = SceneStats::default();
        for node in self.nodes.values() {
            match &node.representation {
                ObjectRepresentation::Gaussian4D { .. } => stats.gaussian_count += 1,
                ObjectRepresentation::Mesh { .. } => stats.mesh_count += 1,
                ObjectRepresentation::Avatar { .. } => stats.avatar_count += 1,
                ObjectRepresentation::Transitioning { .. } => stats.transitioning_count += 1,
                ObjectRepresentation::Pending => stats.pending_count += 1,
            }
        }
        stats.total_nodes = self.nodes.len();
        stats
    }
}

/// Scene statistics
#[derive(Clone, Debug, Default)]
pub struct SceneStats {
    pub total_nodes: usize,
    pub gaussian_count: usize,
    pub mesh_count: usize,
    pub avatar_count: usize,
    pub transitioning_count: usize,
    pub pending_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_scene_creation() {
        let scene = HybridScene::new("Test Scene");
        assert_eq!(scene.name, "Test Scene");
        assert!(scene.nodes.is_empty());
    }
    
    #[test]
    fn test_add_node() {
        let mut scene = HybridScene::new("Test");
        let node = HybridSceneNode::new("Object1", ObjectRepresentation::Pending);
        let id = scene.add_node(node);
        
        assert!(scene.get_node(id).is_some());
        assert_eq!(scene.stats().total_nodes, 1);
    }
    
    #[test]
    fn test_motion_score_smoothing() {
        let mut node = HybridSceneNode::new("Test", ObjectRepresentation::Pending);
        
        // Simulate low motion
        for _ in 0..10 {
            node.update_motion_score(0.05);
        }
        
        assert!(node.motion_score < 0.2);
        assert!(node.average_motion_score() < 0.1);
    }
}
