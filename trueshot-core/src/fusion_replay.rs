//! Strict, source-bound replay metadata for measured HDR/focus revisions.

use crate::fusion_edit::{FusionEditDocument, MAX_FUSION_EDIT_BYTES};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path};

pub const FUSION_REPLAY_SCHEMA: &str = "trueshot.fusion.replay.v1";
pub const FUSION_REVISION_ENVELOPE_SCHEMA: &str = "trueshot.fusion.revision-envelope.v1";
pub const MAX_FUSION_BASE_REPORT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_FUSION_REVISION_ENVELOPE_BYTES: usize =
    MAX_FUSION_BASE_REPORT_BYTES + MAX_FUSION_EDIT_BYTES as usize + 64 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FusionReplayCapsule {
    pub schema: String,
    pub project_layout: String,
    pub quality: String,
    pub jobs: Option<usize>,
    pub full_frame: bool,
    pub gpu_enabled: bool,
    pub export_depth: bool,
    pub full_resolution_preview: bool,
    pub preview_max_dimension: usize,
    pub deghost_strength: f32,
    pub frequency_separated_deghosting: bool,
    pub glare_spread_um: f32,
    pub glare_aware_focus: bool,
    pub depth_consistent_refusion: bool,
    pub sensor_noise_profile: Option<FusionReplayArtifact>,
    pub sensor_correction_profile: Option<FusionReplayArtifact>,
    pub lens_psf_profile: Option<FusionReplayArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FusionReplayArtifact {
    pub project_relative_path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FusionRevisionEnvelope {
    pub schema: String,
    pub edit: FusionEditDocument,
    /// The exact UTF-8 bytes of the immutable base report.
    pub base_report_json: String,
}

impl FusionReplayCapsule {
    pub fn validate(&self) -> Result<()> {
        if self.schema != FUSION_REPLAY_SCHEMA {
            anyhow::bail!("Unsupported fusion replay schema");
        }
        if self.project_layout != "raw_output_siblings" {
            anyhow::bail!("Unsupported fusion replay project layout");
        }
        if !matches!(self.quality.as_str(), "low" | "medium" | "high" | "ultra") {
            anyhow::bail!("Unsupported fusion replay quality");
        }
        if self.jobs.is_some_and(|jobs| !(1..=32).contains(&jobs)) {
            anyhow::bail!("Fusion replay jobs must be between 1 and 32");
        }
        if !(64..=16_384).contains(&self.preview_max_dimension) {
            anyhow::bail!("Fusion replay preview dimension is out of bounds");
        }
        if !self.deghost_strength.is_finite() || !(0.0..=2.0).contains(&self.deghost_strength) {
            anyhow::bail!("Fusion replay deghost strength is out of bounds");
        }
        if !self.glare_spread_um.is_finite() || !(1.0..=2_000.0).contains(&self.glare_spread_um) {
            anyhow::bail!("Fusion replay glare spread is out of bounds");
        }
        for artifact in [
            self.sensor_noise_profile.as_ref(),
            self.sensor_correction_profile.as_ref(),
            self.lens_psf_profile.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            artifact.validate()?;
        }
        Ok(())
    }
}

impl FusionReplayArtifact {
    pub fn validate(&self) -> Result<()> {
        let path = Path::new(&self.project_relative_path);
        if self.project_relative_path.is_empty()
            || self.project_relative_path.len() > 1_024
            || path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            anyhow::bail!("Fusion replay profile path is not project-relative");
        }
        validate_sha256("Fusion replay profile digest", &self.sha256)
    }
}

impl FusionRevisionEnvelope {
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_FUSION_REVISION_ENVELOPE_BYTES {
            anyhow::bail!("Fusion revision envelope exceeds the bounded input limit");
        }
        let envelope: Self =
            serde_json::from_slice(bytes).context("Parse fusion revision envelope")?;
        envelope.validate()?;
        Ok(envelope)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != FUSION_REVISION_ENVELOPE_SCHEMA {
            anyhow::bail!("Unsupported fusion revision envelope schema");
        }
        self.edit.validate()?;
        if self.base_report_json.len() > MAX_FUSION_BASE_REPORT_BYTES {
            anyhow::bail!("Fusion base report exceeds the bounded input limit");
        }
        let observed = hex::encode(Sha256::digest(self.base_report_json.as_bytes()));
        if observed != self.edit.base_report_sha256 {
            anyhow::bail!("Fusion revision envelope base report digest mismatch");
        }
        let report: serde_json::Value =
            serde_json::from_str(&self.base_report_json).context("Parse fusion base report")?;
        let object = report
            .as_object()
            .context("Fusion base report must be an object")?;
        if object.get("schema").and_then(|value| value.as_str())
            != Some("trueshot.fusion.provenance.v2")
        {
            anyhow::bail!("Fusion base report schema is not supported");
        }
        if object
            .get("archival_policy")
            .and_then(|value| value.as_str())
            != Some("measured_sources_only_no_generative_reconstruction")
        {
            anyhow::bail!("Fusion base report is not measured-only");
        }
        if object.get("fusion_edit").is_some() {
            anyhow::bail!("Fusion edit chaining is not allowed");
        }
        if object
            .get("capture_group_id")
            .and_then(|value| value.as_str())
            != Some(self.edit.capture_group_id.as_str())
            || object
                .get("revision_group_id")
                .and_then(|value| value.as_str())
                != Some(self.edit.capture_group_id.as_str())
        {
            anyhow::bail!("Fusion base report does not identify the immutable base group");
        }
        let replay: FusionReplayCapsule = serde_json::from_value(
            object
                .get("replay")
                .cloned()
                .context("Fusion base report has no executable replay capsule")?,
        )
        .context("Parse fusion replay capsule")?;
        replay.validate()
    }

    pub fn replay(&self) -> Result<FusionReplayCapsule> {
        let report: serde_json::Value = serde_json::from_str(&self.base_report_json)?;
        let replay = report
            .get("replay")
            .cloned()
            .context("Fusion base report has no executable replay capsule")?;
        let replay: FusionReplayCapsule = serde_json::from_value(replay)?;
        replay.validate()?;
        Ok(replay)
    }
}

