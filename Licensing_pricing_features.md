# TrueShot Licensing, Pricing, and Feature Packaging

Date: 2026-02-09

## 1) Licensing System Implementation (Current State)

TrueShot now uses a signed, device-bound license model with explicit feature gating:

- `trueshot-core/src/licensing/manager.rs`
  - Ed25519 license verification, device activation, offline grace checks.
  - Trial issuance path with targeted feature overrides (`create_trial_with_features`).
- `trueshot-core/src/licensing/license.rs`
  - Tier + feature entitlement model (resolution, usage, commercial flags, add-on flags).
  - Expanded feature flags for modular paid bundles.
- `trueshot-server/src/licensing.rs`
  - Central server `LicenseGate` runtime.
  - License status snapshot for UI/ops.
  - Feature entitlement enforcement helper for paid endpoints.
  - Bundle catalog and bundle-aware trial creation.
- `trueshot-server/src/api/license.rs`
  - `GET /api/license/status`
  - `GET /api/license/bundles`
  - `POST /api/license/import`
  - `POST /api/license/trial`
- `trueshot-server/src/api/health.rs`
  - Readiness now checks real license validity/status instead of hardcoded `true`.

## 2) Enforced Feature Gates (Server)

Implemented server-side paywall checks are currently active on:

- `Feature::AdvancedCaptureAutomation`
  - `POST /api/cameras/{id}/hdr`
  - `POST /api/cameras/{id}/focus_stack`
  - `POST /api/cameras/{id}/hdr_focus_stack`
  - `POST /api/cameras/{id}/interval/start`
- `Feature::CloudSyncBackup`
  - Storage OAuth URL/callback
  - Add/remove/sync storage connection
  - Backup job list/get/start/restore
- `Feature::TeamCollaboration`
  - Share analytics
  - Public share publish/read
  - Public gallery listing

When entitlement is missing, endpoints return `402 Payment Required` with a structured error payload.

## 3) Core License + Add-On Packaging

### Core License (required)

Core license covers the platform foundation and must be purchased by all paying users:

- Capture and scan wizard baseline
- 3DGS baseline workflows
- Core photogrammetry/SfM + reconstruction pipeline
- Standard exports (OBJ/PLY/GLB/USD)
- Local project management
- Security baseline (auth, audit, provenance, at-rest encryption support)

### Add-On Groups (modular upcharges)

- `advanced_capture`
  - HDR bracketing, focus stacking, HDR+focus stack, intervalometer automation
- `room_reconstruction`
  - Room-scale scan/reconstruction workflows and AEC-style capture patterns
- `avatar_studio`
  - Avatar reconstruction, rigging-ready human pipeline
- `cloud_sync_backup`
  - Cloud/NAS connectors, sync validation, backup/restore pipeline
- `team_collaboration`
  - Public gallery/share analytics/review-oriented collaboration surfaces
- `pipeline_automation`
  - Automation API/webhooks/pipeline integrations
- `dynamic_4dgs`
  - Advanced dynamic 4D Gaussian workflows
  - Available as an add-on for all core tiers

## 3.5) Feature Awareness + Gating UX (Implemented Pattern)

To maximize upgrade discovery while preserving entitlement controls:

- Navigation/tabs remain visible for paid workflows.
- If a user opens a feature without entitlement, the app shows a feature landing panel (value pitch + capability list + lifetime price + trial CTA + buy CTA).
- Functional controls and protected content remain locked until entitlement is present.
- Server-side API gates remain the source of truth (`402 Payment Required`) to prevent bypass.

Current dashboard implementations:

- Scan Wizard presets:
  - `room` preset gated by `room_reconstruction`.
  - `human` preset gated by `avatar_reconstruction`.
- Share Asset modal:
  - Collaboration sharing/publishing/analytics gated by `team_collaboration` with unlock panel fallback.
- Camera Control Pro:
  - Advanced capture workflows gated by `advanced_capture_automation`.
- Avatar Studio / Scene Reconstruction / XR Scanner:
  - Feature landing panels with trial/upgrade CTAs for `avatar_studio`, `dynamic_4dgs`, and WebXR entitlements.
- Device Manager:
  - Storage tab upsell with `cloud_sync_backup` entitlement and trial CTA.

## 4) Lifetime Pricing Recommendation

All prices below are lifetime licenses (one-time purchase). These are tentative ranges pending market validation.

### Market anchors (2026-02-09)

