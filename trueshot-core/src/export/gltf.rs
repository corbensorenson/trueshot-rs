use crate::export::write_provenance_for_export;
use crate::reconstruction::Mesh;
use crate::security::provenance::ProvenanceSigner;
use anyhow::{Context, Result};
use serde_json::json;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// Export mesh as GLTF 2.0 with separate binary buffer
pub fn export_gltf(mesh: &Mesh, path: &Path) -> Result<()> {
    tracing::info!("Exporting GLTF to {:?}", path);

    if mesh.is_empty() {
        anyhow::bail!("Cannot export empty mesh");
    }

    // Calculate buffer layout
    let vertex_count = mesh.vertices.len();
    let face_count = mesh.faces.len();
    let index_count = face_count * 3;

    // Buffer sizes (all data tightly packed)
    let position_size = vertex_count * 12; // 3 floats * 4 bytes
    let color_size = vertex_count * 4; // RGBA normalized u8
    let normal_size = if !mesh.normals.is_empty() {
        vertex_count * 12
    } else {
        0
    };
    let index_size = index_count * 4; // u32 indices

    let total_buffer_size = position_size + color_size + normal_size + index_size;

    // Calculate byte offsets
    let position_offset = 0;
    let color_offset = position_size;
    let normal_offset = color_offset + color_size;
    let index_offset = if normal_size > 0 {
        normal_offset + normal_size
    } else {
        color_offset + color_size
    };

    // Compute bounding box for positions
    let (min_pos, max_pos) = compute_bounds(&mesh.vertices);

    // Build buffer filename
    let bin_filename = path
        .file_stem()
        .map(|s| format!("{}.bin", s.to_string_lossy()))
        .unwrap_or_else(|| "model.bin".to_string());

    // Build accessors
    let mut accessors = vec![
        // Accessor 0: POSITION
        json!({
            "bufferView": 0,
            "componentType": 5126, // FLOAT
            "count": vertex_count,
            "type": "VEC3",
            "min": [min_pos.x, min_pos.y, min_pos.z],
            "max": [max_pos.x, max_pos.y, max_pos.z]
        }),
        // Accessor 1: COLOR_0
        json!({
            "bufferView": 1,
            "componentType": 5121, // UNSIGNED_BYTE
            "normalized": true,
            "count": vertex_count,
            "type": "VEC4"
        }),
    ];

    let mut buffer_views = vec![
        // BufferView 0: Positions
        json!({
            "buffer": 0,
            "byteOffset": position_offset,
            "byteLength": position_size,
            "target": 34962 // ARRAY_BUFFER
        }),
        // BufferView 1: Colors
        json!({
            "buffer": 0,
            "byteOffset": color_offset,
            "byteLength": color_size,
            "target": 34962
        }),
    ];

    let mut attributes = json!({
        "POSITION": 0,
        "COLOR_0": 1
    });

    let mut next_accessor = 2;
    let mut next_buffer_view = 2;

    // Add normals if present
    if !mesh.normals.is_empty() {
        buffer_views.push(json!({
            "buffer": 0,
            "byteOffset": normal_offset,
            "byteLength": normal_size,
            "target": 34962
        }));
        accessors.push(json!({
            "bufferView": next_buffer_view,
            "componentType": 5126,
            "count": vertex_count,
            "type": "VEC3"
        }));
        attributes["NORMAL"] = json!(next_accessor);
        next_accessor += 1;
        next_buffer_view += 1;
    }

    // Add indices
    buffer_views.push(json!({
        "buffer": 0,
        "byteOffset": index_offset,
        "byteLength": index_size,
        "target": 34963 // ELEMENT_ARRAY_BUFFER
    }));
    let indices_accessor = next_accessor;
    accessors.push(json!({
        "bufferView": next_buffer_view,
        "componentType": 5125, // UNSIGNED_INT
        "count": index_count,
        "type": "SCALAR"
    }));

    let provenance_sidecar = format!(
        "{}.provenance.json",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("asset")
    );
    let provenance_key_id = ProvenanceSigner::global().key_id();

    // Build complete GLTF JSON
    let gltf = json!({
        "asset": {
            "version": "2.0",
            "generator": "TrueShot Core",
            "extras": {
                "trueshot:provenance_sidecar": provenance_sidecar,
                "trueshot:provenance_key_id": provenance_key_id,
            }
        },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{
            "mesh": 0,
            "name": "TrueShot Mesh"
        }],
        "meshes": [{
            "name": "Mesh",
            "primitives": [{
                "attributes": attributes,
                "indices": indices_accessor,
                "mode": 4 // TRIANGLES
            }]
        }],
        "buffers": [{
            "uri": bin_filename,
            "byteLength": total_buffer_size
        }],
        "bufferViews": buffer_views,
        "accessors": accessors
    });

    // Write GLTF JSON
    let file =
        File::create(path).with_context(|| format!("Failed to create GLTF file: {:?}", path))?;
    serde_json::to_writer_pretty(file, &gltf).context("Failed to write GLTF JSON")?;

    // Write binary buffer
    let bin_path = path.with_extension("bin");
    write_binary_buffer(mesh, &bin_path)?;
    write_provenance_for_export(path)?;
    write_provenance_for_export(&bin_path)?;

    tracing::info!(
        "Exported GLTF: {} vertices, {} faces, {} bytes",
        vertex_count,
        face_count,
        total_buffer_size
    );
    Ok(())
}

