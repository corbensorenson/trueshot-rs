//! Streaming capture manifests for collection-scale local processing.
//!
//! A manifest is newline-delimited JSON: one header followed by one complete
//! focus/HDR group per line. Readers retain only the current line and group,
//! making memory independent of collection size.

use crate::capture::AdaptiveCaptureProvenance;
use crate::smart_loader::SequenceCropPlan;
use crate::types::Sequence;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, ErrorKind, Write};
use std::path::{Component, Path, PathBuf};

pub const CAPTURE_MANIFEST_SCHEMA: &str = "trueshot.capture.v1";
pub const DEFAULT_CAPTURE_MANIFEST: &str = ".trueshot/capture_manifest.v1.jsonl";
pub const MAX_CAPTURE_MANIFEST_RECORD_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureManifestHeader {
    pub schema: String,
    pub created_at: DateTime<Utc>,
    pub group_count: u64,
    /// Root relative to the manifest directory. Absolute roots are rejected.
    pub source_root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureGroup {
    pub group_id: String,
    pub sequence: Sequence,
    #[serde(default)]
    pub frame_order: Vec<CaptureFrameOrder>,
    pub crop_plan: Option<SequenceCropPlan>,
    #[serde(default)]
    pub adaptive_capture: Option<AdaptiveCaptureProvenance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureFrameOrder {
    pub frame_index: u32,
    pub focus_index: u16,
    pub exposure_index: u16,
    pub burst_index: u16,
    pub sequence_reference: bool,
}

impl CaptureGroup {
    pub fn from_sequence(sequence: Sequence) -> Self {
        let group_id = stable_group_id(&sequence);
        let frame_order = derive_frame_order(&sequence);
        Self {
            group_id,
            sequence,
            frame_order,
            crop_plan: None,
            adaptive_capture: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "record_type", rename_all = "snake_case")]
enum ManifestRecord {
    Header(CaptureManifestHeader),
    Group(Box<CaptureGroup>),
}

pub struct CaptureManifestReader {
    reader: BufReader<File>,
    header: CaptureManifestHeader,
    resolved_root: PathBuf,
    line_number: u64,
    groups_read: u64,
    line_buffer: Vec<u8>,
}

impl CaptureManifestReader {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("Open capture manifest {}", path.display()))?;
        let mut reader = BufReader::with_capacity(256 * 1024, file);
        let mut first_line = Vec::with_capacity(4 * 1024);
        if read_bounded_line(&mut reader, &mut first_line)? == 0 {
            anyhow::bail!("Capture manifest {} is empty", path.display());
        }
        let ManifestRecord::Header(header) = serde_json::from_slice(trim_line_ending(&first_line))
            .with_context(|| format!("Parse capture manifest header {}", path.display()))?
        else {
            anyhow::bail!("Capture manifest must begin with a header record");
        };
        if header.schema != CAPTURE_MANIFEST_SCHEMA {
            anyhow::bail!(
                "Unsupported capture manifest schema {}; expected {}",
                header.schema,
                CAPTURE_MANIFEST_SCHEMA
            );
        }
        validate_source_root(&header.source_root)?;
        let manifest_directory = path.parent().unwrap_or_else(|| Path::new("."));
        let resolved_root = manifest_directory
            .join(&header.source_root)
            .canonicalize()
            .with_context(|| {
                format!(
                    "Resolve manifest source root {}",
                    manifest_directory.join(&header.source_root).display()
                )
            })?;
        let allowed_root = manifest_directory
            .parent()
            .unwrap_or(manifest_directory)
            .canonicalize()
            .with_context(|| {
                format!(
                    "Resolve allowed capture root {}",
                    manifest_directory.display()
                )
            })?;
        if !resolved_root.starts_with(&allowed_root) {
            anyhow::bail!(
                "Manifest source root {} escapes allowed input root {}",
                resolved_root.display(),
                allowed_root.display()
            );
        }
        Ok(Self {
            reader,
            header,
            resolved_root,
            line_number: 1,
            groups_read: 0,
            line_buffer: Vec::with_capacity(64 * 1024),
        })
    }

    pub fn header(&self) -> &CaptureManifestHeader {
        &self.header
    }

    pub fn total_groups(&self) -> u64 {
        self.header.group_count
    }

    fn read_next(&mut self) -> Option<Result<CaptureGroup>> {
        loop {
            match read_bounded_line(&mut self.reader, &mut self.line_buffer) {
                Ok(0) => {
                    if self.groups_read != self.header.group_count {
                        return Some(Err(anyhow::anyhow!(
                            "Capture manifest ended after {} groups; header declared {}",
                            self.groups_read,
                            self.header.group_count
                        )));
                    }
                    return None;
                }
                Ok(_) => {
                    self.line_number += 1;
                    if !trim_ascii(&self.line_buffer).is_empty() {
                        break;
                    }
                }
                Err(error) => return Some(Err(error.into())),
            }
        }

        let parsed = (|| -> Result<CaptureGroup> {
            let ManifestRecord::Group(group) =
                serde_json::from_slice(trim_line_ending(&self.line_buffer))
                    .with_context(|| format!("Parse manifest line {}", self.line_number))?
            else {
                anyhow::bail!("Unexpected header at manifest line {}", self.line_number);
            };
            let mut group = *group;
            if group.frame_order.is_empty() {
                group.frame_order = derive_frame_order(&group.sequence);
            }
            validate_group(&group)?;
            if stable_group_id(&group.sequence) != group.group_id {
                anyhow::bail!(
                    "Capture group ID does not match content at manifest line {}",
                    self.line_number
                );
            }
            for path in &mut group.sequence.paths {
                validate_relative_path(path)?;
                *path = self.resolved_root.join(&*path);
            }
            self.groups_read += 1;
            if self.groups_read > self.header.group_count {
                anyhow::bail!(
                    "Capture manifest contains more than declared {} groups",
                    self.header.group_count
                );
            }
            Ok(group)
        })();
        Some(parsed)
    }
}

fn read_bounded_line<R: BufRead>(reader: &mut R, buffer: &mut Vec<u8>) -> std::io::Result<usize> {
    buffer.clear();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(buffer.len());
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map(|index| index + 1).unwrap_or(available.len());
        if buffer.len().saturating_add(consumed) > MAX_CAPTURE_MANIFEST_RECORD_BYTES {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "Capture manifest record exceeds {} bytes",
                    MAX_CAPTURE_MANIFEST_RECORD_BYTES
                ),
            ));
        }
        buffer.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(buffer.len());
        }
    }
}

