//! Benchmark for unified mesh and tracking components
//!
//! Tests performance of:
//! - Marching cubes mesh extraction
//! - VoxelGrid operations
//! - Object tracking and segmentation

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use nalgebra as na;
use trueshot_core::mesh::{DensityVoxel, MarchingCubes, MarchingCubesConfig, VoxelGrid};
use trueshot_core::tracking::{
    BoundingBox3D, MotionAnalyzer, MotionConfig, ObjectSegmenter, SegmentationConfig,
};
use uuid::Uuid;

/// Benchmark marching cubes on different grid sizes
fn bench_marching_cubes(c: &mut Criterion) {
    let mut group = c.benchmark_group("marching_cubes");

    for size in [16_usize, 32, 64].iter() {
        group.bench_with_input(BenchmarkId::new("grid_size", size), size, |b, &size| {
            // Create a grid with a sphere in the center
            let min = na::Point3::new(-1.0, -1.0, -1.0);
            let voxel_size = 2.0 / (size.saturating_sub(1).max(1) as f32);
            let mut grid =
                VoxelGrid::<DensityVoxel>::with_dims(min, [size, size, size], voxel_size);

            // Fill with sphere SDF
            let center = na::Point3::origin();
            let radius = 0.7;
            for z in 0..size {
                for y in 0..size {
                    for x in 0..size {
                        let voxel = [x, y, z];
                        let pos = grid.voxel_to_world(voxel);
                        let dist = na::distance(&pos, &center) - radius;
                        grid.set(
                            voxel,
                            DensityVoxel {
                                density: -dist,
                                color: [1.0, 1.0, 1.0],
                                weight: 1.0,
                            },
                        );
                    }
                }
            }

            let mc = MarchingCubes::new(MarchingCubesConfig {
                threshold: 0.0,
                compute_uvs: false,
                ..MarchingCubesConfig::default()
            });

            b.iter(|| black_box(mc.extract(&grid)));
        });
    }

    group.finish();
}

/// Benchmark VoxelGrid operations
fn bench_voxel_grid(c: &mut Criterion) {
    let mut group = c.benchmark_group("voxel_grid");

    // Benchmark grid creation
    group.bench_function("create_64x64x64", |b| {
        let min = na::Point3::new(0.0, 0.0, 0.0);

        b.iter(|| black_box(VoxelGrid::<f32>::with_dims(min, [64, 64, 64], 1.0 / 64.0)));
    });

    // Benchmark random access
    group.bench_function("random_access_1000", |b| {
        let min = na::Point3::new(0.0, 0.0, 0.0);
        let grid = VoxelGrid::<f32>::with_dims(min, [64, 64, 64], 1.0 / 64.0);

        // Pre-generate random indices
        let indices: Vec<(usize, usize, usize)> = (0..1000)
            .map(|i| (i % 64, (i * 7) % 64, (i * 13) % 64))
            .collect();

        b.iter(|| {
            for &(x, y, z) in &indices {
                black_box(grid.get([x, y, z]));
            }
        });
    });

    group.finish();
}

/// Benchmark object segmentation
fn bench_segmentation(c: &mut Criterion) {
    let mut group = c.benchmark_group("segmentation");

    for num_points in [100, 500, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::new("dbscan_points", num_points),
            num_points,
            |b, &num_points| {
                // Create clustered point cloud (3 clusters)
                let mut points = Vec::with_capacity(num_points);
                let centers = [
                    na::Point3::new(0.0, 0.0, 0.0),
                    na::Point3::new(5.0, 0.0, 0.0),
                    na::Point3::new(0.0, 5.0, 0.0),
                ];

                for i in 0..num_points {
                    let center = centers[i % 3];
                    let offset = na::Vector3::new(
                        (i as f32 * 0.123).sin() * 0.5,
                        (i as f32 * 0.456).cos() * 0.5,
                        (i as f32 * 0.789).sin() * 0.5,
                    );
                    points.push(center + offset);
                }

                let segmenter = ObjectSegmenter::new(SegmentationConfig {
                    min_cluster_size: 10,
                    eps_distance: 1.0,
                    ..Default::default()
                });

                b.iter(|| black_box(segmenter.segment(&points)));
            },
        );
    }

    group.finish();
}

/// Benchmark motion analysis
fn bench_motion_analysis(c: &mut Criterion) {
    let mut group = c.benchmark_group("motion_analysis");

    group.bench_function("update_100_objects", |b| {
        let mut analyzer = MotionAnalyzer::new(MotionConfig::default());

        // Pre-generate object data
        let objects: Vec<(Uuid, na::Point3<f32>, BoundingBox3D)> = (0..100)
            .map(|i| {
                let pos = na::Point3::new(i as f32, 0.0, 0.0);
                let bounds = BoundingBox3D::new(
                    pos - na::Vector3::new(0.5, 0.5, 0.5),
                    pos + na::Vector3::new(0.5, 0.5, 0.5),
                );
                (Uuid::new_v4(), pos, bounds)
            })
            .collect();

        b.iter(|| {
            for (id, pos, bounds) in &objects {
                black_box(analyzer.update(*id, *pos, bounds.clone()));
            }
            analyzer.advance_frame();
        });
    });

    group.bench_function("predict_position", |b| {
        let mut analyzer = MotionAnalyzer::new(MotionConfig::default());
        let id = Uuid::new_v4();

        // Initialize with some history
        for i in 0..10 {
            let pos = na::Point3::new(i as f32, 0.0, 0.0);
            let bounds = BoundingBox3D::new(
                pos - na::Vector3::new(0.5, 0.5, 0.5),
                pos + na::Vector3::new(0.5, 0.5, 0.5),
            );
            analyzer.update(id, pos, bounds);
            analyzer.advance_frame();
        }

        b.iter(|| black_box(analyzer.predict(&id, 5.0)));
    });

    group.finish();
}

/// Benchmark BoundingBox3D operations
fn bench_bounding_box(c: &mut Criterion) {
    let mut group = c.benchmark_group("bounding_box");

    group.bench_function("iou_1000", |b| {
        let boxes: Vec<BoundingBox3D> = (0..100)
            .map(|i| {
                let offset = na::Vector3::new(
                    (i as f32 * 0.1).sin(),
                    (i as f32 * 0.2).cos(),
                    (i as f32 * 0.3).sin(),
                );
                BoundingBox3D::new(
                    na::Point3::origin() + offset,
                    na::Point3::new(1.0, 1.0, 1.0) + offset,
                )
            })
            .collect();

        b.iter(|| {
            let mut total = 0.0f32;
            for i in 0..100 {
                for j in (i + 1)..100 {
                    total += boxes[i].iou(&boxes[j]);
                }
            }
            black_box(total)
        });
    });

    group.bench_function("from_points_1000", |b| {
        let points: Vec<na::Point3<f32>> = (0..1000)
            .map(|i| {
                na::Point3::new(
                    (i as f32 * 0.1).sin(),
                    (i as f32 * 0.2).cos(),
                    (i as f32 * 0.3).sin(),
                )
            })
            .collect();

        b.iter(|| black_box(BoundingBox3D::from_points(points.iter().cloned())));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_marching_cubes,
    bench_voxel_grid,
    bench_segmentation,
    bench_motion_analysis,
    bench_bounding_box,
);
criterion_main!(benches);
