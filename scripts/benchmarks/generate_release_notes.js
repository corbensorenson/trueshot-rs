#!/usr/bin/env node

const fs = require('fs');
const path = require('path');

function usage() {
  console.error('usage: generate_release_notes.js <baseline.json> <current.json> [--datasets <manifest.json>] [--out <path>]');
  process.exit(1);
}

function loadJson(p) {
  return JSON.parse(fs.readFileSync(p, 'utf8'));
}

function formatDelta(value, digits = 2) {
  if (value === null || value === undefined) return 'n/a';
  const sign = value > 0 ? '+' : '';
  return `${sign}${value.toFixed(digits)}`;
}

function summarize(run) {
  const sequences = run.sequences || [];
  const loadTotals = sequences.map(s => s.load_total_ms || 0);
  const seqCount = sequences.length;
  const totalFrames = run.total_frames_loaded || 0;
  const meanLoadMs = seqCount > 0 ? loadTotals.reduce((a, b) => a + b, 0) / seqCount : 0;

  return {
    datasetPath: run.dataset_path || null,
    timestamp: run.timestamp_utc || null,
    seqCount,
    totalFrames,
    bboxCoveragePct: run.bbox_coverage_pct ?? null,
    psnrDb: run.psnr_db ?? null,
    ssim: run.ssim ?? null,
    chamfer: run.chamfer ?? null,
    hausdorff: run.hausdorff ?? null,
    fscore: run.fscore ?? null,
    precision: run.precision ?? null,
    recall: run.recall ?? null,
    normalConsistency: run.normal_consistency ?? null,
    segIou: run.seg_iou ?? null,
    segDice: run.seg_dice ?? null,
    gtMatches: run.gt_matches ?? null,
    meshMatches: run.mesh_matches ?? null,
    segMatches: run.seg_matches ?? null,
    meanLoadMs,
  };
}

function diffNumber(current, baseline) {
  if (baseline === null || baseline === undefined) return null;
  if (current === null || current === undefined) return null;
  return current - baseline;
}

function renderDatasets(manifest) {
  if (!manifest || !manifest.datasets || manifest.datasets.length === 0) return 'No dataset manifest provided.';
  return manifest.datasets.map(ds => {
    const lines = [];
    lines.push(
      '- **' +
        ds.name +
        '** (id: `' +
        ds.id +
        '`, version: `' +
        ds.version +
        '`, modality: `' +
        ds.modality +
        '`, license: `' +
        ds.license +
        '`)'
    );
    if (ds.capture_protocol) lines.push(`  - protocol: ${ds.capture_protocol}`);
    if (ds.download_url) lines.push(`  - source: ${ds.download_url}`);
    return lines.join('\n');
  }).join('\n');
}

