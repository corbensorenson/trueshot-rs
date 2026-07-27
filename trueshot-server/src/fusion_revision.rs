use crate::at_rest::{
    decrypt_file_handle_to_bytes, encrypt_file_in_place, policy_for_project, require_master_key,
    ProjectKeyStore,
};
use crate::config::AppConfig;
use crate::fs_safety::open_project_file_read;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use trueshot_core::fusion_edit::{FusionEditDocument, MAX_FUSION_EDIT_BYTES};
use trueshot_core::fusion_replay::{
    FusionReplayArtifact, FusionReplayCapsule, FusionRevisionEnvelope,
    FUSION_REVISION_ENVELOPE_SCHEMA, MAX_FUSION_BASE_REPORT_BYTES,
};
use trueshot_core::lens_psf::MAX_LENS_PSF_ARTIFACT_BYTES;
use trueshot_core::scheduler::Job;
use trueshot_core::sensor_correction::MAX_SENSOR_CORRECTION_PROFILE_BYTES;
use trueshot_core::sensor_noise::MAX_SENSOR_NOISE_PROFILE_BYTES;
use uuid::Uuid;
use walkdir::WalkDir;
use zeroize::Zeroizing;

pub const FUSION_REVISION_JOB_KIND: &str = "local_fusion_revision";
const MAX_REVISION_INPUT_FILES: usize = 2_000_000;
const MAX_FUSION_PROCESSOR_INPUT_BYTES: usize =
    trueshot_core::fusion_replay::MAX_FUSION_REVISION_ENVELOPE_BYTES
        + 2 * (MAX_SENSOR_NOISE_PROFILE_BYTES as usize
            + MAX_SENSOR_CORRECTION_PROFILE_BYTES as usize
            + MAX_LENS_PSF_ARTIFACT_BYTES as usize)
        + 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FusionRevisionJobPayload {
    pub schema: String,
    pub project_id: String,
    pub report_path: String,
    pub report_sha256: String,
    pub edit_path: String,
    pub edit_digest: String,
}

#[derive(Clone, Default)]
pub struct FusionRevisionExecutor {
    cancellations: Arc<Mutex<HashMap<Uuid, Arc<AtomicBool>>>>,
}

pub struct FusionRevisionJob {
    id: Uuid,
    config: AppConfig,
    payload: FusionRevisionJobPayload,
    cancellation: Arc<AtomicBool>,
    cancellations: Arc<Mutex<HashMap<Uuid, Arc<AtomicBool>>>>,
}

impl FusionRevisionJob {
    fn check_cancelled(&self) -> Result<()> {
        if self.cancellation.load(Ordering::Acquire) {
            anyhow::bail!("Fusion revision cancelled by operator");
        }
        Ok(())
    }
}

impl Drop for FusionRevisionJob {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.cancellations.lock() {
            guard.remove(&self.id);
        }
    }
}

impl FusionRevisionExecutor {
    pub fn build_job(
        &self,
        id: Uuid,
        config: &AppConfig,
        payload: FusionRevisionJobPayload,
    ) -> Result<FusionRevisionJob> {
        payload.validate()?;
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut guard = self
            .cancellations
            .lock()
            .map_err(|_| anyhow::anyhow!("Fusion cancellation registry is unavailable"))?;
        if guard.len() >= 1_024 && !guard.contains_key(&id) {
            anyhow::bail!("Fusion revision executor is at its bounded job limit");
        }
        guard.insert(id, cancellation.clone());
        drop(guard);
        Ok(FusionRevisionJob {
            id,
            config: config.clone(),
            payload,
            cancellation,
            cancellations: self.cancellations.clone(),
        })
    }

    pub fn cancel(&self, id: Uuid) -> bool {
        let Ok(guard) = self.cancellations.lock() else {
            return false;
        };
        let Some(cancellation) = guard.get(&id) else {
            return false;
        };
        cancellation.store(true, Ordering::Release);
        true
    }
}

