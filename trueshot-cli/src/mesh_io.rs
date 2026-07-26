use anyhow::{Context, Result};
use nalgebra as na;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use trueshot_core::reconstruction::{Face, Mesh};

#[derive(Clone, Copy, Debug)]
enum PlyFormat {
    Ascii,
    BinaryLittleEndian,
    BinaryBigEndian,
}

#[derive(Clone, Copy, Debug)]
enum PlyScalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

#[derive(Clone, Debug)]
enum PlyProperty {
    Scalar {
        name: String,
        ty: PlyScalar,
    },
    List {
        name: String,
        count_ty: PlyScalar,
        item_ty: PlyScalar,
    },
}

#[derive(Clone, Debug)]
struct PlyElement {
    name: String,
    count: usize,
    properties: Vec<PlyProperty>,
}

#[derive(Clone, Copy, Debug)]
enum ScalarValue {
    I64(i64),
    U64(u64),
    F64(f64),
}

pub fn load_mesh(path: &Path) -> Result<Mesh> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "ply" => load_ply(path),
        "obj" => load_obj(path),
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

pub fn export_obj(
    mesh: &Mesh,
    path: &Path,
    include_normals: bool,
    include_colors: bool,
) -> Result<()> {
    let mut file = File::create(path)
        .with_context(|| format!("Failed to create OBJ file: {}", path.display()))?;

    for (i, v) in mesh.vertices.iter().enumerate() {
        if include_colors {
            let c = mesh.colors.get(i).copied().unwrap_or([255, 255, 255]);
            writeln!(
                file,
                "v {} {} {} {} {} {}",
                v.x,
                v.y,
                v.z,
                c[0] as f32 / 255.0,
                c[1] as f32 / 255.0,
                c[2] as f32 / 255.0
            )?;
        } else {
            writeln!(file, "v {} {} {}", v.x, v.y, v.z)?;
        }
    }

    if include_normals && !mesh.normals.is_empty() {
        for n in &mesh.normals {
            writeln!(file, "vn {} {} {}", n.x, n.y, n.z)?;
        }
    }

    if !mesh.uvs.is_empty() {
        for uv in &mesh.uvs {
            writeln!(file, "vt {} {}", uv[0], uv[1])?;
        }
    }

    for face in &mesh.faces {
        let v0 = face.vertices[0] + 1;
        let v1 = face.vertices[1] + 1;
        let v2 = face.vertices[2] + 1;

        if include_normals && !mesh.normals.is_empty() && !mesh.uvs.is_empty() {
            writeln!(file, "f {0}/{0}/{0} {1}/{1}/{1} {2}/{2}/{2}", v0, v1, v2)?;
        } else if include_normals && !mesh.normals.is_empty() {
            writeln!(file, "f {0}//{0} {1}//{1} {2}//{2}", v0, v1, v2)?;
        } else if !mesh.uvs.is_empty() {
            writeln!(file, "f {0}/{0} {1}/{1} {2}/{2}", v0, v1, v2)?;
        } else {
            writeln!(file, "f {} {} {}", v0, v1, v2)?;
        }
    }
    Ok(())
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
                        (r.clamp(0.0, 1.0) * 255.0) as u8,
                        (g.clamp(0.0, 1.0) * 255.0) as u8,
                        (b.clamp(0.0, 1.0) * 255.0) as u8,
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
                let mut face_indices: Vec<usize> = Vec::new();
                for vert in parts {
                    let (v_idx, vt_idx, vn_idx) = parse_obj_indices(vert)?;
                    let key = (v_idx, vt_idx, vn_idx);
                    let new_idx = if let Some(existing) = index_map.get(&key) {
                        *existing
                    } else {
                        let pos = positions
                            .get(resolve_obj_index(v_idx, positions.len())?)
                            .copied()
                            .context("OBJ vertex index out of bounds")?;
                        out_positions.push(pos);

                        let color = position_colors
                            .get(resolve_obj_index(v_idx, position_colors.len())?)
                            .copied()
                            .flatten()
                            .unwrap_or([255, 255, 255]);
                        out_colors.push(color);

                        if let Some(vn) = vn_idx {
                            let n = normals
                                .get(resolve_obj_index(vn, normals.len())?)
                                .copied()
                                .context("OBJ normal index out of bounds")?;
                            out_normals.push(n);
                        }

                        if let Some(vt) = vt_idx {
                            let uv = uvs
                                .get(resolve_obj_index(vt, uvs.len())?)
                                .copied()
                                .context("OBJ uv index out of bounds")?;
                            out_uvs.push(uv);
                        }

                        let idx = out_positions.len() - 1;
                        index_map.insert(key, idx);
                        idx
                    };
                    face_indices.push(new_idx);
                }
                triangulate_face(&face_indices, &mut faces);
            }
            _ => {}
        }
    }

    if out_normals.len() != out_positions.len() {
        out_normals.clear();
    }
    if out_uvs.len() != out_positions.len() {
        out_uvs.clear();
    }
    if !any_color || out_colors.len() != out_positions.len() {
        out_colors.clear();
    }

    Ok(Mesh {
        vertices: out_positions,
        colors: out_colors,
        normals: out_normals,
        uvs: out_uvs,
        faces,
    })
}