function main() {
  const args = process.argv.slice(2);
  if (args.length < 2) usage();

  const baselinePath = args[0];
  const currentPath = args[1];
  let datasetManifestPath = null;
  let outPath = null;

  for (let i = 2; i < args.length; i += 1) {
    if (args[i] === '--datasets') {
      datasetManifestPath = args[i + 1];
      i += 1;
    } else if (args[i] === '--out') {
      outPath = args[i + 1];
      i += 1;
    }
  }

  const baselineRaw = loadJson(baselinePath);
  const currentRaw = loadJson(currentPath);

  const baseline = summarize(baselineRaw);
  const current = summarize(currentRaw);

  const deltas = {
    seqCount: diffNumber(current.seqCount, baseline.seqCount),
    totalFrames: diffNumber(current.totalFrames, baseline.totalFrames),
    bboxCoveragePct: diffNumber(current.bboxCoveragePct, baseline.bboxCoveragePct),
    psnrDb: diffNumber(current.psnrDb, baseline.psnrDb),
    ssim: diffNumber(current.ssim, baseline.ssim),
    chamfer: diffNumber(current.chamfer, baseline.chamfer),
    hausdorff: diffNumber(current.hausdorff, baseline.hausdorff),
    fscore: diffNumber(current.fscore, baseline.fscore),
    precision: diffNumber(current.precision, baseline.precision),
    recall: diffNumber(current.recall, baseline.recall),
    normalConsistency: diffNumber(current.normalConsistency, baseline.normalConsistency),
    segIou: diffNumber(current.segIou, baseline.segIou),
    segDice: diffNumber(current.segDice, baseline.segDice),
    gtMatches: diffNumber(current.gtMatches, baseline.gtMatches),
    meshMatches: diffNumber(current.meshMatches, baseline.meshMatches),
    segMatches: diffNumber(current.segMatches, baseline.segMatches),
    meanLoadMs: diffNumber(current.meanLoadMs, baseline.meanLoadMs),
  };

  let datasetManifest = null;
  if (datasetManifestPath) {
    datasetManifest = loadJson(datasetManifestPath);
  }

  const lines = [];
  lines.push(`# Release Benchmarks`);
  lines.push('');
  lines.push('Baseline: ' + baselinePath);
  lines.push('Current : ' + currentPath);
  lines.push('');
  lines.push('## Dataset Summary');
  lines.push('');
  lines.push(renderDatasets(datasetManifest));
  lines.push('');
  lines.push('## KPI Deltas');
  lines.push('');
  lines.push(`- Sequences: ${current.seqCount} (${formatDelta(deltas.seqCount, 0)})`);
  lines.push(`- Frames: ${current.totalFrames} (${formatDelta(deltas.totalFrames, 0)})`);
  lines.push(`- BBox Coverage %: ${current.bboxCoveragePct ?? 'n/a'} (${formatDelta(deltas.bboxCoveragePct)})`);
  lines.push(`- PSNR (preview, dB): ${current.psnrDb ?? 'n/a'} (${formatDelta(deltas.psnrDb)})`);
  lines.push(`- SSIM (preview): ${current.ssim ?? 'n/a'} (${formatDelta(deltas.ssim)})`);
  lines.push(`- Chamfer (mesh): ${current.chamfer ?? 'n/a'} (${formatDelta(deltas.chamfer)})`);
  lines.push(`- Hausdorff (mesh): ${current.hausdorff ?? 'n/a'} (${formatDelta(deltas.hausdorff)})`);
  lines.push(`- F-score (mesh): ${current.fscore ?? 'n/a'} (${formatDelta(deltas.fscore, 4)})`);
  lines.push(`- Precision (mesh): ${current.precision ?? 'n/a'} (${formatDelta(deltas.precision, 4)})`);
  lines.push(`- Recall (mesh): ${current.recall ?? 'n/a'} (${formatDelta(deltas.recall, 4)})`);
  lines.push(`- Normal Consistency: ${current.normalConsistency ?? 'n/a'} (${formatDelta(deltas.normalConsistency, 4)})`);
  lines.push(`- Segmentation IoU: ${current.segIou ?? 'n/a'} (${formatDelta(deltas.segIou, 4)})`);
  lines.push(`- Segmentation Dice: ${current.segDice ?? 'n/a'} (${formatDelta(deltas.segDice, 4)})`);
  lines.push(`- GT Matches: ${current.gtMatches ?? 'n/a'} (${formatDelta(deltas.gtMatches, 0)})`);
  lines.push(`- Mesh Matches: ${current.meshMatches ?? 'n/a'} (${formatDelta(deltas.meshMatches, 0)})`);
  lines.push(`- Seg Matches: ${current.segMatches ?? 'n/a'} (${formatDelta(deltas.segMatches, 0)})`);
  lines.push(`- Mean Load ms: ${current.meanLoadMs.toFixed(2)} (${formatDelta(deltas.meanLoadMs)})`);
  lines.push('');
  lines.push('## Notes');
  lines.push('- Replace this section with release-specific insights.');
  lines.push('- If any KPI regresses, include mitigation or investigation notes.');

  const content = lines.join('\n');

  if (outPath) {
    fs.mkdirSync(path.dirname(outPath), { recursive: true });
    fs.writeFileSync(outPath, content);
    console.log(`Wrote release notes to ${outPath}`);
  } else {
    console.log(content);
  }
}

main();
