# TrueShot Feature Matrix

Date: 2026-02-08

Legend: **Shipping**, **In Progress**, **Planned**

| Area | Capability | Status | Notes |
| --- | --- | --- | --- |
| Capture | Scan Wizard with quality scoring | Shipping | Live guidance and quality overlays in dashboard |
| Capture | Adaptive next-best-view planning | Shipping | Backend plan evolution per session |
| Capture | WebRTC low-latency streaming | Planned | WebRTC server still a stub |
| Reconstruction | Photogrammetry (SfM + MVS) | Shipping | PatchMatch + fusion pipeline |
| Reconstruction | 3D Gaussian Splatting | Shipping | Trainer + renderer with gradients |
| Reconstruction | Avatar pipeline (SMPL-X + mesh) | In Progress | SMPL-X fit + mesh reconstruction in core |
| Reconstruction | Scene reconstruction sync | Shipping | Audio sync + pose-aware confidence |
| Exports | glTF/GLB/USD/PLY/OBJ | Shipping | Provenance embedded in exports |
| Security | Encryption at rest | Shipping | Envelope-wrapped project keys |
| Security | Audit anchors + provenance signing | Shipping | Signed anchors and embedded metadata |
| Platform | API server + dashboard | Shipping | Actix + React stack |
| Platform | CLI workflows | Shipping | Headless processing + export |
| Platform | gRPC SDK surface | Planned | Proto crate is a stub |
| Platform | Distributed event bus | Planned | NATS/Redis integration not wired |
| Platform | OpenAPI generation | Planned | Static spec exists only |
| Platform | Role-based access + SSO | Planned | User/role management still missing |
| Release | Signed installers + auto-update | Planned | Launcher signing not implemented |
| Release | SBOM/SLSA attestations | Planned | Supply chain hardening in progress |