fn parse_obj_indices(s: &str) -> Result<(i32, Option<i32>, Option<i32>)> {
    let mut parts = s.split('/');
    let v = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("Invalid OBJ face token"))?;
    let v_idx: i32 = v.parse().context("Invalid OBJ vertex index")?;
    let vt_idx: Option<i32> = match parts.next() {
        Some("") | None => None,
        Some(vt) => Some(vt.parse().context("Invalid OBJ uv index")?),
    };
    let vn_idx: Option<i32> = match parts.next() {
        Some("") | None => None,
        Some(vn) => Some(vn.parse().context("Invalid OBJ normal index")?),
    };
    Ok((v_idx, vt_idx, vn_idx))
}

fn resolve_obj_index(idx: i32, len: usize) -> Result<usize> {
    if idx == 0 {
        anyhow::bail!("OBJ indices are 1-based; 0 is invalid");
    }
    if idx > 0 {
        Ok((idx - 1) as usize)
    } else {
        let resolved = len as i32 + idx;
        if resolved < 0 {
            anyhow::bail!("OBJ negative index out of bounds");
        }
        Ok(resolved as usize)
    }
}

fn triangulate_face(indices: &[usize], faces: &mut Vec<Face>) {
    if indices.len() < 3 {
        return;
    }
    if indices.len() == 3 {
        faces.push(Face {
            vertices: [indices[0], indices[1], indices[2]],
        });
        return;
    }
    for i in 1..(indices.len() - 1) {
        faces.push(Face {
            vertices: [indices[0], indices[i], indices[i + 1]],
        });
    }
}

fn load_ply(path: &Path) -> Result<Mesh> {
    let file =
        File::open(path).with_context(|| format!("Failed to open PLY: {}", path.display()))?;
    let mut reader = BufReader::new(file);

    let mut header_lines = Vec::new();
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            anyhow::bail!("Unexpected EOF while reading PLY header");
        }
        let trimmed = line.trim();
        header_lines.push(trimmed.to_string());
        if trimmed == "end_header" {
            break;
        }
    }

    let (format, elements) = parse_ply_header(&header_lines)?;

    let mut vertices: Vec<na::Point3<f32>> = Vec::new();
    let mut normals: Vec<na::Vector3<f32>> = Vec::new();
    let mut colors: Vec<[u8; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut faces: Vec<Face> = Vec::new();

    let mut vertex_has_norm = false;
    let mut vertex_has_color = false;
    let mut vertex_has_uv = false;
    if let Some(vertex_element) = elements.iter().find(|el| el.name == "vertex") {
        for prop in &vertex_element.properties {
            if let PlyProperty::Scalar { name, .. } = prop {
                match name.as_str() {
                    "nx" | "ny" | "nz" => vertex_has_norm = true,
                    "red" | "green" | "blue" | "r" | "g" | "b" => vertex_has_color = true,
                    "u" | "v" | "s" | "t" | "texture_u" | "texture_v" => vertex_has_uv = true,
                    _ => {}
                }
            }
        }
    }

    for element in &elements {
        match format {
            PlyFormat::Ascii => {
                for _ in 0..element.count {
                    let mut line = String::new();
                    reader.read_line(&mut line)?;
                    if element.name == "vertex" {
                        parse_ply_vertex_ascii(
                            line.trim(),
                            &element.properties,
                            &mut vertices,
                            &mut normals,
                            &mut colors,
                            &mut uvs,
                        )?;
                    } else if element.name == "face" {
                        parse_ply_face_ascii(line.trim(), &element.properties, &mut faces)?;
                    }
                }
            }
            PlyFormat::BinaryLittleEndian | PlyFormat::BinaryBigEndian => {
                let endian = format;
                for _ in 0..element.count {
                    if element.name == "vertex" {
                        parse_ply_vertex_binary(
                            &mut reader,
                            endian,
                            &element.properties,
                            &mut vertices,
                            &mut normals,
                            &mut colors,
                            &mut uvs,
                        )?;
                    } else if element.name == "face" {
                        parse_ply_face_binary(
                            &mut reader,
                            endian,
                            &element.properties,
                            &mut faces,
                        )?;
                    } else {
                        skip_ply_element_binary(&mut reader, endian, &element.properties)?;
                    }
                }
            }
        }
    }

    if !vertex_has_norm || normals.len() != vertices.len() {
        normals.clear();
    }
    if !vertex_has_color || colors.len() != vertices.len() {
        colors.clear();
    }
    if !vertex_has_uv || uvs.len() != vertices.len() {
        uvs.clear();
    }

    Ok(Mesh {
        vertices,
        colors,
        normals,
        uvs,
        faces,
    })
}

