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
| Capture | Explicit, deadlock-safe scan workflows | Shipping | Project load is side-effect free; file-backed and no-camera integration tests enforce bounded startup/capture behavior |
| Capture | Thread-affine webcam and 360-camera lifecycle | Shipping | Nokhwa handles stay on bounded dedicated actors with deadline, panic, shutdown, and atomic-capture tests; no unsafe thread assertions |
| Capture | Group-amortized Nikon Z9 NEF ROI decode | Shipping | One scaled preview/bbox per HDR-focus group; bounded parallel decoders fill ordered slots in one reusable contiguous `u16` arena with no per-frame crop allocation or sidecar; advertised layouts are restricted by the NEF support matrix |
| Capture | Streaming capture manifests | Shipping | Incremental atomic JSONL writer records explicit HDR/focus/burst order, one reference frame, stable content IDs and one reusable crop plan per group |
| Capture | Uncertainty-driven HDR/focus planner | Planned | Deterministic shutter/ISO/focus scheduling will optimize expected information gain under motion, time, thermal, and lens-travel budgets |
| Processing | Native HDR + focus fusion | In Progress | Tiled `f32` fusion consumes the `u16` arena directly with lazy exposure/WB calibration, subpixel alignment, median/MAD deghosting, confidence, and depth-consistent dominant-plane refusion; CLI strength/throughput controls exist, while direct Helicon/Lightroom qualification remains |
| Processing | Deterministic focus/HDR quality gates | In Progress | Synthetic baseline is 44.127 dB PSNR and 100% depth accuracy with radiance/deghost/CFA invariants; redistributable real corpus and direct competitor gates remain |
| Processing | Calibrated probabilistic RAW fusion | Planned | Per-ISO/CFA censored Poisson-Gaussian likelihood, posterior uncertainty, source contribution, selective local alignment, and traceable fallback are specified in the 2026 research plan |
| Processing | Physical PSF-aware focus fusion | Planned | Diopter-space sub-plane depth, lens/PSF calibration, aperture-constrained halo suppression, and boundary trimaps will replace uniform frame-index depth |
| Processing | Camera-profiled color pipeline | Planned | Current archive is untagged camera-linear RGB; Z9 camera-to-standard transform, ICC/DNG tagging, and DeltaE 2000 gates are release blockers |
| Processing | Camera-profiled RAW normalization | Shipping | Real TIFF make/model identity selects verified sensor black/saturation levels; Z9 firmware 5.00 uses measured 1008/15311 levels and explicit overrides remain available |
| Processing | CFA-exact adaptive demosaic | In Progress | AHD preserves measured RGGB samples and true black, uses cache-sized parallel row bands, and has deterministic PSNR/throughput baselines; exact-parity Apple Metal acceleration and real chart/corpus gates remain |
| Processing | Hierarchical Bayer-preserving super-resolution | Shipping | Native mode retains Bayer output; requested SR uses alignment diversity and joint high-resolution demosaic |
| Processing | Native FAST/BRIEF + robust geometry | Shipping | FAST-9, BRIEF, adaptive RANSAC, MAGSAC, triangulation, and regression tests run without OpenCV |
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
| Platform | Optional Redis distributed event bus | Shipping | Bounded loop-safe relay, forced-disconnect recovery, shared calibration cache, and fail-fast local-only degradation |
| Platform | OpenAPI generation | Shipping | Generated at runtime from route annotations |
| Platform | Role-based access + SSO | Planned | User/role management still missing |
| Release | Signed installers + auto-update | Planned | Launcher signing not implemented |
| Release | SBOM/SLSA attestations | Planned | Supply chain hardening in progress |
| Release | Strict Rust quality gate | Shipping | Formatting, workspace tests, doctests, and strict default-feature Clippy pass for all targets |
| Release | Executed benchmark smoke gates | In Progress | Public-API benchmarks execute in CI; deterministic full E2E quality fixture remains open |
