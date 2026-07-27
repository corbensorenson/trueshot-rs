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

## Native Bayer Extraction

`trueshot.lens-psf-extraction-plan.v1` removes hand-entered optical
measurements from the calibration path. Each retained NEF capture declares:

- a relative path and exact lowercase SHA-256;
- a preregistered `fit` or `holdout` split;
- independently measured subject distance;
- independently measured focus distance plus one-sigma uncertainty when
  available;
- one two-edge target at every declared field-radius knot;
- two bounded slanted-edge ROIs and their known target-plane edge separation.

Use high-contrast, matte, parallel edges between 1.5 and 20 degrees from the
sensor axes. The target must be rigid, its physical edge separation must be
measured rather than inferred from a print setting, and center/corner ROIs must
remain inside the full sensor. Do not tune ROIs after inspecting holdout
results.

```bash
cargo run -p trueshot-cli --release -- \
  extract-lens-psf \
  --plan retained/z9-50mm-f7.1-extraction-plan.json \
  --capture-root retained \
  --output retained/z9-50mm-f7.1-measurements.json
```

The extractor verifies every retained source before parsing it, selectively
decodes only the union of each target's two ROIs, and measures native green
Bayer sites using full-sensor CFA parity. It estimates edge orientation with a
structure tensor and deterministic sub-degree refinement, builds an 8x
supersampled edge-spread function, robustly fits the analytic uniform-disk PSF,
and reports segmented diameter uncertainty, residual, MTF50, pair parallelism,
pair disagreement, measured field radius, magnification-derived effective
focal length, decoded pixels, and full-frame-equivalent decode fraction.

Clipping, weak contrast, low gradient coherence, axis-aligned edges, excessive
uncertainty/residual, nonparallel pairs, inconsistent diameters, incorrect
field placement, identity drift, and source-hash drift fail closed. A failed
run writes a diagnostic report but never publishes measurements.

An independently measured focus distance is authoritative because camera lens
telemetry is quantized and may be approximate. Its declared uncertainty must
be at most 2% by default. When EXIF `SubjectDistance` is also present, the
extractor reports and gates their relative disagreement. If no independent
distance is supplied, usable EXIF distance is accepted and explicitly labeled
`exif_subject_distance`; missing distance fails closed. Approximate encrypted
Nikon MakerNote distance is not silently promoted to calibration truth. The
selected source and uncertainty are embedded in every published measurement,
so the downstream profile's measurement digest binds this provenance rather
than depending on a detached report.

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
identity rejection, tile invariance, native analytic-disk extraction across
2.5-16 pixel blur diameters and 3-17 degree edge slants, and distance-provenance
gating. The extraction sweep's maximum diameter relative error is 0.005365.
A private Z9 NEF verifies real parser, source-hash, and fail-closed missing
distance behavior, but it is not an optical target and cannot qualify a lens.
A product quality claim still requires retained real target captures for every
marketed lens, focal length, aperture, focus envelope, and relevant
wavelength/temperature configuration, plus halo/MTF validation on real hair,
fur, transparency, and glossy edges.
