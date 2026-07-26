# TrueShot Development Log (Post-Consolidation)

This log tracks changes after the current repo consolidation. Historical notes that referenced retired crates or prototype modules have been removed to avoid confusion. See `docs/FEATURE_MATRIX.md` for shipping vs planned capabilities.

## 2026-02-08

- Added adaptive next-best-view scan planning with backend plan history.
- Implemented FFT-based audio sync and pose-aware confidence fields for scene reconstruction.
- Added encryption-at-rest for pipeline outputs with decryption-aware exports.
- Embedded provenance metadata in glTF/USD exports and signed audit anchors.
- Added device telemetry and storage health reporting.

## 2026-02-07

- Implemented quality scoring and scan wizard UI guidance.
- Added mesh/GT KPIs (PSNR/SSIM/Chamfer) with CI gating scripts.
- Hardened licensing and provenance key handling.

