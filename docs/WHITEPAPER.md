# TrueShot: Hybrid Photogrammetry Studio

**Version**: 6.8.0
**Date**: February 2026
**Status**: Active development

## Abstract

TrueShot is a Rust-based 3D scanning platform that combines deterministic RAW processing with photogrammetry (SfM + MVS) and 3D Gaussian Splatting. It ships as a production API server with a real-time dashboard and a CLI for headless workflows. The system is designed to produce provenance-traceable outputs with strong privacy controls.

## 1. System Architecture

```mermaid
graph TD
  "Dashboard" --> "API Server"
  "CLI" --> "API Server"
  "API Server" --> "Core Pipeline"
  "Core Pipeline" --> "Storage"
  "Core Pipeline" --> "Exports"
  "Device Manager" --> "API Server"
  "Telemetry" --> "API Server"
  "Core Pipeline" --> "Benchmarks"
```

### Core Components

- `trueshot-core`: Reconstruction, rendering, calibration, and export logic.
- `trueshot-server`: Actix API server, orchestration, audit, and encryption-at-rest.
- `trueshot-dashboard`: React UI for capture, quality monitoring, and export.
- `trueshot-cli`: CLI for offline processing and scripted workflows.
- `trueshot-device-manager`: Device discovery and telemetry inputs.
- `trueshot-storage`: Asset management and storage primitives.

## 2. Capture-to-Reconstruction Pipeline

1. **Ingestion**: RAW and video streams are ingested with project-scoped encryption-at-rest.
2. **Analysis**: Quality scoring and scan planning produce adaptive capture guidance.
3. **Reconstruction**:
   - Photogrammetry (SfM + MVS) for metric geometry
   - 3D Gaussian Splatting for view-dependent appearance
4. **Fusion**: Hybrid outputs combine geometry with high-fidelity textures.
5. **Export**: glTF/GLB/USD/PLY/OBJ with embedded provenance metadata.

## 3. Product Experience

- **Scan Wizard**: Live quality feedback, uncertainty overlays, and guidance.
- **Dashboards**: Device telemetry, storage health, and project views.
- **CLI**: Headless mode for automation and batch workloads.

## 4. Security and Provenance

- Per-project encryption-at-rest with envelope-wrapped keys.
- Audit trails with signed anchors.
- Provenance records embedded in exports.

## 5. Benchmarks and KPIs

Benchmark tooling lives in `benchmarks/` and `trueshot-core/examples/`. See `docs/FEATURE_MATRIX.md` for what is shipping vs in progress.

## 6. Implementation Status

The authoritative status is maintained in `docs/FEATURE_MATRIX.md` and `upgrade_list.md`.

