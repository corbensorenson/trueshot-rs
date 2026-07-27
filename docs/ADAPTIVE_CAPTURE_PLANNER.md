# Adaptive HDR and Focus Capture Planner

TrueShot's deterministic planner is implemented in
`trueshot-core/src/capture/adaptive_planner.rs`. It ranks only
camera-supported shutter, exact-ISO, and focus candidates using expected
posterior information gain per millisecond.

## Evidence Contract

- Radiance probes contain anchored linear-radiance means, variances, CFA site,
  and represented pixel weight.
- `radiance_anchor_exposure` is the sensor exposure used by those means.
  Candidate sensor signal is computed from the exact exposure ratio.
- Focus probes contain posterior mean and variance in diopters, not camera
  step indices.
- Every candidate is evaluated with an exact measured ISO entry from the
  sensor-noise profile. Unsupported ISO values are rejected, never
  interpolated silently.
- Focus utility is reduced when predicted sensor SNR is poor and becomes zero
  when all represented radiance probes clip.

## Constraints

Candidates are rejected before ranking when they exceed:

- motion blur in pixels;
- remaining wall-clock budget;
- thermal budget; or
- exact sensor-calibration coverage.

Capture cost includes exposure, readout, settling, and lens travel. HDR and
focus have independent stopping thresholds, so completion of one objective
does not force redundant captures for the other. Ties are deterministic and
prefer lower ISO, shorter shutter, then lower focus coordinate.

## Archival Policy

The planner only chooses measurements. It does not reconstruct missing
highlights or focus detail, and it does not authorize generative content in
archival output.

## Remaining Production Integration

- Build compact radiance/focus posterior probes from preview and completed RAW
  captures.
- Parse camera-supported shutter and ISO choices into exact numeric
  candidates and map selected diopters to calibrated lens drive commands.
- Feed measured motion, readout/settle latency, thermals, and lens travel time
  back into the posterior after each capture.
- Persist every candidate set, rejection reason, utility, selected action, and
  independent stopping decision in the capture manifest.
- Validate at least 20% capture-time reduction at equal quality, or higher
  quality at equal time, on the preregistered real stack corpus.
