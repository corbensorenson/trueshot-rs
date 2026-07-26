#!/usr/bin/env node

const fs = require('fs');
const path = require('path');

function usage() {
  console.error('usage: compare_results.js <baseline.json> <current.json> [--json] [--out <path>]');
  process.exit(1);
}

function loadJson(p) {
  const raw = fs.readFileSync(p, 'utf8');
  return JSON.parse(raw);
}

function sum(arr) {
  return arr.reduce((a, b) => a + b, 0);
}

function summarize(run) {
  const sequences = run.sequences || [];
  const loadTotals = sequences.map(s => s.load_total_ms || 0);
  const seqCount = sequences.length;
  const totalFrames = run.total_frames_loaded || 0;

  const meanLoadMs = seqCount > 0 ? sum(loadTotals) / seqCount : 0;
  const minLoadMs = seqCount > 0 ? Math.min(...loadTotals) : 0;
  const maxLoadMs = seqCount > 0 ? Math.max(...loadTotals) : 0;
  const sumLoadMs = sum(loadTotals);

  const timingAgg = {};
  for (const seq of sequences) {
    const timings = seq.timings || {};
    for (const [label, stats] of Object.entries(timings)) {
      if (!timingAgg[label]) {
        timingAgg[label] = { totalSamples: 0, weightedMeanMs: 0, minMs: null, maxMs: null };
      }
      const count = stats.count || 0;
      const meanMs = stats.mean_ms || 0;
      timingAgg[label].weightedMeanMs += meanMs * count;
      timingAgg[label].totalSamples += count;
      const minMs = stats.min_ms ?? null;
      const maxMs = stats.max_ms ?? null;
      if (minMs !== null) {
        timingAgg[label].minMs = timingAgg[label].minMs === null ? minMs : Math.min(timingAgg[label].minMs, minMs);
      }
      if (maxMs !== null) {
        timingAgg[label].maxMs = timingAgg[label].maxMs === null ? maxMs : Math.max(timingAgg[label].maxMs, maxMs);
      }
    }
  }

  for (const label of Object.keys(timingAgg)) {
    const entry = timingAgg[label];
    if (entry.totalSamples > 0) {
      entry.weightedMeanMs = entry.weightedMeanMs / entry.totalSamples;
    }
  }

  return {
    datasetPath: run.dataset_path || null,
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
    minLoadMs,
    maxLoadMs,
    sumLoadMs,
    timings: timingAgg,
  };
}

function diffNumber(current, baseline) {
  if (baseline === null || baseline === undefined) return null;
  return current - baseline;
}

function formatDelta(value, digits = 2) {
  if (value === null) return 'n/a';
  const sign = value > 0 ? '+' : '';
  return `${sign}${value.toFixed(digits)}`;
}

