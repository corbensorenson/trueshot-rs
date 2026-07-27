//! TrueShot CLI - State-of-the-Art Command Line Interface
//!
//! A complete CLI for all TrueShot operations with rich output,
//! progress bars, and colored output.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use config::{Config, File};
use console::{style, Emoji};
use image::{imageops::FilterType, DynamicImage, GrayImage, RgbImage};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use nalgebra as na;
use reqwest::blocking::Client as HttpClient;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use toml::Value as TomlValue;
use tray_icon::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    TrayIconBuilder,
};
use trueshot_core::crash_handler::init_crash_handler;
use trueshot_core::demosaic_ahd::ahd_demosaic_f32_owned;
use trueshot_core::export::usd::{export_usd_with_options, UsdExportOptions};
use trueshot_core::export::{
    export_fbx, export_glb, export_gltf, export_ply, export_point_cloud_ply,
    save_bytes_with_digest, save_depth_tiff_with_digest, save_metric_depth_pfm_with_digest,
    save_png_preview_with_digest, save_tiff16_from_f32_with_digest, save_u16_map_png_with_digest,
    save_u8_map_png_with_digest, PlyExportOptions,
};
use trueshot_core::fusion_edit::FusionEditDocument;
use trueshot_core::fusion_replay::{
    FusionReplayArtifact, FusionReplayCapsule, FusionRevisionEnvelope, FUSION_REPLAY_SCHEMA,
    MAX_FUSION_REVISION_ENVELOPE_BYTES,
};
use trueshot_core::gaussian_splatting::{Camera as GsCamera, GaussianSplatTrainer, TrainingConfig};
use trueshot_core::gpu::{get_gpu_context, GpuAhdEngine};
use trueshot_core::intrinsics::{
    estimate_intrinsics_with_report, IntrinsicsReport, IntrinsicsSource,
};
use trueshot_core::inventory::{Device, Inventory, Machine, Model, Sequence, SequenceStatus};
use trueshot_core::lens_psf::{
    calibrate_lens_psf, LensPsfCalibrationConfig, LensPsfMeasurementSet, LensPsfProfile,
};
use trueshot_core::lens_psf_extract::{extract_lens_psf_measurements, LensPsfExtractionPlan};
use trueshot_core::licensing::{Feature, LicenseError, LicenseManager};
use trueshot_core::native_fusion::{
    fuse_native_group_with_id, fusion_provenance_preview_with_frequency, NativeFusionConfig,
    NativeFusionResult, FREQUENCY_FLAG_DETAIL_REFERENCE, FREQUENCY_FLAG_DETAIL_SINGLE_SOURCE,
    FREQUENCY_FLAG_MEASUREMENT_CLAMPED, FREQUENCY_FLAG_SEPARATED, FREQUENCY_FLAG_SPLIT_SOURCES,
    FUSION_EDIT_MEASURED_SOURCE, FUSION_FLAG_BRACKET_ALIGNED, FUSION_FLAG_CENSORED,
    FUSION_FLAG_CENSOR_CONFLICT, FUSION_FLAG_DISOCCLUDED, FUSION_FLAG_OUTLIER_REJECTED,
    FUSION_FLAG_SOURCE_FALLBACK, FUSION_FLAG_UNCALIBRATED_NOISE, FUSION_FLAG_VISIBILITY_CORRECTED,
    SENSOR_CORRECTION_DEFECT_REPAIRED, SENSOR_CORRECTION_FLAT_FIELD,
};
use trueshot_core::performance_telemetry::{ProcessTelemetrySnapshot, ProcessTelemetryWindow};
use trueshot_core::postprocess::postprocess_f32;
use trueshot_core::processing_journal::{
    artifact_digest_from_parts, ArtifactDigest, ArtifactVerification, ClaimDecision,
    GroupProcessingStatus, ProcessingJournal,
};
use trueshot_core::reconstruction::multicam_sfm::{
    patchmatch_stereo, CameraIntrinsics, CameraPose, DepthMap, FeatureType, MvsInput,
    PatchMatchConfig, ReprojectionStats, SfmConfig, SfmPipeline,
};
use trueshot_core::resource_manager::{
    AdaptiveDecodeController, CancellationToken, MemoryCreditPool, NativeSequenceMemoryEstimate,
    PipelinePressureSample, SystemResources,
};
use trueshot_core::sensor_calibration::{
    CalibrationSplit, IsoCalibrationReport, SensorCalibrationAccumulator, SensorCalibrationConfig,
    SENSOR_CALIBRATION_ISO_REPORT_SCHEMA,
};
use trueshot_core::sensor_correction::{
    CorrectionCalibrationSplit, SensorCorrectionAccumulator, SensorCorrectionCalibrationConfig,
    SensorCorrectionProfile,
};
use trueshot_core::sensor_noise::{SensorNoiseProfile, SENSOR_NOISE_PROFILE_SCHEMA};
use trueshot_core::smart_loader::{NativeGroupArena, SmartLoader};
use trueshot_core::timing::HierarchicalTimer;
use trueshot_core::types::ProcessingOptions;
use trueshot_core::validation::validate_photogrammetry_input;
use zeroize::Zeroizing;

const MAX_FUSION_PROCESSOR_INPUT_BYTES: usize = MAX_FUSION_REVISION_ENVELOPE_BYTES + 4096;
use uuid::Uuid;

const SENSOR_CALIBRATION_ARTIFACT_SCHEMA: &str = "trueshot.sensor-calibration.artifact.v2";

mod mesh_io;

// Emojis for rich output
static ROCKET: Emoji<'_, '_> = Emoji("🚀 ", "");
static CHECK: Emoji<'_, '_> = Emoji("✅ ", "[OK] ");
static CAMERA: Emoji<'_, '_> = Emoji("📷 ", "");
static CUBE: Emoji<'_, '_> = Emoji("🧊 ", "");
static FOLDER: Emoji<'_, '_> = Emoji("📁 ", "");
static GPU: Emoji<'_, '_> = Emoji("🖥️  ", "");
static WARNING: Emoji<'_, '_> = Emoji("⚠️  ", "[!] ");

const DEFAULT_CONFIG: &str = r#"[server]
host = "0.0.0.0"
port = 3000
admin_token_ttl_seconds = 3600
guest_token_ttl_seconds = 900
max_upload_bytes = 10737418240
max_project_bytes = 107374182400

[paths]
projects_dir = "./projects"
inventory_db = "./inventory.redb"

[hardware]
camera_indices = [0]
turntable_type = "auto"
serial_port = "/dev/tty.usbserial-10"
mock_devices = false
"#;

#[derive(Debug, Deserialize, Clone)]
struct CliConfig {
    server: CliServerConfig,
    paths: CliPathsConfig,
}

#[derive(Debug, Deserialize, Clone)]
struct CliServerConfig {
    host: String,
    port: u16,
}

#[derive(Debug, Deserialize, Clone)]
struct CliPathsConfig {
    projects_dir: PathBuf,
    inventory_db: PathBuf,
}

#[derive(Debug, Serialize)]
struct InventorySnapshot {
    models: Vec<Model>,
    sequences: Vec<Sequence>,
    machines: Vec<Machine>,
    devices: Vec<Device>,
}

#[derive(Debug, Clone)]
struct InventoryContext {
    model_id: uuid::Uuid,
    sequence_id: uuid::Uuid,
    model_name: String,
}