impl FusionRevisionJobPayload {
    pub fn validate(&self) -> Result<()> {
        if self.schema != "trueshot.fusion.revision-job.v1" {
            anyhow::bail!("Unsupported fusion revision job schema");
        }
        validate_simple_project_id(&self.project_id)?;
        validate_relative_path(&self.report_path)?;
        validate_relative_path(&self.edit_path)?;
        if !self.report_path.ends_with("_fusion_report.json") {
            anyhow::bail!("Fusion revision report path is invalid");
        }
        if !self.edit_path.ends_with(".json") {
            anyhow::bail!("Fusion revision edit path is invalid");
        }
        if !self.edit_path.starts_with(".trueshot/fusion_edits/") {
            anyhow::bail!("Fusion revision edit path is outside the immutable edit store");
        }
        validate_sha256("report_sha256", &self.report_sha256)?;
        validate_sha256("edit_digest", &self.edit_digest)
    }
}

pub fn preflight(config: &AppConfig, payload: &FusionRevisionJobPayload) -> Result<()> {
    prepare_revision(config, payload).map(|_| ())
}

#[async_trait]
impl Job for FusionRevisionJob {
    fn name(&self) -> &str {
        "Measured HDR/focus revision"
    }

    async fn execute(&self, progress_tx: mpsc::Sender<f32>) -> Result<()> {
        self.check_cancelled()?;
        let _ = progress_tx.send(0.05).await;
        let prepared = prepare_revision(&self.config, &self.payload)?;
        self.check_cancelled()?;
        let _ = progress_tx.send(0.12).await;

        let executable = resolve_packaged_cli()?;
        let mut command = tokio::process::Command::new(executable);
        command
            .arg("process")
            .arg("--input")
            .arg(&prepared.raw_root)
            .arg("--output")
            .arg(&prepared.output_root)
            .arg("--mode")
            .arg("burst")
            .arg("--quality")
            .arg(&prepared.replay.quality)
            .arg("--preview-max-dimension")
            .arg(prepared.replay.preview_max_dimension.to_string())
            .arg("--deghost-strength")
            .arg(prepared.replay.deghost_strength.to_string())
            .arg("--glare-spread-um")
            .arg(prepared.replay.glare_spread_um.to_string())
            .arg("--fusion-revision-stdin")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(jobs) = prepared.replay.jobs {
            command.arg("--jobs").arg(jobs.to_string());
        }
        if prepared.replay.full_frame {
            command.arg("--full-frame");
        }
        if !prepared.replay.gpu_enabled {
            command.arg("--no-gpu");
        }
        if prepared.replay.export_depth {
            command.arg("--depth");
        }
        if prepared.replay.full_resolution_preview {
            command.arg("--full-resolution-preview");
        }
        if !prepared.replay.frequency_separated_deghosting {
            command.arg("--no-frequency-deghost");
        }
        if !prepared.replay.glare_aware_focus {
            command.arg("--no-glare-focus");
        }
        if !prepared.replay.depth_consistent_refusion {
            command.arg("--no-depth-refusion");
        }
        let envelope_bytes = Zeroizing::new(serde_json::to_vec(&FusionProcessorInput {
            envelope: &prepared.envelope,
            encrypted_raw_key: prepared.encrypted_raw_key.as_deref(),
            sensor_noise_profile_json: prepared
                .sensor_noise_profile_json
                .as_ref()
                .map(|value| value.as_str()),
            sensor_correction_profile_json: prepared
                .sensor_correction_profile_json
                .as_ref()
                .map(|value| value.as_str()),
            lens_psf_profile_json: prepared
                .lens_psf_profile_json
                .as_ref()
                .map(|value| value.as_str()),
        })?);
        if envelope_bytes.len() > MAX_FUSION_PROCESSOR_INPUT_BYTES {
            anyhow::bail!("Fusion processor input exceeds the bounded size limit");
        }
        let mut child = command
            .spawn()
            .context("Launch packaged TrueShot processor")?;
        let stderr = child
            .stderr
            .take()
            .context("Fusion processor stderr is unavailable")?;
        let stderr_task = tokio::spawn(read_bounded_stderr(stderr));
        let mut stdin = child
            .stdin
            .take()
            .context("Fusion processor stdin is unavailable")?;
        stdin
            .write_all(&envelope_bytes)
            .await
            .context("Send bounded fusion revision envelope")?;
        stdin
            .shutdown()
            .await
            .context("Close fusion revision stdin")?;
        drop(stdin);
        let _ = progress_tx.send(0.2).await;

        let status = loop {
            if self.cancellation.load(Ordering::Acquire) {
                let _ = child.kill().await;
                let _ = child.wait().await;
                let _ = stderr_task.await;
                anyhow::bail!("Fusion revision cancelled by operator");
            }
            match tokio::time::timeout(std::time::Duration::from_millis(250), child.wait()).await {
                Ok(status) => break status.context("Wait for fusion revision processor")?,
                Err(_) => {
                    let _ = progress_tx.send(0.25).await;
                }
            }
        };
        if !status.success() {
            let diagnostic = stderr_task
                .await
                .unwrap_or_else(|error| format!("stderr reader failed: {error}"));
            let diagnostic = diagnostic.trim();
            if diagnostic.is_empty() {
                anyhow::bail!("Fusion revision processor exited with status {status}");
            }
            anyhow::bail!("Fusion revision processor exited with status {status}: {diagnostic}");
        }
        let _ = stderr_task.await;
        let _ = progress_tx.send(0.92).await;
        if prepared.encrypt_revision_outputs {
            encrypt_revision_outputs(
                &self.config,
                &self.payload.project_id,
                &prepared.output_root,
                &self.payload.edit_digest,
            )?;
        }
        let _ = progress_tx.send(1.0).await;
        Ok(())
    }