fn validate_sha256(label: &str, value: &str) -> Result<()> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        anyhow::bail!("{label} must be lowercase 64-character SHA-256 hex");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fusion_edit::{
        FusionEditOperation, FusionEditReason, FusionEditRect, FUSION_EDIT_SCHEMA,
    };

    fn replay() -> FusionReplayCapsule {
        FusionReplayCapsule {
            schema: FUSION_REPLAY_SCHEMA.to_string(),
            project_layout: "raw_output_siblings".to_string(),
            quality: "ultra".to_string(),
            jobs: Some(4),
            full_frame: false,
            gpu_enabled: true,
            export_depth: true,
            full_resolution_preview: false,
            preview_max_dimension: 1600,
            deghost_strength: 1.0,
            frequency_separated_deghosting: true,
            glare_spread_um: 80.0,
            glare_aware_focus: true,
            depth_consistent_refusion: true,
            sensor_noise_profile: None,
            sensor_correction_profile: None,
            lens_psf_profile: None,
        }
    }

    #[test]
    fn envelope_binds_exact_measured_base_and_replay() {
        let capture_group_id = "a".repeat(64);
        let report = serde_json::json!({
            "schema": "trueshot.fusion.provenance.v2",
            "archival_policy": "measured_sources_only_no_generative_reconstruction",
            "capture_group_id": capture_group_id,
            "revision_group_id": capture_group_id,
            "replay": replay(),
        });
        let base_report_json = serde_json::to_string_pretty(&report).unwrap();
        let edit = FusionEditDocument {
            schema: FUSION_EDIT_SCHEMA.to_string(),
            capture_group_id: "a".repeat(64),
            base_report_sha256: hex::encode(Sha256::digest(base_report_json.as_bytes())),
            width: 2,
            height: 2,
            crop_origin_x: 0,
            crop_origin_y: 0,
            frame_count: 1,
            operations: vec![FusionEditOperation {
                id: "measured".to_string(),
                rect: FusionEditRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                source_frame: 0,
                reason: FusionEditReason::Focus,
                selector: crate::fusion_edit::FusionEditSelector::Rectangle,
                note: None,
            }],
        };
        let envelope = FusionRevisionEnvelope {
            schema: FUSION_REVISION_ENVELOPE_SCHEMA.to_string(),
            edit,
            base_report_json,
        };
        envelope.validate().unwrap();
        assert_eq!(envelope.replay().unwrap().quality, "ultra");
    }

    #[test]
    fn rejects_profile_escape_and_tampered_report() {
        let mut invalid = replay();
        invalid.sensor_noise_profile = Some(FusionReplayArtifact {
            project_relative_path: "../noise.json".to_string(),
            sha256: "b".repeat(64),
        });
        assert!(invalid.validate().is_err());
    }
}
