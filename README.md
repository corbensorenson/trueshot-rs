# TrueShot: Intelligent 3D Scanning Studio

**Version 6.8.0** | Rust-based hybrid photogrammetry + 3D Gaussian Splatting platform

TrueShot is a full-stack 3D capture system that pairs a Rust reconstruction core with a production server, a real-time dashboard, and a power-user CLI.

TrueShot is **local-first**: capture, reconstruction, rendering, and export run on the user's hardware. Vendor infrastructure is limited to licensing, downloads, and update metadata.

## Current Status

TrueShot is under active pre-release hardening. The repository contains working
capture, RAW fusion, reconstruction, export, licensing, and local-server
surfaces, but the open gates in `ROADMAP.md` and `upgrade_list.md` still block a
production release. `docs/FEATURE_MATRIX.md` is the authoritative distinction
between Shipping, In Progress, and Planned capability.

The current macOS validation baseline (2026-07-27) is:

- `cargo test --workspace`: 523 passed, 0 failed, 6 ignored
- `cargo test -p trueshot-server`: 81 passed, 0 failed
- `cargo clippy --workspace --all-targets -- -D warnings`: passed
- Workspace Rust lints reject dead code, unused imports, and unused variables;
  narrow retained benchmark baselines use explained, self-auditing exceptions.

## Key Capabilities

- **Smart Scan Wizard**: Object analysis, quality scoring, and adaptive next-best-view planning
- **Hybrid Reconstruction**: Photogrammetry (SfM + MVS) and 3D Gaussian Splatting pipelines
- **RAW + Multi-Modal Fusion**: Burst/HDR/focus-stack processing with deterministic fusion paths
- **High-Fidelity Exports**: glTF/GLB/USD/PLY/OBJ with embedded provenance metadata
- **Security by Default**: Encryption-at-rest, signed provenance, and audit anchoring
- **Hardened Local API**: Scoped automation tokens, persistent revocation, trusted-proxy identity, opaque correlated 5xx responses, and descriptor-rooted project access
- **Local Runtime**: Actix server, React dashboard, telemetry, and metrics

## Quick Start

### Build the Workspace

```bash
cargo build --workspace
```

### Run the Server

```bash
./target/debug/trueshot serve --port 3000
```

### Run the Dashboard

```bash
cd trueshot-dashboard
npm install
npm run dev
```

## Production Deployment

- Start from `config.production.toml` and copy to `config.toml`.
- Set `TRUESHOT_ENV=production` and configure TLS or `server.tls_proxy=true`.
- Follow the checklist in `docs/PRODUCTION_READINESS.md`.
- Local-first policy: `docs/LOCAL_FIRST_POLICY.md`.

### CLI Examples

```bash
# Process a directory into a hybrid reconstruction
trueshot process --input ./captures --output ./output --mode hybrid --quality high

# Export a model to GLB
trueshot export --input ./output/model.gltf --output ./output/model.glb --format glb

# Calibrate cameras (checkerboard)
trueshot calibrate --images ./calibration/*.jpg --cols 9 --rows 6 --square-size-mm 25

# Fit exact-ISO noise plus optics-bound flat-field/defect correction
trueshot calibrate-noise \
  --dark ./sensor-calibration/dark \
  --flat-level ./sensor-calibration/flat-05 \
  --flat-level ./sensor-calibration/flat-15 \
  --flat-level ./sensor-calibration/flat-30 \
  --flat-level ./sensor-calibration/flat-50 \
  --flat-level ./sensor-calibration/flat-70 \
  --flat-level ./sensor-calibration/flat-90 \
  --output ./sensor-calibration/camera-noise.json
```

The command writes the noise profile, a sibling
`camera-noise_spatial_correction.json`, and a digest-bound calibration report.

## Project Structure

```
trueshot-rs/
├── trueshot-core/            # Core reconstruction + rendering pipeline
├── trueshot-server/          # Actix API server + runtime orchestration
├── trueshot-dashboard/       # React dashboard UI
├── trueshot-cli/             # CLI entrypoint for offline + headless workflows
├── trueshot-device-manager/  # Device discovery + telemetry
├── trueshot-sfm/             # SfM/MVS helpers
├── trueshot-storage/         # Storage + asset management
├── trueshot-vision/          # Vision utilities and calibration
├── trueshot-calibration/     # Calibration tools
├── trueshot-ai/              # AI helpers (inference wrappers)
├── trueshot-launcher/        # Packaging/launcher utilities
├── profiles/                 # Config profiles
└── Cargo.toml                # Workspace configuration
```

Capture datasets are intentionally excluded from the repository. Place local
test captures under `realTest/`; the directory is ignored by Git.

## Testing

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts/check_core_feature_builds.sh
```

Physical-camera, full-sensor Metal AHD, fusion-quality, packaging, and clean-host
qualification gates are documented in `docs/PRODUCTION_READINESS.md`; a green
unit-test suite is not treated as evidence for those hardware-dependent claims.

## Documentation

- `ROADMAP.md`: Authoritative execution order and release gates
- `docs/FEATURE_MATRIX.md`: What is shipping versus planned
- `docs/WHITEPAPER.md`: Architecture and pipeline overview
- `docs/DEVELOPMENT_LOG.md`: Post-consolidation change log
- `LAUNCHER_README.md`: Packaging and launcher notes

## Contributing

- Use `cargo fmt --all --check` and strict all-target workspace Clippy
- Add behavioral and security regression tests for every changed public operation
- Keep documentation in sync with code changes

## License

TrueShot is available under the permissive [MIT License](LICENSE).

---

**TrueShot**: Deterministic, high-fidelity capture-to-reconstruction tooling.
