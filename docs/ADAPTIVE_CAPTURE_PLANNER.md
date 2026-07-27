# Adaptive HDR and Focus Capture Planner

TrueShot's deterministic planner is implemented in
`trueshot-core/src/capture/adaptive_planner.rs`. It ranks only
camera-supported shutter, exact-ISO, and focus candidates using expected
posterior information gain per millisecond.

## Evidence Contract

- Radiance probes contain anchored linear-radiance means, variances, CFA site,
  represented pixel weight, and a stable spatial/CFA identity.
- `radiance_anchor_exposure` is the sensor exposure used by those means.
  Candidate sensor signal is computed from the exact exposure ratio.
- Focus probes contain posterior mean and variance in diopters, not camera
  step indices, and retain a stable spatial identity.
- Every candidate is evaluated with an exact measured ISO entry from the
  sensor-noise profile. Unsupported ISO values are rejected, never
  interpolated silently.
- Focus utility is reduced when predicted sensor SNR is poor and becomes zero
  when all represented radiance probes clip.

Completed NEFs can now supply this evidence through the bounded selective RAW
path in `trueshot-core/src/capture/raw_observation.rs`:

- only the requested RAW ROI is decoded;
- a deterministic tile/CFA lattice caps work and memory independently of full
  sensor resolution;
- camera identity, bit depth, exact ISO calibration, black/white levels,
  exposure, ROI, focal length, and aperture fail closed;
- radiance samples are normalized to one anchor exposure with calibrated
  Poisson-Gaussian variance;
- fully clipped probes update the posterior as one-sided censored
  observations through a moment-matched truncated Gaussian, while mixed
  clipping is never misrepresented as a bound on a textured tile mean;
- same-CFA two-pixel focus energy is noise-whitened and exposure-normalized;
- repeated frames reduce posterior variance by precision accumulation; and
- three or more physically distinct, nonuniform diopter planes produce a
  continuous log-response peak and propagated uncertainty.

The accumulator rejects camera, calibration artifact, ROI, focal-length, or
aperture drift. Near-identical focus metadata is merged within a tight
diopter tolerance so metadata jitter cannot fabricate extra focus planes.

## Local Measured Session

`MeasuredAdaptiveSession` is the transactional state machine between the
planner and a camera adapter:

1. a retained reference NEF initializes calibrated radiance evidence and a
   bounded prior over the camera-supported focus candidate coordinates;
2. exactly one next candidate is staged;
3. the completed RAW must match that candidate's ISO, shutter, and physical
   focus metadata;
4. RAW assimilation, measured elapsed/motion/thermal telemetry, provenance,
   and replanning are computed on cloned state; and
5. the transition commits only after every check succeeds.

Rejected or malformed RAWs leave the session unchanged. Automatic quality,
information, budget, and calibration termination is attributed. Operator and
hardware interruption can terminate without claiming that the staged action
executed.

The authenticated local server exposes this state machine at
`/api/cameras/adaptive`. It derives candidates from the connected camera's
declared shutter/ISO options, accepts at most 4,096 API candidates and 32
active sessions, selectively decodes only project-local regular NEFs off the
async runtime, blocks symlink escapes, enforces the advanced-capture
entitlement on every route, and uses generation checks to reject concurrent
session advancement.

Every accepted start, assimilation, or termination is persisted before the
live generation advances. Checkpoints are immutable, schema-versioned
generations under the local projects directory. Publication uses a unique
same-directory partial, file sync, no-replace hard-link publication, directory
sync, and bounded two-generation retention. Each envelope seals the canonical
payload with SHA-256; restore revalidates sensor identity, posterior
accumulators, contiguous frame attribution, termination state, and the
deterministically recomputed next decision. Startup removes orphan partials,
loads the newest valid generation, and can discard a corrupt newest generation
only after an older generation passes every restore invariant. If publication
fails, the in-memory session remains at its prior generation.

The API deliberately returns an absolute focus-diopter request rather than
converting it to an uncalibrated relative lens step. Until a body/lens pair has
an absolute drive calibration and EXIF readback qualification, the existing
relative `drive_focus` adapter is not authorized to execute that request.

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

- Map selected diopters to calibrated lens-drive commands and verify achieved
  focus from captured RAW metadata; unsupported lenses must fail closed.
- Feed measured motion, readout/settle latency, thermals, and lens travel time
  back into the posterior after each capture.
- Connect the session API to the dashboard capture flow after absolute
  body/lens focus-drive calibration is available.
- Validate at least 20% capture-time reduction at equal quality, or higher
  quality at equal time, on the preregistered real stack corpus.
