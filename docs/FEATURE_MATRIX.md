# TrueShot Feature Matrix

Date: 2026-07-27

Legend: **Shipping**, **In Progress**, **Planned**

| Area | Capability | Status | Notes |
| --- | --- | --- | --- |
| Capture | Scan Wizard with quality scoring | Shipping | Live guidance and quality overlays in dashboard |
| Capture | Adaptive next-best-view planning | Shipping | Backend plan evolution per session |
| Capture | Live coverage heatmap + IQA alerts | Shipping | Coverage overlay + blur/parallax/exposure warnings |
| Capture | Auto-capture gating | Shipping | Auto-capture respects quality thresholds |
| Capture | WebRTC low-latency streaming | Planned | WebRTC server still a stub |
| Capture | HDR bracketing + focus stacking | In Progress | gPhoto now applies and exactly reads back declared ISO/shutter/aperture/WB/target settings, downloads the real camera file into local app storage through sync + atomic rename, and fails unsupported controls instead of reporting success; Nikon end-to-end, interruption, throughput, focus-readback, and planner-loop validation remain |
| Capture | Burst capture + best-frame selection | In Progress | Capture path wired; full selection policy tuning pending |
| Capture | Explicit, deadlock-safe scan workflows | Shipping | Project load is side-effect free; file-backed and no-camera integration tests enforce bounded startup/capture behavior |
| Capture | Thread-affine webcam and 360-camera lifecycle | Shipping | Nokhwa handles stay on bounded dedicated actors with deadline, panic, shutdown, and atomic-capture tests; no unsafe thread assertions |
| Capture | Group-amortized Nikon Z9 NEF ROI decode | Shipping | One scaled preview/bbox per HDR-focus group; bounded parallel decoders fill ordered slots in one reusable contiguous `u16` arena with no per-frame crop allocation or sidecar; advertised layouts are restricted by the NEF support matrix |
| Capture | Streaming capture manifests | Shipping | Incremental atomic JSONL writer records explicit HDR/focus/burst order, one reference frame, stable content IDs and one reusable crop plan per group |
| Capture | Uncertainty-driven HDR/focus planner | In Progress | Calibrated deterministic core parses bounded camera option grids, selectively decodes completed-NEF ROIs into stable spatial/CFA radiance and noise-whitened focus probes, uses the exact shutter/ISO/aperture sensor-exposure domain, assimilates repeated and fully censored evidence with posterior uncertainty, fits nonuniform diopter peaks, and cuts modeled time 82.7% versus a fixed 21-shot grid. Entitlement-gated local API sessions atomically verify/assimilate project-local NEFs with telemetry, path-escape, and provenance gates; immutable checksum-sealed generations persist before live commit and recover from interrupted/corrupt newest writes. Calibrated absolute lens drive, dashboard integration, and real timing gates remain |
| Processing | Native HDR + focus fusion | In Progress | Tiled `f32` fusion consumes the `u16` arena directly with lazy exposure/WB calibration, focus-plane scale/translation, per-bracket exposure-normalized global translation, selective compact local motion, forward/backward disocclusion rejection, median/MAD deghosting, scale-decoupled UHD focus evidence, physically scaled saturated-core glare exclusion, exact glare/source/state maps, confidence, and depth-consistent aperture-valid refusion. Glare suppression never changes measured radiance; real wavelength/lens calibration and direct Helicon/Lightroom qualification remain |
| Processing | Deterministic focus/HDR quality gates | In Progress | Scale-decoupled synthetic baseline is 44.161 dB PSNR and 100% depth accuracy with a 44 dB floor plus radiance/deghost/CFA/visibility/local-motion/disocclusion/source-fallback/glare invariants; glare focus energy is reduced at least 28% with bit-exact radiance and tile-invariant diagnostics. Redistributable real corpus and direct competitor gates remain |
| Processing | Calibrated probabilistic RAW fusion | In Progress | Bounded paired dark/flat fitting, whole-pair fit/holdout splits, per-CFA temporal/fixed-pattern/gain estimation, variance and residual-coverage gates, probabilistic censor margins, duplicate-proof SHA-256 evidence reports, exact-ISO profile publication/loading, censored likelihood, posterior uncertainty, and exact provenance maps are implemented; retained real Z9 calibration and real-sensor posterior qualification remain |
| Processing | Physical PSF-aware focus fusion | In Progress | Native fusion validates lens/sensor geometry, fits bounded-memory sub-plane peaks on nonuniform diopters, applies thin-lens PSF confidence, projects the continuous sensor surface onto the conservative aperture-valid set in linear raster passes, tags correction provenance, and exports unquantized meter depth; held-out real-lens calibration, breathing, halo-energy qualification, and mixed-pixel trimaps remain |
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
