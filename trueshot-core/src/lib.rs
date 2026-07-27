// Suppress warnings for intentionally unused items (future expansion points)
#![allow(dead_code)]
#![allow(unused_variables)]

pub mod align_raw;
pub mod bayer_cache;
pub mod brdf;
pub mod cloud_client;
pub mod color_chart;
pub mod color_grade;
pub mod config;
pub mod crash_handler;
pub mod demosaic_ahd;
pub mod director;
pub mod error;
pub mod events;
pub mod exif_parser;
pub mod export;
mod focus_evidence;
pub mod focus_grouping;
pub mod fusion_edit;
pub mod fusion_engine;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod grade_stats;
pub mod hierarchical_collapse;
pub mod hierarchical_grading;
pub mod hierarchical_pipeline;
pub mod intrinsics;
pub mod inventory;
pub mod joint_demosaic;
pub mod lens_psf;
pub mod lens_psf_extract;
pub mod logging;
pub mod metrics;
pub mod native_fusion;
pub mod object_detection;
pub mod optimized_ops;
pub mod performance_telemetry;
pub mod plugins;
pub mod postprocess;
pub mod preprocessing;
pub mod progress;
pub mod project;
pub mod quality_analyzer;
pub mod resource_manager;
pub mod scanning;
pub mod scheduler;
pub mod sensor_calibration;
pub mod sensor_correction;
pub mod sensor_noise;
pub mod sff;
pub mod smart_loader;
pub mod timing;
pub mod types;
pub mod validation;

// Re-export specialized crates
pub use trueshot_calibration as calibration;
#[cfg(feature = "vision")]
pub use trueshot_vision as vision;

#[cfg(not(feature = "vision"))]
pub mod vision {
    pub mod change_detection {
        pub struct SceneChangeDetector;
        impl Default for SceneChangeDetector {
            fn default() -> Self {
                Self::new()
            }
        }

        impl SceneChangeDetector {
            pub fn new() -> Self {
                Self
            }
            pub fn reset(&mut self) {}
            pub fn update<T>(&mut self, _frame: &T) -> bool {
                true
            }
        }
    }
}
pub use trueshot_storage as storage;

// New Photogrammetry Modules
pub mod photogrammetry {
    pub mod guidance;
    pub mod heatmap;
}

// New Mesh Modules
pub mod mesh {
    pub mod editing;
    pub mod io;
    pub mod lod;
    pub mod marching_cubes;
    pub mod optimization;
    pub mod texture;
    pub mod texture_opt;
    pub mod voxel;

    // Re-exports
    pub use voxel::{
        ColorDensityGrid, ConfidenceGrid, ConfidenceVoxel, CoverageGrid, CoverageVoxel,
        DensityGrid, DensityVoxel, TsdfGrid, TsdfVoxel, VoxelData, VoxelGrid,
    };

    pub use marching_cubes::{
        DensitySource, ExtractedMesh, MarchingCubes, MarchingCubesConfig, MeshTriangle, MeshVertex,
        GPU_EDGE_TABLE, GPU_TRI_TABLE,
    };

    pub use editing::{apply_mesh_edits, MeshEditOp};
    pub use io::{ensure_vertex_normals, load_mesh};
    pub use lod::{generate_lods, LOD_HIGH, LOD_LOW, LOD_MED};
}

// Unified Tracking Module
pub mod tracking;

pub mod compute {
    pub mod async_utils;
    pub mod cluster;
    pub mod context;
    pub mod gpu;
    pub mod wavelet;
}
pub mod ai {
    pub mod material;
    pub mod model_cache;
    pub mod model_manifest;
    pub mod naming;
    pub mod segmentation;
    pub mod splatting;
}
pub mod io {
    pub mod direct;
    pub mod ingest;
    pub mod octree;
    pub mod sidecar;
}

// Security: provenance, token storage
pub mod security;

pub mod planning {
    pub mod feedback;
}
pub mod system {
    pub mod plugins;
}
// pub mod camera; // Removed: Use trueshot-device-manager

// Re-export key types
pub use error::{Error, Result};
pub use types::*;
pub mod nef;
pub mod raw_io;
pub mod reconstruction;

// Native 3D Gaussian Splatting (no external Python/CUDA required)
pub mod gaussian_splatting;

// Native Structure from Motion (requires trueshot-vision)
#[cfg(feature = "vision")]
pub mod sfm;

// Licensing system for commercial deployment
pub mod licensing;

// Avatar capture and creation system
pub mod avatar;

// Scene reconstruction from crowd-sourced footage
pub mod scene_reconstruction;

// Advanced capture modes (HDR, focus stacking)
pub mod capture;
pub mod capture_manifest;
pub mod processing_journal;

// RAW processing engine (Photo Editor backend)
pub mod raw_processing;

// Live Hybrid Mesh/4DGS streaming system
pub mod live_hybrid;

// Unified streaming protocol for all modes
pub mod streaming;