    fn is_cancelled(&self) -> bool {
        self.cancellation.load(Ordering::Acquire)
    }
}

async fn read_bounded_stderr(mut stderr: tokio::process::ChildStderr) -> String {
    const MAX_RETAINED_BYTES: usize = 64 * 1024;
    let mut retained = Vec::with_capacity(MAX_RETAINED_BYTES);
    let mut chunk = [0u8; 8 * 1024];
    loop {
        let read = match stderr.read(&mut chunk).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => return format!("stderr read failed: {error}"),
        };
        if read >= MAX_RETAINED_BYTES {
            retained.clear();
            retained.extend_from_slice(&chunk[read - MAX_RETAINED_BYTES..read]);
            continue;
        }
        let overflow = retained
            .len()
            .saturating_add(read)
            .saturating_sub(MAX_RETAINED_BYTES);
        if overflow > 0 {
            retained.drain(..overflow);
        }
        retained.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8_lossy(&retained).into_owned()
}

struct PreparedRevision {
    raw_root: PathBuf,
    output_root: PathBuf,
    replay: FusionReplayCapsule,
    envelope: FusionRevisionEnvelope,
    sensor_noise_profile_json: Option<Zeroizing<String>>,
    sensor_correction_profile_json: Option<Zeroizing<String>>,
    lens_psf_profile_json: Option<Zeroizing<String>>,
    encrypt_revision_outputs: bool,
    encrypted_raw_key: Option<Zeroizing<[u8; 32]>>,
}

#[derive(Serialize)]
struct FusionProcessorInput<'a> {
    envelope: &'a FusionRevisionEnvelope,
    encrypted_raw_key: Option<&'a [u8; 32]>,
    sensor_noise_profile_json: Option<&'a str>,
    sensor_correction_profile_json: Option<&'a str>,
    lens_psf_profile_json: Option<&'a str>,
}

