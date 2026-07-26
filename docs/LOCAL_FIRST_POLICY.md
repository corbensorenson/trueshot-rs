# TrueShot Local-First Compute Policy

Date: 2026-02-09

## Purpose

TrueShot is a local-first visual/3D platform. All customer capture, reconstruction, rendering, and export compute runs on the customer's hardware. The vendor infrastructure is restricted to licensing and commerce control-plane operations only.

## Hard Boundary

Allowed vendor-hosted services:
- Sales website and downloads.
- Licensing activation, entitlement check-ins, and seat management.
- Update metadata and signed release distribution.

Disallowed vendor-hosted services:
- Any capture, reconstruction, rendering, training, or export workloads.
- Storage or processing of customer scan data.
- Remote job execution or hosted pipelines.

## Engineering Rules

- The API surface is categorized by role:
  - `local_workload` endpoints run on customer hardware.
  - `control_plane` endpoints handle authentication, licensing, and system metadata for local deployments.
- Endpoint tags are classified in `docs/endpoint_classification.json` and enforced in CI.
- CI should fail if new endpoints are added that violate this boundary.
- Default configs must not enable remote workloads.

## Operational Guarantees

- Customer data remains on customer storage.
- Vendor infrastructure never executes customer compute tasks.
- Offline operation is supported within the license grace window.

## Enforcement Targets

- Documentation and release notes must state the local-first boundary.
- Any exceptions require explicit product leadership approval and a public policy update.