#[derive(Parser)]
#[command(name = "trueshot")]
#[command(author = "TrueShot Team")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "State-of-the-art 3D reconstruction and photogrammetry", long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Disable colored output
    #[arg(long, global = true)]
    no_color: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the TrueShot server
    Serve {
        /// Server port
        #[arg(short, long, default_value_t = 3000)]
        port: u16,

        /// Run with system tray icon
        #[arg(long)]
        tray: bool,

        /// Run in background (daemon mode)
        #[arg(short, long)]
        daemon: bool,
    },

    /// Process images through reconstruction pipeline
    Process {
        /// Input directory containing images
        #[arg(short, long)]
        input: PathBuf,

        /// Output directory for results
        #[arg(short, long)]
        output: PathBuf,

        /// Reconstruction mode
        #[arg(short, long, value_enum, default_value_t = Mode::Hybrid)]
        mode: Mode,

        /// Quality level
        #[arg(short, long, value_enum, default_value_t = Quality::Medium)]
        quality: Quality,

        /// Number of parallel jobs
        #[arg(short, long)]
        jobs: Option<usize>,

        /// Decode complete RAW frames instead of the exact shared object ROI
        #[arg(long)]
        full_frame: bool,

        /// Disable GPU acceleration
        #[arg(long)]
        no_gpu: bool,

        /// Export a full-resolution normalized depth TIFF in burst mode
        #[arg(long)]
        depth: bool,

        /// Export a full-resolution PNG instead of a bounded preview in burst mode
        #[arg(long)]
        full_resolution_preview: bool,

        /// Long-edge pixel limit for the default burst preview
        #[arg(long, default_value_t = 1600)]
        preview_max_dimension: usize,

        /// Robust HDR motion rejection (0 disables, 1 standard, 2 strongest)
        #[arg(long, default_value_t = 1.0)]
        deghost_strength: f32,

        /// Disable sparse same-CFA low/detail frequency deghosting
        #[arg(long)]
        no_frequency_deghost: bool,

        /// Calibrated green-channel glare spread at the sensor, in micrometers
        #[arg(long, default_value_t = 80.0)]
        glare_spread_um: f32,

        /// Disable glare exclusion from focus scoring (radiance is never altered)
        #[arg(long)]
        no_glare_focus: bool,

        /// Measured exact-ISO sensor noise profile JSON for native burst fusion
        #[arg(long)]
        sensor_noise_profile: Option<PathBuf>,

        /// Measured spatial gain/defect profile for native burst fusion
        #[arg(long)]
        sensor_correction_profile: Option<PathBuf>,

        /// Measured lens breathing/pupil/field-PSF profile for native burst fusion
        #[arg(long)]
        lens_psf_profile: Option<PathBuf>,

        /// Source-bound measured-frame edit document for one immutable burst revision
        #[arg(long)]
        fusion_edits: Option<PathBuf>,

        /// Read a server-validated edit and exact base report envelope from stdin
        #[arg(long)]
        fusion_revision_stdin: bool,

        /// Skip the second CFA-safe pass when depth regularization changes a focus plane
        #[arg(long)]
        no_depth_refusion: bool,

        /// Attempt to start a local trial if no license is present
        #[arg(long)]
        trial: bool,

        /// Trial duration in days (1-90)
        #[arg(long)]
        trial_days: Option<i64>,
    },

    /// Export model to different formats
    Export {
        /// Input model file
        #[arg(short, long)]
        input: PathBuf,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,

        /// Export format
        #[arg(short, long, value_enum)]
        format: ExportFormat,

        /// Include vertex colors
        #[arg(long, default_value_t = true)]
        colors: bool,

        /// Include normals
        #[arg(long, default_value_t = true)]
        normals: bool,

        /// Allow non-commercial export when commercial rights are not licensed
        #[arg(long)]
        noncommercial: bool,

        /// Attempt to start a local trial if no license is present
        #[arg(long)]
        trial: bool,

        /// Trial duration in days (1-90)
        #[arg(long)]
        trial_days: Option<i64>,
    },

    /// Pipeline automation jobs
    Jobs {
        #[command(subcommand)]
        action: JobsCommand,
    },

    /// Calibrate cameras
    Calibrate {
        /// Calibration images (checkerboard pattern)
        #[arg(short, long, num_args = 1..)]
        images: Vec<PathBuf>,

        /// Checkerboard columns
        #[arg(long, default_value_t = 9)]
        cols: u32,

        /// Checkerboard rows
        #[arg(long, default_value_t = 6)]
        rows: u32,

        /// Checkerboard square size in mm
        #[arg(long, default_value_t = 25.0)]
        square_size_mm: f32,

        /// Output calibration file
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Fit exact-ISO noise and optics-bound spatial correction from paired NEFs
    CalibrateNoise {
        /// Directory containing repeated lens-capped dark frames
        #[arg(long)]
        dark: PathBuf,

        /// Repeated flat-field level directory; provide at least five
        #[arg(long = "flat-level", required = true)]
        flat_levels: Vec<PathBuf>,

        /// Output sensor-noise JSON; spatial correction and report are written beside it
        #[arg(short, long)]
        output: PathBuf,

        /// Maximum deterministic samples retained per frame pair and CFA site
        #[arg(long, default_value_t = 32_768)]
        max_samples_per_pair_per_site: usize,

        /// Maximum held-out variance relative error
        #[arg(long, default_value_t = 0.10)]
        maximum_variance_error: f32,

        /// Absolute tolerance around nominal 90%/95% residual coverage
        #[arg(long, default_value_t = 0.03)]
        coverage_tolerance: f32,
    },

    /// Extract source-hashed lens PSF measurements from retained native Bayer targets
    ExtractLensPsf {
        /// Source-hashed target geometry and bounded edge-ROI plan
        #[arg(short, long)]
        plan: PathBuf,

        /// Root directory containing the plan's relative retained-capture paths
        #[arg(long)]
        capture_root: PathBuf,

        /// Output measurement-set JSON for calibrate-lens-psf
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Fit a digest-bound lens breathing, pupil, and field-PSF profile
    CalibrateLensPsf {
        /// Retained fit/holdout defocus measurement-set JSON
        #[arg(short, long)]
        measurements: PathBuf,

        /// Output lens PSF profile JSON
        #[arg(short, long)]
        output: PathBuf,

        /// Maximum corrected holdout p95 relative defocus error
        #[arg(long, default_value_t = 0.05)]
        maximum_p95_error: f32,

        /// Minimum p95 error reduction versus the ideal thin-lens model
        #[arg(long, default_value_t = 0.50)]
        minimum_error_reduction: f32,
    },

    /// Manage model inventory
    Inventory {
        #[command(subcommand)]
        action: InventoryAction,
    },

    /// Show system status and diagnostics
    Status {
        /// Show detailed hardware information
        #[arg(long)]
        hardware: bool,

        /// Check for updates
        #[arg(long)]
        check_updates: bool,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum JobsCommand {
    /// Submit a pipeline job to the server
    Submit(Box<JobsSubmitArgs>),

    /// List pipeline jobs
    List {
        /// Server base URL (defaults to config server.host/server.port)
        #[arg(long)]
        server: Option<String>,

        /// API token (defaults to TRUESHOT_API_TOKEN)
        #[arg(long)]
        api_token: Option<String>,
    },

    /// Get a job by id
    Get {
        /// Job id
        #[arg(long)]
        id: String,

        /// Server base URL (defaults to config server.host/server.port)
        #[arg(long)]
        server: Option<String>,

        /// API token (defaults to TRUESHOT_API_TOKEN)
        #[arg(long)]
        api_token: Option<String>,
    },
}

#[derive(clap::Args)]
struct JobsSubmitArgs {
    /// Job kind (e.g. unified_photogrammetry, unified_gaussian_splatting)
    #[arg(long)]
    kind: String,

    /// Job display name
    #[arg(long)]
    name: String,

    /// Request id (optional, enables idempotency)
    #[arg(long)]
    request_id: Option<String>,

    /// JSON payload string
    #[arg(long)]
    payload: Option<String>,

    /// JSON payload file path
    #[arg(long)]
    payload_file: Option<PathBuf>,

    /// Workspace path for unified jobs
    #[arg(long)]
    workspace: Option<PathBuf>,

    /// Livescan path for unified jobs
    #[arg(long)]
    livescan: Option<PathBuf>,

    /// DSLR path for unified jobs
    #[arg(long)]
    dslr: Option<PathBuf>,

    /// Job type override (gaussian_splatting or photogrammetry)
    #[arg(long)]
    job_type: Option<String>,

    /// Webhook URL for status callbacks
    #[arg(long)]
    webhook_url: Option<String>,

    /// Server base URL (defaults to config server.host/server.port)
    #[arg(long)]
    server: Option<String>,

    /// API token (defaults to TRUESHOT_API_TOKEN)
    #[arg(long)]
    api_token: Option<String>,
}

#[derive(Subcommand)]
enum InventoryAction {
    /// List all models
    List {
        /// Filter by tag
        #[arg(short, long)]
        tag: Option<String>,
    },
    /// Show model details
    Show {
        /// Model ID
        id: String,
    },
    /// Delete a model
    Delete {
        /// Model ID
        id: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },
    /// Export inventory to JSON
    Export {
        /// Output file
        #[arg(short, long)]
        output: PathBuf,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show current configuration
    Show,
    /// Set a configuration value
    Set {
        /// Configuration key
        key: String,
        /// Configuration value
        value: String,
    },
    /// Reset configuration to defaults
    Reset,
    /// Open configuration file in editor
    Edit,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// Classic photogrammetry (SfM + MVS)
    Photogrammetry,
    /// 3D Gaussian Splatting
    Gaussians,
    /// Hybrid mode (best of both)
    Hybrid,
    /// Burst collapse (focus stacking)
    Burst,
    /// Live scanning mode
    Live,
    /// Avatar creation
    Avatar,
    /// Quick scan (fast, lower quality)
    Quick,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mode::Photogrammetry => write!(f, "photogrammetry"),
            Mode::Gaussians => write!(f, "3dgs"),
            Mode::Hybrid => write!(f, "hybrid"),
            Mode::Burst => write!(f, "burst"),
            Mode::Live => write!(f, "live"),
            Mode::Avatar => write!(f, "avatar"),
            Mode::Quick => write!(f, "quick"),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Quality {
    /// Fast processing, lower quality
    Low,
    /// Balanced quality and speed
    Medium,
    /// High quality, slower processing
    High,
    /// Maximum quality, longest processing
    Ultra,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum ExportFormat {
    /// glTF 2.0 (recommended)
    Gltf,
    /// GLB (binary glTF)
    Glb,
    /// Universal Scene Description
    Usd,
    /// USDA (ASCII USD)
    Usda,
    /// USDZ (USD zip bundle)
    Usdz,
    /// PLY (Stanford polygon)
    Ply,
    /// Wavefront OBJ
    Obj,
    /// FBX (ASCII)
    Fbx,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let _crash_guard = init_crash_handler(env::var("TRUESHOT_SENTRY_DSN").ok());

    // Initialize logging
    if cli.verbose {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();
    } else {
        let level = if matches!(
            &cli.command,
            Commands::CalibrateNoise { .. }
                | Commands::ExtractLensPsf { .. }
                | Commands::CalibrateLensPsf { .. }
        ) {
            tracing::Level::WARN
        } else {
            tracing::Level::INFO
        };
        tracing_subscriber::fmt()
            .with_max_level(level)
            .with_target(false)
            .init();
    }

    match cli.command {
        Commands::Serve { port, tray, daemon } => cmd_serve(port, tray, daemon),
        Commands::Process {
            input,
            output,
            mode,
            quality,
            jobs,
            full_frame,
            no_gpu,
            depth,
            full_resolution_preview,
            preview_max_dimension,
            deghost_strength,
            no_frequency_deghost,
            glare_spread_um,
            no_glare_focus,
            sensor_noise_profile,
            sensor_correction_profile,
            lens_psf_profile,
            fusion_edits,
            fusion_revision_stdin,
            no_depth_refusion,
            trial,
            trial_days,
        } => cmd_process(
            input,
            output,
            mode,
            quality,
            jobs,
            full_frame,
            no_gpu,
            depth,
            full_resolution_preview,
            preview_max_dimension,
            deghost_strength,
            no_frequency_deghost,
            glare_spread_um,
            no_glare_focus,
            sensor_noise_profile,
            sensor_correction_profile,
            lens_psf_profile,
            fusion_edits,
            fusion_revision_stdin,
            no_depth_refusion,
            trial,
            trial_days,
        ),
        Commands::Export {
            input,
            output,
            format,
            colors,
            normals,
            noncommercial,
            trial,
            trial_days,
        } => cmd_export(
            input,
            output,
            format,
            colors,
            normals,
            noncommercial,
            trial,
            trial_days,
        ),
        Commands::Calibrate {
            images,
            cols,
            rows,
            square_size_mm,
            output,
        } => cmd_calibrate(images, cols, rows, square_size_mm, output),
        Commands::CalibrateNoise {
            dark,
            flat_levels,
            output,
            max_samples_per_pair_per_site,
            maximum_variance_error,
            coverage_tolerance,
        } => cmd_calibrate_noise(
            dark,
            flat_levels,
            output,
            max_samples_per_pair_per_site,
            maximum_variance_error,
            coverage_tolerance,
        ),
        Commands::ExtractLensPsf {
            plan,
            capture_root,
            output,
        } => cmd_extract_lens_psf(plan, capture_root, output),
        Commands::CalibrateLensPsf {
            measurements,
            output,
            maximum_p95_error,
            minimum_error_reduction,
        } => cmd_calibrate_lens_psf(
            measurements,
            output,
            maximum_p95_error,
            minimum_error_reduction,
        ),
        Commands::Inventory { action } => cmd_inventory(action),
        Commands::Status {
            hardware,
            check_updates,
        } => cmd_status(hardware, check_updates),
        Commands::Config { action } => cmd_config(action),
        Commands::Jobs { action } => cmd_jobs(action),
    }
}

fn cmd_serve(port: u16, tray: bool, daemon: bool) -> Result<()> {
    println!(
        "{} Starting TrueShot Server on port {}...",
        ROCKET,
        style(port).cyan()
    );

    if daemon {
        println!("  Running in daemon mode (background)");
    }

    // Start the actual server
    let mut child = Command::new("cargo")
        .args(["run", "-p", "trueshot-server", "--release"])
        .env("TRUESHOT_SERVER_PORT", port.to_string())
        .spawn()
        .context("Failed to start server")?;

    if tray {
        run_with_tray(port)?;
    } else if !daemon {
        child.wait()?;
    } else {
        println!("{} Server started with PID: {}", CHECK, child.id());
    }

    Ok(())
}

fn cmd_process(
    input: PathBuf,
    output: PathBuf,
    mode: Mode,
    quality: Quality,
    jobs: Option<usize>,
    full_frame: bool,
    no_gpu: bool,
    export_depth: bool,
    full_resolution_preview: bool,
    preview_max_dimension: usize,
    deghost_strength: f32,
    no_frequency_deghost: bool,
    glare_spread_um: f32,
    no_glare_focus: bool,
    sensor_noise_profile: Option<PathBuf>,
    sensor_correction_profile: Option<PathBuf>,
    lens_psf_profile: Option<PathBuf>,
    fusion_edits: Option<PathBuf>,
    fusion_revision_stdin: bool,
    no_depth_refusion: bool,
    trial: bool,
    trial_days: Option<i64>,
) -> Result<()> {
    if !(64..=16_384).contains(&preview_max_dimension) {
        anyhow::bail!("Preview max dimension must be between 64 and 16384 pixels");
    }
    if !deghost_strength.is_finite() || !(0.0..=2.0).contains(&deghost_strength) {
        anyhow::bail!("Deghost strength must be between 0 and 2");
    }
    if !glare_spread_um.is_finite() || !(1.0..=2_000.0).contains(&glare_spread_um) {
        anyhow::bail!("Glare spread must be between 1 and 2000 micrometers");
    }
    if (sensor_noise_profile.is_some()
        || sensor_correction_profile.is_some()
        || lens_psf_profile.is_some()
        || fusion_edits.is_some()
        || fusion_revision_stdin)
        && mode != Mode::Burst
    {
        anyhow::bail!("Sensor calibration profiles currently apply only to --mode burst");
    }
    if fusion_edits.is_some() && fusion_revision_stdin {
        anyhow::bail!("--fusion-edits and --fusion-revision-stdin are mutually exclusive");
    }
    let (fusion_revision, encrypted_raw_key) = if fusion_revision_stdin {
        let mut bytes = Zeroizing::new(Vec::new());
        io::stdin()
            .take((MAX_FUSION_PROCESSOR_INPUT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .context("Read bounded fusion revision envelope from stdin")?;
        if bytes.len() > MAX_FUSION_PROCESSOR_INPUT_BYTES {
            anyhow::bail!("Fusion processor input exceeds the bounded size limit");
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).context("Parse fusion processor input")?;
        if value.get("envelope").is_some() {
            let input: FusionProcessorInput =
                serde_json::from_value(value).context("Validate fusion processor input")?;
            input.envelope.validate()?;
            (
                Some(input.envelope),
                input.encrypted_raw_key.map(Zeroizing::new),
            )
        } else {
            (Some(FusionRevisionEnvelope::from_json_bytes(&bytes)?), None)
        }
    } else {
        (None, None)
    };
    let mut license_manager = init_license_manager()?;
    let required = process_required_features(mode);
    ensure_cli_license(&mut license_manager, &required, trial, trial_days)?;
    enforce_cli_scan_limit(&license_manager)?;
    enforce_cli_max_resolution(&license_manager, &input)?;

    let started_at = std::time::SystemTime::now();
    let started_iso = now_rfc3339(started_at);
    let start_instant = std::time::Instant::now();
    let telemetry_started = ProcessTelemetrySnapshot::capture();
    if no_gpu {
        env::set_var("TRUESHOT_DISABLE_GPU", "1");
    }
    let inventory_ctx = create_inventory_context(&input, &output, mode, quality)
        .context("Failed to create inventory entries")?;
    let mut run_state = RunStateManager::load_or_init(&input, &output, mode, quality)
        .context("Failed to initialize run state")?;
    update_inventory_sequence(&inventory_ctx, &output, SequenceStatus::Processing);
    println!("{} TrueShot Processing Pipeline", CUBE);
    println!();
    println!("  {} Input:   {}", FOLDER, style(input.display()).cyan());
    println!("  {} Output:  {}", FOLDER, style(output.display()).green());
    println!("  {} Mode:    {}", CAMERA, style(mode).yellow());
    println!("  {} Quality: {:?}", style("⚙").dim(), quality);

    if no_gpu {
        println!("  {} GPU:     {}", GPU, style("Disabled").red());
    } else {
        println!("  {} GPU:     {}", GPU, style("Auto-detect").green());
    }
    println!();

    let result = match mode {
        Mode::Burst => run_burst_pipeline(
            &input,
            &output,
            quality,
            jobs,
            full_frame,
            no_gpu,
            export_depth,
            full_resolution_preview,
            preview_max_dimension,
            deghost_strength,
            !no_frequency_deghost,
            glare_spread_um,
            !no_glare_focus,
            sensor_noise_profile.as_deref(),
            sensor_correction_profile.as_deref(),
            lens_psf_profile.as_deref(),
            fusion_edits.as_deref(),
            fusion_revision.as_ref(),
            !no_depth_refusion,
            encrypted_raw_key,
            Some(&inventory_ctx),
            Some(&mut run_state),
        ),
        Mode::Photogrammetry | Mode::Gaussians | Mode::Hybrid | Mode::Quick => {
            run_reconstruction_pipeline(
                &input,
                &output,
                mode,
                quality,
                Some(&inventory_ctx),
                Some(&mut run_state),
            )
        }
        Mode::Live => {
            anyhow::bail!("Live mode is only available via the server and live capture workflow")
        }
        Mode::Avatar => {
            anyhow::bail!("Avatar mode requires the full capture stack; use the server UI")
        }
    };

    if let Err(err) = result {
        eprintln!("{} Processing failed: {}", WARNING, err);
        update_inventory_sequence(&inventory_ctx, &output, SequenceStatus::Failed);
        run_state.mark_failed();
        if let Err(report_error) = write_run_report(
            &output.join("run_report.json"),
            RunReportKind::Process {
                mode,
                quality,
                input: input.clone(),
                output: output.clone(),
                jobs,
                gpu_disabled: no_gpu,
            },
            &started_iso,
            started_at,
            start_instant.elapsed().as_secs_f64(),
            "failed",
            Some(&inventory_ctx),
            &telemetry_started,
        ) {
            eprintln!("{} Failed to retain run report: {report_error:#}", WARNING);
        }
        return Err(err);
    }

    write_inventory_manifest(&output, &inventory_ctx, &input)?;
    update_inventory_sequence(&inventory_ctx, &output, SequenceStatus::Completed);
    run_state.mark_completed();
    write_run_report(
        &output.join("run_report.json"),
        RunReportKind::Process {
            mode,
            quality,
            input,
            output: output.clone(),
            jobs,
            gpu_disabled: no_gpu,
        },
        &started_iso,
        started_at,
        start_instant.elapsed().as_secs_f64(),
        "success",
        Some(&inventory_ctx),
        &telemetry_started,
    )?;

    println!();
    println!("{} Processing complete!", CHECK);
    println!("  Output saved to: {}", style(output.display()).green());

    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FusionProcessorInput {
    envelope: FusionRevisionEnvelope,
    encrypted_raw_key: Option<[u8; 32]>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct UsageLedger {
    counts: BTreeMap<String, u32>,
}

fn enforce_cli_scan_limit(manager: &LicenseManager) -> Result<()> {
    let Some(max) = manager.scans_per_month() else {
        return Ok(());
    };
    let key = manager
        .license_key_hash()
        .unwrap_or_else(|| manager.device_hash());
    let month_key = current_month_key();
    let entry_key = format!("{key}:{month_key}");
    let mut ledger = load_usage_ledger();
    let current = ledger.counts.get(&entry_key).copied().unwrap_or(0);
    if current >= max {
        anyhow::bail!(
            "Monthly scan limit exceeded (limit {max}). Upgrade your license to continue."
        );
    }
    ledger.counts.insert(entry_key, current + 1);
    save_usage_ledger(&ledger)?;
    Ok(())
}

fn enforce_cli_max_resolution(manager: &LicenseManager, input: &Path) -> Result<()> {
    let Some(max_resolution) = manager.max_resolution() else {
        return Ok(());
    };
    let image_dir = if input.join("images").is_dir() {
        input.join("images")
    } else {
        input.to_path_buf()
    };
    let image_paths = collect_sfm_images(&image_dir)?;
    for path in image_paths.iter().take(32) {
        if let Ok((width, height)) = image::image_dimensions(path) {
            let max_dim = width.max(height);
            if max_dim > max_resolution {
                anyhow::bail!(
                    "Input image {}x{} exceeds licensed max resolution ({}).",
                    width,
                    height,
                    max_resolution
                );
            }
        }
    }
    Ok(())
}

fn current_month_key() -> String {
    chrono::Utc::now().format("%Y-%m").to_string()
}

fn usage_ledger_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("TrueShot")
        .join("usage.json")
}

fn load_usage_ledger() -> UsageLedger {
    let path = usage_ledger_path();
    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Ok(parsed) = serde_json::from_str::<UsageLedger>(&raw) {
            return parsed;
        }
    }
    UsageLedger::default()
}

fn save_usage_ledger(ledger: &UsageLedger) -> Result<()> {
    let path = usage_ledger_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_string_pretty(ledger)?;
    std::fs::write(path, payload)?;
    Ok(())
}

fn cmd_export(
    input: PathBuf,
    output: PathBuf,
    format: ExportFormat,
    colors: bool,
    normals: bool,
    noncommercial: bool,
    trial: bool,
    trial_days: Option<i64>,
) -> Result<()> {
    let mut license_manager = init_license_manager()?;
    ensure_cli_license(&mut license_manager, &[], trial, trial_days)?;
    if !license_manager.is_feature_enabled(Feature::CommercialUse) {
        if !noncommercial {
            anyhow::bail!(
                "Commercial-use entitlement required for export. Re-run with --noncommercial to tag output as non-commercial."
            );
        }
        std::env::set_var("TRUESHOT_EXPORT_RIGHTS", "non-commercial (explicit opt-in)");
    }

    let started_at = std::time::SystemTime::now();
    let started_iso = now_rfc3339(started_at);
    let start_instant = std::time::Instant::now();
    let telemetry_started = ProcessTelemetrySnapshot::capture();
    println!("{} Exporting model...", CUBE);
    println!("  Input:  {}", style(input.display()).cyan());
    println!("  Output: {}", style(output.display()).green());
    println!("  Format: {:?}", format);

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );

    pb.set_message("Loading mesh...");
    let mut mesh = mesh_io::load_mesh(&input)?;

    if normals {
        mesh_io::ensure_vertex_normals(&mut mesh);
    } else {
        mesh.normals.clear();
    }

    if !colors {
        mesh.colors.clear();
    }

    pb.set_message("Exporting...");
    match format {
        ExportFormat::Gltf => export_gltf(&mesh, &output)?,
        ExportFormat::Glb => export_glb(&mesh, &output)?,
        ExportFormat::Usd | ExportFormat::Usda => {
            let options = UsdExportOptions {
                include_normals: normals && !mesh.normals.is_empty(),
                include_uvs: !mesh.uvs.is_empty(),
                include_colors: colors && !mesh.colors.is_empty(),
                ..Default::default()
            };
            export_usd_with_options(&mesh, &output, &options)?;
        }
        ExportFormat::Usdz => {
            let options = UsdExportOptions {
                include_normals: normals && !mesh.normals.is_empty(),
                include_uvs: !mesh.uvs.is_empty(),
                include_colors: colors && !mesh.colors.is_empty(),
                ..Default::default()
            };
            trueshot_core::export::usdz::export_usdz_with_options(&mesh, &output, &options)?;
        }
        ExportFormat::Ply => {
            let options = PlyExportOptions {
                include_normals: normals && !mesh.normals.is_empty(),
                include_colors: colors && !mesh.colors.is_empty(),
                include_uvs: !mesh.uvs.is_empty(),
                ..Default::default()
            };
            export_ply(&mesh, &output, &options)?;
        }
        ExportFormat::Fbx => {
            export_fbx(&mesh, &output)?;
        }
        ExportFormat::Obj => {
            mesh_io::export_obj(&mesh, &output, normals, colors)?;
        }
    }

    pb.finish_with_message(format!("{} Export complete!", CHECK));
    let report_path = output.with_extension("report.json");
    write_run_report(
        &report_path,
        RunReportKind::Export {
            input,
            output,
            format,
            include_colors: colors,
            include_normals: normals,
        },
        &started_iso,
        started_at,
        start_instant.elapsed().as_secs_f64(),
        "success",
        None,
        &telemetry_started,
    )?;

    Ok(())
}

fn init_license_manager() -> Result<LicenseManager> {
    let mut manager =
        LicenseManager::new().map_err(|err| anyhow::anyhow!(license_error_message(&err)))?;
    if let Ok(key) = env::var("TRUESHOT_LICENSE_KEY") {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            let device_name = env::var("TRUESHOT_LICENSE_DEVICE_NAME")
                .ok()
                .and_then(|value| {
                    let trimmed_name = value.trim().to_string();
                    if trimmed_name.is_empty() {
                        None
                    } else {
                        Some(trimmed_name)
                    }
                });
            manager
                .load_license_key(trimmed, device_name)
                .map_err(|err| anyhow::anyhow!(license_error_message(&err)))?;
        }
    }
    Ok(manager)
}

fn process_required_features(mode: Mode) -> Vec<Feature> {
    match mode {
        Mode::Avatar => vec![Feature::AvatarReconstruction],
        _ => Vec::new(),
    }
}

fn ensure_cli_license(
    manager: &mut LicenseManager,
    features: &[Feature],
    allow_trial: bool,
    trial_days: Option<i64>,
) -> Result<()> {
    if let Err(err) = manager.verify() {
        if allow_trial {
            let days = trial_days.unwrap_or(14).clamp(1, 90);
            manager
                .create_trial_with_features(days, features)
                .map_err(|trial_err| anyhow::anyhow!(license_error_message(&trial_err)))?;
        } else {
            return Err(anyhow::anyhow!(license_error_message(&err)));
        }
    }

    if let Err(err) = manager.verify() {
        return Err(anyhow::anyhow!(license_error_message(&err)));
    }

    sync_trial_env(manager);

    for feature in features {
        if let Err(err) = manager.require_feature(*feature) {
            return Err(anyhow::anyhow!(license_error_message(&err)));
        }
    }

    Ok(())
}

fn sync_trial_env(manager: &LicenseManager) {
    if let Some(trial) = manager.trial_info() {
        std::env::set_var("TRUESHOT_LICENSE_TRIAL", "1");
        if let Some(expires) = trial.expires_at {
            std::env::set_var("TRUESHOT_LICENSE_TRIAL_EXPIRES_AT", expires.to_rfc3339());
        } else {
            std::env::remove_var("TRUESHOT_LICENSE_TRIAL_EXPIRES_AT");
        }
        if let Some(days) = trial.days_remaining {
            std::env::set_var("TRUESHOT_LICENSE_TRIAL_DAYS_REMAINING", days.to_string());
        } else {
            std::env::remove_var("TRUESHOT_LICENSE_TRIAL_DAYS_REMAINING");
        }
    } else {
        std::env::set_var("TRUESHOT_LICENSE_TRIAL", "0");
        std::env::remove_var("TRUESHOT_LICENSE_TRIAL_EXPIRES_AT");
        std::env::remove_var("TRUESHOT_LICENSE_TRIAL_DAYS_REMAINING");
    }
}

fn license_error_message(err: &LicenseError) -> String {
    match err {
        LicenseError::NoLicense => {
            "No valid license found. Install a license or run with --trial (requires TRUESHOT_LICENSE_ENABLE_LOCAL_TRIAL_ISSUER=1).".to_string()
        }
        LicenseError::Expired => "License expired. Please renew to continue.".to_string(),
        LicenseError::DeviceNotActivated => {
            "License is not activated for this device. Activate on the license server or install a device-bound license.".to_string()
        }
        LicenseError::GracePeriodExpired => {
            "Offline grace period expired. Connect to the license server to refresh.".to_string()
        }
        LicenseError::MissingPublicKey => {
            "License public key is missing. Set TRUESHOT_LICENSE_PUBLIC_KEY_* or enable dev mode for local testing.".to_string()
        }
        LicenseError::FeatureNotAvailable(feature) => {
            format!("Required feature not licensed: {feature}")
        }
        LicenseError::IntegrityFailure(detail) => {
            format!("License integrity verification failed: {detail}")
        }
        LicenseError::ActivationFailed(reason) => {
            format!("License activation failed: {reason}")
        }
        LicenseError::InvalidKeyFormat => "License key format is invalid.".to_string(),
        LicenseError::InvalidSignature(detail) => {
            format!("License signature invalid: {detail}")
        }
        LicenseError::SignatureVerificationFailed => {
            "License signature verification failed.".to_string()
        }
        LicenseError::SerializationError(detail) => {
            format!("License parsing error: {detail}")
        }
        LicenseError::FileNotFound(path) => {
            format!("License file not found: {path}")
        }
        LicenseError::DeviceLimitReached(limit) => {
            format!("Device activation limit reached (max {limit}).")
        }
        LicenseError::NetworkError(detail) => {
            format!("License network error: {detail}")
        }
        LicenseError::IoError(err) => {
            format!("License IO error: {err}")
        }
    }
}

fn cmd_calibrate(
    images: Vec<PathBuf>,
    cols: u32,
    rows: u32,
    square_size_mm: f32,
    output: Option<PathBuf>,
) -> Result<()> {
    let started_at = std::time::SystemTime::now();
    let started_iso = now_rfc3339(started_at);
    let start_instant = std::time::Instant::now();
    let telemetry_started = ProcessTelemetrySnapshot::capture();
    println!("{} Camera Calibration", CAMERA);
    println!("  Checkerboard: {}x{}", cols, rows);
    println!("  Images: {}", images.len());
    println!("  Square size: {:.2} mm", square_size_mm);

    if images.is_empty() {
        println!("{} No calibration images provided!", WARNING);
        return Ok(());
    }

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );

    pb.set_message("Running calibration...");
    let intrinsics = trueshot_core::calibration::lens::calibrate_checkerboard(
        &images,
        rows as i32,
        cols as i32,
        square_size_mm,
    )?;

    let output_path = output.unwrap_or_else(|| PathBuf::from("calibration.json"));
    let json = serde_json::json!({
        "camera_matrix": intrinsics.camera_matrix,
        "dist_coeffs": intrinsics.dist_coeffs,
        "rms_error": intrinsics.rms_error,
        "width": intrinsics.width,
        "height": intrinsics.height,
        "rows": rows,
        "cols": cols,
        "square_size_mm": square_size_mm,
    });
    std::fs::write(&output_path, serde_json::to_string_pretty(&json)?)?;

    pb.finish_with_message("Complete!");
    println!();
    println!(
        "{} Calibration saved to: {}",
        CHECK,
        style(output_path.display()).green()
    );

    let report_path = output_path.with_extension("report.json");
    write_run_report(
        &report_path,
        RunReportKind::Calibrate {
            images,
            output: output_path.clone(),
            rows,
            cols,
            square_size_mm,
            rms_error: intrinsics.rms_error,
            width: intrinsics.width as u32,
            height: intrinsics.height as u32,
        },
        &started_iso,
        started_at,
        start_instant.elapsed().as_secs_f64(),
        "success",
        None,
        &telemetry_started,
    )?;

    Ok(())
}

#[derive(Debug)]
struct NoiseCalibrationFile {
    path: PathBuf,
    metadata: trueshot_core::nef::parser::Z9Metadata,
    sha256: Option<String>,
    role: String,
    level: Option<u32>,
}

#[derive(Debug, Serialize)]
struct NoiseCalibrationSourceRecord {
    path: String,
    sha256: String,
    role: String,
    level: Option<u32>,
    iso: u32,
    shutter_seconds: Option<f64>,
    aperture: Option<f32>,
    focal_length_mm: Option<f32>,
    focus_distance_m: Option<f32>,
    lens_model: Option<String>,
    timestamp: Option<String>,
}

#[derive(Debug, Serialize)]
struct SpatialCorrectionArtifactSummary {
    profile_path: String,
    profile_published: bool,
    profile_sha256: Option<String>,
    aperture: f32,
    focal_length_mm: f32,
    focus_distance_min_m: f32,
    focus_distance_max_m: f32,
    lens_model: String,
    config: SensorCorrectionCalibrationConfig,
    grid_width: u16,
    grid_height: u16,
    fit_flat_pairs: u32,
    holdout_flat_pairs: u32,
    raw_holdout_p95_relative_error: f32,
    corrected_holdout_p95_relative_error: f32,
    defect_pixels: usize,
}

#[derive(Debug, Serialize)]
struct NoiseCalibrationArtifactReport {
    schema: String,
    iso_report_schema: String,
    camera_make: String,
    camera_model: String,
    bits_per_sample: u16,
    width: u32,
    height: u32,
    pairing_policy: String,
    profile_path: String,
    profile_published: bool,
    profile_sha256: Option<String>,
    spatial_correction: Option<SpatialCorrectionArtifactSummary>,
    config: SensorCalibrationConfig,
    iso_reports: Vec<IsoCalibrationReport>,
    sources: Vec<NoiseCalibrationSourceRecord>,
    passed: bool,
    failures: Vec<String>,
}

fn cmd_calibrate_noise(
    dark: PathBuf,
    flat_levels: Vec<PathBuf>,
    output: PathBuf,
    max_samples_per_pair_per_site: usize,
    maximum_variance_error: f32,
    coverage_tolerance: f32,
) -> Result<()> {
    let report_path = calibration_report_path(&output);
    let correction_path = spatial_correction_profile_path(&output);
    if output.exists() || correction_path.exists() || report_path.exists() {
        anyhow::bail!(
            "Refusing to overwrite calibration artifacts; choose a new output path (noise: {}, correction: {}, report: {})",
            output.display(),
            correction_path.display(),
            report_path.display()
        );
    }
    let config = SensorCalibrationConfig {
        max_samples_per_pair_per_site,
        maximum_variance_relative_error: maximum_variance_error,
        coverage_absolute_tolerance: coverage_tolerance,
        ..SensorCalibrationConfig::default()
    };
    config.validate()?;
    if flat_levels.len() < config.minimum_flat_levels {
        anyhow::bail!(
            "Noise calibration requires at least {} flat-level directories",
            config.minimum_flat_levels
        );
    }

    println!("{} Paired RAW Sensor Calibration", CAMERA);
    println!("  Dark frames: {}", dark.display());
    println!("  Flat levels: {}", flat_levels.len());
    println!("  Noise output: {}", output.display());
    println!("  Spatial correction: {}", correction_path.display());

    let mut files = inspect_noise_calibration_directory(&dark, "dark", None)?;
    for (level, directory) in flat_levels.iter().enumerate() {
        files.extend(inspect_noise_calibration_directory(
            directory,
            "flat",
            Some(u32::try_from(level).context("Too many flat levels")?),
        )?);
    }
    let reference = files
        .first()
        .context("No NEF calibration files were discovered")?;
    validate_noise_calibration_identity(&files, &reference.metadata)?;

    let camera_make = reference.metadata.camera_make.clone();
    let camera_model = reference.metadata.camera_model.clone();
    let bits_per_sample = reference.metadata.bits_per_sample;
    let width = reference.metadata.width;
    let height = reference.metadata.height;
    let sensor_levels = reference
        .metadata
        .sensor_levels
        .context("Noise calibration requires verified black/white sensor levels")?;
    let flat_reference = files
        .iter()
        .find(|file| file.role == "flat")
        .context("Spatial correction requires flat-field evidence")?;
    let correction_aperture = flat_reference
        .metadata
        .aperture
        .context("Flat-field calibration requires aperture metadata")?;
    let correction_focal_length = flat_reference
        .metadata
        .focal_length
        .context("Flat-field calibration requires focal-length metadata")?;
    let correction_lens_model = flat_reference
        .metadata
        .lens_model
        .clone()
        .context("Flat-field calibration requires lens-model metadata")?;
    let mut correction_focus_min = f32::INFINITY;
    let mut correction_focus_max = 0.0f32;
    for file in files.iter().filter(|file| file.role == "flat") {
        let aperture = file
            .metadata
            .aperture
            .context("Flat-field calibration source is missing aperture")?;
        let focal_length = file
            .metadata
            .focal_length
            .context("Flat-field calibration source is missing focal length")?;
        let lens_model = file
            .metadata
            .lens_model
            .as_deref()
            .context("Flat-field calibration source is missing lens model")?;
        let focus_distance = file
            .metadata
            .focus_distance
            .context("Flat-field calibration source is missing focus distance")?;
        if !focus_distance.is_finite() || focus_distance <= 0.0 {
            anyhow::bail!(
                "Flat-field source {} has invalid focus distance",
                file.path.display()
            );
        }
        correction_focus_min = correction_focus_min.min(focus_distance);
        correction_focus_max = correction_focus_max.max(focus_distance);
        if (aperture - correction_aperture).abs() > correction_aperture.max(aperture) * 0.01
            || (focal_length - correction_focal_length).abs()
                > correction_focal_length.max(focal_length) * 0.005
            || normalize_calibration_identity(lens_model)
                != normalize_calibration_identity(&correction_lens_model)
        {
            anyhow::bail!(
                "Flat-field source {} mixes optical settings; spatial calibration requires one aperture/focal-length configuration",
                file.path.display()
            );
        }
    }
    let correction_config = SensorCorrectionCalibrationConfig::default();
    let mut correction_accumulator = SensorCorrectionAccumulator::new(
        camera_make.clone(),
        camera_model.clone(),
        bits_per_sample,
        width as usize,
        height as usize,
        sensor_levels.black,
        sensor_levels.white,
        correction_lens_model,
        correction_aperture,
        correction_focal_length,
        correction_focus_min,
        correction_focus_max,
        correction_config.clone(),
    )?;
    let expected_dark_frames = config
        .minimum_dark_pairs_per_split
        .checked_mul(4)
        .context("Dark calibration pair requirement overflow")?;
    let expected_flat_frames = config
        .minimum_flat_pairs_per_split
        .checked_mul(4)
        .context("Flat calibration pair requirement overflow")?;
    let mut dark_by_iso: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    let mut flat_by_level_iso: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
    let mut all_isos = std::collections::BTreeSet::new();
    for (index, file) in files.iter().enumerate() {
        let iso = file
            .metadata
            .iso
            .context("Noise calibration source is missing exact ISO metadata")?;
        all_isos.insert(iso);
        if file.role == "dark" {
            dark_by_iso.entry(iso).or_default().push(index);
        } else {
            flat_by_level_iso
                .entry((file.level.context("Flat source has no level")?, iso))
                .or_default()
                .push(index);
        }
    }
    for bucket in dark_by_iso.values_mut() {
        sort_calibration_indices(bucket, &files);
    }
    for bucket in flat_by_level_iso.values_mut() {
        sort_calibration_indices(bucket, &files);
    }

    let mut preflight_failures = Vec::new();
    for iso in &all_isos {
        let dark_count = dark_by_iso.get(iso).map_or(0, Vec::len);
        if dark_count < expected_dark_frames || dark_count % 2 != 0 {
            preflight_failures.push(format!(
                "ISO {iso} dark frames {dark_count}; require an even count of at least {expected_dark_frames}"
            ));
        }
        for level in 0..flat_levels.len() {
            let level = u32::try_from(level).context("Too many flat levels")?;
            let count = flat_by_level_iso.get(&(level, *iso)).map_or(0, Vec::len);
            if count < expected_flat_frames || count % 2 != 0 {
                preflight_failures.push(format!(
                    "ISO {iso} flat level {level} frames {count}; require an even count of at least {expected_flat_frames}"
                ));
            }
        }
    }
    if !preflight_failures.is_empty() {
        anyhow::bail!(
            "Noise calibration preflight failed:\n- {}",
            preflight_failures.join("\n- ")
        );
    }
    for file in &mut files {
        file.sha256 = Some(sha256_file(&file.path)?);
    }
    reject_duplicate_calibration_sources(&files)?;
    let decode_count = files.len();
    let progress = ProgressBar::new(u64::try_from(decode_count).unwrap_or(u64::MAX));
    progress.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
        )
        .unwrap()
        .progress_chars("#>-"),
    );

    let mut iso_models = Vec::new();
    let mut iso_reports = Vec::new();
    let mut failures = Vec::new();
    for iso in all_isos {
        progress.set_message(format!("ISO {iso}: dark pairs"));
        let mut accumulator = SensorCalibrationAccumulator::new(
            iso,
            width as usize,
            height as usize,
            sensor_levels.black,
            sensor_levels.white,
            config.clone(),
        )?;
        add_noise_calibration_pairs(
            &mut accumulator,
            dark_by_iso
                .get(&iso)
                .context("Preflight lost dark ISO bucket")?,
            &files,
            None,
            &progress,
            None,
        )?;
        for level in 0..flat_levels.len() {
            let level = u32::try_from(level).context("Too many flat levels")?;
            progress.set_message(format!("ISO {iso}: flat level {level}"));
            add_noise_calibration_pairs(
                &mut accumulator,
                flat_by_level_iso
                    .get(&(level, iso))
                    .context("Preflight lost flat ISO bucket")?,
                &files,
                Some(level),
                &progress,
                Some(&mut correction_accumulator),
            )?;
        }
        let outcome = accumulator.evaluate()?;
        if let Some(model) = outcome.model {
            iso_models.push(model);
        } else {
            failures.push(format!(
                "ISO {} failed: {}",
                iso,
                outcome.report.failures.join("; ")
            ));
        }
        iso_reports.push(outcome.report);
    }
    progress.finish_with_message("Calibration evidence evaluated");
    let correction_outcome = correction_accumulator.evaluate()?;
    let mut correction_profile = correction_outcome.profile;
    if correction_profile.is_none() {
        failures.push(format!(
            "spatial correction failed: {}",
            correction_outcome.failures.join("; ")
        ));
    }

    let profile_path = output.clone();
    let sources = files
        .iter()
        .map(|file| {
            Ok(NoiseCalibrationSourceRecord {
                path: file.path.display().to_string(),
                sha256: file
                    .sha256
                    .clone()
                    .context("Calibration source was not hashed")?,
                role: file.role.clone(),
                level: file.level,
                iso: file.metadata.iso.unwrap_or_default(),
                shutter_seconds: file.metadata.exposure_time,
                aperture: file.metadata.aperture,
                focal_length_mm: file.metadata.focal_length,
                focus_distance_m: file.metadata.focus_distance,
                lens_model: file.metadata.lens_model.clone(),
                timestamp: file.metadata.timestamp.map(|value| value.to_rfc3339()),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let passed = failures.is_empty()
        && iso_models.len() == iso_reports.len()
        && correction_profile.is_some();
    let spatial_correction =
        correction_profile
            .as_ref()
            .map(|profile| SpatialCorrectionArtifactSummary {
                profile_path: correction_path.display().to_string(),
                profile_published: false,
                profile_sha256: None,
                aperture: profile.aperture,
                focal_length_mm: profile.focal_length_mm,
                focus_distance_min_m: profile.focus_distance_min_m,
                focus_distance_max_m: profile.focus_distance_max_m,
                lens_model: profile.lens_model.clone(),
                config: correction_config.clone(),
                grid_width: profile.grid_width,
                grid_height: profile.grid_height,
                fit_flat_pairs: profile.fit_flat_pairs,
                holdout_flat_pairs: profile.holdout_flat_pairs,
                raw_holdout_p95_relative_error: profile.raw_holdout_p95_relative_error,
                corrected_holdout_p95_relative_error: profile.corrected_holdout_p95_relative_error,
                defect_pixels: profile.defects.len(),
            });
    let mut report = NoiseCalibrationArtifactReport {
        schema: SENSOR_CALIBRATION_ARTIFACT_SCHEMA.to_string(),
        iso_report_schema: SENSOR_CALIBRATION_ISO_REPORT_SCHEMA.to_string(),
        camera_make,
        camera_model,
        bits_per_sample,
        width,
        height,
        pairing_policy:
            "sorted captures paired consecutively; even pair indices fit, odd pair indices holdout"
                .to_string(),
        profile_path: profile_path.display().to_string(),
        profile_published: false,
        profile_sha256: None,
        spatial_correction,
        config,
        iso_reports,
        sources,
        passed,
        failures,
    };
    write_atomic_json(&report_path, &report)?;
    println!("  Calibration report: {}", report_path.display());
    if !passed {
        anyhow::bail!(
            "Sensor calibration gates failed; profile was not published. Inspect {}",
            report_path.display()
        );
    }
    let profile = SensorNoiseProfile {
        schema: SENSOR_NOISE_PROFILE_SCHEMA.to_string(),
        camera_make: report.camera_make.clone(),
        camera_model: report.camera_model.clone(),
        bits_per_sample: report.bits_per_sample,
        calibration_id: "unpublished:paired-photon-transfer".to_string(),
        iso_models,
    };
    profile.save_json(&profile_path)?;
    let spatial_profile = correction_profile
        .take()
        .context("Passed calibration lost its spatial correction profile")?;
    if let Err(error) = spatial_profile.save_json(&correction_path) {
        let _ = std::fs::remove_file(&profile_path);
        return Err(error).context("Publish spatial sensor correction profile");
    }
    // Reload to prove the exact published artifact satisfies runtime gates.
    let published = SensorNoiseProfile::load_json(&profile_path)
        .context("Published sensor-noise profile failed runtime validation")?;
    let published_correction = match SensorCorrectionProfile::load_json(&correction_path) {
        Ok(profile) => profile,
        Err(error) => {
            let _ = std::fs::remove_file(&profile_path);
            let _ = std::fs::remove_file(&correction_path);
            return Err(error).context("Published spatial correction failed runtime validation");
        }
    };
    report.profile_published = true;
    report.profile_sha256 = published
        .calibration_id
        .strip_prefix("sha256:")
        .map(str::to_string);
    if let Some(summary) = &mut report.spatial_correction {
        summary.profile_published = true;
        summary.profile_sha256 = published_correction
            .calibration_id
            .strip_prefix("sha256:")
            .map(str::to_string);
    }
    write_atomic_json(&report_path, &report)?;
    println!(
        "{} Calibrated profiles published: {}, {}",
        CHECK,
        style(profile_path.display()).green(),
        style(correction_path.display()).green()
    );
    Ok(())
}

fn cmd_calibrate_lens_psf(
    measurements: PathBuf,
    output: PathBuf,
    maximum_p95_error: f32,
    minimum_error_reduction: f32,
) -> Result<()> {
    if output.exists() {
        anyhow::bail!(
            "Refusing to overwrite lens PSF profile {}",
            output.display()
        );
    }
    let report_path = output.with_extension("report.json");
    if report_path.exists() {
        anyhow::bail!(
            "Refusing to overwrite lens PSF report {}",
            report_path.display()
        );
    }
    let set = LensPsfMeasurementSet::load_json(&measurements)
        .with_context(|| format!("Load lens PSF measurements {}", measurements.display()))?;
    let config = LensPsfCalibrationConfig {
        maximum_calibrated_p95_relative_error: maximum_p95_error,
        minimum_p95_error_reduction: minimum_error_reduction,
        ..Default::default()
    };
    let (profile, report) = calibrate_lens_psf(&set, &config)?;
    let measurement_sha256 = sha256_file(&measurements)?;
    if !report.passed {
        write_atomic_json(
            &report_path,
            &serde_json::json!({
                "schema": "trueshot.lens-psf-calibration-report.v1",
                "measurement_path": measurements.display().to_string(),
                "measurement_sha256": measurement_sha256,
                "profile_path": output.display().to_string(),
                "profile_published": false,
                "config": config,
                "evaluation": report
            }),
        )?;
        anyhow::bail!(
            "Lens PSF holdout gates failed; profile was not published. Inspect {}",
            report_path.display()
        );
    }
    let profile = profile.context("Passing lens PSF calibration produced no profile")?;
    profile.save_json(&output)?;
    let published = LensPsfProfile::load_json(&output)
        .context("Published lens PSF profile failed runtime validation")?;
    write_atomic_json(
        &report_path,
        &serde_json::json!({
            "schema": "trueshot.lens-psf-calibration-report.v1",
            "measurement_path": measurements.display().to_string(),
            "measurement_sha256": measurement_sha256,
            "profile_path": output.display().to_string(),
            "profile_published": true,
            "profile_sha256": published.calibration_id.strip_prefix("sha256:"),
            "config": config,
            "evaluation": report
        }),
    )?;
    println!(
        "{} Lens PSF profile published: {} ({})",
        CHECK,
        style(output.display()).green(),
        published.calibration_id
    );
    println!("  Calibration report: {}", report_path.display());
    Ok(())
}

fn cmd_extract_lens_psf(plan: PathBuf, capture_root: PathBuf, output: PathBuf) -> Result<()> {
    if output.exists() {
        anyhow::bail!(
            "Refusing to overwrite lens PSF measurements {}",
            output.display()
        );
    }
    let report_path = lens_psf_extraction_report_path(&output);
    if report_path.exists() {
        anyhow::bail!(
            "Refusing to overwrite lens PSF extraction report {}",
            report_path.display()
        );
    }
    let plan_sha256 = sha256_file(&plan)?;
    let extraction_plan = LensPsfExtractionPlan::load_json(&plan)
        .with_context(|| format!("Load lens PSF extraction plan {}", plan.display()))?;
    let (measurements, extraction) =
        match extract_lens_psf_measurements(&extraction_plan, &capture_root) {
            Ok(result) => result,
            Err(error) => {
                write_atomic_json(
                    &report_path,
                    &serde_json::json!({
                        "schema": "trueshot.lens-psf-extraction-run.v1",
                        "plan_path": plan.display().to_string(),
                        "plan_sha256": plan_sha256,
                        "capture_root": capture_root.display().to_string(),
                        "measurement_path": output.display().to_string(),
                        "measurement_published": false,
                        "extraction_error": format!("{error:#}")
                    }),
                )?;
                return Err(error).with_context(|| {
                    format!(
                        "Lens PSF extraction failed; diagnostic report written to {}",
                        report_path.display()
                    )
                });
            }
        };
    let passed = extraction.passed;
    let measurement_count = extraction.measurements;
    let decode_fraction = extraction.decode_fraction;
    let report = serde_json::json!({
        "schema": "trueshot.lens-psf-extraction-run.v1",
        "plan_path": plan.display().to_string(),
        "plan_sha256": plan_sha256,
        "capture_root": capture_root.display().to_string(),
        "measurement_path": output.display().to_string(),
        "measurement_published": passed,
        "extraction": extraction
    });
    if !passed {
        write_atomic_json(&report_path, &report)?;
        anyhow::bail!(
            "Lens PSF extraction quality gates failed; measurements were not published. Inspect {}",
            report_path.display()
        );
    }
    let measurements =
        measurements.context("Passing lens PSF extraction produced no measurement set")?;
    measurements.save_json(&output)?;
    let published = LensPsfMeasurementSet::load_json(&output)
        .context("Published lens PSF measurements failed runtime validation")?;
    write_atomic_json(&report_path, &report)?;
    println!(
        "{} Lens PSF measurements published: {} ({} measurements, {:.3}% decoded/full-frame pixels)",
        CHECK,
        style(output.display()).green(),
        measurement_count,
        decode_fraction * 100.0
    );
    println!(
        "  Retained sources: {}, extraction report: {}",
        published.sources.len(),
        report_path.display()
    );
    Ok(())
}

fn inspect_noise_calibration_directory(
    directory: &Path,
    role: &str,
    level: Option<u32>,
) -> Result<Vec<NoiseCalibrationFile>> {
    let paths = trueshot_core::exif_parser::scan_nef_files(directory)
        .with_context(|| format!("Scan calibration directory {}", directory.display()))?;
    if paths.is_empty() {
        anyhow::bail!(
            "No NEFs found in calibration directory {}",
            directory.display()
        );
    }
    paths
        .into_iter()
        .map(|path| {
            let mut parser = trueshot_core::nef::parser::Z9NefParser::new(&path);
            parser
                .parse()
                .with_context(|| format!("Parse calibration NEF {}", path.display()))?;
            let metadata = parser.get_metadata()?.clone();
            Ok(NoiseCalibrationFile {
                path,
                metadata,
                sha256: None,
                role: role.to_string(),
                level,
            })
        })
        .collect()
}

fn validate_noise_calibration_identity(
    files: &[NoiseCalibrationFile],
    reference: &trueshot_core::nef::parser::Z9Metadata,
) -> Result<()> {
    for file in files {
        let metadata = &file.metadata;
        if metadata.width != reference.width
            || metadata.height != reference.height
            || metadata.bits_per_sample != reference.bits_per_sample
            || metadata.camera_make != reference.camera_make
            || metadata.camera_model != reference.camera_model
            || metadata.sensor_levels != reference.sensor_levels
            || metadata.cfa_pattern != [0, 1, 1, 2]
        {
            anyhow::bail!(
                "Calibration source {} does not match the reference camera encoding",
                file.path.display()
            );
        }
        if metadata.iso.is_none() {
            anyhow::bail!("Calibration source {} has no ISO", file.path.display());
        }
    }
    Ok(())
}

fn sort_calibration_indices(indices: &mut [usize], files: &[NoiseCalibrationFile]) {
    indices.sort_by(|left, right| {
        files[*left]
            .metadata
            .timestamp
            .cmp(&files[*right].metadata.timestamp)
            .then_with(|| files[*left].path.cmp(&files[*right].path))
    });
}

fn reject_duplicate_calibration_sources(files: &[NoiseCalibrationFile]) -> Result<()> {
    let mut seen = BTreeMap::<&str, &Path>::new();
    for file in files {
        let digest = file
            .sha256
            .as_deref()
            .context("Calibration source was not hashed")?;
        if let Some(first) = seen.insert(digest, &file.path) {
            anyhow::bail!(
                "Calibration source content is duplicated: {} and {}",
                first.display(),
                file.path.display()
            );
        }
    }
    Ok(())
}

fn add_noise_calibration_pairs(
    accumulator: &mut SensorCalibrationAccumulator,
    indices: &[usize],
    files: &[NoiseCalibrationFile],
    level: Option<u32>,
    progress: &ProgressBar,
    mut correction_accumulator: Option<&mut SensorCorrectionAccumulator>,
) -> Result<()> {
    for (pair_index, pair) in indices.chunks_exact(2).enumerate() {
        let first = decode_full_calibration_nef(&files[pair[0]])?;
        progress.inc(1);
        let second = decode_full_calibration_nef(&files[pair[1]])?;
        progress.inc(1);
        let split = if pair_index & 1 == 0 {
            CalibrationSplit::Fit
        } else {
            CalibrationSplit::Holdout
        };
        if let Some(level) = level {
            accumulator.add_flat_pair(level, &first.raw.data, &second.raw.data, split)?;
            if let Some(correction) = correction_accumulator.as_deref_mut() {
                correction.add_flat_pair(
                    &first.raw.data,
                    &second.raw.data,
                    match split {
                        CalibrationSplit::Fit => CorrectionCalibrationSplit::Fit,
                        CalibrationSplit::Holdout => CorrectionCalibrationSplit::Holdout,
                    },
                )?;
            }
        } else {
            accumulator.add_dark_pair(&first.raw.data, &second.raw.data, split)?;
        }
    }
    Ok(())
}

fn decode_full_calibration_nef(
    file: &NoiseCalibrationFile,
) -> Result<trueshot_core::raw_io::NativeNefRoi> {
    let rect = trueshot_core::types::Rect::new(
        0.0,
        0.0,
        file.metadata.width as f64,
        file.metadata.height as f64,
    );
    trueshot_core::raw_io::load_nef_roi_native(&file.path, rect)
        .with_context(|| format!("Decode calibration NEF {}", file.path.display()))
}

fn calibration_report_path(profile_path: &Path) -> PathBuf {
    let stem = profile_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("sensor-noise");
    profile_path.with_file_name(format!("{stem}_calibration_report.json"))
}

fn lens_psf_extraction_report_path(measurement_path: &Path) -> PathBuf {
    let stem = measurement_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("lens-psf-measurements");
    measurement_path.with_file_name(format!("{stem}_extraction_report.json"))
}

fn spatial_correction_profile_path(noise_profile_path: &Path) -> PathBuf {
    let stem = noise_profile_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("sensor-noise");
    noise_profile_path.with_file_name(format!("{stem}_spatial_correction.json"))
}

fn normalize_calibration_identity(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn sha256_file(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::with_capacity(1024 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn write_atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let partial = path.with_extension(format!("partial-{}-{}", std::process::id(), Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)?;
    let result = (|| -> Result<()> {
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        std::fs::hard_link(&partial, path)
            .with_context(|| format!("Publish JSON without replacing {}", path.display()))?;
        std::fs::remove_file(&partial)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(partial);
    }
    result
}

fn run_burst_pipeline(
    input: &Path,
    output: &Path,
    quality: Quality,
    jobs: Option<usize>,
    full_frame: bool,
    no_gpu: bool,
    export_depth: bool,
    full_resolution_preview: bool,
    preview_max_dimension: usize,
    deghost_strength: f32,
    frequency_separated_deghosting: bool,
    glare_spread_um: f32,
    glare_aware_focus: bool,
    sensor_noise_profile_path: Option<&Path>,
    sensor_correction_profile_path: Option<&Path>,
    lens_psf_profile_path: Option<&Path>,
    fusion_edits_path: Option<&Path>,
    fusion_revision: Option<&FusionRevisionEnvelope>,
    depth_consistent_refusion: bool,
    encrypted_raw_key: Option<Zeroizing<[u8; 32]>>,
    _inventory_ctx: Option<&InventoryContext>,
    mut run_state: Option<&mut RunStateManager>,
) -> Result<()> {
    if let Some(state) = run_state.as_deref_mut() {
        state.mark_step_started("scan_input");
    }
    if !input.exists() {
        anyhow::bail!("Input path does not exist: {}", input.display());
    }
    std::fs::create_dir_all(output)?;

    let mut options = build_processing_options(quality, jobs, no_gpu, full_frame);
    let initial_decode_workers = jobs
        .unwrap_or_else(|| num_cpus::get_physical().clamp(1, 8))
        .max(1);
    options.max_parallel_sequences = Some(initial_decode_workers);
    let build_loader = |options: ProcessingOptions| {
        let loader = SmartLoader::new(options);
        match encrypted_raw_key.as_deref() {
            Some(key) => loader.with_encrypted_raw_key(*key),
            None => loader,
        }
    };
    let mut loader = build_loader(options.clone());
    let mut capture_groups = loader.open_capture_groups(input)?;
    let group_count = capture_groups.total_groups();
    println!(
        "  Processing {} capture groups via {}",
        style(group_count).cyan().bold(),
        if capture_groups.is_streaming_manifest() {
            "streaming manifest"
        } else {
            "legacy importer"
        }
    );
    if let Some(state) = run_state.as_deref_mut() {
        state.mark_step_completed("scan_input", vec![]);
        state.mark_step_started("sfm");
    }
    let sensor_noise_profile = sensor_noise_profile_path
        .map(|path| SensorNoiseProfile::load_json_with_key(path, encrypted_raw_key.as_deref()))
        .transpose()
        .context("Load sensor noise calibration profile")?;
    if let Some(profile) = &sensor_noise_profile {
        println!(
            "  Sensor noise: {} {} {}-bit, {} ISO models ({})",
            profile.camera_make,
            profile.camera_model,
            profile.bits_per_sample,
            profile.iso_models.len(),
            profile.calibration_id
        );
    }
    let sensor_correction_profile = sensor_correction_profile_path
        .map(|path| SensorCorrectionProfile::load_json_with_key(path, encrypted_raw_key.as_deref()))
        .transpose()
        .context("Load sensor spatial correction profile")?;
    if let Some(profile) = &sensor_correction_profile {
        println!(
            "  Sensor correction: {}x{} grid, {} defects, {:.3}-{:.3} m focus ({})",
            profile.grid_width,
            profile.grid_height,
            profile.defects.len(),
            profile.focus_distance_min_m,
            profile.focus_distance_max_m,
            profile.calibration_id
        );
    }
    let lens_psf_profile = lens_psf_profile_path
        .map(|path| LensPsfProfile::load_json_with_key(path, encrypted_raw_key.as_deref()))
        .transpose()
        .context("Load lens PSF calibration profile")?;
    if let Some(profile) = &lens_psf_profile {
        println!(
            "  Lens PSF: {} focus x {} radius knots, ideal/corrected p95 {:.4}/{:.4} ({})",
            profile.focus_knots.len(),
            profile.radius_knots.len(),
            profile.ideal_holdout_p95_relative_error,
            profile.calibrated_holdout_p95_relative_error,
            profile.calibration_id
        );
    }
    let fusion_edit = match fusion_revision {
        Some(envelope) => Some(envelope.edit.clone()),
        None => fusion_edits_path
            .map(FusionEditDocument::load_json)
            .transpose()
            .context("Load measured-source fusion edit document")?,
    };
    let fusion_edit_digest = fusion_edit
        .as_ref()
        .map(FusionEditDocument::digest)
        .transpose()?;
    if let (Some(edit), Some(digest)) = (&fusion_edit, &fusion_edit_digest) {
        println!(
            "  Fusion revision: {} operations / {} pixels, edit {}",
            edit.operations.len(),
            edit.edited_pixel_count(),
            &digest[..12]
        );
    }
    let fusion_config = NativeFusionConfig {
        deghost_strength,
        frequency_separated_deghosting,
        glare_spread_um,
        glare_aware_focus,
        depth_consistent_refusion,
        sensor_noise_profile,
        sensor_correction_profile,
        lens_psf_profile,
        fusion_edit: None,
        ..native_fusion_config(quality)
    };
    let gpu_ahd = if no_gpu {
        None
    } else {
        get_gpu_context().and_then(|context| match GpuAhdEngine::new(context) {
            Ok(engine) => engine,
            Err(error) => {
                tracing::warn!("Metal AHD initialization failed; using CPU: {error:#}");
                None
            }
        })
    };
    if gpu_ahd.is_some() {
        println!("  Demosaic: bounded CFA-exact, parity-gated Metal AHD");
    } else {
        println!("  Demosaic: deterministic CPU AHD");
    }
    let mut native_arena = NativeGroupArena::default();
    let resources = SystemResources::query();
    let memory_budget_bytes = configured_memory_budget(resources.available_memory)?;
    let memory_credits = MemoryCreditPool::new(memory_budget_bytes)?;
    let cancellation = CancellationToken::default();
    install_cancellation_listener(cancellation.clone());
    let journal = ProcessingJournal::open(
        &output
            .join(".trueshot")
            .join("burst_processing_journal.redb"),
    )?;
    let (export_sender, export_receiver, export_worker) = burst_export_worker()?;
    let mut export_pending = false;
    let retry_limit = env::var("TRUESHOT_GROUP_RETRY_LIMIT")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(3)
        .clamp(1, 100);
    let mut adaptive_workers = jobs.is_none().then(|| {
        AdaptiveDecodeController::new(
            initial_decode_workers,
            resources.physical_cores.max(initial_decode_workers).min(32),
        )
    });
    println!(
        "  Memory budget: {:.1} MiB",
        memory_budget_bytes as f64 / (1024.0 * 1024.0)
    );
    let mut failures = Vec::new();
    let mut fusion_edit_target_seen = false;
    let replay_capsule = build_fusion_replay_capsule(
        input,
        output,
        quality,
        jobs,
        full_frame,
        no_gpu,
        export_depth,
        full_resolution_preview,
        preview_max_dimension,
        deghost_strength,
        frequency_separated_deghosting,
        glare_spread_um,
        glare_aware_focus,
        depth_consistent_refusion,
        sensor_noise_profile_path,
        sensor_correction_profile_path,
        lens_psf_profile_path,
    )?;

    let mp = MultiProgress::new();
    let seq_pb = mp.add(ProgressBar::new(group_count));
    seq_pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ETA {eta_precise} {msg}",
            )
            .unwrap()
            .progress_chars("#>-"),
    );

    for capture_group in &mut capture_groups {
        if cancellation.is_cancelled() {
            break;
        }
        let capture_group = capture_group?;
        let sequence = &capture_group.sequence;
        let group_edit = fusion_edit
            .as_ref()
            .filter(|edit| edit.capture_group_id == capture_group.group_id);
        if fusion_edit.is_some() && group_edit.is_none() {
            seq_pb.set_message(format!("Skipped non-target {}", sequence.meta.bone_id));
            seq_pb.inc(1);
            continue;
        }
        if group_edit.is_some() {
            fusion_edit_target_seen = true;
        }
        let group_edit_digest = group_edit.map(FusionEditDocument::digest).transpose()?;
        let processing_group_id = group_edit_digest.as_ref().map_or_else(
            || capture_group.group_id.clone(),
            |digest| fusion_revision_group_id(&capture_group.group_id, digest),
        );
        if let Some(edit) = group_edit {
            let base_output =
                burst_group_output_path(output, sequence, &capture_group.group_id, None);
            let base_report = fusion_report_path_for_output(&base_output);
            let actual_report_sha256 = if let Some(envelope) = fusion_revision {
                hex::encode(Sha256::digest(envelope.base_report_json.as_bytes()))
            } else {
                sha256_file(&base_report).with_context(|| {
                    format!(
                        "Verify immutable base fusion report for edit {}",
                        edit.capture_group_id
                    )
                })?
            };
            if actual_report_sha256 != edit.base_report_sha256 {
                anyhow::bail!(
                    "Fusion edit base report digest mismatch: expected {}, observed {}",
                    edit.base_report_sha256,
                    actual_report_sha256
                );
            }
        }
        if let Some(entry) = journal.get(&processing_group_id)? {
            if entry.status == GroupProcessingStatus::Committed {
                if journal.verify_committed_with(
                    &processing_group_id,
                    output,
                    resume_verification_policy(&processing_group_id)?,
                )? {
                    seq_pb.set_message(format!("Skipped committed {}", sequence.meta.bone_id));
                    seq_pb.inc(1);
                    continue;
                }
                journal.invalidate_committed(
                    &processing_group_id,
                    "Committed artifact verification failed; scheduling deterministic rebuild",
                )?;
            }
        }
        match journal.claim(&processing_group_id, retry_limit)? {
            ClaimDecision::Process { .. } => {}
            ClaimDecision::AlreadyCommitted => {
                seq_pb.inc(1);
                continue;
            }
            ClaimDecision::RetryLimitReached { attempts } => {
                failures.push(format!(
                    "{} reached retry limit after {} attempts",
                    sequence.meta.bone_id, attempts
                ));
                seq_pb.inc(1);
                continue;
            }
        }

        seq_pb.set_message(format!("Sequence {}", sequence.meta.bone_id));
        let step_pb = mp.add(ProgressBar::new(4));
        step_pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:30.magenta/blue}] {pos}/{len} {msg}")
                .unwrap(),
        );
        let failure_pb = step_pb.clone();

        let mut timer = HierarchicalTimer::new(&sequence.meta.bone_id);
        let group_started = std::time::Instant::now();
        let mut group_fusion_config = fusion_config.clone();
        group_fusion_config.fusion_edit = group_edit.cloned();
        let process_result = (|| -> Result<BurstExportTask> {
            let crop_plan = loader.resolved_sequence_crop_plan(
                sequence,
                capture_group.crop_plan,
                &mut timer,
            )?;
            let rect = crop_plan
                .rect
                .context("Resolved burst crop has no rectangle")?;
            let (x0, y0, x1, y1) = rect.to_bounds();
            let width = x1.checked_sub(x0).context("Burst crop width underflow")?;
            let height = y1.checked_sub(y0).context("Burst crop height underflow")?;
            let demosaic_scratch_bytes = gpu_ahd
                .as_ref()
                .map(|engine| engine.scratch_bytes(width, height))
                .transpose()?
                .flatten()
                .unwrap_or(0);
            let estimate = NativeSequenceMemoryEstimate::estimate(
                sequence.len(),
                width,
                height,
                num_cpus::get(),
                fusion_config.tile_size,
                fusion_config.focus_coarse_stride,
                fusion_config.glare_fallback_radius_pixels,
                fusion_config.local_alignment_cell_size,
                fusion_config.analysis_max_dimension,
                demosaic_scratch_bytes,
                group_edit.is_some(),
            )?;
            let memory_permit =
                memory_credits.acquire(estimate.peak_memory_bytes, &cancellation)?;
            ensure_not_cancelled(&cancellation)?;

            step_pb.set_message("Decoding native ROI group");
            let faults_before = major_page_faults();
            let decode_started = std::time::Instant::now();
            let group = loader.load_sequence_native_with_plan_into(
                sequence,
                Some(crop_plan),
                &mut native_arena,
                &mut timer,
            )?;
            let decode_seconds = decode_started.elapsed().as_secs_f64();
            let major_page_faults = major_page_faults().saturating_sub(faults_before);
            ensure_not_cancelled(&cancellation)?;
            let native_input_bytes = group.size_bytes();
            let decoded_megapixels =
                group.len() as f64 * group.width as f64 * group.height as f64 / 1_000_000.0;
            step_pb.inc(1);

            step_pb.set_message("Fusing HDR and focus planes");
            let fusion_started = std::time::Instant::now();
            let fused = fuse_native_group_with_id(
                &group,
                &sequence.meta,
                &group_fusion_config,
                Some(&capture_group.group_id),
            )?;
            let fusion_seconds = fusion_started.elapsed().as_secs_f64();
            drop(group);
            ensure_not_cancelled(&cancellation)?;
            let fused_bytes = fused.size_bytes();
            step_pb.inc(1);

            step_pb.set_message("Demosaicing and tone mapping");
            let demosaic_started = std::time::Instant::now();
            let NativeFusionResult {
                bayer,
                depth,
                metric_depth_m,
                focus_diopters,
                confidence: _,
                radiance_uncertainty: _,
                source_map,
                detail_source_map,
                fusion_flags,
                frequency_flags,
                sensor_correction_map,
                glare_map,
                boundary_trimap,
                fusion_edit_map,
                foreground_mask,
                transforms,
                frame_alignments,
                radiance_anchor,
                noise_model_calibrated,
                sensor_correction_id,
                lens_psf_calibration_id,
                lens_psf_calibrated,
                defect_repaired_pixels,
                depth_refusion_pixels,
                visibility_adjusted_pixels,
                visibility_constrained,
                mixed_boundary_pixels,
                boundary_source_fallback_pixels,
                trimap_radius_pixels,
                trimap_physical_scale,
                glare_radius_pixels,
                glare_physical_scale,
                glare_affected_pixels,
                frequency_separated_pixels,
                detail_single_source_pixels,
                detail_reference_pixels,
                operator_override_pixels,
                fusion_edit_digest,
                fusion_edit_base_report_sha256,
                focus_kernel,
            } = fused;
            let (fusion_overlay_rgb, fusion_overlay_alpha) =
                fusion_provenance_preview_with_frequency(
                    &source_map,
                    &detail_source_map,
                    &fusion_flags,
                    &frequency_flags,
                    preview_max_dimension.min(2048),
                )?;
            let rgb_cam: [[f32; 4]; 3] = [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ];
            let (linear_rgb, demosaic_backend, demosaic_bands, demosaic_adapter, demosaic_fallback) =
                if let Some(engine) = gpu_ahd.as_ref() {
                    match engine.demosaic(&bayer, &rgb_cam) {
                        Ok(Some(output)) => (
                            output.image,
                            "metal_ahd",
                            output.bands,
                            Some(output.adapter),
                            None,
                        ),
                        Ok(None) => (
                            ahd_demosaic_f32_owned(bayer, &rgb_cam)?,
                            "cpu_ahd",
                            0,
                            None,
                            Some("workload_below_metal_threshold".to_string()),
                        ),
                        Err(error) => {
                            tracing::warn!(
                                "Metal AHD failed for {}; using deterministic CPU: {error:#}",
                                sequence.meta.bone_id
                            );
                            (
                                ahd_demosaic_f32_owned(bayer, &rgb_cam)?,
                                "cpu_ahd",
                                0,
                                None,
                                Some(format!("metal_runtime_error: {error:#}")),
                            )
                        }
                    }
                } else {
                    (
                        ahd_demosaic_f32_owned(bayer, &rgb_cam)?,
                        "cpu_ahd",
                        0,
                        None,
                        Some(if no_gpu {
                            "operator_disabled_gpu".to_string()
                        } else {
                            "qualified_metal_unavailable".to_string()
                        }),
                    )
                };
            let display_rgb = postprocess_f32(&linear_rgb)?;
            let demosaic_and_postprocess_seconds = demosaic_started.elapsed().as_secs_f64();
            ensure_not_cancelled(&cancellation)?;
            step_pb.inc(1);

            let output_path = burst_group_output_path(
                output,
                sequence,
                &capture_group.group_id,
                group_edit_digest.as_deref(),
            );
            let preview_path = output_path.with_extension("png");
            let output_stem = output_path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("trueshot-burst");
            let depth_path = output_path.with_file_name(format!("{output_stem}_depth.tiff"));
            let metric_depth_path =
                output_path.with_file_name(format!("{output_stem}_depth_m.pfm"));
            let source_map_path =
                output_path.with_file_name(format!("{output_stem}_source_map.png"));
            let detail_source_map_path =
                output_path.with_file_name(format!("{output_stem}_detail_source_map.png"));
            let fusion_flags_path =
                output_path.with_file_name(format!("{output_stem}_fusion_flags.png"));
            let frequency_flags_path =
                output_path.with_file_name(format!("{output_stem}_frequency_flags.png"));
            let glare_map_path = output_path.with_file_name(format!("{output_stem}_glare_map.png"));
            let sensor_correction_map_path =
                output_path.with_file_name(format!("{output_stem}_sensor_correction.png"));
            let boundary_trimap_path =
                output_path.with_file_name(format!("{output_stem}_boundary_trimap.png"));
            let fusion_edit_map_path =
                output_path.with_file_name(format!("{output_stem}_fusion_edit_map.png"));
            let fusion_overlay_path =
                output_path.with_file_name(format!("{output_stem}_fusion_overlay.png"));
            let fusion_report_path =
                output_path.with_file_name(format!("{output_stem}_fusion_report.json"));
            let source_map_name = portable_asset_filename(&source_map_path)?;
            let detail_source_map_name = portable_asset_filename(&detail_source_map_path)?;
            let fusion_flags_name = portable_asset_filename(&fusion_flags_path)?;
            let frequency_flags_name = portable_asset_filename(&frequency_flags_path)?;
            let glare_map_name = portable_asset_filename(&glare_map_path)?;
            let sensor_correction_map_name = portable_asset_filename(&sensor_correction_map_path)?;
            let boundary_trimap_name = portable_asset_filename(&boundary_trimap_path)?;
            let fusion_edit_map_name = fusion_edit_map
                .as_ref()
                .map(|_| portable_asset_filename(&fusion_edit_map_path))
                .transpose()?;
            let fusion_overlay_name = portable_asset_filename(&fusion_overlay_path)?;
            let accepted_transforms = transforms
                .iter()
                .filter(|transform| transform.accepted)
                .count();
            let transform_count = transforms.len();
            let accepted_bracket_transforms = frame_alignments
                .iter()
                .filter(|alignment| !alignment.reference_frame && alignment.global_accepted)
                .count();
            let local_aligned_cells = frame_alignments
                .iter()
                .map(|alignment| u64::from(alignment.local_aligned_cells))
                .sum::<u64>();
            let disoccluded_cells = frame_alignments
                .iter()
                .map(|alignment| u64::from(alignment.disoccluded_cells))
                .sum::<u64>();
            let flag_count = |flag: u8| {
                fusion_flags
                    .iter()
                    .filter(|value| **value & flag != 0)
                    .count()
            };
            let frequency_flag_count = |flag: u8| {
                frequency_flags
                    .iter()
                    .filter(|value| **value & flag != 0)
                    .count()
            };
            let frequency_flag_legend = serde_json::json!({
                "separated": {"bit": FREQUENCY_FLAG_SEPARATED, "pixels": frequency_flag_count(FREQUENCY_FLAG_SEPARATED)},
                "split_sources": {"bit": FREQUENCY_FLAG_SPLIT_SOURCES, "pixels": frequency_flag_count(FREQUENCY_FLAG_SPLIT_SOURCES)},
                "detail_reference": {"bit": FREQUENCY_FLAG_DETAIL_REFERENCE, "pixels": frequency_flag_count(FREQUENCY_FLAG_DETAIL_REFERENCE)},
                "detail_single_source": {"bit": FREQUENCY_FLAG_DETAIL_SINGLE_SOURCE, "pixels": frequency_flag_count(FREQUENCY_FLAG_DETAIL_SINGLE_SOURCE)},
                "measurement_clamped": {"bit": FREQUENCY_FLAG_MEASUREMENT_CLAMPED, "pixels": frequency_flag_count(FREQUENCY_FLAG_MEASUREMENT_CLAMPED)}
            });
            let mut fusion_report_value = serde_json::json!({
                "schema": "trueshot.fusion.provenance.v2",
                "width": source_map.dim().1,
                "height": source_map.dim().0,
                "source_sentinel": u16::MAX,
                "source_map": source_map_name,
                "fusion_flags": fusion_flags_name,
                "sensor_correction_map": sensor_correction_map_name,
                "sensor_correction_legend": {
                    "flat_field_applied": SENSOR_CORRECTION_FLAT_FIELD,
                    "defect_repaired": SENSOR_CORRECTION_DEFECT_REPAIRED
                },
                "glare_map": glare_map_name,
                "boundary_trimap": boundary_trimap_name,
                "boundary_trimap_legend": {
                    "interior": trueshot_core::native_fusion::BOUNDARY_TRIMAP_INTERIOR,
                    "psf_support": trueshot_core::native_fusion::BOUNDARY_TRIMAP_PSF_SUPPORT,
                    "crossing_core": trueshot_core::native_fusion::BOUNDARY_TRIMAP_CROSSING_CORE
                },
                "overlay": fusion_overlay_name,
                "flag_legend": {
                    "censored": {"bit": FUSION_FLAG_CENSORED, "pixels": flag_count(FUSION_FLAG_CENSORED)},
                    "outlier_rejected": {"bit": FUSION_FLAG_OUTLIER_REJECTED, "pixels": flag_count(FUSION_FLAG_OUTLIER_REJECTED)},
                    "source_fallback": {"bit": FUSION_FLAG_SOURCE_FALLBACK, "pixels": flag_count(FUSION_FLAG_SOURCE_FALLBACK)},
                    "uncalibrated_noise": {"bit": FUSION_FLAG_UNCALIBRATED_NOISE, "pixels": flag_count(FUSION_FLAG_UNCALIBRATED_NOISE)},
                    "censor_conflict": {"bit": FUSION_FLAG_CENSOR_CONFLICT, "pixels": flag_count(FUSION_FLAG_CENSOR_CONFLICT)},
                    "visibility_corrected": {"bit": FUSION_FLAG_VISIBILITY_CORRECTED, "pixels": flag_count(FUSION_FLAG_VISIBILITY_CORRECTED)},
                    "bracket_aligned": {"bit": FUSION_FLAG_BRACKET_ALIGNED, "pixels": flag_count(FUSION_FLAG_BRACKET_ALIGNED)},
                    "disoccluded": {"bit": FUSION_FLAG_DISOCCLUDED, "pixels": flag_count(FUSION_FLAG_DISOCCLUDED)}
                },
                "noise_model_calibrated": noise_model_calibrated,
                "sensor_correction_id": sensor_correction_id,
                "lens_psf_calibration_id": lens_psf_calibration_id,
                "lens_psf_calibrated": lens_psf_calibrated,
                "physical_focus_policy": if lens_psf_calibrated {
                    "calibrated_breathing_pupil_field_psf"
                } else {
                    "ideal_thin_lens_explicit_fallback"
                },
                "defect_repaired_pixels": defect_repaired_pixels,
                "depth_refusion_pixels": depth_refusion_pixels,
                "visibility_adjusted_pixels": visibility_adjusted_pixels,
                "visibility_constrained": visibility_constrained,
                "mixed_boundary_pixels": mixed_boundary_pixels,
                "boundary_source_fallback_pixels": boundary_source_fallback_pixels,
                "trimap_radius_pixels": trimap_radius_pixels,
                "trimap_physical_scale": trimap_physical_scale,
                "boundary_policy": "single_traceable_measured_focus_plane_no_cross_depth_interpolation",
                "glare_radius_pixels": glare_radius_pixels,
                "glare_physical_scale": glare_physical_scale,
                "glare_affected_pixels": glare_affected_pixels,
                "glare_policy": "focus_evidence_suppression_only_measured_radiance_unchanged",
                "focus_kernel": focus_kernel,
                "demosaic": {
                    "backend": demosaic_backend,
                    "bands": demosaic_bands,
                    "adapter": demosaic_adapter,
                    "fallback": demosaic_fallback,
                    "scratch_bytes_admitted": demosaic_scratch_bytes,
                    "measured_cfa_policy": "exact",
                    "generative_reconstruction": false
                },
                "local_aligned_cells": local_aligned_cells,
                "disoccluded_cells": disoccluded_cells,
                "frame_alignments": frame_alignments,
                "frequency_separated_deghosting": frequency_separated_deghosting,
                "archival_policy": "measured_sources_only_no_generative_reconstruction"
            });
            if let Some(replay) = replay_capsule.as_ref() {
                fusion_report_value
                    .as_object_mut()
                    .expect("fusion report literal is an object")
                    .insert("replay".to_string(), serde_json::to_value(replay)?);
            }
            let report = fusion_report_value
                .as_object_mut()
                .expect("fusion report literal is an object");
            report.insert(
                "capture_group_id".to_string(),
                serde_json::json!(capture_group.group_id),
            );
            report.insert(
                "revision_group_id".to_string(),
                serde_json::json!(processing_group_id),
            );
            report.insert("frame_count".to_string(), serde_json::json!(sequence.len()));
            report.insert(
                "crop_origin".to_string(),
                serde_json::json!({"x": x0, "y": y0}),
            );
            report.insert(
                "detail_source_map".to_string(),
                serde_json::json!(detail_source_map_name),
            );
            report.insert(
                "frequency_flags".to_string(),
                serde_json::json!(frequency_flags_name),
            );
            report.insert("frequency_flag_legend".to_string(), frequency_flag_legend);
            report.insert(
                "frequency_separated_pixels".to_string(),
                serde_json::json!(frequency_separated_pixels),
            );
            report.insert(
                "detail_single_source_pixels".to_string(),
                serde_json::json!(detail_single_source_pixels),
            );
            report.insert(
                "detail_reference_pixels".to_string(),
                serde_json::json!(detail_reference_pixels),
            );
            report.insert(
                "frequency_policy".to_string(),
                serde_json::json!(
                    "same_cfa_sparse_low_detail_measured_sources_only_envelope_clamped"
                ),
            );
            if let (Some(edit), Some(edit_digest), Some(edit_map_name)) = (
                group_edit,
                fusion_edit_digest.as_ref(),
                fusion_edit_map_name.as_ref(),
            ) {
                report.insert(
                    "fusion_edit".to_string(),
                    serde_json::json!({
                        "schema": edit.schema,
                        "digest": edit_digest,
                        "base_report_sha256": fusion_edit_base_report_sha256,
                        "map": edit_map_name,
                        "map_legend": {
                            "measured_source_override": FUSION_EDIT_MEASURED_SOURCE
                        },
                        "operation_count": edit.operations.len(),
                        "edited_pixels": operator_override_pixels,
                        "operations": edit.operations,
                        "policy": "aligned_uncensored_measured_frame_only_atomic_fail_closed"
                    }),
                );
            }
            report.insert(
                "performance".to_string(),
                serde_json::json!({
                    "decode_seconds": decode_seconds,
                    "fusion_seconds": fusion_seconds,
                    "demosaic_and_postprocess_seconds": demosaic_and_postprocess_seconds,
                    "processing_before_export_seconds": group_started.elapsed().as_secs_f64(),
                    "decoded_megapixels": decoded_megapixels,
                    "native_input_bytes": native_input_bytes,
                    "fused_bytes": fused_bytes,
                    "admitted_peak_memory_bytes": estimate.peak_memory_bytes,
                    "major_page_faults": major_page_faults
                }),
            );
            let fusion_report = serde_json::to_vec_pretty(&fusion_report_value)?;
            let output_root = output.to_path_buf();
            let group_id = processing_group_id.clone();
            let label = sequence.meta.bone_id.clone();
            let depth = export_depth.then_some(depth);
            let metric_depth_m = export_depth.then_some(metric_depth_m).flatten();
            let physical_focus_planes = focus_diopters.len();
            let preview_max_dimension = if full_resolution_preview {
                usize::MAX
            } else {
                preview_max_dimension
            };
            step_pb.set_message("Queued for atomic export");
            Ok(BurstExportTask {
                group_id,
                label,
                started_at: group_started,
                decode_seconds,
                decoded_megapixels,
                major_page_faults,
                export: Box::new(move || {
                    // Retain admission credits until every large array is written
                    // and dropped by the export worker.
                    let _memory_permit = memory_permit;
                    step_pb.set_message("Exporting outputs");
                    let linear_digest = save_tiff16_from_f32_with_digest(
                        &linear_rgb,
                        &foreground_mask,
                        &output_path,
                    )?;
                    let preview_digest = save_png_preview_with_digest(
                        &display_rgb,
                        &foreground_mask,
                        &preview_path,
                        preview_max_dimension,
                    )?;
                    let mut artifacts = vec![
                        artifact_digest_from_parts(
                            &output_path,
                            &output_root,
                            linear_digest.size_bytes,
                            linear_digest.sha256,
                        )?,
                        artifact_digest_from_parts(
                            &preview_path,
                            &output_root,
                            preview_digest.size_bytes,
                            preview_digest.sha256,
                        )?,
                    ];
                    let source_digest =
                        save_u16_map_png_with_digest(&source_map, &source_map_path)?;
                    artifacts.push(artifact_digest_from_parts(
                        &source_map_path,
                        &output_root,
                        source_digest.size_bytes,
                        source_digest.sha256,
                    )?);
                    let detail_source_digest =
                        save_u16_map_png_with_digest(&detail_source_map, &detail_source_map_path)?;
                    artifacts.push(artifact_digest_from_parts(
                        &detail_source_map_path,
                        &output_root,
                        detail_source_digest.size_bytes,
                        detail_source_digest.sha256,
                    )?);
                    let flags_digest =
                        save_u8_map_png_with_digest(&fusion_flags, &fusion_flags_path)?;
                    artifacts.push(artifact_digest_from_parts(
                        &fusion_flags_path,
                        &output_root,
                        flags_digest.size_bytes,
                        flags_digest.sha256,
                    )?);
                    let frequency_digest =
                        save_u8_map_png_with_digest(&frequency_flags, &frequency_flags_path)?;
                    artifacts.push(artifact_digest_from_parts(
                        &frequency_flags_path,
                        &output_root,
                        frequency_digest.size_bytes,
                        frequency_digest.sha256,
                    )?);
                    let correction_digest = save_u8_map_png_with_digest(
                        &sensor_correction_map,
                        &sensor_correction_map_path,
                    )?;
                    artifacts.push(artifact_digest_from_parts(
                        &sensor_correction_map_path,
                        &output_root,
                        correction_digest.size_bytes,
                        correction_digest.sha256,
                    )?);
                    let glare_digest = save_u8_map_png_with_digest(&glare_map, &glare_map_path)?;
                    artifacts.push(artifact_digest_from_parts(
                        &glare_map_path,
                        &output_root,
                        glare_digest.size_bytes,
                        glare_digest.sha256,
                    )?);
                    let trimap_digest =
                        save_u8_map_png_with_digest(&boundary_trimap, &boundary_trimap_path)?;
                    artifacts.push(artifact_digest_from_parts(
                        &boundary_trimap_path,
                        &output_root,
                        trimap_digest.size_bytes,
                        trimap_digest.sha256,
                    )?);
                    if let Some(edit_map) = fusion_edit_map {
                        let edit_digest =
                            save_u8_map_png_with_digest(&edit_map, &fusion_edit_map_path)?;
                        artifacts.push(artifact_digest_from_parts(
                            &fusion_edit_map_path,
                            &output_root,
                            edit_digest.size_bytes,
                            edit_digest.sha256,
                        )?);
                    }
                    let overlay_digest = save_png_preview_with_digest(
                        &fusion_overlay_rgb,
                        &fusion_overlay_alpha,
                        &fusion_overlay_path,
                        usize::MAX,
                    )?;
                    artifacts.push(artifact_digest_from_parts(
                        &fusion_overlay_path,
                        &output_root,
                        overlay_digest.size_bytes,
                        overlay_digest.sha256,
                    )?);
                    let report_digest =
                        save_bytes_with_digest(&fusion_report, &fusion_report_path)?;
                    artifacts.push(artifact_digest_from_parts(
                        &fusion_report_path,
                        &output_root,
                        report_digest.size_bytes,
                        report_digest.sha256,
                    )?);
                    if let Some(depth) = depth {
                        let depth_digest = save_depth_tiff_with_digest(&depth, &depth_path)?;
                        artifacts.push(artifact_digest_from_parts(
                            &depth_path,
                            &output_root,
                            depth_digest.size_bytes,
                            depth_digest.sha256,
                        )?);
                    }
                    if let Some(metric_depth_m) = metric_depth_m {
                        let metric_digest =
                            save_metric_depth_pfm_with_digest(&metric_depth_m, &metric_depth_path)?;
                        artifacts.push(artifact_digest_from_parts(
                            &metric_depth_path,
                            &output_root,
                            metric_digest.size_bytes,
                            metric_digest.sha256,
                        )?);
                    }
                    step_pb.inc(1);
                    step_pb.finish_with_message(format!(
                        "Sequence complete ({:.1} MiB native + {:.1} MiB fused, {}/{} focus transforms, {} bracket transforms, {} local/{} disoccluded cells, {} physical focus planes, anchor {:.3e})",
                        native_input_bytes as f64 / (1024.0 * 1024.0),
                        fused_bytes as f64 / (1024.0 * 1024.0),
                        accepted_transforms,
                        transform_count,
                        accepted_bracket_transforms,
                        local_aligned_cells,
                        disoccluded_cells,
                        physical_focus_planes,
                        radiance_anchor,
                    ));
                    Ok(artifacts)
                }),
            })
        })();

        match process_result {
            Ok(task) => {
                let mut writer_wait_seconds = 0.0;
                if export_pending {
                    let wait_started = std::time::Instant::now();
                    let outcome = export_receiver
                        .recv()
                        .context("Burst export worker stopped unexpectedly")?;
                    writer_wait_seconds = wait_started.elapsed().as_secs_f64();
                    complete_burst_export(&journal, outcome, &mut failures)?;
                }
                let pressure_sample = PipelinePressureSample {
                    decoded_megapixels: task.decoded_megapixels,
                    decode_seconds: task.decode_seconds,
                    writer_wait_seconds,
                    available_memory_ratio: available_memory_ratio(),
                    major_page_faults: task.major_page_faults,
                };
                export_sender
                    .send(task)
                    .map_err(|_| anyhow::anyhow!("Burst export worker stopped unexpectedly"))?;
                export_pending = true;
                if let Some(controller) = adaptive_workers.as_mut() {
                    let adjustment = controller.observe(pressure_sample);
                    if adjustment.changed {
                        tracing::info!(
                            "Adaptive NEF workers: {} -> {} (decode {:.2}s, writer wait {:.2}s, major faults {})",
                            adjustment.previous_workers,
                            adjustment.workers,
                            pressure_sample.decode_seconds,
                            pressure_sample.writer_wait_seconds,
                            pressure_sample.major_page_faults,
                        );
                        options.max_parallel_sequences = Some(adjustment.workers);
                        loader = build_loader(options.clone());
                    }
                }
            }
            Err(error) => {
                failure_pb.abandon_with_message(format!("Sequence failed: {error:#}"));
                if cancellation.is_cancelled() {
                    journal.mark_interrupted(
                        &processing_group_id,
                        "Operator cancellation; safe to resume",
                    )?;
                } else {
                    journal.mark_failed(&processing_group_id, &error)?;
                }
                failures.push(format!("{}: {error:#}", sequence.meta.bone_id));
            }
        }
        seq_pb.inc(1);

        if options.verbose_timing {
            timer.report_console(true);
        }
    }

    if export_pending {
        let outcome = export_receiver
            .recv()
            .context("Burst export worker stopped unexpectedly")?;
        complete_burst_export(&journal, outcome, &mut failures)?;
    }
    drop(export_sender);
    export_worker
        .join()
        .map_err(|_| anyhow::anyhow!("Burst export worker panicked"))?;

    seq_pb.finish_with_message("All sequences complete");
    if cancellation.is_cancelled() {
        anyhow::bail!("Burst processing cancelled; completed groups are safe to resume");
    }
    if fusion_edit.is_some() && !fusion_edit_target_seen {
        anyhow::bail!("Fusion edit capture group was not present in the input collection");
    }
    if !failures.is_empty() {
        anyhow::bail!(
            "{} capture groups failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
    if let Some(state) = run_state {
        state.mark_step_completed("sfm", collect_artifacts(output, &["focus_stacks"]));
        state.mark_step_completed("reports", collect_artifacts(output, &["run_report.json"]));
    }
    Ok(())
}

fn configured_memory_budget(available_memory: u64) -> Result<u64> {
    if let Ok(value) = env::var("TRUESHOT_MEMORY_BUDGET_MIB") {
        let mebibytes = value
            .parse::<u64>()
            .context("TRUESHOT_MEMORY_BUDGET_MIB must be an integer")?;
        let budget = mebibytes
            .checked_mul(1024 * 1024)
            .context("Configured memory budget overflow")?;
        if budget == 0 {
            anyhow::bail!("TRUESHOT_MEMORY_BUDGET_MIB must be greater than zero");
        }
        if available_memory != 0 && budget > available_memory {
            anyhow::bail!(
                "Configured memory budget is {:.1} MiB, but only {:.1} MiB is currently available",
                budget as f64 / (1024.0 * 1024.0),
                available_memory as f64 / (1024.0 * 1024.0),
            );
        }
        return Ok(budget);
    }
    if available_memory == 0 {
        anyhow::bail!("Unable to determine available system memory");
    }
    Ok((available_memory / 4 * 3)
        .max(64 * 1024 * 1024)
        .min(available_memory))
}

fn resume_verification_policy(group_id: &str) -> Result<ArtifactVerification> {
    match env::var("TRUESHOT_RESUME_VERIFY")
        .unwrap_or_else(|_| "sampled".to_string())
        .to_ascii_lowercase()
        .as_str()
    {
        "fast" | "metadata" => Ok(ArtifactVerification::Metadata),
        "full" | "hash" => Ok(ArtifactVerification::FullHash),
        "sampled" => {
            let sample_rate = env::var("TRUESHOT_RESUME_HASH_SAMPLE_RATE")
                .ok()
                .map(|value| {
                    value
                        .parse::<u32>()
                        .context("TRUESHOT_RESUME_HASH_SAMPLE_RATE must be an integer")
                })
                .transpose()?
                .unwrap_or(1_000)
                .max(1);
            let prefix = group_id
                .get(..8)
                .context("Capture group ID is too short for sampled verification")?;
            let sample =
                u32::from_str_radix(prefix, 16).context("Capture group ID is not hexadecimal")?;
            Ok(if sample % sample_rate == 0 {
                ArtifactVerification::FullHash
            } else {
                ArtifactVerification::Metadata
            })
        }
        value => anyhow::bail!(
            "Unknown TRUESHOT_RESUME_VERIFY value {value}; use metadata, sampled, or full"
        ),
    }
}

type BurstExportJob = Box<dyn FnOnce() -> Result<Vec<ArtifactDigest>> + Send + 'static>;

struct BurstExportTask {
    group_id: String,
    label: String,
    started_at: std::time::Instant,
    decode_seconds: f64,
    decoded_megapixels: f64,
    major_page_faults: u64,
    export: BurstExportJob,
}

struct BurstExportOutcome {
    group_id: String,
    label: String,
    duration_ms: u64,
    result: Result<Vec<ArtifactDigest>>,
}

fn burst_export_worker() -> Result<(
    std::sync::mpsc::SyncSender<BurstExportTask>,
    std::sync::mpsc::Receiver<BurstExportOutcome>,
    std::thread::JoinHandle<()>,
)> {
    let (task_sender, task_receiver) = std::sync::mpsc::sync_channel::<BurstExportTask>(1);
    let (result_sender, result_receiver) = std::sync::mpsc::channel::<BurstExportOutcome>();
    let worker = std::thread::Builder::new()
        .name("trueshot-burst-export".to_string())
        .spawn(move || {
            while let Ok(task) = task_receiver.recv() {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task.export))
                    .map_err(|payload| {
                        let message = payload
                            .downcast_ref::<&str>()
                            .copied()
                            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                            .unwrap_or("unknown panic");
                        anyhow::anyhow!("Burst export worker panicked: {message}")
                    })
                    .and_then(|result| result);
                let outcome = BurstExportOutcome {
                    group_id: task.group_id,
                    label: task.label,
                    duration_ms: task.started_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    result,
                };
                if result_sender.send(outcome).is_err() {
                    break;
                }
            }
        })
        .context("Start burst export worker")?;
    Ok((task_sender, result_receiver, worker))
}

fn complete_burst_export(
    journal: &ProcessingJournal,
    outcome: BurstExportOutcome,
    failures: &mut Vec<String>,
) -> Result<()> {
    match outcome.result {
        Ok(artifacts) => {
            journal.mark_committed(&outcome.group_id, outcome.duration_ms, artifacts)?;
        }
        Err(error) => {
            journal.mark_failed(&outcome.group_id, &error)?;
            failures.push(format!("{}: {error:#}", outcome.label));
        }
    }
    Ok(())
}

fn install_cancellation_listener(cancellation: CancellationToken) {
    let _ = std::thread::Builder::new()
        .name("trueshot-signal-listener".to_string())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()
            else {
                return;
            };
            if runtime.block_on(tokio::signal::ctrl_c()).is_ok() {
                cancellation.cancel();
                eprintln!("Cancellation requested; finishing the durable export boundary...");
            }
        });
}

fn ensure_not_cancelled(cancellation: &CancellationToken) -> Result<()> {
    if cancellation.is_cancelled() {
        anyhow::bail!("Processing cancelled")
    }
    Ok(())
}

fn available_memory_ratio() -> f64 {
    let resources = SystemResources::query();
    if resources.total_memory == 0 {
        0.0
    } else {
        resources.available_memory as f64 / resources.total_memory as f64
    }
}

#[cfg(unix)]
fn major_page_faults() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the supplied rusage structure on success.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if status == 0 {
        // SAFETY: status 0 guarantees initialization by getrusage.
        unsafe { usage.assume_init() }.ru_majflt.max(0) as u64
    } else {
        0
    }
}

#[cfg(not(unix))]
fn major_page_faults() -> u64 {
    0
}

fn burst_group_output_path(
    output: &Path,
    sequence: &trueshot_core::types::Sequence,
    group_id: &str,
    edit_digest: Option<&str>,
) -> PathBuf {
    let sanitize = |value: &str| {
        value
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    let revision =
        edit_digest.map_or_else(String::new, |digest| format!("_edit_{}", &digest[..12]));
    output.join(format!(
        "{}_{}_{:03}deg_{}{}.tiff",
        sanitize(&sequence.meta.bone_id),
        sanitize(&sequence.meta.vantage),
        sequence.meta.rot_deg as u32,
        &group_id[..12],
        revision,
    ))
}

fn fusion_report_path_for_output(output_path: &Path) -> PathBuf {
    let stem = output_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("trueshot-burst");
    output_path.with_file_name(format!("{stem}_fusion_report.json"))
}

fn fusion_revision_group_id(capture_group_id: &str, edit_digest: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"trueshot-fusion-revision-v1\0");
    digest.update(capture_group_id.as_bytes());
    digest.update([0]);
    digest.update(edit_digest.as_bytes());
    hex::encode(digest.finalize())
}

#[allow(clippy::too_many_arguments)]
fn build_fusion_replay_capsule(
    input: &Path,
    output: &Path,
    quality: Quality,
    jobs: Option<usize>,
    full_frame: bool,
    no_gpu: bool,
    export_depth: bool,
    full_resolution_preview: bool,
    preview_max_dimension: usize,
    deghost_strength: f32,
    frequency_separated_deghosting: bool,
    glare_spread_um: f32,
    glare_aware_focus: bool,
    depth_consistent_refusion: bool,
    sensor_noise_profile_path: Option<&Path>,
    sensor_correction_profile_path: Option<&Path>,
    lens_psf_profile_path: Option<&Path>,
) -> Result<Option<FusionReplayCapsule>> {
    let input_layout = if input.is_absolute() {
        input.to_path_buf()
    } else {
        std::env::current_dir()?.join(input)
    };
    let output_layout = if output.is_absolute() {
        output.to_path_buf()
    } else {
        std::env::current_dir()?.join(output)
    };
    let Some(input_layout_parent) = input_layout.parent() else {
        return Ok(None);
    };
    let Some(output_layout_parent) = output_layout.parent() else {
        return Ok(None);
    };
    if input_layout.file_name().and_then(|name| name.to_str()) != Some("raw")
        || output_layout.file_name().and_then(|name| name.to_str()) != Some("output")
        || input_layout_parent != output_layout_parent
    {
        return Ok(None);
    }
    let input = input
        .canonicalize()
        .with_context(|| format!("Resolve replay input {}", input.display()))?;
    let output = output
        .canonicalize()
        .with_context(|| format!("Resolve replay output {}", output.display()))?;
    let project_root = output
        .parent()
        .context("Replay output has no project root")?;
    let profile = |path: Option<&Path>| -> Result<Option<FusionReplayArtifact>> {
        let Some(path) = path else {
            return Ok(None);
        };
        let canonical = path
            .canonicalize()
            .with_context(|| format!("Resolve replay profile {}", path.display()))?;
        let Ok(relative) = canonical.strip_prefix(project_root) else {
            return Ok(None);
        };
        Ok(Some(FusionReplayArtifact {
            project_relative_path: relative.to_string_lossy().replace('\\', "/"),
            sha256: sha256_file(&canonical)?,
        }))
    };
    let sensor_noise_profile = profile(sensor_noise_profile_path)?;
    if sensor_noise_profile.is_none() && sensor_noise_profile_path.is_some() {
        return Ok(None);
    }
    let sensor_correction_profile = profile(sensor_correction_profile_path)?;
    if sensor_correction_profile.is_none() && sensor_correction_profile_path.is_some() {
        return Ok(None);
    }
    let lens_psf_profile = profile(lens_psf_profile_path)?;
    if lens_psf_profile.is_none() && lens_psf_profile_path.is_some() {
        return Ok(None);
    }
    let replay = FusionReplayCapsule {
        schema: FUSION_REPLAY_SCHEMA.to_string(),
        project_layout: "raw_output_siblings".to_string(),
        quality: match quality {
            Quality::Low => "low",
            Quality::Medium => "medium",
            Quality::High => "high",
            Quality::Ultra => "ultra",
        }
        .to_string(),
        jobs,
        full_frame,
        gpu_enabled: !no_gpu,
        export_depth,
        full_resolution_preview,
        preview_max_dimension,
        deghost_strength,
        frequency_separated_deghosting,
        glare_spread_um,
        glare_aware_focus,
        depth_consistent_refusion,
        sensor_noise_profile,
        sensor_correction_profile,
        lens_psf_profile,
    };
    replay.validate()?;
    Ok(Some(replay))
}

fn native_fusion_config(quality: Quality) -> NativeFusionConfig {
    NativeFusionConfig {
        analysis_max_dimension: match quality {
            Quality::Low => 384,
            Quality::Medium => 512,
            Quality::High => 768,
            Quality::Ultra => 1024,
        },
        ..NativeFusionConfig::default()
    }
}

fn run_reconstruction_pipeline(
    input: &Path,
    output: &Path,
    mode: Mode,
    quality: Quality,
    inventory_ctx: Option<&InventoryContext>,
    mut run_state: Option<&mut RunStateManager>,
) -> Result<()> {
    std::fs::create_dir_all(output)?;

    if let Some(state) = run_state.as_deref_mut() {
        state.mark_step_started("scan_input");
    }

    let image_dir = if input.join("images").is_dir() {
        input.join("images")
    } else {
        input.to_path_buf()
    };
    validate_photogrammetry_input(&image_dir, 6)
        .context("Photogrammetry input validation failed")?;

    let image_paths = collect_sfm_images(&image_dir)?;
    if image_paths.len() < 2 {
        anyhow::bail!("Need at least 2 images for reconstruction");
    }

    if let Some(state) = run_state.as_deref_mut() {
        state.mark_step_completed("scan_input", vec![]);
    }

    let sfm_config = sfm_config_from_quality(quality);
    let mut pipeline = SfmPipeline::new(sfm_config);
    let mut intrinsics_reports: Vec<(String, IntrinsicsReport)> = Vec::new();
    for path in &image_paths {
        let (intrinsics, report) = estimate_intrinsics_with_report(path)
            .with_context(|| format!("Failed to estimate intrinsics for {}", path.display()))?;
        intrinsics_reports.push((path.to_string_lossy().to_string(), report));
        pipeline.add_image_with_intrinsics(path, intrinsics, None)?;
    }
    let skip_sfm = run_state
        .as_deref()
        .map(|s| s.should_skip("sfm"))
        .unwrap_or(false);
    let mut reconstruction = if skip_sfm {
        match load_sparse_checkpoint(output) {
            Ok(recon) => recon,
            Err(err) => {
                eprintln!(
                    "{} Sparse checkpoint missing or invalid ({}). Re-running SFM.",
                    WARNING, err
                );
                if let Some(state) = run_state.as_deref_mut() {
                    state.mark_step_started("sfm");
                }
                pipeline.run()?
            }
        }
    } else {
        if let Some(state) = run_state.as_deref_mut() {
            state.mark_step_started("sfm");
        }
        pipeline.run()?
    };
    let reprojection_stats = if skip_sfm && sparse_checkpoint_path(output).exists() {
        None
    } else {
        pipeline.reprojection_stats()
    };

    let poses = reconstruction.poses.clone();
    let cameras = reconstruction.cameras.clone();
    let mut color_images = load_color_images(&image_paths, 1.0)?;
    colorize_sparse_points(
        &mut reconstruction,
        &mut color_images,
        &poses,
        &cameras,
        sparse_color_views_for_quality(quality),
    );

    let sparse_path = output.join("sparse.ply");
    reconstruction.export_ply(&sparse_path)?;
    if !run_state
        .as_deref()
        .map(|s| s.should_skip("sfm"))
        .unwrap_or(false)
    {
        save_sparse_checkpoint(output, &reconstruction)?;
    }
    if let Some(state) = run_state.as_deref_mut() {
        state.mark_step_completed(
            "sfm",
            collect_artifacts(
                output,
                &[
                    ".trueshot/checkpoints/sparse_reconstruction.json",
                    "sparse.ply",
                ],
            ),
        );
    }

    let mut dense_points: Vec<(na::Point3<f64>, [u8; 3])> = Vec::new();
    if should_run_dense(quality) {
        if run_state
            .as_deref()
            .map(|s| s.should_skip("dense"))
            .unwrap_or(false)
        {
            if let Ok(points) = load_dense_checkpoint(output) {
                dense_points = points;
            }
        }
        if dense_points.is_empty() {
            if let Some(state) = run_state.as_deref_mut() {
                state.mark_step_started("dense");
            }
            dense_points = run_dense_mvs(&image_paths, &poses, &cameras, quality)?;
            if !dense_points.is_empty() {
                let dense_path = output.join("dense.ply");
                export_dense_point_cloud(&dense_points, &dense_path)?;
                save_dense_checkpoint(output, &dense_points)?;
            }
        }
        if let Some(state) = run_state.as_deref_mut() {
            state.mark_step_completed(
                "dense",
                collect_artifacts(
                    output,
                    &["dense.ply", ".trueshot/checkpoints/dense_points.zst"],
                ),
            );
        }
    }

    if matches!(mode, Mode::Gaussians | Mode::Hybrid) {
        let gaussians_path = output.join("gaussians.ply");
        let skip_gaussians = run_state
            .as_deref()
            .map(|s| s.should_skip("gaussian"))
            .unwrap_or(false)
            && gaussians_path.exists();
        if !skip_gaussians {
            if let Some(state) = run_state.as_deref_mut() {
                state.mark_step_started("gaussian");
            }
            run_gaussian_splatting(
                output,
                &image_paths,
                &poses,
                &cameras,
                quality,
                &dense_points,
                &reconstruction.points,
            )?;
        }
        if let Some(state) = run_state.as_deref_mut() {
            state.mark_step_completed("gaussian", collect_artifacts(output, &["gaussians.ply"]));
        }
    }

    if let Some(state) = run_state.as_deref_mut() {
        state.mark_step_started("reports");
    }
    write_intrinsics_report(output, &intrinsics_reports)?;
    write_reconstruction_report(
        output,
        mode,
        quality,
        &image_paths,
        &reconstruction,
        &dense_points,
        reprojection_stats.as_ref(),
        inventory_ctx,
    )?;
    if let Some(state) = run_state {
        state.mark_step_completed(
            "reports",
            collect_artifacts(
                output,
                &["reconstruction_report.json", "intrinsics_report.json"],
            ),
        );
    }

    Ok(())
}

fn build_processing_options(
    quality: Quality,
    jobs: Option<usize>,
    _no_gpu: bool,
    full_frame: bool,
) -> ProcessingOptions {
    let mut options = ProcessingOptions {
        max_parallel_sequences: jobs,
        export_format: "tiff16".to_string(),
        verbose_timing: matches!(quality, Quality::High | Quality::Ultra),
        full_decode: full_frame,
        ..ProcessingOptions::default()
    };

    match quality {
        Quality::Low => {
            options.noise_sigma = 18.0;
            options.grade_k = 1.8;
        }
        Quality::Medium => {
            options.noise_sigma = 10.0;
            options.grade_k = 1.5;
        }
        Quality::High => {
            options.noise_sigma = 6.0;
            options.grade_k = 1.2;
        }
        Quality::Ultra => {
            options.noise_sigma = 4.0;
            options.grade_k = 1.0;
        }
    }

    options
}

fn collect_sfm_images(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut images: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_sfm_image_path(path))
        .collect();

    images.sort();
    Ok(images)
}

fn is_sfm_image_path(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "tif" | "tiff")
}

fn sfm_config_from_quality(quality: Quality) -> SfmConfig {
    match quality {
        Quality::Low => SfmConfig {
            feature_type: FeatureType::Orb,
            max_features: 3000,
            match_ratio: 0.8,
            min_matches: 25,
            ba_iterations: 20,
            local_ba_window: 4,
            local_ba_stride: 2,
            local_ba_iterations: 12,
            local_ba_min_points: 120,
            local_ba_min_rmse: 1.2,
            enable_dense: false,
            num_threads: num_cpus::get().max(2),
        },
        Quality::Medium => SfmConfig {
            feature_type: FeatureType::Akaze,
            max_features: 6000,
            match_ratio: 0.75,
            min_matches: 30,
            ba_iterations: 40,
            local_ba_window: 5,
            local_ba_stride: 2,
            local_ba_iterations: 18,
            local_ba_min_points: 160,
            local_ba_min_rmse: 1.0,
            enable_dense: true,
            num_threads: num_cpus::get().max(2),
        },
        Quality::High => SfmConfig {
            feature_type: FeatureType::Sift,
            max_features: 10000,
            match_ratio: 0.7,
            min_matches: 40,
            ba_iterations: 80,
            local_ba_window: 6,
            local_ba_stride: 2,
            local_ba_iterations: 28,
            local_ba_min_points: 220,
            local_ba_min_rmse: 0.8,
            enable_dense: true,
            num_threads: num_cpus::get().max(4),
        },
        Quality::Ultra => SfmConfig {
            feature_type: FeatureType::Sift,
            max_features: 16000,
            match_ratio: 0.68,
            min_matches: 50,
            ba_iterations: 120,
            local_ba_window: 7,
            local_ba_stride: 1,
            local_ba_iterations: 36,
            local_ba_min_points: 260,
            local_ba_min_rmse: 0.7,
            enable_dense: true,
            num_threads: num_cpus::get().max(4),
        },
    }
}

fn sparse_color_views_for_quality(quality: Quality) -> usize {
    match quality {
        Quality::Low => 2,
        Quality::Medium => 4,
        Quality::High => 6,
        Quality::Ultra => 8,
    }
}

fn should_run_dense(quality: Quality) -> bool {
    matches!(quality, Quality::Medium | Quality::High | Quality::Ultra)
}

fn load_color_images(paths: &[PathBuf], scale: f32) -> Result<Vec<RgbImage>> {
    let mut images = Vec::with_capacity(paths.len());
    for path in paths {
        let img = image::open(path)
            .with_context(|| format!("Failed to open image {}", path.display()))?
            .to_rgb8();
        if (scale - 1.0).abs() > f32::EPSILON {
            let width = (img.width() as f32 * scale).round().max(1.0) as u32;
            let height = (img.height() as f32 * scale).round().max(1.0) as u32;
            let resized = image::imageops::resize(&img, width, height, FilterType::Lanczos3);
            images.push(resized);
        } else {
            images.push(img);
        }
    }
    Ok(images)
}

fn colorize_sparse_points(
    reconstruction: &mut trueshot_core::reconstruction::multicam_sfm::SparseReconstruction,
    images: &mut [RgbImage],
    poses: &[CameraPose],
    intrinsics: &[CameraIntrinsics],
    max_views: usize,
) {
    if images.is_empty() || poses.is_empty() || intrinsics.is_empty() {
        return;
    }

    let view_limit = max_views.min(images.len());
    for point in &mut reconstruction.points {
        let mut colored = false;
        for view_idx in 0..view_limit {
            if let Some((x, y)) =
                project_point(&point.position, &poses[view_idx], &intrinsics[view_idx])
            {
                let pixel = images[view_idx].get_pixel(x, y);
                point.color = [pixel[0], pixel[1], pixel[2]];
                colored = true;
                break;
            }
        }
        if !colored {
            point.color = [180, 180, 180];
        }
    }
}

fn project_point(
    point_world: &na::Point3<f64>,
    pose: &CameraPose,
    intrinsics: &CameraIntrinsics,
) -> Option<(u32, u32)> {
    let rotation = pose.rotation.to_rotation_matrix();
    let cam = rotation.inverse() * (point_world.coords - pose.translation);
    if cam.z <= 0.0 {
        return None;
    }
    let x = (intrinsics.fx * cam.x / cam.z + intrinsics.cx).round();
    let y = (intrinsics.fy * cam.y / cam.z + intrinsics.cy).round();
    if x < 0.0 || y < 0.0 {
        return None;
    }
    let (x, y) = (x as u32, y as u32);
    if x < intrinsics.width && y < intrinsics.height {
        Some((x, y))
    } else {
        None
    }
}

fn run_dense_mvs(
    image_paths: &[PathBuf],
    poses: &[CameraPose],
    intrinsics: &[CameraIntrinsics],
    quality: Quality,
) -> Result<Vec<(na::Point3<f64>, [u8; 3])>> {
    let scale = dense_scale_for_quality(quality);
    let images = load_mvs_images(image_paths, intrinsics, scale)?;
    let adjusted_intrinsics: Vec<CameraIntrinsics> =
        images.iter().map(|img| img.intrinsics.clone()).collect();

    let patch_config = patchmatch_config_from_quality(quality);
    let mut depth_maps: Vec<DepthMap> = Vec::with_capacity(images.len());

    for (i, image) in images.iter().enumerate() {
        let src_indices = select_source_indices(i, images.len(), max_source_views(quality));
        let src_images: Vec<&GrayImage> =
            src_indices.iter().map(|&idx| &images[idx].gray).collect();
        let src_poses: Vec<&CameraPose> = src_indices.iter().map(|&idx| &poses[idx]).collect();
        let src_intrinsics: Vec<&CameraIntrinsics> = src_indices
            .iter()
            .map(|&idx| &adjusted_intrinsics[idx])
            .collect();

        let input = MvsInput {
            ref_image: &image.gray,
            ref_pose: &poses[i],
            ref_intrinsics: &adjusted_intrinsics[i],
            src_images,
            src_poses,
            src_intrinsics,
        };

        let depth_map = patchmatch_stereo(&input, &patch_config);
        depth_maps.push(depth_map);
    }

    let consistency = consistency_threshold_for_quality(quality);
    let min_views = min_views_for_quality(quality);
    Ok(fuse_depth_maps_colored(
        &depth_maps,
        &images,
        poses,
        &adjusted_intrinsics,
        consistency,
        min_views,
    ))
}

struct MvsImage {
    rgb: RgbImage,
    gray: GrayImage,
    intrinsics: CameraIntrinsics,
}

fn load_mvs_images(
    image_paths: &[PathBuf],
    intrinsics: &[CameraIntrinsics],
    scale: f32,
) -> Result<Vec<MvsImage>> {
    let mut images = Vec::with_capacity(image_paths.len());
    for (idx, path) in image_paths.iter().enumerate() {
        let img = image::open(path)
            .with_context(|| format!("Failed to open image {}", path.display()))?;
        let (rgb, gray) = downscale_image_pair(&img, scale);
        let intr = scale_intrinsics(&intrinsics[idx], scale, rgb.width(), rgb.height());
        images.push(MvsImage {
            rgb,
            gray,
            intrinsics: intr,
        });
    }
    Ok(images)
}

fn downscale_image_pair(img: &DynamicImage, scale: f32) -> (RgbImage, GrayImage) {
    let rgb = img.to_rgb8();
    let gray = img.to_luma8();
    if (scale - 1.0).abs() <= f32::EPSILON {
        return (rgb, gray);
    }
    let width = (rgb.width() as f32 * scale).round().max(1.0) as u32;
    let height = (rgb.height() as f32 * scale).round().max(1.0) as u32;
    let rgb_resized = image::imageops::resize(&rgb, width, height, FilterType::Lanczos3);
    let gray_resized = image::imageops::resize(&gray, width, height, FilterType::Lanczos3);
    (rgb_resized, gray_resized)
}

fn scale_intrinsics(
    intrinsics: &CameraIntrinsics,
    scale: f32,
    width: u32,
    height: u32,
) -> CameraIntrinsics {
    CameraIntrinsics {
        fx: intrinsics.fx * scale as f64,
        fy: intrinsics.fy * scale as f64,
        cx: intrinsics.cx * scale as f64,
        cy: intrinsics.cy * scale as f64,
        width,
        height,
        distortion: intrinsics.distortion.clone(),
        distortion_model: intrinsics.distortion_model,
    }
}

fn dense_scale_for_quality(quality: Quality) -> f32 {
    match quality {
        Quality::Low => 0.5,
        Quality::Medium => 0.75,
        Quality::High => 1.0,
        Quality::Ultra => 1.0,
    }
}

fn patchmatch_config_from_quality(quality: Quality) -> PatchMatchConfig {
    match quality {
        Quality::Low => PatchMatchConfig {
            patch_radius: 3,
            num_iterations: 2,
            num_samples: 6,
            ncc_threshold: 0.55,
            ..Default::default()
        },
        Quality::Medium => PatchMatchConfig {
            patch_radius: 5,
            num_iterations: 3,
            num_samples: 8,
            ncc_threshold: 0.6,
            ..Default::default()
        },
        Quality::High => PatchMatchConfig {
            patch_radius: 7,
            num_iterations: 4,
            num_samples: 10,
            ncc_threshold: 0.65,
            ..Default::default()
        },
        Quality::Ultra => PatchMatchConfig {
            patch_radius: 7,
            num_iterations: 5,
            num_samples: 12,
            ncc_threshold: 0.7,
            ..Default::default()
        },
    }
}

fn max_source_views(quality: Quality) -> usize {
    match quality {
        Quality::Low => 4,
        Quality::Medium => 6,
        Quality::High => 10,
        Quality::Ultra => 12,
    }
}

fn select_source_indices(index: usize, total: usize, max_sources: usize) -> Vec<usize> {
    let mut indices = Vec::new();
    let mut offset = 1;
    while indices.len() < max_sources && offset < total {
        if index >= offset {
            indices.push(index - offset);
        }
        if indices.len() >= max_sources {
            break;
        }
        if index + offset < total {
            indices.push(index + offset);
        }
        offset += 1;
    }
    indices
}

fn consistency_threshold_for_quality(quality: Quality) -> f32 {
    match quality {
        Quality::Low => 0.02,
        Quality::Medium => 0.015,
        Quality::High => 0.01,
        Quality::Ultra => 0.008,
    }
}

fn min_views_for_quality(quality: Quality) -> usize {
    match quality {
        Quality::Low => 2,
        Quality::Medium => 2,
        Quality::High => 3,
        Quality::Ultra => 3,
    }
}

fn fuse_depth_maps_colored(
    depth_maps: &[DepthMap],
    images: &[MvsImage],
    poses: &[CameraPose],
    intrinsics: &[CameraIntrinsics],
    consistency_threshold: f32,
    min_views: usize,
) -> Vec<(na::Point3<f64>, [u8; 3])> {
    let mut points = Vec::new();
    if depth_maps.is_empty() {
        return points;
    }

    for (ref_idx, depth_map) in depth_maps.iter().enumerate() {
        let intr = &intrinsics[ref_idx];
        let pose = &poses[ref_idx];
        let rgb = &images[ref_idx].rgb;

        for y in 0..depth_map.height {
            for x in 0..depth_map.width {
                let (depth, confidence, _normal) = depth_map.get(x, y).unwrap();
                if depth <= 0.0 || confidence < 0.5 {
                    continue;
                }

                let point_world = unproject_to_world(x, y, depth, intr, pose);

                let mut consistent_views = 1;
                for (src_idx, src_depth_map) in depth_maps.iter().enumerate() {
                    if src_idx == ref_idx {
                        continue;
                    }
                    let src_pose = &poses[src_idx];
                    let src_intr = &intrinsics[src_idx];
                    if let Some((sx, sy, sz)) = project_to_camera(&point_world, src_pose, src_intr)
                    {
                        if let Some((src_depth, src_conf, _)) = src_depth_map.get(sx, sy) {
                            if src_conf > 0.5 {
                                let depth_diff = (src_depth as f64 - sz).abs();
                                if depth_diff < consistency_threshold as f64 {
                                    consistent_views += 1;
                                }
                            }
                        }
                    }
                }

                if consistent_views >= min_views {
                    let pixel = rgb.get_pixel(x.min(rgb.width() - 1), y.min(rgb.height() - 1));
                    points.push((point_world, [pixel[0], pixel[1], pixel[2]]));
                }
            }
        }
    }

    points
}

fn unproject_to_world(
    x: u32,
    y: u32,
    depth: f32,
    intrinsics: &CameraIntrinsics,
    pose: &CameraPose,
) -> na::Point3<f64> {
    let x3d = (x as f64 - intrinsics.cx) * depth as f64 / intrinsics.fx;
    let y3d = (y as f64 - intrinsics.cy) * depth as f64 / intrinsics.fy;
    let point_cam = na::Point3::new(x3d, y3d, depth as f64);
    let rotation = pose.rotation.to_rotation_matrix();
    na::Point3::from(rotation * point_cam.coords + pose.translation)
}

fn project_to_camera(
    point_world: &na::Point3<f64>,
    pose: &CameraPose,
    intrinsics: &CameraIntrinsics,
) -> Option<(u32, u32, f64)> {
    let rotation = pose.rotation.to_rotation_matrix();
    let point_cam = rotation.inverse() * (point_world.coords - pose.translation);
    if point_cam.z <= 0.0 {
        return None;
    }
    let px = (intrinsics.fx * point_cam.x / point_cam.z + intrinsics.cx).round();
    let py = (intrinsics.fy * point_cam.y / point_cam.z + intrinsics.cy).round();
    if px < 0.0 || py < 0.0 {
        return None;
    }
    let (px, py) = (px as u32, py as u32);
    if px < intrinsics.width && py < intrinsics.height {
        Some((px, py, point_cam.z))
    } else {
        None
    }
}

fn export_dense_point_cloud(points: &[(na::Point3<f64>, [u8; 3])], path: &Path) -> Result<()> {
    let mut positions = Vec::with_capacity(points.len());
    let mut colors = Vec::with_capacity(points.len());
    for (p, c) in points {
        positions.push(na::Point3::new(p.x as f32, p.y as f32, p.z as f32));
        colors.push(*c);
    }
    export_point_cloud_ply(&positions, Some(&colors), None, path)?;
    Ok(())
}

fn write_intrinsics_report(output: &Path, reports: &[(String, IntrinsicsReport)]) -> Result<()> {
    if reports.is_empty() {
        return Ok(());
    }

    let mut counts = std::collections::HashMap::new();
    for (_, report) in reports {
        *counts.entry(report.source).or_insert(0usize) += 1;
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let json = serde_json::json!({
        "generated_at_unix": ts,
        "counts": {
            "focal_plane": counts.get(&IntrinsicsSource::FocalPlane).cloned().unwrap_or(0),
            "focal_length_35mm": counts.get(&IntrinsicsSource::FocalLength35mm).cloned().unwrap_or(0),
            "heuristic": counts.get(&IntrinsicsSource::Heuristic).cloned().unwrap_or(0),
            "calibration": counts.get(&IntrinsicsSource::Calibration).cloned().unwrap_or(0),
        },
        "images": reports.iter().map(|(path, report)| {
            serde_json::json!({
                "path": path,
                "source": report.source,
                "fx": report.fx,
                "fy": report.fy,
                "cx": report.cx,
                "cy": report.cy,
                "width": report.width,
                "height": report.height,
                "focal_length_mm": report.focal_length_mm,
                "focal_length_35mm": report.focal_length_35mm,
                "rms_error": report.rms_error,
            })
        }).collect::<Vec<_>>()
    });

    let report_path = output.join("intrinsics_report.json");
    std::fs::write(report_path, serde_json::to_string_pretty(&json)?)?;
    Ok(())
}

fn write_reconstruction_report(
    output: &Path,
    mode: Mode,
    quality: Quality,
    image_paths: &[PathBuf],
    reconstruction: &trueshot_core::reconstruction::multicam_sfm::SparseReconstruction,
    dense_points: &[(na::Point3<f64>, [u8; 3])],
    reprojection_stats: Option<&ReprojectionStats>,
    inventory_ctx: Option<&InventoryContext>,
) -> Result<()> {
    let point_errors: Vec<f64> = reconstruction
        .points
        .iter()
        .map(|p| p.error)
        .filter(|e| e.is_finite())
        .collect();
    let mean_error = mean(&point_errors).unwrap_or(0.0);
    let median_error = median(point_errors).unwrap_or(0.0);

    let sparse_bbox = bbox_from_points(reconstruction.points.iter().map(|p| &p.position));
    let dense_bbox = bbox_from_dense(dense_points);

    let outputs = serde_json::json!({
        "sparse_ply": output.join("sparse.ply").to_string_lossy(),
        "dense_ply": output.join("dense.ply").to_string_lossy(),
        "gaussians_ply": output.join("gaussians.ply").to_string_lossy(),
    });

    let json = serde_json::json!({
        "mode": mode.to_string(),
        "quality": format!("{:?}", quality),
        "image_count": image_paths.len(),
        "sparse_points": reconstruction.points.len(),
        "dense_points": dense_points.len(),
        "mean_point_error": mean_error,
        "median_point_error": median_error,
        "sparse_bbox": sparse_bbox,
        "dense_bbox": dense_bbox,
        "inventory": inventory_ctx.map(|ctx| serde_json::json!({
            "model_id": ctx.model_id.to_string(),
            "sequence_id": ctx.sequence_id.to_string(),
            "model_name": ctx.model_name,
        })),
        "reprojection_stats": reprojection_stats.map(|stats| serde_json::json!({
            "points": stats.points,
            "observations": stats.observations,
            "invalid_observations": stats.invalid_observations,
            "mean_error_px": stats.mean_error_px,
            "median_error_px": stats.median_error_px,
            "p90_error_px": stats.p90_error_px,
            "max_error_px": stats.max_error_px,
            "mean_track_len": stats.mean_track_len,
            "median_track_len": stats.median_track_len,
            "min_track_len": stats.min_track_len,
            "max_track_len": stats.max_track_len,
            "points_with_2plus": stats.points_with_2plus,
            "points_with_3plus": stats.points_with_3plus,
        })),
        "outputs": outputs,
    });

    let report_path = output.join("reconstruction_report.json");
    std::fs::write(report_path, serde_json::to_string_pretty(&json)?)?;
    Ok(())
}

fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        Some((values[mid - 1] + values[mid]) * 0.5)
    } else {
        Some(values[mid])
    }
}

