use crate::reconstruction::{Face, Mesh};
use anyhow::{Context, Result};
use nalgebra as na;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub fn load_mesh(path: &Path) -> Result<Mesh> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "obj" => load_obj(path),
        "ply" => load_ply(path),
        _ => anyhow::bail!("Unsupported mesh format: {}", ext),
    }
}

pub fn ensure_vertex_normals(mesh: &mut Mesh) {
    if !mesh.normals.is_empty() && mesh.normals.len() == mesh.vertices.len() {
        return;
    }
    let mut normals = vec![na::Vector3::new(0.0f32, 0.0f32, 0.0f32); mesh.vertices.len()];
    for face in &mesh.faces {
        if face.vertices[0] >= mesh.vertices.len()
            || face.vertices[1] >= mesh.vertices.len()
            || face.vertices[2] >= mesh.vertices.len()
        {
            continue;
        }
        let v0 = mesh.vertices[face.vertices[0]];
        let v1 = mesh.vertices[face.vertices[1]];
        let v2 = mesh.vertices[face.vertices[2]];
        let e1 = v1 - v0;
        let e2 = v2 - v0;
        let n = e1.cross(&e2);
        normals[face.vertices[0]] += n;
        normals[face.vertices[1]] += n;
        normals[face.vertices[2]] += n;
    }
    for n in &mut normals {
        let len = n.norm();
        if len > 1e-6 {
            *n /= len;
        } else {
            *n = na::Vector3::new(0.0, 0.0, 1.0);
        }
    }
    mesh.normals = normals;
}

