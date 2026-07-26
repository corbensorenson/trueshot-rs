/**
 * WebXR Scanner Component
 * 
 * Provides an immersive VR/AR interface for 3D scanning.
 * Integrates with TrueShot's reconstruction pipeline.
 */

import { useState, useEffect, useCallback, useRef } from 'react';
import {
    Box,
    Headphones,
    Camera,
    Grid3x3,
    Home,
    Play,
    Square,
    Check,
    AlertTriangle,
    Loader2,
    X,
    Eye,
} from 'lucide-react';
import {
    useWebXRScanning,
    ScanMode,
    CapturedFrame,
    type XRSessionState
} from '../utils/webxr';
import { createLicenseTrial, getLicenseBundles, getLicenseStatus, getLicenseTiers, startXrSession, completeXrSession, type LicenseBundleInfo, type LicenseStatusResponse, type LicenseTierInfo } from '../api/client';
import { FeatureUnlockPanel } from './FeatureUnlockPanel';
import toast from 'react-hot-toast';

const formatBundlePrice = (bundle?: LicenseBundleInfo | null) => {
    if (!bundle) return 'Pricing unavailable';
    if (!bundle.price_usd) return 'Contact sales';
    const billing = bundle.billing ? ` ${bundle.billing}` : '';
    return `$${bundle.price_usd}${billing}`;
};

const formatTierPrice = (tier?: LicenseTierInfo | null) => {
    if (!tier) return 'Contact sales';
    if (!tier.price_usd) return 'Contact sales';
    const billing = tier.billing ? ` ${tier.billing}` : '';
    return `$${tier.price_usd}${billing}`;
};

interface XRScannerProps {
    isOpen: boolean;
    onClose: () => void;
    onScanComplete: (frames: CapturedFrame[]) => void;
}

