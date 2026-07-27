## TrueShot Production Readiness

This checklist is the minimum bar for a production deployment. It assumes `TRUESHOT_ENV=production`.

### Rust Toolchain Contract

- Minimum supported Rust version (MSRV): 1.80.
- Release validation uses the current stable Rust toolchain; the 2026-07-26 baseline was verified with Rust 1.90.0.
- Required gates:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `./scripts/check_rust_future_incompat.sh`
- Optional OpenCV and platform-specific feature combinations require their native SDKs and dedicated feature-matrix runners; they are not implied by the default workspace gate.

### Required Security Configuration

- Set `server.cookie_secure=true`.
- Configure TLS:
  - Provide `server.tls.cert_path` and `server.tls.key_path`, or
  - Set `server.tls_proxy=true` and ensure `server.public_base_url` is `https://...`.
- Set `server.allowed_origins` to your dashboard origins (no localhost).
- If and only if a reverse proxy terminates requests, set
  `server.trusted_proxy_cidrs` to the proxy network(s). Forwarded client IPs are
  ignored for every other socket peer; do not use broad public CIDRs.
- Provide one persistent JWT HMAC secret source:
  - `TRUESHOT_HMAC_SECRET` containing at least 32 random bytes encoded as base64,
  - `TRUESHOT_HMAC_SECRET_FILE` or `server.hmac_secret_path` pointing to a
    current-user-owned `0600` regular file containing raw or base64 key bytes,
    or
  - a pre-provisioned `trueshot/server_hmac_secret` OS keychain entry.
  - Production startup never creates this secret implicitly.
- Exercise packaged password-abuse controls against the persistent auth
  database: five failures in 15 minutes must return generic credentials text
  and activate a restart-persistent `429` with `Retry-After`; a valid login
  after lock expiry must clear the failure record.
- Verify access-token revocation against the packaged auth database: one logout
  must reject that JTI after restart while leaving sibling sessions valid;
  logout-all must reject every older subject generation and stale refresh token
  while leaving unrelated subjects valid.
- Verify the packaged server's opaque-failure contract: every 5xx must return
  the stable JSON error envelope and matching UUID `X-Correlation-ID`, while
  SQL, provider payloads, absolute paths, bearer-bearing concrete routes, and
  arbitrary internal headers remain absent. Browser clients must be able to
  read the exposed correlation header.
- On first launch after upgrading a database with public galleries, require the
  public-share migration to finish before binding the HTTP listener. Verify the
  legacy `public_token` column is empty, aliases still resolve, and no former
  bearer appears in the database or SQLite sidecar files.
- Provide an audit anchor in production:
  - `privacy.audit_anchor_url` must be set or startup will fail.
- Set a master key for encryption-at-rest if enabled:
  - `TRUESHOT_MASTER_KEY` (base64, 32 bytes) or `privacy.encryption_master_key_path`.
- New encrypted assets use authenticated-seekable `TSE2`. Inventory and migrate
  legacy `TSE1` RAW before enabling measured refusion; bounded legacy report
  reads remain supported.

### Baseline Observability

- Logs:
  - Logs write to `logs/trueshot.log` and stdout by default.
  - Control stdout via `TRUESHOT_LOG_STDOUT=false` if needed.
- Metrics:
  - Enable `server.metrics_enabled=true` and set `server.metrics_path`.
- Tracing:
  - Configure `server.telemetry_otlp_endpoint` or `OTEL_EXPORTER_OTLP_ENDPOINT`.

### Storage & Quotas

- Ensure `paths.projects_dir` and `paths.auth_db`/`jobs_db`/`inventory_db` live on durable storage.
- Set `server.max_upload_bytes`, `server.max_project_bytes`, `server.min_free_bytes`.
- Use external storage backups (NAS/S3/GCS/Azure) and validate restores.

### Optional Multi-Node Redis

- Leave `server.redis_url` unset for the default local-only deployment; Redis is never required for capture, processing, reconstruction, or export.
- Configure Redis only when multiple local TrueShot nodes need shared calibration caching and event relay.
- Tune `redis_connect_timeout_ms`, `redis_response_timeout_ms`, `redis_reconnect_initial_ms`, `redis_reconnect_max_ms`, and `redis_event_buffer_capacity` for the studio network.
- Redis failure is bounded and non-fatal: cache requests fail open to local storage and the event bridge reconnects with capped backoff.
- Release validation runs cache, degraded-offline, cross-node relay, and forced-disconnect recovery against pinned `redis:7.4.10-alpine`.

### High-Volume NEF Processing

- Prefer capture-time JSONL manifests over legacy directory discovery.
- Size native admission with `TRUESHOT_MEMORY_BUDGET_MIB`; the default is 75% of currently available memory.
- Keep `TRUESHOT_NEF_ACCESS_MODE` unset for sidecar-free one-pass ingestion.
- Use `TRUESHOT_RESUME_VERIFY=sampled` for normal operation and `full` for validation or regulated workflows.
- Run `capture_manifest_scale_benchmark`, `nef_group_benchmark --verify-full`, and `nef_corruption_runner` on release hardware before advertising a camera/storage combination.
- Keep `scripts/run_apple_metal_ahd_qualification.sh` passing at every advertised
  production sensor geometry on dedicated Apple Silicon release runners. The
  retained 8256x5504/11-band M1 record gates adapter/backend, alternating
  p50/p95 speedup, exact measured CFA values, explicit direction-map equality,
  reconstructed-channel tolerance, bounded scratch, checksums, and a mandatory
  full-sensor 6.25x HDR case with exact power-of-two normalization.
