//! Benchmarks for TrueShot fusion algorithms

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use image::{ImageBuffer, Rgb};
use ndarray::Array3;
use std::path::PathBuf;
use trueshot_core::{
    config::ProcessingConfig,
    fusion::{QifFusion, FusionParameters},
    loader::{LoadedImage, BoundingBox},
};

fn create_test_image(width: u32, height: u32, pattern: &str) -> LoadedImage {
    let mut data = Vec::with_capacity((width * height * 3) as usize);
    
    for y in 0..height {
        for x in 0..width {
            let (r, g, b) = match pattern {
                "gradient" => {
                    let intensity = (x as f32 / width as f32 * 65535.0) as u16;
                    (intensity, intensity, intensity)
                }
                "checkerboard" => {
                    let checker = ((x / 8) + (y / 8)) % 2;
                    if checker == 0 { (0, 0, 0) } else { (65535, 65535, 65535) }
                }
                "noise" => {
                    let noise = ((x * 17 + y * 23) % 65536) as u16;
                    (noise, noise, noise)
                }
                _ => (32768, 32768, 32768), // Mid-gray
            };
            
            data.push(r);
            data.push(g);
            data.push(b);
        }
    }
    
    let image_buffer = ImageBuffer::from_raw(width, height, data).unwrap();
    
    LoadedImage {
        data: image_buffer,
        bbox: BoundingBox::new(0, 0, width, height),
        original_size: (width, height),
        path: PathBuf::from("benchmark.nef"),
        warp_applied: false,
    }
}

fn create_test_images(count: usize, width: u32, height: u32) -> Vec<LoadedImage> {
    let patterns = ["gradient", "checkerboard", "noise", "uniform"];
    
    (0..count)
        .map(|i| {
            let pattern = patterns[i % patterns.len()];
            create_test_image(width, height, pattern)
        })
        .collect()
}

fn bench_qif_fusion_small(c: &mut Criterion) {
    let mut group = c.benchmark_group("qif_fusion_small");
    
    let config = ProcessingConfig::default();
    let fusion_engine = QifFusion::new(config);
    
    for image_count in [2, 4, 8].iter() {
        let images = create_test_images(*image_count, 64, 64);
        
        group.bench_with_input(
            BenchmarkId::new("images", image_count),
            &images,
            |b, images| {
                b.iter(|| {
                    let result = fusion_engine.fuse_images(
                        black_box(images.clone()),
                        black_box(None),
                    );
                    black_box(result)
                });
            },
        );
    }
    
    group.finish();
}

fn bench_qif_fusion_medium(c: &mut Criterion) {
    let mut group = c.benchmark_group("qif_fusion_medium");
    
    let config = ProcessingConfig::default();
    let fusion_engine = QifFusion::new(config);
    
    for image_count in [2, 4].iter() {
        let images = create_test_images(*image_count, 256, 256);
        
        group.bench_with_input(
            BenchmarkId::new("images", image_count),
            &images,
            |b, images| {
                b.iter(|| {
                    let result = fusion_engine.fuse_images(
                        black_box(images.clone()),
                        black_box(None),
                    );
                    black_box(result)
                });
            },
        );
    }
    
    group.finish();
}

fn bench_qif_fusion_large(c: &mut Criterion) {
    let mut group = c.benchmark_group("qif_fusion_large");
    group.sample_size(10); // Reduce sample size for large images
    
    let config = ProcessingConfig::default();
    let fusion_engine = QifFusion::new(config);
    
    let images = create_test_images(2, 1024, 1024);
    
    group.bench_function("2_images_1024x1024", |b| {
        b.iter(|| {
            let result = fusion_engine.fuse_images(
                black_box(images.clone()),
                black_box(None),
            );
            black_box(result)
        });
    });
    
    group.finish();
}

