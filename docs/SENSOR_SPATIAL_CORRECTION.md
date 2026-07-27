# RAW Sensor Spatial Correction

TrueShot can fit and apply a measured full-sensor flat-field and persistent
defect profile without converting the Bayer data to RGB. The
`trueshot.sensor-correction.v1` artifact is bound to camera identity, sensor
geometry and bit depth, lens model, aperture, focal length, and the measured
focus envelope. Runtime fusion fails closed if any of those values, the
current focus distance, or the even-origin Bayer crop disagree.

## Capture Requirements

Use the paired RAW protocol in `docs/SENSOR_NOISE_CALIBRATION.md`. Spatial
calibration additionally requires:

- One lens, focal length, and aperture for the entire flat corpus.
- Flat pairs at the near and far endpoints plus representative interior
  positions of every focus range the profile will be allowed to correct.
- A stabilized integrating sphere or independently verified uniform source
  covering the complete sensor.
- Stationary capture with illumination flicker below the paired
  agreement threshold.
- At least two fit and two holdout flat pairs after all identity and exposure
  checks.

Do not use a wall, monitor, sky, improvised diffuser, or source with an
unmeasured gradient. The estimator cannot distinguish illumination
nonuniformity from optical shading. Capture separate profiles when lens,
aperture, focal length, sensor crop/readout geometry, or focus envelope
changes.

## Fit And Publication

The existing command produces both calibration profiles in one streamed pass:

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

For this example the successful artifacts are:

- `z9-noise.json`: exact-ISO Poisson-Gaussian noise profile.
- `z9-noise_spatial_correction.json`: optics-bound spatial correction.
- `z9-noise_calibration_report.json`: source hashes, split policy, metrics,
  failures, and exact published artifact digests.

The spatial fitter:

1. Reuses each already decoded paired flat, so spatial calibration adds no
   second full-NEF decode.
2. Estimates a compact per-CFA log-domain gain grid in full-sensor
   coordinates.
3. Uses only flats whose per-CFA center response is between 10% and 85% of
   usable sensor range; darker and near-clipped levels remain available to the
   noise fitter but cannot bias the spatial model.
4. Uses complete held-out pairs to gate corrected p95 relative error and
   improvement over the uncorrected response.
5. Records the measured focus-distance envelope and rejects runtime frames
   outside it.
6. Classifies only repeatable, pair-agreeing extreme pixels as persistent
   defects.
7. Publishes no runtime profile if any identity, sample-count, gain-range,
   holdout, or defect-count bound fails.

## Runtime

```shell
trueshot process \
  --mode burst \
  --input /path/to/capture \
  --output /path/to/output \
  --sensor-noise-profile /path/to/z9-noise.json \
  --sensor-correction-profile /path/to/z9-noise_spatial_correction.json
```

Native fusion replaces a mapped defect with the median of nondefective
same-CFA neighbors before interpolation. It then applies the bilinearly sampled
flat-field gain after black subtraction. Sensor clipping remains evaluated in
the original sensor domain, while radiance variance is scaled by gain squared.
This preserves censored-observation semantics and propagates the correction
through posterior uncertainty.

Every burst exports an exact `*_sensor_correction.png` provenance map and
records the profile SHA-256 plus repaired-pixel count in the fusion report.
The flat-field bit means a calibrated gain affected contributing evidence; the
defect bit means contributing evidence required same-CFA repair. Rejected
bracket outliers do not claim defect-repair provenance.

## Qualification Boundary

Synthetic fit/holdout, profile round-trip, optics mismatch, crop parity,
defect replacement, uncertainty propagation, tile parity, and native fusion
quality gates are implemented. No retained real Z9 integrating-sphere corpus
or per-lens operating envelope is currently shipped. Product claims still
require held-out real measurements across and beyond supported focus envelopes,
temperatures, exposure durations, lenses, focal lengths, and apertures.

The spatial profile does not calibrate dark current versus exposure and
temperature, color response, lens PSF, pupil shape, focus breathing, flare, or
wavelength-dependent glare.
