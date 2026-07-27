# Paired RAW Sensor Calibration

TrueShot fits exact-camera, exact-bit-depth, exact-ISO, per-CFA
Poisson-Gaussian sensor models from retained NEF evidence. The workflow is
deliberately fail-closed: no profile is published unless independent holdout
pairs pass every declared variance and interval-coverage gate.

## Capture Protocol

Use manual exposure, native RAW bit depth, fixed camera settings, a stable
sensor temperature, and disabled in-camera noise reduction. Do not preprocess
or convert the NEFs.

For every ISO to be supported, capture:

- At least 16 lens-capped dark frames. Consecutive frames form eight pairs:
  four fit pairs and four interleaved holdout pairs.
- At least five independently exposed uniform flat-field levels, with at least
  eight frames per level. TrueShot recommends seven levels near 5%, 15%, 30%,
  50%, 70%, 86%, and 95% of usable sensor range.
- A brightest flat whose robust peak reaches at least 90% of usable range.
- Pair exposures whose per-CFA robust means agree within 1%.

Use a stabilized integrating sphere or high-quality uniform light source.
Defocus the lens, exclude flickering illumination, avoid gradients and
vignetting where possible, and allow the camera to reach the operating
temperature that production captures will use. Capture separate profiles when
readout mode, bit depth, or material thermal behavior changes.

Place the files in one dark directory and one directory per flat level:

```text
calibration/
  dark/
  flat-05/
  flat-15/
  flat-30/
  flat-50/
  flat-70/
  flat-86/
  flat-95/
```

Each directory may contain multiple ISO values, but every ISO must have the
required frame count in the dark directory and every flat-level directory.

## Command

```shell
trueshot calibrate-noise \
  --dark calibration/dark \
  --flat-level calibration/flat-05 \
  --flat-level calibration/flat-15 \
  --flat-level calibration/flat-30 \
  --flat-level calibration/flat-50 \
  --flat-level calibration/flat-70 \
  --flat-level calibration/flat-86 \
  --flat-level calibration/flat-95 \
  --output calibration/z9-noise.json
```

The command refuses to overwrite an existing profile or report. Choose a new
output path for every calibration run so prior qualified evidence remains
immutable.

## Estimator

Spatial variance from a single flat confounds temporal sensor noise with pixel
response nonuniformity, lens shading, and illumination gradients. TrueShot
instead:

1. Sorts captures deterministically and pairs adjacent frames.
2. Interleaves complete pairs between fit and holdout sets.
3. Samples every CFA site independently with a deterministic bounded stride.
4. Estimates temporal read noise from robust centered dark-frame differences.
5. Estimates persistent fixed-pattern uncertainty from per-pixel dark means
   across independent pairs and validates its contribution on held-out pairs.
6. Exposure-normalizes each flat pair before differencing, rejecting pairs
   whose robust means differ by more than the declared tolerance.
7. Fits `variance_DN2 = read_variance_DN2 + signal_DN / electrons_per_DN`
   over robust signal bins with iteratively reweighted Huber regression.
8. Reserves a calibrated high-signal noise tail below encoded white as the
   censoring threshold. The default is four standard deviations at full-scale
   signal and is independent of how bright the operator made the final flat.

Persistent fixed pattern and frame-center drift are retained in the runtime
single-frame uncertainty model, but are not incorrectly injected into the
paired temporal shot-noise regression where they cancel.

## Release Gates

Every ISO and every RGGB CFA site must pass:

- At least five genuinely separated fit and holdout signal levels.
- At least 90% peak sensor-range coverage.
- At most 10% held-out dark temporal-variance error.
- At most 10% held-out fixed-pattern error as a contribution to total dark
  variance.
- At most 10% maximum held-out photon-transfer variance error.
- Empirical 90% and 95% normalized residual coverage within 3 percentage
  points of nominal.
- Positive finite read noise and conversion gain.

Structural identity, sensor levels, camera make/model, dimensions, bit depth,
exact ISO, RGGB layout, pair counts, and duplicate source content are also
validated. One failed ISO or CFA site prevents publication of the whole
profile.

## Artifacts And Provenance

A successful run produces:

- `<name>.json`: the bounded `trueshot.sensor-noise.v1` runtime profile.
- `<name>_calibration_report.json`: the
  `trueshot.sensor-calibration.artifact.v1` evidence report.

The report records configuration, pairing policy, every source path and
SHA-256, per-ISO `trueshot.sensor-calibration.iso.v1` diagnostics, gate
failures, publication state, and the completed profile SHA-256. The report is
written before profile publication and finalized only after the profile
round-trips through production validation.

Full NEFs are decoded one pair at a time. Only deterministic bounded samples
are retained, so memory does not grow with full-frame pixel count.

## Qualification Boundary

The fitter, synthetic photon-transfer recovery, synthetic holdout gates, and
end-to-end posterior-coverage test are implemented. The repository does not
yet contain a retained real Nikon Z9 dark/flat corpus or a qualified Z9 profile.
TrueShot must not claim measured Z9 uncertainty coverage until that evidence is
captured and the report passes.

This profile also does not yet replace separate calibration for lens shading,
defect pixels, dark-current-versus-exposure/temperature, color response, or
lens PSF/breathing. Those remain explicit product-quality work rather than
being folded into invented constants.
