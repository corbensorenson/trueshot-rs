#!/usr/bin/env node

const fs = require('fs');

function usage() {
  console.error('usage: ci_gate.js <baseline.json> <current.json> [--max-psnr-drop <val>] [--max-ssim-drop <val>] [--max-chamfer-increase <val>] [--max-hausdorff-increase <val>] [--max-fscore-drop <val>] [--max-normal-drop <val>] [--max-iou-drop <val>] [--max-dice-drop <val>]');
  process.exit(1);
}

function loadJson(path) {
  return JSON.parse(fs.readFileSync(path, 'utf8'));
}

function getArg(args, name, fallback) {
  const idx = args.indexOf(name);
  if (idx !== -1 && idx + 1 < args.length) {
    return parseFloat(args[idx + 1]);
  }
  return fallback;
}

function diff(current, baseline) {
  if (current === null || current === undefined) return null;
  if (baseline === null || baseline === undefined) return null;
  return current - baseline;
}

function main() {
  const args = process.argv.slice(2);
  if (args.length < 2) usage();

  const baseline = loadJson(args[0]);
  const current = loadJson(args[1]);

  const maxPsnrDrop = getArg(args, '--max-psnr-drop', parseFloat(process.env.CI_MAX_PSNR_DROP || '0.1'));
  const maxSsimDrop = getArg(args, '--max-ssim-drop', parseFloat(process.env.CI_MAX_SSIM_DROP || '0.005'));
  const maxChamferIncrease = getArg(args, '--max-chamfer-increase', parseFloat(process.env.CI_MAX_CHAMFER_INCREASE || '0.5'));
  const maxHausdorffIncrease = getArg(args, '--max-hausdorff-increase', parseFloat(process.env.CI_MAX_HAUSDORFF_INCREASE || '0.5'));
  const maxFscoreDrop = getArg(args, '--max-fscore-drop', parseFloat(process.env.CI_MAX_FSCORE_DROP || '0.02'));
  const maxNormalDrop = getArg(args, '--max-normal-drop', parseFloat(process.env.CI_MAX_NORMAL_DROP || '0.02'));
  const maxIouDrop = getArg(args, '--max-iou-drop', parseFloat(process.env.CI_MAX_IOU_DROP || '0.02'));
  const maxDiceDrop = getArg(args, '--max-dice-drop', parseFloat(process.env.CI_MAX_DICE_DROP || '0.02'));

  const psnrDelta = diff(current.psnr_db, baseline.psnr_db);
  const ssimDelta = diff(current.ssim, baseline.ssim);
  const chamferDelta = diff(current.chamfer, baseline.chamfer);
  const hausdorffDelta = diff(current.hausdorff, baseline.hausdorff);
  const fscoreDelta = diff(current.fscore, baseline.fscore);
  const normalDelta = diff(current.normal_consistency, baseline.normal_consistency);
  const iouDelta = diff(current.seg_iou, baseline.seg_iou);
  const diceDelta = diff(current.seg_dice, baseline.seg_dice);

  const failures = [];
  if (psnrDelta !== null && psnrDelta < -maxPsnrDrop) {
    failures.push(`PSNR dropped by ${psnrDelta.toFixed(4)} (max allowed drop ${maxPsnrDrop})`);
  }
  if (ssimDelta !== null && ssimDelta < -maxSsimDrop) {
    failures.push(`SSIM dropped by ${ssimDelta.toFixed(6)} (max allowed drop ${maxSsimDrop})`);
  }
  if (chamferDelta !== null && chamferDelta > maxChamferIncrease) {
    failures.push(`Chamfer increased by ${chamferDelta.toFixed(6)} (max allowed increase ${maxChamferIncrease})`);
  }
  if (hausdorffDelta !== null && hausdorffDelta > maxHausdorffIncrease) {
    failures.push(`Hausdorff increased by ${hausdorffDelta.toFixed(6)} (max allowed increase ${maxHausdorffIncrease})`);
  }
  if (fscoreDelta !== null && fscoreDelta < -maxFscoreDrop) {
    failures.push(`F-score dropped by ${fscoreDelta.toFixed(6)} (max allowed drop ${maxFscoreDrop})`);
  }
  if (normalDelta !== null && normalDelta < -maxNormalDrop) {
    failures.push(`Normal consistency dropped by ${normalDelta.toFixed(6)} (max allowed drop ${maxNormalDrop})`);
  }
  if (iouDelta !== null && iouDelta < -maxIouDrop) {
    failures.push(`Seg IoU dropped by ${iouDelta.toFixed(6)} (max allowed drop ${maxIouDrop})`);
  }
  if (diceDelta !== null && diceDelta < -maxDiceDrop) {
    failures.push(`Seg Dice dropped by ${diceDelta.toFixed(6)} (max allowed drop ${maxDiceDrop})`);
  }

  if (failures.length > 0) {
    console.error('Benchmark gate failed:');
    for (const failure of failures) {
      console.error(`- ${failure}`);
    }
    process.exit(1);
  }

  console.log('Benchmark gate passed.');
}

main();