fn bbox_from_points<'a, I>(points: I) -> Option<serde_json::Value>
where
    I: Iterator<Item = &'a na::Point3<f64>>,
{
    let mut min = na::Vector3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = na::Vector3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut count = 0usize;

    for p in points {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        min.z = min.z.min(p.z);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
        max.z = max.z.max(p.z);
        count += 1;
    }

    if count == 0 {
        return None;
    }

    Some(serde_json::json!({
        "min": [min.x, min.y, min.z],
        "max": [max.x, max.y, max.z],
    }))
}

fn bbox_from_dense(points: &[(na::Point3<f64>, [u8; 3])]) -> Option<serde_json::Value> {
    bbox_from_points(points.iter().map(|(p, _)| p))
}

fn run_gaussian_splatting(
    output: &Path,
    image_paths: &[PathBuf],
    poses: &[CameraPose],
    intrinsics: &[CameraIntrinsics],
    quality: Quality,
    dense_points: &[(na::Point3<f64>, [u8; 3])],
    sparse_points: &[trueshot_core::reconstruction::multicam_sfm::Point3D],
) -> Result<()> {
    let mut initial_points: Vec<(na::Point3<f32>, [u8; 3])> = Vec::new();
    if !dense_points.is_empty() {
        for (p, c) in dense_points {
            initial_points.push((na::Point3::new(p.x as f32, p.y as f32, p.z as f32), *c));
        }
    } else {
        for p in sparse_points {
            initial_points.push((
                na::Point3::new(
                    p.position.x as f32,
                    p.position.y as f32,
                    p.position.z as f32,
                ),
                p.color,
            ));
        }
    }

    if initial_points.is_empty() {
        anyhow::bail!("No points available to initialize Gaussian splatting");
    }

    let mut cameras = Vec::with_capacity(image_paths.len());
    for (idx, path) in image_paths.iter().enumerate() {
        let pose = &poses[idx];
        let intr = &intrinsics[idx];
        let rotation = pose.rotation.to_rotation_matrix();
        let mut transform = na::Matrix4::<f32>::identity();
        transform
            .fixed_view_mut::<3, 3>(0, 0)
            .copy_from(&rotation.matrix().map(|v| v as f32));
        transform[(0, 3)] = pose.translation.x as f32;
        transform[(1, 3)] = pose.translation.y as f32;
        transform[(2, 3)] = pose.translation.z as f32;

        let intr_matrix = na::Matrix3::<f32>::new(
            intr.fx as f32,
            0.0,
            intr.cx as f32,
            0.0,
            intr.fy as f32,
            intr.cy as f32,
            0.0,
            0.0,
            1.0,
        );

        cameras.push(GsCamera {
            transform,
            intrinsics: intr_matrix,
            width: intr.width,
            height: intr.height,
            image_path: path.clone(),
        });
    }

    let config = training_config_from_quality(quality);
    let mut trainer = GaussianSplatTrainer::new(&initial_points, cameras, config.clone());

    let pb = ProgressBar::new(config.iterations as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    for _ in 0..config.iterations {
        let loss = trainer.step()?;
        pb.inc(1);
        if trainer.iteration() % 50 == 0 {
            pb.set_message(format!("loss {:.4}", loss));
        }
    }
    pb.finish_with_message("3DGS training complete");

    let output_path = output.join("gaussians.ply");
    trainer.export_ply(&output_path)?;
    Ok(())
}

fn training_config_from_quality(quality: Quality) -> TrainingConfig {
    TrainingConfig {
        iterations: match quality {
            Quality::Low => 1000,
            Quality::Medium => 3000,
            Quality::High => 8000,
            Quality::Ultra => 20000,
        },
        ..TrainingConfig::default()
    }
}

fn config_file_path() -> PathBuf {
    if let Ok(path) = env::var("TRUESHOT_CONFIG_PATH") {
        return PathBuf::from(path);
    }
    PathBuf::from("config.toml")
}

fn ensure_default_config(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    std::fs::write(path, DEFAULT_CONFIG).context("Failed to write default config")?;
    Ok(())
}

fn load_config_doc() -> Result<TomlValue> {
    let path = config_file_path();
    if path.exists() {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config {}", path.display()))?;
        let doc = toml::from_str(&content).context("Failed to parse config TOML")?;
        Ok(doc)
    } else {
        let doc = toml::from_str(DEFAULT_CONFIG).context("Failed to parse default config")?;
        Ok(doc)
    }
}

fn save_config_doc(doc: &TomlValue) -> Result<()> {
    let path = config_file_path();
    let rendered = toml::to_string_pretty(doc).context("Failed to render config")?;
    std::fs::write(&path, rendered)
        .with_context(|| format!("Failed to write config {}", path.display()))?;
    Ok(())
}

fn parse_toml_value(raw: &str) -> TomlValue {
    let wrapped = format!("value = {}", raw);
    let parsed_value = toml::from_str::<TomlValue>(&wrapped)
        .ok()
        .and_then(|parsed| match parsed {
            TomlValue::Table(map) => map.get("value").cloned(),
            _ => None,
        });
    if let Some(value) = parsed_value {
        return value;
    }
    TomlValue::String(raw.to_string())
}

fn set_toml_value(root: &mut TomlValue, key: &str, value: TomlValue) -> Result<()> {
    let parts: Vec<&str> = key.split('.').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        anyhow::bail!("Invalid config key");
    }
    let mut current = root
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("Config root must be a table"))?;
    for part in &parts[..parts.len() - 1] {
        let entry = current
            .entry(part.to_string())
            .or_insert_with(|| TomlValue::Table(toml::map::Map::new()));
        current = entry
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("Config path {} is not a table", part))?;
    }
    current.insert(parts[parts.len() - 1].to_string(), value);
    Ok(())
}

