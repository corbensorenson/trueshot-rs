# TrueShot Product Roadmap

Last reconciled: 2026-07-27

This is the authoritative execution order for TrueShot. `upgrade_list.md` is the
detailed red-team ledger and `docs/FEATURE_MATRIX.md` is the customer-capability
inventory. A capability is not promoted to Shipping, or used in a superiority
claim, until its roadmap gate passes at the scale customers actually use.

## Product Contract

TrueShot is a local-first visual-computing library with a built-in GUI, CLI, and
future Python API. Capture, decoding, fusion, reconstruction, editing, and export
run on customer hardware. The hosted surface is limited to sales, licensing,
entitlement delivery, and updates.

The product contract is:

- Preserve measured evidence. Archival defaults do not invent highlights,
  focus, geometry, or texture.
- Make uncertainty, rejected sources, fallbacks, calibration state, and manual
  intervention inspectable.
- Optimize for complete customer workflows, not isolated kernels.
- Gate claims at production geometry, bit depth, thermal state, and memory
  pressure. A small synthetic pass cannot qualify a full-sensor path.
- Keep features visible but entitlement-gated so users can evaluate upgrades
  without receiving unpaid functionality.
- Prefer one validated core semantic path shared by GUI, CLI, and Python API.

## Gate 0: Stop-Ship Correctness And Security

These items block an external paid pilot.

### R0.1 Make Metal demosaic parity true at production resolution

Status: In progress; the reproduced Apple M1 correctness defect is fixed and
production-scale nominal/HDR evidence is green as of 2026-07-27.

Current evidence:

- CPU and WGSL now derive homogeneity decisions from the same quantized Bayer
  measurements, fixed-point camera-to-XYZ/Lab transform, integer interpolation,
  and overflow-bounded chroma metric. Float directional candidates remain the
  selected high-precision output.
- The unqualified one-count decision margin was removed. Qualification compares
  explicit CPU/Metal direction maps and fails on any mismatch.
- The retained Apple M1 release gate now defaults to `8256x5504`, 11 bands, and
  runs both nominal and `6.25x` HDR-linear workloads over a full-frame
  adversarial composite.
- Nominal/HDR each report zero direction mismatches, zero values over the
  `0.001` tolerance, exact measured CFA samples, and maximum normalized errors
  of `4.1723251e-7`/`2.9802322e-7`.
- Nominal/HDR p50 speedups are `2.28x`/`2.45x`; p95 speedups are `2.10x`/`2.94x`.
  Admitted scratch remains `484,790,336` bytes.
- Source-bound evidence is retained in
  `docs/benchmarks/apple_metal_ahd_qualification_2026-07-27.json`.

Required work:

- Keep the shared integer classifier, overflow-safe chroma distance, zero-margin
  selection, and adversarial full-frame fixture release-blocking.
- Provision a dedicated Apple Silicon runner and make the full-sensor nominal
  and HDR script mandatory rather than conditionally skipped by hosted CI.
- Add retained energy and thermal-state collection to the isolated gate.
- Qualify every advertised sensor geometry and Apple Silicon generation; disable
  Metal for unqualified adapters/geometries.
- Keep deterministic CPU fallback for every regime not represented by retained
  production-scale evidence.

Exit evidence:

- Zero direction-selection mismatches and no value above the declared tolerance
  at every supported full-sensor geometry.
- Exact measured CFA samples, including HDR-linear input.
- Full-sensor gate fails CI on parity, seam, fallback, memory, or performance
  regression.

### R0.2 Remove synchronous blocking from authentication middleware

Status: Complete 2026-07-27.

Implementation evidence:

- Bootstrap and API-token storage lookups execute inside the middleware's
  returned async future; no Tokio runtime bridge remains.
- The wrapped service is reference-counted so it can be called safely after
  authentication awaits without blocking an Actix worker.
- Actix tests cover bootstrap enable/disable transitions, sixteen concurrent
  token requests, unavailable storage, and invalid token states.

Exit evidence:

- `cargo test -p trueshot-server auth::tests` passes.
- `cargo clippy -p trueshot-server -- -D warnings` passes.
- Source inspection finds no `block_in_place` or `Handle::block_on` in auth
  middleware.

### R0.3 Make API-token scopes enforce authority

Status: Complete 2026-07-27.

Current evidence:

- API tokens now retain an explicit principal kind and their active owner's
  current role rather than receiving an unconditional Admin identity.
- Middleware and `require_admin` both enforce a centralized, fail-closed route
  policy for `read`, `capture`, `process`, `export`, `license`, and `admin`.