function main() {
  const args = process.argv.slice(2);
  if (args.length < 2) usage();

  const baselinePath = args[0];
  const currentPath = args[1];

  let outputJson = false;
  let outPath = null;

  for (let i = 2; i < args.length; i += 1) {
    if (args[i] === '--json') {
      outputJson = true;
    } else if (args[i] === '--out') {
      outPath = args[i + 1];
      i += 1;
    }
  }

  const baseline = summarize(loadJson(baselinePath));
  const current = summarize(loadJson(currentPath));

  const comparison = {
    baseline: baselinePath,
    current: currentPath,
    deltas: {
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
      minLoadMs: diffNumber(current.minLoadMs, baseline.minLoadMs),
      maxLoadMs: diffNumber(current.maxLoadMs, baseline.maxLoadMs),
      sumLoadMs: diffNumber(current.sumLoadMs, baseline.sumLoadMs),
    },
    timingDeltas: {},
  };

  const labels = new Set([...Object.keys(baseline.timings), ...Object.keys(current.timings)]);
  for (const label of labels) {
    const cur = current.timings[label]?.weightedMeanMs ?? null;
    const base = baseline.timings[label]?.weightedMeanMs ?? null;
    comparison.timingDeltas[label] = diffNumber(cur ?? null, base ?? null);
  }

  if (outputJson) {
    const payload = JSON.stringify({ baseline, current, comparison }, null, 2);
    if (outPath) {
      fs.mkdirSync(path.dirname(outPath), { recursive: true });
      fs.writeFileSync(outPath, payload);
      console.log(`Wrote comparison to ${outPath}`);
    } else {
      console.log(payload);
    }
    return;
  }

  console.log('=== Benchmark Comparison ===');
  console.log(`Baseline: ${baselinePath}`);
  console.log(`Current : ${currentPath}`);
  console.log('');
  console.log(`Sequences: ${current.seqCount} (${formatDelta(comparison.deltas.seqCount, 0)})`);
  console.log(`Frames   : ${current.totalFrames} (${formatDelta(comparison.deltas.totalFrames, 0)})`);
  console.log(`BBox %   : ${current.bboxCoveragePct ?? 'n/a'} (${formatDelta(comparison.deltas.bboxCoveragePct)})`);
  console.log(`PSNR dB  : ${current.psnrDb ?? 'n/a'} (${formatDelta(comparison.deltas.psnrDb)})`);
  console.log(`SSIM     : ${current.ssim ?? 'n/a'} (${formatDelta(comparison.deltas.ssim)})`);
  console.log(`Chamfer  : ${current.chamfer ?? 'n/a'} (${formatDelta(comparison.deltas.chamfer)})`);
  console.log(`Hausdorff: ${current.hausdorff ?? 'n/a'} (${formatDelta(comparison.deltas.hausdorff)})`);
  console.log(`F-score  : ${current.fscore ?? 'n/a'} (${formatDelta(comparison.deltas.fscore, 4)})`);
  console.log(`Precision: ${current.precision ?? 'n/a'} (${formatDelta(comparison.deltas.precision, 4)})`);
  console.log(`Recall   : ${current.recall ?? 'n/a'} (${formatDelta(comparison.deltas.recall, 4)})`);
  console.log(`Normals  : ${current.normalConsistency ?? 'n/a'} (${formatDelta(comparison.deltas.normalConsistency, 4)})`);
  console.log(`Seg IoU  : ${current.segIou ?? 'n/a'} (${formatDelta(comparison.deltas.segIou, 4)})`);
  console.log(`Seg Dice : ${current.segDice ?? 'n/a'} (${formatDelta(comparison.deltas.segDice, 4)})`);
  console.log(`GT Match : ${current.gtMatches ?? 'n/a'} (${formatDelta(comparison.deltas.gtMatches, 0)})`);
  console.log(`Mesh GT  : ${current.meshMatches ?? 'n/a'} (${formatDelta(comparison.deltas.meshMatches, 0)})`);
  console.log(`Seg GT   : ${current.segMatches ?? 'n/a'} (${formatDelta(comparison.deltas.segMatches, 0)})`);
  console.log('');
  console.log(`Load ms  : mean ${current.meanLoadMs.toFixed(2)} (${formatDelta(comparison.deltas.meanLoadMs)})`);
  console.log(`           min ${current.minLoadMs.toFixed(2)} (${formatDelta(comparison.deltas.minLoadMs)})`);
  console.log(`           max ${current.maxLoadMs.toFixed(2)} (${formatDelta(comparison.deltas.maxLoadMs)})`);
  console.log(`           sum ${current.sumLoadMs.toFixed(2)} (${formatDelta(comparison.deltas.sumLoadMs)})`);

  const timingLabels = Object.keys(comparison.timingDeltas).sort();
  if (timingLabels.length > 0) {
    console.log('');
    console.log('Timing labels (weighted mean ms):');
    for (const label of timingLabels) {
      const cur = current.timings[label]?.weightedMeanMs ?? null;
      const delta = comparison.timingDeltas[label];
      const curStr = cur === null ? 'n/a' : cur.toFixed(2);
      console.log(`  ${label}: ${curStr} (${formatDelta(delta)})`);
    }
  }
}

main();