fn trim_line_ending(mut value: &[u8]) -> &[u8] {
    while value
        .last()
        .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
    {
        value = &value[..value.len() - 1];
    }
    value
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

impl Iterator for CaptureManifestReader {
    type Item = Result<CaptureGroup>;

    fn next(&mut self) -> Option<Self::Item> {
        self.read_next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.header.group_count.saturating_sub(self.groups_read);
        let remaining = usize::try_from(remaining).unwrap_or(usize::MAX);
        (remaining, Some(remaining))
    }
}

pub enum CaptureGroupSource {
    Manifest(CaptureManifestReader),
    Legacy(std::vec::IntoIter<CaptureGroup>),
}

impl CaptureGroupSource {
    pub fn total_groups(&self) -> u64 {
        match self {
            Self::Manifest(reader) => reader.total_groups(),
            Self::Legacy(groups) => groups.len() as u64,
        }
    }

    pub fn is_streaming_manifest(&self) -> bool {
        matches!(self, Self::Manifest(_))
    }
}

impl Iterator for CaptureGroupSource {
    type Item = Result<CaptureGroup>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Manifest(reader) => reader.next(),
            Self::Legacy(groups) => groups.next().map(Ok),
        }
    }
}

pub fn discover_capture_manifest(input: &Path) -> Option<PathBuf> {
    let direct = input.join(DEFAULT_CAPTURE_MANIFEST);
    direct.is_file().then_some(direct)
}