export function XRScanner({ isOpen, onClose, onScanComplete }: XRScannerProps) {
    const {
        state,
        capabilities,
        session,
        error,
        isSupported,
        startScan,
        startCapture,
        stopCapture,
        endSession,
        onFrame,
    } = useWebXRScanning();

    const [selectedMode, setSelectedMode] = useState<ScanMode | null>(null);
    const [showModeSelect, setShowModeSelect] = useState(true);
    const [frameCount, setFrameCount] = useState(0);
    const [licenseStatus, setLicenseStatus] = useState<LicenseStatusResponse | null>(null);
    const [licenseBundles, setLicenseBundles] = useState<LicenseBundleInfo[]>([]);
    const [licenseTiers, setLicenseTiers] = useState<LicenseTierInfo[]>([]);
    const [unlockBusy, setUnlockBusy] = useState(false);
    const [unlockError, setUnlockError] = useState<string | null>(null);
    const [sessionId, setSessionId] = useState<string | null>(null);
    const sessionStartedAtRef = useRef<number | null>(null);

    const refreshLicensing = useCallback(async () => {
        try {
            const [status, bundles, tiers] = await Promise.all([
                getLicenseStatus(),
                getLicenseBundles(),
                getLicenseTiers(),
            ]);
            setLicenseStatus(status);
            setLicenseBundles(bundles);
            setLicenseTiers(tiers);
        } catch {
            setLicenseStatus(null);
            setLicenseBundles([]);
            setLicenseTiers([]);
        }
    }, []);

    useEffect(() => {
        if (!isOpen) return;
        refreshLicensing();
    }, [isOpen, refreshLicensing]);

    useEffect(() => {
        if (!isOpen) {
            setSessionId(null);
            sessionStartedAtRef.current = null;
            setSelectedMode(null);
            setShowModeSelect(true);
            setFrameCount(0);
        }
    }, [isOpen]);

    const xrLocked = licenseStatus ? !(licenseStatus.license_valid && licenseStatus.features?.webxr_scanning) : false;
    const trialAvailable = licenseStatus?.trial_available ?? true;
    const xrBundle = licenseBundles.find(bundle => bundle.key === 'xr_scanning') ?? null;
    const coreTier = licenseTiers.find(tier => tier.key === 'hobby') ?? licenseTiers[0] ?? null;
    const xrBundleName = xrBundle?.name ?? coreTier?.name ?? 'Core License';
    const xrPriceLabel = xrBundle ? formatBundlePrice(xrBundle) : formatTierPrice(coreTier);

    const startXrTrial = async () => {
        setUnlockBusy(true);
        setUnlockError(null);
        try {
            await createLicenseTrial({ duration_days: 14, bundles: xrBundle ? [xrBundle.key] : [] });
            await refreshLicensing();
            toast.success('XR Scanner trial activated.');
        } catch (err) {
            const message = err instanceof Error ? err.message : 'Trial activation failed';
            setUnlockError(message);
            toast.error('Trial unavailable. Purchase required.');
        } finally {
            setUnlockBusy(false);
        }
    };

    const openXrPurchase = () => {
        const subject = encodeURIComponent(`TrueShot purchase: ${xrBundleName}`);
        const body = encodeURIComponent(`I want to buy the ${xrBundleName} lifetime license for XR scanning.`);
        window.open(`mailto:sales@trueshot.ai?subject=${subject}&body=${body}`, '_blank');
    };

    // Subscribe to frames
    useEffect(() => {
        if (state === 'scanning') {
            const unsubscribe = onFrame(() => {
                setFrameCount(prev => prev + 1);
            });
            return unsubscribe;
        }
    }, [state, onFrame]);

    // Handle scan mode selection
    const handleModeSelect = async (mode: ScanMode) => {
        setSelectedMode(mode);
        setShowModeSelect(false);

        try {
            try {
                const response = await startXrSession({ mode });
                setSessionId(response.session_id);
                sessionStartedAtRef.current = Date.now();
            } catch (err) {
                const message = err instanceof Error ? err.message : 'Failed to log XR session';
                toast.error(`XR session logging failed: ${message}`);
            }
            await startScan(mode);
            toast.success(`${mode} scanning mode activated`);
        } catch (err) {
            const message = err instanceof Error ? err.message : 'Failed to start XR';
            toast.error(`Failed to start XR: ${message}`);
            setShowModeSelect(true);
        }
    };

    // Handle capture start
    const handleStartCapture = () => {
        startCapture();
        setFrameCount(0);
        toast.success('Capture started - move around to scan');
    };

    // Handle capture complete
    const handleComplete = async () => {
        const frames = await stopCapture();
        toast.success(`Captured ${frames.length} frames`);
        onScanComplete(frames);
        if (sessionId) {
            try {
                const durationSeconds = sessionStartedAtRef.current
                    ? (Date.now() - sessionStartedAtRef.current) / 1000
                    : undefined;
                await completeXrSession({
                    session_id: sessionId,
                    mode: selectedMode ?? 'object',
                    frame_count: frames.length,
                    duration_seconds: durationSeconds,
                });
            } catch (err) {
                const message = err instanceof Error ? err.message : 'Failed to log XR session';
                toast.error(`XR session completion failed: ${message}`);
            }
        }
        await endSession();
        onClose();
    };

    // Handle cancel
    const handleCancel = async () => {
        await endSession();
        onClose();
    };

    if (!isOpen) return null;

    return (
        <div className="fixed inset-0 bg-black/90 flex flex-col items-center justify-center z-50">
            {/* Header */}
            <div className="absolute top-0 left-0 right-0 p-4 flex justify-between items-center">
                <h1 className="text-white text-xl font-medium flex items-center gap-2">
                    <Headphones className="w-6 h-6" />
                    TrueShot XR Scanner
                </h1>
                <button
                    onClick={handleCancel}
                    className="p-2 rounded-lg bg-white/10 hover:bg-white/20 text-white transition-colors"
                >
                    <X className="w-5 h-5" />
                </button>
            </div>

            {xrLocked && (
                <div className="max-w-3xl w-full px-6">
                    <FeatureUnlockPanel
                        title="XR Scanner"
                        subtitle="Unlock immersive VR/AR scanning modes with coverage feedback and TrueShot reconstruction."
                        bundleName={xrBundleName}
                        priceLabel={xrPriceLabel}
                        capabilities={[
                            'Immersive object/portion/room scans',
                            'Live coverage + quality feedback',
                            'Capture frames for 3D reconstruction',
                            'Hand + depth sensing integration',
                        ]}
                        trialAvailable={trialAvailable}
                        onStartTrial={startXrTrial}
                        onBuy={openXrPurchase}
                        busy={unlockBusy}
                        errorMessage={unlockError}
                    />
                    <div className="flex justify-center mt-6">
                        <button
                            onClick={handleCancel}
                            className="px-4 py-2 bg-white/10 hover:bg-white/20 rounded-lg text-white transition-colors"
                        >
                            Close
                        </button>
                    </div>
                </div>
            )}

            {!xrLocked && error && (
                <div className="bg-red-500/20 border border-red-500 rounded-lg p-4 max-w-md">
                    <div className="flex items-center gap-2 text-red-400 mb-2">
                        <AlertTriangle className="w-5 h-5" />
                        <span className="font-medium">WebXR Error</span>
                    </div>
                    <p className="text-red-300 text-sm">{error}</p>
                    <button
                        onClick={onClose}
                        className="mt-4 px-4 py-2 bg-red-500 rounded-lg text-white"
                    >
                        Close
                    </button>
                </div>
            )}

            {/* Not Supported State */}
            {!xrLocked && !isSupported && !error && (
                <div className="bg-yellow-500/20 border border-yellow-500 rounded-lg p-6 max-w-md text-center">
                    <AlertTriangle className="w-12 h-12 text-yellow-400 mx-auto mb-4" />
                    <h2 className="text-white text-lg font-medium mb-2">WebXR Not Available</h2>
                    <p className="text-gray-300 text-sm mb-4">
                        VR/AR scanning requires a WebXR-compatible browser and device.
                        Try opening this page on a VR headset browser (Quest, Vision Pro, etc.)
                    </p>
                    <button
                        onClick={onClose}
                        className="px-4 py-2 bg-yellow-500 rounded-lg text-black font-medium"
                    >
                        Got it
                    </button>
                </div>
            )}

            {/* Mode Selection */}
            {!xrLocked && isSupported && showModeSelect && !error && (
                <div className="max-w-2xl w-full px-4">
                    <h2 className="text-white text-2xl font-light text-center mb-8">
                        Select Scan Mode
                    </h2>

                    <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                        {/* Object Mode */}
                        <ModeCard
                            icon={<Box className="w-10 h-10" />}
                            title="Object"
                            description="Scan a single object by walking around it"
                            available={capabilities?.immersiveVR || false}
                            onClick={() => handleModeSelect('object')}
                        />

                        {/* Portion Mode */}
                        <ModeCard
                            icon={<Grid3x3 className="w-10 h-10" />}
                            title="Portion"
                            description="Scan a table, desk, or section of a room"
                            available={capabilities?.immersiveVR || false}
                            onClick={() => handleModeSelect('portion')}
                        />

                        {/* Room Mode */}
                        <ModeCard
                            icon={<Home className="w-10 h-10" />}
                            title="Room"
                            description="Full room walkthrough with floor plan"
                            available={capabilities?.immersiveAR || false}
                            onClick={() => handleModeSelect('room')}
                        />
                    </div>

                    <p className="text-gray-500 text-sm text-center mt-6">
                        {capabilities?.handTracking ? '✓ Hand tracking available' : ''}
                        {capabilities?.depthSensing ? ' • ✓ Depth sensing available' : ''}
                    </p>
                </div>
            )}

            {/* Active Session UI */}
            {!xrLocked && !showModeSelect && session && (
                <div className="flex flex-col items-center gap-8">
                    {/* State indicator */}
                    <StateIndicator state={state} />

                    {/* Progress */}
                    {session.progress && (
                        <div className="text-center">
                            <div className="text-white text-4xl font-light mb-2">
                                {Math.round(session.progress.coveragePercent)}%
                            </div>
                            <div className="text-gray-400">
                                Coverage • {frameCount} frames
                            </div>
                            <div className="mt-2">
                                <QualityBadge quality={session.progress.quality} />
                            </div>
                        </div>
                    )}

                    {/* Controls */}
                    <div className="flex gap-4">
                        {state === 'active' && (
                            <button
                                onClick={handleStartCapture}
                                className="flex items-center gap-2 px-6 py-3 bg-green-500 hover:bg-green-600 rounded-xl text-white font-medium transition-colors"
                            >
                                <Play className="w-5 h-5" />
                                Start Capture
                            </button>
                        )}

                        {state === 'scanning' && (
                            <button
                                onClick={handleComplete}
                                className="flex items-center gap-2 px-6 py-3 bg-blue-500 hover:bg-blue-600 rounded-xl text-white font-medium transition-colors"
                            >
                                <Check className="w-5 h-5" />
                                Complete Scan
                            </button>
                        )}

                        <button
                            onClick={handleCancel}
                            className="flex items-center gap-2 px-6 py-3 bg-white/10 hover:bg-white/20 rounded-xl text-white transition-colors"
                        >
                            <Square className="w-5 h-5" />
                            Cancel
                        </button>
                    </div>

                    {/* Tips */}
                    <div className="text-gray-500 text-sm text-center max-w-md">
                        {selectedMode === 'object' &&
                            'Walk slowly around the object. Vary your height for better coverage.'}
                        {selectedMode === 'portion' &&
                            'Move around the area you want to capture. Include the edges clearly.'}
                        {selectedMode === 'room' &&
                            'Walk through the entire room. Look at walls, floor, and ceiling.'}
                    </div>
                </div>
            )}

            {/* Processing State */}
            {!xrLocked && state === 'processing' && (
                <div className="flex flex-col items-center gap-4">
                    <Loader2 className="w-12 h-12 text-blue-400 animate-spin" />
                    <span className="text-white">Processing scan data...</span>
                </div>
            )}
        </div>
    );
}

