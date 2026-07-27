use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use zstd;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SplatPoint {
    pub position: [f32; 3],
    pub scale: [f32; 3],
    pub color: [u8; 4],
    pub rotation: [u8; 4],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SplatEditOp {
    PruneOpacity { min_alpha: u8 },
    Bounds { min: [f32; 3], max: [f32; 3] },
    Sphere { center: [f32; 3], radius: f32 },
    Density { target: usize },
}

pub fn load_splat(path: &Path) -> Result<Vec<SplatPoint>> {
    let mut file = File::open(path)
        .with_context(|| format!("Failed to open splat file: {}", path.display()))?;
    load_splat_from_reader(&mut file)
}

pub fn load_splat_from_reader<R: Read>(mut reader: R) -> Result<Vec<SplatPoint>> {
    let mut data = Vec::new();
    reader.read_to_end(&mut data)?;
    parse_splat_bytes(&data)
}

pub fn save_splat(path: &Path, points: &[SplatPoint]) -> Result<()> {
    let mut file = File::create(path)
        .with_context(|| format!("Failed to write splat file: {}", path.display()))?;
    save_splat_to_writer(&mut file, points)
}

pub fn save_splat_to_writer<W: Write>(mut writer: W, points: &[SplatPoint]) -> Result<()> {
    let data = build_splat_bytes(points);
    writer.write_all(&data)?;
    Ok(())
}

pub fn save_spz(path: &Path, points: &[SplatPoint]) -> Result<()> {
    let mut file = File::create(path)
        .with_context(|| format!("Failed to write spz file: {}", path.display()))?;
    save_spz_to_writer(&mut file, points)
}

pub fn save_spz_to_writer<W: Write>(mut writer: W, points: &[SplatPoint]) -> Result<()> {
    let payload = build_splat_bytes(points);
    let compressed = zstd::encode_all(payload.as_slice(), 3)?;
    writer.write_all(b"SPZ1")?;
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(&compressed)?;
    Ok(())
}

pub fn apply_splat_edits(mut points: Vec<SplatPoint>, ops: &[SplatEditOp]) -> Vec<SplatPoint> {
    for op in ops {
        match *op {
            SplatEditOp::PruneOpacity { min_alpha } => {
                points.retain(|p| p.color[3] >= min_alpha);
            }
            SplatEditOp::Bounds { min, max } => {
                points.retain(|p| {
                    p.position[0] >= min[0]
                        && p.position[0] <= max[0]
                        && p.position[1] >= min[1]
                        && p.position[1] <= max[1]
                        && p.position[2] >= min[2]
                        && p.position[2] <= max[2]
                });
            }
            SplatEditOp::Sphere { center, radius } => {
                let r2 = radius * radius;
                points.retain(|p| {
                    let dx = p.position[0] - center[0];
                    let dy = p.position[1] - center[1];
                    let dz = p.position[2] - center[2];
                    dx * dx + dy * dy + dz * dz <= r2
                });
            }
            SplatEditOp::Density { target } => {
                if points.len() > target {
                    points.sort_by(|a, b| b.color[3].cmp(&a.color[3]));
                    points.truncate(target);
                }
            }
        }
    }
    points
}

fn parse_splat_bytes(data: &[u8]) -> Result<Vec<SplatPoint>> {
    if data.len() % 32 != 0 {
        anyhow::bail!("Invalid splat file length");
    }
    let count = data.len() / 32;
    let mut points = Vec::with_capacity(count);
    for i in 0..count {
        let offset = i * 32;
        let fx = f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
        let fy = f32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap());
        let fz = f32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap());
        let sx = f32::from_le_bytes(data[offset + 12..offset + 16].try_into().unwrap());
        let sy = f32::from_le_bytes(data[offset + 16..offset + 20].try_into().unwrap());
        let sz = f32::from_le_bytes(data[offset + 20..offset + 24].try_into().unwrap());
        let color = [
            data[offset + 24],
            data[offset + 25],
            data[offset + 26],
            data[offset + 27],
        ];
        let rotation = [
            data[offset + 28],
            data[offset + 29],
            data[offset + 30],
            data[offset + 31],
        ];
        points.push(SplatPoint {
            position: [fx, fy, fz],
            scale: [sx, sy, sz],
            color,
            rotation,
        });
    }
    Ok(points)
}

fn build_splat_bytes(points: &[SplatPoint]) -> Vec<u8> {
    let mut data = Vec::with_capacity(points.len() * 32);
    for p in points {
        data.extend_from_slice(&p.position[0].to_le_bytes());
        data.extend_from_slice(&p.position[1].to_le_bytes());
        data.extend_from_slice(&p.position[2].to_le_bytes());
        data.extend_from_slice(&p.scale[0].to_le_bytes());
        data.extend_from_slice(&p.scale[1].to_le_bytes());
        data.extend_from_slice(&p.scale[2].to_le_bytes());
        data.extend_from_slice(&p.color);
        data.extend_from_slice(&p.rotation);
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_backed_splat_codec_round_trips() {
        let points = vec![SplatPoint {
            position: [1.0, 2.0, 3.0],
            scale: [0.1, 0.2, 0.3],
            color: [10, 20, 30, 40],
            rotation: [1, 2, 3, 4],
        }];
        let mut encoded = Vec::new();
        save_splat_to_writer(&mut encoded, &points).unwrap();
        let decoded = load_splat_from_reader(encoded.as_slice()).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].position, points[0].position);
        assert_eq!(decoded[0].color, points[0].color);
    }
}