fn prepare_revision(
    config: &AppConfig,
    payload: &FusionRevisionJobPayload,
) -> Result<PreparedRevision> {
    payload.validate()?;
    let projects_root = config
        .paths
        .projects_dir
        .canonicalize()
        .context("Resolve projects root")?;
    let project_root = projects_root.join(&payload.project_id);
    let project_root = project_root
        .canonicalize()
        .context("Resolve fusion revision project")?;
    if !project_root.starts_with(&projects_root) {
        anyhow::bail!("Fusion revision project escaped the projects root");
    }
    let raw_root = canonical_real_directory(&project_root.join("raw"), &project_root)?;
    let output_root = canonical_real_directory(&project_root.join("output"), &project_root)?;
    let encrypted_raw_paths = inspect_raw_inputs(&raw_root)?;
    let mut encrypted_project_key = if encrypted_raw_paths.is_empty() {
        None
    } else {
        let master = require_master_key(&config.privacy, &config.paths.projects_dir)?;
        let key = ProjectKeyStore::new(&config.paths.projects_dir, master)
            .load_or_create(&payload.project_id)?;
        for path in &encrypted_raw_paths {
            trueshot_storage::encrypted::SeekableEncryptedFile::open(path, &key).with_context(
                || {
                    format!(
                        "Encrypted RAW {} is not an authenticated seekable TSE2 asset",
                        path.display()
                    )
                },
            )?;
        }
        Some(Zeroizing::new(key))
    };

    let report_path = output_root.join(&payload.report_path);
    let report_bytes = read_bounded_project_artifact(
        config,
        &payload.project_id,
        &report_path,
        MAX_FUSION_BASE_REPORT_BYTES,
    )?
    .context("Fusion base report not found")?;
    let report_sha256 = hex::encode(Sha256::digest(&report_bytes));
    if report_sha256 != payload.report_sha256 {
        anyhow::bail!("Fusion base report changed after revision authoring");
    }
    let edit_path = output_root.join(&payload.edit_path);
    let edit_bytes = read_bounded_project_artifact(
        config,
        &payload.project_id,
        &edit_path,
        MAX_FUSION_EDIT_BYTES as usize,
    )?
    .context("Fusion edit document not found")?;
    let edit: FusionEditDocument =
        serde_json::from_slice(&edit_bytes).context("Parse fusion edit document")?;
    edit.validate()?;
    if edit.digest()? != payload.edit_digest || edit.base_report_sha256 != report_sha256 {
        anyhow::bail!("Fusion edit identity does not match the immutable base report");
    }
    let base_report_json =
        String::from_utf8(report_bytes).context("Fusion base report is not UTF-8 JSON")?;
    let envelope = FusionRevisionEnvelope {
        schema: FUSION_REVISION_ENVELOPE_SCHEMA.to_string(),
        edit,
        base_report_json,
    };
    envelope.validate()?;
    let replay = envelope.replay()?;
    let opened_sensor_noise = open_replay_profile(
        &config.paths.projects_dir,
        &payload.project_id,
        &project_root,
        replay.sensor_noise_profile.as_ref(),
    )?;
    let opened_sensor_correction = open_replay_profile(
        &config.paths.projects_dir,
        &payload.project_id,
        &project_root,
        replay.sensor_correction_profile.as_ref(),
    )?;
    let opened_lens_psf = open_replay_profile(
        &config.paths.projects_dir,
        &payload.project_id,
        &project_root,
        replay.lens_psf_profile.as_ref(),
    )?;
    let encrypted_profile_present = [
        opened_sensor_noise.as_ref(),
        opened_sensor_correction.as_ref(),
        opened_lens_psf.as_ref(),
    ]
    .into_iter()
    .flatten()
    .any(|profile| profile.encrypted);
    if encrypted_profile_present && encrypted_project_key.is_none() {
        let master = require_master_key(&config.privacy, &config.paths.projects_dir)?;
        encrypted_project_key = Some(Zeroizing::new(
            ProjectKeyStore::new(&config.paths.projects_dir, master)
                .load_or_create(&payload.project_id)?,
        ));
    }
    let sensor_noise_profile_json = read_replay_profile(
        opened_sensor_noise,
        replay.sensor_noise_profile.as_ref(),
        encrypted_project_key.as_deref(),
        MAX_SENSOR_NOISE_PROFILE_BYTES as usize,
    )?;
    let sensor_correction_profile_json = read_replay_profile(
        opened_sensor_correction,
        replay.sensor_correction_profile.as_ref(),
        encrypted_project_key.as_deref(),
        MAX_SENSOR_CORRECTION_PROFILE_BYTES as usize,
    )?;
    let lens_psf_profile_json = read_replay_profile(
        opened_lens_psf,
        replay.lens_psf_profile.as_ref(),
        encrypted_project_key.as_deref(),
        MAX_LENS_PSF_ARTIFACT_BYTES as usize,
    )?;
    let encrypt_revision_outputs = policy_for_project(
        &config.paths.projects_dir,
        &payload.project_id,
        &config.privacy,
    )
    .is_some_and(|policy| policy.scopes.iter().any(|scope| scope == "output"));

    Ok(PreparedRevision {
        raw_root,
        output_root,
        replay,
        envelope,
        sensor_noise_profile_json,
        sensor_correction_profile_json,
        lens_psf_profile_json,
        encrypt_revision_outputs,
        encrypted_raw_key: encrypted_project_key,
    })
}

