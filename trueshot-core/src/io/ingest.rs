use crate::scanning::session::ScanSession;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::fs;
use std::path::Path;

pub struct IngestionManager;

impl IngestionManager {
    /// Ingest images from an SD card based on a ScanSession log.
    /// Renames detailed images to `angle_{A}_cam_{N}.ext`.
    pub fn ingest(session: &ScanSession, card_path: &Path, output_dir: &Path) -> Result<usize> {
        if !output_dir.exists() {
            fs::create_dir_all(output_dir)?;
        }

        // 1. Gather all candidate files from card
        let mut candidates = Vec::new();
        for entry in fs::read_dir(card_path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy().to_lowercase();
                    if matches!(
                        ext_str.as_str(),
                        "jpg" | "jpeg" | "nef" | "cr2" | "arw" | "dng"
                    ) {
                        let metadata = fs::metadata(&path)?;
                        let created = metadata.created().unwrap_or(metadata.modified()?);
                        let created_utc: DateTime<Utc> = DateTime::from(created);
                        candidates.push((path, created_utc));
                    }
                }
            }
        }

        // Sort by time
        candidates.sort_by_key(|k| k.1);

        let mut copied_count = 0;

        // 2. Iterate Sequence
        for event in &session.events {
            // Filter for HighRes events
            // In Director we logged "DSLR_Group".
            if !event.cameras.contains(&"DSLR_Group".to_string()) {
                continue;
            }

            // Find candidates near event timestamp
            // Tolerance window: e.g. -2s to +5s (camera clock might drift, or write delay)
            // But relative ordering should be preserved.
            // Matching "bursts" is harder if clocks are widely offset.
            // Assumption: User synchronized clocks or we depend on relative gaps.
            // Simple approach: Clocks are within 30s.

            // We use a sliding window logic or just simple distance check.
            let target_time = event.timestamp;

            // Find ALL files within window [T-5s, T+10s]
            let mut matches = Vec::new();
            for (path, time) in &candidates {
                let diff = (*time - target_time).num_seconds().abs();
                if diff < 10 {
                    matches.push(path.clone());
                }
            }

            // Refine: We expect `event.file_count_expected` files.
            // If we found more, maybe multiple cameras?
            // If we have 1 event, we take the closest ones?
            // This naive matching is risky if shots are fast.
            // Better: Consume candidates from the list.

            // BUT: `candidates` loop is inefficient if we restart every time.
            // Optimization: Maintain an index.

            // Let's assume sequential shooting.
            // We just peel off the first `file_count_expected` candidates that are "after" the previous event's last file time?
            // Safer: Just proximity for now.

            for (i, src_path) in matches.iter().take(event.file_count_expected).enumerate() {
                let ext = src_path.extension().unwrap_or_default().to_string_lossy();
                let filename = format!("angle_{}_cam_{}.{}", event.turntable_angle, i, ext);
                let dest_path = output_dir.join(filename);

                if !dest_path.exists() {
                    fs::copy(src_path, &dest_path).context("Failed to copy image")?;
                    copied_count += 1;
                }
            }
        }

        Ok(copied_count)
    }
}
