//! Benchmarks for TrueShot alignment algorithms

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use image::{ImageBuffer, Rgb};
use nalgebra::Matrix3;
use trueshot_core::{
    config::{ProcessingConfig, AlignmentMethod, WarpInterpolation},
    warp::{WarpField, WarpFieldGenerator},
};

fn create_test_image_rgb16(width: u32, height: u32, pattern: &str) -> ImageBuffer<Rgb<u16>, Vec<u16>> {
    let mut data = Vec::with_capacity((width * height * 3) as usize);
    
    for y in 0..height {
        for x in 0..width {
            let (r, g, b) = match pattern {
                "gradient_x" => {
                    let intensity = (x as f32 / width as f32 * 65535.0) as u16;
                    (intensity, intensity, intensity)
                }
                "gradient_y" => {
                    let intensity = (y as f32 / height as f32 * 65535.0) as u16;
                    (intensity, intensity, intensity)
                }
                "checkerboard" => {
                    let checker = ((x / 8) + (y / 8)) % 2;
                    if checker == 0 { (0, 0, 0) } else { (65535, 65535, 65535) }
                }
                "circles" => {
                    let cx = width as f32 / 2.0;
                    let cy = height as f32 / 2.0;
                    let dist = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                    let intensity = ((dist * 0.1).sin() * 32767.0 + 32767.0) as u16;
                    (intensity, intensity, intensity)
                }
                "noise" => {
                    let noise = ((x * 17 + y * 23 + x * y * 7) % 65536) as u16;
                    (noise, noise, noise)
                }
                _ => (32768, 32768, 32768), // Mid-gray
            };
            
            data.push(r);
            data.push(g);
            data.push(b);
        }
    }
    
    ImageBuffer::from_raw(width, height, data).unwrap()
}

fn create_shifted_image(
    base_image: &ImageBuffer<Rgb<u16>, Vec<u16>>,
    dx: i32,
    dy: i32,
) -> ImageBuffer<Rgb<u16>, Vec<u16>> {
    let (width, height) = base_image.dimensions();
    let mut shifted_data = vec![0u16; (width * height * 3) as usize];
    
    for y in 0..height {
        for x in 0..width {
            let src_x = (x as i32 - dx).max(0).min(width as i32 - 1) as u32;
            let src_y = (y as i32 - dy).max(0).min(height as i32 - 1) as u32;
            
            let src_pixel = base_image.get_pixel(src_x, src_y);
            let dst_idx = ((y * width + x) * 3) as usize;
            
            shifted_data[dst_idx] = src_pixel[0];
            shifted_data[dst_idx + 1] = src_pixel[1];
            shifted_data[dst_idx + 2] = src_pixel[2];
        }
    }
    
    ImageBuffer::from_raw(width, height, shifted_data).unwrap()
}

