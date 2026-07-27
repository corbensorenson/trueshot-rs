# HDR and Focus Fusion Research Review

Date: 2026-07-26

## Objective

This review maps current HDR and multi-focus research to TrueShot's native
NEF pipeline. The target is not merely a visually pleasing composite. The
shipping archival path must be deterministic, source-attributable,
radiometrically defensible, memory bounded, and fast on Apple Silicon.

## Current TrueShot Baseline

The production native path already has several uncommon strengths:

- Bayer-preserving ROI decode and fusion without intermediate cropped files.
- One focus-plane transform shared across that plane's HDR brackets.
- Exposure-normalized, white-balanced linear radiance fusion.
- Median/MAD bracket rejection and depth-consistent CFA refusion.
- Bounded tiles, parallel bands, deterministic tests, and retained depth and
  confidence products.

The current quality ceiling is nevertheless constrained by five simplifying
assumptions:

1. `read_noise_dn` is a fixed scalar and shot noise is only approximated.
2. Saturated samples are tapered or discarded rather than handled as censored
   observations of an unknown radiance.
3. Focus confidence comes from one local spatial scale and depth is stored as
   a uniformly spaced frame index.
4. Alignment is one scale/translation per focus plane; bracket motion and
   local non-rigid motion are not estimated.
5. Selection-map smoothing does not model aperture geometry or visibility at
   foreground/background occlusions.

## Highest-Value Research Findings

### 1. Calibrated probabilistic RAW fusion

A Bayesian multi-exposure method based on a modified Poisson model explicitly
accounts for background and saturation and improves low-SNR fusion. Its
application is ptychography, but the sensor-estimation principle transfers
directly to camera RAW: infer latent radiance from a calibrated likelihood
instead of hand-tuned confidence curves.

TrueShot should implement its own censored Poisson-Gaussian estimator:

- Calibrate per-camera, per-ISO, per-CFA-site conversion gain, read noise,
  row/column bias, black drift, and saturation headroom.
- Treat clipped values as interval/censored evidence, not ordinary samples.
- Estimate radiance, posterior variance, and outlier probability together.
- Keep a fast closed-form weighted least-squares path for well-behaved pixels
  and invoke bounded IRLS only for clipping, motion, or disagreement.
- Export uncertainty and source-contribution maps for diagnostics and retouch.

