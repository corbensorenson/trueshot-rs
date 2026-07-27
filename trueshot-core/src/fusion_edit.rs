//! Deterministic, source-bound operator corrections for archival RAW fusion.
//!
//! Edits select an actual measured frame over a bounded rectangle. They never
//! synthesize pixels, interpolate incompatible sources, or mutate a base result.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const FUSION_EDIT_SCHEMA: &str = "trueshot.fusion.edits.v2";
pub const LEGACY_FUSION_EDIT_SCHEMA_V1: &str = "trueshot.fusion.edits.v1";
pub const MAX_FUSION_EDIT_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_FUSION_EDIT_OPERATIONS: usize = 2_048;
pub const MAX_FUSION_EDIT_NOTE_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FusionEditDocument {
    pub schema: String,
    pub capture_group_id: String,
    pub base_report_sha256: String,
    pub width: u32,
    pub height: u32,
    pub crop_origin_x: u32,
    pub crop_origin_y: u32,
    pub frame_count: u16,
    pub operations: Vec<FusionEditOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FusionEditOperation {
    pub id: String,
    pub rect: FusionEditRect,
    pub source_frame: u16,
    pub reason: FusionEditReason,
    #[serde(default)]
    pub selector: FusionEditSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FusionEditRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FusionEditReason {
    Motion,
    Disocclusion,
    Focus,
    Glare,
    Boundary,
    Other,
}

/// Recomputed fusion evidence that limits which pixels inside a rectangle may
/// be rebound to the selected measured source.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FusionEditSelector {
    #[default]
    Rectangle,
    GlareAffected,
    BoundaryAffected,
    BoundaryCrossingCore,
}

#[derive(Debug, Clone, Copy)]
pub struct FusionEditBinding<'a> {
    pub capture_group_id: &'a str,
    pub width: usize,
    pub height: usize,
    pub crop_origin_x: usize,
    pub crop_origin_y: usize,
    pub frame_count: usize,
}

impl FusionEditDocument {
    pub fn load_json(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path)
            .with_context(|| format!("Inspect fusion edit document {}", path.display()))?;
        if metadata.len() > MAX_FUSION_EDIT_BYTES {
            anyhow::bail!(
                "Fusion edit document exceeds {} byte limit",
                MAX_FUSION_EDIT_BYTES
            );
        }
        let bytes = std::fs::read(path)
            .with_context(|| format!("Read fusion edit document {}", path.display()))?;
        let document: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("Parse fusion edit document {}", path.display()))?;
        document.validate()?;
        Ok(document)
    }

    pub fn validate(&self) -> Result<()> {
        if !matches!(
            self.schema.as_str(),
            FUSION_EDIT_SCHEMA | LEGACY_FUSION_EDIT_SCHEMA_V1
        ) {
            anyhow::bail!(
                "Unsupported fusion edit schema {}; expected {} or {}",
                self.schema,
                FUSION_EDIT_SCHEMA,
                LEGACY_FUSION_EDIT_SCHEMA_V1
            );
        }
        validate_sha256("capture_group_id", &self.capture_group_id)?;
        validate_sha256("base_report_sha256", &self.base_report_sha256)?;
        if self.width == 0 || self.height == 0 || self.frame_count == 0 {
            anyhow::bail!("Fusion edit dimensions and frame count must be positive");
        }
        if self.operations.is_empty() || self.operations.len() > MAX_FUSION_EDIT_OPERATIONS {
            anyhow::bail!(
                "Fusion edit document must contain 1-{} operations",
                MAX_FUSION_EDIT_OPERATIONS
            );
        }

        for (index, operation) in self.operations.iter().enumerate() {
            validate_operation(
                index,
                operation,
                self.width,
                self.height,
                self.frame_count,
                &self.schema,
            )?;
        }
        let mut ordered = self.operations.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|operation| {
            (
                operation.rect.x,
                operation.rect.y,
                operation.rect.width,
                operation.rect.height,
                operation.id.as_str(),
            )
        });
        for (index, left) in ordered.iter().enumerate() {
            let left_x1 = left.rect.x + left.rect.width;
            for right in &ordered[index + 1..] {
                if right.rect.x >= left_x1 {
                    break;
                }
                if rectangles_overlap(left.rect, right.rect) {
                    anyhow::bail!(
                        "Fusion edit operations {} and {} overlap; revisions must be unambiguous",
                        left.id,
                        right.id
                    );
                }
            }
        }
        Ok(())
    }

    pub fn validate_binding(&self, binding: FusionEditBinding<'_>) -> Result<()> {
        self.validate()?;
        if self.capture_group_id != binding.capture_group_id {
            anyhow::bail!(
                "Fusion edit capture group {} does not match {}",
                self.capture_group_id,
                binding.capture_group_id
            );
        }
        let expected = (
            usize::try_from(self.width)?,
            usize::try_from(self.height)?,
            usize::try_from(self.crop_origin_x)?,
            usize::try_from(self.crop_origin_y)?,
            usize::from(self.frame_count),
        );
        let actual = (
            binding.width,
            binding.height,
            binding.crop_origin_x,
            binding.crop_origin_y,
            binding.frame_count,
        );
        if expected != actual {
            anyhow::bail!(
                "Fusion edit binding {:?} does not match decoded group {:?}",
                expected,
                actual
            );
        }
        Ok(())
    }

    /// SHA-256 over deterministic compact JSON. The digest names a new output
    /// revision while the base report remains immutable.
    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }

    pub fn edited_pixel_count(&self) -> u64 {
        self.operations
            .iter()
            .map(|operation| u64::from(operation.rect.width) * u64::from(operation.rect.height))
            .sum()
    }
}

