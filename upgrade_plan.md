# TrueShot Upgrade Plan (Red‑Team Driven)

## Goals
1. Ship a secure, production‑grade capture‑to‑3D system with zero unauthenticated control paths, zero path traversal, and a cryptographically sound licensing and token story.
2. Deliver state‑of‑the‑art reconstruction quality and speed across photogrammetry, 3D Gaussian Splatting, and real‑time hybrid streaming with GPU‑first execution.
3. Make all workflows deterministic, observable, and testable at scale with reproducible builds and measurable quality targets.

## Threat Model And Trust Boundaries
1. Remote clients can access the API, WebSocket, and MJPEG streams over LAN/WAN. Assume hostile network and untrusted clients.
2. User‑supplied paths, filenames, and multipart uploads are attacker‑controlled input.
3. External processes (COLMAP, 3DGS training, device control tools) are untrusted and can hang or fail.
4. Local token stores, license caches, and hardware registries can be read or modified by local users or malware.

## Red‑Team Findings (Traceable To Code)
| ID | Severity | Finding | Evidence |
| --- | --- | --- | --- |
| F01 | Critical | Hardcoded JWT secret with axum‑based middleware that is not wired into Actix; tokens can be forged if used. | `trueshot-server/src/auth/jwt.rs` |
| F02 | Critical | API auth is optional; if `api_key` is unset, all endpoints are open by default. | `trueshot-server/src/main.rs` |
| F03 | Critical | WebSocket path `/api/ws` is explicitly exempted from auth checks; device events leak to unauthenticated clients. | `trueshot-server/src/main.rs`, `trueshot-server/src/api/websocket.rs` |
| F04 | Critical | Path traversal in project creation, purge, open, and import; user input is joined without canonicalization. | `trueshot-server/src/api/project.rs` |
| F05 | Critical | Multipart upload has no size/type limits or quota enforcement; can fill disk or crash the server. | `trueshot-server/src/api/project.rs` |
| F06 | High | `open_project_fs` executes OS commands on attacker‑controlled paths; remote callers can trigger arbitrary filesystem opens. | `trueshot-server/src/api/project.rs` |
| F07 | High | CORS is permissive in non‑production and overly broad in production, allowing any `https://*` origin. | `trueshot-server/src/main.rs` |
| F08 | High | OAuth callback accepts `state` but never validates it; `code` is stored as access token; CSRF and account binding attacks are possible. | `trueshot-server/src/api/storage.rs` |
| F09 | High | TokenStore claims encryption but stores raw JSON tokens in redb; credentials are plaintext at rest. | `trueshot-core/src/security/token_store.rs` |
| F10 | High | Licensing uses placeholder keys, dev‑mode fallbacks, and stub integrity checks; trivially bypassable. | `trueshot-core/src/licensing/manager.rs`, `trueshot-core/src/licensing/encryption.rs`, `trueshot-core/src/licensing/integrity.rs` |
| F11 | High | mDNS cluster discovery has no authentication or encryption; rogue nodes can register. | `trueshot-core/src/compute/cluster.rs` |
| F12 | High | Hardware control endpoints allow PTZ, capture, and turntable movement without rate limiting or auth hardening. | `trueshot-server/src/api/hardware.rs` |
| F13 | Medium | Camera registry is written without atomic fs semantics; concurrent writes can corrupt the registry. | `trueshot-device-manager/src/camera/registry.rs` |
| F14 | High | Numerous `unwrap`/`expect` paths in production code can panic on malformed inputs or edge cases. | `trueshot-core/src/object_detection.rs`, `trueshot-core/src/hierarchical_pipeline.rs`, `trueshot-core/src/compute/gpu.rs`, `trueshot-core/src/settings.rs` |
| F15 | High | Object detection hard‑codes Nikon Z9 resolution, breaking ROI scaling for other cameras. | `trueshot-core/src/object_detection.rs` |
| F16 | High | Scan wizard endpoints are mocked; object analysis, plan computation, and capture flow are simulated. | `trueshot-server/src/api/scan.rs` |
| F17 | High | CLI processing/export/calibration are simulated and do not drive the real pipeline. | `trueshot-cli/src/main.rs` |
| F18 | High | LiveHybrid stream compression functions are stubbed; no real zstd/lz4 compression. | `trueshot-core/src/live_hybrid/streaming.rs` |
| F19 | High | GPU collapse and GPU Mertens are TODO; GPU pipeline claims are not realized. | `trueshot-core/src/gpu/gpu_collapse.rs`, `trueshot-core/src/gpu/gpu_mertens.rs` |
| F20 | Medium | SFM reprojection errors and pose confidence are hard‑coded placeholders. | `trueshot-core/src/sfm/mod.rs`, `trueshot-vision/src/pose.rs` |
| F21 | Medium | Storage API state is in‑memory only; all connections are lost on restart. | `trueshot-server/src/api/storage.rs` |
| F22 | Medium | Config sources are inconsistent between root `config.toml`, server config, and core settings; behavior changes by working directory. | `config.toml`, `trueshot-server/config.toml`, `trueshot-core/src/settings.rs` |
| F23 | Medium | Legacy COLMAP-based rig solving and photogrammetry paths have been removed; native SfM/MVS is now the only pipeline. | `trueshot-core/src/scanning/rig.rs` |
| F24 | Medium | Bayer cache clones large arrays and clears entire cache on overflow; avoids LRU and causes spikes. | `trueshot-core/src/bayer_cache.rs` |
| F25 | Medium | LiveHybrid segmentation uses O(n²) region growing, which will not scale to large Gaussian sets. | `trueshot-core/src/live_hybrid/segmentation.rs` |
| F26 | Medium | GPU compute path blocks with `Maintain::Wait` and unwraps map_async results; no error recovery. | `trueshot-core/src/compute/gpu.rs` |
| F27 | Low | `deny.toml` references an outdated version of `trueshot-core` and no longer reflects the workspace. | `deny.toml` |
| F28 | Low | Documentation claims crates and features that are not present in the workspace. | `README.md`, `docs/WHITEPAPER.md` |