- New tokens default to `read`; unsupported or ambiguous wildcard scope sets are
  rejected. Revoked, expired, inactive-owner, missing-owner, and unavailable
  storage states fail closed.
- A generated matrix walks the compiled OpenAPI operations and all Actix route
  declarations under `api/` and `guest/`, normalizes catch-all templates, and
  fails when an API handler is absent from policy coverage. It evaluates
  anonymous/public, guest, interactive Admin invariants, wildcard, and every
  narrow token scope. Scale-anchor and XR operations were added to OpenAPI; the
  two intentionally non-OpenAPI browser routes are explicitly pinned public.

Exit evidence:

- A read-only token is rejected by every mutation route.
- Revoked, expired, disabled-owner, and deleted-owner tokens fail immediately.
- An automated route matrix covers anonymous, guest, interactive admin, and
  narrowly scoped API-token principals.

### R0.4 Eliminate project-file symlink escape

Status: In progress.

Current evidence:

- Unix project reads now walk from a canonical root descriptor with `openat`,
  `O_NOFOLLOW`, directory-only intermediate components, regular-file and
  single-link checks. The opened descriptor, not the pathname, remains the
  authority for the response.
- Raw, processed, output, fusion-artifact, bounded report/IMU/metadata, and
  public-share reads use the rooted descriptor path. TSE2 readers authenticate
  and stream directly from that descriptor with no plaintext sibling.
- Project metadata replacement uses a same-directory descriptor-relative
  temporary, `fsync`, and `renameat`, so a final symlink is replaced rather than
  followed.
- Project creation is exclusive and mode `0700`. Annotation layers, edit
  histories, mesh/splat edit inputs and outputs, calibration frame/color/profile
  I/O, multipart upload publication, share LOD inputs and outputs, queued fusion
  report/edit reads, and wizard background/plan/burst/SD-import artifacts now use
  descriptor-rooted reads or staged publication. Encrypted edit and LOD outputs
  are transformed into authenticated `TSE2` before their plaintext staging file
  is ever published.
- Staged uploads remain hidden until quota and MIME validation complete.
  No-replace commit is atomic, concurrent writers cannot replace a winner, and
  mutation history updates are serialized.
- Focused tests reject final/intermediate symlinks and hard links, preserve
  clear and encrypted bytes across post-open pathname swaps, exercise bounded
  HTTP ranges, prove atomic metadata writes do not touch a symlink target, and
  cover exclusive project/file creation, rooted deletion, encrypted streaming,
  descriptor-backed mesh/splat/GLB codecs, and attribute-preserving point LODs.
- Unix project asset and fusion-report inventory now uses a bounded
  descriptor-rooted `openat` walker. It obtains size/time from opened
  single-link regular files, omits links, never enters redirected directories,
  and supplies fusion artifact presence without pathname recanonicalization.
  Whole-project quota accounting uses the same walker and fails closed on
  excessive entry count or size overflow.
- RAW purge recursively deletes with descriptor-relative `unlinkat` and never
  follows nested or final symlinks. Calibration compute opens every frame under
  the `_calibration` descriptor root, enforces 64-frame, 128 MiB per-frame, and
  2 GiB aggregate bounds, and passes encoded bytes to OpenCV `imdecode` rather
  than allowing a later filename reopen.
- A direct Actix fusion-artifact regression serves a real safe artifact and
  rejects final and intermediate symlink escapes without leaking outside bytes.
  The complete server suite passes 67 tests and strict all-target server and
  calibration Clippy is clean on macOS. The OpenCV calibration feature also
  compiles when pointed at Xcode's installed `libclang`; dependency-generation
  unification and provisioned CI remain tracked separately.
- Queued fusion calibration profiles are opened from the project descriptor
  root before launch, decrypted from that retained descriptor when necessary,
  bounded, checked against the replay SHA-256, held in zeroizing memory, and
  sent inline through the bounded child stdin envelope. The packaged CLI
  independently enforces exact profile presence, size, digest, and typed schema
  before processing. A pathname-swap regression proves an encrypted profile
  remains bound to the originally opened descriptor and fails under a wrong
  key.
- Remaining: make RAW traversal and output publication descriptor-native
  across the packaged fusion child boundary, and implement equivalent
  race-safe handles on non-Unix platforms.

Required work:

- Replace file resolution with canonical, no-follow open semantics rooted in a
  canonical project directory.
- Apply one helper to raw, processed, output, fusion, calibration, share, and
  download paths.
- Add intermediate-directory and final-file symlink, swap-race, hard-link, and
  encrypted-file tests.

Exit evidence:

- No API can read or write outside the project root through links or races.
- The direct fusion-artifact route has an end-to-end escape regression test.

### R0.5 Restore honest standalone feature builds

Status: Complete 2026-07-27.

Implementation evidence:

- `gpu` explicitly enables `wgpu`; the Metal qualification example declares
  `required-features = ["gpu"]`.
- WGPU-only compute and live-hybrid modules are feature-gated, while the
  no-default CPU Gaussian rasterizer imports its complete SH basis.
- `scripts/check_core_feature_builds.sh` runs default, no-default, isolated
  `wgpu`, isolated `gpu`, all-target, and example checks in CI.

Exit evidence:

- `scripts/check_core_feature_builds.sh` passes locally and is a required
  cross-platform workflow step.

### R0.6 Close existing dependency and native-feature release blockers

Status: Open; tracked in `upgrade_list.md` items 154 and 155.

Required work:

- Converge the three OpenCV binding generations and test explicit native
  feature combinations on provisioned macOS, Linux, and Windows runners.
- Remove or time-bound every exploitable Rust/dashboard advisory, yanked crate,
  unknown source, and unreviewed license.
- Retain machine-readable per-target audit evidence for each release.

## Gate 1: Paid Beta Image Fidelity

### R1.1 Make the camera color model part of demosaic and export

Status: Open.

Current evidence:

- The production burst path passes an identity `rgb_cam` into AHD.
- AHD's Lab homogeneity decisions therefore use a fictitious color space.
- Archive output remains untagged camera-linear RGB.

Required work:

- Derive a bounded camera-to-XYZ/working-space transform from validated DNG/ICC
  metadata or a signed camera profile.
- Use the same transform in CPU/Metal demosaic, postprocess, preview, TIFF/DNG,
  and future Python bindings.
- Add chart holdout, DeltaE 2000, neutral, saturated-edge, and cross-application
  ColorSync/Lightroom/Photoshop gates.

Exit evidence:

- Profile identity and calibration state are in provenance.
- Unsupported cameras are explicitly camera-linear, not mislabeled.
- Preregistered chart and cross-application color gates pass.

### R1.2 Settle joint multi-frame CFA reconstruction versus merged-Bayer AHD

Status: Open architecture decision, benchmark first.

Current evidence:

- `joint_demosaic.rs` says focus/HDR stacks should accumulate measured CFA
  samples directly into RGB rather than demosaic a merged Bayer mosaic.
- The production native burst path currently fuses one Bayer mosaic and runs AHD.

Required work:

- Build one evaluation harness over identical aligned stacks and compare:
  current AHD, a strong non-generative single-frame fallback such as RCD/AMaZE,
  and direct multi-frame CFA reconstruction with uncertainty-aware kernels.
- Measure PSNR/SSIM, DeltaE, zipper/moire, false color, MTF, halo energy,
  highlight behavior, low-light detail, motion fallback, wall time, memory, and
  energy on synthetic and legally usable real stacks.
- Make direct measured-CFA reconstruction the burst primary if it wins the
  preregistered suite; keep a qualified single-frame path for incomplete groups.

Exit evidence:

- One evidence-backed architecture decision is recorded.
- GUI, CLI, and Python API use the same selected core.
- No broad "state of the art" or competitor claim exceeds the retained results.

### R1.3 Complete real optical and sensor qualification

Status: In progress; detailed in `upgrade_list.md` items 157 and 158.

Required work:

- Retain per-ISO/per-CFA dark, flat, chart, integrating-sphere, lens-PSF, glossy,
  hair/fur/transparency, and dynamic bracket stacks.
- Qualify posterior coverage, physical depth, MTF, halo energy, glare
  suppression, disocclusion, fallback, and measured-only archival behavior.
- Execute the preregistered Helicon Focus and Lightroom comparison protocol.

Exit evidence:

- TrueShot beats the best preregistered competitor result on the primary metric
  for at least 80% of scenes without a safety-metric regression, or marketing
  states the measured tradeoff.

## Gate 2: General Availability Reliability

### R2.1 Harden credentials, sessions, proxies, and public links

Status: In progress.

Current evidence:

- JWT HMAC secret loading now uses explicit environment, permission-checked
  file, then persistent OS keychain precedence. Production refuses to create a
  missing keychain secret; minimum/maximum key length, regular-file,
  ownership, and `0600` checks are covered.
- Bootstrap API keys are compared as fixed-size SHA-256 digests with
  constant-time equality. Pairing and short-code alphabets use unbiased
  sampling.