/// Atomically write a manifest. Paths are stored relative to `source_root`.
pub fn write_capture_manifest<I>(
    path: &Path,
    source_root: &Path,
    groups: I,
    group_count: u64,
) -> Result<()>
where
    I: IntoIterator<Item = CaptureGroup>,
{
    let mut writer = CaptureManifestWriter::begin(path, source_root, group_count)?;
    for group in groups {
        writer.append(group)?;
    }
    writer.finish()
}

/// Bounded capture-time manifest writer. Call `append` as each complete
/// HDR/focus group is captured, then atomically publish with `finish`.
pub struct CaptureManifestWriter {
    target: PathBuf,
    temporary: PathBuf,
    source_root: PathBuf,
    expected_groups: u64,
    written_groups: u64,
    writer: Option<BufWriter<File>>,
}

impl CaptureManifestWriter {
    pub fn begin(path: &Path, source_root: &Path, expected_groups: u64) -> Result<Self> {
        let manifest_directory = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(manifest_directory)?;
        let source_root = source_root
            .canonicalize()
            .with_context(|| format!("Resolve capture source root {}", source_root.display()))?;
        let relative_root = lexical_relative_path(manifest_directory, &source_root)?;
        validate_source_root(&relative_root)?;
        let temporary =
            path.with_file_name(format!(".capture-manifest.{}.part", uuid::Uuid::new_v4()));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .with_context(|| format!("Create manifest {}", temporary.display()))?;
        let mut writer = BufWriter::with_capacity(256 * 1024, file);
        serde_json::to_writer(
            &mut writer,
            &ManifestRecord::Header(CaptureManifestHeader {
                schema: CAPTURE_MANIFEST_SCHEMA.to_string(),
                created_at: Utc::now(),
                group_count: expected_groups,
                source_root: relative_root,
            }),
        )?;
        writer.write_all(b"\n")?;
        Ok(Self {
            target: path.to_path_buf(),
            temporary,
            source_root,
            expected_groups,
            written_groups: 0,
            writer: Some(writer),
        })
    }

    pub fn append(&mut self, mut group: CaptureGroup) -> Result<()> {
        if self.written_groups >= self.expected_groups {
            anyhow::bail!(
                "Manifest already contains declared {} groups",
                self.expected_groups
            );
        }
        validate_group(&group)?;
        for frame_path in &mut group.sequence.paths {
            let canonical = frame_path
                .canonicalize()
                .with_context(|| format!("Resolve capture frame {}", frame_path.display()))?;
            *frame_path = canonical
                .strip_prefix(&self.source_root)
                .with_context(|| {
                    format!(
                        "Frame {} is outside manifest source root {}",
                        canonical.display(),
                        self.source_root.display()
                    )
                })?
                .to_path_buf();
            validate_relative_path(frame_path)?;
        }
        group.group_id = stable_group_id(&group.sequence);
        let writer = self
            .writer
            .as_mut()
            .context("Capture manifest writer is already finished")?;
        serde_json::to_writer(&mut *writer, &ManifestRecord::Group(Box::new(group)))?;
        writer.write_all(b"\n")?;
        self.written_groups += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        if self.written_groups != self.expected_groups {
            anyhow::bail!(
                "Manifest declared {} groups but writer received {}",
                self.expected_groups,
                self.written_groups
            );
        }
        let mut writer = self
            .writer
            .take()
            .context("Capture manifest writer is already finished")?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        std::fs::rename(&self.temporary, &self.target)
            .with_context(|| format!("Publish capture manifest {}", self.target.display()))?;
        sync_manifest_directory(&self.target)?;
        Ok(())
    }
}

