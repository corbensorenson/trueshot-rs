# NEF ROI Architecture

Date: 2026-07-26

## Production Default

TrueShot uses a group-amortized, sidecar-free path for Nikon HDR/focus stacks:

1. Group frames in acquisition order as `F1E1..FnEm`.
2. Select `FnEm`, the furthest-focus frame at the longest exposure.
3. Extract that frame's embedded JPEG preview once.
4. Use scaled JPEG IDCT to produce a small luma preview without a full RGB expansion.
5. Detect and pad one Bayer-aligned bounding box.
6. Reuse that immutable crop for every frame in the group; native crop-only workflows call `load_nef_roi_native`, while grouped workflows call `SmartLoader::load_sequence_native_into`.
7. Reserve one reusable contiguous `u16` group arena and assign every source an immutable ordered slot.
8. Memory-map each NEF read-only and decode directly into its slot without allocating a per-frame `RawBuffer`.
9. Fail the complete group with the original frame index and path if any slot cannot be decoded; never shift focus/exposure semantics by silently dropping a frame.

## Why Default Decoding Is Sequential

Nikon compression `34713` stores a variable-length predictive entropy stream. The byte offset and predictor state for an arbitrary row cannot be inferred from image coordinates. An exact first read must consume preceding entropy symbols.

The sidecar-free decoder performs the minimum legal work:

- It reconstructs only the first two vertical predictors on rows before the ROI.
- It skips residual payload bits without calculating discarded differences.
- It reconstructs horizontal predictors only through the ROI's right edge.
- It applies the linearization curve and writes pixels only inside the ROI.
- It stops immediately after the final ROI pixel.

This makes CPU time depend primarily on the crop's bottom edge, while allocation depends only on crop area.

## Repeated Interactive Access

`TRUESHOT_NEF_ACCESS_MODE=indexed` explicitly enables persistent entropy checkpoints. This mode is not for bulk ingestion. A tested Z9 index is approximately 2.8 MB per NEF and has a substantial cold-build cost, but repeated random crops can be much faster.

Use indexed mode only when the same asset will be cropped repeatedly enough to amortize index creation and storage. The default `stream` mode never creates index files.

## Scale Rules

- Detect once per HDR/focus group, never once per frame.
- Preserve native `u16` CFA data until downstream processing needs normalization.
- Bound files in flight by memory and storage throughput, not only CPU count.
- Commit outputs idempotently at group boundaries.
- Persist progress and failed-file diagnostics so million-file jobs can resume safely.
- Track p50, p95, and p99 detection/decode/write latency separately.

The production burst path enforces these rules:

- Capture uses a bounded JSONL manifest with one validated header and one
  content-addressed record per group. `CaptureManifestWriter` appends records
  while photographs are taken and atomically publishes only a complete
  manifest. Frame order (`frame`, `focus`, `exposure`, `burst`, `reference`) is
  explicit and exactly one reference frame is required.
- Legacy directories remain supported, but manifests are the scale path. The
  reader reuses a bounded line buffer, rejects records over 16 MiB and prevents
  path traversal.
- Each group obtains byte-denominated memory credits before decode. Credits
  cover the native arena, tiled fusion workspaces, post-processing arrays and
  the queued export, and remain held until export drops those arrays.
- Native `u16` CFA values flow directly into tiled `f32` HDR/focus fusion.
  Exposure, white balance and accepted subpixel transforms are sampled lazily;
  only compact green-plane alignment analysis uses `f64`.
- Automatic decoder concurrency reacts conservatively to decode throughput,
  writer backpressure, available memory and major page faults. An explicit
  `--jobs` setting disables adaptation.
- A one-deep asynchronous writer overlaps the next decode/fusion group without
  allowing unbounded output queues. TIFF strips are encoded directly from
  `f32`, hashes are calculated over the final byte stream and same-directory
  temporary files are synced before atomic rename.
- Stable output names and a durable REDB journal make every group idempotent.
  Running work is reclaimed after a crash, corrupt committed artifacts are
  rebuilt, failures retain diagnostics and operator cancellation does not
  consume the retry budget.
- Archival RGBA16 TIFF plus a bounded PNG preview is the default. Use `--depth`
  for a depth TIFF and `--full-resolution-preview` only when a full-size PNG is
  required.
- Fused RGB images can pass to SfM/MVS as shared in-memory `Arc<RgbImage>`
  buffers. Focus-stack intermediates are persisted only when
  `MultiCamConfig::persist_focus_stacks` is enabled.

Runtime controls:

- `TRUESHOT_MEMORY_BUDGET_MIB`: explicit native pipeline admission budget.
- `TRUESHOT_GROUP_RETRY_LIMIT`: persisted per-group retry ceiling, default 3.
- `TRUESHOT_RESUME_VERIFY=metadata|sampled|full`: artifact validation policy,
  default `sampled`.
- `TRUESHOT_RESUME_HASH_SAMPLE_RATE`: one-in-N full-hash sample rate, default
  1000.
- `TRUESHOT_NEF_ACCESS_MODE=indexed`: opt-in repeated random-access index.

## Local Exactness Benchmark

Run the benchmark against local capture data:

```bash
cargo run -p trueshot-core --release --example nef_roi_benchmark -- \
  realTest/_Z9Z5339.NEF --verify-full
```

Leave `TRUESHOT_NEF_ACCESS_MODE` unset to test the production sidecar-free path.

Run the grouped direct-to-arena benchmark:

```bash
cargo run -p trueshot-core --release --example nef_group_benchmark -- \
  realTest --workers 8 --verify-full --storage-class local-ssd
```

On the 2026-07-26 local baseline, all 21 Z9 files matched full decode exactly.
The 1310x1304 crop occupied 68.42 MiB for the complete group. Decode time was
4.43 seconds with one worker, 1.37 seconds with four workers, and 0.84-1.04
seconds with 8-21 workers. The benchmark reports p50/p95/p99 group latency by
camera, model, compression, bit depth, strip count and operator-supplied
storage class.

Run the complete native fusion benchmark without writing cropped
intermediates:

```bash
cargo run -p trueshot-core --release --example nef_native_fusion_benchmark -- \
  realTest
```

The local 21-frame baseline decoded in 0.96 seconds, aligned and fused in 0.78
seconds, demosaiced/post-processed in 0.17 seconds and atomically exported in
0.21 seconds.

Run the manifest scale and local corruption gates:

```bash
cargo run -p trueshot-core --release --example capture_manifest_scale_benchmark -- \
  1000000
cargo run -p trueshot-core --release --example nef_corruption_runner -- \
  realTest/_Z9Z5339.NEF
```

The million-group gate generated and streamed a 609,361,230-byte manifest with
0.6 MiB RSS growth (5.9 MiB baseline, 6.5 MiB peak). The corruption runner
completed six truncation probes and eight critical TIFF-header mutations with
zero panics; all header mutations were rejected, while two near-end
truncations remained readable because the requested ROI data was intact.
