# TrueShot Automation API (Pipeline Jobs)

Date: 2026-02-09

This document describes the first‑class automation endpoints and CLI wrappers for TrueShot pipeline integration. All endpoints require an API token (`X-API-Token`) unless noted.

## Authentication

Use an API token created via the dashboard:

- Header: `X-API-Token: <token>`
- New tokens default to the least-privilege `read` scope.
- Canonical scopes are `read`, `capture`, `process`, `export`, `license`, and
  `admin`. The wildcard `*` grants every capability and must be used alone.
- Job submission requires `process`; listing and reading jobs require `read`.
- API tokens retain their owner's current role and are rejected immediately
  when expired, revoked, orphaned, or owned by an inactive account.

Tokens are required for all `/api/jobs` endpoints.

## Endpoints

### Submit a job

`POST /api/jobs`

Request body:

```json
{
  "id": "06b3a1a4-177e-4c2f-9e4b-7f2c7a266b62",
  "kind": "unified_photogrammetry",
  "name": "Studio Scan A",
  "payload": {
    "workspace_path": "/data/projects/studio_scan_a",
    "livescan_path": "/data/projects/studio_scan_a/livescan",
    "dslr_path": "/data/projects/studio_scan_a/dslr",
    "job_type": "photogrammetry",
    "webhook_url": "https://hooks.example.com/trueshot"
  }
}
```

Notes:
- `id` is the request id (idempotent). Reusing the same id returns the existing job.
- `payload.webhook_url` (or `payload.webhooks: ["..."]`) enables webhook callbacks.
- `job_type` can be `photogrammetry` or `gaussian_splatting` for unified jobs.

### List jobs

`GET /api/jobs`

Returns the job list with current status/progress.

### Get a job

`GET /api/jobs/{id}`

Returns a single job record.

## Webhook Callbacks

When a job status changes, TrueShot posts a JSON payload to `payload.webhook_url` or `payload.webhooks`:

```json
{
  "event": "job.status",
  "job": {
    "id": "06b3a1a4-177e-4c2f-9e4b-7f2c7a266b62",
    "request_id": "06b3a1a4-177e-4c2f-9e4b-7f2c7a266b62",
    "kind": "unified_photogrammetry",
    "name": "Studio Scan A",
    "status": "running",
    "progress": 0.42,
    "attempts": 1,
    "max_attempts": 3,
    "created_at": "2026-02-09T23:11:04Z",
    "started_at": "2026-02-09T23:11:06Z",
    "finished_at": null,
    "last_error": null
  },
  "payload": {
    "workspace_path": "/data/projects/studio_scan_a",
    "livescan_path": "/data/projects/studio_scan_a/livescan",
    "dslr_path": "/data/projects/studio_scan_a/dslr",
    "job_type": "photogrammetry",
    "webhook_url": "https://hooks.example.com/trueshot"
  },
  "sent_at": "2026-02-09T23:11:07Z"
}
```

## CLI Examples

Set `TRUESHOT_API_TOKEN` or pass `--api-token` explicitly.

Submit a job:

```bash
TRUESHOT_API_TOKEN=... trueshot jobs submit \
  --kind unified_photogrammetry \
  --name "Studio Scan A" \
  --workspace /data/projects/studio_scan_a \
  --livescan /data/projects/studio_scan_a/livescan \
  --dslr /data/projects/studio_scan_a/dslr \
  --job-type photogrammetry \
  --webhook-url https://hooks.example.com/trueshot
```

Submit with a raw payload JSON:

```bash
TRUESHOT_API_TOKEN=... trueshot jobs submit \
  --kind unified_gaussian_splatting \
  --name "GS batch" \
  --payload '{"workspace_path":"/data/gs","job_type":"gaussian_splatting","webhook_url":"https://hooks.example.com/trueshot"}'
```

List jobs:

```bash
TRUESHOT_API_TOKEN=... trueshot jobs list
```

Get job:

```bash
TRUESHOT_API_TOKEN=... trueshot jobs get --id 06b3a1a4-177e-4c2f-9e4b-7f2c7a266b62
```
