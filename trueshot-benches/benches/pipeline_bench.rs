//! Capture-guidance pipeline benchmarks.
//!
//! These gates track the sparse coverage data structure used by live capture
//! guidance without introducing filesystem or reconstruction noise.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use nalgebra as na;
use trueshot_core::photogrammetry::heatmap::CoverageVoxelGrid;
use trueshot_core::reconstruction::ColoredPoint;

fn coverage_points(count: usize) -> Vec<ColoredPoint> {
    (0..count)
        .map(|i| {
            let ring = (i / 1_000) as f32;
            let angle = i as f32 * 0.061_803_4;
            ColoredPoint {
                position: na::Point3::new(
                    angle.cos() * (1.0 + ring * 0.01),
                    angle.sin() * (1.0 + ring * 0.01),
                    (i % 257) as f32 * 0.002,
                ),
                color: [255, 255, 255],
                confidence: 1.0,
            }
        })
        .collect()
}

fn benchmark_coverage_ingest(c: &mut Criterion) {
    let mut group = c.benchmark_group("capture_coverage_ingest");
    for count in [1_000, 10_000, 100_000] {
        let points = coverage_points(count);
        group.bench_with_input(BenchmarkId::from_parameter(count), &points, |b, points| {
            b.iter(|| {
                let mut grid = CoverageVoxelGrid::new(black_box(0.02));
                grid.add_points(black_box(points));
                black_box(grid.get_stats())
            });
        });
    }
    group.finish();
}

fn benchmark_coverage_queries(c: &mut Criterion) {
    let points = coverage_points(100_000);
    let mut grid = CoverageVoxelGrid::new(0.02);
    grid.add_points(&points);
    let queries: Vec<_> = points
        .iter()
        .step_by(10)
        .map(|point| point.position)
        .collect();

    c.bench_function("capture_coverage_query_10k", |b| {
        b.iter(|| {
            for query in &queries {
                black_box(grid.get_density(black_box(query)));
            }
        });
    });
}

criterion_group!(
    benches,
    benchmark_coverage_ingest,
    benchmark_coverage_queries
);
criterion_main!(benches);