fn load_cli_config() -> Result<CliConfig> {
    let cfg = Config::builder()
        .set_default("server.host", "127.0.0.1")?
        .set_default("server.port", 3000)?
        .set_default("paths.projects_dir", "./projects")?
        .set_default("paths.inventory_db", "./inventory.redb")?
        .add_source(File::with_name("config").required(false))
        .add_source(config::Environment::with_prefix("TRUESHOT").separator("__"))
        .build()
        .context("Failed to load configuration")?;
    Ok(cfg.try_deserialize()?)
}

fn resolve_config_path(path: &PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path.clone();
    }
    let cfg_path = config_file_path();
    let base = cfg_path.parent().unwrap_or_else(|| Path::new("."));
    base.join(path)
}

fn load_inventory() -> Result<Inventory> {
    let cfg = load_cli_config()?;
    let inventory_path = resolve_config_path(&cfg.paths.inventory_db);
    Inventory::new(&inventory_path)
        .with_context(|| format!("Failed to open inventory at {}", inventory_path.display()))
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn portable_asset_filename(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Export asset path has no portable UTF-8 filename: {}",
                path.display()
            )
        })
}

fn confirm_delete(model_id: &str) -> Result<bool> {
    print!("Type DELETE to confirm deletion of {}: ", model_id);
    io::stdout().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim() == "DELETE")
}

