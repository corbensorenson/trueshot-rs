//! Benchmarks the shipping HDR and focus-fusion implementations.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use image::{DynamicImage, Rgb, RgbImage};
use trueshot_core::capture::{
    FocusStackConfig, FocusStacker, HdrAlgorithm, HdrConfig, HdrMerger, StackAlgorithm,
};

fn synthetic_images(count: usize, size: u32) -> Vec<DynamicImage> {
    (0..count)
        .map(|frame| {
            DynamicImage::ImageRgb8(RgbImage::from_fn(size, size, |x, y| {
                let checker = (((x / 8) + (y / 8) + frame as u32) & 1) as u8;
                let gradient = ((x + y + frame as u32 * 11) & 0xff) as u8;
                Rgb([
                    gradient.saturating_add(checker * 32),
                    gradient,
                    gradient.saturating_sub(checker * 16),
                ])
            }))
        })
        .collect()
}

fn benchmark_focus_fusion(c: &mut Criterion) {
    let mut group = c.benchmark_group("focus_fusion");
    let stacker = FocusStacker::new(FocusStackConfig {
        algorithm: StackAlgorithm::WeightedFocus,
        align_images: false,
        ..FocusStackConfig::default()
    });

    for size in [64_u32, 128, 256] {
        let images = synthetic_images(4, size);
        group.bench_with_input(
            BenchmarkId::new("weighted_4_frames", size),
            &size,
            |b, _| b.iter(|| black_box(stacker.stack(black_box(&images)).expect("focus fusion"))),
        );
    }
    group.finish();
}

fn benchmark_hdr_fusion(c: &mut Criterion) {
    let mut group = c.benchmark_group("hdr_fusion");
    let merger = HdrMerger::new(HdrConfig {
        bracket_count: 3,
        algorithm: HdrAlgorithm::MertensFusion,
        align_images: false,
        ..HdrConfig::default()
    });
    let evs = [-2.0, 0.0, 2.0];

    for size in [64_u32, 128, 256] {
        let images = synthetic_images(evs.len(), size);
        group.bench_with_input(BenchmarkId::new("mertens_3_frames", size), &size, |b, _| {
            b.iter(|| {
                black_box(
                    merger
                        .merge(black_box(&images), black_box(&evs))
                        .expect("HDR fusion"),
                )
            })
        });
    }
    group.finish();
}

criterion_group!(benches, benchmark_focus_fusion, benchmark_hdr_fusion);
criterion_main!(benches);