fn bench_warp_field_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("warp_field_creation");
    
    let sizes = [(64, 64), (256, 256), (512, 512)];
    
    for (width, height) in sizes.iter() {
        group.bench_with_input(
            BenchmarkId::new("identity", format!("{}x{}", width, height)),
            &(*width, *height),
            |b, &(w, h)| {
                b.iter(|| {
                    let warp = WarpField::identity(black_box((w, h)));
                    black_box(warp)
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("from_matrix", format!("{}x{}", width, height)),
            &(*width, *height),
            |b, &(w, h)| {
                let matrix = Matrix3::new(
                    1.0, 0.0, 10.0,
                    0.0, 1.0, 20.0,
                    0.0, 0.0, 1.0,
                );
                
                b.iter(|| {
                    let warp = WarpField::from_matrix(
                        black_box(matrix),
                        black_box((w, h)),
                        black_box((w, h)),
                        black_box(WarpInterpolation::Bilinear),
                    );
                    black_box(warp)
                });
            },
        );
    }
    
    group.finish();
}

fn bench_point_transformation(c: &mut Criterion) {
    let mut group = c.benchmark_group("point_transformation");
    
    let warp = WarpField::identity((1024, 1024));
    let translation_matrix = Matrix3::new(
        1.0, 0.0, 10.0,
        0.0, 1.0, 20.0,
        0.0, 0.0, 1.0,
    );
    let translation_warp = WarpField::from_matrix(
        translation_matrix,
        (1024, 1024),
        (1024, 1024),
        WarpInterpolation::Bilinear,
    );
    
    group.bench_function("identity_transform", |b| {
        b.iter(|| {
            let point = warp.transform_point(black_box(512.0), black_box(512.0));
            black_box(point)
        });
    });
    
    group.bench_function("translation_transform", |b| {
        b.iter(|| {
            let point = translation_warp.transform_point(black_box(512.0), black_box(512.0));
            black_box(point)
        });
    });
    
    let point_count = 10000;
    let points: Vec<(f32, f32)> = (0..point_count)
        .map(|i| (i as f32 % 1024.0, (i / 1024) as f32 % 1024.0))
        .collect();
    
    group.bench_function("batch_transform_identity", |b| {
        b.iter(|| {
            let results: Vec<_> = points
                .iter()
                .map(|&(x, y)| warp.transform_point(black_box(x), black_box(y)))
                .collect();
            black_box(results)
        });
    });
    
    group.bench_function("batch_transform_translation", |b| {
        b.iter(|| {
            let results: Vec<_> = points
                .iter()
                .map(|&(x, y)| translation_warp.transform_point(black_box(x), black_box(y)))
                .collect();
            black_box(results)
        });
    });
    
    group.finish();
}

fn bench_phase_correlation(c: &mut Criterion) {
    let mut group = c.benchmark_group("phase_correlation");
    
    let sizes = [(64, 64), (128, 128), (256, 256)];
    let shifts = [(0, 0), (5, 3), (10, 15)];
    
    for (width, height) in sizes.iter() {
        for (dx, dy) in shifts.iter() {
            let base_image = create_test_image_rgb16(*width, *height, "checkerboard");
            let shifted_image = create_shifted_image(&base_image, *dx, *dy);
            
            let generator = WarpFieldGenerator::new(
                AlignmentMethod::PhaseCorrelation,
                50,
                0.1,
            );
            
            group.bench_with_input(
                BenchmarkId::new(
                    "compute_warp",
                    format!("{}x{}_shift_{}_{}", width, height, dx, dy),
                ),
                &(&base_image, &shifted_image, &generator),
                |b, (base, shifted, gen)| {
                    b.iter(|| {
                        let warp = gen.compute_warp_field(
                            black_box(base),
                            black_box(shifted),
                            black_box(WarpInterpolation::Bilinear),
                        );
                        black_box(warp)
                    });
                },
            );
        }
    }
    
    group.finish();
}

fn bench_cross_correlation(c: &mut Criterion) {
    let mut group = c.benchmark_group("cross_correlation");
    
    let template_sizes = [16, 32, 64];
    let search_ranges = [10, 20, 50];
    
    for template_size in template_sizes.iter() {
        for search_range in search_ranges.iter() {
            let image_size = template_size + search_range * 2;
            let base_image = create_test_image_rgb16(image_size, image_size, "circles");
            let shifted_image = create_shifted_image(&base_image, 5, 7);
            
            group.bench_with_input(
                BenchmarkId::new(
                    "template_matching",
                    format!("template_{}_search_{}", template_size, search_range),
                ),
                &(&base_image, &shifted_image, *template_size, *search_range),
                |b, (base, shifted, template_sz, search_rng)| {
                    b.iter(|| {
                        // Simulate template matching computation
                        let mut best_score = 0.0f32;
                        let mut best_offset = (0, 0);
                        
                        for dy in -*search_rng..*search_rng {
                            for dx in -*search_rng..*search_rng {
                                let mut score = 0.0f32;
                                let mut count = 0;
                                
                                for ty in 0..*template_sz {
                                    for tx in 0..*template_sz {
                                        let base_x = (*template_sz + tx) as u32;
                                        let base_y = (*template_sz + ty) as u32;
                                        let shifted_x = (base_x as i32 + dx) as u32;
                                        let shifted_y = (base_y as i32 + dy) as u32;
                                        
                                        if shifted_x < base.width() && shifted_y < base.height() {
                                            let base_pixel = base.get_pixel(base_x, base_y);
                                            let shifted_pixel = shifted.get_pixel(shifted_x, shifted_y);
                                            
                                            let base_gray = (base_pixel[0] as f32 + base_pixel[1] as f32 + base_pixel[2] as f32) / 3.0;
                                            let shifted_gray = (shifted_pixel[0] as f32 + shifted_pixel[1] as f32 + shifted_pixel[2] as f32) / 3.0;
                                            
                                            score += base_gray * shifted_gray;
                                            count += 1;
                                        }
                                    }
                                }
                                
                                if count > 0 {
                                    score /= count as f32;
                                    if score > best_score {
                                        best_score = score;
                                        best_offset = (dx, dy);
                                    }
                                }
                            }
                        }
                        
                        black_box((best_score, best_offset))
                    });
                },
            );
        }
    }
    
    group.finish();
}

fn bench_warp_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("warp_serialization");
    
    let warp = WarpField::identity((1024, 1024));
    
    group.bench_function("serialize", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&warp));
            black_box(json)
        });
    });
    
    let json = serde_json::to_string(&warp).unwrap();
    
    group.bench_function("deserialize", |b| {
        b.iter(|| {
            let warp: Result<WarpField, _> = serde_json::from_str(black_box(&json));
            black_box(warp)
        });
    });
    
    group.finish();
}

fn bench_displacement_calculation(c: &mut Criterion) {
    let mut group = c.benchmark_group("displacement_calculation");
    
    let matrices = [
        ("identity", Matrix3::identity()),
        ("translation", Matrix3::new(1.0, 0.0, 10.0, 0.0, 1.0, 20.0, 0.0, 0.0, 1.0)),
        ("rotation", Matrix3::new(0.866, -0.5, 0.0, 0.5, 0.866, 0.0, 0.0, 0.0, 1.0)),
        ("scale", Matrix3::new(1.1, 0.0, 0.0, 0.0, 1.1, 0.0, 0.0, 0.0, 1.0)),
    ];
    
    for (name, matrix) in matrices.iter() {
        let warp = WarpField::from_matrix(
            *matrix,
            (1024, 1024),
            (1024, 1024),
            WarpInterpolation::Bilinear,
        );
        
        group.bench_function(*name, |b| {
            b.iter(|| {
                let displacement = warp.get_max_displacement();
                black_box(displacement)
            });
        });
    }
    
    group.finish();
}

criterion_group!(
    benches,
    bench_warp_field_creation,
    bench_point_transformation,
    bench_phase_correlation,
    bench_cross_correlation,
    bench_warp_serialization,
    bench_displacement_calculation
);

criterion_main!(benches);
