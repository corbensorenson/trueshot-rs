#[path = "../trueshot-core/src/focus_evidence.rs"]
mod focus_evidence;

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
fn main() {
    use focus_evidence::{
        compute_focus_metric_apple_neon, compute_focus_metric_scalar,
        compute_trimmed_focus_mean, sorted_trimmed_focus_mean_at,
    };
    use std::hint::black_box;
    use std::time::{Duration, Instant};

    let width = 2_048;
    let height = 1_536;
    let green = (0..width * height)
        .map(|index| {
            let x = (index % width) as f32;
            let y = (index / width) as f32;
            (0.45
                + 0.18 * (x * 0.071).sin()
                + 0.13 * (y * 0.053).cos()
                + 0.04 * ((x + y) * 0.29).sin())
            .max(0.0)
        })
        .collect::<Vec<_>>();
    let valid = vec![true; green.len()];
    let mut scalar_metric = vec![0.0f32; green.len()];
    let mut scalar_smooth = vec![0.0f32; green.len()];
    let mut apple_metric = vec![0.0f32; green.len()];
    let mut apple_smooth = vec![0.0f32; green.len()];

    let run_scalar = |metric: &mut [f32], smooth: &mut [f32]| {
        let started = Instant::now();
        compute_focus_metric_scalar(
            black_box(&green),
            black_box(&valid),
            metric,
            width,
            height,
        );
        for y in 0..height {
            for x in 0..width {
                smooth[y * width + x] =
                    sorted_trimmed_focus_mean_at(black_box(metric), width, height, x, y);
            }
        }
        black_box(smooth[width * height / 2]);
        started.elapsed()
    };
    let run_apple = |metric: &mut [f32], smooth: &mut [f32]| {
        let started = Instant::now();
        // SAFETY: this executable is compiled only for Apple AArch64 and all
        // buffers have identical validated dimensions.
        unsafe {
            compute_focus_metric_apple_neon(
                black_box(&green),
                black_box(&valid),
                metric,
                width,
                height,
            );
        }
        compute_trimmed_focus_mean(black_box(metric), smooth, width, height);
        black_box(smooth[width * height / 2]);
        started.elapsed()
    };

    run_scalar(&mut scalar_metric, &mut scalar_smooth);
    run_apple(&mut apple_metric, &mut apple_smooth);
    let mut scalar_times = Vec::with_capacity(9);
    let mut apple_times = Vec::with_capacity(9);
    for iteration in 0..9 {
        if iteration & 1 == 0 {
            scalar_times.push(run_scalar(&mut scalar_metric, &mut scalar_smooth));
            apple_times.push(run_apple(&mut apple_metric, &mut apple_smooth));
        } else {
            apple_times.push(run_apple(&mut apple_metric, &mut apple_smooth));
            scalar_times.push(run_scalar(&mut scalar_metric, &mut scalar_smooth));
        }
    }
    scalar_times.sort_unstable();
    apple_times.sort_unstable();
    let percentile = |values: &[Duration], fraction: f32| {
        values[((values.len() - 1) as f32 * fraction).round() as usize]
    };
    let scalar_p50 = percentile(&scalar_times, 0.50);
    let scalar_p95 = percentile(&scalar_times, 0.95);
    let apple_p50 = percentile(&apple_times, 0.50);
    let apple_p95 = percentile(&apple_times, 0.95);
    let speedup_p50 = scalar_p50.as_secs_f64() / apple_p50.as_secs_f64();
    let speedup_p95 = scalar_p95.as_secs_f64() / apple_p95.as_secs_f64();
    let maximum_error = scalar_smooth
        .iter()
        .zip(&apple_smooth)
        .map(|(left, right)| (left - right).abs())
        .fold(0.0f32, f32::max);
    println!(
        "{{\"schema\":\"trueshot.apple-focus-qualification.v1\",\
         \"width\":{width},\"height\":{height},\"iterations\":9,\
         \"scalar_p50_ms\":{:.6},\"scalar_p95_ms\":{:.6},\
         \"apple_p50_ms\":{:.6},\"apple_p95_ms\":{:.6},\
         \"speedup_p50\":{speedup_p50:.6},\"speedup_p95\":{speedup_p95:.6},\
         \"maximum_absolute_error\":{maximum_error:.9}}}",
        scalar_p50.as_secs_f64() * 1_000.0,
        scalar_p95.as_secs_f64() * 1_000.0,
        apple_p50.as_secs_f64() * 1_000.0,
        apple_p95.as_secs_f64() * 1_000.0,
    );
    assert!(
        maximum_error <= 2e-5,
        "Apple focus evidence exceeded parity tolerance"
    );
    assert!(
        speedup_p50 >= 1.5,
        "Apple focus evidence p50 speedup regressed to {speedup_p50:.2}x"
    );
    assert!(
        speedup_p95 >= 1.3,
        "Apple focus evidence p95 speedup regressed to {speedup_p95:.2}x"
    );
}

#[cfg(not(all(target_arch = "aarch64", target_os = "macos")))]
fn main() {
    eprintln!("Apple focus qualification requires aarch64 macOS");
    std::process::exit(2);
}