fn load_obj(path: &Path) -> Result<Mesh> {
    let file =
        File::open(path).with_context(|| format!("Failed to open OBJ: {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut positions: Vec<na::Point3<f32>> = Vec::new();
    let mut position_colors: Vec<Option<[u8; 3]>> = Vec::new();
    let mut any_color = false;
    let mut normals: Vec<na::Vector3<f32>> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();

    let mut out_positions: Vec<na::Point3<f32>> = Vec::new();
    let mut out_colors: Vec<[u8; 3]> = Vec::new();
    let mut out_normals: Vec<na::Vector3<f32>> = Vec::new();
    let mut out_uvs: Vec<[f32; 2]> = Vec::new();
    let mut faces: Vec<Face> = Vec::new();

    let mut index_map: HashMap<(i32, Option<i32>, Option<i32>), usize> = HashMap::new();

    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let tag = parts.next().unwrap_or("");
        match tag {
            "v" => {
                let x: f32 = parts.next().unwrap_or("0").parse().unwrap_or(0.0);
                let y: f32 = parts.next().unwrap_or("0").parse().unwrap_or(0.0);
                let z: f32 = parts.next().unwrap_or("0").parse().unwrap_or(0.0);
                let color = if let (Some(r), Some(g), Some(b)) =
                    (parts.next(), parts.next(), parts.next())
                {
                    let r: f32 = r.parse().unwrap_or(1.0);
                    let g: f32 = g.parse().unwrap_or(1.0);
                    let b: f32 = b.parse().unwrap_or(1.0);
                    any_color = true;
                    Some([
                        (r.max(0.0).min(1.0) * 255.0) as u8,
                        (g.max(0.0).min(1.0) * 255.0) as u8,
                        (b.max(0.0).min(1.0) * 255.0) as u8,
                    ])
                } else {
                    None
                };
                positions.push(na::Point3::new(x, y, z));
                position_colors.push(color);
            }
            "vn" => {
                let x: f32 = parts.next().unwrap_or("0").parse().unwrap_or(0.0);
                let y: f32 = parts.next().unwrap_or("0").parse().unwrap_or(0.0);
                let z: f32 = parts.next().unwrap_or("0").parse().unwrap_or(0.0);
                normals.push(na::Vector3::new(x, y, z));
            }
            "vt" => {
                let u: f32 = parts.next().unwrap_or("0").parse().unwrap_or(0.0);
                let v: f32 = parts.next().unwrap_or("0").parse().unwrap_or(0.0);
                uvs.push([u, v]);
            }
            "f" => {
                let indices: Vec<&str> = parts.collect();
                if indices.len() < 3 {
                    continue;
                }
                let mut face_indices = Vec::new();
                for idx in &indices[0..3] {
                    let comps: Vec<&str> = idx.split('/').collect();
                    let v_idx: i32 = comps
                        .get(0)
                        .and_then(|v| v.parse::<i32>().ok())
                        .unwrap_or(0);
                    let vt_idx: Option<i32> = comps.get(1).and_then(|v| {
                        if v.is_empty() {
                            None
                        } else {
                            v.parse::<i32>().ok()
                        }
                    });
                    let vn_idx: Option<i32> = comps.get(2).and_then(|v| {
                        if v.is_empty() {
                            None
                        } else {
                            v.parse::<i32>().ok()
                        }
                    });
                    let key = (v_idx, vt_idx, vn_idx);
                    let out_idx = if let Some(existing) = index_map.get(&key) {
                        *existing
                    } else {
                        let vpos = if v_idx < 0 {
                            let idx = (positions.len() as i32 + v_idx) as usize;
                            positions.get(idx).copied().unwrap_or(na::Point3::origin())
                        } else {
                            positions
                                .get((v_idx - 1).max(0) as usize)
                                .copied()
                                .unwrap_or(na::Point3::origin())
                        };
                        out_positions.push(vpos);

                        if any_color {
                            let c = if v_idx < 0 {
                                let idx = (position_colors.len() as i32 + v_idx) as usize;
                                position_colors.get(idx).and_then(|c| *c)
                            } else {
                                position_colors
                                    .get((v_idx - 1).max(0) as usize)
                                    .and_then(|c| *c)
                            };
                            out_colors.push(c.unwrap_or([255, 255, 255]));
                        }

                        if let Some(vt) = vt_idx {
                            let uv = if vt < 0 {
                                let idx = (uvs.len() as i32 + vt) as usize;
                                uvs.get(idx).copied().unwrap_or([0.0, 0.0])
                            } else {
                                uvs.get((vt - 1).max(0) as usize)
                                    .copied()
                                    .unwrap_or([0.0, 0.0])
                            };
                            out_uvs.push(uv);
                        }

                        if let Some(vn) = vn_idx {
                            let n = if vn < 0 {
                                let idx = (normals.len() as i32 + vn) as usize;
                                normals.get(idx).copied().unwrap_or(na::Vector3::z())
                            } else {
                                normals
                                    .get((vn - 1).max(0) as usize)
                                    .copied()
                                    .unwrap_or(na::Vector3::z())
                            };
                            out_normals.push(n);
                        }
                        let new_idx = out_positions.len() - 1;
                        index_map.insert(key, new_idx);
                        new_idx
                    };
                    face_indices.push(out_idx);
                }
                if face_indices.len() == 3 {
                    faces.push(Face {
                        vertices: [face_indices[0], face_indices[1], face_indices[2]],
                    });
                }
            }
            _ => {}
        }
    }

    Ok(Mesh {
        vertices: out_positions,
        colors: out_colors,
        normals: out_normals,
        uvs: out_uvs,
        faces,
    })
}

fn load_ply(path: &Path) -> Result<Mesh> {
    let file =
        File::open(path).with_context(|| format!("Failed to open PLY: {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut vertex_count = 0usize;
    let mut face_count = 0usize;
    let mut is_ascii = false;
    let mut vertex_properties: Vec<String> = Vec::new();
    let mut current_element: Option<String> = None;

    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.starts_with("format ") {
            if trimmed.contains("ascii") {
                is_ascii = true;
            } else {
                anyhow::bail!("PLY must be ascii: {}", path.display());
            }
        } else if trimmed.starts_with("element ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let name = parts[1];
                let count = parts[2].parse::<usize>().unwrap_or(0);
                current_element = Some(name.to_string());
                if name == "vertex" {
                    vertex_count = count;
                } else if name == "face" {
                    face_count = count;
                }
            }
        } else if trimmed.starts_with("property ") {
            if let Some(ref element) = current_element {
                if element == "vertex" {
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if let Some(name) = parts.last() {
                        vertex_properties.push(name.to_string());
                    }
                }
            }
        } else if trimmed == "end_header" {
            break;
        }
    }

    if !is_ascii || vertex_count == 0 {
        anyhow::bail!("PLY header invalid: {}", path.display());
    }

    let index_of = |name: &str| vertex_properties.iter().position(|p| p == name);
    let x_idx = index_of("x").ok_or_else(|| anyhow::anyhow!("PLY missing x"))?;
    let y_idx = index_of("y").ok_or_else(|| anyhow::anyhow!("PLY missing y"))?;
    let z_idx = index_of("z").ok_or_else(|| anyhow::anyhow!("PLY missing z"))?;
    let nx_idx = index_of("nx");
    let ny_idx = index_of("ny");
    let nz_idx = index_of("nz");
    let u_idx = index_of("s").or_else(|| index_of("u"));
    let v_idx = index_of("t").or_else(|| index_of("v"));
    let r_idx = index_of("red");
    let g_idx = index_of("green");
    let b_idx = index_of("blue");

    let mut vertices = Vec::with_capacity(vertex_count);
    let mut normals: Vec<na::Vector3<f32>> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut colors: Vec<[u8; 3]> = Vec::new();

    for _ in 0..vertex_count {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() <= z_idx {
            continue;
        }
        let x = parts
            .get(x_idx)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);
        let y = parts
            .get(y_idx)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);
        let z = parts
            .get(z_idx)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);
        vertices.push(na::Point3::new(x, y, z));

        if let (Some(nx_idx), Some(ny_idx), Some(nz_idx)) = (nx_idx, ny_idx, nz_idx) {
            let nx = parts
                .get(nx_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0);
            let ny = parts
                .get(ny_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0);
            let nz = parts
                .get(nz_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0);
            normals.push(na::Vector3::new(nx, ny, nz));
        }

        if let (Some(u_idx), Some(v_idx)) = (u_idx, v_idx) {
            let u = parts
                .get(u_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0);
            let v = parts
                .get(v_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0);
            uvs.push([u, v]);
        }

        if let (Some(r_idx), Some(g_idx), Some(b_idx)) = (r_idx, g_idx, b_idx) {
            let r = parts
                .get(r_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(255.0);
            let g = parts
                .get(g_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(255.0);
            let b = parts
                .get(b_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(255.0);
            colors.push([
                if r <= 1.0 { (r * 255.0) as u8 } else { r as u8 },
                if g <= 1.0 { (g * 255.0) as u8 } else { g as u8 },
                if b <= 1.0 { (b * 255.0) as u8 } else { b as u8 },
            ]);
        }
    }

    let mut faces = Vec::new();
    for _ in 0..face_count {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let count = parts[0].parse::<usize>().unwrap_or(0);
        if count < 3 || parts.len() < count + 1 {
            continue;
        }
        let mut indices: Vec<usize> = Vec::with_capacity(count);
        for idx in 0..count {
            let value = parts
                .get(idx + 1)
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            indices.push(value);
        }
        for tri in 1..(indices.len() - 1) {
            faces.push(Face {
                vertices: [indices[0], indices[tri], indices[tri + 1]],
            });
        }
    }

    Ok(Mesh {
        vertices,
        colors,
        normals,
        uvs,
        faces,
    })
}
