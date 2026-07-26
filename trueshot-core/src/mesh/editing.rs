use anyhow::Result;
use nalgebra as na;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::gaussian_splatting::gs2mesh::{simplify_textured_mesh, TexturedMesh};
use crate::mesh::io::ensure_vertex_normals;
use crate::mesh::optimization::smooth_mesh;
use crate::reconstruction::{Face, Mesh};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MeshEditOp {
    Smooth {
        iterations: usize,
        lambda: f32,
    },
    Decimate {
        target_triangles: usize,
        preserve_boundaries: bool,
        preserve_uv_seams: bool,
        uv_seam_threshold: f32,
    },
    RecomputeNormals,
    FillHoles {
        max_hole_vertices: usize,
    },
}

pub fn apply_mesh_edits(mesh: &mut Mesh, ops: &[MeshEditOp]) -> Result<()> {
    for op in ops {
        match *op {
            MeshEditOp::Smooth { iterations, lambda } => {
                smooth_mesh(mesh, iterations, lambda);
            }
            MeshEditOp::Decimate {
                target_triangles,
                preserve_boundaries,
                preserve_uv_seams,
                uv_seam_threshold,
            } => {
                if target_triangles >= mesh.faces.len() {
                    continue;
                }
                let textured = mesh_to_textured(mesh);
                let simplified = simplify_textured_mesh(
                    textured,
                    target_triangles,
                    preserve_boundaries,
                    preserve_uv_seams,
                    uv_seam_threshold,
                )?;
                *mesh = textured_to_mesh(&simplified);
            }
            MeshEditOp::RecomputeNormals => {
                ensure_vertex_normals(mesh);
            }
            MeshEditOp::FillHoles { max_hole_vertices } => {
                fill_small_holes(mesh, max_hole_vertices);
                ensure_vertex_normals(mesh);
            }
        }
    }
    Ok(())
}

fn mesh_to_textured(mesh: &Mesh) -> TexturedMesh {
    let mut normals = mesh.normals.clone();
    if normals.len() != mesh.vertices.len() {
        normals = vec![na::Vector3::z(); mesh.vertices.len()];
    }
    let mut uvs = mesh
        .uvs
        .iter()
        .map(|uv| na::Vector2::new(uv[0], uv[1]))
        .collect::<Vec<_>>();
    if uvs.len() != mesh.vertices.len() {
        uvs.resize(mesh.vertices.len(), na::Vector2::zeros());
    }
    let indices = mesh
        .faces
        .iter()
        .map(|f| {
            [
                f.vertices[0] as u32,
                f.vertices[1] as u32,
                f.vertices[2] as u32,
            ]
        })
        .collect();
    TexturedMesh {
        vertices: mesh.vertices.clone(),
        normals,
        uvs,
        indices,
        texture: None,
        lod_chain: Vec::new(),
    }
}

fn textured_to_mesh(mesh: &TexturedMesh) -> Mesh {
    let uvs = mesh.uvs.iter().map(|uv| [uv.x, uv.y]).collect::<Vec<_>>();
    let faces = mesh
        .indices
        .iter()
        .map(|tri| Face {
            vertices: [tri[0] as usize, tri[1] as usize, tri[2] as usize],
        })
        .collect();
    Mesh {
        vertices: mesh.vertices.clone(),
        colors: Vec::new(),
        normals: mesh.normals.clone(),
        uvs,
        faces,
    }
}