fn parse_ply_header(lines: &[String]) -> Result<(PlyFormat, Vec<PlyElement>)> {
    let mut format = None;
    let mut elements: Vec<PlyElement> = Vec::new();
    let mut current: Option<PlyElement> = None;

    for line in lines {
        let mut parts = line.split_whitespace();
        let tag = parts.next().unwrap_or("");
        match tag {
            "format" => {
                let fmt = parts.next().unwrap_or("");
                format = Some(match fmt {
                    "ascii" => PlyFormat::Ascii,
                    "binary_little_endian" => PlyFormat::BinaryLittleEndian,
                    "binary_big_endian" => PlyFormat::BinaryBigEndian,
                    _ => anyhow::bail!("Unsupported PLY format: {}", fmt),
                });
            }
            "element" => {
                if let Some(el) = current.take() {
                    elements.push(el);
                }
                let name = parts.next().unwrap_or("").to_string();
                let count: usize = parts.next().unwrap_or("0").parse().unwrap_or(0);
                current = Some(PlyElement {
                    name,
                    count,
                    properties: Vec::new(),
                });
            }
            "property" => {
                let current_el = current.as_mut().context("PLY property without element")?;
                let prop_type = parts.next().unwrap_or("");
                if prop_type == "list" {
                    let count_ty = parse_ply_scalar(parts.next().unwrap_or(""))?;
                    let item_ty = parse_ply_scalar(parts.next().unwrap_or(""))?;
                    let name = parts.next().unwrap_or("").to_string();
                    current_el.properties.push(PlyProperty::List {
                        name,
                        count_ty,
                        item_ty,
                    });
                } else {
                    let ty = parse_ply_scalar(prop_type)?;
                    let name = parts.next().unwrap_or("").to_string();
                    current_el.properties.push(PlyProperty::Scalar { name, ty });
                }
            }
            _ => {}
        }
    }

    if let Some(el) = current.take() {
        elements.push(el);
    }

    let format = format.context("PLY header missing format")?;
    Ok((format, elements))
}

fn parse_ply_scalar(s: &str) -> Result<PlyScalar> {
    match s {
        "char" | "int8" => Ok(PlyScalar::I8),
        "uchar" | "uint8" => Ok(PlyScalar::U8),
        "short" | "int16" => Ok(PlyScalar::I16),
        "ushort" | "uint16" => Ok(PlyScalar::U16),
        "int" | "int32" => Ok(PlyScalar::I32),
        "uint" | "uint32" => Ok(PlyScalar::U32),
        "float" | "float32" => Ok(PlyScalar::F32),
        "double" | "float64" => Ok(PlyScalar::F64),
        _ => anyhow::bail!("Unsupported PLY scalar type: {}", s),
    }
}

