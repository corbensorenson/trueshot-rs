# TrueShot Feature Matrix

Date: 2026-07-26

Legend: **Shipping**, **In Progress**, **Planned**

| Area | Capability | Status | Notes |
| --- | --- | --- | --- |
| Capture | Scan Wizard with quality scoring | Shipping | Live guidance and quality overlays in dashboard |
| Capture | Adaptive next-best-view planning | Shipping | Backend plan evolution per session |
| Capture | Live coverage heatmap + IQA alerts | Shipping | Coverage overlay + blur/parallax/exposure warnings |
| Capture | Auto-capture gating | Shipping | Auto-capture respects quality thresholds |
| Capture | WebRTC low-latency streaming | Planned | WebRTC server still a stub |
| Capture | HDR bracketing + focus stacking | In Progress | End-to-end hardware validation pending |
| Capture | Burst capture + best-frame selection | In Progress | Capture path wired; full selection policy tuning pending |
| Capture | Group-amortized Nikon NEF ROI decode | Shipping | One scaled preview/bbox per HDR-focus group; bounded parallel decoders fill ordered slots in one reusable contiguous `u16` arena with no per-frame crop allocation or sidecar |
| Capture | Streaming capture manifests | Shipping | Incremental atomic JSONL writer records explicit HDR/focus/burst order, one reference frame, stable content IDs and one reusable crop plan per group |
| Processing | Native HDR + focus fusion | Shipping | Tiled `f32` fusion consumes the `u16` arena directly with lazy exposure/WB calibration, subpixel alignment, confidence and depth |
| Processing | Million-file bounded execution | Shipping | Memory-credit admission, adaptive decode workers, one-deep async export, durable retries/cancellation and crash-safe artifact-verified resume |
| Reconstruction | Photogrammetry (SfM + MVS) | Shipping | PatchMatch + fusion pipeline |
| Reconstruction | In-memory fused-image handoff | Shipping | SfM feature extraction and dense MVS share `Arc<RgbImage>` buffers; focus-stack persistence is optional |
| Reconstruction | 3D Gaussian Splatting | Shipping | Trainer + renderer with gradients |
| Reconstruction | Avatar pipeline (SMPL-X + mesh) | In Progress | SMPL-X fit + mesh reconstruction in core |
| Reconstruction | Scene reconstruction sync | Shipping | Audio sync + pose-aware confidence |
| Reconstruction | Dynamic 4D Gaussian Splatting | In Progress | Training path present; productization pending |
| Reconstruction | On-device preview + hybrid | Planned | Device preview + quality preset pipeline not productized |
| Editing | Mesh cleanup toolkit | Planned | Hole fill/decimate/smooth/normals repair pending |
| Editing | Splat editing toolkit | Planned | Brush/plane/sphere prune + density controls pending |
| Output | Floorplan + measurement export | Shipping | Area/perimeter/GeoJSON/CSV outputs |
| Exports | glTF/GLB/USD/PLY/OBJ | Shipping | Provenance embedded in exports |
| Exports | FBX/USDC/USDZ | Planned | Engine/DCC export breadth still pending |
| Sharing | Share links + viewer | Shipping | Expiring links + share viewer route |
| Sharing | Share analytics + public gallery | In Progress | Analytics present; gallery/short-links pending |
| Storage | Cloud/NAS backup + restore | Shipping | Provider-backed backups with integrity checks |
| Security | Encryption at rest | Shipping | Envelope-wrapped project keys |
| Security | Audit anchors + provenance signing | Shipping | Signed anchors and embedded metadata |
| Platform | Local-first compute boundary | Shipping | All capture/recon/render/export runs on customer hardware |
| Licensing | Modular bundles + trials | Shipping | Entitlements + trials enforced server-side |
| Licensing | License activation (key + JSON) | Shipping | Device-bound activation + seat management |
| Platform | API server + dashboard | Shipping | Actix + React stack |
| Platform | CLI workflows | Shipping | Headless processing + export |
| Platform | gRPC SDK surface | Planned | Proto crate is a stub |
| Platform | Distributed event bus | Planned | NATS/Redis integration not wired |
| Platform | OpenAPI generation | Shipping | Generated at runtime from route annotations |
| Platform | Role-based access + SSO | Planned | User/role management still missing |
| Release | Signed installers + auto-update | Planned | Launcher signing not implemented |
| Release | SBOM/SLSA attestations | Planned | Supply chain hardening in progress |
