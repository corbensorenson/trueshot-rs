/**
 * Camera Control Pro - Advanced DSLR/Mirrorless Control
 * 
 * Professional tethered capture with:
 * - HDR bracketing with auto-merge
 * - Focus stacking with auto-stack
 * - Combined HDR + Focus Stack workflows
 * - Save location control (camera/computer/both)
 */

import { useState, useEffect } from 'react';
import {
    Camera, Sun, Layers, Focus,
    Play, Square, HardDrive, Folder,
    ChevronDown, ChevronUp, Check, X, Loader2, RefreshCw,
    Zap, Image, Gauge, Target
} from 'lucide-react';
import toast from 'react-hot-toast';
import { ThemeToggleFloating } from './ThemeToggleFloating';
import {
    getCameras,
    CameraProfile,
    startIntervalometer,
    stopIntervalometer,
    getIntervalometerStatus,
    IntervalometerStatus,
    IntervalometerRamp,
    setCameraConfig,
    driveFocus,
    capturePhoto,
    captureHdrBracket,
    captureFocusStack,
    captureHdrFocusStack,
    getLicenseBundles,
    getLicenseStatus,
    createLicenseTrial,
    type LicenseBundleInfo,
    type LicenseStatusResponse
} from '../api/client';
import { FeatureUnlockPanel } from './FeatureUnlockPanel';

// ============================================================================
// Types
// ============================================================================

type CaptureMode = 'single' | 'hdr' | 'focus-stack' | 'hdr-focus';

interface HDRConfig {
    bracketCount: 3 | 5 | 7 | 9;
    evSpacing: 1 | 2 | 3;
    autoMerge: boolean;
    mergeAlgorithm: 'mertens' | 'debevec' | 'robertson';
}

interface FocusStackConfig {
    mode: 'front-to-back' | 'back-to-front' | 'center-out';
    sliceCount: number;
    stepSize: 'auto' | 'small' | 'medium' | 'large' | 'custom';
    customStepSize?: number;
    autoStack: boolean;
    stackAlgorithm: 'weighted-focus' | 'laplacian-pyramid' | 'depth-map';
}

type SaveLocation = 'camera' | 'computer' | 'both';

interface CaptureQueueItem {
    id: string;
    mode: CaptureMode;
    progress: number;
    status: 'pending' | 'capturing' | 'processing' | 'complete' | 'error';
    currentShot: number;
    totalShots: number;
    description: string;
}

interface CameraSettings {
    iso: string | number;
    aperture: string;
    shutterSpeed: string;
    whiteBalance: string;
    focusMode: string;
}

interface ConnectedCamera {
    id: string;
    name: string;
    model: string;
    connected: boolean;
    battery: number | null;
    storageUsed: number;
    storageTotal: number;
    settings: CameraSettings;
}

const formatBundlePrice = (bundle?: LicenseBundleInfo | null) => {
    if (!bundle) return 'Pricing unavailable';
    if (!bundle.price_usd) return 'Contact sales';
    const billing = bundle.billing ? ` ${bundle.billing}` : '';
    return `$${bundle.price_usd}${billing}`;
};

// ============================================================================
// Component
// ============================================================================

