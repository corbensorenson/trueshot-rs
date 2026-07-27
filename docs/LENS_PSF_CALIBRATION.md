# Lens PSF Calibration

TrueShot can replace its explicitly reported ideal thin-lens fallback with a
retained, digest-bound model of:

- focus breathing through effective focal length versus focus distance;
- entrance-pupil diameter versus focus distance;
- residual field-dependent PSF scale versus normalized sensor radius.

The calibration changes physical depth mapping, sub-plane confidence,
aperture-visibility projection, and mixed-boundary support. It never changes
measured RAW radiance and does not generate image content.

## Measurement Contract

`trueshot.lens-psf-measurements.v1` is a bounded JSON artifact. Each
measurement must identify:

- one retained source by lowercase SHA-256;
- `fit` or `holdout` split chosen before fitting;
- focus and target subject distance in meters;
- normalized sensor radius from `0` at center to `1` at a corner;
- independently measured effective focal length in millimeters;
- observed defocus-circle diameter in pixels;
- sensor pixel pitch in micrometers.

The artifact also declares camera, full sensor dimensions, lens, nominal focal
length, aperture, calibration target, measurement method, radius knots, and
the retained source manifest. Missing, duplicate, malformed, or unreferenced
source hashes fail validation. Every measurement radius must match one declared
radius knot, every source must be referenced, and one source may not cross the
preregistered fit/holdout boundary.

Effective focal length should come from known target geometry and image
magnification. Defocus diameter should come from a preregistered edge/point
target method, not visual tuning. Fit and holdout captures must be separate
files.

## Publication

```bash
cargo run -p trueshot-cli --release -- \
  calibrate-lens-psf \
  --measurements retained/lens-psf-measurements.json \
  --output profiles/z9-105mm-f4-lens-psf.json \
  --maximum-p95-error 0.05 \
  --minimum-error-reduction 0.50
```

Publication requires:

- at least two fit samples for every focus/radius cell;
- at least twelve independent holdout measurements;
- at least one independently retained holdout sample for every fit cell;
- corrected holdout p95 relative error at or below the declared threshold;
- the declared p95 reduction versus ideal thin-lens prediction;
- valid bounded optical parameters, focus knots, and calibrated sensor-plane
  coordinates.

The profile and companion report are written atomically. Profile publication
uses an atomic no-clobber link, so a concurrent writer cannot replace an
existing artifact after the preflight check. The profile embeds the canonical
measurement-set digest and its publication thresholds; runtime rejects a
profile whose retained metrics no longer satisfy those thresholds. The report
additionally hashes the exact supplied JSON bytes. Runtime reloads the
completed profile and derives its `sha256:` calibration identity from the
exact profile bytes.

## Processing

```bash
cargo run -p trueshot-cli --release -- \
  process \
  --input capture \
  --output result \
  --mode burst \
  --lens-psf-profile profiles/z9-105mm-f4-lens-psf.json
```

Every frame must match camera, full sensor geometry, lens identity, focal
length, aperture, and calibrated focus envelope. Any mismatch fails before
fusion. The fusion report records:

- `lens_psf_calibrated`;
- `lens_psf_calibration_id`;
- `physical_focus_policy`.

Without a supplied profile, processing remains available but reports
`ideal_thin_lens_explicit_fallback`.

## Qualification Boundary

The synthetic gates prove fitter recovery, independent holdout improvement,
source-manifest integrity, focus/radius interpolation, runtime attribution,
identity rejection, and tile invariance. A product quality claim still
requires retained real target captures for every marketed lens, focal length,
aperture, focus envelope, and relevant wavelength/temperature configuration,
plus halo/MTF validation on real hair, fur, transparency, and glossy edges.