- [Polycam pricing](https://poly.cam/pricing): annual plans in the low hundreds, no one-time purchase option.
- [KIRI Engine pricing](https://www.kiriengine.app/pricing): annual plans with free tier plus paid creator/pro tiers.
- [Avaturn pricing](https://avaturn.me/pricing): tiered pricing from free to paid plans for avatar workflows.
- [RealityCapture 1.5](https://www.unrealengine.com/en-US/blog/realitycapture-1-5-is-now-available): now free to use under Epic’s model, pushing baseline expectations lower.
- [Cascable 6 pricing](https://blog.cascable.se/2023/11/cascable-6): one-time iOS purchase (`$99.99`) for pro camera control.

Implication:
- Competitors skew subscription. A lower one-time lifetime entry price is a differentiator, but add-ons must capture high-value specialist workflows.

### Core Tiers (Server catalog source of truth)

The server catalog defines the canonical pricing + device limits. The core license tiers map to:
- `Hobby` -> `Core Solo`
- `Education` -> `Core Team`
- `Pro` -> `Core Studio`

Pricing note: these are provisional targets pending market research and regional calibration. Final prices should follow the market-validation work in `upgrade_list.md` (item 119).

- `Core Solo` (1 device): **$99**
- `Core Team` (3 devices): **$249**
- `Core Studio` (10 devices): **$599**

### Add-On Lifetime Prices (per core license)

- `advanced_capture`: **$49**
- `room_reconstruction`: **$79**
- `avatar_studio`: **$99**
- `cloud_sync_backup`: **$79**
- `team_collaboration`: **$99**
- `pipeline_automation`: **$99**
- `dynamic_4dgs`: **$149**

### Bundle Options

- `Pro Capture Pack` (`advanced_capture` + `room_reconstruction`): **$109**
- `Studio Pipeline Pack` (`cloud_sync_backup` + `pipeline_automation` + `team_collaboration`): **$229**
- `Avatar + Dynamic Pack` (`avatar_studio` + `dynamic_4dgs`): **$199**
- `All Add-Ons Bundle`: **$549**

## 5) Trial Model (Implemented and Recommended Policy)

### Implemented

- Admin API can issue bundle-aware trial licenses:
  - `POST /api/license/trial`
  - configurable duration (`1-90` days)
  - selectable bundle list
- Default trial profile enables all paid bundles for evaluation.

### Recommended commercial policy

- 14-day default trial
- One trial per org + machine-bound fingerprint
- Trial watermark/badge in UI and exports
- Automatic downgrade to core capabilities after expiry

## 6) Full TrueShot Capability Inventory

### Capture and Hardware

- Multi-camera discovery/control
- PTZ, autofocus, focus drive, focus point
- Burst capture + IQA best-frame selection
- HDR bracketing
- Focus stacking
- HDR + focus stacking
- Intervalometer with ramping
- Turntable homing/rotation with feedback diagnostics
- SD card ingestion and import
- Live camera stream endpoint with hardened controls

### Reconstruction and AI

- Adaptive scan planning with quality intelligence
- SfM + dense MVS mesh reconstruction
- 3D Gaussian splatting training/rendering
- Dynamic 4DGS training path
- Scene reconstruction with sync primitives
- Avatar reconstruction pipeline (SMPL-X + mesh path)
- Color chart detection + DeltaE calibration

### Editing and Output

- Mesh edit pipeline
- Splat edit pipeline
- Progressive LOD generation for shared assets
- Export: OBJ, PLY, GLB, USD (+ provenance metadata)
- Digital twin export path

### Sharing and Collaboration

- Expiring share links
- Download/embedding controls
- Public share publishing
- Public gallery listing
- Share analytics
- Share social card/short-link routing

### Security, Compliance, and Ops

- Signed license verification
- Device-bound activation + offline grace
- Project encryption-at-rest and decrypt-aware reads
- Audit logging + anchoring
- Provenance signing with legal metadata
- Auth sessions with refresh + CSRF protection
- Rate limiting and transport hardening
- OpenAPI generation and telemetry integrations

### Integrations and Platform

- Cloud/NAS storage connectors
- Backup/restore jobs
- Distributed event bus bridge (Redis)
- Dashboard + API + CLI stack

## 7) Packaging Rules for Fair Pricing

- Core includes features every customer needs to ship value.
- Specialized vertical workflows are add-ons (no forced overpayment).
- Collaboration and cloud costs are monetized where operational burden is highest.
- Dynamic/advanced R&D-heavy capabilities are premium to preserve moat and margin.

## 8) Near-Term Follow-Through

To keep licensing airtight as additional features ship:

1. Add entitlement checks to each new paid endpoint at merge time.
2. Keep `/api/license/status` as the single source of truth for UI gating.
3. Add CI tests that verify paid endpoints return `402` when entitlements are absent.