fn create_inventory_context(
    input: &Path,
    output: &Path,
    mode: Mode,
    quality: Quality,
) -> Result<InventoryContext> {
    let inventory = load_inventory()?;
    let model_name = output
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("TrueShot Model")
        .to_string();
    let description = format!(
        "CLI process from {} (mode={}, quality={:?})",
        input.display(),
        mode,
        quality
    );
    let model = inventory.create_model(&model_name, &description)?;
    let sequence = inventory.create_sequence(model.id, "CLI Scan")?;
    Ok(InventoryContext {
        model_id: model.id,
        sequence_id: sequence.id,
        model_name,
    })
}

fn update_inventory_sequence(ctx: &InventoryContext, output: &Path, status: SequenceStatus) {
    let inventory = match load_inventory() {
        Ok(inv) => inv,
        Err(err) => {
            eprintln!("{} Inventory update failed: {}", WARNING, err);
            return;
        }
    };

    let folder = output.display().to_string();
    if let Err(err) = inventory.update_sequence_folder(&ctx.sequence_id, &folder) {
        eprintln!(
            "{} Inventory sequence folder update failed: {}",
            WARNING, err
        );
    }
    if let Err(err) = inventory.update_sequence_status(&ctx.sequence_id, status) {
        eprintln!(
            "{} Inventory sequence status update failed: {}",
            WARNING, err
        );
    }
    if let Err(err) = inventory.touch_model(&ctx.model_id) {
        eprintln!("{} Inventory model update failed: {}", WARNING, err);
    }
}

