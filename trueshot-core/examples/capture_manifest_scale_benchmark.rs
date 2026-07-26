use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;
use sysinfo::{Pid, System};
use trueshot_core::capture_manifest::{
    CaptureGroup, CaptureManifestHeader, CaptureManifestReader, CAPTURE_MANIFEST_SCHEMA,
};
use trueshot_core::types::{Meta, Sequence};

#[derive(Serialize)]
#[serde(tag = "record_type", rename_all = "snake_case")]
enum Record<'a> {
    Header(&'a CaptureManifestHeader),
    Group(&'a CaptureGroup),
}

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let count = arguments
        .next()
        .and_then(|value| value.to_str().and_then(|value| value.parse::<u64>().ok()))
        .unwrap_or(1_000_000);
    let root = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("trueshot-manifest-scale"));
    let keep = arguments.any(|value| value == "--keep");
    run(count, &root, keep)
}

fn run(count: u64, root: &Path, keep: bool) -> Result<()> {
    let manifest_directory = root.join(".trueshot");
    std::fs::create_dir_all(&manifest_directory)?;
    let path = manifest_directory.join("capture_manifest.v1.jsonl");
    let generation_started = Instant::now();
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)?;
    let mut writer = BufWriter::with_capacity(1024 * 1024, file);
    let header = CaptureManifestHeader {
        schema: CAPTURE_MANIFEST_SCHEMA.to_string(),
        created_at: Utc::now(),
        group_count: count,
        source_root: PathBuf::from(".."),
    };
    serde_json::to_writer(&mut writer, &Record::Header(&header))?;
    writer.write_all(b"\n")?;
    for index in 0..count {
        let sequence = Sequence {
            paths: vec![
                PathBuf::from(format!("virtual/{index}/f0e0.nef")),
                PathBuf::from(format!("virtual/{index}/f0e1.nef")),
            ],
            meta: Meta {
                focus_steps: 1,
                exposures: vec![-1.0, 1.0],
                shutter_speeds: vec![0.004, 0.016],
                ref_focus: 0,
                ref_exp: 0.0,
                rot_deg: (index % 360) as f32,
                vantage: "mid".to_string(),
                burst_factor: 1,
                bone_id: format!("scale-{index}"),
                cam_mul: [2.0, 1.0, 1.5, 1.0],
            },
        };
        let group = CaptureGroup::from_sequence(sequence);
        serde_json::to_writer(&mut writer, &Record::Group(&group))?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    writer.get_ref().sync_all()?;
    drop(writer);
    let generation_elapsed = generation_started.elapsed();
    let manifest_bytes = std::fs::metadata(&path)?.len();

    let mut system = System::new();
    let pid = Pid::from_u32(std::process::id());
    system.refresh_process(pid);
    let baseline_rss = system
        .process(pid)
        .context("Current process is absent from system telemetry")?
        .memory();
    let read_started = Instant::now();
    let mut reader = CaptureManifestReader::open(&path)?;
    let mut read = 0u64;
    let mut peak_rss = baseline_rss;
    while let Some(group) = reader.next() {
        let group = group?;
        if group.sequence.paths.len() != 2 {
            anyhow::bail!("Scale manifest group has wrong frame count");
        }
        read += 1;
        if read % 10_000 == 0 || read == count {
            system.refresh_process(pid);
            peak_rss = peak_rss.max(
                system
                    .process(pid)
                    .context("Current process disappeared from system telemetry")?
                    .memory(),
            );
        }
    }
    if read != count {
        anyhow::bail!("Read {read} groups, expected {count}");
    }
    let rss_growth = peak_rss.saturating_sub(baseline_rss);
    let rss_limit = 64 * 1024 * 1024;
    if rss_growth > rss_limit {
        anyhow::bail!(
            "Manifest reader RSS grew {:.1} MiB; limit is {:.1} MiB",
            rss_growth as f64 / (1024.0 * 1024.0),
            rss_limit as f64 / (1024.0 * 1024.0)
        );
    }
    println!(
        "groups={count} bytes={manifest_bytes} generation={:.2}s read={:.2}s baseline_rss={:.1}MiB peak_rss={:.1}MiB growth={:.1}MiB",
        generation_elapsed.as_secs_f64(),
        read_started.elapsed().as_secs_f64(),
        baseline_rss as f64 / (1024.0 * 1024.0),
        peak_rss as f64 / (1024.0 * 1024.0),
        rss_growth as f64 / (1024.0 * 1024.0),
    );
    if !keep {
        std::fs::remove_file(&path)?;
        let _ = std::fs::remove_dir(&manifest_directory);
        let _ = std::fs::remove_dir(root);
    } else {
        println!("manifest={}", path.display());
    }
    Ok(())
}
