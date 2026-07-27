import axios, { AxiosError, type AxiosRequestConfig, type AxiosRequestHeaders } from 'axios';

// API Configuration - Uses environment variables with fallback defaults
export const API_Base = import.meta.env.VITE_API_BASE || 'http://127.0.0.1:3000/api';
export const WS_Base = import.meta.env.VITE_WS_BASE || 'ws://127.0.0.1:3000/api/ws';

// Derived URLs for specific endpoints
export const STREAM_BASE = API_Base.replace('/api', ''); // Base URL without /api for streams

let inMemoryToken: string | null = null;

export const getAuthToken = () => inMemoryToken;
export const setAuthToken = (token: string) => {
    inMemoryToken = token;
};
export const clearAuthToken = () => {
    inMemoryToken = null;
};

const traceFromResponse = (res: Response): string | null => {
    const traceId = res.headers.get('x-trace-id');
    if (traceId) {
        console.debug(`[trace] ${res.url} ${traceId}`);
    }
    return traceId;
};

const csrfTokenFromCookie = () => {
    if (typeof document === 'undefined') return null;
    const match = document.cookie.split('; ').find((row) => row.startsWith('trueshot_csrf='));
    if (!match) return null;
    return decodeURIComponent(match.split('=').slice(1).join('='));
};

const fetchWithTrace = async (input: RequestInfo | URL, init?: RequestInit) => {
    const method = (init?.method || 'GET').toUpperCase();
    const needsCsrf = !['GET', 'HEAD', 'OPTIONS'].includes(method);
    const csrf = needsCsrf ? csrfTokenFromCookie() : null;
    const headers = new Headers(init?.headers || {});
    if (csrf && !headers.has('x-csrf-token')) {
        headers.set('x-csrf-token', csrf);
    }
    const res = await fetch(input, {
        credentials: 'include',
        ...init,
        headers,
    });
    traceFromResponse(res);
    return res;
};

const fetchAuthed = async (input: RequestInfo | URL, init?: RequestInit) => {
    const headers = new Headers(init?.headers || {});
    for (const [key, value] of Object.entries(authHeaders())) {
        if (!headers.has(key)) {
            headers.set(key, value);
        }
    }
    return fetchWithTrace(input, {
        ...init,
        headers,
    });
};

export const authHeaders = (): Record<string, string> => {
    const token = getAuthToken();
    return token ? { Authorization: `Bearer ${token}` } : {};
};