fn write_inventory_manifest(output: &Path, ctx: &InventoryContext, input: &Path) -> Result<()> {
    let json = serde_json::json!({
        "model_id": ctx.model_id.to_string(),
        "sequence_id": ctx.sequence_id.to_string(),
        "model_name": ctx.model_name,
        "input": input.display().to_string(),
        "output": output.display().to_string(),
    });
    let path = output.join("inventory.json");
    std::fs::write(path, serde_json::to_string_pretty(&json)?)?;
    Ok(())
}

enum RunReportKind {
    Process {
        mode: Mode,
        quality: Quality,
        input: PathBuf,
        output: PathBuf,
        jobs: Option<usize>,
        gpu_disabled: bool,
    },
    Export {
        input: PathBuf,
        output: PathBuf,
        format: ExportFormat,
        include_colors: bool,
        include_normals: bool,
    },
    Calibrate {
        images: Vec<PathBuf>,
        output: PathBuf,
        rows: u32,
        cols: u32,
        square_size_mm: f32,
        rms_error: f64,
        width: u32,
        height: u32,
    },
}

fn now_rfc3339(at: std::time::SystemTime) -> String {
    DateTime::<Utc>::from(at).to_rfc3339()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RunStepState {
    name: String,
    status: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    artifacts: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RunState {
    run_id: String,
    status: String,
    mode: String,
    quality: String,
    input: String,
    output: String,
    started_at: String,
    updated_at: String,
    steps: Vec<RunStepState>,
}

struct RunStateManager {
    path: PathBuf,
    state: RunState,
}

impl RunStateManager {
    fn load_or_init(input: &Path, output: &Path, mode: Mode, quality: Quality) -> Result<Self> {
        let path = run_state_path(output);
        if path.exists() {
            if let Ok(raw) = std::fs::read_to_string(&path) {
                if let Ok(mut state) = serde_json::from_str::<RunState>(&raw) {
                    if state.input == input.display().to_string()
                        && state.output == output.display().to_string()
                        && state.mode == mode.to_string()
                        && state.quality == format!("{:?}", quality)
                    {
                        let now = now_rfc3339(std::time::SystemTime::now());
                        if state.status != "completed" {
                            state.status = "in_progress".to_string();
                        }
                        state.updated_at = now;
                        merge_steps(&mut state.steps, &default_reconstruction_steps());
                        let manager = Self { path, state };
                        manager.persist()?;
                        return Ok(manager);
                    } else {
                        let archived = path
                            .with_extension(format!("json.bak.{}", chrono::Utc::now().timestamp()));
                        let _ = std::fs::rename(&path, archived);
                    }
                }
            }
        }

        let now = now_rfc3339(std::time::SystemTime::now());
        let state = RunState {
            run_id: uuid::Uuid::new_v4().to_string(),
            status: "in_progress".to_string(),
            mode: mode.to_string(),
            quality: format!("{:?}", quality),
            input: input.display().to_string(),
            output: output.display().to_string(),
            started_at: now.clone(),
            updated_at: now,
            steps: default_reconstruction_steps(),
        };
        let manager = Self { path, state };
        manager.persist()?;
        Ok(manager)
    }

    fn persist(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let payload = serde_json::to_string_pretty(&self.state)?;
        std::fs::write(&tmp, payload)?;
        std::fs::rename(tmp, &self.path)?;
        Ok(())
    }

    fn mark_step_started(&mut self, name: &str) {
        let now = now_rfc3339(std::time::SystemTime::now());
        if let Some(step) = self.state.steps.iter_mut().find(|s| s.name == name) {
            step.status = "in_progress".to_string();
            step.started_at.get_or_insert_with(|| now.clone());
            step.finished_at = None;
        }
        self.state.updated_at = now;
        let _ = self.persist();
    }

    fn mark_step_completed(&mut self, name: &str, artifacts: Vec<String>) {
        let now = now_rfc3339(std::time::SystemTime::now());
        if let Some(step) = self.state.steps.iter_mut().find(|s| s.name == name) {
            step.status = "completed".to_string();
            step.finished_at = Some(now.clone());
            if !artifacts.is_empty() {
                step.artifacts = artifacts;
            }
        }
        self.state.updated_at = now;
        let _ = self.persist();
    }

    fn mark_failed(&mut self) {
        let now = now_rfc3339(std::time::SystemTime::now());
        self.state.status = "failed".to_string();
        self.state.updated_at = now;
        let _ = self.persist();
    }

    fn mark_completed(&mut self) {
        let now = now_rfc3339(std::time::SystemTime::now());
        self.state.status = "completed".to_string();
        self.state.updated_at = now;
        let _ = self.persist();
    }

    fn should_skip(&self, name: &str) -> bool {
        self.state
            .steps
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.status == "completed")
            .unwrap_or(false)
    }
}

fn run_state_path(output: &Path) -> PathBuf {
    output.join(".trueshot").join("run_state.json")
}

fn default_reconstruction_steps() -> Vec<RunStepState> {
    let names = ["scan_input", "sfm", "dense", "gaussian", "reports"];
    names
        .iter()
        .map(|name| RunStepState {
            name: name.to_string(),
            status: "pending".to_string(),
            started_at: None,
            finished_at: None,
            artifacts: Vec::new(),
        })
        .collect()
}

fn merge_steps(existing: &mut Vec<RunStepState>, defaults: &[RunStepState]) {
    for step in defaults {
        if !existing.iter().any(|s| s.name == step.name) {
            existing.push(step.clone());
        }
    }
}

fn collect_artifacts(output: &Path, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .filter_map(|name| {
            let path = output.join(name);
            if path.exists() {
                Some(path.to_string_lossy().to_string())
            } else {
                None
            }
        })
        .collect()
}

#[derive(Debug, Serialize, Deserialize)]
struct DensePointRecord {
    position: [f64; 3],
    color: [u8; 3],
}

fn checkpoint_dir(output: &Path) -> PathBuf {
    output.join(".trueshot").join("checkpoints")
}

fn sparse_checkpoint_path(output: &Path) -> PathBuf {
    checkpoint_dir(output).join("sparse_reconstruction.json")
}

fn dense_checkpoint_path(output: &Path) -> PathBuf {
    checkpoint_dir(output).join("dense_points.zst")
}

fn save_sparse_checkpoint(
    output: &Path,
    reconstruction: &trueshot_core::reconstruction::multicam_sfm::SparseReconstruction,
) -> Result<()> {
    let path = sparse_checkpoint_path(output);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(reconstruction)?;
    std::fs::write(path, json)?;
    Ok(())
}

fn load_sparse_checkpoint(
    output: &Path,
) -> Result<trueshot_core::reconstruction::multicam_sfm::SparseReconstruction> {
    let path = sparse_checkpoint_path(output);
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("Missing sparse checkpoint at {}", path.display()))?;
    let recon = serde_json::from_str(&data)?;
    Ok(recon)
}