fn validate_operation(
    index: usize,
    operation: &FusionEditOperation,
    width: u32,
    height: u32,
    frame_count: u16,
    schema: &str,
) -> Result<()> {
    if operation.id.is_empty()
        || operation.id.len() > 80
        || operation
            .id
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    {
        anyhow::bail!("Fusion edit operation {} has an invalid id", index);
    }
    if operation.source_frame >= frame_count {
        anyhow::bail!(
            "Fusion edit operation {} selects frame {} outside 0..{}",
            operation.id,
            operation.source_frame,
            frame_count
        );
    }
    if operation.rect.width == 0 || operation.rect.height == 0 {
        anyhow::bail!(
            "Fusion edit operation {} has an empty rectangle",
            operation.id
        );
    }
    let x1 = operation
        .rect
        .x
        .checked_add(operation.rect.width)
        .context("Fusion edit rectangle x overflow")?;
    let y1 = operation
        .rect
        .y
        .checked_add(operation.rect.height)
        .context("Fusion edit rectangle y overflow")?;
    if x1 > width || y1 > height {
        anyhow::bail!(
            "Fusion edit operation {} exceeds {}x{} output",
            operation.id,
            width,
            height
        );
    }
    if let Some(note) = &operation.note {
        if note.len() > MAX_FUSION_EDIT_NOTE_BYTES || note.chars().any(char::is_control) {
            anyhow::bail!(
                "Fusion edit operation {} note is invalid or exceeds {} bytes",
                operation.id,
                MAX_FUSION_EDIT_NOTE_BYTES
            );
        }
    }
    if schema == LEGACY_FUSION_EDIT_SCHEMA_V1 {
        if operation.selector != FusionEditSelector::Rectangle {
            anyhow::bail!(
                "Legacy fusion edit operation {} cannot use an evidence selector",
                operation.id
            );
        }
        if matches!(
            operation.reason,
            FusionEditReason::Glare | FusionEditReason::Boundary
        ) {
            anyhow::bail!(
                "Legacy fusion edit operation {} uses physical reason {:?}; migrate it to schema {} with a matching evidence selector",
                operation.id,
                operation.reason,
                FUSION_EDIT_SCHEMA
            );
        }
    } else {
        let selector_matches_reason = matches!(
            (operation.reason, operation.selector),
            (FusionEditReason::Glare, FusionEditSelector::GlareAffected)
                | (
                    FusionEditReason::Boundary,
                    FusionEditSelector::BoundaryAffected | FusionEditSelector::BoundaryCrossingCore
                )
                | (
                    FusionEditReason::Motion
                        | FusionEditReason::Disocclusion
                        | FusionEditReason::Focus
                        | FusionEditReason::Other,
                    FusionEditSelector::Rectangle
                )
        );
        if !selector_matches_reason {
            anyhow::bail!(
                "Fusion edit operation {} reason {:?} is incompatible with selector {:?}",
                operation.id,
                operation.reason,
                operation.selector
            );
        }
    }
    Ok(())
}

