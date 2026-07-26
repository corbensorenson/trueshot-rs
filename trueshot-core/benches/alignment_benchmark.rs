//! Benchmarks the native FFT alignment path used by RAW fusion.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use ndarray::Array2;
use trueshot_core::align_raw::align_phasecorr_gray_with_scale;

fn patterned_image(size: usize) -> Array2<f64> {
    Array2::from_shape_fn((size, size), |(y, x)| {
        let checker = ((x / 8) + (y / 8)) & 1;
        let wave = ((x as f64 * 0.071).sin() + (y as f64 * 0.113).cos()) * 0.25;
        checker as f64 * 0.5 + wave
    })
}

fn shifted_image(source: &Array2<f64>, dx: isize, dy: isize) -> Array2<f64> {
    let (height, width) = source.dim();
    Array2::from_shape_fn((height, width), |(y, x)| {
        let source_x = (x as isize - dx).clamp(0, width as isize - 1) as usize;
        let source_y = (y as isize - dy).clamp(0, height as isize - 1) as usize;
        source[[source_y, source_x]]
    })
}

fn benchmark_phase_correlation(c: &mut Criterion) {
    let mut group = c.benchmark_group("native_phase_correlation");

    for size in [64_usize, 128, 256] {
        let reference = patterned_image(size);
        let shifted = shifted_image(&reference, 5, -3);
        group.bench_with_input(BenchmarkId::new("shift_and_scale", size), &size, |b, _| {
            b.iter(|| {
                black_box(align_phasecorr_gray_with_scale(
                    black_box(&reference),
                    black_box(&shifted),
                    1,
                ))
            });
        });
    }

    group.finish();
}

criterion_group!(benches, benchmark_phase_correlation);
criterion_main!(benches);