fn parse_ply_vertex_ascii(
    line: &str,
    properties: &[PlyProperty],
    vertices: &mut Vec<na::Point3<f32>>,
    normals: &mut Vec<na::Vector3<f32>>,
    colors: &mut Vec<[u8; 3]>,
    uvs: &mut Vec<[f32; 2]>,
) -> Result<()> {
    let mut iter = line.split_whitespace();
    let mut pos = [0.0f32; 3];
    let mut norm = [0.0f32; 3];
    let mut color = [255u8; 3];
    let mut uv = [0.0f32; 2];
    let mut has_norm = false;
    let mut has_color = false;
    let mut has_uv = false;

    for prop in properties {
        match prop {
            PlyProperty::Scalar { name, ty } => {
                let token = iter.next().context("PLY vertex line too short")?;
                let value = parse_ascii_scalar(token, *ty)?;
                match name.as_str() {
                    "x" => pos[0] = value.to_f64() as f32,
                    "y" => pos[1] = value.to_f64() as f32,
                    "z" => pos[2] = value.to_f64() as f32,
                    "nx" => {
                        norm[0] = value.to_f64() as f32;
                        has_norm = true;
                    }
                    "ny" => {
                        norm[1] = value.to_f64() as f32;
                        has_norm = true;
                    }
                    "nz" => {
                        norm[2] = value.to_f64() as f32;
                        has_norm = true;
                    }
                    "red" | "r" => {
                        color[0] = value.to_u8();
                        has_color = true;
                    }
                    "green" | "g" => {
                        color[1] = value.to_u8();
                        has_color = true;
                    }
                    "blue" | "b" => {
                        color[2] = value.to_u8();
                        has_color = true;
                    }
                    "u" | "s" | "texture_u" => {
                        uv[0] = value.to_f64() as f32;
                        has_uv = true;
                    }
                    "v" | "t" | "texture_v" => {
                        uv[1] = value.to_f64() as f32;
                        has_uv = true;
                    }
                    _ => {}
                }
            }
            PlyProperty::List { .. } => {
                // Ignore list properties on vertex
            }
        }
    }

    vertices.push(na::Point3::new(pos[0], pos[1], pos[2]));
    if has_norm {
        normals.push(na::Vector3::new(norm[0], norm[1], norm[2]));
    } else {
        normals.push(na::Vector3::new(0.0, 0.0, 1.0));
    }
    if has_color {
        colors.push(color);
    } else {
        colors.push([255, 255, 255]);
    }
    if has_uv {
        uvs.push(uv);
    } else {
        uvs.push([0.0, 0.0]);
    }

    Ok(())
}

fn parse_ply_face_ascii(
    line: &str,
    properties: &[PlyProperty],
    faces: &mut Vec<Face>,
) -> Result<()> {
    let mut iter = line.split_whitespace();
    for prop in properties {
        match prop {
            PlyProperty::List { name, .. }
                if name == "vertex_indices" || name == "vertex_index" =>
            {
                let count_token = iter.next().context("PLY face missing list count")?;
                let count: usize = count_token.parse().unwrap_or(0);
                let mut indices = Vec::with_capacity(count);
                for _ in 0..count {
                    let idx_token = iter.next().context("PLY face missing index")?;
                    let idx: usize = idx_token.parse().unwrap_or(0);
                    indices.push(idx);
                }
                triangulate_face(&indices, faces);
            }
            PlyProperty::Scalar { .. } => {
                iter.next();
            }
            PlyProperty::List { .. } => {
                let count_token = iter.next().context("PLY face missing list count")?;
                let count: usize = count_token.parse().unwrap_or(0);
                for _ in 0..count {
                    iter.next();
                }
            }
        }
    }
    Ok(())
}