// ============================================================================
// Sub-components
// ============================================================================

function ModeCard({
    icon,
    title,
    description,
    available,
    onClick
}: {
    icon: React.ReactNode;
    title: string;
    description: string;
    available: boolean;
    onClick: () => void;
}) {
    return (
        <button
            onClick={onClick}
            disabled={!available}
            className={`
        p-6 rounded-2xl text-left transition-all
        ${available
                    ? 'bg-white/10 hover:bg-white/20 hover:scale-105 cursor-pointer'
                    : 'bg-white/5 opacity-50 cursor-not-allowed'}
      `}
        >
            <div className={`${available ? 'text-blue-400' : 'text-gray-600'} mb-4`}>
                {icon}
            </div>
            <h3 className="text-white text-lg font-medium mb-2">{title}</h3>
            <p className="text-gray-400 text-sm">{description}</p>
            {!available && (
                <span className="text-yellow-500 text-xs mt-2 block">Not available</span>
            )}
        </button>
    );
}

function StateIndicator({ state }: { state: XRSessionState }) {
    const configs: Record<XRSessionState, { color: string; label: string; icon: React.ReactNode }> = {
        idle: { color: 'gray', label: 'Ready', icon: <Eye /> },
        requesting: { color: 'yellow', label: 'Requesting...', icon: <Loader2 className="animate-spin" /> },
        active: { color: 'green', label: 'Session Active', icon: <Check /> },
        scanning: { color: 'blue', label: 'Scanning...', icon: <Camera /> },
        processing: { color: 'purple', label: 'Processing', icon: <Loader2 className="animate-spin" /> },
        error: { color: 'red', label: 'Error', icon: <AlertTriangle /> },
    };

    const config = configs[state];

    return (
        <div className={`flex items-center gap-2 px-4 py-2 rounded-full bg-${config.color}-500/20`}>
            <span className={`w-5 h-5 text-${config.color}-400`}>{config.icon}</span>
            <span className={`text-${config.color}-400 font-medium`}>{config.label}</span>
        </div>
    );
}

function QualityBadge({ quality }: { quality: string }) {
    const colors: Record<string, string> = {
        low: 'bg-red-500/20 text-red-400',
        medium: 'bg-yellow-500/20 text-yellow-400',
        high: 'bg-green-500/20 text-green-400',
        excellent: 'bg-blue-500/20 text-blue-400',
    };

    return (
        <span className={`px-3 py-1 rounded-full text-sm ${colors[quality] || colors.low}`}>
            {quality.charAt(0).toUpperCase() + quality.slice(1)} Quality
        </span>
    );
}

export default XRScanner;