fn rectangles_overlap(left: FusionEditRect, right: FusionEditRect) -> bool {
    left.x < right.x + right.width
        && right.x < left.x + left.width
        && left.y < right.y + right.height
        && right.y < left.y + left.height
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

    fn document() -> FusionEditDocument {
        FusionEditDocument {
            schema: FUSION_EDIT_SCHEMA.to_string(),
            capture_group_id: "a".repeat(64),
            base_report_sha256: "b".repeat(64),
            width: 64,
            height: 48,
            crop_origin_x: 12,
            crop_origin_y: 20,
            frame_count: 6,
            operations: vec![
                FusionEditOperation {
                    id: "motion-left".to_string(),
                    rect: FusionEditRect {
                        x: 2,
                        y: 3,
                        width: 8,
                        height: 9,
                    },
                    source_frame: 2,
                    reason: FusionEditReason::Motion,
                    selector: FusionEditSelector::Rectangle,
                    note: Some("Use the measured reference without interpolation.".to_string()),
                },
                FusionEditOperation {
                    id: "focus-right".to_string(),
                    rect: FusionEditRect {
                        x: 20,
                        y: 3,
                        width: 8,
                        height: 9,
                    },
                    source_frame: 5,
                    reason: FusionEditReason::Focus,
                    selector: FusionEditSelector::Rectangle,
                    note: None,
                },
            ],
        }
    }

    #[test]
    fn validates_bound_non_overlapping_measured_edits() {
        let document = document();
        document
            .validate_binding(FusionEditBinding {
                capture_group_id: &"a".repeat(64),
                width: 64,
                height: 48,
                crop_origin_x: 12,
                crop_origin_y: 20,
                frame_count: 6,
            })
            .unwrap();
        assert_eq!(document.edited_pixel_count(), 144);
        assert_eq!(document.digest().unwrap().len(), 64);
    }

    #[test]
    fn rejects_overlap_and_binding_drift() {
        let mut overlapping = document();
        overlapping.operations[1].rect.x = 9;
        assert!(overlapping.validate().is_err());

        let document = document();
        assert!(document
            .validate_binding(FusionEditBinding {
                capture_group_id: &"a".repeat(64),
                width: 65,
                height: 48,
                crop_origin_x: 12,
                crop_origin_y: 20,
                frame_count: 6,
            })
            .is_err());
    }

    #[test]
    fn digest_is_order_sensitive_but_stable() {
        let first = document();
        let mut second = first.clone();
        second.operations.swap(0, 1);
        assert_eq!(first.digest().unwrap(), first.digest().unwrap());
        assert_ne!(first.digest().unwrap(), second.digest().unwrap());
    }

    #[test]
    fn physical_reasons_require_matching_evidence_selectors() {
        let mut glare = document();
        glare.operations[0].reason = FusionEditReason::Glare;
        assert!(glare.validate().is_err());
        glare.operations[0].selector = FusionEditSelector::GlareAffected;
        assert!(glare.validate().is_ok());

        let mut boundary = document();
        boundary.operations[0].reason = FusionEditReason::Boundary;
        boundary.operations[0].selector = FusionEditSelector::BoundaryAffected;
        assert!(boundary.validate().is_ok());
        boundary.operations[0].selector = FusionEditSelector::BoundaryCrossingCore;
        assert!(boundary.validate().is_ok());
    }

    #[test]
    fn legacy_v1_defaults_to_rectangles_and_rejects_new_selectors() {
        let payload = serde_json::json!({
            "schema": LEGACY_FUSION_EDIT_SCHEMA_V1,
            "capture_group_id": "a".repeat(64),
            "base_report_sha256": "b".repeat(64),
            "width": 8,
            "height": 8,
            "crop_origin_x": 0,
            "crop_origin_y": 0,
            "frame_count": 1,
            "operations": [{
                "id": "legacy",
                "rect": {"x": 0, "y": 0, "width": 2, "height": 2},
                "source_frame": 0,
                "reason": "focus"
            }]
        });
        let mut legacy: FusionEditDocument = serde_json::from_value(payload).unwrap();
        assert_eq!(legacy.operations[0].selector, FusionEditSelector::Rectangle);
        legacy.validate().unwrap();
        legacy.operations[0].selector = FusionEditSelector::BoundaryAffected;
        assert!(legacy.validate().is_err());

        legacy.operations[0].selector = FusionEditSelector::Rectangle;
        legacy.operations[0].reason = FusionEditReason::Glare;
        assert!(legacy.validate().is_err());
        legacy.operations[0].reason = FusionEditReason::Boundary;
        assert!(legacy.validate().is_err());
    }
}