fn save_dense_checkpoint(output: &Path, points: &[(na::Point3<f64>, [u8; 3])]) -> Result<()> {
    let path = dense_checkpoint_path(output);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let records: Vec<DensePointRecord> = points
        .iter()
        .map(|(p, c)| DensePointRecord {
            position: [p.x, p.y, p.z],
            color: *c,
        })
        .collect();
    let encoded = bincode::serialize(&records)?;
    let compressed = zstd::encode_all(encoded.as_slice(), 3)?;
    std::fs::write(path, compressed)?;
    Ok(())
}

fn load_dense_checkpoint(output: &Path) -> Result<Vec<(na::Point3<f64>, [u8; 3])>> {
    let path = dense_checkpoint_path(output);
    let bytes = std::fs::read(&path)
        .with_context(|| format!("Missing dense checkpoint at {}", path.display()))?;
    let decoded = zstd::decode_all(bytes.as_slice())?;
    let records: Vec<DensePointRecord> = bincode::deserialize(&decoded)?;
    let points = records
        .into_iter()
        .map(|r| {
            (
                na::Point3::new(r.position[0], r.position[1], r.position[2]),
                r.color,
            )
        })
        .collect();
    Ok(points)
}

fn write_run_report(
    path: &Path,
    kind: RunReportKind,
    started_iso: &str,
    started_at: std::time::SystemTime,
    duration_seconds: f64,
    status: &str,
    inventory_ctx: Option<&InventoryContext>,
    telemetry_started: &ProcessTelemetrySnapshot,
) -> Result<()> {
    let finished_at = std::time::SystemTime::now();
    let telemetry_finished = ProcessTelemetrySnapshot::capture();
    let telemetry = ProcessTelemetryWindow::between(telemetry_started, &telemetry_finished);
    let finished_iso = now_rfc3339(finished_at);
    let started_unix = started_at
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let finished_unix = finished_at
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let run_id = uuid::Uuid::new_v4().to_string();
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    let base = serde_json::json!({
        "run_id": run_id,
        "status": status,
        "started_at": started_iso,
        "finished_at": finished_iso,
        "started_at_unix": started_unix,
        "finished_at_unix": finished_unix,
        "duration_seconds": duration_seconds,
        "cli_version": env!("CARGO_PKG_VERSION"),
        "host": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "cores": cores,
        },
        "performance": telemetry,
        "inventory": inventory_ctx.map(|ctx| serde_json::json!({
            "model_id": ctx.model_id.to_string(),
            "sequence_id": ctx.sequence_id.to_string(),
            "model_name": ctx.model_name,
        })),
    });

    let payload = match kind {
        RunReportKind::Process {
            mode,
            quality,
            input,
            output,
            jobs,
            gpu_disabled,
        } => {
            let artifacts = collect_artifacts(
                &output,
                &[
                    "sparse.ply",
                    "dense.ply",
                    "gaussians.ply",
                    "reconstruction_report.json",
                    "intrinsics_report.json",
                    "inventory.json",
                    "run_report.json",
                ],
            );
            serde_json::json!({
                "kind": "process",
                "mode": mode.to_string(),
                "quality": format!("{:?}", quality),
                "input": input.to_string_lossy(),
                "output": output.to_string_lossy(),
                "jobs": jobs,
                "gpu_disabled": gpu_disabled,
                "artifacts": artifacts,
            })
        }
        RunReportKind::Export {
            input,
            output,
            format,
            include_colors,
            include_normals,
        } => {
            serde_json::json!({
                "kind": "export",
                "input": input.to_string_lossy(),
                "output": output.to_string_lossy(),
                "format": format!("{:?}", format),
                "include_colors": include_colors,
                "include_normals": include_normals,
            })
        }
        RunReportKind::Calibrate {
            images,
            output,
            rows,
            cols,
            square_size_mm,
            rms_error,
            width,
            height,
        } => {
            serde_json::json!({
                "kind": "calibrate",
                "images": images.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
                "output": output.to_string_lossy(),
                "rows": rows,
                "cols": cols,
                "square_size_mm": square_size_mm,
                "rms_error": rms_error,
                "width": width,
                "height": height,
            })
        }
    };

    let json = merge_json(base, payload);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, serde_json::to_string_pretty(&json)?)?;
    std::fs::rename(&partial, path)?;
    Ok(())
}

fn merge_json(base: serde_json::Value, payload: serde_json::Value) -> serde_json::Value {
    let mut base_map = match base {
        serde_json::Value::Object(map) => map,
        _ => return payload,
    };
    if let serde_json::Value::Object(payload_map) = payload {
        for (k, v) in payload_map {
            base_map.insert(k, v);
        }
    }
    serde_json::Value::Object(base_map)
}

fn cmd_inventory(action: InventoryAction) -> Result<()> {
    match action {
        InventoryAction::List { tag } => {
            let inventory = load_inventory()?;
            let mut models = inventory.list_models()?;

            if let Some(t) = tag.as_ref() {
                models.retain(|m| {
                    contains_ignore_case(&m.name, t)
                        || contains_ignore_case(&m.description, t)
                        || contains_ignore_case(&m.notes, t)
                });
            }

            models.sort_by_key(|m| m.updated_at);
            models.reverse();

            println!("{} Model Inventory", FOLDER);
            if let Some(t) = tag {
                println!("  Filtering by: {}", style(t).yellow());
            }
            println!();
            if models.is_empty() {
                println!(
                    "  {} No models found. Use 'trueshot process' to create models.",
                    WARNING
                );
                return Ok(());
            }
            for model in models {
                println!(
                    "  {}  {}  {}",
                    style(model.id).cyan(),
                    style(model.name).bold(),
                    style(model.updated_at.to_rfc3339()).dim()
                );
            }
        }
        InventoryAction::Show { id } => {
            let inventory = load_inventory()?;
            let model_id =
                uuid::Uuid::parse_str(&id).with_context(|| format!("Invalid model id: {}", id))?;
            let model = inventory.get_model(&model_id)?;
            let model = match model {
                Some(model) => model,
                None => {
                    println!("{} Model not found: {}", WARNING, id);
                    return Ok(());
                }
            };
            println!("Model: {}", style(&model.id).cyan());
            println!("  Name: {}", style(&model.name).bold());
            println!("  Description: {}", model.description);
            println!("  Notes: {}", model.notes);
            println!("  Created: {}", model.created_at.to_rfc3339());
            println!("  Updated: {}", model.updated_at.to_rfc3339());
            if let Some(path) = &model.thumbnail_path {
                println!("  Thumbnail: {}", path);
            }
            let sequences = inventory.list_sequences_for_model(&model.id)?;
            println!();
            println!("  Sequences: {}", sequences.len());
            for seq in sequences {
                println!(
                    "    {}  {}  {}",
                    style(seq.id).cyan(),
                    seq.name,
                    style(format!("{:?}", seq.status)).dim()
                );
            }
        }
        InventoryAction::Delete { id, force } => {
            let inventory = load_inventory()?;
            let model_id =
                uuid::Uuid::parse_str(&id).with_context(|| format!("Invalid model id: {}", id))?;
            if !force && !confirm_delete(&id)? {
                println!("Aborted.");
                return Ok(());
            }
            let deleted = inventory.delete_model(&model_id)?;
            if deleted {
                println!("{} Deleted model: {}", CHECK, id);
            } else {
                println!("{} Model not found: {}", WARNING, id);
            }
        }
        InventoryAction::Export { output } => {
            let inventory = load_inventory()?;
            let models = inventory.list_models()?;
            let mut sequences = Vec::new();
            for model in &models {
                sequences.extend(inventory.list_sequences_for_model(&model.id)?);
            }
            let machines = inventory.list_machines()?;
            let mut devices = Vec::new();
            for machine in &machines {
                devices.extend(inventory.list_devices_for_machine(&machine.id)?);
            }
            let snapshot = InventorySnapshot {
                models,
                sequences,
                machines,
                devices,
            };
            std::fs::write(&output, serde_json::to_string_pretty(&snapshot)?)
                .with_context(|| format!("Failed to write {}", output.display()))?;
            println!("{} Exported inventory to: {}", CHECK, output.display());
        }
    }
    Ok(())
}

