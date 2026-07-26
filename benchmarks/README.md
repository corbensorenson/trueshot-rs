# TrueShot Benchmarks

This folder defines the benchmark KPIs, datasets, and runner conventions used to measure
core pipeline quality and performance. The goal is to ship releases with **measurable**
quality deltas, not subjective claims.

## KPIs (Release Gate Targets)

### Reconstruction Fidelity
- `chamfer_distance_mm` (lower is better)
- `psnr_db` and `ssim` on reprojections (higher is better)
- `geometric_completeness_pct` (higher is better)
- `segmentation_iou` and `segmentation_dice` when GT masks are provided

### Temporal Stability
- `point_drift_mm_per_min`
- `frame_to_frame_jitter_mm`
- `pose_consistency_deg`

### Latency & Throughput
- `capture_to_preview_ms`
- `capture_to_recon_ms`
- `frames_per_second`
- `gpu_utilization_pct`, `cpu_utilization_pct`, `ram_mb`

### Failure Rate & Robustness
- `pipeline_failure_rate_pct`
- `alignment_retry_rate_pct`
- `outlier_rejection_pct`

## Dataset Manifest

Datasets are described in `benchmarks/datasets/manifest.example.json` and validated
against `benchmarks/datasets/manifest.schema.json`.

Each dataset entry should include:
- `id`, `name`, `version`
- `modality` and `capture_protocol`
- `license`, `checksum_sha256`, `size_bytes`
- `download_url` and optional `mirrors`
- `expected_metrics` if known

## Running Benchmarks

Run all Criterion benches:

```bash
./scripts/benchmarks/run_benchmarks.sh
```

Results are stored in `benchmarks/results/` with a timestamped folder.

## Release Notes Policy

Every release should include:
- Dataset versions used
- KPI deltas versus previous release
- Notes on regressions or invalidated datasets


## Local RealTest NEF Benchmark

If you have local NEF captures under `realTest/`, you can run:

```bash
./scripts/benchmarks/run_realtest.sh
```

This writes a JSON report to `benchmarks/results/realtest_<timestamp>.json` with:
- frame counts
- per-sequence load timings
- object bbox coverage (from preview-based detection)

Optional: provide a ground-truth image directory to compute PSNR/SSIM on preview JPEGs:

```bash
cargo run --example realtest_benchmark -- realTest --gt benchmarks/datasets/ground_truth --out benchmarks/results/realtest_gt.json
```

Or via script:

```bash
REALTEST_GT_DIR=benchmarks/datasets/ground_truth ./scripts/benchmarks/run_realtest.sh
```

Optional: provide predicted and ground-truth mesh directories to compute Chamfer distance:

```bash
cargo run --example realtest_benchmark -- realTest \
  --pred-mesh benchmarks/datasets/pred_mesh \
  --gt-mesh benchmarks/datasets/gt_mesh \
  --out benchmarks/results/realtest_mesh.json
```

Enable CI gating when a baseline exists:

```bash
REALTEST_CI_GATE=1 ./scripts/benchmarks/run_realtest.sh
```

Optional: provide GT masks and a segmentation model to compute IoU/Dice:

```bash
REALTEST_GT_MASK_DIR=benchmarks/datasets/gt_masks \
REALTEST_SEG_MODEL=benchmarks/datasets/segmentation_model.onnx \
./scripts/benchmarks/run_realtest.sh
```

Bootstrap GT masks from the realTest NEFs (heuristic by default, or provide a model):

```bash
cargo run -p trueshot-core --example bootstrap_gt_masks -- realTest benchmarks/datasets/gt_masks
# Optional: --seg-model benchmarks/datasets/segmentation_model.onnx
```


## Compare Benchmark Runs

You can compare two JSON runs (baseline vs current) with:

```bash
node ./scripts/benchmarks/compare_results.js <baseline.json> <current.json>
```

To emit JSON output:

```bash
node ./scripts/benchmarks/compare_results.js <baseline.json> <current.json> --json --out benchmarks/results/compare.json
```

## Generate Release Notes Snippet

Once you have a baseline and current JSON run, you can generate a release-notes section:

```bash
node ./scripts/benchmarks/generate_release_notes.js <baseline.json> <current.json> \
  --datasets benchmarks/datasets/manifest.example.json \
  --out benchmarks/results/release_notes.md
```