## Progress Updates (2026-02-07)
1. Guest‑phone scope enforcement added per‑endpoint (`system:read`, `stream:read`), including WebSocket and MJPEG stream gating.
2. GPU collapse and GPU Mertens implemented with real WGSL kernels and wgpu pipelines.
3. Material estimation now supports an optional ONNX model path with heuristic fallback.

## Upgrade Plan

### Phase 0 — Security, Trust, And Guardrails (Immediate)
1. Replace API key with a full auth stack: JWT with rotating keys, short‑lived access tokens, refresh tokens, and RBAC scopes for hardware control, project management, storage, and admin. Address F01, F02, F03, F12.
2. Enforce mandatory auth on WebSocket and MJPEG streams with tokenized URLs or subprotocol auth. Address F03, F12.
3. Completed (2026-02-07): Enforce per‑endpoint scopes for guest tokens (`system:read`, `stream:read`) and reject scope violations.
4. Implement strict path canonicalization and root‑anchored file policies for all project and import endpoints, rejecting `..`, absolute paths, and non‑UTF8 filenames. Address F04, F06.
5. Add multipart upload limits, MIME checks, antivirus hooks, and per‑project quotas with soft and hard limits. Address F05.
6. Harden CORS with explicit allowlists from config, and add CSRF protection for any cookie‑based flows. Address F07.
7. Make OAuth real: state validation, PKCE, secure token exchange, encrypted storage, and per‑provider scopes. Address F08, F09.
8. Encrypt tokens at rest with OS‑backed keystores or envelope encryption (e.g., AES‑GCM + KMS). Address F09.
9. Lock down mDNS discovery with mTLS or shared‑secret registration and signed node manifests. Address F11.
10. Implement request rate limiting, job quotas, and hardware safety interlocks (movement speed caps, soft‑stop limits). Address F12.

### Phase 1 — Reliability, Correctness, And Consistency
1. Eliminate all `unwrap`/`expect` on production paths, introduce typed error propagation, and convert panics into recoverable errors with user‑visible diagnostics. Address F14.
2. Generalize object detection to camera‑agnostic metadata: read actual sensor dimensions and EXIF, and scale ROI accordingly. Address F15.
3. Replace mocked ScanWizard endpoints with the real pipeline: background capture, segmentation, scan plan, and step execution. Address F16.
4. Wire CLI commands to the real engine with progress callbacks and proper error reporting instead of simulated sleeps. Address F17.
5. Unify config across server and core with a single schema, environment overrides, and validated defaults. Address F22.
6. Persist storage connections with encrypted on‑disk storage and migration management. Address F21.
7. Introduce atomic file writes and file locks for registry and config updates. Address F13.

### Phase 2 — Performance, Scale, And GPU‑First Execution
1. Completed (2026-02-07): Implemented GPU collapse and GPU Mertens using wgpu kernels and tiled memory layouts. Address F19.
2. Completed: removed COLMAP integration from production builds; rig solve and SfM/MVS are fully native. Address F23.
3. Replace the global Bayer cache with an adaptive, segmented LRU that stores `Arc` buffers and supports eviction policies per job. Address F24.
4. Convert LiveHybrid segmentation to spatial indexing (KD‑tree or voxel grid) and GPU‑assisted clustering. Address F25.
5. Make GPU compute non‑blocking with async map pipelines, error retries, and device loss handling. Address F26.

### Phase 3 — State‑Of‑The‑Art Reconstruction And AI
1. Integrate modern feature extractors and matchers (SuperPoint/LightGlue) with learned outlier rejection and BA warm starts.
2. Add neural reconstruction modes: NeRF‑style radiance fields, 3DGS with dynamic splatting, and hybrid photogrammetry‑NeRF fusion.
3. Partial (2026-02-07): Material/BRDF estimation now supports optional ONNX inference with heuristic fallback; semantic segmentation remains to be integrated.
4. Implement real‑time scan quality feedback: coverage heatmaps, visibility entropy, and adaptive capture planning.
5. Add multi‑sensor fusion: depth cameras (RealSense/ToF), structured light, and IMU‑assisted pose priors.

### Phase 4 — Observability, QA, And Reproducibility
1. Add structured tracing with job IDs, per‑stage timing metrics, and OpenTelemetry exporters for server and pipeline. Address F26.
2. Introduce fuzzing for RAW/EXIF parsing, property‑based tests for alignment/fusion, and golden datasets with acceptance thresholds.
3. Implement deterministic build and run pipelines, including pinned model checksums and reproducible GPU kernels.
4. Build a performance regression suite with automated profiling baselines and alerting.

### Phase 5 — Documentation And Productization
1. Align README and whitepaper with the actual workspace, and keep a capability matrix that maps features to code paths. Address F28.
2. Update licensing documentation to match the unified cryptographic implementation. Address F10.
3. Publish API specs with versioning and automatic client generation for dashboard and CLI.

## Success Metrics
1. Zero unauthenticated access to hardware control, project data, or streams.
2. End‑to‑end scan pipeline runs without panics across malformed inputs and large datasets.
3. GPU and CPU parity validated with deterministic output diffs and performance gains over baseline.
4. Reconstruction quality improves measurably on benchmark datasets with stable, reproducible results.
5. Documentation matches shipped behavior, with a live feature matrix and API spec coverage above 95%.
