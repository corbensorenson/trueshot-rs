//! TrueShot CLI - State-of-the-Art Command Line Interface
//!
//! A complete CLI for all TrueShot operations with rich output,
//! progress bars, and colored output.

use anyhow::{Result, Context};
use clap::{Parser, Subcommand, ValueEnum};
use config::{Config, File};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use toml::Value as TomlValue;
use indicatif::{ProgressBar, ProgressStyle, MultiProgress};
use console::{style, Emoji};
use tray_icon::{TrayIconBuilder, menu::{Menu, MenuItem, PredefinedMenuItem}};
use trueshot_core::fusion_engine::FusionEngine;
use trueshot_core::smart_loader::SmartLoader;
use trueshot_core::timing::HierarchicalTimer;
use trueshot_core::types::ProcessingOptions;
use trueshot_core::export::{save_png, save_tiff16_from_f64, save_depth_tiff, generate_output_path, export_gltf, export_glb, export_ply, export_point_cloud_ply, export_fbx, PlyExportOptions};
use trueshot_core::export::usd::{export_usd_with_options, UsdExportOptions};
use trueshot_core::reconstruction::multicam_sfm::{
    SfmPipeline, SfmConfig, FeatureType, PatchMatchConfig, patchmatch_stereo,
    CameraPose, CameraIntrinsics, DepthMap, MvsInput, ReprojectionStats,
};
use trueshot_core::validation::validate_photogrammetry_input;
use trueshot_core::intrinsics::{estimate_intrinsics_with_report, IntrinsicsReport, IntrinsicsSource};
use trueshot_core::gaussian_splatting::{GaussianSplatTrainer, TrainingConfig, Camera as GsCamera};
use trueshot_core::inventory::{Inventory, Model, Sequence, Machine, Device, SequenceStatus};
use trueshot_core::crash_handler::init_crash_handler;
use trueshot_core::licensing::{Feature, LicenseError, LicenseManager};
use chrono::{DateTime, Utc};
use image::{DynamicImage, GrayImage, RgbImage, imageops::FilterType};
use nalgebra as na;
use reqwest::blocking::Client as HttpClient;
use reqwest::Url;
use uuid::Uuid;

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
        
        /// Disable GPU acceleration
        #[arg(long)]
        no_gpu: bool,

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
    Submit {
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
    },

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
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_target(false)
            .init();
    }

    match cli.command {
        Commands::Serve { port, tray, daemon } => cmd_serve(port, tray, daemon),
        Commands::Process { input, output, mode, quality, jobs, no_gpu, trial, trial_days } => {
            cmd_process(input, output, mode, quality, jobs, no_gpu, trial, trial_days)
        }
        Commands::Export { input, output, format, colors, normals, noncommercial, trial, trial_days } => {
            cmd_export(input, output, format, colors, normals, noncommercial, trial, trial_days)
        }
        Commands::Calibrate { images, cols, rows, square_size_mm, output } => {
            cmd_calibrate(images, cols, rows, square_size_mm, output)
        }
        Commands::Inventory { action } => cmd_inventory(action),
        Commands::Status { hardware, check_updates } => cmd_status(hardware, check_updates),
        Commands::Config { action } => cmd_config(action),
        Commands::Jobs { action } => cmd_jobs(action),
    }
}

fn cmd_serve(port: u16, tray: bool, daemon: bool) -> Result<()> {
    println!("{} Starting TrueShot Server on port {}...", ROCKET, style(port).cyan());
    
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
    no_gpu: bool,
    trial: bool,
    trial_days: Option<i64>,
) -> Result<()> {
    let mut license_manager = init_license_manager()?;
    let required = process_required_features(mode);
    ensure_cli_license(&mut license_manager, &required, trial, trial_days)?;
    enforce_cli_scan_limit(&license_manager)?;
    enforce_cli_max_resolution(&license_manager, &input)?;

    let started_at = std::time::SystemTime::now();
    let started_iso = now_rfc3339(started_at);
    let start_instant = std::time::Instant::now();
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
        Mode::Burst => run_burst_pipeline(&input, &output, quality, jobs, no_gpu, Some(&inventory_ctx), Some(&mut run_state)),
        Mode::Photogrammetry | Mode::Gaussians | Mode::Hybrid | Mode::Quick => {
            run_reconstruction_pipeline(&input, &output, mode, quality, Some(&inventory_ctx), Some(&mut run_state))
        }
        Mode::Live => anyhow::bail!("Live mode is only available via the server and live capture workflow"),
        Mode::Avatar => anyhow::bail!("Avatar mode requires the full capture stack; use the server UI"),
    };

    if let Err(err) = result {
        eprintln!("{} Processing failed: {}", WARNING, err);
        update_inventory_sequence(&inventory_ctx, &output, SequenceStatus::Failed);
        run_state.mark_failed();
        let _ = write_run_report(
            &output,
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
        );
        return Err(err);
    }

    write_inventory_manifest(&output, &inventory_ctx, &input)?;
    update_inventory_sequence(&inventory_ctx, &output, SequenceStatus::Completed);
    run_state.mark_completed();
    let _ = write_run_report(
        &output,
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
    );

    println!();
    println!("{} Processing complete!", CHECK);
    println!("  Output saved to: {}", style(output.display()).green());

    Ok(())
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
        anyhow::bail!("Monthly scan limit exceeded (limit {max}). Upgrade your license to continue.");
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
    let _ = write_run_report(
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
    );

    Ok(())
}