/// Export mesh as GLB (single binary file)
pub fn export_glb(mesh: &Mesh, path: &Path) -> Result<()> {
    tracing::info!("Exporting GLB to {:?}", path);
    let file = File::create(path)?;
    export_glb_to_writer(
        mesh,
        file,
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("asset.glb"),
    )?;
    write_provenance_for_export(path)?;
    Ok(())
}

pub fn export_glb_to_writer<W: Write>(mesh: &Mesh, writer: W, asset_name: &str) -> Result<()> {
    if mesh.is_empty() {
        anyhow::bail!("Cannot export empty mesh");
    }

    // Build binary buffer
    let bin_data = build_binary_data(mesh)?;

    // Build JSON (same as GLTF but with embedded buffer)
    let vertex_count = mesh.vertices.len();
    let face_count = mesh.faces.len();
    let index_count = face_count * 3;

    let position_size = vertex_count * 12;
    let color_size = vertex_count * 4;
    let normal_size = if !mesh.normals.is_empty() {
        vertex_count * 12
    } else {
        0
    };
    let index_size = index_count * 4;

    let (min_pos, max_pos) = compute_bounds(&mesh.vertices);

    let mut accessors = vec![
        json!({
            "bufferView": 0,
            "componentType": 5126,
            "count": vertex_count,
            "type": "VEC3",
            "min": [min_pos.x, min_pos.y, min_pos.z],
            "max": [max_pos.x, max_pos.y, max_pos.z]
        }),
        json!({
            "bufferView": 1,
            "componentType": 5121,
            "normalized": true,
            "count": vertex_count,
            "type": "VEC4"
        }),
    ];

    let mut buffer_views = vec![
        json!({ "buffer": 0, "byteOffset": 0, "byteLength": position_size, "target": 34962 }),
        json!({ "buffer": 0, "byteOffset": position_size, "byteLength": color_size, "target": 34962 }),
    ];

    let mut attributes = json!({ "POSITION": 0, "COLOR_0": 1 });
    let mut next_accessor = 2;
    let mut offset = position_size + color_size;

    if !mesh.normals.is_empty() {
        buffer_views.push(json!({ "buffer": 0, "byteOffset": offset, "byteLength": normal_size, "target": 34962 }));
        accessors.push(json!({ "bufferView": buffer_views.len() - 1, "componentType": 5126, "count": vertex_count, "type": "VEC3" }));
        attributes["NORMAL"] = json!(next_accessor);
        next_accessor += 1;
        offset += normal_size;
    }

    buffer_views.push(
        json!({ "buffer": 0, "byteOffset": offset, "byteLength": index_size, "target": 34963 }),
    );
    accessors.push(json!({ "bufferView": buffer_views.len() - 1, "componentType": 5125, "count": index_count, "type": "SCALAR" }));
    let indices_accessor = next_accessor;

    let provenance_sidecar = format!(
        "{}.provenance.json",
        Path::new(asset_name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("asset")
    );
    let provenance_key_id = ProvenanceSigner::global().key_id();
    let gltf = json!({
        "asset": {
            "version": "2.0",
            "generator": "TrueShot Core",
            "extras": {
                "trueshot:provenance_sidecar": provenance_sidecar,
                "trueshot:provenance_key_id": provenance_key_id,
            }
        },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{ "mesh": 0, "name": "TrueShot Mesh" }],
        "meshes": [{ "name": "Mesh", "primitives": [{ "attributes": attributes, "indices": indices_accessor, "mode": 4 }] }],
        "buffers": [{ "byteLength": bin_data.len() }],
        "bufferViews": buffer_views,
        "accessors": accessors
    });

    let json_str = serde_json::to_string(&gltf)?;
    let json_bytes = json_str.as_bytes();

    // Pad JSON to 4-byte boundary
    let json_padded_len = (json_bytes.len() + 3) & !3;
    let bin_padded_len = (bin_data.len() + 3) & !3;

    // GLB structure
    let total_length = 12 + 8 + json_padded_len + 8 + bin_padded_len;

    let mut writer = BufWriter::new(writer);

    // GLB Header
    writer.write_all(b"glTF")?; // magic
    writer.write_all(&2u32.to_le_bytes())?; // version
    writer.write_all(&(total_length as u32).to_le_bytes())?; // length

    // JSON chunk
    writer.write_all(&(json_padded_len as u32).to_le_bytes())?; // chunkLength
    writer.write_all(&0x4E4F534Au32.to_le_bytes())?; // chunkType: JSON
    writer.write_all(json_bytes)?;
    for _ in 0..(json_padded_len - json_bytes.len()) {
        writer.write_all(b" ")?;
    }

    // BIN chunk
    writer.write_all(&(bin_padded_len as u32).to_le_bytes())?; // chunkLength
    writer.write_all(&0x004E4942u32.to_le_bytes())?; // chunkType: BIN
    writer.write_all(&bin_data)?;
    for _ in 0..(bin_padded_len - bin_data.len()) {
        writer.write_all(&[0])?;
    }

    writer.flush()?;

    tracing::info!(
        "Exported GLB: {} vertices, {} faces",
        vertex_count,
        face_count
    );
    Ok(())
}

