use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use trueshot_core::nef::parser::Z9NefParser;
use trueshot_core::nef::raw_data::Roi;

fn main() -> Result<()> {
    let source = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: nef_corruption_runner <image.nef>")?;
    run(&source)
}

fn run(source: &Path) -> Result<()> {
    let original_len = std::fs::metadata(source)
        .with_context(|| format!("Read source NEF {}", source.display()))?
        .len();
    if original_len < 8192 {
        anyhow::bail!("Source NEF is too small for corruption testing");
    }
    let directory = tempfile::tempdir()?;
    let candidate = directory.path().join("corrupt.nef");
    std::fs::copy(source, &candidate)?;

    let truncation_lengths = [
        original_len.saturating_sub(1),
        original_len * 3 / 4,
        original_len / 2,
        1024 * 1024,
        4096,
        0,
    ];
    let mut rejected_truncations = 0usize;
    for length in truncation_lengths {
        OpenOptions::new()
            .write(true)
            .open(&candidate)?
            .set_len(length.min(original_len))?;
        match guarded_probe(&candidate) {
            ProbeResult::Rejected => rejected_truncations += 1,
            ProbeResult::Decoded => {
                if length <= original_len / 2 {
                    anyhow::bail!(
                        "Severely truncated NEF decoded instead of failing closed at {} bytes",
                        length
                    );
                }
            }
            ProbeResult::Panicked => {
                anyhow::bail!("NEF parser panicked on truncation at {} bytes", length)
            }
        }
    }
    if rejected_truncations < 4 {
        anyhow::bail!(
            "Only {rejected_truncations} of {} truncations were rejected",
            truncation_lengths.len()
        );
    }

    std::fs::copy(source, &candidate)?;
    let mut original_header = [0u8; 8];
    OpenOptions::new()
        .read(true)
        .open(source)?
        .read_exact(&mut original_header)?;
    for offset in 0..original_header.len() {
        let mut file = OpenOptions::new().read(true).write(true).open(&candidate)?;
        file.seek(SeekFrom::Start(offset as u64))?;
        file.write_all(&[original_header[offset] ^ 0xff])?;
        file.sync_all()?;
        drop(file);
        match guarded_probe(&candidate) {
            ProbeResult::Rejected => {}
            ProbeResult::Decoded => {
                anyhow::bail!("Mutated critical TIFF header byte {offset} was accepted")
            }
            ProbeResult::Panicked => {
                anyhow::bail!("NEF parser panicked on TIFF header mutation {offset}")
            }
        }
        let mut file = OpenOptions::new().write(true).open(&candidate)?;
        file.seek(SeekFrom::Start(offset as u64))?;
        file.write_all(&[original_header[offset]])?;
    }

    println!(
        "source_bytes={original_len} truncations={} rejected={} header_mutations={} panics=0",
        truncation_lengths.len(),
        rejected_truncations,
        original_header.len()
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeResult {
    Rejected,
    Decoded,
    Panicked,
}

fn guarded_probe(path: &Path) -> ProbeResult {
    match catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        let mut parser = Z9NefParser::new(path);
        parser.parse()?;
        let metadata = parser.get_metadata()?;
        let width = metadata.width.min(16);
        let height = metadata.height.min(16);
        if width == 0 || height == 0 {
            anyhow::bail!("NEF reports empty dimensions");
        }
        let roi = Roi::new(
            metadata.width.saturating_sub(width) / 2,
            metadata.height.saturating_sub(height) / 2,
            width,
            height,
        );
        let mut output = vec![0u16; roi.area() as usize];
        parser.load_roi_into(&roi, &mut output)?;
        Ok(())
    })) {
        Err(_) => ProbeResult::Panicked,
        Ok(Err(_)) => ProbeResult::Rejected,
        Ok(Ok(())) => ProbeResult::Decoded,
    }
}
