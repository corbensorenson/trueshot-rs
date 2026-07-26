use criterion::{black_box, criterion_group, criterion_main, Criterion};
use trueshot_core::photogrammetry::heatmap::{apply_heatmap_to_points, ColoredPoint};
use nalgebra as na;

fn benchmark_heatmap(c: &mut Criterion) {
    let points: Vec<ColoredPoint> = (0..10000).map(|i| {
        ColoredPoint {
            position: na::Point3::new((i % 100) as f32, (i / 100) as f32, 0.0),
            color: [255, 255, 255],
            confidence: 1.0,
        }
    }).collect();

    c.bench_function("heatmap_generation_10k", |b| b.iter(|| {
        apply_heatmap_to_points(black_box(&points), black_box(1.0))
    }));
}

criterion_group!(benches, benchmark_heatmap);
criterion_main!(benches);