fn write_binary_buffer(mesh: &Mesh, path: &Path) -> Result<()> {
    let data = build_binary_data(mesh)?;
    let mut file = File::create(path)
        .with_context(|| format!("Failed to create binary buffer: {:?}", path))?;
    file.write_all(&data)?;
    Ok(())
}

fn build_binary_data(mesh: &Mesh) -> Result<Vec<u8>> {
    let vertex_count = mesh.vertices.len();
    let index_count = mesh.faces.len() * 3;

    let position_size = vertex_count * 12;
    let color_size = vertex_count * 4;
    let normal_size = if !mesh.normals.is_empty() {
        vertex_count * 12
    } else {
        0
    };
    let index_size = index_count * 4;

    let total_size = position_size + color_size + normal_size + index_size;
    let mut data = Vec::with_capacity(total_size);

    // Write positions (VEC3 floats)
    for v in &mesh.vertices {
        data.extend_from_slice(&v.x.to_le_bytes());
        data.extend_from_slice(&v.y.to_le_bytes());
        data.extend_from_slice(&v.z.to_le_bytes());
    }

    // Write colors (RGBA u8, alpha = 255)
    for c in &mesh.colors {
        data.push(c[0]);
        data.push(c[1]);
        data.push(c[2]);
        data.push(255); // Alpha
    }
    // Pad if fewer colors than vertices
    for _ in mesh.colors.len()..vertex_count {
        data.extend_from_slice(&[128, 128, 128, 255]); // Gray default
    }

    // Write normals if present
    for n in &mesh.normals {
        data.extend_from_slice(&n.x.to_le_bytes());
        data.extend_from_slice(&n.y.to_le_bytes());
        data.extend_from_slice(&n.z.to_le_bytes());
    }

    // Write indices (u32)
    for face in &mesh.faces {
        data.extend_from_slice(&(face.vertices[0] as u32).to_le_bytes());
        data.extend_from_slice(&(face.vertices[1] as u32).to_le_bytes());
        data.extend_from_slice(&(face.vertices[2] as u32).to_le_bytes());
    }

    Ok(data)
}

fn compute_bounds(
    vertices: &[nalgebra::Point3<f32>],
) -> (nalgebra::Point3<f32>, nalgebra::Point3<f32>) {
    if vertices.is_empty() {
        return (nalgebra::Point3::origin(), nalgebra::Point3::origin());
    }

    let mut min = vertices[0];
    let mut max = vertices[0];

    for v in vertices {
        min.x = min.x.min(v.x);
        min.y = min.y.min(v.y);
        min.z = min.z.min(v.z);
        max.x = max.x.max(v.x);
        max.y = max.y.max(v.y);
        max.z = max.z.max(v.z);
    }

    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reconstruction::Face;
    use nalgebra::Point3;

    #[test]
    fn test_export_gltf_simple() {
        let mesh = Mesh {
            vertices: vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
                Point3::new(0.5, 1.0, 0.0),
            ],
            colors: vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]],
            normals: vec![],
            uvs: vec![],
            faces: vec![Face {
                vertices: [0, 1, 2],
            }],
        };

        let temp_dir = std::env::temp_dir();
        let gltf_path = temp_dir.join("test_export.gltf");

        export_gltf(&mesh, &gltf_path).expect("Export failed");

        assert!(gltf_path.exists());
        assert!(gltf_path.with_extension("bin").exists());
    }

    #[test]
    fn test_export_glb() {
        let mesh = Mesh {
            vertices: vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
                Point3::new(0.5, 1.0, 0.0),
            ],
            colors: vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]],
            normals: vec![],
            uvs: vec![],
            faces: vec![Face {
                vertices: [0, 1, 2],
            }],
        };

        let temp_dir = std::env::temp_dir();
        let glb_path = temp_dir.join("test_export.glb");

        export_glb(&mesh, &glb_path).expect("Export failed");

        assert!(glb_path.exists());

        // Verify GLB magic bytes
        let data = std::fs::read(&glb_path).unwrap();
        assert_eq!(&data[0..4], b"glTF");
    }
}