impl Drop for CaptureManifestWriter {
    fn drop(&mut self) {
        if self.writer.is_some() {
            self.writer.take();
            let _ = std::fs::remove_file(&self.temporary);
        }
    }
}

#[cfg(unix)]
fn sync_manifest_directory(path: &Path) -> Result<()> {
    File::open(path.parent().unwrap_or_else(|| Path::new(".")))?
        .sync_all()
        .context("Sync capture manifest directory")
}

#[cfg(not(unix))]
fn sync_manifest_directory(_path: &Path) -> Result<()> {
    Ok(())
}

pub fn stable_group_id(sequence: &Sequence) -> String {
    let mut digest = Sha256::new();
    digest.update(b"trueshot-capture-group-v1\0");
    digest.update([sequence.meta.focus_steps]);
    digest.update((sequence.paths.len() as u64).to_le_bytes());
    for path in &sequence.paths {
        digest.update(path.to_string_lossy().as_bytes());
        digest.update([0]);
    }
    for shutter in &sequence.meta.shutter_speeds {
        digest.update(shutter.to_bits().to_le_bytes());
    }
    hex::encode(digest.finalize())
}

fn validate_group(group: &CaptureGroup) -> Result<()> {
    if group.group_id.len() != 64 || !group.group_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("Capture group ID must be a 64-character SHA-256 hex digest");
    }
    if group.sequence.paths.is_empty() {
        anyhow::bail!("Capture group {} has no frames", group.group_id);
    }
    let focus_steps = usize::from(group.sequence.meta.focus_steps).max(1);
    if group.sequence.paths.len() % focus_steps != 0 {
        anyhow::bail!(
            "Capture group {} has {} frames for {} focus planes",
            group.group_id,
            group.sequence.paths.len(),
            focus_steps
        );
    }
    if group.frame_order.len() != group.sequence.paths.len() {
        anyhow::bail!(
            "Capture group {} has {} frame-order records for {} paths",
            group.group_id,
            group.frame_order.len(),
            group.sequence.paths.len()
        );
    }
    let exposure_count = group.sequence.meta.shutter_speeds.len().max(1);
    let burst_count = usize::from(group.sequence.meta.burst_factor).max(1);
    let mut references = 0usize;
    for (expected_index, order) in group.frame_order.iter().enumerate() {
        if order.frame_index as usize != expected_index
            || order.focus_index as usize >= focus_steps
            || order.exposure_index as usize >= exposure_count
            || order.burst_index as usize >= burst_count
        {
            anyhow::bail!(
                "Capture group {} has invalid frame ordering at index {}",
                group.group_id,
                expected_index
            );
        }
        references += usize::from(order.sequence_reference);
    }
    if references != 1 {
        anyhow::bail!(
            "Capture group {} must identify exactly one sequence reference frame",
            group.group_id
        );
    }
    if let Some(plan) = group.crop_plan {
        if plan.reference_index >= group.sequence.paths.len() {
            anyhow::bail!("Capture group crop reference is outside the frame list");
        }
    }
    if let Some(provenance) = &group.adaptive_capture {
        provenance.validate(group.sequence.paths.len())?;
    }
    Ok(())
}

fn derive_frame_order(sequence: &Sequence) -> Vec<CaptureFrameOrder> {
    let exposure_count = sequence.meta.shutter_speeds.len().max(1);
    let burst_count = usize::from(sequence.meta.burst_factor).max(1);
    let plane_stride = exposure_count.saturating_mul(burst_count).max(1);
    let reference_index = sequence
        .ref_index()
        .min(sequence.paths.len().saturating_sub(1));
    sequence
        .paths
        .iter()
        .enumerate()
        .map(|(frame_index, _)| {
            let within_plane = frame_index % plane_stride;
            CaptureFrameOrder {
                frame_index: frame_index as u32,
                focus_index: (frame_index / plane_stride) as u16,
                exposure_index: (within_plane / burst_count) as u16,
                burst_index: (within_plane % burst_count) as u16,
                sequence_reference: frame_index == reference_index,
            }
        })
        .collect()
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!(
                    "Manifest path must be relative and cannot traverse: {}",
                    path.display()
                )
            }
        }
    }
    Ok(())
}

