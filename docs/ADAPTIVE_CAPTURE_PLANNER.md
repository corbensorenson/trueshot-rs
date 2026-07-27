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
does not force redundant captures for the other. Each objective also has an
explicit maximum posterior-variance target, preventing low-value overcapture
after the requested quality has already been reached. Ties are deterministic
and prefer lower ISO, shorter shutter, then lower focus coordinate.

## Camera Candidate Contract

`build_camera_candidates` converts camera-declared shutter and ISO strings plus
verified focus diopters into a bounded canonical grid. It accepts numeric
seconds and fractions such as `0.5s` and `1/125`, accepts `100` and `ISO 100`,
deduplicates exact values, and reports rejected options such as `Auto`, `Bulb`,
or malformed strings. It never guesses a numeric setting.

The grid is limited to 100,000 entries and every candidate is validated before
planning. Duplicate candidate records are rejected.

## Decision And Manifest Provenance

Every decision contains a canonically ordered evaluation for every supplied
candidate:

- eligible candidates retain HDR information, focus information, full capture
  cost, and utility per millisecond;
- rejected candidates retain the exact reason: missing exact-ISO calibration,
  time budget, thermal budget, or motion blur;
- aggregate counters are validated against the detailed evaluations;
- the selected action must exactly match an eligible evaluation.

The compact `trueshot.adaptive-capture.v1` trace records each posterior,
decision, and retained frame index. A completed capture group must declare a
validated termination reason: quality targets reached, marginal information
exhausted, resource budget exhausted, operator stop, or hardware failure.
Duplicate/out-of-range frame attribution and false termination claims fail
manifest validation. The trace is optional for legacy groups and remains
bounded by the streaming manifest record limit.

## Synthetic Closed-Loop Gate

The deterministic hardware-loop simulation compares adaptive selection against
a conventional fixed grid of three HDR exposures at seven focus positions.
Both paths must reach radiance and focus variance targets of 0.005. The current
baseline reaches them in 178 ms of modeled exposure/readout/settle/lens travel
versus 1,027 ms for the fixed 21-shot grid, an 82.7% reduction. This is a
synthetic regression gate, not a real-camera performance claim.

## Archival Policy

The planner only chooses measurements. It does not reconstruct missing
highlights or focus detail, and it does not authorize generative content in
archival output.

## Remaining Production Integration

- Build compact radiance/focus posterior probes from preview and completed RAW
  captures.
- Map selected diopters to calibrated lens-drive commands and verify achieved
  focus from captured RAW metadata; unsupported lenses must fail closed.
- Feed measured motion, readout/settle latency, thermals, and lens travel time
  back into the posterior after each capture.
- Wire the planner/trace contract into the production server camera adapter.
- Validate at least 20% capture-time reduction at equal quality, or higher
  quality at equal time, on the preregistered real stack corpus.
