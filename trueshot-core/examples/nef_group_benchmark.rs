use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;
use trueshot_core::nef::parser::Z9NefParser;
use trueshot_core::nef::raw_data::Roi;
use trueshot_core::smart_loader::{NativeGroupArena, SmartLoader};
use trueshot_core::timing::HierarchicalTimer;
use trueshot_core::types::ProcessingOptions;

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let input = args
        .next()
        .map(PathBuf::from)
        .context("usage: nef_group_benchmark <nef_dir> [--verify-full] [--workers N]")?;
    let trailing: Vec<_> = args.collect();
    let verify_full = trailing.iter().any(|arg| arg == "--verify-full");
    let workers = trailing
        .windows(2)
        .find(|pair| pair[0] == "--workers")
        .and_then(|pair| pair[1].to_str())
        .and_then(|value| value.parse::<usize>().ok());
    let storage_class = trailing
        .windows(2)
        .find(|pair| pair[0] == "--storage-class")
        .and_then(|pair| pair[1].to_str())
        .unwrap_or("unspecified")
        .to_string();

    let options = ProcessingOptions {
        full_decode: false,
        max_parallel_sequences: workers,
        ..ProcessingOptions::default()
    };
    let loader = SmartLoader::new(options);
    let sequences = loader.scan_and_group(&input)?;
    let mut arena = NativeGroupArena::default();
    let run_start = Instant::now();
    let mut decoded_frames = 0usize;
    let mut decoded_pixels = 0usize;
    let mut latency_by_layout: BTreeMap<String, Vec<f64>> = BTreeMap::new();

    for (sequence_index, sequence) in sequences.iter().enumerate() {
        let mut timer = HierarchicalTimer::new(&sequence.meta.bone_id);
        let group_start = Instant::now();
        let group = loader.load_sequence_native_into(sequence, &mut arena, &mut timer)?;
        let group_ms = group_start.elapsed().as_secs_f64() * 1000.0;
        let (x0, y0, x1, y1) = group.rect.to_bounds();
        let roi = Roi::new(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);
        let layout = group
            .metadata
            .first()
            .map(|metadata| {
                format!(
                    "{} {} compression={} bits={} strips={} storage={}",
                    metadata.camera_make,
                    metadata.camera_model,
                    metadata.compression,
                    metadata.bits_per_sample,
                    metadata.strip_offsets.len(),
                    storage_class
                )
            })
            .unwrap_or_else(|| format!("unknown storage={storage_class}"));
        latency_by_layout.entry(layout).or_default().push(group_ms);

        if verify_full {
            for (frame_index, path) in sequence.paths.iter().enumerate() {
                let mut parser = Z9NefParser::new(path);
                parser.parse()?;
                let full = parser.load_full()?;
                let native = group
                    .frame(frame_index)
                    .context("Native group frame is missing")?;
                let exact = (0..roi.height).all(|y| {
                    (0..roi.width).all(|x| {
                        native[(y * roi.width + x) as usize]
                            == full.get_pixel(roi.x + x, roi.y + y).unwrap()
                    })
                });
                if !exact {
                    anyhow::bail!(
                        "Native group frame {} differs from full decode: {}",
                        frame_index,
                        path.display()
                    );
                }
            }
        }

        decoded_frames += group.len();
        decoded_pixels += group.len() * group.width * group.height;
        println!(
            "sequence={sequence_index} frames={} roi={}x{} native_mib={:.2} decode_ms={group_ms:.2} parity={}",
            group.len(),
            group.width,
            group.height,
            group.size_bytes() as f64 / 1024.0 / 1024.0,
            if verify_full { "exact" } else { "not_requested" }
        );
    }

    let elapsed = run_start.elapsed().as_secs_f64();
    println!(
        "sequences={} frames={} megapixels={:.2} elapsed_s={elapsed:.3} frames_per_s={:.2} arena_capacity_mib={:.2}",
        sequences.len(),
        decoded_frames,
        decoded_pixels as f64 / 1_000_000.0,
        decoded_frames as f64 / elapsed.max(f64::EPSILON),
        arena.capacity_bytes() as f64 / 1024.0 / 1024.0
    );
    for (layout, mut latencies) in latency_by_layout {
        latencies.sort_unstable_by(f64::total_cmp);
        println!(
            "latency_layout=\"{}\" groups={} p50_ms={:.2} p95_ms={:.2} p99_ms={:.2}",
            layout,
            latencies.len(),
            percentile(&latencies, 0.50),
            percentile(&latencies, 0.95),
            percentile(&latencies, 0.99),
        );
    }
    Ok(())
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    let index = ((sorted.len().saturating_sub(1)) as f64 * quantile).round() as usize;
    sorted.get(index).copied().unwrap_or(0.0)
}