export const establishSession = async (token?: string) => {
    const bearer = token || getAuthToken();
    if (bearer) {
        const res = await fetchWithTrace(`${API_Base}/auth/session`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${bearer}` },
        });
        if (!res.ok) return null;
        return bearer;
    }
    const res = await fetchWithTrace(`${API_Base}/auth/refresh`, {
        method: 'POST',
    });
    if (!res.ok) return null;
    return 'cookie';
};

export const loginWithApiKey = async (apiKey: string) => {
    const res = await fetchWithTrace(`${API_Base}/auth/session`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-API-Key': apiKey },
        credentials: 'include',
    });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    const data = await res.json();
    if (data?.token) {
        return data.token;
    }
    return null;
};

export interface BootstrapStatusResponse {
    required: boolean;
}

export interface TokenResponse {
    token: string;
    role: string;
    expires_in_seconds: number;
    refresh_expires_in_seconds?: number | null;
}

export interface ApiTokenSummary {
    token_id: string;
    name: string;
    scopes: string[];
    created_at: number;
    expires_at?: number | null;
    last_used?: number | null;
    revoked: boolean;
}

export interface ApiTokenResponse {
    token: string;
    token_id: string;
    name: string;
    expires_at?: number | null;
    scopes: string[];
}

export const getBootstrapStatus = async (): Promise<BootstrapStatusResponse> => {
    const res = await fetchWithTrace(`${API_Base}/auth/bootstrap/status`);
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return res.json();
};

export const bootstrapAdmin = async (payload: { email: string; name: string; password: string }) => {
    const res = await fetchWithTrace(`${API_Base}/auth/bootstrap`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
    });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    const data = (await res.json()) as TokenResponse;
    if (data?.token) {
        setAuthToken(data.token);
    }
    return data;
};

export const loginWithPassword = async (payload: { email: string; password: string }) => {
    const res = await fetchWithTrace(`${API_Base}/auth/login`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
    });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    const data = (await res.json()) as TokenResponse;
    if (data?.token) {
        setAuthToken(data.token);
    }
    return data;
};

export const logoutAll = async () => {
    const res = await fetchAuthed(`${API_Base}/auth/logout_all`, {
        method: 'POST',
        headers: authHeaders(),
    });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    clearAuthToken();
};

export const listApiTokens = async (): Promise<ApiTokenSummary[]> => {
    const res = await fetchAuthed(`${API_Base}/auth/tokens`, { headers: authHeaders() });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return res.json();
};

export const createApiToken = async (payload: { name: string; scopes?: string[]; expires_in_seconds?: number }) => {
    const res = await fetchAuthed(`${API_Base}/auth/tokens`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return (await res.json()) as ApiTokenResponse;
};

export interface LicenseStatusResponse {
    status: string;
    license_valid: boolean;
    tier?: string | null;
    expires_at?: string | null;
    features: Record<string, boolean>;
    bundles: Record<string, boolean>;
    trial_available?: boolean;
    trial_reason?: string | null;
    trial_active?: boolean;
    trial_expires_at?: string | null;
    trial_days_remaining?: number | null;
}

export interface LicenseBundleInfo {
    key: string;
    name: string;
    description: string;
    features: string[];
    price_usd?: number;
    billing?: string;
}

export interface LicenseTierInfo {
    key: string;
    name: string;
    max_devices: number;
    price_usd?: number;
    billing?: string;
}

export interface LicenseDeviceInfo {
    fingerprint_hash: string;
    device_name: string;
    activated_at: string;
    last_seen: string;
}

export interface CoverageStatus {
    orientation_index: number;
    azimuth_bins: number;
    elevation_bins: number;
    counts: number[];
    coverage_score: number;
    coverage_density: number;
}

export const getLicenseStatus = async (): Promise<LicenseStatusResponse> => {
    const res = await fetchAuthed(`${API_Base}/license/entitlements`, { headers: authHeaders() });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return res.json();
};

export const getLicenseBundles = async (): Promise<LicenseBundleInfo[]> => {
    const res = await fetchAuthed(`${API_Base}/license/catalog`, { headers: authHeaders() });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return res.json();
};

export const getLicenseTiers = async (): Promise<LicenseTierInfo[]> => {
    const res = await fetchAuthed(`${API_Base}/license/tiers`, { headers: authHeaders() });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return res.json();
};

export const createLicenseTrial = async (payload: { duration_days?: number; bundles?: string[] }) => {
    const res = await fetchAuthed(`${API_Base}/license/trial/self`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return res.json();
};

export const importLicense = async (payload: { license_json: string }) => {
    const res = await fetchAuthed(`${API_Base}/license/import`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return res.json();
};

export const startXrSession = async (payload: { mode: string; device?: string; notes?: string }) => {
    const res = await fetchAuthed(`${API_Base}/xr/session/start`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return res.json() as Promise<{ session_id: string }>;
};

export const completeXrSession = async (payload: { session_id: string; mode: string; frame_count: number; duration_seconds?: number }) => {
    const res = await fetchAuthed(`${API_Base}/xr/session/complete`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return res.json();
};

export const getLicenseDevices = async (): Promise<LicenseDeviceInfo[]> => {
    const res = await fetchAuthed(`${API_Base}/license/devices`, { headers: authHeaders() });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return res.json();
};

export const activateLicenseDevice = async (payload: { device_name?: string }) => {
    const res = await fetchAuthed(`${API_Base}/license/activate`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return res.json();
};

export const activateLicenseKey = async (payload: { license_key: string; device_name?: string }) => {
    const res = await fetchAuthed(`${API_Base}/license/activate-key`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return res.json();
};

export const deactivateLicenseDevice = async (payload: { fingerprint_hash: string }) => {
    const res = await fetchAuthed(`${API_Base}/license/deactivate`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return res.json();
};

export const revokeApiToken = async (tokenId: string) => {
    const res = await fetchAuthed(`${API_Base}/auth/tokens/${encodeURIComponent(tokenId)}`, {
        method: 'DELETE',
        headers: authHeaders(),
    });
    if (!res.ok) {
        throw new Error(await res.text());
    }
    return res.json();
};

export const clearSession = async () => {
    await fetchWithTrace(`${API_Base}/auth/session`, {
        method: 'DELETE',
        credentials: 'include',
        headers: authHeaders(),
    }).catch(() => undefined);
};

// Helper to build stream URL for a camera
export const getStreamUrl = (cameraId: number | string) =>
    `${API_Base}/stream/${cameraId}`;

export const api = axios.create({
    baseURL: API_Base,
    timeout: 30000,
    withCredentials: true,
});

api.interceptors.request.use((config) => {
    const token = getAuthToken();
    const headers = (config.headers || {}) as AxiosRequestHeaders;
    if (token) {
        headers.Authorization = `Bearer ${token}`;
    }
    const method = (config.method || 'get').toUpperCase();
    if (!['GET', 'HEAD', 'OPTIONS'].includes(method)) {
        const csrf = csrfTokenFromCookie();
        if (csrf && !headers['x-csrf-token']) {
            headers['x-csrf-token'] = csrf;
        }
    }
    config.headers = headers;
    return config;
});


// Retry configuration
const MAX_RETRIES = 3;
const RETRY_DELAY_BASE = 1000; // 1s base, exponential backoff

// Axios retry interceptor for automatic retries on network failures
api.interceptors.response.use(
    (response) => {
        const traceId = response.headers?.['x-trace-id'];
        if (traceId) {
            console.debug(`[trace] ${response.config?.url} ${traceId}`);
        }
        return response;
    },
    async (error: AxiosError) => {
        const config = error.config as (AxiosRequestConfig & { _retryCount?: number });
        const traceId = error.response?.headers?.['x-trace-id'];
        if (traceId) {
            console.warn(`[trace] ${config?.url} ${traceId}`);
        }

        // Initialize retry count
        if (!config._retryCount) {
            config._retryCount = 0;
        }

        // Check if we should retry
        const isNetworkError = !error.response;
        const isServerError = error.response?.status && error.response.status >= 500;
        const shouldRetry = (isNetworkError || isServerError) && config._retryCount < MAX_RETRIES;

        if (shouldRetry) {
            config._retryCount += 1;
            const delay = RETRY_DELAY_BASE * Math.pow(2, config._retryCount - 1);
            console.log(`API retry attempt ${config._retryCount}/${MAX_RETRIES} after ${delay}ms`);

            await new Promise(resolve => setTimeout(resolve, delay));
            return api(config);
        }

        return Promise.reject(error);
    }
);

export type SystemEvent =
    | { type: "DeviceConnected", kind: string, id: string }
    | { type: "DeviceDisconnected", id: string }
    | { type: "TurntableStatus", connected: boolean, angle: number, moving: boolean }
    | { type: string, payload?: unknown }; // Fallback

export const getSystemStats = async () => {
    const res = await fetchAuthed(`${API_Base}/system/stats`, { headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

// WebSocket Connection
export const connectWebSocket = (
    onMessage: (event: SystemEvent) => void,
    onConnect?: () => void,
    onDisconnect?: () => void
) => {
    let ws: WebSocket | null = null;
    let keepAlive: ReturnType<typeof setInterval> | null = null;

    const connect = () => {
        establishSession().then((token) => {
            if (!token) {
                throw new Error('Missing auth token');
            }
            const socket = new WebSocket(WS_Base);
            ws = socket;

            socket.onopen = () => {
                console.log("Connected to System Event Bus");
                if (onConnect) onConnect();

                // Trigger device re-scan on connect (Hot-plug via frontend load)
                // We use fetch directly or import the function
                // To avoid circular dependencies if scanHardware uses api/client, we can just fetch here
                fetchAuthed(`${API_Base}/hardware/scan`, { method: 'POST', headers: authHeaders() }).catch(console.error);

                keepAlive = setInterval(() => {
                    if (socket.readyState === WebSocket.OPEN) {
                        socket.send(JSON.stringify({ type: 'PING' }));
                    }
                }, 30000);
            };

            socket.onmessage = (event) => {
                try {
                    const data = JSON.parse(event.data) as SystemEvent;
                    onMessage(data);
                } catch (e) {
                    console.error("WS Parse Error", e);
                }
            };

            socket.onclose = () => {
                if (keepAlive) {
                    clearInterval(keepAlive);
                }
                if (onDisconnect) onDisconnect();
                setTimeout(connect, 2000); // Reconnect
            };

            socket.onerror = (err) => {
                console.error("WS Error", err);
                socket.close();
            };
        }).catch((e) => {
            console.error("WS Session Failed", e);
            setTimeout(connect, 2000);
        });
    };

    connect();

    return { ws, close: () => ws?.close() };
};

export const createProject = async (name: string, description?: string) => {
    const res = await fetchAuthed(`${API_Base}/projects`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify({ name, description })
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export interface ProjectSummary {
    name: string;
    created?: string;
}

export const getProjects = async (): Promise<ProjectSummary[]> => {
    const res = await fetchAuthed(`${API_Base}/projects`, { headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export interface ImuDiagnostics {
    status: string;
    samples: number;
    duration_seconds: number;
    sample_rate_hz: number;
    accel_mean: number;
    accel_rms: number;
    accel_peak: number;
    gyro_mean: number;
    gyro_rms: number;
    gyro_peak: number;
    warnings: string[];
}

export const getImuDiagnostics = async (projectId: string): Promise<ImuDiagnostics> => {
    const res = await fetchAuthed(`${API_Base}/projects/${encodeURIComponent(projectId)}/imu/diagnostics`, {
        headers: authHeaders(),
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export interface ProjectLicense {
    title?: string | null;
    url?: string | null;
    data_ownership?: string | null;
    export_rights?: string | null;
    updated_at?: string | null;
}

export const getProjectLicense = async (projectId: string): Promise<ProjectLicense> => {
    const res = await fetchAuthed(`${API_Base}/projects/${encodeURIComponent(projectId)}/license`, {
        headers: authHeaders(),
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const updateProjectLicense = async (projectId: string, payload: ProjectLicense): Promise<ProjectLicense> => {
    const res = await fetchAuthed(`${API_Base}/projects/${encodeURIComponent(projectId)}/license`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export interface ProjectAsset {
    path: string;
    bytes: number;
    modified_at?: string | null;
}

export const listProjectAssets = async (projectId: string, scope: 'raw' | 'output' | 'processed' | 'all' = 'output') => {
    const res = await fetchAuthed(`${API_Base}/projects/${encodeURIComponent(projectId)}/assets?scope=${scope}`, { headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json() as Promise<ProjectAsset[]>;
};

export interface FusionArtifactRef {
    path: string;
    present: boolean;
    bytes?: number | null;
}

export interface FusionFlagSummary {
    bit: number;
    pixels: number;
}

export interface FusionReportSummary {
    report_path: string;
    report_sha256: string;
    label: string;
    modified_at?: string | null;
    schema: string;
    width: number;
    height: number;
    integrity_complete: boolean;
    warnings: string[];
    artifacts: Record<string, FusionArtifactRef>;
    flags: Record<string, FusionFlagSummary>;
    frequency_flags: Record<string, FusionFlagSummary>;
    boundary_trimap_legend: Record<string, number>;
    sensor_correction_legend: Record<string, number>;
    metrics: Record<string, number>;
    policy: {
        archival: string;
        focus: string;
        boundary: string;
        glare: string;
        frequency: string;
    };
    calibration: {
        noise_model_calibrated: boolean;
        lens_psf_calibrated: boolean;
        sensor_correction_id?: string | null;
        lens_psf_calibration_id?: string | null;
    };
    demosaic: {
        backend?: string | null;
        adapter?: string | null;
        fallback: boolean;
        generative_reconstruction: boolean;
    };
    performance: {
        decode_seconds?: number | null;
        fusion_seconds?: number | null;
        demosaic_and_postprocess_seconds?: number | null;
        processing_before_export_seconds?: number | null;
        decoded_megapixels?: number | null;
        admitted_peak_memory_bytes?: number | null;
        major_page_faults?: number | null;
    };
    capture_group_id?: string | null;
    revision_group_id?: string | null;
    frame_count?: number | null;
    crop_origin_x?: number | null;
    crop_origin_y?: number | null;
    glare_physical_scale: boolean;
    trimap_physical_scale: boolean;
    fusion_edit_digest?: string | null;
    editable_base: boolean;
    revision_executable: boolean;
}

export interface FusionReportInventory {
    reports: FusionReportSummary[];
    rejected_reports: number;
    truncated: boolean;
}

export const listFusionReports = async (projectId: string, limit = 32): Promise<FusionReportInventory> => {
    const params = new URLSearchParams({ limit: String(Math.max(1, Math.min(128, limit))) });
    const res = await fetchAuthed(
        `${API_Base}/projects/${encodeURIComponent(projectId)}/fusion-reports?${params.toString()}`,
        { headers: authHeaders() },
    );
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const fetchFusionArtifact = async (projectId: string, artifactPath: string): Promise<Blob> => {
    const encodedPath = artifactPath
        .split('/')
        .filter(Boolean)
        .map(segment => encodeURIComponent(segment))
        .join('/');
    const res = await fetchAuthed(
        `${API_Base}/projects/${encodeURIComponent(projectId)}/fusion-artifact/${encodedPath}`,
        { headers: authHeaders() },
    );
    if (!res.ok) throw new Error(await res.text());
    return res.blob();
};

export type FusionEditReason = 'motion' | 'disocclusion' | 'focus' | 'glare' | 'boundary' | 'other';
export type FusionEditSelector = 'rectangle' | 'glare_affected' | 'boundary_affected' | 'boundary_crossing_core';

export interface FusionEditOperation {
    id: string;
    rect: { x: number; y: number; width: number; height: number };
    source_frame: number;
    reason: FusionEditReason;
    selector: FusionEditSelector;
    note?: string;
}

export interface FusionEditReceipt {
    schema: string;
    capture_group_id: string;
    base_report_sha256: string;
    digest: string;
    path: string;
    operations: number;
    edited_pixels: number;
    encrypted: boolean;
    download_filename: string;
    cli_argument: string | null;
    document: {
        schema: string;
        capture_group_id: string;
        base_report_sha256: string;
        width: number;
        height: number;
        crop_origin_x: number;
        crop_origin_y: number;
        frame_count: number;
        operations: FusionEditOperation[];
    };
}

export const createFusionEdit = async (
    projectId: string,
    reportPath: string,
    operations: FusionEditOperation[],
): Promise<FusionEditReceipt> => {
    const res = await fetchAuthed(
        `${API_Base}/projects/${encodeURIComponent(projectId)}/fusion-edits`,
        {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ report_path: reportPath, operations }),
        },
    );
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export interface FusionRevisionJob {
    id: string;
    request_id?: string | null;
    kind: string;
    name: string;
    status: 'pending' | 'running' | 'completed' | 'failed' | 'cancelled';
    progress: number;
    attempts: number;
    max_attempts: number;
    created_at: string;
    started_at?: string | null;
    finished_at?: string | null;
    last_error?: string | null;
}

export const executeFusionRevision = async (
    projectId: string,
    report: Pick<FusionReportSummary, 'report_path' | 'report_sha256'>,
    edit: Pick<FusionEditReceipt, 'path' | 'digest'>,
): Promise<FusionRevisionJob> => {
    const res = await fetchAuthed(
        `${API_Base}/projects/${encodeURIComponent(projectId)}/fusion-revisions`,
        {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                request_id: crypto.randomUUID(),
                report_path: report.report_path,
                report_sha256: report.report_sha256,
                edit_path: edit.path,
                edit_digest: edit.digest,
            }),
        },
    );
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const getFusionRevision = async (
    projectId: string,
    jobId: string,
): Promise<FusionRevisionJob> => {
    const res = await fetchAuthed(
        `${API_Base}/projects/${encodeURIComponent(projectId)}/fusion-revisions/${encodeURIComponent(jobId)}`,
        { headers: authHeaders() },
    );
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const cancelFusionRevision = async (projectId: string, jobId: string): Promise<void> => {
    const res = await fetchAuthed(
        `${API_Base}/projects/${encodeURIComponent(projectId)}/fusion-revisions/${encodeURIComponent(jobId)}/cancel`,
        { method: 'POST', headers: authHeaders() },
    );
    if (!res.ok) throw new Error(await res.text());
};

export type MeshEditOp =
    | { op: 'smooth'; iterations: number; lambda: number }
    | { op: 'decimate'; target_triangles: number; preserve_boundaries: boolean; preserve_uv_seams: boolean; uv_seam_threshold: number }
    | { op: 'recompute_normals' }
    | { op: 'fill_holes'; max_hole_vertices: number };

export type SplatEditOp =
    | { op: 'prune_opacity'; min_alpha: number }
    | { op: 'bounds'; min: [number, number, number]; max: [number, number, number] }
    | { op: 'sphere'; center: [number, number, number]; radius: number }
    | { op: 'density'; target: number };

export interface EditResponse {
    id: string;
    output_path: string;
    history_path: string;
}

export interface EditHistoryEntry {
    id: string;
    created_at: string;
    asset_type: string;
    input_path: string;
    output_path: string;
    operations: unknown;
}

export const applyMeshEdits = async (projectId: string, payload: {
    input_path: string;
    output_name?: string;
    output_format?: string;
    ops: MeshEditOp[];
}) => {
    const res = await fetchAuthed(`${API_Base}/projects/${projectId}/edits/mesh`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json() as Promise<EditResponse>;
};

export const applySplatEdits = async (projectId: string, payload: {
    input_path: string;
    output_name?: string;
    write_spz?: boolean;
    ops: SplatEditOp[];
}) => {
    const res = await fetchAuthed(`${API_Base}/projects/${projectId}/edits/splat`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json() as Promise<EditResponse>;
};

export const getEditHistory = async (projectId: string) => {
    const res = await fetchAuthed(`${API_Base}/projects/${projectId}/edits/history`, { headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json() as Promise<EditHistoryEntry[]>;
};

export interface ShareLinkResponse {
    token: string;
    asset_url: string;
    download_url: string;
    viewer_url: string;
    short_url?: string | null;
    card_url?: string | null;
    lods?: { level: number; asset_url: string; bytes: number }[];
    expires_at: number;
    max_uses?: number | null;
    remaining_uses?: number | null;
    allow_download: boolean;
    allow_embed: boolean;
    project_id: string;
    asset_path: string;
    public?: boolean | null;
}

export interface ShareReferrerCount {
    referrer: string;
    count: number;
}

export interface ShareAnalytics {
    views: number;
    asset_requests: number;
    downloads: number;
    embeds: number;
    last_access?: number | null;
    top_referrers: ShareReferrerCount[];
}

export interface SharePublicResponse {
    public: boolean;
    short_url?: string | null;
    card_url: string;
    viewer_url: string;
    title?: string | null;
    description?: string | null;
    tags: string[];
    cover_path?: string | null;
    created_at: number;
    updated_at: number;
}

export interface PublicShareSummary {
    token: string;
    short_code: string;
    short_url: string;
    viewer_url: string;
    card_url: string;
    asset_url: string;
    download_url: string;
    title?: string | null;
    description?: string | null;
    tags: string[];
    cover_path?: string | null;
    created_at: number;
    updated_at: number;
    views: number;
}

export interface AnnotationPoint {
    id: string;
    label: string;
    position: [number, number, number];
    created_at?: number;
    author?: string | null;
}

export interface AnnotationLayer {
    asset_path: string;
    layer: string;
    created_at: number;
    updated_at: number;
    annotations: AnnotationPoint[];
}

export const createShareLink = async (payload: {
    project_id: string;
    asset_path: string;
    expires_in_seconds?: number;
    max_uses?: number;
    allow_download?: boolean;
    allow_embed?: boolean;
    public?: boolean;
    title?: string;
    description?: string;
    tags?: string[];
    short_code?: string;
    cover_path?: string;
}) => {
    const res = await fetchAuthed(`${API_Base}/share`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json() as Promise<ShareLinkResponse>;
};

export const getShareAnalytics = async (token: string) => {
    const res = await fetchAuthed(`${API_Base}/share/${token}/analytics`, { headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json() as Promise<ShareAnalytics>;
};

export const setSharePublic = async (token: string, payload: {
    public: boolean;
    title?: string;
    description?: string;
    tags?: string[];
    short_code?: string;
    cover_path?: string;
}) => {
    const res = await fetchAuthed(`${API_Base}/share/${token}/public`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json() as Promise<SharePublicResponse>;
};

export const getSharePublic = async (token: string) => {
    const res = await fetchAuthed(`${API_Base}/share/${token}/public`, { headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json() as Promise<SharePublicResponse>;
};

export const listPublicShares = async (payload: {
    limit?: number;
    offset?: number;
    tag?: string;
    sort?: 'recent' | 'popular';
}) => {
    const params = new URLSearchParams();
    if (payload.limit) params.set('limit', payload.limit.toString());
    if (payload.offset) params.set('offset', payload.offset.toString());
    if (payload.tag) params.set('tag', payload.tag);
    if (payload.sort) params.set('sort', payload.sort);
    const res = await fetchWithTrace(`${API_Base}/public/shares?${params.toString()}`);
    if (!res.ok) throw new Error(await res.text());
    return res.json() as Promise<PublicShareSummary[]>;
};

export const getShareAnnotations = async (token: string, layer?: string) => {
    const params = new URLSearchParams();
    if (layer) params.set('layer', layer);
    const res = await fetchWithTrace(`${API_Base}/share/${token}/annotations?${params.toString()}`);
    if (!res.ok) throw new Error(await res.text());
    return res.json() as Promise<AnnotationLayer>;
};

export const getProjectAnnotations = async (projectId: string, assetPath: string, layer?: string) => {
    const params = new URLSearchParams();
    params.set('asset_path', assetPath);
    if (layer) params.set('layer', layer);
    const res = await fetchAuthed(`${API_Base}/projects/${projectId}/annotations?${params.toString()}`, { headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json() as Promise<AnnotationLayer>;
};

export const saveProjectAnnotations = async (projectId: string, payload: {
    asset_path: string;
    layer?: string;
    annotations: AnnotationPoint[];
    merge?: boolean;
}) => {
    const res = await fetchAuthed(`${API_Base}/projects/${projectId}/annotations`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json() as Promise<AnnotationLayer>;
};



export const stopScan = async () => {
    const res = await fetchAuthed(`${API_Base}/scan/stop`, {
        method: 'POST',
        headers: authHeaders(),
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};



export const openProjectFolder = async (id: string) => {
    const res = await fetchAuthed(`${API_Base}/projects/${id}/open`, { method: 'POST', headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const importModel = async (id: string, file: File) => {
    const formData = new FormData();
    formData.append('file', file);
    const res = await fetchAuthed(`${API_Base}/projects/${id}/import`, {
        method: 'POST',
        headers: authHeaders(),
        body: formData
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const getHardwareStatus = async () => {
    // We don't have a dedicated hardware endpoint yet, assume WS pushes updates
    // In V2 add GET /api/status
    return {
        camera: "Connected",
        turntable: "Connected",
        battery: 100
    };
};
export const captureCalibrationFrame = async () => {
    const res = await fetchAuthed(`${API_Base}/calibration/capture`, { method: 'POST', headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const computeCalibration = async (rows = 9, cols = 6, size = 25.0) => {
    const res = await fetchAuthed(`${API_Base}/calibration/compute?rows=${rows}&cols=${cols}&square_size_mm=${size}`, { method: 'POST', headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const computeColorCalibration = async (payload: { camera_id?: string; frame_index?: number } = {}) => {
    const params = new URLSearchParams();
    if (payload.camera_id) params.set('camera_id', payload.camera_id);
    if (payload.frame_index !== undefined) params.set('frame_index', String(payload.frame_index));
    const suffix = params.toString().length ? `?${params.toString()}` : '';
    const res = await fetchAuthed(`${API_Base}/calibration/color/compute${suffix}`, { method: 'POST', headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const clearCalibrationSession = async () => {
    const res = await fetchAuthed(`${API_Base}/calibration/session`, { method: 'DELETE', headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export interface StorageInfo {
    capacity_gb: number;
    free_gb: number;
    remaining_shots?: number;
}

export interface CameraProfile {
    id: string;
    name: string;
    nickname?: string;
    connected?: boolean;
    battery_level?: number;  // 0-100 percentage
    capabilities: {
        resolutions: [number, number][];
        frame_rates: number[];
        has_gimbal: boolean;
        has_zoom: boolean;
        has_autofocus: boolean;
        // Advanced DSLR
        iso_options: string[];
        shutter_speed_options: string[];
        aperture_options: string[];
        wb_options?: string[];
        storage_info?: StorageInfo;
    };
    last_settings?: {
        iso?: string;
        shutter_speed?: string;
        aperture?: string;
        wb?: string;
        // ...
    }
}

export interface IntervalometerRamp {
    shutter_start?: string | null;
    shutter_end?: string | null;
    iso_start?: string | null;
    iso_end?: string | null;
}

export interface IntervalometerStatus {
    camera_id: string;
    active: boolean;
    interval_ms: number;
    total_frames?: number | null;
    captured_frames: number;
    started_at: string;
    last_capture_at?: string | null;
    next_capture_at?: string | null;
    last_error?: string | null;
    ramp?: IntervalometerRamp | null;
}

export interface IntervalometerStartRequest {
    interval_ms: number;
    total_frames?: number | null;
    ramp?: IntervalometerRamp | null;
    capture_target?: string | null;
}

export interface CaptureSequenceResult {
    status: string;
    shots: string[];
}

export interface HdrBracketRequest {
    bracket_count: number;
    ev_spacing: number;
    base_shutter?: string | null;
    capture_target?: string | null;
}

export interface FocusStackRequest {
    slice_count: number;
    step_size: number;
    direction: 'near' | 'far';
    capture_target?: string | null;
}

export interface HdrFocusStackRequest {
    bracket_count: number;
    ev_spacing: number;
    base_shutter?: string | null;
    slice_count: number;
    step_size: number;
    direction: 'near' | 'far';
    capture_target?: string | null;
}

export interface AdaptiveCaptureCandidate {
    shutter_seconds: number;
    iso: number;
    focus_diopters: number;
    readout_ms: number;
    settle_ms: number;
}

export interface AdaptiveCandidateUtility {
    candidate: AdaptiveCaptureCandidate;
    hdr_information_nats: number;
    focus_information_nats: number;
    capture_cost_ms: number;
    utility_per_ms: number;
}

export type AdaptiveCandidateEvaluation =
    | { status: 'eligible'; utility: AdaptiveCandidateUtility }
    | {
        status: 'rejected';
        candidate: AdaptiveCaptureCandidate;
        reason: 'missing_exact_iso_calibration' | 'time_budget' | 'thermal_budget' | 'motion_blur';
        predicted_cost_ms?: number | null;
    };

export interface AdaptiveCaptureStatus {
    decision: {
        selected?: AdaptiveCandidateUtility | null;
        stop_hdr: boolean;
        stop_focus: boolean;
        hdr_target_reached: boolean;
        focus_target_reached: boolean;
        rejected_motion: number;
        rejected_budget: number;
        rejected_calibration: number;
        evaluations: AdaptiveCandidateEvaluation[];
    };
    posterior: {
        radiance: Array<{ probe_id: number; mean: number; variance: number; weight: number; cfa_site: number }>;
        focus: Array<{ probe_id: number; mean_diopters: number; variance_diopters2: number; weight: number }>;
        radiance_anchor_exposure: number;
        current_focus_diopters: number;
        motion_pixels_per_second: number;
        elapsed_ms: number;
        thermal_load: number;
    };
    retained_frame_count: number;
    termination?: string | null;
}

export interface AdaptiveCaptureSession {
    session_id: string;
    camera_id: string;
    project_id?: string | null;
    generation: number;
    status: AdaptiveCaptureStatus;
}

export interface StartAdaptiveCaptureRequest {
    camera_id: string;
    project_id: string;
    reference_raw_path: string;
    sensor_profile_path: string;
    roi: { x: number; y: number; width: number; height: number };
    focus_diopters: number[];
    readout_ms: number;
    settle_ms: number;
    planner?: {
        remaining_time_ms?: number;
        maximum_motion_blur_px?: number;
        maximum_thermal_load?: number;
        thermal_load_per_second?: number;
        lens_ms_per_diopter?: number;
        focus_psf_sigma_diopters?: number;
        focus_measurement_variance?: number;
        target_radiance_variance?: number;
        target_focus_variance_diopters2?: number;
        minimum_hdr_information_nats?: number;
        minimum_focus_information_nats?: number;
    };
    observation?: {
        tile_columns?: number;
        tile_rows?: number;
        maximum_samples_per_tile_site?: number;
        minimum_radiance_samples?: number;
        minimum_focus_samples?: number;
    };
}

export interface AdaptiveCaptureStep {
    session_id: string;
    generation: number;
    capture_path: string;
    selected: AdaptiveCaptureCandidate;
    camera_options: { shutter_speed: string; iso: string };
    measured_capture_elapsed_ms: number;
    report: {
        radiance_updates: number;
        censored_constraints: number;
        censor_conflicts: number;
        focus_updates: number;
        accumulated_focus_planes: number;
    };
    status: AdaptiveCaptureStatus;
}

export const getCameras = async (): Promise<CameraProfile[]> => {
    const res = await fetchAuthed(`${API_Base}/cameras`, { headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const updateCameraNickname = async (id: string, nickname: string) => {
    const res = await fetchAuthed(`${API_Base}/cameras/${id}/nickname`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify({ nickname })
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const setCameraPtz = async (id: string, pan: number, tilt: number, zoom: number) => {
    const res = await fetchAuthed(`${API_Base}/cameras/${id}/ptz`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify({ pan, tilt, zoom })
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const setCameraConfig = async (
    id: string,
    config: { iso?: string, shutter_speed?: string, aperture?: string, wb?: string, capture_target?: string }
) => {
    const res = await fetchAuthed(`${API_Base}/cameras/${id}/config`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(config)
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const setFocusPoint = async (id: string, x: number, y: number) => {
    const res = await fetchAuthed(`${API_Base}/cameras/${id}/focus_point`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify({ x, y })
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const triggerAutofocus = async (id: string) => {
    const res = await fetchAuthed(`${API_Base}/cameras/${id}/autofocus`, { method: 'POST', headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};


export const driveFocus = async (id: string, step: number) => {
    const res = await fetchAuthed(`${API_Base}/cameras/${id}/focus/drive`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify({ step })
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const capturePhoto = async (id: string) => {
    const res = await fetchAuthed(`${API_Base}/cameras/${id}/capture`, {
        method: 'POST',
        headers: authHeaders()
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const captureHdrBracket = async (id: string, payload: HdrBracketRequest): Promise<CaptureSequenceResult> => {
    const res = await fetchAuthed(`${API_Base}/cameras/${id}/hdr`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload)
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const captureFocusStack = async (id: string, payload: FocusStackRequest): Promise<CaptureSequenceResult> => {
    const res = await fetchAuthed(`${API_Base}/cameras/${id}/focus_stack`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload)
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const captureHdrFocusStack = async (id: string, payload: HdrFocusStackRequest): Promise<CaptureSequenceResult> => {
    const res = await fetchAuthed(`${API_Base}/cameras/${id}/hdr_focus_stack`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload)
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const startAdaptiveCapture = async (
    payload: StartAdaptiveCaptureRequest
): Promise<AdaptiveCaptureSession> => {
    const res = await fetchAuthed(`${API_Base}/cameras/adaptive`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload),
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const captureNextAdaptive = async (
    sessionId: string,
    payload: {
        confirmed_focus_diopters: number;
        motion_pixels_per_second: number;
        thermal_load: number;
    }
): Promise<AdaptiveCaptureStep> => {
    const res = await fetchAuthed(
        `${API_Base}/cameras/adaptive/${encodeURIComponent(sessionId)}/capture-next`,
        {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', ...authHeaders() },
            body: JSON.stringify(payload),
        },
    );
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const getAdaptiveCapture = async (sessionId: string): Promise<AdaptiveCaptureSession> => {
    const res = await fetchAuthed(
        `${API_Base}/cameras/adaptive/${encodeURIComponent(sessionId)}`,
        { headers: authHeaders() },
    );
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const terminateAdaptiveCapture = async (
    sessionId: string,
    reason: 'operator_stopped' | 'hardware_failure'
): Promise<AdaptiveCaptureSession> => {
    const res = await fetchAuthed(
        `${API_Base}/cameras/adaptive/${encodeURIComponent(sessionId)}/terminate`,
        {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', ...authHeaders() },
            body: JSON.stringify({ reason }),
        },
    );
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const startIntervalometer = async (id: string, payload: IntervalometerStartRequest): Promise<IntervalometerStatus> => {
    const res = await fetchAuthed(`${API_Base}/cameras/${id}/interval/start`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify(payload)
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const stopIntervalometer = async (id: string): Promise<IntervalometerStatus> => {
    const res = await fetchAuthed(`${API_Base}/cameras/${id}/interval/stop`, {
        method: 'POST',
        headers: authHeaders()
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const getIntervalometerStatus = async (id: string): Promise<IntervalometerStatus> => {
    const res = await fetchAuthed(`${API_Base}/cameras/${id}/interval/status`, {
        headers: authHeaders()
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};
// --- Turntable API ---
export interface TurntableStatus {
    connected: boolean;
    type: string;
    angle: number;
    moving: boolean;
}

export const getTurntableStatus = async (): Promise<TurntableStatus> => {
    const res = await fetchAuthed(`${API_Base}/turntable/status`, { headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const rotateTurntable = async (degrees: number) => {
    const res = await fetchAuthed(`${API_Base}/turntable/rotate`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify({ degrees })
    });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const homeTurntable = async () => {
    const res = await fetchAuthed(`${API_Base}/turntable/home`, { method: 'POST', headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

export const scanHardware = async () => {
    const res = await fetchAuthed(`${API_Base}/hardware/scan`, { method: 'POST', headers: authHeaders() });
    if (!res.ok) throw new Error(await res.text());
    return res.json();
};

// ============================================================================
// ScanWizard API - Intelligent Scanning Endpoints
// ============================================================================

export interface BackgroundStatus {
    captured: boolean;
    timestamp: string | null;
    frame_count: number;
}

export interface BoundingBox {
    x: number;
    y: number;
    width: number;
    height: number;
}

export interface ObjectDetection {
    detected: boolean;
    confidence: number;
    bounding_box: BoundingBox | null;
    stable: boolean;
    stable_duration_ms: number;
}

export interface SizeInfo {
    category: 'tiny' | 'small' | 'medium' | 'large' | 'xlarge';
    dimensions: [number, number, number];
}

export interface ComplexityInfo {
    category: 'simple' | 'moderate' | 'complex' | 'intricate';
    feature_count: number;
    score: number;
}

export interface SurfaceInfo {
    surface_type: 'matte' | 'glossy' | 'transparent' | 'mixed';
    specular_ratio: number;
}

export interface ObjectAnalysis {
    size: SizeInfo;
    complexity: ComplexityInfo;
    surface: SurfaceInfo;
    has_underside_detail: boolean;
    aspect_ratio: number;
}

export interface ScanStep {
    step_type: 'camera_position' | 'object_orientation' | 'capture';
    instruction: string;
    camera_position?: number;
    object_orientation?: number;
    rotation_angle?: number;
    photo_index?: number;
}

export interface ScanPlan {
    quality_level: string;
    object_orientations: number;
    camera_positions_per_orientation: number;
    photos_per_rotation: number;
    total_photos: number;
    estimated_time_seconds: number;
    steps: ScanStep[];
}

export interface ScanProgress {
    status: 'idle' | 'capturing' | 'paused' | 'complete' | 'error' | 'stopped';
    current_step: number;
    total_steps: number;
    photos_captured: number;
    current_instruction: string;
    error_message: string | null;
    warnings?: string[];
    quality?: QualityAssessment | null;
}

export interface SDCardStatus {
    detected: boolean;
    volume_name: string | null;
    image_count: number;
    total_size_mb: number;
}

export interface QualityDefectScore {
    defect: string;
    score: number;
    threshold: number;
    status: string;
}

export interface QualityAssessment {
    score: number;
    pass: boolean;
    issues: string[];
    actions: string[];
    defects: QualityDefectScore[];
}

export interface QualityHistoryEntry {
    captured_at: string;
    score: number;
    pass: boolean;
    issues: string[];
    actions: string[];
}

export interface ScaleAnchor {
    known_distance_m: number;
    measured_units: number;
    meters_per_unit: number;
    label?: string | null;
    origin_lat?: number | null;
    origin_lon?: number | null;
    origin_alt?: number | null;
    crs?: string | null;
    updated_at: string;
}

export interface ScaleAnchorStatus {
    configured: boolean;
    anchor?: ScaleAnchor | null;
}

export interface ScaleAnchorRequest {
    known_distance_m: number;
    measured_units: number;
    label?: string | null;
    origin_lat?: number | null;
    origin_lon?: number | null;
    origin_alt?: number | null;
    crs?: string | null;
}

// Background Calibration
export const wizard = {
    // Background
    getBackgroundStatus: async (): Promise<BackgroundStatus> => {
        const res = await fetchAuthed(`${API_Base}/wizard/background/status`, { headers: authHeaders() });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    captureBackground: async (): Promise<{ success: boolean; frame_count: number; timestamp: string }> => {
        const res = await fetchAuthed(`${API_Base}/wizard/background/capture`, { method: 'POST', headers: authHeaders() });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    // Detection
    getDetectionStatus: async (): Promise<ObjectDetection> => {
        const res = await fetchAuthed(`${API_Base}/wizard/detection/status`, { headers: authHeaders() });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    // Analysis
    analyzeObject: async (): Promise<ObjectAnalysis> => {
        const res = await fetchAuthed(`${API_Base}/wizard/analyze`, { method: 'POST', headers: authHeaders() });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    // Plan
    computePlan: async (qualityLevel: string, analysis: ObjectAnalysis, preset?: string): Promise<ScanPlan> => {
        const res = await fetchAuthed(`${API_Base}/wizard/plan/compute`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', ...authHeaders() },
            body: JSON.stringify({ quality_level: qualityLevel, analysis, preset }),
        });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    // Quality
    getQuality: async (): Promise<QualityAssessment> => {
        const res = await fetchAuthed(`${API_Base}/wizard/quality`, { headers: authHeaders() });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    getUncertaintyMap: async (): Promise<Blob> => {
        const res = await fetchAuthed(`${API_Base}/wizard/quality/uncertainty`, { headers: authHeaders() });
        if (!res.ok) throw new Error(await res.text());
        return res.blob();
    },

    getQualityHistory: async (): Promise<QualityHistoryEntry[]> => {
        const res = await fetchAuthed(`${API_Base}/wizard/quality/history`, { headers: authHeaders() });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    // Scale anchor
    getScaleAnchor: async (): Promise<ScaleAnchorStatus> => {
        const res = await fetchAuthed(`${API_Base}/wizard/scale-anchor`, { headers: authHeaders() });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    setScaleAnchor: async (payload: ScaleAnchorRequest): Promise<ScaleAnchor> => {
        const res = await fetchAuthed(`${API_Base}/wizard/scale-anchor`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', ...authHeaders() },
            body: JSON.stringify(payload),
        });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },
};

// Scan Execution
export const scan = {
    start: async (payload?: { auto_capture?: boolean }): Promise<{ status: string; session_id: string }> => {
        const res = await fetchAuthed(`${API_Base}/scan/start`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', ...authHeaders() },
            body: payload ? JSON.stringify(payload) : undefined,
        });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    stop: async (): Promise<{ status: string }> => {
        const res = await fetchAuthed(`${API_Base}/scan/stop`, { method: 'POST', headers: authHeaders() });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    getProgress: async (): Promise<ScanProgress> => {
        const res = await fetchAuthed(`${API_Base}/scan/progress`, { headers: authHeaders() });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    getCoverage: async (): Promise<CoverageStatus> => {
        const res = await fetchAuthed(`${API_Base}/scan/coverage`, { headers: authHeaders() });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    executeStep: async (stepIndex: number): Promise<{ success: boolean; step_index: number }> => {
        const res = await fetchAuthed(`${API_Base}/scan/execute-step`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', ...authHeaders() },
            body: JSON.stringify({ step_index: stepIndex }),
        });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    capture: async (): Promise<{ success: boolean; timestamp: string }> => {
        const res = await fetchAuthed(`${API_Base}/scan/capture`, { method: 'POST', headers: authHeaders() });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    // SD Card
    getSDCardStatus: async (): Promise<SDCardStatus> => {
        const res = await fetchAuthed(`${API_Base}/scan/sdcard/status`, { headers: authHeaders() });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },

    importFromSDCard: async (): Promise<{ success: boolean; imported_count: number; session_matched: boolean }> => {
        const res = await fetchAuthed(`${API_Base}/scan/sdcard/import`, { method: 'POST', headers: authHeaders() });
        if (!res.ok) throw new Error(await res.text());
        return res.json();
    },
};