fn fill_small_holes(mesh: &mut Mesh, max_hole_vertices: usize) {
    let boundary = find_boundary_edges(&mesh.faces);
    if boundary.is_empty() {
        return;
    }
    let mut adjacency: HashMap<usize, Vec<usize>> = HashMap::new();
    for (u, v) in &boundary {
        adjacency.entry(*u).or_default().push(*v);
        adjacency.entry(*v).or_default().push(*u);
    }

    let mut visited: HashSet<(usize, usize)> = HashSet::new();
    let mut loops = Vec::new();

    for (u, v) in &boundary {
        let key = if u < v { (*u, *v) } else { (*v, *u) };
        if visited.contains(&key) {
            continue;
        }
        let mut loop_vertices = Vec::new();
        loop_vertices.push(*u);
        let mut prev = *u;
        let mut current = *v;
        visited.insert(key);
        loop {
            loop_vertices.push(current);
            let neighbors = adjacency.get(&current).cloned().unwrap_or_default();
            let mut next = None;
            for n in neighbors {
                if n == prev {
                    continue;
                }
                let k = if current < n {
                    (current, n)
                } else {
                    (n, current)
                };
                if !visited.contains(&k) {
                    next = Some(n);
                    visited.insert(k);
                    break;
                }
            }
            match next {
                Some(n) => {
                    prev = current;
                    current = n;
                    if current == loop_vertices[0] {
                        break;
                    }
                }
                None => break,
            }
        }
        if loop_vertices.len() >= 3 {
            loops.push(loop_vertices);
        }
    }

    for loop_vertices in loops {
        if loop_vertices.len() > max_hole_vertices {
            continue;
        }
        let centroid = compute_centroid(mesh, &loop_vertices);
        let center_idx = mesh.vertices.len();
        mesh.vertices.push(centroid);
        if !mesh.colors.is_empty() {
            mesh.colors.push(average_color(mesh, &loop_vertices));
        }
        if !mesh.uvs.is_empty() {
            mesh.uvs.push(average_uv(mesh, &loop_vertices));
        }
        if !mesh.normals.is_empty() {
            mesh.normals.push(na::Vector3::z());
        }
        for i in 0..loop_vertices.len() {
            let a = loop_vertices[i];
            let b = loop_vertices[(i + 1) % loop_vertices.len()];
            mesh.faces.push(Face {
                vertices: [a, b, center_idx],
            });
        }
    }
}

fn compute_centroid(mesh: &Mesh, loop_vertices: &[usize]) -> na::Point3<f32> {
    let mut sum = na::Vector3::zeros();
    for idx in loop_vertices {
        sum += mesh.vertices[*idx].coords;
    }
    let count = loop_vertices.len() as f32;
    na::Point3::from(sum / count.max(1.0))
}

fn average_color(mesh: &Mesh, loop_vertices: &[usize]) -> [u8; 3] {
    let mut r = 0u32;
    let mut g = 0u32;
    let mut b = 0u32;
    let mut count = 0u32;
    for idx in loop_vertices {
        if let Some(color) = mesh.colors.get(*idx) {
            r += color[0] as u32;
            g += color[1] as u32;
            b += color[2] as u32;
            count += 1;
        }
    }
    if count == 0 {
        return [255, 255, 255];
    }
    [(r / count) as u8, (g / count) as u8, (b / count) as u8]
}

fn average_uv(mesh: &Mesh, loop_vertices: &[usize]) -> [f32; 2] {
    let mut u = 0.0;
    let mut v = 0.0;
    let mut count = 0.0;
    for idx in loop_vertices {
        if let Some(uv) = mesh.uvs.get(*idx) {
            u += uv[0];
            v += uv[1];
            count += 1.0;
        }
    }
    if count == 0.0 {
        return [0.0, 0.0];
    }
    [u / count, v / count]
}

fn find_boundary_edges(faces: &[Face]) -> Vec<(usize, usize)> {
    let mut edge_counts: HashMap<(usize, usize), u32> = HashMap::new();
    for face in faces {
        let edges = [
            (face.vertices[0], face.vertices[1]),
            (face.vertices[1], face.vertices[2]),
            (face.vertices[2], face.vertices[0]),
        ];
        for (u, v) in edges {
            let key = if u < v { (u, v) } else { (v, u) };
            *edge_counts.entry(key).or_insert(0) += 1;
        }
    }
    edge_counts
        .into_iter()
        .filter(|(_, count)| *count == 1)
        .map(|(edge, _)| edge)
        .collect()
}