struct OpenedReplayProfile {
    file: std::fs::File,
    encrypted: bool,
}

fn open_replay_profile(
    projects_root: &Path,
    project_id: &str,
    project_root: &Path,
    artifact: Option<&FusionReplayArtifact>,
) -> Result<Option<OpenedReplayProfile>> {
    let Some(artifact) = artifact else {
        return Ok(None);
    };
    artifact.validate()?;
    let clear = project_root.join(&artifact.project_relative_path);
    match open_project_file_read(projects_root, project_id, &clear) {
        Ok(file) => {
            return Ok(Some(OpenedReplayProfile {
                file,
                encrypted: false,
            }));
        }
        Err(response) if response.status() == actix_web::http::StatusCode::NOT_FOUND => {}
        Err(response) => {
            anyhow::bail!(
                "Rooted replay profile open failed with {}",
                response.status()
            )
        }
    }
    let encrypted = PathBuf::from(format!("{}.enc", clear.display()));
    match open_project_file_read(projects_root, project_id, &encrypted) {
        Ok(file) => Ok(Some(OpenedReplayProfile {
            file,
            encrypted: true,
        })),
        Err(response) if response.status() == actix_web::http::StatusCode::NOT_FOUND => {
            anyhow::bail!(
                "Fusion replay profile is missing: {}",
                artifact.project_relative_path
            )
        }
        Err(response) => {
            anyhow::bail!(
                "Rooted encrypted replay profile open failed with {}",
                response.status()
            )
        }
    }
}

fn read_replay_profile(
    opened: Option<OpenedReplayProfile>,
    artifact: Option<&FusionReplayArtifact>,
    encrypted_key: Option<&[u8; 32]>,
    max_bytes: usize,
) -> Result<Option<Zeroizing<String>>> {
    let Some(opened) = opened else {
        return Ok(None);
    };
    let artifact = artifact.context("Opened replay profile is missing replay identity")?;
    artifact.validate()?;
    let bytes = if opened.encrypted {
        let key = encrypted_key.context("Encrypted replay profile key is unavailable")?;
        Zeroizing::new(decrypt_file_handle_to_bytes(opened.file, key, max_bytes)?)
    } else {
        if opened.file.metadata()?.len() > max_bytes as u64 {
            anyhow::bail!("Replay profile exceeds the bounded size limit");
        }
        let mut reader = opened.file.take(max_bytes.saturating_add(1) as u64);
        let mut bytes = Zeroizing::new(Vec::new());
        reader.read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes {
            anyhow::bail!("Replay profile exceeds the bounded size limit");
        }
        bytes
    };
    let observed = hex::encode(Sha256::digest(bytes.as_slice()));
    if observed != artifact.sha256 {
        anyhow::bail!("Fusion replay profile digest changed");
    }
    let json = std::str::from_utf8(&bytes)
        .context("Fusion replay profile is not UTF-8 JSON")?
        .to_owned();
    Ok(Some(Zeroizing::new(json)))
}

