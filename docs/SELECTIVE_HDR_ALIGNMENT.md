# Selective HDR Alignment and Provenance

Date: 2026-07-26

## Purpose

TrueShot aligns each exposure bracket without allowing exposure changes to
masquerade as focus breathing. It corrects local motion only where a compact
global model is insufficient, rejects unsupported regions, and records exactly
which measured frame supplied each output sample. The archival path does not
invent highlights or focus detail.

## Alignment Model

1. The median-exposure frame is the reference for each focus plane.
2. Compact green-channel analysis images are exposure normalized.
3. Focus planes retain their independently estimated scale and translation.
4. HDR brackets estimate translation only. Scale is deliberately fixed because
   exposure changes do not cause lens breathing.
5. An identity normalized-cross-correlation fast path avoids FFT work when the
   bracket already agrees with its reference.
6. Accepted global translations are converted to full-resolution Bayer
   coordinates and sampled without mixing CFA sites.
7. A rejected nonreference bracket fails closed: it is excluded and marked
   disoccluded rather than fused at an unverified identity transform.

## Selective Local Refinement

The local stage stores one compact cell grid per bracket, not a dense
full-resolution flow field.

- Gradient agreement is evaluated per cell after global registration.
- Cells above the agreement threshold remain on the global model.
- Only unexplained, textured cells run a bounded residual search.
- A parabolic peak fit provides subpixel residual motion.
- The reverse match must return to the source within the configured
  consistency threshold.
- Forward/backward failures are classified as disoccluded. Those observations
  are omitted from fusion rather than blurred into the output.
- Empty motion grids are discarded so static brackets have no per-pixel local
  sampling cost.

This is intentionally conservative. A rejected observation may reduce local
signal-to-noise ratio; an accepted false correspondence can create a
measurement-destroying ghost.

## Source Fallback

Robust radiance estimation still operates after registration. If local
disocclusion removes a bracket and the measured focus-plane reference becomes
the dominant surviving source, the pixel is tagged as a source fallback. If no
valid uncensored sample remains, the existing censored-likelihood policy
returns an attributed conservative bound rather than generative content.

## Exported Evidence

Native CLI processing exports these crash-safe artifacts beside the image:

- `*_source_map.png`: exact 16-bit source-frame identifier per pixel.
- `*_fusion_flags.png`: exact 8-bit bitfield per pixel.
- `*_fusion_overlay.png`: bounded RGBA visualization for inspection.
- `*_fusion_report.json`: schema, artifact names, flag counts, calibration and
  refusion state, per-frame transforms, local/disocclusion cell counts, and the
  archival policy.

The visible overlay uses magenta for disocclusion, red for source fallback,
pink for censor conflict, orange for robust outlier rejection, yellow for
clipping, cyan for bracket alignment, blue for aperture-visibility correction,
and gray for uncalibrated noise. Exact maps remain authoritative when multiple
states overlap.

## Current Gates

- Bounded local translation is recovered on a synthetic moving region.
- Forward/backward-inconsistent motion is marked disoccluded.
- Rejected global registration cannot enter radiance fusion.
- The measured reference source and fallback flag are retained.
- Sparse critical flags survive overlay downsampling.
- Source and flag PNG maps round-trip exact `u16` and `u8` values.
- The native memory estimator includes compact analyses, motion grids, retained
  maps, and PNG encoding scratch.
- Existing focus, exposure, censored-likelihood, deghosting, CFA, tiling, and
  aperture-visibility gates remain release blocking.

## Measured Engineering Baseline

On the private 21-frame 1310x1304 Nikon Z9 crop, a 2026-07-26 debug run on the
development Mac measured:

- scan: 357.58 ms
- native decode: 11.43 s
- fusion: 16.95 s
- demosaic/display: 1.41 s
- focus transforms accepted: 7/7
- nonreference bracket transforms accepted: 14/14
- locally corrected cells: 192
- disoccluded cells: 40
- depth-consistent refusion: 679,019/1,708,240 pixels (39.75%)

These numbers establish reproducible engineering evidence, not release
throughput or competitor superiority.

## Remaining Qualification

- Capture a redistributable real corpus with controlled translation, articulated
  motion, specular motion, foliage/hair, and large disocclusions.
- Gate ghost residual, radiance error, fallback precision/recall, and false
  disocclusion rate against the prior robust-only path.
- Establish optimized release p50/p95, peak RSS, energy, and thermal baselines
  on each supported Apple Silicon class.
- Add an interactive dashboard view for source/deghost inspection and explicit
  measured-source retouching without changing archival defaults.