fn parse_ply_vertex_binary<R: Read>(
    reader: &mut R,
    endian: PlyFormat,
    properties: &[PlyProperty],
    vertices: &mut Vec<na::Point3<f32>>,
    normals: &mut Vec<na::Vector3<f32>>,
    colors: &mut Vec<[u8; 3]>,
    uvs: &mut Vec<[f32; 2]>,
) -> Result<()> {
    let mut pos = [0.0f32; 3];
    let mut norm = [0.0f32; 3];
    let mut color = [255u8; 3];
    let mut uv = [0.0f32; 2];
    let mut has_norm = false;
    let mut has_color = false;
    let mut has_uv = false;

    for prop in properties {
        match prop {
            PlyProperty::Scalar { name, ty } => {
                let value = read_binary_scalar(reader, *ty, endian)?;
                match name.as_str() {
                    "x" => pos[0] = value.to_f64() as f32,
                    "y" => pos[1] = value.to_f64() as f32,
                    "z" => pos[2] = value.to_f64() as f32,
                    "nx" => {
                        norm[0] = value.to_f64() as f32;
                        has_norm = true;
                    }
                    "ny" => {
                        norm[1] = value.to_f64() as f32;
                        has_norm = true;
                    }
                    "nz" => {
                        norm[2] = value.to_f64() as f32;
                        has_norm = true;
                    }
                    "red" | "r" => {
                        color[0] = value.to_u8();
                        has_color = true;
                    }
                    "green" | "g" => {
                        color[1] = value.to_u8();
                        has_color = true;
                    }
                    "blue" | "b" => {
                        color[2] = value.to_u8();
                        has_color = true;
                    }
                    "u" | "s" | "texture_u" => {
                        uv[0] = value.to_f64() as f32;
                        has_uv = true;
                    }
                    "v" | "t" | "texture_v" => {
                        uv[1] = value.to_f64() as f32;
                        has_uv = true;
                    }
                    _ => {}
                }
            }
            PlyProperty::List {
                count_ty, item_ty, ..
            } => {
                let count = read_binary_scalar(reader, *count_ty, endian)?.to_u64() as usize;
                for _ in 0..count {
                    let _ = read_binary_scalar(reader, *item_ty, endian)?;
                }
            }
        }
    }

    vertices.push(na::Point3::new(pos[0], pos[1], pos[2]));
    if has_norm {
        normals.push(na::Vector3::new(norm[0], norm[1], norm[2]));
    } else {
        normals.push(na::Vector3::new(0.0, 0.0, 1.0));
    }
    if has_color {
        colors.push(color);
    } else {
        colors.push([255, 255, 255]);
    }
    if has_uv {
        uvs.push(uv);
    } else {
        uvs.push([0.0, 0.0]);
    }

    Ok(())
}

fn parse_ply_face_binary<R: Read>(
    reader: &mut R,
    endian: PlyFormat,
    properties: &[PlyProperty],
    faces: &mut Vec<Face>,
) -> Result<()> {
    for prop in properties {
        match prop {
            PlyProperty::List {
                name,
                count_ty,
                item_ty,
            } if name == "vertex_indices" || name == "vertex_index" => {
                let count = read_binary_scalar(reader, *count_ty, endian)?.to_u64() as usize;
                let mut indices = Vec::with_capacity(count);
                for _ in 0..count {
                    let idx = read_binary_scalar(reader, *item_ty, endian)?.to_u64() as usize;
                    indices.push(idx);
                }
                triangulate_face(&indices, faces);
            }
            PlyProperty::List {
                count_ty, item_ty, ..
            } => {
                let count = read_binary_scalar(reader, *count_ty, endian)?.to_u64() as usize;
                for _ in 0..count {
                    let _ = read_binary_scalar(reader, *item_ty, endian)?;
                }
            }
            PlyProperty::Scalar { ty, .. } => {
                let _ = read_binary_scalar(reader, *ty, endian)?;
            }
        }
    }
    Ok(())
}

fn skip_ply_element_binary<R: Read>(
    reader: &mut R,
    endian: PlyFormat,
    properties: &[PlyProperty],
) -> Result<()> {
    for prop in properties {
        match prop {
            PlyProperty::Scalar { ty, .. } => {
                let _ = read_binary_scalar(reader, *ty, endian)?;
            }
            PlyProperty::List {
                count_ty, item_ty, ..
            } => {
                let count = read_binary_scalar(reader, *count_ty, endian)?.to_u64() as usize;
                for _ in 0..count {
                    let _ = read_binary_scalar(reader, *item_ty, endian)?;
                }
            }
        }
    }
    Ok(())
}