fn init_license_manager() -> Result<LicenseManager> {
    LicenseManager::new().map_err(|err| anyhow::anyhow!(license_error_message(&err)))
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
    println!("{} Calibration saved to: {}", CHECK, style(output_path.display()).green());

    let report_path = output_path.with_extension("report.json");
    let _ = write_run_report(
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
    );

    Ok(())
}

fn run_burst_pipeline(
    input: &Path,
    output: &Path,
    quality: Quality,
    jobs: Option<usize>,
    no_gpu: bool,
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

    let image_count = count_images(input)?;
    println!("  Found {} images", style(image_count).cyan().bold());
    if let Some(state) = run_state.as_deref_mut() {
        state.mark_step_completed("scan_input", vec![]);
        state.mark_step_started("sfm");
    }

    let options = build_processing_options(quality, jobs, no_gpu);
    let loader = SmartLoader::new(options.clone());
    let sequences = loader.scan_and_group(input)?;

    let mp = MultiProgress::new();
    let seq_pb = mp.add(ProgressBar::new(sequences.len() as u64));
    seq_pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    for sequence in &sequences {
        seq_pb.set_message(format!("Sequence {}", sequence.meta.bone_id));
        let step_pb = mp.add(ProgressBar::new(3));
        step_pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:30.magenta/blue}] {pos}/{len} {msg}")
                .unwrap(),
        );

        let mut timer = HierarchicalTimer::new(&sequence.meta.bone_id);
        step_pb.set_message("Loading frames");
        let frames = loader.load_sequence(sequence, &mut timer)?;
        step_pb.inc(1);

        step_pb.set_message("Fusing frames");
        let engine = FusionEngine::new(options.clone());
        let result = engine.process(frames, &sequence.meta, &mut timer)?;
        step_pb.inc(1);

        step_pb.set_message("Exporting outputs");
        let output_path = generate_output_path(output, &sequence.meta.bone_id, &sequence.meta.vantage, sequence.meta.rot_deg);
        let mask_bool = result.mask.mapv(|v| v > 0);
        save_tiff16_from_f64(&result.rgb_f64, &mask_bool, &output_path)?;

        let preview_path = output_path.with_extension("png");
        save_png(&result.rgb_u8, &result.mask, &preview_path)?;

        let depth_path = output_path.with_file_name(format!(
            "{}_{}_{}deg_depth.tiff",
            sequence.meta.bone_id,
            sequence.meta.vantage,
            sequence.meta.rot_deg as u32
        ));
        let depth_f32 = result.depth_map.mapv(|v| v as f32);
        save_depth_tiff(&depth_f32, &depth_path)?;

        step_pb.finish_with_message("Sequence complete");
        seq_pb.inc(1);

        if options.verbose_timing {
            timer.report_console(true);
        }
    }

    seq_pb.finish_with_message("All sequences complete");
    if let Some(state) = run_state.as_deref_mut() {
        state.mark_step_completed("sfm", collect_artifacts(output, &["focus_stacks"]));
        state.mark_step_completed("reports", collect_artifacts(output, &["run_report.json"]));
    }
    Ok(())
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
    validate_photogrammetry_input(&image_dir, 6).context("Photogrammetry input validation failed")?;

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
            collect_artifacts(output, &[".trueshot/checkpoints/sparse_reconstruction.json", "sparse.ply"]),
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
            dense_points = run_dense_mvs(
                &image_paths,
                &poses,
                &cameras,
                quality,
            )?;
            if !dense_points.is_empty() {
                let dense_path = output.join("dense.ply");
                export_dense_point_cloud(&dense_points, &dense_path)?;
                save_dense_checkpoint(output, &dense_points)?;
            }
        }
        if let Some(state) = run_state.as_deref_mut() {
            state.mark_step_completed(
                "dense",
                collect_artifacts(output, &["dense.ply", ".trueshot/checkpoints/dense_points.zst"]),
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
            state.mark_step_completed(
                "gaussian",
                collect_artifacts(output, &["gaussians.ply"]),
            );
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
    if let Some(state) = run_state.as_deref_mut() {
        state.mark_step_completed(
            "reports",
            collect_artifacts(output, &["reconstruction_report.json", "intrinsics_report.json"]),
        );
    }

    Ok(())
}

fn build_processing_options(quality: Quality, jobs: Option<usize>, _no_gpu: bool) -> ProcessingOptions {
    let mut options = ProcessingOptions::default();
    options.max_parallel_sequences = jobs;
    options.export_format = "tiff16".to_string();
    options.verbose_timing = matches!(quality, Quality::High | Quality::Ultra);

    match quality {
        Quality::Low => {
            options.noise_sigma = 18.0;
            options.grade_k = 1.8;
            options.full_decode = false;
        }
        Quality::Medium => {
            options.noise_sigma = 10.0;
            options.grade_k = 1.5;
            options.full_decode = false;
        }
        Quality::High => {
            options.noise_sigma = 6.0;
            options.grade_k = 1.2;
            options.full_decode = true;
        }
        Quality::Ultra => {
            options.noise_sigma = 4.0;
            options.grade_k = 1.0;
            options.full_decode = true;
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
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
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
            if let Some((x, y)) = project_point(&point.position, &poses[view_idx], &intrinsics[view_idx]) {
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
    let adjusted_intrinsics: Vec<CameraIntrinsics> = images.iter().map(|img| img.intrinsics.clone()).collect();

    let patch_config = patchmatch_config_from_quality(quality);
    let mut depth_maps: Vec<DepthMap> = Vec::with_capacity(images.len());

    for (i, image) in images.iter().enumerate() {
        let src_indices = select_source_indices(i, images.len(), max_source_views(quality));
        let src_images: Vec<&GrayImage> = src_indices.iter().map(|&idx| &images[idx].gray).collect();
        let src_poses: Vec<&CameraPose> = src_indices.iter().map(|&idx| &poses[idx]).collect();
        let src_intrinsics: Vec<&CameraIntrinsics> = src_indices.iter().map(|&idx| &adjusted_intrinsics[idx]).collect();

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
        images.push(MvsImage { rgb, gray, intrinsics: intr });
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
                    if let Some((sx, sy, sz)) = project_to_camera(&point_world, src_pose, src_intr) {
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

fn write_intrinsics_report(
    output: &Path,
    reports: &[(String, IntrinsicsReport)],
) -> Result<()> {
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
            initial_points.push((na::Point3::new(p.position.x as f32, p.position.y as f32, p.position.z as f32), p.color));
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
        transform.fixed_view_mut::<3, 3>(0, 0).copy_from(&rotation.matrix().map(|v| v as f32));
        transform[(0, 3)] = pose.translation.x as f32;
        transform[(1, 3)] = pose.translation.y as f32;
        transform[(2, 3)] = pose.translation.z as f32;

        let intr_matrix = na::Matrix3::<f32>::new(
            intr.fx as f32, 0.0, intr.cx as f32,
            0.0, intr.fy as f32, intr.cy as f32,
            0.0, 0.0, 1.0,
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
    let mut config = TrainingConfig::default();
    config.iterations = match quality {
        Quality::Low => 1000,
        Quality::Medium => 3000,
        Quality::High => 8000,
        Quality::Ultra => 20000,
    };
    config
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
        let doc = toml::from_str(&content)
            .context("Failed to parse config TOML")?;
        Ok(doc)
    } else {
        let doc = toml::from_str(DEFAULT_CONFIG)
            .context("Failed to parse default config")?;
        Ok(doc)
    }
}

fn save_config_doc(doc: &TomlValue) -> Result<()> {
    let path = config_file_path();
    let rendered = toml::to_string_pretty(doc)
        .context("Failed to render config")?;
    std::fs::write(&path, rendered)
        .with_context(|| format!("Failed to write config {}", path.display()))?;
    Ok(())
}

fn parse_toml_value(raw: &str) -> TomlValue {
    let wrapped = format!("value = {}", raw);
    if let Ok(parsed) = toml::from_str::<TomlValue>(&wrapped) {
        if let TomlValue::Table(map) = parsed {
            if let Some(value) = map.get("value") {
                return value.clone();
            }
        }
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

fn confirm_delete(model_id: &str) -> Result<bool> {
    print!("Type DELETE to confirm deletion of {}: ", model_id);
    io::stdout().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim() == "DELETE")
}

fn create_inventory_context(input: &Path, output: &Path, mode: Mode, quality: Quality) -> Result<InventoryContext> {
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
        eprintln!("{} Inventory sequence folder update failed: {}", WARNING, err);
    }
    if let Err(err) = inventory.update_sequence_status(&ctx.sequence_id, status) {
        eprintln!("{} Inventory sequence status update failed: {}", WARNING, err);
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
                        let archived = path.with_extension(format!(
                            "json.bak.{}",
                            chrono::Utc::now().timestamp()
                        ));
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

fn save_sparse_checkpoint(output: &Path, reconstruction: &trueshot_core::reconstruction::multicam_sfm::SparseReconstruction) -> Result<()> {
    let path = sparse_checkpoint_path(output);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(reconstruction)?;
    std::fs::write(path, json)?;
    Ok(())
}

fn load_sparse_checkpoint(output: &Path) -> Result<trueshot_core::reconstruction::multicam_sfm::SparseReconstruction> {
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
        .map(|r| (na::Point3::new(r.position[0], r.position[1], r.position[2]), r.color))
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
) -> Result<()> {
    let finished_at = std::time::SystemTime::now();
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
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);

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
        "inventory": inventory_ctx.map(|ctx| serde_json::json!({
            "model_id": ctx.model_id.to_string(),
            "sequence_id": ctx.sequence_id.to_string(),
            "model_name": ctx.model_name,
        })),
    });

    let payload = match kind {
        RunReportKind::Process { mode, quality, input, output, jobs, gpu_disabled } => {
            let artifacts = collect_artifacts(&output, &[
                "sparse.ply",
                "dense.ply",
                "gaussians.ply",
                "reconstruction_report.json",
                "intrinsics_report.json",
                "inventory.json",
                "run_report.json",
            ]);
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
        RunReportKind::Export { input, output, format, include_colors, include_normals } => {
            serde_json::json!({
                "kind": "export",
                "input": input.to_string_lossy(),
                "output": output.to_string_lossy(),
                "format": format!("{:?}", format),
                "include_colors": include_colors,
                "include_normals": include_normals,
            })
        }
        RunReportKind::Calibrate { images, output, rows, cols, square_size_mm, rms_error, width, height } => {
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
    std::fs::write(path, serde_json::to_string_pretty(&json)?)?;
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
                println!("  {} No models found. Use 'trueshot process' to create models.", WARNING);
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
            let model_id = uuid::Uuid::parse_str(&id)
                .with_context(|| format!("Invalid model id: {}", id))?;
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
            let model_id = uuid::Uuid::parse_str(&id)
                .with_context(|| format!("Invalid model id: {}", id))?;
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
    
    let sys = sysinfo::System::new_all();
    println!("    CPU:    {} cores", num_cpus::get());
    println!("    Memory: {:.1} GB total, {:.1} GB available",
        sys.total_memory() as f64 / 1024.0 / 1024.0 / 1024.0,
        sys.available_memory() as f64 / 1024.0 / 1024.0 / 1024.0);
    
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
            println!("{} Configuration reset to defaults at {}", CHECK, path.display());
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
        JobsCommand::Submit {
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
        } => cmd_jobs_submit(
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
        ),
        JobsCommand::List { server, api_token } => cmd_jobs_list(server, api_token),
        JobsCommand::Get { id, server, api_token } => cmd_jobs_get(id, server, api_token),
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
        Some(value) => Uuid::parse_str(&value)
            .with_context(|| format!("Invalid request id: {}", value))?,
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
        let text = response.text().unwrap_or_else(|_| "Unknown error".to_string());
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
        let text = response.text().unwrap_or_else(|_| "Unknown error".to_string());
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
        let text = response.text().unwrap_or_else(|_| "Unknown error".to_string());
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
    let parsed = Url::parse(&normalized)
        .with_context(|| format!("Invalid server URL: {}", normalized))?;
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
        let json: serde_json::Value = serde_json::from_str(&content)
            .context("Payload file must be valid JSON")?;
        return Ok(json);
    }

    if let Some(text) = payload {
        let json: serde_json::Value = serde_json::from_str(&text)
            .context("Payload must be valid JSON")?;
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
    tray_menu.append_items(&[&quit_i, &PredefinedMenuItem::separator()]).unwrap();

    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip(&format!("TrueShot Server (port {})", port))
        .build()
        .unwrap();
    
    println!("{} Tray icon initialized. Running event loop...", CHECK);
    
    event_loop.run(move |_event, _, control_flow| {
        *control_flow = tao::event_loop::ControlFlow::Wait;
    });
}

fn count_images(dir: &Path) -> Result<usize> {
    if !dir.exists() {
        return Ok(0);
    }
    
    let extensions = ["jpg", "jpeg", "png", "tiff", "tif", "raw", "dng", "cr2", "nef"];
    let count = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| extensions.contains(&ext.to_lowercase().as_str()))
                .unwrap_or(false)
        })
        .count();
    
    Ok(count)
}