fn read_bounded_project_artifact(
    config: &AppConfig,
    project_id: &str,
    logical_path: &Path,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>> {
    match open_project_file_read(&config.paths.projects_dir, project_id, logical_path) {
        Ok(file) => {
            if file.metadata()?.len() > max_bytes as u64 {
                anyhow::bail!("Fusion artifact exceeds the bounded read limit");
            }
            let mut reader = file.take(max_bytes.saturating_add(1) as u64);
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut reader, &mut bytes)?;
            if bytes.len() > max_bytes {
                anyhow::bail!("Fusion artifact exceeds the bounded read limit");
            }
            return Ok(Some(bytes));
        }
        Err(response) if response.status() == actix_web::http::StatusCode::NOT_FOUND => {}
        Err(response) => {
            anyhow::bail!(
                "Rooted fusion artifact open failed with {}",
                response.status()
            )
        }
    }
    let encrypted = PathBuf::from(format!("{}.enc", logical_path.display()));
    let encrypted_file =
        match open_project_file_read(&config.paths.projects_dir, project_id, &encrypted) {
            Ok(file) => file,
            Err(response) if response.status() == actix_web::http::StatusCode::NOT_FOUND => {
                return Ok(None)
            }
            Err(response) => {
                anyhow::bail!(
                    "Rooted encrypted fusion artifact open failed with {}",
                    response.status()
                )
            }
        };
    let master = require_master_key(&config.privacy, &config.paths.projects_dir)?;
    let key =
        ProjectKeyStore::new(&config.paths.projects_dir, master).load_or_create(project_id)?;
    Ok(Some(decrypt_file_handle_to_bytes(
        encrypted_file,
        &key,
        max_bytes,
    )?))
}

fn inspect_raw_inputs(raw_root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = 0usize;
    let mut raw_files = 0usize;
    let mut encrypted_raw_paths = Vec::new();
    for entry in WalkDir::new(raw_root).follow_links(false) {
        let entry = entry.context("Inspect fusion revision RAW input")?;
        if entry.file_type().is_symlink() {
            anyhow::bail!("Fusion revision RAW input cannot contain symbolic links");
        }
        if entry.file_type().is_file() {
            files = files.saturating_add(1);
            if files > MAX_REVISION_INPUT_FILES {
                anyhow::bail!("Fusion revision RAW input exceeds the bounded file limit");
            }
            if trueshot_core::exif_parser::is_nef_asset_path(entry.path()) {
                raw_files = raw_files.saturating_add(1);
                if entry.path().extension().and_then(|value| value.to_str()) == Some("enc") {
                    encrypted_raw_paths.push(entry.path().to_path_buf());
                }
            }
        }
    }
    if raw_files == 0 {
        anyhow::bail!("Fusion revision RAW input is empty");
    }
    Ok(encrypted_raw_paths)
}

fn encrypt_revision_outputs(
    config: &AppConfig,
    project_id: &str,
    output_root: &Path,
    edit_digest: &str,
) -> Result<()> {
    let master = require_master_key(&config.privacy, &config.paths.projects_dir)?;
    let key =
        ProjectKeyStore::new(&config.paths.projects_dir, master).load_or_create(project_id)?;
    let marker = format!("_edit_{}", &edit_digest[..12]);
    let mut matched = 0usize;
    for entry in WalkDir::new(output_root).max_depth(8).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains(&marker) && !name.ends_with(".enc"))
        {
            encrypt_file_in_place(path, &key, 0)?;
            matched = matched.saturating_add(1);
        }
    }
    if matched == 0 {
        anyhow::bail!("Fusion revision completed without discoverable revision artifacts");
    }
    Ok(())
}

fn resolve_packaged_cli() -> Result<PathBuf> {
    let server = std::env::current_exe()?.canonicalize()?;
    let directory = server
        .parent()
        .context("Server executable has no directory")?;
    let cli = directory.join("trueshot");
    let cli = cli
        .canonicalize()
        .context("Packaged trueshot processor is missing beside the server")?;
    if cli.parent() != Some(directory)
        || cli.file_name().and_then(|name| name.to_str()) != Some("trueshot")
    {
        anyhow::bail!("Packaged trueshot processor identity is invalid");
    }
    Ok(cli)
}

fn canonical_real_directory(path: &Path, project_root: &Path) -> Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!("Fusion revision scope is not a real directory");
    }
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(project_root) {
        anyhow::bail!("Fusion revision scope escaped the project");
    }
    Ok(canonical)
}