fn bench_fusion_parameters(c: &mut Criterion) {
    let mut group = c.benchmark_group("fusion_parameters");
    
    let config = ProcessingConfig::default();
    let images = create_test_images(4, 128, 128);
    
    let focus_methods = [
        ("laplacian", trueshot_core::config::FocusFusionMethod::Laplacian),
        ("gradient", trueshot_core::config::FocusFusionMethod::Gradient),
        ("variance", trueshot_core::config::FocusFusionMethod::Variance),
    ];
    
    for (name, method) in focus_methods.iter() {
        let mut test_config = config.clone();
        test_config.focus_fusion_method = method.clone();
        let fusion_engine = QifFusion::new(test_config);
        
        group.bench_function(name, |b| {
            b.iter(|| {
                let result = fusion_engine.fuse_images(
                    black_box(images.clone()),
                    black_box(None),
                );
                black_box(result)
            });
        });
    }
    
    group.finish();
}

fn bench_image_loading(c: &mut Criterion) {
    let mut group = c.benchmark_group("image_loading");
    
    for size in [64, 256, 512].iter() {
        group.bench_with_input(
            BenchmarkId::new("create_test_image", size),
            size,
            |b, &size| {
                b.iter(|| {
                    let image = create_test_image(
                        black_box(size),
                        black_box(size),
                        black_box("gradient"),
                    );
                    black_box(image)
                });
            },
        );
    }
    
    group.finish();
}

fn bench_weight_computation(c: &mut Criterion) {
    use trueshot_core::fusion::WeightNormalization;
    use ndarray::Array4;
    
    let mut group = c.benchmark_group("weight_computation");
    
    let sizes = [(64, 64), (128, 128), (256, 256)];
    
    for (width, height) in sizes.iter() {
        let weights = Array4::<f32>::from_elem((2, 2, *height, *width), 0.5);
        
        group.bench_with_input(
            BenchmarkId::new("normalize_weights", format!("{}x{}", width, height)),
            &weights,
            |b, weights| {
                b.iter(|| {
                    let mut test_weights = weights.clone();
                    // Simulate weight normalization
                    let sum: f32 = test_weights.sum();
                    if sum > 0.0 {
                        test_weights /= sum;
                    }
                    black_box(test_weights)
                });
            },
        );
    }
    
    group.finish();
}

fn bench_parallel_processing(c: &mut Criterion) {
    use rayon::prelude::*;
    
    let mut group = c.benchmark_group("parallel_processing");
    
    let data_sizes = [1000, 10000, 100000];
    
    for size in data_sizes.iter() {
        let data: Vec<f32> = (0..*size).map(|i| i as f32).collect();
        
        group.bench_with_input(
            BenchmarkId::new("sequential", size),
            &data,
            |b, data| {
                b.iter(|| {
                    let result: Vec<f32> = data
                        .iter()
                        .map(|&x| x * x + 1.0)
                        .collect();
                    black_box(result)
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("parallel", size),
            &data,
            |b, data| {
                b.iter(|| {
                    let result: Vec<f32> = data
                        .par_iter()
                        .map(|&x| x * x + 1.0)
                        .collect();
                    black_box(result)
                });
            },
        );
    }
    
    group.finish();
}

fn bench_memory_allocation(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_allocation");
    
    let sizes = [1024, 4096, 16384];
    
    for size in sizes.iter() {
        group.bench_with_input(
            BenchmarkId::new("vec_allocation", size),
            size,
            |b, &size| {
                b.iter(|| {
                    let data: Vec<f32> = vec![0.0; black_box(size * size * 3)];
                    black_box(data)
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("vec_with_capacity", size),
            size,
            |b, &size| {
                b.iter(|| {
                    let mut data = Vec::with_capacity(black_box(size * size * 3));
                    data.resize(size * size * 3, 0.0);
                    black_box(data)
                });
            },
        );
    }
    
    group.finish();
}

criterion_group!(
    benches,
    bench_qif_fusion_small,
    bench_qif_fusion_medium,
    bench_qif_fusion_large,
    bench_fusion_parameters,
    bench_image_loading,
    bench_weight_computation,
    bench_parallel_processing,
    bench_memory_allocation
);

criterion_main!(benches);
