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
- Provide an audit anchor in production:
  - `privacy.audit_anchor_url` must be set or startup will fail.
- Set a master key for encryption-at-rest if enabled:
  - `TRUESHOT_MASTER_KEY` (base64, 32 bytes) or `privacy.encryption_master_key_path`.

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
- Keep `scripts/run_apple_metal_ahd_qualification.sh` passing on dedicated Apple Silicon release runners. The retained M1 record gates adapter/backend, alternating p50/p95 speedup, exact measured CFA values, reconstructed-channel tolerance, bounded scratch, checksums, and a mandatory 6.25x HDR case with exact power-of-two normalization; real chart/corpus and every supported Apple generation remain required.
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
- Optional OpenCV feature validation remains release work: the workspace
  currently resolves three incompatible `opencv` crate generations and does
  not provision a complete native binding toolchain in CI.