fn parse_ascii_scalar(token: &str, ty: PlyScalar) -> Result<ScalarValue> {
    Ok(match ty {
        PlyScalar::I8 | PlyScalar::I16 | PlyScalar::I32 => {
            ScalarValue::I64(token.parse().unwrap_or(0))
        }
        PlyScalar::U8 | PlyScalar::U16 | PlyScalar::U32 => {
            ScalarValue::U64(token.parse().unwrap_or(0))
        }
        PlyScalar::F32 | PlyScalar::F64 => ScalarValue::F64(token.parse().unwrap_or(0.0)),
    })
}

fn read_binary_scalar<R: Read>(
    reader: &mut R,
    ty: PlyScalar,
    endian: PlyFormat,
) -> Result<ScalarValue> {
    Ok(match ty {
        PlyScalar::I8 => {
            let mut buf = [0u8; 1];
            reader.read_exact(&mut buf)?;
            ScalarValue::I64(i8::from_ne_bytes(buf) as i64)
        }
        PlyScalar::U8 => {
            let mut buf = [0u8; 1];
            reader.read_exact(&mut buf)?;
            ScalarValue::U64(buf[0] as u64)
        }
        PlyScalar::I16 => ScalarValue::I64(read_i16(reader, endian)? as i64),
        PlyScalar::U16 => ScalarValue::U64(read_u16(reader, endian)? as u64),
        PlyScalar::I32 => ScalarValue::I64(read_i32(reader, endian)? as i64),
        PlyScalar::U32 => ScalarValue::U64(read_u32(reader, endian)? as u64),
        PlyScalar::F32 => ScalarValue::F64(read_f32(reader, endian)? as f64),
        PlyScalar::F64 => ScalarValue::F64(read_f64(reader, endian)?),
    })
}

fn read_i16<R: Read>(reader: &mut R, endian: PlyFormat) -> Result<i16> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf)?;
    Ok(match endian {
        PlyFormat::BinaryBigEndian => i16::from_be_bytes(buf),
        _ => i16::from_le_bytes(buf),
    })
}

fn read_u16<R: Read>(reader: &mut R, endian: PlyFormat) -> Result<u16> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf)?;
    Ok(match endian {
        PlyFormat::BinaryBigEndian => u16::from_be_bytes(buf),
        _ => u16::from_le_bytes(buf),
    })
}

fn read_i32<R: Read>(reader: &mut R, endian: PlyFormat) -> Result<i32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(match endian {
        PlyFormat::BinaryBigEndian => i32::from_be_bytes(buf),
        _ => i32::from_le_bytes(buf),
    })
}

fn read_u32<R: Read>(reader: &mut R, endian: PlyFormat) -> Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(match endian {
        PlyFormat::BinaryBigEndian => u32::from_be_bytes(buf),
        _ => u32::from_le_bytes(buf),
    })
}

fn read_f32<R: Read>(reader: &mut R, endian: PlyFormat) -> Result<f32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(match endian {
        PlyFormat::BinaryBigEndian => f32::from_be_bytes(buf),
        _ => f32::from_le_bytes(buf),
    })
}

fn read_f64<R: Read>(reader: &mut R, endian: PlyFormat) -> Result<f64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(match endian {
        PlyFormat::BinaryBigEndian => f64::from_be_bytes(buf),
        _ => f64::from_le_bytes(buf),
    })
}

impl ScalarValue {
    fn to_f64(self) -> f64 {
        match self {
            ScalarValue::I64(v) => v as f64,
            ScalarValue::U64(v) => v as f64,
            ScalarValue::F64(v) => v,
        }
    }

    fn to_u64(self) -> u64 {
        match self {
            ScalarValue::I64(v) => v.max(0) as u64,
            ScalarValue::U64(v) => v,
            ScalarValue::F64(v) => v.max(0.0) as u64,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            ScalarValue::I64(v) => v.clamp(0, 255) as u8,
            ScalarValue::U64(v) => v.min(255) as u8,
            ScalarValue::F64(v) => {
                let scaled = if v <= 1.0 { v * 255.0 } else { v };
                scaled.round().clamp(0.0, 255.0) as u8
            }
        }
    }
}