Source: [Bayesian multi-exposure image fusion for robust high dynamic range ptychography](https://arxiv.org/abs/2403.11344).

Implementation state (2026-07-27): TrueShot's paired calibration command now
fits two independently gated artifacts in one streamed decode pass. Exact-ISO
noise profiles drive a bounded analytic censored Poisson-Gaussian survival
likelihood and posterior uncertainty. Ordinary observations use the calibrated
heteroscedastic variance and retain the allocation-free robust weighted
least-squares fast path. A clipped observation contributes its one-sided
survival probability, including the variance derivative in the analytic
score/Fisher information; a stable inverse Mills ratio handles far-tail
constraints, and a six-sigma compatibility gate identifies contradictory
clipping evidence instead of forcing a biased estimate. All-clipped pixels
remain attributed lower bounds with infinite uncertainty rather than invented
detail. An optics-bound spatial profile fits a compact per-CFA
full-sensor gain grid in the log domain, records the measured focus envelope,
and identifies only persistent, pair-agreeing defect pixels. Complete flat
pairs are held out; publication requires corrected p95 relative error at most
3% and at least 50% improvement. Shadow and near-clipped levels still inform
the noise model but are excluded from spatial fitting by a retained 10%-85%
usable-signal gate.
Native fusion repairs mapped defects from same-CFA neighbors before
interpolation, applies gain after black subtraction, evaluates clipping in the
uncorrected sensor domain, and scales variance by gain squared. Camera, sensor
geometry/bit depth, lens, aperture, focal length, focus envelope, crop
containment, and Bayer origin mismatches fail closed. The exact correction map
and profile digest are exported. Retained real integrating-sphere, thermal,
exposure-duration, and focus-envelope qualification remain.

The conditional-censor simulation records 28,021 clipping events across
40,000 deterministic draws: nominal 95% intervals cover 96.203%, mean bias is
0.005563 against a 0.15-sigma gate, and compatible evidence produces 0% false
conflicts. The independent ordinary end-to-end posterior gate covers
3,472/3,600 pixels (96.444%). The private 21-frame NEF integration fixture has
no clipped pixels and no exact sensor profile, so it verifies production
execution and diagnostics but cannot qualify the real calibrated posterior.
Exact evidence is retained in
`docs/benchmarks/censored_pg_qualification_2026-07-27.json`.

### 2. Physical focus coordinates and PSF-aware depth

Scene-adaptive focus acquisition research models each focal slice as a sharp
signal convolved with a spatially varying point-spread function (PSF), filters
depth evidence using estimated blur kernels, fits focus responses, and chooses
new capture positions from estimated scene depth. TrueShot already parses
aperture, focal length, and focus distance, but production fusion currently
reduces focus to a uniform frame index.

TrueShot should:

- Convert focus distance to diopters and sensor-plane distance before fitting;
  neither camera focus steps nor metric distances are uniformly spaced.
- Compute noise-whitened focus evidence at multiple scales.
- Fit the local response curve around its maximum for sub-plane depth.
- Derive confidence from peak prominence, curvature, model residual, and
  neighboring depth consistency rather than only the top-two score gap.
- Calibrate the lens's focus breathing and circle-of-confusion/PSF curve by
  aperture, focal length, subject distance, and image radius.
- Feed the posterior depth coverage back to capture control and stop when an
  additional slice has negligible expected information gain.

Sources: [Scene-Adaptive Image Acquisition for Focus Stacking](https://image.ee.tsinghua.edu.cn/pdf/2018_ICIP_lwt.pdf) and [Depth From Focus With Your Mobile Phone](https://openaccess.thecvf.com/content_cvpr_2015/papers/Suwajanakorn_Depth_From_Focus_2015_CVPR_paper.pdf).

Implementation state (2026-07-27): `trueshot.lens-psf.v1` now retains
focus-breathing effective focal length, entrance-pupil scale, and residual
field PSF on bounded focus/normalized-radius grids. Calibration consumes a
source-hashed, preregistered fit/holdout measurement artifact and refuses
publication unless every grid cell has evidence and independent holdout p95
error and improvement gates pass. Runtime identity binds camera, full sensor
geometry, lens, focal length, aperture, and focus envelope; interpolation is
in diopters and sensor radius. A retained-target extractor now verifies exact
source hashes, selectively decodes bounded native-Bayer edge pairs, fits an
analytic disk PSF/MTF50 with uncertainty and residual gates, and derives
effective focal length from measured target geometry. Independently measured
focus distance and uncertainty are preferred; EXIF distance is an attributed
fallback, and disagreement fails closed. The deterministic 2.5-16 pixel,
3-17 degree synthetic sweep has 0.005365 maximum diameter relative error.
Calibrated image distance drives metric depth, field PSF drives sub-plane
confidence and crossing support, and the maximum retained effective pupil
drives the linear aperture projection. The exact profile digest is exported.
No profile remains a visible
`ideal_thin_lens_explicit_fallback`, not an implied calibration. See
`docs/LENS_PSF_CALIBRATION.md`; retained synthetic and fallback evidence is in
`docs/benchmarks/lens_psf_qualification_2026-07-27.json` and
`docs/benchmarks/lens_psf_extraction_qualification_2026-07-27.json`.

### 3. Occlusion-correct, halo-free compositing

Focus halos are not just bad mask smoothing. At an occlusion boundary, a
defocused foreground physically covers rays from the background. Naively
choosing a sharp foreground slice beside a sharp background slice can count
incompatible rays and create a halo. Stanford's focal-stack analysis derives a
bound on the allowed selection-surface gradient based on aperture geometry and
shows how depth-value dilations enforce it in linear time with respect to the
number of pixels and depth labels.

TrueShot should implement a visibility-aware selection stage:

- Infer foreground/background ordering at each depth discontinuity.
- Convert the focus/depth map to sensor distance and enforce the aperture-bound
  slope constraint before CFA refusion.
- Build an uncertainty trimap around hair, fur, transparent edges, and crossing
  structures.
- Use source selection in definite regions and a gradient/Laplacian-domain
  transition only inside the physically valid trimap.
- Retain the original stack and provenance for manual retouch where no source
  observation can identify a unique solution.

Source: [Focal Stack Compositing for Depth of Field Control](https://graphics.stanford.edu/papers/focalstack/).

Implementation state (2026-07-27): native fusion now detects measured depth
crossings, evaluates their support on both measured and aperture-projected
surfaces, converts the bidirectional thin-lens defocus diameter through
verified sensor pitch, and expands variable-radius seeds with a bounded
linear-time max-plus transform. The exact
tri-state map distinguishes interior, physical PSF support, and crossing core.
Inside that support, depth-consistent refusion selects one aperture-valid focus
plane and re-estimates it only from traceable measured brackets; it never
interpolates radiance across incompatible depth rays. Every such pixel carries
source-fallback provenance, the map is atomically exported, radius truncation
is explicit, and diagnostics remain available when refusion is disabled.
Synthetic crossing-halo energy falls from 23.759999 to 0.002434, exceeding the
50% gate, while the 44.161 dB/100% focus baseline and exact tile parity remain.
Real hair, fur, transparency, and compound-lens calibration remain required.

### 4. Scale-decoupled UHD focus decisions

Single-scale focus maps are not sufficient: small support gives accurate edges
but is noisy, while large support is stable but shifts boundaries. Recent UHD
work separates low-resolution region decisions from native-resolution
Laplacian edge refinement and reports 4K inference with low memory and compute.
Earlier probabilistic two-scale methods reach the same architectural
conclusion without requiring a large network.

TrueShot should build its own deterministic, Metal-friendly variant:

- Estimate coarse focus regions on downsampled, exposure-normalized green.
- Use integral-image or separable multiscale noise-whitened gradient energy.
- Refine only a narrow boundary/low-confidence set at native Bayer resolution.
- Store compact label and uncertainty pyramids rather than a full metric stack.
- Optionally distill the decision rule into a tiny local LUT after the
  deterministic implementation is validated; never make an unverified learned
  LUT the archival authority.

Sources: [UHD-MFF](https://arxiv.org/abs/2606.31242) and [Multi-focus image fusion using boosted random walks with two-scale focus maps](https://doi.org/10.1016/j.neucom.2019.01.048).

Implementation state (2026-07-27): the native path retains its coarse
region/native-edge architecture and now dispatches the focus stencil to a
dependency-free ARM64 NEON kernel on Apple Silicon. The robust 3x3 trimmed mean
is computed by an equivalent sum/min/max pass rather than sorting nine values
for every pixel. A portable scalar path remains selectable, full-pipeline tests
gate Bayer/depth parity, and the production report records the selected kernel.
On one Apple M1, an isolated 2048x1536 alternating benchmark measured 4.83x
p50 and 5.48x p95 speedup with 4.77e-7 maximum absolute error. The
source-hashed record is retained in
`docs/benchmarks/apple_focus_qualification_2026-07-27.json`, and ARM64 macOS CI
enforces conservative 1.5x/1.3x speedup and 2e-5 parity floors. This is a
focus-kernel result, not an end-to-end NEF, energy, thermal, or Metal AHD claim.

The deferred demosaic now has a separate, bounded Apple Metal implementation.
Four compute stages implement directional green interpolation, red/blue
reconstruction plus the CPU's integer Lab LUT, homogeneity voting, and 3x3
direction selection. Six-row halos make every emitted row independent of band
boundaries, adapter limits cap the 512-row ceiling, and production admits the
exact unified-memory scratch before decoding. The implementation uses one
Bayer-derived integer direction classifier, fixed-point
camera-to-XYZ/Lab transform, overflow-bounded chroma distance, and zero-margin
selection on CPU and WGSL. Float directional candidates remain the final output
representation. HDR-linear inputs use a shared power-of-two normalization scale,
so division and rescaling retain every measured `f32` CFA value exactly. One
reusable haloed host band performs HDR normalization, eliminating the previous
full-frame temporary and making its memory part of the admitted scratch bound.

The retained Apple M1 release record is
`docs/benchmarks/apple_metal_ahd_qualification_2026-07-27.json`. Its nominal and
6.25x HDR workloads both use an adversarial 8256x5504, 11-band composite with
chroma edges, Siemens-star spokes, a resolution wedge, saturated transitions,
dark detail, and band-seam crossings. Both have zero direction mismatches,
identical direction checksums, exact measured CFA values, zero values beyond
1e-3, and maximum normalized errors of 4.17e-7/2.98e-7. Nominal/HDR p50
speedups are 2.28x/2.45x with 484.79 MB admitted scratch. This is isolated
single-generation parity/performance evidence, not end-to-end NEF, energy,
thermal, calibrated chart-quality, or cross-generation evidence.

The production dispatch was also exercised on the private 21-frame Nikon Z9
stack. The existing group-amortized loader used one embedded preview and one
1310x1304 crop, Metal used three bands with exactly 77,143,488 admitted scratch
bytes, and the atomic export report recorded the Apple M1 adapter, no fallback,
exact measured-CFA policy, and no generative reconstruction. This proves the
real integration path, but the private stack is not a substitute for a
redistributable chart corpus or release-mode end-to-end qualification.

The release native-fusion benchmark on that group measured 358.68 ms scan,
0.90 seconds decode, 1.17 seconds fusion, and 0.07 seconds demosaic/display.
It reported p05/p50/p95 radiance uncertainty of
0.000020/0.000063/0.000206, 192 selectively aligned cells, 40 disoccluded
cells, and 679,020 depth-refused pixels. Two independent current-build
production runs then produced byte-identical fused TIFF, fusion-state, source,
boundary, and glare artifacts and semantically identical reports after
output-path fields were removed. These are one-machine integration and
repeatability measurements, not p50/p95 throughput or competitor claims.

Production admission also now accounts for Apple unified memory through Mach
free, inactive, and speculative pages rather than `sysinfo`'s near-free-only
value. On the retained 16 GiB M1, the old path reported only 42.5-140.9 MiB
available and rejected a bounded 245.6 MiB job; the corrected path reported
5.1 GiB reclaimable and completed under a 512 MiB explicit budget. Active,
wired, and compressed memory remain excluded.

### 5. Exposure-aware local alignment and frequency-separated deghosting

The NTIRE 2025 RAW challenge uses nine noisy, misaligned RAW frames with
different exposures and imposes explicit efficiency limits. Its winning work
uses recursive multi-exposure alignment. NTIRE 2026 expands the practical
failure model to motion, illumination change, and handheld jitter. Other HDR
work repeatedly finds value in separating low-frequency structure from
high-frequency detail.

TrueShot should avoid a full-frame generative reconstruction and instead:

- Align exposure-normalized gradients/census features, not raw brightness.
- Estimate global scale/rotation/translation first, then bounded tile flow only
  in regions whose radiance residual exceeds calibrated noise.
- Anchor low-frequency color and illumination to a selected reference while
  taking high-frequency detail from aligned inlier observations.
- Detect disocclusion and saturation jointly; fall back to one traceable source
  rather than synthesizing content.
- Persist a deghost overlay identifying aligned, rejected, clipped, and
  reference-fallback regions.

Sources: [NTIRE 2025 Efficient Burst HDR and Restoration](https://arxiv.org/abs/2505.12089), [Recursive Multi-Exposure Alignment](https://openaccess.thecvf.com/content/CVPR2025W/NTIRE/html/Qiu_Recursive_Multi-Exposure_Alignment_with_Spatiotemporal_Decoupling_for_Efficient_Burst_HDR_CVPRW_2025_paper.html), and [NTIRE 2026 Dynamic Multi-Exposure Fusion](https://arxiv.org/abs/2604.09030).

Implementation state (2026-07-27): dynamic pixels now enter a sparse,
measurement-only low/detail estimator after calibrated disagreement,
rejection, or disocclusion triggers. A same-CFA 3x3 binomial kernel avoids
Bayer color mixing; low-frequency radiance and native detail residuals are
robustly estimated from aligned bracket observations, independently
attributed, and recombined inside the measured center-sample radiance
envelope. Static regions retain the ordinary path bit-for-bit. Production
exports independent low/detail source and frequency-state maps, a visible
overlay, portable schema-v2 JSON, signed sidecars, and a
`--no-frequency-deghost` ablation.

The adversarial synthetic gate reduces error from 0.03030242 to 0.00000406,
preserves the measured envelope at every CFA site, and is exactly tile
invariant. A paired release ablation on the private 21-frame, 1310x1304 Z9
stack changed 5.161% of CFA values with MAE 4e-8, RMSE 1e-6, maximum change
0.00029515, and 120.006 dB path parity; 52,157 pixels entered the sparse
frequency path. Two production runs produced ten byte-identical TIFF/PNG
artifacts, including the new maps. These figures prove sparse deterministic
integration, not real-scene quality: the stack has no dynamic ground truth and
used no exact sensor calibration profile.

### 6. Optimize capture, not only post-processing

Adaptive exposure research shows that shutter and ISO should be selected
together under a time budget: long exposure increases photons but risks blur,
while ISO changes amplification and noise behavior. Its experiments also
report that fixing a blurry reference after capture is less effective and can
damage static regions.

TrueShot can implement a deterministic planner without adopting reinforcement
learning:

- Estimate motion, clipping percentiles, noise, and current radiance
  uncertainty from preview and completed brackets.
- Enumerate camera-supported shutter/ISO candidates under time, thermal, and
  motion-blur constraints.
- Select the candidate with maximum expected reduction in worst-case radiance
  uncertainty per millisecond.
- Jointly schedule focus and exposure order to minimize lens travel and scene
  drift while refreshing the reference periodically.
- Stop HDR or focus capture independently once each posterior coverage target
  is met.

Sources: [AdaptiveAE](https://arxiv.org/abs/2508.13503) and [Noise-Optimal Capture for High Dynamic Range Photography](https://people.csail.mit.edu/hasinoff/pubs/hasinoff-hdrnoise-2010.pdf).

### 7. Explicit glare and illumination handling

Retinex-MEF identifies a failure mode that ordinary reflectance/illumination
decomposition misses: overexposure glare changes neighborhoods and can produce
color shifts and detail loss. This matters for TrueShot's glossy objects,
specular materials, and product captures.

TrueShot should add a deterministic glare model:

- Detect highlight bloom from saturated-core distance, wavelength-dependent
  spread, and cross-exposure persistence.
- Exclude flare/glare spread from sharpness evidence.
- Separate low-frequency illumination/glare from shared reflectance before
  exposure selection.
- Preserve physically real specular structure while preventing a bright
  bracket's glare from being mistaken for object detail.

Source: [Retinex-MEF](https://openaccess.thecvf.com/content/ICCV2025/html/Bai_Retinex-MEF_Retinex-based_Glare_Effects_Aware_Unsupervised_Multi-Exposure_Image_Fusion_ICCV_2025_paper.html).

Implementation state (2026-07-27): native fusion now derives a bounded
saturated-core distance field, combines it with summed-area low-frequency
bloom and cross-bracket rejection evidence, and suppresses only contaminated
focus scores and confidence. Verified sensor pitch converts physical
green-channel spread into native pixels; missing/inconsistent geometry uses an
explicit bounded fallback and reports that fact. The exact maximum-evidence
map is exported atomically beside source/state maps. Measured radiance is never
changed by this stage. Production burst processing exposes validated
`--glare-spread-um` calibration and `--no-glare-focus` ablation controls. Real
per-lens/aperture/wavelength calibration and dynamic glossy-object
qualification remain required.

### 8. Benchmark with full-resolution and source-preservation gates

The established MFIF benchmark contains 105 pairs, 30 algorithms, and 20
metrics, while new UHD work shows that models trained or evaluated only at low
resolution can degrade at 4K. No-reference sharpness, entropy, and spatial
frequency can reward oversharpening, so they cannot be release authorities.

TrueShot's benchmark should include:

- Native-resolution Z9 CFA fixtures, not only resized RGB pairs.
- Exact-source contribution, radiance, depth, PSF, occlusion, and color ground
  truth where synthetic.
- MTF50, edge-location error, halo energy by foreground/background side,
  false-color/zipper energy, ghost residual, DeltaE 2000, and uncertainty
  calibration error.
- Stratified results for smooth surfaces, repeated texture, hair/fur, specular
  objects, missing focus planes, large exposure gaps, and moving brackets.
- Apple Silicon p50/p95 wall time, peak RSS, energy, thermals, bytes read, and
  output digest.

Sources: [Multi-focus Image Fusion: A Benchmark](https://arxiv.org/abs/2005.01116) and [UHD-MFF](https://arxiv.org/abs/2606.31242).

The production gate is implemented in
`scripts/run_apple_nef_fusion_qualification.py`. It runs the actual licensed
CLI path in isolated temporary directories, requires Apple Metal AHD with no
fallback and measured-only provenance, hashes every primary artifact, removes
timing fields before semantic-report comparison, and obtains process counters
from macOS `proc_pid_rusage` plus thermal/low-power state from `NSProcessInfo`.
It fails on missing primary energy evidence, low-power mode, serious thermals,
absolute wall/memory/energy ceilings, output nondeterminism, or more than 15%
regression against a supplied baseline.

The source-bound Apple M1 record in
`docs/benchmarks/apple_nef_fusion_qualification_2026-07-27.json` covers one
warmup and five production executions of a private 21-frame native-resolution
1310x1304 Z9 ROI. It measured 5.939/6.073 seconds wall p50/p95, 0.899 seconds
decode p95, 2.901 seconds fusion p95, 0.070 seconds Metal
demosaic/postprocess p95, 310.3 MiB maximum physical footprint, 711.4 MiB
maximum RSS, 41.603 J maximum primary energy, and nominal thermals. All 11
primary artifacts and semantic provenance were exact across runs. This is
performance and integration evidence, not calibrated sensor/lens ground truth,
full-sensor qualification, or a competitor-quality result.

The complementary source-bound full-sensor record is
`docs/benchmarks/apple_nef_full_sensor_qualification_2026-07-27.json`. Three
independent production runs decoded all 21 complete 8280x5520 frames
(959.818 megapixels), released the 1.920 GB native arena before RGB
postprocessing, and produced 11 byte-identical primary artifacts with identical
semantic provenance. Wall p50/p95 was 88.556/91.206 seconds; decode, fusion, and
Metal demosaic/postprocess p95 were 2.422/41.703/1.012 seconds. Maximum RSS,
physical footprint, primary energy, and source page-in amplification were
3.637 GB, 3.403 GB, 609.782 J, and 0.946x, with nominal thermals. The gate was
run from clean revision `d2790400` with exact native geometry, a 4.48 GiB
admission budget, 4 GiB observed-memory ceilings, an 800 J energy ceiling, and
no Metal fallback. This closes the Apple M1/Z9 full-sensor integration and
bounded-performance gap only; it does not establish CPU/Metal reconstructed
channel parity. The private uncalibrated stack also cannot establish sensor/lens
accuracy or superiority over a competitor. Full-sensor parity is the stop-ship
gate in `ROADMAP.md` R0.1.

## Methods Not Suitable for the Default Archival Path

UltraFusion treats missing highlights as guided inpainting and demonstrates
nine-stop inputs. Diffusion HDR and generative multi-focus systems may produce
pleasing results when every source is clipped or a focal plane is missing.
That is generation, not measurement-preserving reconstruction.

TrueShot may later offer an explicitly labeled, local-only creative recovery
mode, but:

- It must never overwrite or masquerade as the archival result.
- Generated pixels need an exported mask and provenance.
- The package must run on customer hardware.
- The model, weights, training data, and redistribution rights require a
  separate legal and security review.

Sources: [UltraFusion](https://arxiv.org/abs/2501.11515), [LF-Diff](https://arxiv.org/abs/2404.00849), and [Generative Multi-Focus Image Fusion](https://arxiv.org/abs/2512.21495).

## Recommended Implementation Order

1. Add calibrated Poisson-Gaussian/censored HDR likelihood and uncertainty.
2. Promote physical focus metadata into the capture manifest and fusion API.
3. Replace frame-index depth with multiscale diopter/PSF response fitting.
4. Add aperture-constrained occlusion correction and boundary-specific gates.
5. Add exposure-aware global plus selective local alignment and deghost overlay.
6. Add the deterministic adaptive exposure/focus acquisition planner.
7. Add glare-aware detail exclusion and low/high-frequency fusion.
8. Evaluate an optional compact LUT accelerator only after deterministic
   ground truth and Apple Metal parity exist.

Items 1-7 now exist in the bounded native architecture. They remain **In
Progress** as a product claim until retained sensor/lens calibration,
redistributable dynamic/occlusion ground truth, live camera actuation, and
direct Helicon/Lightroom qualification pass. This order improved the
correctness model before acceleration and created useful intermediate products
- calibrated uncertainty, metric depth, visibility, and source maps - that can
become differentiating user controls and reconstruction inputs.
