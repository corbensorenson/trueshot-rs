//! Disk-backed, crash-safe processing journal for idempotent group execution.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

const GROUP_JOURNAL: TableDefinition<&str, &str> =
    TableDefinition::new("capture_group_processing_v1");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupProcessingStatus {
    Running,
    Committed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactDigest {
    pub relative_path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    #[serde(default)]
    pub modified_unix_nanos: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupJournalEntry {
    pub group_id: String,
    pub status: GroupProcessingStatus,
    pub attempts: u32,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub duration_ms: Option<u64>,
    pub artifacts: Vec<ArtifactDigest>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimDecision {
    Process { attempt: u32 },
    AlreadyCommitted,
    RetryLimitReached { attempts: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactVerification {
    Metadata,
    FullHash,
}

pub struct ProcessingJournal {
    database: Database,
}

impl ProcessingJournal {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let database = Database::create(path)
            .with_context(|| format!("Open processing journal {}", path.display()))?;
        let transaction = database.begin_write()?;
        {
            let _ = transaction.open_table(GROUP_JOURNAL)?;
        }
        transaction.commit()?;
        Ok(Self { database })
    }

    pub fn get(&self, group_id: &str) -> Result<Option<GroupJournalEntry>> {
        validate_group_id(group_id)?;
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(GROUP_JOURNAL)?;
        let value = table.get(group_id)?;
        value
            .map(|value| serde_json::from_str(value.value()).map_err(Into::into))
            .transpose()
    }

    /// Claim a group for processing. A prior `Running` record is treated as a
    /// crash-interrupted attempt and is safely retried.
    pub fn claim(&self, group_id: &str, retry_limit: u32) -> Result<ClaimDecision> {
        validate_group_id(group_id)?;
        let transaction = self.database.begin_write()?;
        let decision;
        {
            let mut table = transaction.open_table(GROUP_JOURNAL)?;
            let existing = table
                .get(group_id)?
                .map(|value| serde_json::from_str::<GroupJournalEntry>(value.value()))
                .transpose()?;
            if let Some(existing) = existing.as_ref() {
                if existing.status == GroupProcessingStatus::Committed {
                    return Ok(ClaimDecision::AlreadyCommitted);
                }
                if existing.attempts >= retry_limit.max(1) {
                    return Ok(ClaimDecision::RetryLimitReached {
                        attempts: existing.attempts,
                    });
                }
            }
            let now = Utc::now();
            let attempt = existing
                .as_ref()
                .map(|entry| entry.attempts.saturating_add(1))
                .unwrap_or(1);
            let entry = GroupJournalEntry {
                group_id: group_id.to_string(),
                status: GroupProcessingStatus::Running,
                attempts: attempt,
                started_at: now,
                updated_at: now,
                duration_ms: None,
                artifacts: Vec::new(),
                last_error: None,
            };
            let json = serde_json::to_string(&entry)?;
            table.insert(group_id, json.as_str())?;
            decision = ClaimDecision::Process { attempt };
        }
        transaction.commit()?;
        Ok(decision)
    }

    pub fn mark_committed(
        &self,
        group_id: &str,
        duration_ms: u64,
        artifacts: Vec<ArtifactDigest>,
    ) -> Result<()> {
        self.update(group_id, |entry| {
            entry.status = GroupProcessingStatus::Committed;
            entry.updated_at = Utc::now();
            entry.duration_ms = Some(duration_ms);
            entry.artifacts = artifacts;
            entry.last_error = None;
        })
    }

    pub fn mark_failed(&self, group_id: &str, error: &anyhow::Error) -> Result<()> {
        self.update(group_id, |entry| {
            entry.status = GroupProcessingStatus::Failed;
            entry.updated_at = Utc::now();
            entry.last_error = Some(format!("{error:#}"));
        })
    }

    pub fn mark_interrupted(&self, group_id: &str, reason: &str) -> Result<()> {
        self.update(group_id, |entry| {
            entry.status = GroupProcessingStatus::Failed;
            entry.attempts = entry.attempts.saturating_sub(1);
            entry.updated_at = Utc::now();
            entry.last_error = Some(reason.to_string());
        })
    }

    pub fn invalidate_committed(&self, group_id: &str, reason: &str) -> Result<()> {
        self.update(group_id, |entry| {
            entry.status = GroupProcessingStatus::Failed;
            entry.updated_at = Utc::now();
            entry.last_error = Some(reason.to_string());
        })
    }

    pub fn verify_committed(&self, group_id: &str, output_root: &Path) -> Result<bool> {
        self.verify_committed_with(group_id, output_root, ArtifactVerification::FullHash)
    }

    pub fn verify_committed_with(
        &self,
        group_id: &str,
        output_root: &Path,
        verification: ArtifactVerification,
    ) -> Result<bool> {
        let Some(entry) = self.get(group_id)? else {
            return Ok(false);
        };
        if entry.status != GroupProcessingStatus::Committed || entry.artifacts.is_empty() {
            return Ok(false);
        }
        for artifact in &entry.artifacts {
            validate_relative_path(&artifact.relative_path)?;
            let path = output_root.join(&artifact.relative_path);
            let metadata = match std::fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => metadata,
                _ => return Ok(false),
            };
            if metadata.len() != artifact.size_bytes {
                return Ok(false);
            }
            let modified_unix_nanos = metadata_modified_unix_nanos(&metadata)?;
            if artifact.modified_unix_nanos != 0
                && modified_unix_nanos != artifact.modified_unix_nanos
            {
                return Ok(false);
            }
            if (verification == ArtifactVerification::FullHash || artifact.modified_unix_nanos == 0)
                && sha256_file(&path)? != artifact.sha256
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn update(&self, group_id: &str, update: impl FnOnce(&mut GroupJournalEntry)) -> Result<()> {
        validate_group_id(group_id)?;
        let transaction = self.database.begin_write()?;
        {
            let mut table = transaction.open_table(GROUP_JOURNAL)?;
            let mut entry = table
                .get(group_id)?
                .map(|value| serde_json::from_str::<GroupJournalEntry>(value.value()))
                .transpose()?
                .with_context(|| format!("Group {} was not claimed", group_id))?;
            update(&mut entry);
            let json = serde_json::to_string(&entry)?;
            table.insert(group_id, json.as_str())?;
        }
        transaction.commit()?;
        Ok(())
    }
}

pub fn digest_artifact(path: &Path, output_root: &Path) -> Result<ArtifactDigest> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("Read artifact {}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("Artifact is not a regular file: {}", path.display());
    }
    artifact_digest_from_parts(path, output_root, metadata.len(), sha256_file(path)?)
}

pub fn artifact_digest_from_parts(
    path: &Path,
    output_root: &Path,
    size_bytes: u64,
    sha256: String,
) -> Result<ArtifactDigest> {
    let relative_path = path
        .strip_prefix(output_root)
        .with_context(|| {
            format!(
                "Artifact {} is outside output root {}",
                path.display(),
                output_root.display()
            )
        })?
        .to_path_buf();
    validate_relative_path(&relative_path)?;
    let metadata =
        std::fs::metadata(path).with_context(|| format!("Read artifact {}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("Artifact is not a regular file: {}", path.display());
    }
    if metadata.len() != size_bytes {
        anyhow::bail!(
            "Encoded artifact size changed before commit: expected {}, found {}",
            size_bytes,
            metadata.len()
        );
    }
    Ok(ArtifactDigest {
        relative_path,
        size_bytes,
        sha256,
        modified_unix_nanos: metadata_modified_unix_nanos(&metadata)?,
    })
}

fn metadata_modified_unix_nanos(metadata: &std::fs::Metadata) -> Result<u64> {
    let duration = metadata
        .modified()
        .context("Read artifact modification time")?
        .duration_since(std::time::UNIX_EPOCH)
        .context("Artifact modification time predates Unix epoch")?;
    Ok(duration.as_nanos().min(u64::MAX as u128) as u64)
}

fn sha256_file(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("Hash artifact {}", path.display()))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn validate_group_id(group_id: &str) -> Result<()> {
    if group_id.len() == 64 && group_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        anyhow::bail!("Invalid capture group ID")
    }
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        anyhow::bail!("Artifact path must be non-empty and relative");
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        anyhow::bail!("Artifact path cannot traverse outside output root");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GROUP_ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn journal_recovers_running_work_and_verifies_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.redb");
        let journal = ProcessingJournal::open(&path).unwrap();
        assert_eq!(
            journal.claim(GROUP_ID, 3).unwrap(),
            ClaimDecision::Process { attempt: 1 }
        );
        drop(journal);
        let journal = ProcessingJournal::open(&path).unwrap();
        assert_eq!(
            journal.claim(GROUP_ID, 3).unwrap(),
            ClaimDecision::Process { attempt: 2 }
        );

        let artifact = directory.path().join("result.bin");
        std::fs::write(&artifact, b"committed").unwrap();
        let digest = digest_artifact(&artifact, directory.path()).unwrap();
        journal.mark_committed(GROUP_ID, 42, vec![digest]).unwrap();
        drop(journal);
        let journal = ProcessingJournal::open(&path).unwrap();
        assert!(journal
            .verify_committed(GROUP_ID, directory.path())
            .unwrap());
        assert!(journal
            .verify_committed_with(GROUP_ID, directory.path(), ArtifactVerification::Metadata,)
            .unwrap());
        assert_eq!(
            journal.claim(GROUP_ID, 3).unwrap(),
            ClaimDecision::AlreadyCommitted
        );

        std::fs::write(&artifact, b"corrupt").unwrap();
        assert!(!journal
            .verify_committed(GROUP_ID, directory.path())
            .unwrap());
    }

    #[test]
    fn retry_limit_is_persisted() {
        let directory = tempfile::tempdir().unwrap();
        let journal = ProcessingJournal::open(&directory.path().join("journal.redb")).unwrap();
        assert!(matches!(
            journal.claim(GROUP_ID, 1).unwrap(),
            ClaimDecision::Process { attempt: 1 }
        ));
        journal
            .mark_failed(GROUP_ID, &anyhow::anyhow!("decode failed"))
            .unwrap();
        assert_eq!(
            journal.claim(GROUP_ID, 1).unwrap(),
            ClaimDecision::RetryLimitReached { attempts: 1 }
        );
    }

    #[test]
    fn operator_cancellation_does_not_consume_retry_budget() {
        let directory = tempfile::tempdir().unwrap();
        let journal = ProcessingJournal::open(&directory.path().join("journal.redb")).unwrap();
        assert!(matches!(
            journal.claim(GROUP_ID, 1).unwrap(),
            ClaimDecision::Process { attempt: 1 }
        ));
        journal
            .mark_interrupted(GROUP_ID, "operator cancellation")
            .unwrap();
        assert_eq!(
            journal.claim(GROUP_ID, 1).unwrap(),
            ClaimDecision::Process { attempt: 1 }
        );
    }
}