fn validate_source_root(path: &Path) -> Result<()> {
    if path.is_absolute() {
        anyhow::bail!("Manifest source root must be relative: {}", path.display());
    }
    for component in path.components() {
        if matches!(component, Component::RootDir | Component::Prefix(_)) {
            anyhow::bail!("Manifest source root must be relative: {}", path.display());
        }
    }
    Ok(())
}

fn lexical_relative_path(from: &Path, to: &Path) -> Result<PathBuf> {
    let from = from
        .canonicalize()
        .with_context(|| format!("Resolve manifest directory {}", from.display()))?;
    let to = to
        .canonicalize()
        .with_context(|| format!("Resolve capture source root {}", to.display()))?;
    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();
    let common = from_components
        .iter()
        .zip(&to_components)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        anyhow::bail!(
            "Manifest directory {} and source {} have no common root",
            from.display(),
            to.display()
        );
    }
    let mut relative = PathBuf::new();
    for _ in common..from_components.len() {
        relative.push("..");
    }
    for component in &to_components[common..] {
        relative.push(component.as_os_str());
    }
    if relative.as_os_str().is_empty() {
        relative.push(".");
    }
    Ok(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{
        plan_next_capture, AdaptiveCaptureProvenance, AdaptiveCaptureTermination,
        AdaptivePlannerConfig, CaptureCandidate, CapturePosterior, FocusProbe, RadianceProbe,
    };
    use crate::sensor_noise::{
        IsoNoiseModel, SensorNoiseModel, SensorNoiseProfile, SENSOR_NOISE_PROFILE_SCHEMA,
    };
    use crate::types::{Meta, Rect};

    fn sequence(root: &Path, index: usize) -> Sequence {
        let group_directory = root.join(format!("group-{index}"));
        std::fs::create_dir_all(&group_directory).unwrap();
        let first = group_directory.join("f0e0.nef");
        let second = group_directory.join("f0e1.nef");
        std::fs::write(&first, b"frame-0").unwrap();
        std::fs::write(&second, b"frame-1").unwrap();
        Sequence {
            paths: vec![first, second],
            meta: Meta {
                focus_steps: 1,
                exposures: vec![-1.0, 1.0],
                shutter_speeds: vec![0.004, 0.016],
                ref_focus: 0,
                ref_exp: 0.0,
                rot_deg: 0.0,
                vantage: "mid".to_string(),
                burst_factor: 1,
                bone_id: format!("group-{index}"),
                cam_mul: [2.0, 1.0, 1.5, 1.0],
            },
        }
    }

    fn adaptive_trace() -> AdaptiveCaptureProvenance {
        let profile = SensorNoiseProfile {
            schema: SENSOR_NOISE_PROFILE_SCHEMA.to_string(),
            camera_make: "Nikon".to_string(),
            camera_model: "Z9".to_string(),
            bits_per_sample: 14,
            calibration_id: "sha256:manifest-planner-test".to_string(),
            iso_models: vec![IsoNoiseModel {
                iso: 100,
                model: SensorNoiseModel {
                    read_noise_dn: [2.0; 4],
                    electrons_per_dn: [0.8; 4],
                    black_drift_dn: [0.25; 4],
                    saturation_margin_dn: 16.0,
                    calibrated: true,
                },
            }],
        };
        let candidate = CaptureCandidate {
            shutter_seconds: 0.01,
            iso: 100,
            focus_diopters: 2.0,
            readout_ms: 20.0,
            settle_ms: 5.0,
        };
        let mut posterior = CapturePosterior {
            radiance: vec![RadianceProbe {
                mean: 0.2,
                variance: 0.2,
                weight: 1.0,
                cfa_site: 1,
            }],
            focus: vec![FocusProbe {
                mean_diopters: 2.0,
                variance_diopters2: 0.2,
                weight: 1.0,
            }],
            radiance_anchor_exposure: 0.01 / 64.0,
            current_focus_diopters: 1.0,
            motion_pixels_per_second: 0.0,
            elapsed_ms: 0.0,
            thermal_load: 0.0,
        };
        let config = AdaptivePlannerConfig::default();
        let mut trace = AdaptiveCaptureProvenance::new(&profile).unwrap();
        let decision = plan_next_capture(&posterior, &[candidate], &profile, config).unwrap();
        trace.record(posterior.clone(), decision, Some(0)).unwrap();
        posterior.radiance[0].variance = 0.0;
        posterior.focus[0].variance_diopters2 = 0.0;
        let decision = plan_next_capture(&posterior, &[candidate], &profile, config).unwrap();
        trace.record(posterior, decision, None).unwrap();
        trace
            .finish(AdaptiveCaptureTermination::QualityTargetsReached)
            .unwrap();
        trace
    }

    #[test]
    fn manifest_round_trip_streams_groups_and_crop_plan() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("captures");
        std::fs::create_dir_all(&source).unwrap();
        let manifest = directory.path().join(DEFAULT_CAPTURE_MANIFEST);
        let groups: Vec<_> = (0..3)
            .map(|index| {
                let mut group = CaptureGroup::from_sequence(sequence(&source, index));
                group.crop_plan = Some(SequenceCropPlan {
                    reference_index: 1,
                    rect: Some(Rect::new(10.0, 20.0, 100.0, 80.0)),
                });
                group.adaptive_capture = Some(adaptive_trace());
                group
            })
            .collect();
        write_capture_manifest(&manifest, &source, groups, 3).unwrap();

        let mut reader = CaptureManifestReader::open(&manifest).unwrap();
        assert_eq!(reader.total_groups(), 3);
        assert_eq!(reader.size_hint(), (3, Some(3)));
        let first = reader.next().unwrap().unwrap();
        assert_eq!(
            first.sequence.paths[0],
            source.canonicalize().unwrap().join("group-0/f0e0.nef")
        );
        assert_eq!(first.crop_plan.unwrap().reference_index, 1);
        let adaptive = first.adaptive_capture.unwrap();
        assert_eq!(adaptive.iterations.len(), 2);
        assert_eq!(
            adaptive.termination,
            Some(AdaptiveCaptureTermination::QualityTargetsReached)
        );
        assert_eq!(reader.count(), 2);
    }

    #[test]
    fn manifest_rejects_path_traversal() {
        assert!(validate_relative_path(Path::new("../escape.nef")).is_err());
        assert!(validate_relative_path(Path::new("/absolute.nef")).is_err());
    }

    #[test]
    fn bounded_line_reader_rejects_oversized_records() {
        let bytes = vec![b'x'; MAX_CAPTURE_MANIFEST_RECORD_BYTES + 1];
        let mut reader = std::io::BufReader::new(std::io::Cursor::new(bytes));
        let mut line = Vec::new();
        let error = read_bounded_line(&mut reader, &mut line).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn unfinished_incremental_manifest_is_not_published() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("captures");
        std::fs::create_dir_all(&source).unwrap();
        let manifest = directory.path().join(DEFAULT_CAPTURE_MANIFEST);
        {
            let mut writer = CaptureManifestWriter::begin(&manifest, &source, 2).unwrap();
            writer
                .append(CaptureGroup::from_sequence(sequence(&source, 0)))
                .unwrap();
        }
        assert!(!manifest.exists());
        assert!(manifest
            .parent()
            .unwrap()
            .read_dir()
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".part")));
    }
}