- Rate limiting trusts `X-Forwarded-For` only when the socket peer belongs to a
  validated `server.trusted_proxy_cidrs` network. It strips trusted hops from
  the right and falls back to the socket peer for absent or malformed chains.
- Password login now uses a domain-separated hash of the normalized account
  identity and an atomic SQLite failure record. Five failures within 15 minutes
  trigger a 30-second lock, subsequent failures back off exponentially to one
  hour, state survives manager/server restart, unknown identities perform an
  Argon2 verification against a dummy hash, successful login clears state, and
  the API returns generic credentials text with `429` plus `Retry-After`.
- Access JWTs now carry a unique JTI and persistent per-subject session
  generation. Individual logout records a bounded-expiry JTI revocation;
  logout-all atomically advances the subject generation before deleting
  refresh sessions; refresh sessions are bound to their issuance generation,
  so an in-flight stale refresh cannot survive logout-all. Actix middleware and
  direct camera-stream token verification query this persistent authority on
  every request. JTI, generation, refresh, subject isolation, middleware, old
  schema migration, and manager-restart regressions pass.
- The historical Axum LiveHybrid source is neither compiled nor mounted by the
  product server. Its stale signature-only verifier reference was replaced with
  the shared manager contract, but it is not counted as shipping; port/mount or
  removal is tracked explicitly in `upgrade_list.md` items 36 and 38.
- Public gallery rows no longer persist raw bearer tokens. Gallery and short
  URLs use deterministic HMAC-SHA256 aliases while SQLite stores only the
  underlying private-token hash and alias hash. Startup derives/backfills every
  alias before serving, enables SQLite secure deletion, clears the legacy
  plaintext column, truncates WAL state, and vacuums changed databases.
  Original private links remain valid, while alias-aware asset, metadata,
  consumption, analytics, listing, and short-link flows resolve through the
  hash mapping. A database-leak regression restores a legacy bearer and proves
  it absent from the database, WAL, shared-memory, and journal candidates after
  migration while alias access remains functional.

Required work:

- Add environment/file/keychain precedence for the HMAC secret, enforce length
  and file permissions, and refuse ephemeral production secrets. If
  container/headless deployment is not supported, remove that advertised path.
- Trust forwarded client IPs only from configured proxy ranges and add
  per-account login backoff/lockout.
- Compare API keys in constant time.
- Add access-token version or `jti` revocation, not refresh-only revocation.
- Stop retaining raw public share tokens where a hash or one-time reveal works.
- Replace the macOS no-op unsafe anti-debug check with a tested implementation
  or remove the claim and code.

Exit evidence:

- Restart, key loss, revocation, proxy spoofing, brute force, and database leak
  tests demonstrate bounded failure.

### R2.2 Stop exposing internal failures to clients

Status: Complete 2026-07-27.

Implementation evidence:

- The outer Actix middleware assigns a fresh UUID correlation ID and replaces
  every produced 5xx body with one exact JSON envelope. Stable codes distinguish
  internal, unsupported-operation, upstream, unavailable, and timeout failures.
- Rewritten failures retain only an explicit safe-header allowlist for retry and
  CORS behavior, set `Cache-Control: no-store`, and discard arbitrary internal
  headers. Non-5xx status, body, and semantics are unchanged.
- Request traces and failure logs use matched route templates, never concrete
  paths containing share bearers or user identifiers. Audit failures use a
  correlation-aware redacted operation logger.
- Public-share database failures, storage persistence failures, and
  Google/Dropbox/OneDrive OAuth exchange/profile failures no longer construct
  internal error text for a response.

Exit evidence:

- Leakage tests cover plaintext and JSON failures, SQL/path/token/provider
  secrets, arbitrary internal headers, handler errors, CORS/retry preservation,
  exact response shape, valid UUID correlation, and unchanged 400 bodies.
- Source-policy tests prevent concrete-path request tracing and detailed
  share/OAuth failure bodies from returning.
- All 77 server tests and strict server/CLI all-target Clippy pass.

### R2.3 Raise server and lint coverage to match the attack surface

Status: In progress as of 2026-07-27.

Current evidence:

- The API surface contains roughly 139 route annotations across 18 files.
- Tests are concentrated in a small subset of server modules.
- The workspace root now warns on `dead_code`, `unused_variables`, and
  `unused_imports`; all-target strict Clippy treats those warnings as errors.
  The core crate's blanket suppressions are removed. Retained deterministic
  benchmark baselines use narrow `#[expect]` annotations with reasons, which
  fail if the exception stops being necessary.