export default function CameraControlPro() {
    // Camera state
    const [camera, setCamera] = useState<ConnectedCamera | null>(null);
    const [loading, setLoading] = useState(true);
    const [hardwareCameras, setHardwareCameras] = useState<CameraProfile[]>([]);
    const [hardwareCameraId, setHardwareCameraId] = useState<string | null>(null);

    // Capture mode
    const [captureMode, setCaptureMode] = useState<CaptureMode>('single');

    // HDR settings
    const [hdrConfig, setHdrConfig] = useState<HDRConfig>({
        bracketCount: 5,
        evSpacing: 2,
        autoMerge: true,
        mergeAlgorithm: 'mertens',
    });

    // Focus stack settings
    const [focusStackConfig, setFocusStackConfig] = useState<FocusStackConfig>({
        mode: 'front-to-back',
        sliceCount: 15,
        stepSize: 'auto',
        autoStack: true,
        stackAlgorithm: 'weighted-focus',
    });

    // Save location
    const [saveLocation, setSaveLocation] = useState<SaveLocation>('computer');
    const [savePath, setSavePath] = useState('/Volumes/Projects/Wedding2026/RAW');

    // Capture queue
    const [captureQueue, setCaptureQueue] = useState<CaptureQueueItem[]>([]);
    const [isCapturing, setIsCapturing] = useState(false);

    // Intervalometer
    const [intervalMs, setIntervalMs] = useState(2000);
    const [intervalFrames, setIntervalFrames] = useState(120);
    const [intervalLimitEnabled, setIntervalLimitEnabled] = useState(true);
    const [intervalRampEnabled, setIntervalRampEnabled] = useState(false);
    const [intervalRamp, setIntervalRamp] = useState<IntervalometerRamp>({
        shutter_start: null,
        shutter_end: null,
        iso_start: null,
        iso_end: null
    });
    const [intervalStatus, setIntervalStatus] = useState<IntervalometerStatus | null>(null);
    const [intervalBusy, setIntervalBusy] = useState(false);

    // Licensing
    const [licenseStatus, setLicenseStatus] = useState<LicenseStatusResponse | null>(null);
    const [licenseBundles, setLicenseBundles] = useState<LicenseBundleInfo[]>([]);
    const [unlockBusy, setUnlockBusy] = useState(false);
    const [unlockError, setUnlockError] = useState<string | null>(null);

    // UI state
    const [expandedSections, setExpandedSections] = useState({
        hdr: true,
        focusStack: true,
        saveLocation: true,
        intervalometer: true,
        settings: true,
    });

    // ========================================================================
    // Initialization
    // ========================================================================

    useEffect(() => {
        loadCamera();
    }, []);

    useEffect(() => {
        refreshEntitlement();
    }, []);

    const loadCamera = async () => {
        setLoading(true);
        try {
            const cams = await getCameras();
            setHardwareCameras(cams);
            const connected = cams.find(item => item.connected);
            const selected = connected ?? cams[0];
            if (!selected || !selected.connected) {
                setCamera(null);
                setHardwareCameraId(selected ? selected.id : null);
                setIntervalStatus(null);
                setLoading(false);
                return;
            }
            setHardwareCameraId(selected.id);
            setCamera(buildConnectedCamera(selected));
            try {
                const status = await getIntervalometerStatus(selected.id);
                setIntervalStatus(status);
            } catch {
                setIntervalStatus(null);
            }
        } catch (err) {
            console.error(err);
            toast.error('Failed to load cameras');
            setCamera(null);
            setHardwareCameraId(null);
            setIntervalStatus(null);
        }
        setLoading(false);
    };

    const refreshEntitlement = async () => {
        try {
            const [status, bundles] = await Promise.all([getLicenseStatus(), getLicenseBundles()]);
            setLicenseStatus(status);
            setLicenseBundles(bundles);
        } catch {
            setLicenseStatus(null);
            setLicenseBundles([]);
        }
    };

    const buildConnectedCamera = (profile: CameraProfile): ConnectedCamera => {
        const storage = profile.capabilities.storage_info;
        const storageTotal = storage ? storage.capacity_gb * 1024 * 1024 * 1024 : 0;
        const storageUsed = storage ? (storage.capacity_gb - storage.free_gb) * 1024 * 1024 * 1024 : 0;
        const iso = profile.last_settings?.iso ?? profile.capabilities.iso_options?.[0] ?? 'Auto';
        const shutter = profile.last_settings?.shutter_speed ?? profile.capabilities.shutter_speed_options?.[0] ?? 'Auto';
        const aperture = profile.capabilities.aperture_options?.[0] ?? 'Auto';
        const wb = profile.last_settings?.wb ?? profile.capabilities.wb_options?.[0] ?? 'Auto';
        return {
            id: profile.id,
            name: profile.nickname ?? profile.name,
            model: profile.name,
            connected: Boolean(profile.connected),
            battery: profile.battery_level ?? null,
            storageUsed,
            storageTotal,
            settings: {
                iso: typeof iso === 'string' ? iso : String(iso),
                aperture: aperture.toString(),
                shutterSpeed: typeof shutter === 'string' ? shutter : String(shutter),
                whiteBalance: wb,
                focusMode: profile.capabilities.has_autofocus ? 'AF' : 'MF',
            },
        };
    };

    const selectCamera = async (cameraId: string) => {
        const selected = connectedCameras.find(item => item.id === cameraId);
        if (!selected) {
            return;
        }
        setHardwareCameraId(cameraId);
        setCamera(buildConnectedCamera(selected));
        try {
            const status = await getIntervalometerStatus(cameraId);
            setIntervalStatus(status);
        } catch {
            setIntervalStatus(null);
        }
    };

    // ========================================================================
    // Capture Logic
    // ========================================================================

    const startCapture = async () => {
        if (!camera || isCapturing || !hardwareCameraId) return;
        if (advancedLocked && captureMode !== 'single') {
            setUnlockError('Advanced capture modes require the Advanced Capture Automation add-on.');
            toast.error('Advanced capture is locked. Start a trial or upgrade to continue.');
            return;
        }

        setIsCapturing(true);

        const totalShots = calculateTotalShots();
        const queueItem: CaptureQueueItem = {
            id: `cap-${Date.now()}`,
            mode: captureMode,
            progress: 0,
            status: 'capturing',
            currentShot: 0,
            totalShots,
            description: getCaptureModeDescription(),
        };

        setCaptureQueue(prev => [...prev, queueItem]);

        const captureTarget = saveLocation === 'camera'
            ? 'Memory Card'
            : saveLocation === 'computer'
                ? 'Internal RAM'
                : 'Both';

        try {
            if (captureMode === 'single') {
                await capturePhoto(hardwareCameraId);
            } else if (captureMode === 'hdr') {
                await captureHdrBracket(hardwareCameraId, {
                    bracket_count: hdrConfig.bracketCount,
                    ev_spacing: hdrConfig.evSpacing,
                    base_shutter: null,
                    capture_target: captureTarget
                });
            } else if (captureMode === 'focus-stack') {
                await captureFocusStack(hardwareCameraId, {
                    slice_count: focusStackConfig.sliceCount,
                    step_size: focusStackConfig.stepSize === 'small' ? 6
                        : focusStackConfig.stepSize === 'large' ? 24
                            : focusStackConfig.stepSize === 'custom' ? (focusStackConfig.customStepSize ?? 12)
                                : 12,
                    direction: focusStackConfig.mode === 'back-to-front' ? 'far' : 'near',
                    capture_target: captureTarget
                });
            } else {
                await captureHdrFocusStack(hardwareCameraId, {
                    bracket_count: hdrConfig.bracketCount,
                    ev_spacing: hdrConfig.evSpacing,
                    base_shutter: null,
                    slice_count: focusStackConfig.sliceCount,
                    step_size: focusStackConfig.stepSize === 'small' ? 6
                        : focusStackConfig.stepSize === 'large' ? 24
                            : focusStackConfig.stepSize === 'custom' ? (focusStackConfig.customStepSize ?? 12)
                                : 12,
                    direction: focusStackConfig.mode === 'back-to-front' ? 'far' : 'near',
                    capture_target: captureTarget
                });
            }

            setCaptureQueue(prev => prev.map(item =>
                item.id === queueItem.id
                    ? { ...item, status: 'complete', progress: 100, currentShot: totalShots }
                    : item
            ));
            toast.success(`${getCaptureModeLabel()} capture complete!`);
        } catch (err) {
            console.error(err);
            setCaptureQueue(prev => prev.map(item =>
                item.id === queueItem.id
                    ? { ...item, status: 'error' }
                    : item
            ));
            toast.error(`${getCaptureModeLabel()} capture failed`);
        } finally {
            setIsCapturing(false);
        }
    };

    const cancelCapture = () => {
        setIsCapturing(false);
        setCaptureQueue(prev => prev.map(item =>
            item.status === 'capturing'
                ? { ...item, status: 'error' }
                : item
        ));
        toast.error('Capture cancelled');
    };

    const startInterval = async () => {
        if (!hardwareCameraId || intervalBusy) return;
        if (advancedLocked) {
            setUnlockError('Intervalometer requires the Advanced Capture Automation add-on.');
            toast.error('Intervalometer locked. Start a trial or upgrade to continue.');
            return;
        }
        setIntervalBusy(true);
        try {
            const ramp = intervalRampEnabled
                ? {
                    shutter_start: intervalRamp.shutter_start || null,
                    shutter_end: intervalRamp.shutter_end || null,
                    iso_start: intervalRamp.iso_start || null,
                    iso_end: intervalRamp.iso_end || null
                }
                : null;
            const payload = {
                interval_ms: Math.max(200, intervalMs),
                total_frames: intervalLimitEnabled ? Math.max(1, intervalFrames) : null,
                ramp,
                capture_target: null
            };
            const status = await startIntervalometer(hardwareCameraId, payload);
            setIntervalStatus(status);
            toast.success('Intervalometer started');
        } catch (err) {
            console.error(err);
            toast.error('Failed to start intervalometer');
        } finally {
            setIntervalBusy(false);
        }
    };

    const stopInterval = async () => {
        if (!hardwareCameraId || intervalBusy) return;
        if (advancedLocked) {
            setUnlockError('Intervalometer requires the Advanced Capture Automation add-on.');
            toast.error('Intervalometer locked. Start a trial or upgrade to continue.');
            return;
        }
        setIntervalBusy(true);
        try {
            const status = await stopIntervalometer(hardwareCameraId);
            setIntervalStatus(status);
            toast.success('Intervalometer stopped');
        } catch (err) {
            console.error(err);
            toast.error('Failed to stop intervalometer');
        } finally {
            setIntervalBusy(false);
        }
    };

    const calculateTotalShots = (): number => {
        switch (captureMode) {
            case 'single':
                return 1;
            case 'hdr':
                return hdrConfig.bracketCount;
            case 'focus-stack':
                return focusStackConfig.sliceCount;
            case 'hdr-focus':
                return hdrConfig.bracketCount * focusStackConfig.sliceCount;
            default:
                return 1;
        }
    };

    const getCaptureModeLabel = (): string => {
        switch (captureMode) {
            case 'single': return 'Single Shot';
            case 'hdr': return 'HDR Bracket';
            case 'focus-stack': return 'Focus Stack';
            case 'hdr-focus': return 'HDR + Focus Stack';
        }
    };

    const getCaptureModeDescription = (): string => {
        switch (captureMode) {
            case 'single': return 'Single exposure';
            case 'hdr': return `${hdrConfig.bracketCount} shots, ±${hdrConfig.evSpacing * (hdrConfig.bracketCount - 1) / 2} EV`;
            case 'focus-stack': return `${focusStackConfig.sliceCount} focus slices`;
            case 'hdr-focus': return `${focusStackConfig.sliceCount} slices × ${hdrConfig.bracketCount} brackets`;
        }
    };

    // ========================================================================
    // Focus Control
    // ========================================================================

    const moveFocus = async (direction: 'near' | 'far', amount: 'small' | 'large') => {
        const steps = direction === 'near' ? (amount === 'small' ? 5 : 20) : (amount === 'small' ? -5 : -20);
        if (!hardwareCameraId) {
            toast.error('No camera connected');
            return;
        }
        try {
            await driveFocus(hardwareCameraId, steps);
            toast.success(`Focus moved ${direction} (${Math.abs(steps)} steps)`);
        } catch (err) {
            console.error(err);
            toast.error('Focus drive failed');
        }
    };

    // ========================================================================
    // Helpers
    // ========================================================================

    const applyCameraSetting = async (field: 'iso' | 'shutter_speed' | 'aperture' | 'wb', value: string) => {
        if (!hardwareCameraId) {
            toast.error('No camera connected');
            return;
        }
        try {
            await setCameraConfig(hardwareCameraId, { [field]: value });
            setCamera(prev => {
                if (!prev) return prev;
                const settings = { ...prev.settings };
                if (field === 'iso') settings.iso = value;
                if (field === 'shutter_speed') settings.shutterSpeed = value;
                if (field === 'aperture') settings.aperture = value;
                if (field === 'wb') settings.whiteBalance = value;
                return { ...prev, settings };
            });
        } catch (err) {
            console.error(err);
            toast.error('Failed to update camera settings');
        }
    };

    const formatBytes = (bytes: number): string => {
        if (!bytes || Number.isNaN(bytes)) {
            return '—';
        }
        return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
    };

    const formatTimestamp = (value?: string | null): string => {
        if (!value) return '—';
        const parsed = new Date(value);
        if (Number.isNaN(parsed.getTime())) return '—';
        return parsed.toLocaleTimeString();
    };

    const connectedCameras = hardwareCameras.filter(item => item.connected);
    const activeProfile = connectedCameras.find(item => item.id === hardwareCameraId) ?? connectedCameras[0];
    const shutterOptions = activeProfile?.capabilities.shutter_speed_options ?? [];
    const isoOptions = activeProfile?.capabilities.iso_options ?? [];
    const apertureOptions = activeProfile?.capabilities.aperture_options ?? [];
    const wbOptions = activeProfile?.capabilities.wb_options ?? [];
    const advancedLocked = licenseStatus ? !(licenseStatus.license_valid && licenseStatus.features?.advanced_capture_automation) : false;
    const advancedBundle = licenseBundles.find(bundle => bundle.key === 'advanced_capture') ?? null;
    const advancedPriceLabel = formatBundlePrice(advancedBundle);
    const trialAvailable = licenseStatus?.trial_available ?? true;
    const bundleName = advancedBundle?.name ?? 'Advanced Capture Automation';

    const toggleSection = (section: keyof typeof expandedSections) => {
        setExpandedSections(prev => ({ ...prev, [section]: !prev[section] }));
    };

    const startAdvancedTrial = async () => {
        setUnlockBusy(true);
        setUnlockError(null);
        try {
            await createLicenseTrial({ duration_days: 14, bundles: ['advanced_capture'] });
            await refreshEntitlement();
            toast.success('Advanced Capture Automation trial activated.');
        } catch (err) {
            const message = err instanceof Error ? err.message : 'Trial activation failed';
            setUnlockError(message);
            toast.error('Trial unavailable. Purchase required.');
        } finally {
            setUnlockBusy(false);
        }
    };

    const openAdvancedPurchase = () => {
        const subject = encodeURIComponent(`TrueShot purchase: ${bundleName}`);
        const body = encodeURIComponent(`I want to buy the ${bundleName} lifetime add-on.`);
        window.open(`mailto:sales@trueshot.ai?subject=${subject}&body=${body}`, '_blank');
    };

    useEffect(() => {
        if (!hardwareCameraId || !intervalStatus?.active) {
            return;
        }
        let cancelled = false;
        const poll = async () => {
            if (cancelled) return;
            try {
                const status = await getIntervalometerStatus(hardwareCameraId);
                if (!cancelled) {
                    setIntervalStatus(status);
                }
            } catch {
                if (!cancelled) {
                    setIntervalStatus(null);
                }
            }
        };
        poll();
        const timer = setInterval(poll, 2000);
        return () => {
            cancelled = true;
            clearInterval(timer);
        };
    }, [hardwareCameraId, intervalStatus?.active]);

    // ========================================================================
    // Render
    // ========================================================================

    if (loading) {
        return (
            <div className="camera-control-pro camera-control-pro--loading">
                <ThemeToggleFloating />
                <Loader2 className="spin" size={48} />
                <p>Connecting to camera...</p>
            </div>
        );
    }

    if (!camera) {
        return (
            <div className="camera-control-pro camera-control-pro--disconnected">
                <ThemeToggleFloating />
                <Camera size={48} />
                <h2>No Camera Connected</h2>
                <p>Connect a DSLR or mirrorless camera via USB</p>
                <button onClick={loadCamera}>
                    <RefreshCw size={16} /> Retry
                </button>
            </div>
        );
    }

    return (
        <div className="camera-control-pro">
            <ThemeToggleFloating />
            {/* Header */}
            <header className="camera-control-pro__header">
                <div className="camera-control-pro__camera-info">
                    <Camera size={24} />
                    <div>
                        <h2>{camera.name}</h2>
                        <span className="camera-control-pro__status">
                            <span className="camera-control-pro__status-dot connected" />
                            Connected
                        </span>
                    </div>
                </div>
                {connectedCameras.length > 1 && (
                    <select
                        className="camera-control-pro__camera-select"
                        value={hardwareCameraId ?? ''}
                        onChange={(e) => selectCamera(e.target.value)}
                    >
                        {connectedCameras.map(item => (
                            <option key={item.id} value={item.id}>
                                {item.nickname ?? item.name}
                            </option>
                        ))}
                    </select>
                )}
                <div className="camera-control-pro__camera-meta">
                    <span title="Battery">🔋 {camera.battery != null ? `${camera.battery}%` : '—'}</span>
                    <span title="Storage">
                        💾 {formatBytes(camera.storageUsed)} / {formatBytes(camera.storageTotal)}
                    </span>
                </div>
            </header>

            {/* Current Settings */}
            <div className="camera-control-pro__current-settings">
                <div className="camera-control-pro__setting">
                    <span className="camera-control-pro__setting-label">ISO</span>
                    <span className="camera-control-pro__setting-value">{camera.settings.iso}</span>
                </div>
                <div className="camera-control-pro__setting">
                    <span className="camera-control-pro__setting-label">Aperture</span>
                    <span className="camera-control-pro__setting-value">{camera.settings.aperture}</span>
                </div>
                <div className="camera-control-pro__setting">
                    <span className="camera-control-pro__setting-label">Shutter</span>
                    <span className="camera-control-pro__setting-value">{camera.settings.shutterSpeed}</span>
                </div>
                <div className="camera-control-pro__setting">
                    <span className="camera-control-pro__setting-label">WB</span>
                    <span className="camera-control-pro__setting-value">{camera.settings.whiteBalance}</span>
                </div>
            </div>

            <div className="camera-control-pro__section">
                <button
                    className="camera-control-pro__section-header"
                    onClick={() => toggleSection('settings')}
                >
                    <Gauge size={16} />
                    <span>Camera Settings</span>
                    {expandedSections.settings ? <ChevronUp size={16} /> : <ChevronDown size={16} />}
                </button>
                {expandedSections.settings && (
                    <div className="camera-control-pro__section-body">
                        <div className="camera-control-pro__field">
                            <label>ISO</label>
                            <select
                                value={camera.settings.iso}
                                onChange={(e) => applyCameraSetting('iso', e.target.value)}
                            >
                                {(isoOptions.length ? isoOptions : [camera.settings.iso]).map(value => (
                                    <option key={value} value={value}>{value}</option>
                                ))}
                            </select>
                        </div>
                        <div className="camera-control-pro__field">
                            <label>Shutter</label>
                            <select
                                value={camera.settings.shutterSpeed}
                                onChange={(e) => applyCameraSetting('shutter_speed', e.target.value)}
                            >
                                {(shutterOptions.length ? shutterOptions : [camera.settings.shutterSpeed]).map(value => (
                                    <option key={value} value={value}>{value}</option>
                                ))}
                            </select>
                        </div>
                        <div className="camera-control-pro__field">
                            <label>Aperture</label>
                            <select
                                value={camera.settings.aperture}
                                onChange={(e) => applyCameraSetting('aperture', e.target.value)}
                            >
                                {(apertureOptions.length ? apertureOptions : [camera.settings.aperture]).map(value => (
                                    <option key={value} value={value}>{value}</option>
                                ))}
                            </select>
                        </div>
                        <div className="camera-control-pro__field">
                            <label>White Balance</label>
                            <select
                                value={camera.settings.whiteBalance}
                                onChange={(e) => applyCameraSetting('wb', e.target.value)}
                                disabled={wbOptions.length === 0}
                            >
                                {(wbOptions.length ? wbOptions : [camera.settings.whiteBalance]).map(value => (
                                    <option key={value} value={value}>{value}</option>
                                ))}
                            </select>
                        </div>
                    </div>
                )}
            </div>

            {/* Capture Mode Selection */}
            <div className="camera-control-pro__mode-selection">
                <span className="camera-control-pro__section-label">
                    <Camera size={16} /> Capture Mode
                </span>
                <div className="camera-control-pro__mode-buttons">
                    <button
                        className={captureMode === 'single' ? 'active' : ''}
                        onClick={() => setCaptureMode('single')}
                        disabled={isCapturing}
                    >
                        <Image size={16} />
                        Single
                    </button>
                    <button
                        className={captureMode === 'hdr' ? 'active' : ''}
                        onClick={() => setCaptureMode('hdr')}
                        disabled={isCapturing}
                    >
                        <Sun size={16} />
                        HDR
                    </button>
                    <button
                        className={captureMode === 'focus-stack' ? 'active' : ''}
                        onClick={() => setCaptureMode('focus-stack')}
                        disabled={isCapturing}
                    >
                        <Layers size={16} />
                        Focus Stack
                    </button>
                    <button
                        className={captureMode === 'hdr-focus' ? 'active' : ''}
                        onClick={() => setCaptureMode('hdr-focus')}
                        disabled={isCapturing}
                    >
                        <Zap size={16} />
                        HDR + FS
                    </button>
                </div>
            </div>

            {advancedLocked && (
                <FeatureUnlockPanel
                    title="Advanced Capture Automation"
                    subtitle="Unlock HDR bracketing, focus stacking, and intervalometer automation for studio-grade capture workflows."
                    bundleName={bundleName}
                    priceLabel={advancedPriceLabel}
                    capabilities={[
                        'HDR bracketing with auto-merge',
                        'Focus stacking automation',
                        'HDR + focus stack combined workflows',
                        'Intervalometer and ramped exposure control',
                    ]}
                    trialAvailable={trialAvailable}
                    onStartTrial={startAdvancedTrial}
                    onBuy={openAdvancedPurchase}
                    busy={unlockBusy}
                    errorMessage={unlockError}
                />
            )}

            {/* HDR Settings (when enabled) */}
            {(captureMode === 'hdr' || captureMode === 'hdr-focus') && (
                <div className="camera-control-pro__section">
                    <button
                        className="camera-control-pro__section-header"
                        onClick={() => toggleSection('hdr')}
                    >
                        <Sun size={16} />
                        <span>HDR Settings</span>
                        {expandedSections.hdr ? <ChevronUp size={16} /> : <ChevronDown size={16} />}
                    </button>
                    {expandedSections.hdr && (
                        <div className="camera-control-pro__section-body">
                            <div className="camera-control-pro__field">
                                <label>Bracket Steps</label>
                                <div className="camera-control-pro__button-group">
                                    {([3, 5, 7, 9] as const).map(count => (
                                        <button
                                            key={count}
                                            className={hdrConfig.bracketCount === count ? 'active' : ''}
                                            onClick={() => setHdrConfig(prev => ({ ...prev, bracketCount: count }))}
                                            disabled={isCapturing || advancedLocked}
                                        >
                                            {count}
                                        </button>
                                    ))}
                                </div>
                            </div>

                            <div className="camera-control-pro__field">
                                <label>EV Spacing</label>
                                <div className="camera-control-pro__button-group">
                                    {([1, 2, 3] as const).map(ev => (
                                        <button
                                            key={ev}
                                            className={hdrConfig.evSpacing === ev ? 'active' : ''}
                                            onClick={() => setHdrConfig(prev => ({ ...prev, evSpacing: ev }))}
                                            disabled={isCapturing || advancedLocked}
                                        >
                                            {ev} EV
                                        </button>
                                    ))}
                                </div>
                            </div>

                            <div className="camera-control-pro__field camera-control-pro__field--inline">
                                <label>
                                    <input
                                        type="checkbox"
                                        checked={hdrConfig.autoMerge}
                                        onChange={e => setHdrConfig(prev => ({ ...prev, autoMerge: e.target.checked }))}
                                        disabled={isCapturing || advancedLocked}
                                    />
                                    Auto-merge HDR
                                </label>
                            </div>

                            {hdrConfig.autoMerge && (
                                <div className="camera-control-pro__field">
                                    <label>Merge Algorithm</label>
                                    <select
                                        value={hdrConfig.mergeAlgorithm}
                                        onChange={e => setHdrConfig(prev => ({
                                            ...prev,
                                            mergeAlgorithm: e.target.value as HDRConfig['mergeAlgorithm']
                                        }))}
                                        disabled={isCapturing || advancedLocked}
                                    >
                                        <option value="mertens">Mertens Fusion (no tone map)</option>
                                        <option value="debevec">Debevec (full HDR)</option>
                                        <option value="robertson">Robertson</option>
                                    </select>
                                </div>
                            )}
                        </div>
                    )}
                </div>
            )}

            {/* Focus Stack Settings (when enabled) */}
            {(captureMode === 'focus-stack' || captureMode === 'hdr-focus') && (
                <div className="camera-control-pro__section">
                    <button
                        className="camera-control-pro__section-header"
                        onClick={() => toggleSection('focusStack')}
                    >
                        <Layers size={16} />
                        <span>Focus Stack Settings</span>
                        {expandedSections.focusStack ? <ChevronUp size={16} /> : <ChevronDown size={16} />}
                    </button>
                    {expandedSections.focusStack && (
                        <div className="camera-control-pro__section-body">
                            <div className="camera-control-pro__field">
                                <label>Stack Mode</label>
                                <select
                                    value={focusStackConfig.mode}
                                    onChange={e => setFocusStackConfig(prev => ({
                                        ...prev,
                                        mode: e.target.value as FocusStackConfig['mode']
                                    }))}
                                    disabled={isCapturing || advancedLocked}
                                >
                                    <option value="front-to-back">Front to Back</option>
                                    <option value="back-to-front">Back to Front</option>
                                    <option value="center-out">Center Out</option>
                                </select>
                            </div>

                            <div className="camera-control-pro__field">
                                <label>Number of Slices: {focusStackConfig.sliceCount}</label>
                                <input
                                    type="range"
                                    min="5"
                                    max="50"
                                    value={focusStackConfig.sliceCount}
                                    onChange={e => setFocusStackConfig(prev => ({
                                        ...prev,
                                        sliceCount: parseInt(e.target.value)
                                    }))}
                                    disabled={isCapturing || advancedLocked}
                                />
                            </div>

                            <div className="camera-control-pro__field">
                                <label>Focus Step Size</label>
                                <select
                                    value={focusStackConfig.stepSize}
                                    onChange={e => setFocusStackConfig(prev => ({
                                        ...prev,
                                        stepSize: e.target.value as FocusStackConfig['stepSize']
                                    }))}
                                    disabled={isCapturing || advancedLocked}
                                >
                                    <option value="auto">Auto (recommended)</option>
                                    <option value="small">Small (macro)</option>
                                    <option value="medium">Medium</option>
                                    <option value="large">Large</option>
                                    <option value="custom">Custom</option>
                                </select>
                            </div>

                            {focusStackConfig.stepSize === 'custom' && (
                                <div className="camera-control-pro__field">
                                    <label>Custom Step Size</label>
                                    <input
                                        type="number"
                                        min="1"
                                        max="200"
                                    value={focusStackConfig.customStepSize ?? 12}
                                    onChange={e => setFocusStackConfig(prev => ({
                                        ...prev,
                                        customStepSize: parseInt(e.target.value || '0')
                                    }))}
                                    disabled={isCapturing || advancedLocked}
                                />
                            </div>
                            )}

                            <div className="camera-control-pro__field camera-control-pro__field--inline">
                                <label>
                                    <input
                                        type="checkbox"
                                        checked={focusStackConfig.autoStack}
                                        onChange={e => setFocusStackConfig(prev => ({ ...prev, autoStack: e.target.checked }))}
                                        disabled={isCapturing || advancedLocked}
                                    />
                                    Auto-stack images
                                </label>
                            </div>

                            {focusStackConfig.autoStack && (
                                <div className="camera-control-pro__field">
                                    <label>Stack Algorithm</label>
                                    <select
                                        value={focusStackConfig.stackAlgorithm}
                                        onChange={e => setFocusStackConfig(prev => ({
                                            ...prev,
                                            stackAlgorithm: e.target.value as FocusStackConfig['stackAlgorithm']
                                        }))}
                                        disabled={isCapturing || advancedLocked}
                                    >
                                        <option value="weighted-focus">Weighted Focus</option>
                                        <option value="laplacian-pyramid">Laplacian Pyramid</option>
                                        <option value="depth-map">Depth Map</option>
                                    </select>
                                </div>
                            )}

                            <div className="camera-control-pro__focus-controls">
                                <span>Manual Focus Control</span>
                                <div className="camera-control-pro__focus-buttons">
                                    <button onClick={() => moveFocus('far', 'large')} disabled={isCapturing}>
                                        <Focus size={14} /> ⟪ Far
                                    </button>
                                    <button onClick={() => moveFocus('far', 'small')} disabled={isCapturing}>
                                        ⟨
                                    </button>
                                    <button onClick={() => moveFocus('near', 'small')} disabled={isCapturing}>
                                        ⟩
                                    </button>
                                    <button onClick={() => moveFocus('near', 'large')} disabled={isCapturing}>
                                        Near ⟫ <Target size={14} />
                                    </button>
                                </div>
                            </div>
                        </div>
                    )}
                </div>
            )}

            {/* Save Location */}
            <div className="camera-control-pro__section">
                <button
                    className="camera-control-pro__section-header"
                    onClick={() => toggleSection('saveLocation')}
                >
                    <HardDrive size={16} />
                    <span>Save Location</span>
                    {expandedSections.saveLocation ? <ChevronUp size={16} /> : <ChevronDown size={16} />}
                </button>
                {expandedSections.saveLocation && (
                    <div className="camera-control-pro__section-body">
                        <div className="camera-control-pro__save-options">
                            <label className={saveLocation === 'camera' ? 'active' : ''}>
                                <input
                                    type="radio"
                                    name="saveLocation"
                                    checked={saveLocation === 'camera'}
                                    onChange={() => setSaveLocation('camera')}
                                    disabled={isCapturing}
                                />
                                <Camera size={16} />
                                <span>Camera SD Card</span>
                            </label>
                            <label className={saveLocation === 'computer' ? 'active' : ''}>
                                <input
                                    type="radio"
                                    name="saveLocation"
                                    checked={saveLocation === 'computer'}
                                    onChange={() => setSaveLocation('computer')}
                                    disabled={isCapturing}
                                />
                                <HardDrive size={16} />
                                <span>Computer</span>
                            </label>
                            <label className={saveLocation === 'both' ? 'active' : ''}>
                                <input
                                    type="radio"
                                    name="saveLocation"
                                    checked={saveLocation === 'both'}
                                    onChange={() => setSaveLocation('both')}
                                    disabled={isCapturing}
                                />
                                <Check size={16} />
                                <span>Both</span>
                            </label>
                        </div>

                        {(saveLocation === 'computer' || saveLocation === 'both') && (
                            <div className="camera-control-pro__path">
                                <input
                                    type="text"
                                    value={savePath}
                                    onChange={e => setSavePath(e.target.value)}
                                    disabled={isCapturing}
                                />
                                <button disabled={isCapturing}>
                                    <Folder size={16} />
                                </button>
                            </div>
                        )}
                    </div>
                )}
            </div>

            {/* Intervalometer */}
            <div className="camera-control-pro__section">
                <button
                    className="camera-control-pro__section-header"
                    onClick={() => toggleSection('intervalometer')}
                >
                    <Gauge size={16} />
                    <span>Intervalometer / Timelapse</span>
                    {expandedSections.intervalometer ? <ChevronUp size={16} /> : <ChevronDown size={16} />}
                </button>
                {expandedSections.intervalometer && (
                    <div className="camera-control-pro__section-body">
                        <div className="camera-control-pro__field">
                            <label>Interval (ms)</label>
                            <input
                                type="number"
                                min="200"
                                value={intervalMs}
                                onChange={e => setIntervalMs(parseInt(e.target.value || '0'))}
                                disabled={intervalBusy || advancedLocked}
                            />
                        </div>
                        <div className="camera-control-pro__field camera-control-pro__field--inline">
                            <label>
                                <input
                                    type="checkbox"
                                    checked={intervalLimitEnabled}
                                    onChange={e => setIntervalLimitEnabled(e.target.checked)}
                                    disabled={intervalBusy || advancedLocked}
                                />
                                Limit frames
                            </label>
                        </div>
                        {intervalLimitEnabled && (
                            <div className="camera-control-pro__field">
                                <label>Total Frames</label>
                                <input
                                    type="number"
                                    min="1"
                                    value={intervalFrames}
                                    onChange={e => setIntervalFrames(parseInt(e.target.value || '0'))}
                                    disabled={intervalBusy || advancedLocked}
                                />
                            </div>
                        )}
                        <div className="camera-control-pro__field camera-control-pro__field--inline">
                            <label>
                                <input
                                    type="checkbox"
                                    checked={intervalRampEnabled}
                                    onChange={e => setIntervalRampEnabled(e.target.checked)}
                                    disabled={intervalBusy || advancedLocked}
                                />
                                Enable exposure ramping
                            </label>
                        </div>
                        {intervalRampEnabled && (
                            <>
                                <div className="camera-control-pro__field">
                                    <label>Shutter Ramp</label>
                                    <div className="camera-control-pro__interval-row">
                                        <select
                                            value={intervalRamp.shutter_start ?? ''}
                                            onChange={e => setIntervalRamp(prev => ({
                                                ...prev,
                                                shutter_start: e.target.value || null
                                            }))}
                                            disabled={intervalBusy || advancedLocked || shutterOptions.length === 0}
                                        >
                                            <option value="">Start</option>
                                            {shutterOptions.map(option => (
                                                <option key={option} value={option}>{option}</option>
                                            ))}
                                        </select>
                                        <select
                                            value={intervalRamp.shutter_end ?? ''}
                                            onChange={e => setIntervalRamp(prev => ({
                                                ...prev,
                                                shutter_end: e.target.value || null
                                            }))}
                                            disabled={intervalBusy || advancedLocked || shutterOptions.length === 0}
                                        >
                                            <option value="">End</option>
                                            {shutterOptions.map(option => (
                                                <option key={option} value={option}>{option}</option>
                                            ))}
                                        </select>
                                    </div>
                                </div>
                                <div className="camera-control-pro__field">
                                    <label>ISO Ramp</label>
                                    <div className="camera-control-pro__interval-row">
                                        <select
                                            value={intervalRamp.iso_start ?? ''}
                                            onChange={e => setIntervalRamp(prev => ({
                                                ...prev,
                                                iso_start: e.target.value || null
                                            }))}
                                            disabled={intervalBusy || advancedLocked || isoOptions.length === 0}
                                        >
                                            <option value="">Start</option>
                                            {isoOptions.map(option => (
                                                <option key={option} value={option}>{option}</option>
                                            ))}
                                        </select>
                                        <select
                                            value={intervalRamp.iso_end ?? ''}
                                            onChange={e => setIntervalRamp(prev => ({
                                                ...prev,
                                                iso_end: e.target.value || null
                                            }))}
                                            disabled={intervalBusy || advancedLocked || isoOptions.length === 0}
                                        >
                                            <option value="">End</option>
                                            {isoOptions.map(option => (
                                                <option key={option} value={option}>{option}</option>
                                            ))}
                                        </select>
                                    </div>
                                </div>
                            </>
                        )}
                        <div className="camera-control-pro__interval-actions">
                            <button
                                onClick={startInterval}
                                disabled={intervalBusy || advancedLocked || !hardwareCameraId || intervalStatus?.active}
                            >
                                <Play size={16} /> Start Intervalometer
                            </button>
                            <button
                                onClick={stopInterval}
                                disabled={intervalBusy || advancedLocked || !intervalStatus?.active}
                            >
                                <Square size={16} /> Stop
                            </button>
                        </div>
                        {intervalStatus && (
                            <div className="camera-control-pro__interval-status">
                                <div>
                                    <span>Status</span>
                                    <strong>{intervalStatus.active ? 'Running' : 'Idle'}</strong>
                                </div>
                                <div>
                                    <span>Captured</span>
                                    <strong>
                                        {intervalStatus.captured_frames} / {intervalStatus.total_frames ?? '∞'}
                                    </strong>
                                </div>
                                <div>
                                    <span>Next Capture</span>
                                    <strong>{formatTimestamp(intervalStatus.next_capture_at)}</strong>
                                </div>
                                <div>
                                    <span>Last Error</span>
                                    <strong>{intervalStatus.last_error ?? '—'}</strong>
                                </div>
                            </div>
                        )}
                    </div>
                )}
            </div>

            {/* Capture Queue */}
            {captureQueue.length > 0 && (
                <div className="camera-control-pro__queue">
                    <span className="camera-control-pro__section-label">
                        <Gauge size={16} /> Capture Queue
                    </span>
                    {captureQueue.slice(-3).map(item => (
                        <div key={item.id} className={`camera-control-pro__queue-item camera-control-pro__queue-item--${item.status}`}>
                            <div className="camera-control-pro__queue-info">
                                <span className="camera-control-pro__queue-mode">{getCaptureModeLabel()}</span>
                                <span className="camera-control-pro__queue-desc">{item.description}</span>
                            </div>
                            <div className="camera-control-pro__queue-progress">
                                {item.status === 'capturing' && (
                                    <span>Shot {item.currentShot}/{item.totalShots}</span>
                                )}
                                {item.status === 'processing' && (
                                    <span><Loader2 size={14} className="spin" /> Processing...</span>
                                )}
                                {item.status === 'complete' && (
                                    <span><Check size={14} /> Complete</span>
                                )}
                                {item.status === 'error' && (
                                    <span><X size={14} /> Cancelled</span>
                                )}
                            </div>
                            <div className="camera-control-pro__queue-bar">
                                <div
                                    className="camera-control-pro__queue-fill"
                                    style={{ width: `${item.progress}%` }}
                                />
                            </div>
                        </div>
                    ))}
                </div>
            )}

            {/* Main Capture Button */}
            <div className="camera-control-pro__capture">
                {!isCapturing ? (
                    <button
                        className="camera-control-pro__capture-btn"
                        onClick={startCapture}
                    >
                        <Play size={32} />
                        <span>CAPTURE</span>
                        <span className="camera-control-pro__capture-mode">
                            {getCaptureModeLabel()} • {calculateTotalShots()} shots
                        </span>
                    </button>
                ) : (
                    <button
                        className="camera-control-pro__capture-btn camera-control-pro__capture-btn--cancel"
                        onClick={cancelCapture}
                    >
                        <Square size={32} />
                        <span>CANCEL</span>
                    </button>
                )}
            </div>
        </div>
    );
}