fn validate_simple_project_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || value == "."
        || value == ".."
        || value.contains("..")
        || value.contains(['/', '\\', ':'])
        || value.chars().any(char::is_control)
    {
        anyhow::bail!("Invalid fusion revision project id");
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<()> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > 2_048
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        anyhow::bail!("Invalid project-relative fusion revision path");
    }
    Ok(())
}

fn validate_sha256(label: &str, value: &str) -> Result<()> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        anyhow::bail!("{label} must be lowercase SHA-256 hex");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> FusionRevisionJobPayload {
        FusionRevisionJobPayload {
            schema: "trueshot.fusion.revision-job.v1".to_string(),
            project_id: "hero-object".to_string(),
            report_path: "hero_fusion_report.json".to_string(),
            report_sha256: "a".repeat(64),
            edit_path: format!(".trueshot/fusion_edits/hero_{}.json", "b".repeat(64)),
            edit_digest: "b".repeat(64),
        }
    }

    #[test]
    fn revision_payload_is_project_bound_and_path_safe() {
        payload().validate().unwrap();
        let mut escaped = payload();
        escaped.edit_path = "../revision.json".to_string();
        assert!(escaped.validate().is_err());
        let mut arbitrary = payload();
        arbitrary.edit_path = "revision.json".to_string();
        assert!(arbitrary.validate().is_err());
        let mut reused = payload();
        reused.project_id = "other/project".to_string();
        assert!(reused.validate().is_err());
    }

    #[test]
    fn cancellation_registry_is_bounded_to_active_identity() {
        let executor = FusionRevisionExecutor::default();
        assert!(!executor.cancel(Uuid::new_v4()));
    }

    #[cfg(unix)]
    #[test]
    fn encrypted_replay_profile_is_descriptor_bound_digest_checked_and_zero_copy_handoff_ready() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let project_root = directory.path().join("project");
        let raw = project_root.join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        let clear = raw.join("noise.json");
        let encrypted = raw.join("noise.json.enc");
        let encrypted_original = raw.join("noise.original.json.enc");
        let attacker = directory.path().join("attacker.json.enc");
        let payload = br#"{"schema":"trueshot.sensor-noise.v1"}"#;
        let key = [0x51u8; 32];
        std::fs::write(&clear, payload).unwrap();
        trueshot_storage::encrypted::encrypt_file(&clear, &encrypted, &key, 64 * 1024).unwrap();
        trueshot_storage::encrypted::encrypt_bytes(
            &attacker,
            &key,
            br#"{"schema":"attacker"}"#,
            64 * 1024,
        )
        .unwrap();
        std::fs::remove_file(&clear).unwrap();
        let artifact = FusionReplayArtifact {
            project_relative_path: "raw/noise.json".to_string(),
            sha256: hex::encode(Sha256::digest(payload)),
        };

        let opened =
            open_replay_profile(directory.path(), "project", &project_root, Some(&artifact))
                .unwrap();
        assert!(opened.as_ref().unwrap().encrypted);
        std::fs::rename(&encrypted, &encrypted_original).unwrap();
        symlink(&attacker, &encrypted).unwrap();
        let resolved = read_replay_profile(
            opened,
            Some(&artifact),
            Some(&key),
            MAX_SENSOR_NOISE_PROFILE_BYTES as usize,
        )
        .unwrap();
        assert_eq!(
            resolved.as_ref().map(|value| value.as_str()),
            Some(std::str::from_utf8(payload).unwrap())
        );

        std::fs::remove_file(&encrypted).unwrap();
        std::fs::rename(&encrypted_original, &encrypted).unwrap();
        let opened =
            open_replay_profile(directory.path(), "project", &project_root, Some(&artifact))
                .unwrap();
        assert!(read_replay_profile(
            opened,
            Some(&artifact),
            Some(&[0x52u8; 32]),
            MAX_SENSOR_NOISE_PROFILE_BYTES as usize,
        )
        .is_err());
        assert!(!clear.exists());
    }
}
