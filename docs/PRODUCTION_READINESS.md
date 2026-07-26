## TrueShot Production Readiness

This checklist is the minimum bar for a production deployment. It assumes `TRUESHOT_ENV=production`.

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

### High-Volume NEF Processing

- Prefer capture-time JSONL manifests over legacy directory discovery.
- Size native admission with `TRUESHOT_MEMORY_BUDGET_MIB`; the default is 75% of currently available memory.
- Keep `TRUESHOT_NEF_ACCESS_MODE` unset for sidecar-free one-pass ingestion.
- Use `TRUESHOT_RESUME_VERIFY=sampled` for normal operation and `full` for validation or regulated workflows.
- Run `capture_manifest_scale_benchmark`, `nef_group_benchmark --verify-full`, and `nef_corruption_runner` on release hardware before advertising a camera/storage combination.
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

### Known Release Gates

- The focused NEF ingestion/fusion/export/reconstruction tests pass, but the
  full `trueshot-core` suite currently has nine failures outside this pipeline
  (hierarchical grading/pipeline, spatial audio attenuation, deformation MLP,
  motion classification and floorplan geometry). Treat the workspace as
  release-blocked until those failures are triaged and fixed or explicitly
  quarantined with owners and rationale.