- The first forced-lint sweep found three implemented but unmounted scan routes
  and two camera-control placeholders. Scale-anchor GET/POST and scan coverage
  are now mounted and authorization-matrix covered; normalized focus-point and
  autofocus requests now dispatch to the real camera adapter.
- Redundant queue payload/database fields and duplicate hidden license snapshot
  fields were removed.
- Production server mutexes no longer use panic-on-poison lock unwraps.
  License, audit-chain, rate-limit, system-stat, and turntable authority fails
  closed with typed unavailable behavior. The non-authoritative webhook dedup
  cache is explicitly cleared and its poison state reset before resuming.
  Fault-injection tests cover fail-closed and recoverable policies.
- The 2026-07-27 macOS gate passed all 81 server tests, all 523 workspace tests
  and doctests (0 failed, 6 ignored), plus
  `cargo clippy --workspace --all-targets -- -D warnings`.

Required work:

- Generate a route authorization matrix test and require one behavioral
  success/failure test per public operation.

Exit evidence:

- Route coverage inventory is complete and release-blocking.
- Strict lint passes without workspace-wide dead/unused suppression. Complete.
- Long-lived server mutex poison behavior is explicit and regression-tested.
  Complete.

### R2.4 Finish packaging and update safety

Status: Open.

Required work:

- Produce signed/notarized macOS installers and entitlement-aware module
  manifests from one universal product build.
- Verify local server lifecycle, browser launch, rollback, offline grace,
  license refresh, and update signature on clean macOS machines.
- Keep the code package common where practical; enforce paid capability at
  signed entitlement boundaries rather than maintaining combinatorial binaries.

## Gate 3: Moat And Scale

### R3.1 Optimize GPU only after correctness and architecture gates

Status: Deferred behind R0.1 and R1.2.

If Metal AHD remains useful:

- Reuse engine-owned buffers.
- Double-buffer band submission/readback.
- Remove host validation/normalization passes from the hot path.
- Pack Lab and candidate buffers without losing deterministic parity.
- Set a go/no-go target before implementation: full-sensor p50 and p95 at least
  `4x` CPU with less than `256 MiB` admitted scratch on the reference M1, no
  quality regression, and no fallback.

If the target is missed, retire Metal AHD as the burst primary rather than
carrying a second complex implementation for a marginal gain.

### R3.2 Make TSE2 selective reads workload-aware

Status: Measure before changing.

- Record chunk hit rate and decrypted-byte amplification for real NEF access
  patterns.
- Add a bounded 2-4 entry LRU and readahead only if retained evidence beats the
  one-chunk cache without violating memory/privacy bounds.

### R3.3 Ship one stable automation surface

Status: Planned.

- Generate a versioned Python API from the same typed operation schemas used by
  GUI and CLI.
- Expose capability/entitlement discovery, dry-run resource estimates,
  deterministic manifests, cancellation, provenance, and bounded local
  execution.
- Add compatibility tests so GUI, CLI, and Python produce identical semantic
  reports for the same workflow.

## External Review Reconciliation

The external reviews `REVIEW_2026-07-27.md` and `CODEX_REVIEW_ROUND2.md` are
evidence sources, not specifications.

Accepted and independently verified:

- Full-sensor Metal parity failure and undersized retained gate.
- Standalone GPU example/feature breakage.
- Actix middleware blocking bridge.
- API-token Admin/scope bypass.
- Identity camera matrix.
- Lexical project-file resolver used by fusion artifact download.
- Keyring-only HMAC source.
- Server error leakage, proxy trust, refresh-only revocation, raw public-token
  retention, broad lint allowances, and no-op macOS anti-debug code.

Already fixed or not carried as open work:

- Replay `quality` argument injection: the replay capsule now allowlists
  `low|medium|high|ultra` before command construction.
- At-rest master-key sourcing: environment/file/keyring precedence and
  production fail-closed behavior are implemented.
- HDR measured-CFA round-trip: exact power-of-two normalization is implemented;
  the full-sensor backend parity failure remains separate and open.
- TSE2 format design and fusion-revision subprocess containment were reviewed
  favorably; only measured cache tuning remains.
- The claim that scratch omits every host band is stale. Current accounting
  includes the reusable normalization band; end-to-end admission still remains
  subject to full-sensor regression gates.

## Roadmap Maintenance

- Update this file when priority, scope, or an exit gate changes.
- Append implementation evidence and detailed findings to `upgrade_list.md`.
- Update `docs/FEATURE_MATRIX.md` only after current evidence supports the
  customer-facing status.
- Never mark a roadmap item complete from a narrow unit test when the item is a
  production-resolution, hardware, security, or competitor claim.
