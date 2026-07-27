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

- Synthetic focus-stack PSNR must remain at least 40 dB and depth accuracy at
  least 98% on the current deterministic fixture. The 2026-07-26 baseline is
  44.127 dB and 100%.
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
- Confidence- and edge-aware depth regularization with dominant-plane refusion.
- Synthetic focus, radiance invariance, deghost, tile-boundary, and fail-closed
  gates.
- A private 21-frame 1310x1304 Z9 debug sweep measured 45.27% dominant-plane
  refusion coverage and 5.7% fusion overhead (13.08 seconds versus 12.37
  seconds). Disabling robust deghosting as well reached 11.31 seconds.
  Release-mode Apple baselines remain required.

Not yet qualified:

- Direct Helicon/Lightroom corpus results.
- Z9 camera-to-standard color matrix, ICC/DNG profile tagging, and DeltaE 2000
  release gate.
- Per-bracket local motion alignment and user-visible deghost overlay.
- Hair/thin-structure halo gates, retouch workflow, and multi-method automatic
  focus strategy.
- Per-ISO/CFA photon-transfer calibration, censored-saturation likelihood, and
  posterior uncertainty calibration.
- Diopter/PSF-based sub-plane depth and aperture-constrained occlusion
  correction.
- Deterministic uncertainty-driven exposure and focus acquisition scheduling.
- Release-mode Apple Silicon timing, memory, thermal, and energy baselines.

The implementation-grade literature mapping and rejection policy for
generative recovery are recorded in `docs/HDR_FOCUS_RESEARCH_2026.md`.
