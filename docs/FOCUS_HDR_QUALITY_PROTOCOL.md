# Focus/HDR Competitive Quality Protocol

Date: 2026-07-26

## Claim Policy

TrueShot must not claim that it exceeds Helicon Focus or Adobe Lightroom until
the same source captures are processed by all products and the retained
benchmark evidence meets the gates below. Feature presence, synthetic tests,
and visual preference alone are not proof of superiority.

The comparison baseline follows the vendors' documented controls:

- Helicon Focus methods A, B, and C, radius/smoothing, alignment, retouching,
  and 16-bit output:
  <https://www.heliconsoft.com/focus/help9/english/HeliconFocus.html>
- Lightroom HDR Auto Align, Auto Tone, deghost levels and overlay, and HDR DNG
  output:
  <https://helpx.adobe.com/lightroom-classic/desktop/process-and-develop-photos/hdr-photo-merge.html>

## Corpus

Every release-quality run must include:

1. Synthetic linear-RGB/CFA scenes with exact all-in-focus radiance and depth.
2. Static macro targets with slanted edges, Siemens stars, textured surfaces,
   smooth surfaces, hair/fur, thin crossing structures, and depth discontinuities.
3. HDR targets with calibrated neutral/color patches, deep shadows, specular
   highlights, saturated emitters, and at least 12 stops of measured range.
4. Motion cases with independently moving foreground, foliage, changing
   reflections, and one deliberately corrupted/hot-pixel bracket.
5. Joint focus + HDR + burst captures in the supported Z9 lossless 14-bit mode.
6. Failure cases: missing focus planes, two-frame HDR ambiguity, clipped entire
   brackets, alignment rejection, focus breathing, low texture, and malformed
   metadata.

Private camera files may drive local qualification, but CI must use a
redistributable fixture with immutable checksums and documented capture
settings.

## Measurements

| Dimension | Required evidence |
| --- | --- |
| Radiometric fidelity | Linear PSNR, relative radiance error, highlight recovery, shadow SNR |
| Focus fidelity | MTF50, edge acutance, all-in-focus PSNR/MS-SSIM, depth accuracy |
| Artifacts | Halo/zipper energy, seam count, false-color error, ghost residual |
| Color | ColorChecker DeltaE 2000 after a declared camera profile and white balance |
| Geometry | Registration residual, rejected-transform rate, focus-breathing residual |
| Performance | Wall p50/p95, peak RSS, bytes read/written, thermals and energy on supported Macs |
| Determinism | Output digest repeatability for deterministic mode |

No-reference sharpness is diagnostic only. It cannot decide a winner because
oversharpening can increase the score while reducing fidelity.

## Competitor Procedure

1. Use identical decoded source captures and identical crop bounds.
2. Export 16-bit outputs with sharpening/noise reduction disabled where the
   product allows it; record every setting and product version.
3. Run each documented Helicon method and an expert-tuned result, not only the
   default.
4. Run Lightroom HDR at every deghost level and retain the deghost overlay.
5. Align result color spaces before metrics. Never compare untagged
   camera-linear RGB directly with profiled sRGB/ProPhoto output.
6. Keep source hashes, output hashes, configuration, hardware, software
   versions, timings, and metric reports in one signed benchmark manifest.
7. Visually inspect at 100% and print scale using a blinded side-by-side review.

## Release Gates

- Synthetic focus-stack PSNR must remain at least 44 dB and depth accuracy at
  least 98% on the current deterministic fixture. The 2026-07-26 baseline is
  44.161 dB and 100%.
- Static HDR radiance error must remain below 0.2% at valid green samples.
- One corrupted bracket in a 3-shot stack must improve by at least 5x with
  deghosting enabled and finish below 0.2% radiance error.
- CFA samples, black level, exposure invariance, tile invariance, and malformed
  group rejection remain release-blocking unit tests.
- A superiority claim requires TrueShot to beat the best competitor result on
  the preregistered primary metric for at least 80% of corpus scenes, be no
  worse than the declared tolerance on every safety metric, and report
  confidence intervals.
- Until the camera color transform and ICC/DNG export are calibrated, TrueShot
  may claim camera-linear archival output, not Lightroom-equivalent color.

## Current Status

Implemented:

- Group-amortized ROI decode into one native `u16` arena.
- Exposure-normalized, CFA-safe HDR/focus fusion.
- Median/MAD-centered bracket outlier rejection.
- Censored Poisson-Gaussian bracket estimation with exact-ISO/CFA noise-profile
  validation, posterior radiance uncertainty, source attribution, and explicit
  clipping/rejection/fallback state.