- The independently reproduced full-sensor Metal direction-flip defect is fixed
  on the retained Apple M1 adversarial fixture: nominal/HDR have zero direction
  mismatches and roundoff-scale normalized output error. `ROADMAP.md` R0.1
  remains open until the gate is mandatory on a dedicated runner, records
  energy/thermals, and covers every supported Apple generation and geometry.
  Unqualified regimes must use deterministic CPU fallback.
- Run `scripts/run_apple_nef_fusion_qualification.py <fixture> --record <record.json>` on every supported Apple Silicon class. Staging without a production signing key may add `--dev-license`; the record labels that build and the feature is absent from ordinary release builds. The runner requires a clean tracked source revision and at least three independent production executions. It gates wall p95, RSS, physical footprint, primary energy, thermal/low-power state, source page-in amplification, admission budget, oversized-arena release, exact requested geometry/decoded extent, Apple Metal AHD without fallback, measured-only archival provenance, and exact primary artifact plus semantic-report determinism. Use the prior record with `--baseline` to block regressions above 15%.
- The retained Apple M1 baseline uses one warmup and five measured executions of a private 21-frame native-resolution 1310x1304 Z9 ROI. It passes 8-second wall-p95, 384 MiB physical-footprint, 832 MiB RSS, 100 J, and fair-or-better thermal ceilings; observed wall p50/p95 is 5.939/6.073 seconds. This qualifies production integration and performance, not calibrated image quality or full-sensor/cross-generation support.
- The retained Apple M1 full-sensor baseline uses three independent executions
  of all 21 8280x5520 Z9 frames. It passes 120-second wall-p95, 4 GiB observed
  RSS/physical-footprint, 800 J, 1.25x page-in-amplification, exact
  geometry/artifact/semantic-provenance, and fair-or-better thermal ceilings;
  observed wall p50/p95 is 88.556/91.206 seconds, maximum RSS/physical
  footprint is 3.637/3.403 GB, and thermals remain nominal. The 1.920 GB input
  arena is released before postprocessing. This is private, uncalibrated
  integration/performance evidence and does not qualify CPU/Metal parity,
  optical accuracy, competitor superiority, or other Apple generations.
- Treat `docs/NEF_SUPPORT_MATRIX.md` as the authoritative advertised camera/layout boundary.
- Do not advertise Nikon bodies, firmware, compression modes or bit depths until item 148 in `upgrade_list.md` is satisfied for that combination.

### Deployment Checklist

- Build: `cargo build --release --workspace`.
- Configure system limits (open files, memory, CPU).
- Run the service under a supervisor (systemd, Kubernetes, etc).
- Validate `/api/health` and `/api/metrics` (if enabled).
- Confirm CORS and CSRF behavior from the dashboard origin.
- Local-first boundary: confirm that all capture/reconstruction/render/export workloads run on customer hardware (see `docs/LOCAL_FIRST_POLICY.md`).
- Signed updates:
  - Generate release bundles via `scripts/bundle_release.sh`.
  - Sign archives with `scripts/sign_release.sh` using `TRUESHOT_SIGNING_KEY`.
  - Apply updates with signature verification via `scripts/update_release.sh` and `TRUESHOT_UPDATE_PUBKEY`.
- Service install:
  - Use `scripts/install_service.sh /opt/trueshot` to install systemd/launchd service pointing at the `current` symlink.

### Recommended Extras

- WAF / rate limiting at the edge.
- TLS termination with automated cert rotation.
- Offsite backups with tested restore drills.

### Current Validation Baseline

- On 2026-07-26, `cargo test --workspace` passed every executed unit,
  integration, and documentation test. Hardware/keychain-dependent tests remain
  explicitly ignored and require dedicated runners.
- Strict default-feature Clippy passes across every workspace target.
- The workspace has zero Cargo future-incompatibility warnings; optional Redis
  cache and relay behavior passes pinned-container disconnect/recovery tests.
- Production release is blocked by `upgrade_list.md` P0 #155: the current
  all-feature Linux dependency graph has unresolved vulnerability,
  maintenance, license-policy, and duplicate-version findings from
  `cargo-deny` 0.19.0.
- Dashboard release is also blocked by P0 #155: `npm audit --omit=dev` reports
  6 production advisories (5 high, 1 moderate), while the complete graph
  reports 22. Installation currently needs `--legacy-peer-deps` because
  `react-joyride` does not declare React 19 compatibility.
- Optional OpenCV feature validation remains release work: the workspace
  currently resolves three incompatible `opencv` crate generations and does
  not provision a complete native binding toolchain in CI. The calibration
  crate's pinned OpenCV feature compiles locally on macOS when
  `LIBCLANG_PATH` and `DYLD_FALLBACK_LIBRARY_PATH` point at Xcode's
  `XcodeDefault.xctoolchain/usr/lib`; this is source-compatibility evidence, not
  a substitute for generation unification or provisioned release runners.
  `scripts/check_macos_opencv_calibration.sh` reproduces this check, and the
  macOS CI lane provisions Homebrew OpenCV and runs it.