fn cmd_status(hardware: bool, check_updates: bool) -> Result<()> {
    println!("{} TrueShot System Status", ROCKET);
    println!();

    // Version info
    println!("  Version: {}", style(env!("CARGO_PKG_VERSION")).cyan());
    println!();

    // System resources
    println!("  {} System Resources:", style("📊").dim());

    let resources = SystemResources::query();
    println!("    CPU:    {} cores", num_cpus::get());
    println!(
        "    Memory: {:.1} GB total, {:.1} GB available",
        resources.total_memory_gb(),
        resources.available_memory_gb()
    );

    if hardware {
        println!();
        println!("  {} Hardware Details:", GPU);
        // GPU detection would go here
        println!("    GPU:    Detection enabled (use --verbose for details)");
    }

    if check_updates {
        println!();
        println!("  Checking for updates...");
        println!("    {} You are running the latest version!", CHECK);
    }

    Ok(())
}

fn cmd_config(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Show => {
            let cfg = load_cli_config()?;
            let path = config_file_path();
            if path.exists() {
                println!("Config file: {}", path.display());
            } else {
                println!("Config file not found. Showing defaults.");
            }
            println!("Current Configuration:");
            println!("  server.host = {}", cfg.server.host);
            println!("  server.port = {}", cfg.server.port);
            println!(
                "  paths.projects_dir = {}",
                resolve_config_path(&cfg.paths.projects_dir).display()
            );
            println!(
                "  paths.inventory_db = {}",
                resolve_config_path(&cfg.paths.inventory_db).display()
            );
        }
        ConfigAction::Set { key, value } => {
            let path = config_file_path();
            ensure_default_config(&path)?;
            let mut doc = load_config_doc()?;
            let parsed = parse_toml_value(&value);
            set_toml_value(&mut doc, &key, parsed)?;
            save_config_doc(&doc)?;
            println!(
                "{} Updated {} in {}",
                CHECK,
                style(&key).cyan(),
                path.display()
            );
        }
        ConfigAction::Reset => {
            let path = config_file_path();
            std::fs::write(&path, DEFAULT_CONFIG)
                .with_context(|| format!("Failed to write {}", path.display()))?;
            println!(
                "{} Configuration reset to defaults at {}",
                CHECK,
                path.display()
            );
        }
        ConfigAction::Edit => {
            let path = config_file_path();
            ensure_default_config(&path)?;
            let editor = env::var("VISUAL")
                .or_else(|_| env::var("EDITOR"))
                .unwrap_or_else(|_| "vi".to_string());
            let status = Command::new(editor)
                .arg(&path)
                .status()
                .context("Failed to launch editor")?;
            if !status.success() {
                anyhow::bail!("Editor exited with error");
            }
        }
    }
    Ok(())
}

fn cmd_jobs(action: JobsCommand) -> Result<()> {
    match action {
        JobsCommand::Submit(args) => {
            let JobsSubmitArgs {
                kind,
                name,
                request_id,
                payload,
                payload_file,
                workspace,
                livescan,
                dslr,
                job_type,
                webhook_url,
                server,
                api_token,
            } = *args;
            cmd_jobs_submit(
                kind,
                name,
                request_id,
                payload,
                payload_file,
                workspace,
                livescan,
                dslr,
                job_type,
                webhook_url,
                server,
                api_token,
            )
        }
        JobsCommand::List { server, api_token } => cmd_jobs_list(server, api_token),
        JobsCommand::Get {
            id,
            server,
            api_token,
        } => cmd_jobs_get(id, server, api_token),
    }
}

fn cmd_jobs_submit(
    kind: String,
    name: String,
    request_id: Option<String>,
    payload: Option<String>,
    payload_file: Option<PathBuf>,
    workspace: Option<PathBuf>,
    livescan: Option<PathBuf>,
    dslr: Option<PathBuf>,
    job_type: Option<String>,
    webhook_url: Option<String>,
    server: Option<String>,
    api_token: Option<String>,
) -> Result<()> {
    let base_url = resolve_server_url(server)?;
    let token = resolve_api_token(api_token)?;
    let payload = build_job_payload(payload, payload_file, workspace, livescan, dslr, job_type)?;
    let request_id = match request_id {
        Some(value) => {
            Uuid::parse_str(&value).with_context(|| format!("Invalid request id: {}", value))?
        }
        None => Uuid::new_v4(),
    };

    let mut body = serde_json::json!({
        "id": request_id,
        "kind": kind,
        "name": name,
        "payload": payload,
    });
    if let Some(url) = webhook_url {
        if let Some(map) = body.as_object_mut() {
            map.insert("webhook_url".to_string(), serde_json::Value::String(url));
        }
    }

    let client = HttpClient::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;
    let url = format!("{}/api/jobs", base_url);
    let response = client
        .post(url)
        .header("X-API-Token", token)
        .json(&body)
        .send()
        .context("Failed to submit job")?;

    if !response.status().is_success() {
        let text = response
            .text()
            .unwrap_or_else(|_| "Unknown error".to_string());
        anyhow::bail!("Job submission failed: {}", text);
    }

    let json: serde_json::Value = response.json().context("Invalid response JSON")?;
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}

fn cmd_jobs_list(server: Option<String>, api_token: Option<String>) -> Result<()> {
    let base_url = resolve_server_url(server)?;
    let token = resolve_api_token(api_token)?;
    let client = HttpClient::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;
    let url = format!("{}/api/jobs", base_url);
    let response = client
        .get(url)
        .header("X-API-Token", token)
        .send()
        .context("Failed to fetch jobs")?;

    if !response.status().is_success() {
        let text = response
            .text()
            .unwrap_or_else(|_| "Unknown error".to_string());
        anyhow::bail!("Job list failed: {}", text);
    }

    let json: serde_json::Value = response.json().context("Invalid response JSON")?;
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}

fn cmd_jobs_get(id: String, server: Option<String>, api_token: Option<String>) -> Result<()> {
    let base_url = resolve_server_url(server)?;
    let token = resolve_api_token(api_token)?;
    let client = HttpClient::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;
    let url = format!("{}/api/jobs/{}", base_url, id);
    let response = client
        .get(url)
        .header("X-API-Token", token)
        .send()
        .context("Failed to fetch job")?;

    if !response.status().is_success() {
        let text = response
            .text()
            .unwrap_or_else(|_| "Unknown error".to_string());
        anyhow::bail!("Job fetch failed: {}", text);
    }

    let json: serde_json::Value = response.json().context("Invalid response JSON")?;
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}

fn resolve_server_url(server_override: Option<String>) -> Result<String> {
    let raw = match server_override {
        Some(value) => value,
        None => {
            let cfg = load_cli_config()?;
            format!("http://{}:{}", cfg.server.host, cfg.server.port)
        }
    };
    let trimmed = raw.trim_end_matches('/').to_string();
    let normalized = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed
    } else {
        format!("http://{}", trimmed)
    };
    let parsed =
        Url::parse(&normalized).with_context(|| format!("Invalid server URL: {}", normalized))?;
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}

fn resolve_api_token(api_token: Option<String>) -> Result<String> {
    if let Some(token) = api_token {
        if token.trim().is_empty() {
            anyhow::bail!("API token is empty");
        }
        return Ok(token);
    }
    if let Ok(token) = env::var("TRUESHOT_API_TOKEN") {
        if !token.trim().is_empty() {
            return Ok(token);
        }
    }
    anyhow::bail!("API token missing. Provide --api-token or TRUESHOT_API_TOKEN.")
}

fn build_job_payload(
    payload: Option<String>,
    payload_file: Option<PathBuf>,
    workspace: Option<PathBuf>,
    livescan: Option<PathBuf>,
    dslr: Option<PathBuf>,
    job_type: Option<String>,
) -> Result<serde_json::Value> {
    if payload.is_some() && payload_file.is_some() {
        anyhow::bail!("Provide either --payload or --payload-file, not both.");
    }

    if let Some(path) = payload_file {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let json: serde_json::Value =
            serde_json::from_str(&content).context("Payload file must be valid JSON")?;
        return Ok(json);
    }

    if let Some(text) = payload {
        let json: serde_json::Value =
            serde_json::from_str(&text).context("Payload must be valid JSON")?;
        return Ok(json);
    }

    let workspace = workspace
        .ok_or_else(|| anyhow::anyhow!("--workspace is required when no payload is provided"))?;
    Ok(serde_json::json!({
        "workspace_path": workspace,
        "livescan_path": livescan,
        "dslr_path": dslr,
        "job_type": job_type,
    }))
}

fn run_with_tray(port: u16) -> Result<()> {
    let event_loop = tao::event_loop::EventLoop::new();
    let tray_menu = Menu::new();
    let quit_i = MenuItem::new("Quit", true, None);
    tray_menu
        .append_items(&[&quit_i, &PredefinedMenuItem::separator()])
        .unwrap();

    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip(format!("TrueShot Server (port {})", port))
        .build()
        .unwrap();

    println!("{} Tray icon initialized. Running event loop...", CHECK);

    event_loop.run(move |_event, _, control_flow| {
        *control_flow = tao::event_loop::ControlFlow::Wait;
    });
}

#[cfg(test)]
mod burst_pipeline_tests {
    use super::*;

    #[test]
    fn provenance_asset_filename_serializes_as_a_portable_string() {
        let name =
            portable_asset_filename(Path::new("/tmp/capture_detail_source_map.png")).unwrap();
        assert_eq!(
            serde_json::json!({ "detail_source_map": name }),
            serde_json::json!({
                "detail_source_map": "capture_detail_source_map.png"
            })
        );
    }

    #[test]
    fn process_cli_accepts_a_sensor_noise_profile() {
        let cli = Cli::try_parse_from([
            "trueshot",
            "process",
            "--input",
            "capture",
            "--output",
            "out",
            "--mode",
            "burst",
            "--sensor-noise-profile",
            "z9-noise.json",
            "--sensor-correction-profile",
            "z9-correction.json",
            "--lens-psf-profile",
            "z105-f4-psf.json",
            "--fusion-edits",
            "revision.json",
        ])
        .unwrap();
        let Commands::Process {
            sensor_noise_profile,
            sensor_correction_profile,
            lens_psf_profile,
            fusion_edits,
            ..
        } = cli.command
        else {
            panic!("expected process command");
        };
        assert_eq!(sensor_noise_profile, Some(PathBuf::from("z9-noise.json")));
        assert_eq!(
            sensor_correction_profile,
            Some(PathBuf::from("z9-correction.json"))
        );
        assert_eq!(lens_psf_profile, Some(PathBuf::from("z105-f4-psf.json")));
        assert_eq!(fusion_edits, Some(PathBuf::from("revision.json")));
    }

    #[test]
    fn fusion_revision_identity_and_output_are_stable_and_non_destructive() {
        let sequence = trueshot_core::types::Sequence {
            paths: vec![PathBuf::from("capture.nef")],
            meta: trueshot_core::types::Meta {
                focus_steps: 1,
                exposures: vec![0.0],
                shutter_speeds: vec![1.0],
                ref_focus: 0,
                ref_exp: 0.0,
                burst_factor: 1,
                bone_id: "hero/asset".to_string(),
                vantage: "front left".to_string(),
                rot_deg: 15.0,
                cam_mul: [1.0; 4],
            },
        };
        let group_id = "a".repeat(64);
        let edit_digest = "b".repeat(64);
        let base = burst_group_output_path(Path::new("/tmp/out"), &sequence, &group_id, None);
        let edited = burst_group_output_path(
            Path::new("/tmp/out"),
            &sequence,
            &group_id,
            Some(&edit_digest),
        );

        assert_eq!(
            base.file_name().and_then(|name| name.to_str()),
            Some("hero_asset_front_left_015deg_aaaaaaaaaaaa.tiff")
        );
        assert_eq!(
            edited.file_name().and_then(|name| name.to_str()),
            Some("hero_asset_front_left_015deg_aaaaaaaaaaaa_edit_bbbbbbbbbbbb.tiff")
        );
        assert_ne!(fusion_revision_group_id(&group_id, &edit_digest), group_id);
        assert_eq!(
            fusion_revision_group_id(&group_id, &edit_digest),
            fusion_revision_group_id(&group_id, &edit_digest)
        );
    }

    #[test]
    fn lens_psf_calibration_cli_accepts_holdout_gates() {
        let cli = Cli::try_parse_from([
            "trueshot",
            "calibrate-lens-psf",
            "--measurements",
            "measurements.json",
            "--output",
            "lens-psf.json",
            "--maximum-p95-error",
            "0.04",
            "--minimum-error-reduction",
            "0.65",
        ])
        .unwrap();
        let Commands::CalibrateLensPsf {
            measurements,
            output,
            maximum_p95_error,
            minimum_error_reduction,
        } = cli.command
        else {
            panic!("expected calibrate-lens-psf command");
        };
        assert_eq!(measurements, PathBuf::from("measurements.json"));
        assert_eq!(output, PathBuf::from("lens-psf.json"));
        assert_eq!(maximum_p95_error, 0.04);
        assert_eq!(minimum_error_reduction, 0.65);
    }

    #[test]
    fn lens_psf_extraction_cli_binds_plan_root_and_output() {
        let cli = Cli::try_parse_from([
            "trueshot",
            "extract-lens-psf",
            "--plan",
            "retained/plan.json",
            "--capture-root",
            "retained",
            "--output",
            "measurements.json",
        ])
        .unwrap();
        let Commands::ExtractLensPsf {
            plan,
            capture_root,
            output,
        } = cli.command
        else {
            panic!("expected extract-lens-psf command");
        };
        assert_eq!(plan, PathBuf::from("retained/plan.json"));
        assert_eq!(capture_root, PathBuf::from("retained"));
        assert_eq!(output, PathBuf::from("measurements.json"));
        assert_eq!(
            lens_psf_extraction_report_path(&output),
            PathBuf::from("measurements_extraction_report.json")
        );
    }

    #[test]
    fn lens_psf_calibration_publishes_only_after_holdout_and_refuses_overwrite() {
        use trueshot_core::lens_psf::{
            CalibrationSplit, LensPsfMeasurement, LensPsfSourceRecord, LENS_PSF_MEASUREMENTS_SCHEMA,
        };

        let directory = tempfile::tempdir().unwrap();
        let measurements_path = directory.path().join("measurements.json");
        let output_path = directory.path().join("lens-psf.json");
        let nominal_focal = 50.0f32;
        let aperture = 4.0f32;
        let mut measurements = Vec::new();
        for (focus_index, focus) in [0.5f32, 1.0, 2.0].into_iter().enumerate() {
            let effective = nominal_focal * (1.08 - focus_index as f32 * 0.03);
            let pupil_scale = 1.18 - focus_index as f32 * 0.04;
            for radius in [0.0f32, 1.0] {
                for sample in 0..4 {
                    let subject = focus * if sample % 2 == 0 { 1.22 } else { 0.82 };
                    let focused_mm = focus * 1000.0;
                    let subject_mm = subject * 1000.0;
                    let focused_image = effective * focused_mm / (focused_mm - effective);
                    let subject_image = effective * subject_mm / (subject_mm - effective);
                    let radial_scale = 1.0 + radius * radius * 0.22;
                    let observed_mm = nominal_focal / aperture
                        * pupil_scale
                        * (focused_image - subject_image).abs()
                        / subject_image
                        * radial_scale;
                    measurements.push(LensPsfMeasurement {
                        source_sha256: String::new(),
                        split: if sample < 2 {
                            CalibrationSplit::Fit
                        } else {
                            CalibrationSplit::Holdout
                        },
                        focus_distance_m: focus,
                        focus_distance_source: None,
                        focus_distance_uncertainty_m: None,
                        subject_distance_m: subject,
                        field_radius: radius,
                        effective_focal_length_mm: effective,
                        observed_defocus_diameter_px: observed_mm / 0.00435,
                        pixel_pitch_um: 4.35,
                    });
                }
            }
        }
        let sources = measurements
            .iter_mut()
            .enumerate()
            .map(|(index, measurement)| {
                let sha256 = format!("{:064x}", index + 1);
                measurement.source_sha256.clone_from(&sha256);
                LensPsfSourceRecord {
                    path: format!("retained/calibration-{index:04}.nef"),
                    sha256,
                }
            })
            .collect();
        let set = LensPsfMeasurementSet {
            schema: LENS_PSF_MEASUREMENTS_SCHEMA.to_string(),
            camera_make: "NIKON CORPORATION".to_string(),
            camera_model: "NIKON Z 9".to_string(),
            sensor_width: 8256,
            sensor_height: 5504,
            lens_model: "NIKKOR Z 50mm f/1.8 S".to_string(),
            nominal_focal_length_mm: nominal_focal,
            aperture,
            target_id: "iso-12233-depth-target-v1".to_string(),
            measurement_method: "slanted_edge_esf_defocus_diameter_v1".to_string(),
            radius_knots: vec![0.0, 1.0],
            sources,
            measurements,
        };
        std::fs::write(&measurements_path, serde_json::to_vec_pretty(&set).unwrap()).unwrap();

        cmd_calibrate_lens_psf(measurements_path.clone(), output_path.clone(), 0.05, 0.50).unwrap();
        let profile = LensPsfProfile::load_json(&output_path).unwrap();
        assert!(profile.calibration_id.starts_with("sha256:"));
        let report: serde_json::Value = serde_json::from_slice(
            &std::fs::read(output_path.with_extension("report.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(report["profile_published"], true);
        assert_eq!(report["evaluation"]["passed"], true);
        assert!(cmd_calibrate_lens_psf(measurements_path, output_path, 0.05, 0.50).is_err());
    }

    #[test]
    fn process_cli_accepts_physical_glare_controls() {
        let cli = Cli::try_parse_from([
            "trueshot",
            "process",
            "--input",
            "capture",
            "--output",
            "out",
            "--mode",
            "burst",
            "--glare-spread-um",
            "63.5",
            "--no-glare-focus",
            "--no-frequency-deghost",
        ])
        .unwrap();
        let Commands::Process {
            glare_spread_um,
            no_glare_focus,
            no_frequency_deghost,
            ..
        } = cli.command
        else {
            panic!("expected process command");
        };
        assert_eq!(glare_spread_um, 63.5);
        assert!(no_glare_focus);
        assert!(no_frequency_deghost);
    }

    #[test]
    fn noise_calibration_cli_accepts_repeated_flat_levels() {
        let cli = Cli::try_parse_from([
            "trueshot",
            "calibrate-noise",
            "--dark",
            "dark",
            "--flat-level",
            "flat-01",
            "--flat-level",
            "flat-02",
            "--flat-level",
            "flat-03",
            "--flat-level",
            "flat-04",
            "--flat-level",
            "flat-05",
            "--output",
            "z9-noise.json",
        ])
        .unwrap();
        let Commands::CalibrateNoise {
            dark,
            flat_levels,
            output,
            ..
        } = cli.command
        else {
            panic!("expected calibrate-noise command");
        };
        assert_eq!(dark, PathBuf::from("dark"));
        assert_eq!(flat_levels.len(), 5);
        assert_eq!(output, PathBuf::from("z9-noise.json"));
    }

    #[test]
    fn noise_calibration_report_stays_beside_profile() {
        assert_eq!(
            calibration_report_path(Path::new("/tmp/z9.production-noise.json")),
            PathBuf::from("/tmp/z9.production-noise_calibration_report.json")
        );
        assert_eq!(
            spatial_correction_profile_path(Path::new("/tmp/z9.production-noise.json")),
            PathBuf::from("/tmp/z9.production-noise_spatial_correction.json")
        );
    }

    #[test]
    fn run_report_is_atomically_published_with_native_telemetry() {
        let directory = tempfile::tempdir().unwrap();
        let report_path = directory.path().join("run_report.json");
        let started_at = std::time::SystemTime::now();
        let telemetry = ProcessTelemetrySnapshot::capture();
        write_run_report(
            &report_path,
            RunReportKind::Process {
                mode: Mode::Burst,
                quality: Quality::Ultra,
                input: PathBuf::from("input"),
                output: directory.path().to_path_buf(),
                jobs: Some(2),
                gpu_disabled: false,
            },
            &now_rfc3339(started_at),
            started_at,
            0.25,
            "success",
            None,
            &telemetry,
        )
        .unwrap();

        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
        assert_eq!(
            report["performance"]["schema"],
            "trueshot.process-telemetry.v1"
        );
        assert_eq!(report["kind"], "process");
        assert!(!report_path.with_extension("json.partial").exists());
    }

    #[test]
    fn export_worker_isolates_job_panics_and_continues() {
        let (sender, receiver, worker) = burst_export_worker().unwrap();
        sender
            .send(BurstExportTask {
                group_id: "a".repeat(64),
                label: "panic".to_string(),
                started_at: std::time::Instant::now(),
                decode_seconds: 0.1,
                decoded_megapixels: 1.0,
                major_page_faults: 0,
                export: Box::new(|| panic!("synthetic exporter panic")),
            })
            .unwrap();
        let failed = receiver.recv().unwrap();
        assert!(failed.result.unwrap_err().to_string().contains("synthetic"));

        sender
            .send(BurstExportTask {
                group_id: "b".repeat(64),
                label: "healthy".to_string(),
                started_at: std::time::Instant::now(),
                decode_seconds: 0.1,
                decoded_megapixels: 1.0,
                major_page_faults: 0,
                export: Box::new(|| Ok(Vec::new())),
            })
            .unwrap();
        assert!(receiver.recv().unwrap().result.is_ok());
        drop(sender);
        worker.join().unwrap();
    }
}
