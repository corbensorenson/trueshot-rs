// Suppress warnings for intentionally unused items (future expansion points)
#![allow(dead_code)]
#![allow(unused_variables)]

pub mod hierarchical_collapse;
pub mod hierarchical_pipeline;
pub mod hierarchical_grading;
pub mod preprocessing;
pub mod postprocess;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod joint_demosaic;
pub mod demosaic_ahd;
pub mod align_raw;
pub mod focus_grouping;
pub mod sff;
pub mod quality_analyzer;
pub mod resource_manager;
pub mod exif_parser;
pub mod logging;
pub mod progress;
pub mod error;
pub mod types;
pub mod timing;
pub mod object_detection;
pub mod optimized_ops;
pub mod fusion_engine;
pub mod scanning;
pub mod smart_loader;
pub mod bayer_cache;
pub mod brdf;
pub mod grade_stats;
pub mod color_chart;
pub mod color_grade;
pub mod cloud_client;
pub mod config;
pub mod events;
pub mod plugins;
pub mod project;
pub mod director;
pub mod crash_handler;
pub mod inventory;
pub mod scheduler;
pub mod export;
pub mod validation;
pub mod intrinsics;
pub mod metrics;

// Re-export specialized crates
pub use trueshot_calibration as calibration;
#[cfg(feature = "vision")]
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
            pub fn new() -> Self { Self }
            pub fn reset(&mut self) {}
            pub fn update<T>(&mut self, _frame: &T) -> bool { true }
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
    pub mod voxel;
    pub mod marching_cubes;
    pub mod optimization;
    pub mod texture;
    pub mod texture_opt;
    pub mod lod;
    pub mod editing;
    pub mod io;
    
    // Re-exports
    pub use voxel::{
        VoxelGrid, VoxelData,
        TsdfVoxel, CoverageVoxel, ConfidenceVoxel, DensityVoxel,
        DensityGrid, TsdfGrid, CoverageGrid, ConfidenceGrid, ColorDensityGrid,
    };
    
    pub use marching_cubes::{
        MarchingCubes, MarchingCubesConfig,
        MeshVertex, MeshTriangle, ExtractedMesh,
        DensitySource,
        GPU_TRI_TABLE, GPU_EDGE_TABLE,
    };
    
    pub use lod::{LOD_HIGH, LOD_MED, LOD_LOW, generate_lods};
    pub use editing::{MeshEditOp, apply_mesh_edits};
    pub use io::{load_mesh, ensure_vertex_normals};
}

// Unified Tracking Module
pub mod tracking;


pub mod compute {
    pub mod gpu;
    pub mod context;
    pub mod async_utils;
    pub mod wavelet;    
    pub mod cluster;
}
pub mod ai {
    pub mod model_cache;
    pub mod model_manifest;
    pub mod material;
    pub mod naming;
    pub mod segmentation;
    pub mod splatting;
}
pub mod io {
    pub mod direct;
    pub mod octree;
    pub mod sidecar;
    pub mod ingest;
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
pub use types::*;
pub use error::{Error, Result};
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

// RAW processing engine (Photo Editor backend)
pub mod raw_processing;

// Live Hybrid Mesh/4DGS streaming system
pub mod live_hybrid;

// Unified streaming protocol for all modes
pub mod streaming;