- Bounded paired dark/flat sensor calibration with complete-pair fit/holdout
  splits, per-CFA temporal/fixed-pattern/conversion-gain estimation, independent
  variance and residual-coverage gates, immutable source SHA-256 evidence, and
  fail-closed profile publication. Synthetic photon-transfer recovery and an
  end-to-end nominal-95% posterior gate are passing.
- Canonical adaptive shutter/ISO/focus candidate construction, explicit
  per-candidate rejection/utility records, independent posterior quality-target
  stopping, validated termination reasons, and streaming capture-manifest
  provenance. The synthetic closed loop reaches equal declared variance targets
  in 178 ms versus 1,027 ms for a fixed 21-shot grid.
- Exposure-normalized per-bracket global translation, selective compact
  gradient-cell refinement, forward/backward disocclusion rejection, and
  measured-reference fallback.
- Exact 16-bit source maps, exact 8-bit fusion-state maps, a bounded visible
  provenance overlay, and a machine-readable fusion report.
- A local Fusion Inspector discovers validated schema-v2 reports, displays
  source/deghost/frequency/glare/aperture-boundary/sensor-correction evidence,
  exposes calibration and fallback status, and downloads exact archival maps.
  Encrypted report and PNG reads are bounded and decrypted only in memory.
- Confidence- and edge-aware depth regularization with dominant-plane refusion.
- Synthetic focus, radiance invariance, deghost, tile-boundary, and fail-closed
  gates.
- A private 21-frame 1310x1304 Z9 debug sweep measured 45.27% dominant-plane
  refusion coverage and 5.7% fusion overhead (13.08 seconds versus 12.37
  seconds). Disabling robust deghosting as well reached 11.31 seconds.
  Release-mode Apple baselines remain required.
- After selective bracket alignment, the same private stack measured 11.43
  seconds decode and 16.95 seconds fusion in a debug build. All 14
  nonreference brackets passed global registration; 192 compact cells received
  local correction and 40 inconsistent cells were excluded. The retained
  39.75% refusion distribution was unchanged from the immediately preceding
  implementation. This is engineering evidence, not a competitor result.
- The source-bound production Apple M1 gate executes the complete CLI path in
  isolated temporary directories with one warmup and five measured runs. It
  passes exact 11-artifact and semantic-provenance repeatability, Apple Metal
  AHD with no fallback, measured-only archival policy, native energy, low-power,
  and thermal gates. Wall p50/p95 is 5.939/6.073 seconds; stage p95 is
  0.899 seconds decode, 2.901 seconds fusion, and 0.070 seconds Metal
  demosaic/postprocess. Maximum physical footprint/RSS/primary energy are
  310.3 MiB/711.4 MiB/41.603 J, with nominal thermal state throughout.
  Aggregate evidence is retained in
  `docs/benchmarks/apple_nef_fusion_qualification_2026-07-27.json`; the private
  uncalibrated fixture makes this an integration/performance result, not a
  quality-superiority result.

Not yet qualified:

- Direct Helicon/Lightroom corpus results.
- Z9 camera-to-standard color matrix, ICC/DNG profile tagging, and DeltaE 2000
  release gate.
- Interactive source/glare/trimap edit documents and deterministic
  measured-source refusion in the dashboard. Read-only inspection is
  implemented.
- Hair/thin-structure halo gates and multi-method automatic focus strategy.
- Retained real per-ISO Z9 dark/flat calibration and real-sensor posterior
  uncertainty coverage. The capture/fit CLI, independent holdout gates,
  validated profile persistence, and production loading are implemented.
- Held-out calibration of the implemented nonuniform diopter/thin-lens
  sub-plane depth and conservative aperture-visibility projection, plus
  lens-breathing calibration, measured halo-energy/MTF gates, and mixed-pixel
  trimaps. Native fusion exports unquantized metric depth and per-pixel
  visibility-correction provenance when complete verified geometry is valid.
- Live posterior extraction, calibrated lens-drive/readback, production server
  camera-adapter integration, and real efficiency qualification for the
  deterministic uncertainty-driven exposure/focus planner.
- Redistributable NEF timing fixtures, full-sensor 8280x5520 qualification, and
  equivalent release gates on every supported Apple Silicon generation.

The implementation-grade literature mapping and rejection policy for
generative recovery are recorded in `docs/HDR_FOCUS_RESEARCH_2026.md`.
