# 📸 TrueShot: Intelligent 3D Scanning Studio

**Version 6.8.0** | Rust-based hybrid photogrammetry + 3D Gaussian Splatting platform

TrueShot is a full-stack 3D capture system that pairs a Rust reconstruction core with a production server, a real-time dashboard, and a power-user CLI.

## 🎯 Key Capabilities

- **Smart Scan Wizard**: Object analysis, quality scoring, and adaptive next-best-view planning
- **Hybrid Reconstruction**: Photogrammetry (SfM + MVS) and 3D Gaussian Splatting pipelines
- **RAW + Multi-Modal Fusion**: Burst/HDR/focus-stack processing with deterministic fusion paths
- **High-Fidelity Exports**: glTF/GLB/USD/PLY/OBJ with embedded provenance metadata
- **Security by Default**: Encryption-at-rest, signed provenance, and audit anchoring
- **Production Runtime**: Actix server, React dashboard, telemetry + metrics

## 🧭 Quick Start

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

## 🚀 Production Deployment

- Start from `config.production.toml` and copy to `config.toml`.
- Set `TRUESHOT_ENV=production` and configure TLS or `server.tls_proxy=true`.
- Follow the checklist in `docs/PRODUCTION_READINESS.md`.

### CLI Examples

```bash
# Process a directory into a hybrid reconstruction
trueshot process --input ./captures --output ./output --mode hybrid --quality high

# Export a model to GLB
trueshot export --input ./output/model.gltf --output ./output/model.glb --format glb

# Calibrate cameras (checkerboard)
trueshot calibrate --images ./calibration/*.jpg --cols 9 --rows 6 --square-size-mm 25
```

## 🏗️ Project Structure

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

## 🧪 Testing

```bash
cargo test --workspace
cargo test -p trueshot-core --no-run
```

## 📚 Documentation

- `docs/FEATURE_MATRIX.md` — What is shipping vs planned
- `docs/WHITEPAPER.md` — Architecture and pipeline overview
- `docs/DEVELOPMENT_LOG.md` — Post-consolidation change log
- `LAUNCHER_README.md` — Packaging/launcher notes

## 🤝 Contributing

- Use `cargo fmt` and `cargo clippy --workspace`
- Add tests for new functionality where possible
- Keep documentation in sync with code changes

## 📄 License

TrueShot is available under the permissive [MIT License](LICENSE).

---

**TrueShot**: Deterministic, high-fidelity capture-to-reconstruction tooling.
