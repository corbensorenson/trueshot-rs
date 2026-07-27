# Aperture-Visibility Focus Fusion

TrueShot's native focus fusion prevents foreground/background halos by
constraining the focus-selection surface in conjugate sensor space. This is an
optical visibility constraint, not generic mask smoothing.

## Physical Contract

For object distance `Z`, focal length `f`, sensor distance `S`, aperture radius
`A`, and sensor-plane position `p`:

```text
S = f Z / (Z - f)
|gradient S(p)| <= S(p) / A
```

The implementation uses the foreground-favored solution described by Jacobs,
Baek, and Levoy. Real compound lenses can deviate from the thin-lens/paraxial
model, so TrueShot doubles the theoretical halo extent as recommended by the
paper.

In log sensor distance the one-sided foreground constraint becomes a max-plus
distance transform with a constant per-pixel cost. TrueShot solves it with
bounded forward/reverse chamfer raster passes. This:

- preserves the foreground focus surface;
- creates only the physically necessary background transition;
- supports continuous sub-plane diopter estimates;
- is linear in image pixels rather than quadratic in pixel pairs; and
- requires two full-resolution `f32` working maps plus a provenance mask, not
  a full focus-measure volume.

## Verified Geometry

The correction runs only when focus distance, aperture, focal length, and
physical pixel pitch are verified and consistent across the group. The Nikon
Z9 profile derives `4.348... um` pitch from Nikon's published `35.9 mm`
horizontal sensor area and `8,256`-pixel FX image width.

Unsupported or inconsistent camera geometry disables only the aperture
projection. Physical diopter inference can continue when its own metadata is
valid, and the result explicitly reports `visibility_constrained = false`.
No sensor geometry is guessed.

## Provenance

- `visibility_adjusted_pixels` records the number of changed focus
  coordinates.
- `FUSION_FLAG_VISIBILITY_CORRECTED` marks each adjusted pixel.
- Depth-consistent refusion re-reads the selected CFA-safe focus hypotheses,
  so correction affects image pixels rather than only a diagnostic depth map.

## Scale-Decoupled UHD Decisions

Focus selection combines:

- globally aligned low-resolution regional blocks for stable region
  decisions; and
- native-resolution Laplacian residuals only where detail exceeds the
  regional response.

Regional evidence selects planes while native detail evidence controls blend
confidence. This avoids the sharpness regression caused by using compressed
coarse scores for both jobs. The deterministic synthetic baseline is
`44.161 dB` PSNR with `100%` depth classification, gated at `44 dB`.

## Remaining Qualification

- Calibrate pupil/PSF and lens breathing by lens, aperture, distance, and image
  radius.
- Measure foreground/background halo energy, MTF50, color error, and
  thin-structure recall on the preregistered real-lens occlusion corpus.
- Add boundary trimap treatment for transparency, hair, fur, and mixed
  pixels where an opaque two-surface model is insufficient.
- Establish Apple Silicon release p50/p95, RSS, energy, and thermal baselines
  on full-resolution Z9 groups.

## Research Basis

- [Focal Stack Compositing for Depth of Field Control](https://graphics.stanford.edu/papers/focalstack/)
- [UHD-MFF](https://arxiv.org/abs/2606.31242)
- [Nikon Z9 specifications](https://www.nikonusa.com/p/z-9/1669/overview)
