import { useState, useEffect, useCallback } from 'react';
import { Camera, ChevronRight, Play, Box as BoxIcon, Scan, X, Sparkles, Boxes, RotateCcw, Eye, Zap, Settings2, Loader2, CheckCircle2, Circle, Move, RotateCw, HardDrive, Upload, ArrowRight, Pause, ChevronDown, ChevronUp, ExternalLink } from 'lucide-react';
import toast from 'react-hot-toast';
import { createLicenseTrial, getLicenseBundles, getLicenseStatus, getStreamUrl, wizard, scan, type LicenseBundleInfo, type LicenseStatusResponse, type QualityAssessment, type QualityHistoryEntry, type ScanPlan, type ObjectAnalysis, type ScanProgress, type CoverageStatus } from '../api/client';
import { FeatureUnlockPanel } from './FeatureUnlockPanel';

interface ScanWizardProps {
    isOpen: boolean;
    onClose: () => void;
}

// Comprehensive step flow
type WizardStep =
    | 'background'
    | 'place_object'
    | 'analyzing'
    | 'review'
    | 'capturing'      // Guided capture with instructions
    | 'sd_import'      // Import from SD card for HQ processing
    | 'processing';    // Final reconstruction
// Reconstruction options
type ReconstructionMethod = 'hybrid' | 'gaussian_splatting' | 'photogrammetry';
type GaussianImpl = 'trueshot' | 'gsplat' | 'nerfstudio' | 'inria' | 'polycam_api';
type PhotogrammetryImpl = 'trueshot' | 'colmap' | 'meshroom' | 'realitycapture' | 'polycam_api';
type QualityLevel = 'preview' | 'standard' | 'high' | 'ultra';
type CapturePreset = 'object' | 'room' | 'human' | 'glossy' | 'low-texture' | 'outdoor';

interface PresetEntitlement {
    feature: string;
    bundle: string;
}

interface UpsellDefinition {
    title: string;
    subtitle: string;
    bundleName: string;
    capabilities: string[];
}

// AI Analysis result type
interface AnalysisResult extends ObjectAnalysis {}

function formatDuration(seconds: number): string {
    const mins = Math.floor(seconds / 60);
    const secs = Math.max(0, seconds % 60);
    return `${mins}m ${secs}s`;
}

const PRESET_ENTITLEMENTS: Partial<Record<CapturePreset, PresetEntitlement>> = {
    room: { feature: 'room_reconstruction', bundle: 'room_reconstruction' },
    human: { feature: 'avatar_reconstruction', bundle: 'avatar_studio' },
};

const UPSELL_DEFINITIONS: Record<string, UpsellDefinition> = {
    room_reconstruction: {
        title: 'Room Reconstruction',
        subtitle: 'Unlock full room-scale reconstruction workflows for interior scans, walk-through capture, and AEC deliverables.',
        bundleName: 'Room Reconstruction',
        capabilities: [
            'Room-scale planning and guidance',
            'Coverage strategy tuned for interiors',
            'Production room reconstruction pipeline',
            'Higher-fidelity environment outputs',
        ],
    },
    avatar_studio: {
        title: 'Avatar Studio',
        subtitle: 'Unlock full human/avatar reconstruction with pipeline support for personalized digital humans.',
        bundleName: 'Avatar Studio',
        capabilities: [
            'Avatar-first capture presets',
            'Human geometry reconstruction',
            'Rig-friendly avatar outputs',
            'Identity-preserving facial detail flow',
        ],
    },
};

const formatBundlePrice = (bundle?: LicenseBundleInfo | null) => {
    if (!bundle) return 'Pricing unavailable';
    if (!bundle.price_usd) return 'Contact sales';
    const billing = bundle.billing ? ` ${bundle.billing}` : '';
    return `$${bundle.price_usd}${billing}`;
};

export const ScanWizard = ({ isOpen, onClose }: ScanWizardProps) => {
    // Step state
    const [step, setStep] = useState<WizardStep>('background');

    // Background state
    const [backgroundCapturing, setBackgroundCapturing] = useState(false);
    const [backgroundDate, setBackgroundDate] = useState<string | null>(() => localStorage.getItem('trueshot_background_date'));
    const hasBackground = backgroundDate !== null;

    // Object detection state
    const [objectDetected, setObjectDetected] = useState(false);
    const [detectionConfidence, setDetectionConfidence] = useState(0);
    const [stableTimer, setStableTimer] = useState(0);

    // AI Analysis results
    const [analysisResult, setAnalysisResult] = useState<AnalysisResult | null>(null);
    const [analysisProgress, setAnalysisProgress] = useState(0);

    // Quality intelligence
    const [quality, setQuality] = useState<QualityAssessment | null>(null);
    const [qualityHistory, setQualityHistory] = useState<QualityHistoryEntry[]>([]);
    const [qualityLoading, setQualityLoading] = useState(false);
    const [qualityError, setQualityError] = useState<string | null>(null);
    const [showUncertainty, setShowUncertainty] = useState(false);
    const [uncertaintyUrl, setUncertaintyUrl] = useState<string | null>(null);
    const [showCoverage, setShowCoverage] = useState(false);
    const [coverage, setCoverage] = useState<CoverageStatus | null>(null);

    // User preferences (ONLY quality level matters - everything else computed)
    const [qualityLevel, setQualityLevel] = useState<QualityLevel>('standard');
    const [reconstructionMethod, setReconstructionMethod] = useState<ReconstructionMethod>('hybrid'); // Hybrid is default!
    const [gaussianImpl, setGaussianImpl] = useState<GaussianImpl>('trueshot');
    const [photogrammetryImpl, setPhotogrammetryImpl] = useState<PhotogrammetryImpl>('trueshot');
    const [capturePreset, setCapturePreset] = useState<CapturePreset>('object');
    const [autoCaptureEnabled, setAutoCaptureEnabled] = useState(true);
    const [licenseStatus, setLicenseStatus] = useState<LicenseStatusResponse | null>(null);
    const [licenseBundles, setLicenseBundles] = useState<LicenseBundleInfo[]>([]);
    const [unlockBusy, setUnlockBusy] = useState(false);
    const [unlockError, setUnlockError] = useState<string | null>(null);

    // Computed scan plan (from backend)
    const [scanPlan, setScanPlan] = useState<ScanPlan | null>(null);
    const [planLoading, setPlanLoading] = useState(false);
    const [planError, setPlanError] = useState<string | null>(null);

    const refreshLicensing = useCallback(async () => {
        try {
            const [status, bundles] = await Promise.all([
                getLicenseStatus(),
                getLicenseBundles(),
            ]);
            setLicenseStatus(status);
            setLicenseBundles(bundles);
        } catch (err) {
            console.error(err);
        }
    }, []);

    const hasFeature = useCallback((feature: string) => {
        return Boolean(licenseStatus?.license_valid && licenseStatus?.features?.[feature]);
    }, [licenseStatus]);

    useEffect(() => {
        if (!analysisResult) {
            setScanPlan(null);
            return;
        }
        let cancelled = false;
        setPlanLoading(true);
        setPlanError(null);
        wizard
            .computePlan(qualityLevel, analysisResult, capturePreset)
            .then(plan => {
                if (!cancelled) setScanPlan(plan);
            })
            .catch(err => {
                if (cancelled) return;
                const msg = err instanceof Error ? err.message : String(err);
                setPlanError(msg);
                toast.error(`Scan plan failed: ${msg}`);
            })
            .finally(() => {
                if (!cancelled) setPlanLoading(false);
            });
        return () => {
            cancelled = true;
        };
    }, [analysisResult, qualityLevel]);

    // Capture progress
    const [currentStepIndex, setCurrentStepIndex] = useState(0);
    const [capturedPhotos, setCapturedPhotos] = useState(0);
    const [showPlanDetails, setShowPlanDetails] = useState(false);
    const [scanProgress, setScanProgress] = useState<ScanProgress | null>(null);
    const [scanError, setScanError] = useState<string | null>(null);

    useEffect(() => {
        switch (capturePreset) {
            case 'room':
                setQualityLevel('high');
                setReconstructionMethod('photogrammetry');
                break;
            case 'human':
                setQualityLevel('high');
                setReconstructionMethod('gaussian_splatting');
                break;
            case 'glossy':
                setQualityLevel('high');
                setReconstructionMethod('hybrid');
                break;
            case 'low-texture':
                setQualityLevel('ultra');
                setReconstructionMethod('hybrid');
                break;
            case 'outdoor':
                setQualityLevel('high');
                setReconstructionMethod('hybrid');
                break;
            default:
                setQualityLevel('standard');
                setReconstructionMethod('hybrid');
        }
        setGaussianImpl('trueshot');
        setPhotogrammetryImpl('trueshot');
    }, [capturePreset]);

    // SD card import
    const [importProgress, setImportProgress] = useState(0);

    // Processing
    const [processingProgress, setProcessingProgress] = useState(0);

    useEffect(() => {
        if (!isOpen) return;
        refreshLicensing();
    }, [isOpen, refreshLicensing]);

    useEffect(() => {
        if (!isOpen) return;
        let cancelled = false;
        wizard.getBackgroundStatus()
            .then(status => {
                if (cancelled) return;
                if (status.captured && status.timestamp) {
                    setBackgroundDate(status.timestamp);
                }
            })
            .catch(() => {
                // ignore background status errors
            });
        return () => {
            cancelled = true;
        };
    }, [isOpen]);

    useEffect(() => {
        if (step !== 'place_object') return;
        let cancelled = false;
        const poll = async () => {
            if (cancelled) return;
            try {
                const status = await wizard.getDetectionStatus();
                if (cancelled) return;
                const confidencePct = Math.round((status.confidence <= 1 ? status.confidence * 100 : status.confidence));
                setDetectionConfidence(confidencePct);
                setObjectDetected(status.detected);
                const stableSeconds = Math.floor((status.stable_duration_ms || 0) / 1000);
                setStableTimer(Math.min(4, stableSeconds));
                if (status.stable && status.stable_duration_ms >= 2000) {
                    setStep('analyzing');
                    runAnalysis();
                }
            } catch (err) {
                if (!cancelled) {
                    console.error(err);
                }
            }
        };
        poll();
        const interval = setInterval(poll, 1000);
        return () => {
            cancelled = true;
            clearInterval(interval);
        };
    }, [step]);

    const pushQualityHistory = useCallback((assessment: QualityAssessment) => {
        setQualityHistory(prev => {
            const entry: QualityHistoryEntry = {
                captured_at: new Date().toISOString(),
                score: assessment.score,
                pass: assessment.pass,
                issues: assessment.issues,
                actions: assessment.actions,
            };
            if (prev.length > 0) {
                const last = prev[prev.length - 1];
                if (Math.abs(last.score - entry.score) < 0.0001 && last.pass === entry.pass) {
                    return prev;
                }
            }
            return [...prev, entry].slice(-24);
        });
    }, []);

    const pollQuality = useCallback(async () => {
        if (qualityLoading) return;
        setQualityLoading(true);
        try {
            const assessment = await wizard.getQuality();
            setQuality(assessment);
            setQualityError(null);
            pushQualityHistory(assessment);
            try {
                const history = await wizard.getQualityHistory();
                if (history.length > 0) {
                    setQualityHistory(history.slice(-24));
                }
            } catch {
                // ignore history fetch errors; live polling still works
            }
            if (showUncertainty) {
                const blob = await wizard.getUncertaintyMap();
                const nextUrl = URL.createObjectURL(blob);
                setUncertaintyUrl(prev => {
                    if (prev) URL.revokeObjectURL(prev);
                    return nextUrl;
                });
            }
        } catch (err) {
            setQualityError(err instanceof Error ? err.message : 'Quality fetch failed');
        } finally {
            setQualityLoading(false);
        }
    }, [pushQualityHistory, qualityLoading, showUncertainty]);

    useEffect(() => {
        if (!isOpen) return;
        if (!['place_object', 'review', 'capturing'].includes(step)) return;
        let cancelled = false;
        const run = async () => {
            if (cancelled) return;
            await pollQuality();
        };
        run();
        const interval = setInterval(run, 5000);
        return () => {
            cancelled = true;
            clearInterval(interval);
        };
    }, [isOpen, step, pollQuality]);

    useEffect(() => {
        if (!isOpen) return;
        if (step !== 'capturing') return;
        let cancelled = false;
        const poll = async () => {
            if (cancelled) return;
            try {
                const progress = await scan.getProgress();
                if (cancelled) return;
                setScanProgress(progress);
                setCurrentStepIndex(progress.current_step);
                setCapturedPhotos(progress.photos_captured);
                try {
                    const nextCoverage = await scan.getCoverage();
                    if (!cancelled) {
                        setCoverage(nextCoverage);
                    }
                } catch {
                    // Coverage is optional during capture; ignore failures.
                }
                if (progress.status === 'complete') {
                    toast.success('All photos captured! Insert SD card for HQ import.');
                    setStep('sd_import');
                }
                if (progress.status === 'error') {
                    setScanError(progress.error_message ?? 'Scan failed');
                }
            } catch (err) {
                if (!cancelled) {
                    console.error(err);
                }
            }
        };
        poll();
        const interval = setInterval(poll, 2000);
        return () => {
            cancelled = true;
            clearInterval(interval);
        };
    }, [isOpen, step]);

    useEffect(() => {
        if (!showUncertainty) {
            if (uncertaintyUrl) {
                URL.revokeObjectURL(uncertaintyUrl);
                setUncertaintyUrl(null);
            }
            return;
        }
        if (isOpen && ['place_object', 'review', 'capturing'].includes(step)) {
            pollQuality();
        }
    }, [showUncertainty, isOpen, step, pollQuality, uncertaintyUrl]);

    useEffect(() => {
        if (!isOpen && uncertaintyUrl) {
            URL.revokeObjectURL(uncertaintyUrl);
            setUncertaintyUrl(null);
        }
    }, [isOpen, uncertaintyUrl]);

    useEffect(() => {
        if (!isOpen || step !== 'capturing') {
            setCoverage(null);
        }
    }, [isOpen, step]);

    async function runAnalysis() {
        setAnalysisProgress(10);
        try {
            const analysis = await wizard.analyzeObject();
            setAnalysisProgress(100);
            setAnalysisResult(analysis);
            setStep('review');
        } catch (err) {
            console.error(err);
            toast.error('AI analysis failed. Try re-centering the object.');
            setAnalysisProgress(0);
            setStep('place_object');
        }
    }

    const captureBackground = async () => {
        setBackgroundCapturing(true);
        try {
            const result = await wizard.captureBackground();
            const now = result.timestamp || new Date().toISOString();
            localStorage.setItem('trueshot_background_date', now);
            setBackgroundDate(now);
            toast.success("Background captured!");
        } catch (err) {
            console.error(err);
            toast.error('Background capture failed');
        } finally {
            setBackgroundCapturing(false);
        }
    };

    const startCapture = async () => {
        if (!scanPlan) return;
        setStep('capturing');
        setCurrentStepIndex(0);
        setCapturedPhotos(0);
        setScanError(null);
        try {
            await scan.start({ auto_capture: autoCaptureEnabled });
            const progress = await scan.getProgress();
            setScanProgress(progress);
            setCurrentStepIndex(progress.current_step);
            setCapturedPhotos(progress.photos_captured);
        } catch (err) {
            console.error(err);
            toast.error('Failed to start scan');
            setScanError(err instanceof Error ? err.message : 'Scan failed');
        }
    };

    const advanceStep = async () => {
        if (!scanPlan) return;
        if (scanProgress?.status !== 'paused') return;
        try {
            await scan.executeStep(currentStepIndex);
            const progress = await scan.getProgress();
            setScanProgress(progress);
            setCurrentStepIndex(progress.current_step);
            setCapturedPhotos(progress.photos_captured);
        } catch (err) {
            console.error(err);
            toast.error('Failed to advance scan step');
        }
    };

    const importFromSD = async () => {
        try {
            setImportProgress(5);
            await scan.importFromSDCard();
            setImportProgress(100);
            toast.success('Photos imported from SD card!');
            setStep('processing');
            runProcessing();
        } catch (err) {
            console.error(err);
            toast.error('SD card import failed');
        }
    };

    const runProcessing = async () => {
        for (let i = 0; i <= 100; i += 2) {
            await new Promise(r => setTimeout(r, 100));
            setProcessingProgress(i);
        }
        toast.success('3D Model Complete!');
        setTimeout(() => onClose(), 2000);
    };

    const startPresetTrial = async () => {
        const entitlement = PRESET_ENTITLEMENTS[capturePreset];
        if (!entitlement) return;
        setUnlockBusy(true);
        setUnlockError(null);
        try {
            await createLicenseTrial({ duration_days: 14, bundles: [entitlement.bundle] });
            await refreshLicensing();
            toast.success('Trial activated. Feature unlocked.');
        } catch (err) {
            const message = err instanceof Error ? err.message : 'Trial activation failed';
            setUnlockError(message);
            toast.error('Trial unavailable. Purchase required.');
        } finally {
            setUnlockBusy(false);
        }
    };

    const openPresetPurchase = () => {
        const entitlement = PRESET_ENTITLEMENTS[capturePreset];
        if (!entitlement) return;
        const definition = UPSELL_DEFINITIONS[entitlement.bundle];
        const subject = encodeURIComponent(`TrueShot purchase: ${definition?.bundleName || entitlement.bundle}`);
        const body = encodeURIComponent(`I want to buy the ${definition?.bundleName || entitlement.bundle} lifetime add-on.`);
        window.open(`mailto:sales@trueshot.ai?subject=${subject}&body=${body}`, '_blank');
    };

    if (!isOpen) return null;

    const stepLabels = ['Background', 'Detect', 'Analyze', 'Plan', 'Capture', 'Import', 'Process'];
    const stepKeys: WizardStep[] = ['background', 'place_object', 'analyzing', 'review', 'capturing', 'sd_import', 'processing'];
    const currentStepData = scanPlan?.steps[Math.min(currentStepIndex, Math.max(0, (scanPlan?.steps.length ?? 1) - 1))];
    const presetOptions: { id: CapturePreset; label: string; note: string }[] = [
        { id: 'object', label: 'Object', note: 'Balanced quality for turntable scans.' },
        { id: 'room', label: 'Room', note: 'Higher coverage and mesh-first pipeline.' },
        { id: 'human', label: 'Human', note: 'Splat-first capture with extra detail.' },
        { id: 'glossy', label: 'Glossy', note: 'Specular-safe capture with tighter plan.' },
        { id: 'low-texture', label: 'Low Texture', note: 'Max coverage + extra angles.' },
        { id: 'outdoor', label: 'Outdoor', note: 'High quality with adaptive checks.' },
    ];
    const activePreset = presetOptions.find(option => option.id === capturePreset);
    const presetEntitlement = PRESET_ENTITLEMENTS[capturePreset];
    const presetLocked = Boolean(
        presetEntitlement &&
        !hasFeature(presetEntitlement.feature)
    );
    const presetUpsell = presetEntitlement ? UPSELL_DEFINITIONS[presetEntitlement.bundle] : null;
    const bundleInfo = presetEntitlement
        ? licenseBundles.find(bundle => bundle.key === presetEntitlement.bundle)
        : null;
    const bundleNameOverride = bundleInfo?.name ?? null;
    const priceLabel = formatBundlePrice(bundleInfo);
    const trialAvailable = licenseStatus?.trial_available ?? true;
    const captureInstruction = scanProgress?.current_instruction || currentStepData?.instruction || '';

    const activeQuality = scanProgress?.quality ?? quality;
    const qualityScorePct = activeQuality
        ? Math.round((activeQuality.score <= 1 ? activeQuality.score * 100 : activeQuality.score))
        : null;
    const coverageScorePct = coverage ? Math.round((coverage.coverage_score ?? 0) * 100) : null;
    const coverageDensityPct = coverage ? Math.round((coverage.coverage_density ?? 0) * 100) : null;

    const qualityBadge = activeQuality?.pass ? 'PASS' : 'CHECK';
    const qualityBadgeClass = activeQuality?.pass ? 'text-green-400' : 'text-yellow-400';

    const renderCoverageOverlay = () => {
        if (!coverage || coverage.azimuth_bins <= 0 || coverage.elevation_bins <= 0) {
            return null;
        }
        const max = coverage.counts.reduce((acc, val) => Math.max(acc, val), 0);
        const denom = max > 0 ? max : 1;
        const width = coverage.azimuth_bins;
        const height = coverage.elevation_bins;
        return (
            <svg
                viewBox={`0 0 ${width} ${height}`}
                className="w-full h-full opacity-60"
                preserveAspectRatio="none"
            >
                {coverage.counts.map((value, index) => {
                    const x = index % width;
                    const y = Math.floor(index / width);
                    const norm = Math.min(Math.max(value / denom, 0), 1);
                    const r = Math.round(255 * (1 - norm));
                    const g = Math.round(255 * norm);
                    const b = 40;
                    return (
                        <rect
                            key={index}
                            x={x}
                            y={y}
                            width={1}
                            height={1}
                            fill={`rgb(${r},${g},${b})`}
                        />
                    );
                })}
            </svg>
        );
    };

    const renderQualityPanel = () => (
        <div className="border border-white/10 rounded-xl bg-white/5 p-4 space-y-3">
            <div className="flex items-center justify-between">
                <div>
                    <div className="text-xs uppercase tracking-widest text-white/40">Quality Intelligence</div>
                    <div className="text-lg font-bold text-white">Live Assessment</div>
                </div>
                <div className={`text-sm font-bold ${qualityBadgeClass}`}>
                    {activeQuality ? qualityBadge : 'PENDING'}
                </div>
            </div>

            <div className="flex items-center gap-4">
                <div className="flex-1">
                    <div className="text-xs text-white/40 mb-1">Quality Score</div>
                    <div className="flex items-end gap-2">
                        <div className="text-2xl font-black text-white">
                            {qualityScorePct !== null ? `${qualityScorePct}%` : '--'}
                        </div>
                        {qualityLoading && <Loader2 className="w-4 h-4 text-white/40 animate-spin" />}
                    </div>
                </div>
                <div className="flex items-center gap-2">
                    <button
                        onClick={() => setShowCoverage(v => !v)}
                        className={`px-3 py-2 rounded-lg text-xs font-bold uppercase tracking-wider ${showCoverage ? 'bg-accent-blue text-black' : 'bg-white/10 text-white/60 hover:bg-white/20'}`}
                    >
                        {showCoverage ? 'Hide Coverage' : 'Show Coverage'}
                    </button>
                    <button
                        onClick={() => setShowUncertainty(v => !v)}
                        className={`px-3 py-2 rounded-lg text-xs font-bold uppercase tracking-wider ${showUncertainty ? 'bg-accent-cyan text-black' : 'bg-white/10 text-white/60 hover:bg-white/20'}`}
                    >
                        {showUncertainty ? 'Hide Uncertainty' : 'Show Uncertainty'}
                    </button>
                </div>
            </div>

            {qualityError && (
                <div className="text-xs text-red-400">Quality check failed: {qualityError}</div>
            )}

            {activeQuality && (
                <div className="space-y-3">
                    {coverage && (
                        <div className="bg-black/40 rounded-lg p-2 space-y-2">
                            <div className="flex items-center justify-between">
                                <div className="text-white/40 uppercase tracking-widest text-[10px]">Coverage</div>
                                <div className="text-xs text-white/60">
                                    {coverageScorePct !== null ? `${coverageScorePct}% complete` : '--'}
                                    {coverageDensityPct !== null ? ` · ${coverageDensityPct}% density` : ''}
                                </div>
                            </div>
                            <div className="h-20 rounded-md overflow-hidden border border-white/10 bg-black/50">
                                {renderCoverageOverlay()}
                            </div>
                        </div>
                    )}
                    <div className="grid grid-cols-2 gap-3 text-xs text-white/60">
                        <div className="bg-black/40 rounded-lg p-2">
                            <div className="text-white/40 uppercase tracking-widest mb-1">Issues</div>
                            {activeQuality.issues.length === 0 ? (
                                <div className="text-green-400">None detected</div>
                            ) : (
                                <div className="space-y-1">
                                    {activeQuality.issues.slice(0, 3).map(issue => (
                                        <div key={issue}>• {issue}</div>
                                    ))}
                                </div>
                            )}
                        </div>
                        <div className="bg-black/40 rounded-lg p-2">
                            <div className="text-white/40 uppercase tracking-widest mb-1">Actions</div>
                            {activeQuality.actions.length === 0 ? (
                                <div className="text-white/40">No action needed</div>
                            ) : (
                                <div className="space-y-1">
                                    {activeQuality.actions.slice(0, 3).map(action => (
                                        <div key={action}>• {action}</div>
                                    ))}
                                </div>
                            )}
                        </div>
                    </div>

                    {activeQuality.defects.length > 0 && (
                        <div className="bg-black/40 rounded-lg p-2">
                            <div className="text-white/40 uppercase tracking-widest mb-2">Defects</div>
                            <div className="space-y-1 text-xs text-white/60">
                                {activeQuality.defects.slice(0, 5).map(defect => (
                                    <div key={defect.defect} className="flex items-center justify-between">
                                        <span>{defect.defect}</span>
                                        <span className={defect.status === 'ok' ? 'text-green-400' : 'text-yellow-400'}>
                                            {defect.status.toUpperCase()}
                                        </span>
                                    </div>
                                ))}
                            </div>
                        </div>
                    )}

                    {qualityHistory.length > 1 && (
                        <div className="bg-black/40 rounded-lg p-2">
                            <div className="text-white/40 uppercase tracking-widest mb-2">Quality Trend</div>
                            <div className="flex items-end gap-1 h-10">
                                {qualityHistory.map((entry) => (
                                    <div
                                        key={entry.captured_at}
                                        className={`flex-1 rounded-sm ${entry.pass ? 'bg-green-400/70' : 'bg-yellow-400/70'}`}
                                        style={{
                                            height: `${Math.max(10, Math.min(100, Math.round((entry.score <= 1 ? entry.score * 100 : entry.score))))}%`,
                                        }}
                                    />
                                ))}
                            </div>
                        </div>
                    )}
                </div>
            )}
        </div>
    );

    return (
        <div className="fixed inset-0 z-[9999] flex items-center justify-center bg-black/80 backdrop-blur-sm pointer-events-auto">
            <div className="w-[950px] bg-[#0a0a0a] border border-white/10 rounded-2xl overflow-hidden shadow-2xl flex flex-col max-h-[90vh]">

                {/* Header */}
                <div className="p-5 border-b border-white/10 flex justify-between items-center bg-gradient-to-r from-accent-blue/10 to-accent-cyan/10">
                    <div className="flex items-center gap-4">
                        <div className="p-3 bg-gradient-to-br from-accent-blue to-accent-cyan rounded-xl shadow-lg">
                            <Sparkles className="w-6 h-6 text-white" />
                        </div>
                        <div>
                            <h2 className="text-xl font-bold text-white">Smart Scan Wizard</h2>
                            <p className="text-sm text-white/40">AI-optimized automated capture</p>
                        </div>
                    </div>
                    <button onClick={onClose} className="p-2 hover:bg-white/10 rounded-lg transition-colors text-white/40 hover:text-white">
                        <X className="w-5 h-5" />
                    </button>
                </div>

                {/* Progress Steps */}
                <div className="flex border-b border-white/5 overflow-x-auto">
                    {stepLabels.map((label, i) => {
                        const stepIdx = stepKeys.indexOf(step);
                        const active = i <= stepIdx;
                        const current = i === stepIdx;
                        const completed = i < stepIdx;
                        return (
                            <div key={label} className={`flex-1 min-w-[80px] p-2.5 flex items-center justify-center gap-1.5 text-[10px] font-bold uppercase tracking-wider border-b-2 transition-colors ${active ? 'border-accent-cyan text-white' : 'border-transparent text-white/20'
                                } ${current ? 'bg-accent-cyan/5' : ''}`}>
                                {completed ? (
                                    <CheckCircle2 className="w-3.5 h-3.5 text-accent-cyan" />
                                ) : current ? (
                                    <Circle className="w-3.5 h-3.5 text-accent-cyan fill-accent-cyan/30" />
                                ) : (
                                    <Circle className="w-3.5 h-3.5 text-white/20" />
                                )}
                                {label}
                            </div>
                        );
                    })}
                </div>

                {/* Content */}
                <div className="flex-1 p-6 overflow-y-auto min-h-[400px]">

                    {/* Step: Background */}
                    {step === 'background' && (
                        <div className="space-y-6 animate-in fade-in duration-300">
                            <div className="text-center space-y-2 mb-6">
                                <h3 className="text-2xl font-bold text-white">Background Calibration</h3>
                                <p className="text-white/40">Capture empty turntable for object detection</p>
                            </div>

                            <div className="aspect-video bg-black rounded-xl border border-white/10 overflow-hidden relative mx-auto max-w-2xl">
                                <img src={getStreamUrl(1)} className="w-full h-full object-cover" alt="Live" />
                                <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
                                    <div className="bg-black/60 px-6 py-3 rounded-xl backdrop-blur flex items-center gap-3">
                                        <Eye className="w-5 h-5 text-accent-cyan" />
                                        <span className="text-white font-bold">Live Preview</span>
                                    </div>
                                </div>
                            </div>

                            {hasBackground ? (
                                <div className="flex flex-col items-center gap-4">
                                    <div className="p-4 bg-green-500/10 border border-green-500/20 rounded-xl flex items-center gap-4">
                                        <CheckCircle2 className="w-8 h-8 text-green-400" />
                                        <div>
                                            <div className="font-bold text-green-400">Background Ready</div>
                                            <div className="text-sm text-green-400/60">Captured {backgroundDate ? new Date(backgroundDate).toLocaleDateString() : 'recently'}</div>
                                        </div>
                                    </div>
                                    <div className="flex gap-4">
                                        <button
                                            onClick={() => setStep('place_object')}
                                            className="px-8 py-3 bg-accent-cyan hover:bg-accent-cyan/80 text-black rounded-xl font-bold uppercase tracking-wider flex items-center gap-2 transition-all shadow-lg"
                                        >
                                            Continue <ChevronRight className="w-5 h-5" />
                                        </button>
                                        <button
                                            onClick={captureBackground}
                                            disabled={backgroundCapturing}
                                            className="px-6 py-3 bg-white/10 hover:bg-white/20 text-white rounded-xl font-bold flex items-center gap-2"
                                        >
                                            <RotateCcw className="w-4 h-4" /> Recapture
                                        </button>
                                    </div>
                                </div>
                            ) : (
                                <div className="flex justify-center">
                                    <button
                                        onClick={captureBackground}
                                        disabled={backgroundCapturing}
                                        className="px-10 py-4 bg-accent-blue hover:bg-accent-blue/80 text-white rounded-xl font-bold uppercase tracking-wider flex items-center gap-3 disabled:opacity-50"
                                    >
                                        {backgroundCapturing ? (
                                            <><Loader2 className="w-5 h-5 animate-spin" /> Capturing...</>
                                        ) : (
                                            <><Camera className="w-5 h-5" /> Capture Background</>
                                        )}
                                    </button>
                                </div>
                            )}
                        </div>
                    )}

                    {/* Step: Place Object */}
                    {step === 'place_object' && (
                        <div className="space-y-6 animate-in fade-in duration-300">
                            <div className="text-center space-y-2 mb-6">
                                <h3 className="text-2xl font-bold text-white">Place Your Object</h3>
                                <p className="text-white/40">Center on turntable, AI will detect automatically</p>
                            </div>

                            <div className="aspect-video bg-black rounded-xl border-2 border-accent-cyan/50 overflow-hidden relative mx-auto max-w-2xl">
                                <img src={getStreamUrl(1)} className="w-full h-full object-cover" alt="Detection" />

                                {detectionConfidence > 30 && (
                                    <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
                                        <div className={`border-4 ${objectDetected ? 'border-green-400' : 'border-yellow-400'} rounded-lg transition-all`}
                                            style={{ width: '60%', height: '70%', boxShadow: `0 0 40px ${objectDetected ? 'rgba(74, 222, 128, 0.3)' : 'rgba(250, 204, 21, 0.3)'}` }}>
                                        </div>
                                    </div>
                                )}

                                <div className="absolute bottom-4 left-1/2 -translate-x-1/2 bg-black/80 px-6 py-3 rounded-xl backdrop-blur flex items-center gap-4">
                                    {objectDetected ? (
                                        <>
                                            <CheckCircle2 className="w-6 h-6 text-green-400 animate-pulse" />
                                            <div>
                                                <div className="font-bold text-green-400">Detected!</div>
                                                <div className="text-xs text-green-400/60">Hold steady... {Math.max(0, 4 - stableTimer)}s</div>
                                            </div>
                                        </>
                                    ) : (
                                        <>
                                            <Loader2 className="w-6 h-6 text-yellow-400 animate-spin" />
                                            <div>
                                                <div className="font-bold text-yellow-400">Scanning...</div>
                                                <div className="text-xs text-yellow-400/60">{Math.round(detectionConfidence)}%</div>
                                            </div>
                                        </>
                                    )}
                                </div>
                            </div>

                            <div className="max-w-2xl mx-auto">
                                <div className="h-2 bg-white/10 rounded-full overflow-hidden">
                                    <div className={`h-full transition-all ${objectDetected ? 'bg-green-400' : 'bg-yellow-400'}`}
                                        style={{ width: `${detectionConfidence}%` }} />
                                </div>
                            </div>
                        </div>
                    )}

                    {/* Step: Analyzing */}
                    {step === 'analyzing' && (
                        <div className="space-y-8 animate-in fade-in duration-300 py-8">
                            <div className="text-center space-y-2">
                                <div className="w-16 h-16 rounded-full border-4 border-accent-cyan/30 border-t-accent-cyan animate-spin mx-auto" />
                                <h3 className="text-2xl font-bold text-white mt-6">Analyzing Object...</h3>
                                <p className="text-white/40">Computing optimal scan parameters</p>
                            </div>

                            <div className="max-w-md mx-auto space-y-3">
                                {[
                                    { label: 'Detecting dimensions', threshold: 20 },
                                    { label: 'Analyzing complexity', threshold: 40 },
                                    { label: 'Identifying surface type', threshold: 60 },
                                    { label: 'Checking underside detail', threshold: 80 },
                                    { label: 'Computing optimal plan', threshold: 100 },
                                ].map(({ label, threshold }) => (
                                    <div key={label} className="flex items-center gap-4">
                                        {analysisProgress >= threshold ? (
                                            <CheckCircle2 className="w-5 h-5 text-accent-cyan" />
                                        ) : analysisProgress >= threshold - 20 ? (
                                            <Loader2 className="w-5 h-5 text-white/40 animate-spin" />
                                        ) : (
                                            <Circle className="w-5 h-5 text-white/20" />
                                        )}
                                        <span className={analysisProgress >= threshold ? 'text-white' : 'text-white/40'}>{label}</span>
                                    </div>
                                ))}
                            </div>
                        </div>
                    )}

                    {/* Step: Review Plan */}
                    {step === 'review' && analysisResult && (scanPlan || planLoading || planError) && (
                        <div className="space-y-5 animate-in fade-in duration-300">
                            <div className="text-center space-y-1 mb-4">
                                <h3 className="text-2xl font-bold text-white">Optimized Scan Plan</h3>
                                <p className="text-white/40">AI-computed settings based on object analysis</p>
                            </div>

                            {/* Object Analysis */}
                            <div className="grid grid-cols-4 gap-3">
                                <div className="p-3 bg-white/5 rounded-xl border border-white/10 text-center">
                                    <div className="text-xs uppercase text-white/40 mb-1">Size</div>
                                    <div className="text-lg font-bold text-white capitalize">{analysisResult.size.category}</div>
                                    <div className="text-[10px] text-accent-cyan">{analysisResult.size.dimensions.map(d => `${d}cm`).join(' × ')}</div>
                                </div>
                                <div className="p-3 bg-white/5 rounded-xl border border-white/10 text-center">
                                    <div className="text-xs uppercase text-white/40 mb-1">Complexity</div>
                                    <div className="text-lg font-bold text-white capitalize">{analysisResult.complexity.category}</div>
                                    <div className="text-[10px] text-accent-cyan">{analysisResult.complexity.feature_count} features</div>
                                </div>
                                <div className="p-3 bg-white/5 rounded-xl border border-white/10 text-center">
                                    <div className="text-xs uppercase text-white/40 mb-1">Surface</div>
                                    <div className="text-lg font-bold text-white capitalize">{analysisResult.surface.surface_type}</div>
                                    <div className="text-[10px] text-accent-cyan">{Math.round(analysisResult.surface.specular_ratio * 100)}% specular</div>
                                </div>
                                <div className="p-3 bg-white/5 rounded-xl border border-white/10 text-center">
                                    <div className="text-xs uppercase text-white/40 mb-1">Underside</div>
                                    <div className="text-lg font-bold text-white">{analysisResult.has_underside_detail ? 'Yes' : 'No'}</div>
                                    <div className="text-[10px] text-accent-cyan">{analysisResult.has_underside_detail ? 'Flip required' : 'Single pass'}</div>
                                </div>
                            </div>

                            {/* Capture Preset */}
                            <div className="p-4 bg-white/5 rounded-xl border border-white/10">
                                <div className="flex items-center justify-between mb-3">
                                    <h4 className="font-bold text-white flex items-center gap-2">
                                        <Settings2 className="w-5 h-5 text-accent-cyan" />
                                        Capture Preset
                                    </h4>
                                    <div className="text-xs text-white/40">Auto-adjusts quality + pipeline</div>
                                </div>
                                <div className="grid grid-cols-3 gap-2">
                                    {presetOptions.map(option => {
                                        const entitlement = PRESET_ENTITLEMENTS[option.id];
                                        const locked = Boolean(entitlement && !hasFeature(entitlement.feature));
                                        return (
                                        <button
                                            key={option.id}
                                            onClick={() => {
                                                setCapturePreset(option.id);
                                                setUnlockError(null);
                                            }}
                                            className={`p-3 rounded-xl border-2 transition-all text-center ${capturePreset === option.id
                                                ? 'border-accent-cyan bg-accent-cyan/10'
                                                : 'border-white/10 bg-white/5 hover:border-white/20'
                                                }`}
                                        >
                                            <div className={`font-bold ${capturePreset === option.id ? 'text-accent-cyan' : 'text-white'}`}>
                                                {option.label}
                                            </div>
                                            {locked && (
                                                <div className="text-[10px] mt-1 text-yellow-300 uppercase tracking-widest font-bold">
                                                    Add-on
                                                </div>
                                            )}
                                        </button>
                                        );
                                    })}
                                </div>
                                {activePreset && (
                                    <div className="text-xs text-white/60 mt-2">
                                        {activePreset.note}
                                        {presetLocked && presetUpsell ? ` This preset requires ${bundleNameOverride || presetUpsell.bundleName}.` : ''}
                                    </div>
                                )}
                            </div>

                            {presetLocked && presetUpsell && (
                                <FeatureUnlockPanel
                                    title={presetUpsell.title}
                                    subtitle={presetUpsell.subtitle}
                                    bundleName={bundleNameOverride || presetUpsell.bundleName}
                                    priceLabel={priceLabel}
                                    capabilities={presetUpsell.capabilities}
                                    trialAvailable={trialAvailable}
                                    onStartTrial={startPresetTrial}
                                    onBuy={openPresetPurchase}
                                    busy={unlockBusy}
                                    errorMessage={unlockError}
                                />
                            )}

                            {/* Quality Selector - ONLY user input needed */}
                            {!presetLocked && (
                            <div className="p-4 bg-gradient-to-r from-accent-blue/5 to-accent-cyan/5 rounded-xl border border-white/10">
                                <div className="flex items-center justify-between mb-3">
                                    <h4 className="font-bold text-white flex items-center gap-2">
                                        <Settings2 className="w-5 h-5 text-accent-blue" />
                                        Quality Level
                                        <span className="text-xs text-white/40 font-normal ml-2">(only setting you need to choose)</span>
                                    </h4>
                                </div>

                                <div className="grid grid-cols-4 gap-2">
                                    {(['preview', 'standard', 'high', 'ultra'] as QualityLevel[]).map(level => (
                                        <button
                                            key={level}
                                            onClick={() => setQualityLevel(level)}
                                            className={`p-3 rounded-xl border-2 transition-all text-center ${qualityLevel === level
                                                ? 'border-accent-cyan bg-accent-cyan/10'
                                                : 'border-white/10 bg-white/5 hover:border-white/20'
                                                }`}
                                        >
                                            <div className={`font-bold capitalize ${qualityLevel === level ? 'text-accent-cyan' : 'text-white'}`}>
                                                {level}
                                            </div>
                                        </button>
                                    ))}
                                </div>
                            </div>
                            )}

                            {/* Computed Plan Summary */}
                            {!presetLocked && (
                            <div className="p-4 bg-white/5 rounded-xl border border-white/10">
                                <div className="flex items-center justify-between mb-3">
                                    <h4 className="font-bold text-white flex items-center gap-2">
                                        <Scan className="w-5 h-5 text-accent-cyan" />
                                        Computed Scan Plan
                                    </h4>
                                    <button
                                        onClick={() => setShowPlanDetails(!showPlanDetails)}
                                        className="text-xs text-accent-cyan flex items-center gap-1"
                                    >
                                        {showPlanDetails ? 'Hide' : 'Show'} Details
                                        {showPlanDetails ? <ChevronUp className="w-4 h-4" /> : <ChevronDown className="w-4 h-4" />}
                                    </button>
                                </div>

                                {planLoading && (
                                    <div className="text-sm text-white/60 flex items-center gap-2 mb-3">
                                        <Loader2 className="w-4 h-4 animate-spin" />
                                        Computing plan…
                                    </div>
                                )}
                                {planError && (
                                    <div className="text-sm text-red-400 mb-3">{planError}</div>
                                )}

                                {scanPlan && (
                                    <div className="grid grid-cols-4 gap-4 text-center">
                                        <div>
                                            <div className="text-2xl font-bold text-accent-cyan">{scanPlan.object_orientations}</div>
                                            <div className="text-xs text-white/40">Orientations</div>
                                        </div>
                                        <div>
                                            <div className="text-2xl font-bold text-accent-cyan">{scanPlan.camera_positions_per_orientation}</div>
                                            <div className="text-xs text-white/40">Camera Positions</div>
                                        </div>
                                        <div>
                                            <div className="text-2xl font-bold text-accent-cyan">{scanPlan.total_photos}</div>
                                            <div className="text-xs text-white/40">Total Photos</div>
                                        </div>
                                        <div>
                                            <div className="text-2xl font-bold text-accent-cyan">{formatDuration(scanPlan.estimated_time_seconds)}</div>
                                            <div className="text-xs text-white/40">Est. Time</div>
                                        </div>
                                    </div>
                                )}

                                {showPlanDetails && scanPlan && (
                                    <div className="mt-4 pt-4 border-t border-white/10 text-sm text-white/60 space-y-2">
                                        <div className="flex justify-between">
                                            <span>Photos per rotation:</span>
                                            <span className="text-white">{scanPlan.photos_per_rotation}</span>
                                        </div>
                                        <div className="flex justify-between">
                                            <span>Planner:</span>
                                            <span className="text-white">Adaptive backend</span>
                                        </div>
                                    </div>
                                )}
                            </div>
                            )}

                            {/* Auto Capture */}
                            {!presetLocked && (
                            <div className="p-4 bg-white/5 rounded-xl border border-white/10 flex items-center justify-between">
                                <div>
                                    <div className="font-bold text-white">Auto-capture</div>
                                    <div className="text-xs text-white/40">Uses live quality gating and adaptive plan updates. Manual mode still requires step confirmations.</div>
                                </div>
                                <label className="flex items-center gap-2 text-sm text-white">
                                    <input
                                        type="checkbox"
                                        checked={autoCaptureEnabled}
                                        onChange={e => setAutoCaptureEnabled(e.target.checked)}
                                    />
                                    Enabled
                                </label>
                            </div>
                            )}

                            {/* Reconstruction Method */}
                            {!presetLocked && (
                            <div className="p-4 bg-white/5 rounded-xl border border-white/10">
                                <h4 className="font-bold text-white flex items-center gap-2 mb-3">
                                    <Boxes className="w-5 h-5 text-accent-cyan" />
                                    Reconstruction Engine
                                </h4>

                                {/* Hybrid - Featured Option */}
                                <button
                                    onClick={() => setReconstructionMethod('hybrid')}
                                    className={`w-full p-4 rounded-xl border-2 transition-all mb-3 ${reconstructionMethod === 'hybrid'
                                        ? 'border-accent-purple bg-gradient-to-r from-accent-purple/20 to-accent-cyan/20 shadow-lg shadow-accent-purple/20'
                                        : 'border-white/10 bg-white/5 hover:border-white/20'
                                        }`}
                                >
                                    <div className="flex items-center gap-3">
                                        <div className={`p-2 rounded-lg ${reconstructionMethod === 'hybrid' ? 'bg-accent-purple/30' : 'bg-white/10'}`}>
                                            <Zap className={`w-6 h-6 ${reconstructionMethod === 'hybrid' ? 'text-accent-purple' : 'text-white/40'}`} />
                                        </div>
                                        <div className="text-left flex-1">
                                            <div className="flex items-center gap-2">
                                                <span className="font-bold text-white">Hybrid</span>
                                                <span className="text-[10px] px-2 py-0.5 rounded-full bg-accent-purple/30 text-accent-purple font-bold">RECOMMENDED</span>
                                            </div>
                                            <div className="text-xs text-white/50 mt-0.5">Best of both worlds - 3DGS during scan, mesh after</div>
                                        </div>
                                        {reconstructionMethod === 'hybrid' && <CheckCircle2 className="w-5 h-5 text-accent-purple" />}
                                    </div>
                                    {reconstructionMethod === 'hybrid' && (
                                        <div className="mt-3 pt-3 border-t border-white/10 grid grid-cols-3 gap-2 text-[10px]">
                                            <div className="text-center">
                                                <div className="text-accent-cyan font-bold">Real-time</div>
                                                <div className="text-white/40">Processing</div>
                                            </div>
                                            <div className="text-center">
                                                <div className="text-accent-purple font-bold">Mip-Splatting</div>
                                                <div className="text-white/40">Anti-aliased</div>
                                            </div>
                                            <div className="text-center">
                                                <div className="text-accent-blue font-bold">Mesh + Splat</div>
                                                <div className="text-white/40">Dual output</div>
                                            </div>
                                        </div>
                                    )}
                                </button>

                                <div className="grid grid-cols-2 gap-3 mb-3">
                                    <button
                                        onClick={() => setReconstructionMethod('gaussian_splatting')}
                                        className={`p-3 rounded-xl border-2 transition-all flex items-center gap-3 ${reconstructionMethod === 'gaussian_splatting'
                                            ? 'border-accent-cyan bg-accent-cyan/10'
                                            : 'border-white/10 bg-white/5 hover:border-white/20'
                                            }`}
                                    >
                                        <Sparkles className={`w-5 h-5 ${reconstructionMethod === 'gaussian_splatting' ? 'text-accent-cyan' : 'text-white/40'}`} />
                                        <div className="text-left">
                                            <div className="font-bold text-white text-sm">3DGS Only</div>
                                            <div className="text-[10px] text-white/40">Neural, fast rendering</div>
                                        </div>
                                    </button>
                                    <button
                                        onClick={() => setReconstructionMethod('photogrammetry')}
                                        className={`p-3 rounded-xl border-2 transition-all flex items-center gap-3 ${reconstructionMethod === 'photogrammetry'
                                            ? 'border-accent-blue bg-accent-blue/10'
                                            : 'border-white/10 bg-white/5 hover:border-white/20'
                                            }`}
                                    >
                                        <BoxIcon className={`w-5 h-5 ${reconstructionMethod === 'photogrammetry' ? 'text-accent-blue' : 'text-white/40'}`} />
                                        <div className="text-left">
                                            <div className="font-bold text-white text-sm">Mesh Only</div>
                                            <div className="text-[10px] text-white/40">UV textures</div>
                                        </div>
                                    </button>
                                </div>

                                {/* Implementation options - only show for non-hybrid */}
                                {reconstructionMethod !== 'hybrid' && (
                                    <div className="flex flex-wrap gap-2">
                                        {reconstructionMethod === 'gaussian_splatting' ? (
                                            [
                                                { id: 'trueshot' as GaussianImpl, label: 'TrueShot', icon: Zap },
                                                { id: 'gsplat' as GaussianImpl, label: 'GSplat' },
                                                { id: 'nerfstudio' as GaussianImpl, label: 'Nerfstudio' },
                                                { id: 'polycam_api' as GaussianImpl, label: 'Polycam API', icon: ExternalLink },
                                            ].map(({ id, label, icon: Icon }) => (
                                                <button
                                                    key={id}
                                                    onClick={() => setGaussianImpl(id)}
                                                    className={`px-3 py-1.5 rounded-lg text-xs font-bold transition-all flex items-center gap-1.5 ${gaussianImpl === id ? 'bg-accent-cyan text-black' : 'bg-white/10 text-white/60 hover:bg-white/20'
                                                        }`}
                                                >
                                                    {Icon && <Icon className="w-3 h-3" />}
                                                    {label}
                                                </button>
                                            ))
                                        ) : (
                                            [
                                                { id: 'trueshot' as PhotogrammetryImpl, label: 'TrueShot', icon: Zap },
                                                { id: 'colmap' as PhotogrammetryImpl, label: 'COLMAP' },
                                                { id: 'meshroom' as PhotogrammetryImpl, label: 'Meshroom' },
                                                { id: 'polycam_api' as PhotogrammetryImpl, label: 'Polycam API', icon: ExternalLink },
                                            ].map(({ id, label, icon: Icon }) => (
                                                <button
                                                    key={id}
                                                    onClick={() => setPhotogrammetryImpl(id)}
                                                    className={`px-3 py-1.5 rounded-lg text-xs font-bold transition-all flex items-center gap-1.5 ${photogrammetryImpl === id ? 'bg-accent-blue text-white' : 'bg-white/10 text-white/60 hover:bg-white/20'
                                                        }`}
                                                >
                                                    {Icon && <Icon className="w-3 h-3" />}
                                                    {label}
                                                </button>
                                            ))
                                        )}
                                    </div>
                                )}
                            </div>
                            )}

                            {!presetLocked && (
                                <div className="pt-2">
                                    {renderQualityPanel()}
                                </div>
                            )}
                        </div>
                    )}

                    {/* Step: Guided Capture */}
                    {step === 'capturing' && scanPlan && currentStepData && (
                        <div className="space-y-6 animate-in fade-in duration-300">
                            {/* Progress */}
                            <div className="flex items-center justify-between">
                                <div>
                                    <div className="text-sm text-white/40">Step {currentStepIndex + 1} of {scanPlan.steps.length}</div>
                                    <div className="text-2xl font-bold text-white">{capturedPhotos} / {scanPlan.total_photos} photos</div>
                                </div>
                                <div className="text-right">
                                    <div className="text-4xl font-black text-accent-cyan">
                                        {Math.round((capturedPhotos / scanPlan.total_photos) * 100)}%
                                    </div>
                                </div>
                            </div>

                            <div className="h-2 bg-white/10 rounded-full overflow-hidden">
                                <div
                                    className="h-full bg-gradient-to-r from-accent-blue to-accent-cyan transition-all"
                                    style={{ width: `${(capturedPhotos / scanPlan.total_photos) * 100}%` }}
                                />
                            </div>

                            {/* Current Instruction */}
                            <div className={`p-6 rounded-xl border-2 ${currentStepData.step_type === 'capture'
                                ? 'border-green-500/50 bg-green-500/10'
                                : currentStepData.step_type === 'camera_position'
                                    ? 'border-accent-blue/50 bg-accent-blue/10'
                                    : 'border-yellow-500/50 bg-yellow-500/10'
                                }`}>
                                <div className="flex items-center gap-4">
                                    {currentStepData.step_type === 'capture' ? (
                                        <Camera className="w-12 h-12 text-green-400" />
                                    ) : currentStepData.step_type === 'camera_position' ? (
                                        <Move className="w-12 h-12 text-accent-blue" />
                                    ) : (
                                        <RotateCw className="w-12 h-12 text-yellow-400" />
                                    )}
                                    <div className="flex-1">
                                        <div className="text-xs uppercase text-white/40 mb-1">
                                            {currentStepData.step_type === 'capture' ? 'Take Photo' :
                                                currentStepData.step_type === 'camera_position' ? 'Move Camera' : 'Reposition Object'}
                                        </div>
                                        <div className="text-xl font-bold text-white">
                                            {captureInstruction}
                                        </div>
                                        {currentStepData.step_type !== 'capture' && (
                                            <div className="text-sm text-white/60 mt-1">
                                                Press Continue when ready
                                            </div>
                                        )}
                                        {scanProgress?.warnings && scanProgress.warnings.length > 0 && (
                                            <div className="text-xs text-yellow-300 mt-2">
                                                {scanProgress.warnings.slice(0, 2).map(warning => (
                                                    <div key={warning}>• {warning}</div>
                                                ))}
                                            </div>
                                        )}
                                        {scanError && (
                                            <div className="text-xs text-red-400 mt-2">{scanError}</div>
                                        )}
                                    </div>
                                </div>
                            </div>

                            {/* Live Preview */}
                            <div className="aspect-video bg-black rounded-xl border border-white/10 overflow-hidden relative max-w-xl mx-auto">
                                <img src={getStreamUrl(1)} className="w-full h-full object-cover" alt="Live" />
                                {showUncertainty && uncertaintyUrl && (
                                    <img
                                        src={uncertaintyUrl}
                                        className="absolute inset-0 w-full h-full object-cover opacity-50 mix-blend-screen"
                                        alt="Uncertainty"
                                    />
                                )}
                                {showCoverage && coverage && (
                                    <div className="absolute inset-0 w-full h-full mix-blend-screen">
                                        {renderCoverageOverlay()}
                                    </div>
                                )}
                                {currentStepData.step_type === 'capture' && (
                                    <div className="absolute top-4 right-4 bg-red-500 px-3 py-1 rounded-full text-white text-sm font-bold animate-pulse">
                                        ● REC
                                    </div>
                                )}
                            </div>

                            {renderQualityPanel()}

                            {/* Action Buttons */}
                            <div className="flex justify-center gap-4">
                                <button
                                    onClick={async () => {
                                        try {
                                            await scan.stop();
                                            toast.success('Scan stopped');
                                        } catch (err) {
                                            console.error(err);
                                            toast.error('Failed to stop scan');
                                        }
                                    }}
                                    className="px-6 py-3 bg-white/10 hover:bg-white/20 text-white rounded-xl font-bold flex items-center gap-2"
                                >
                                    <Pause className="w-5 h-5" />
                                    Stop
                                </button>
                                <button
                                    onClick={advanceStep}
                                    disabled={scanProgress?.status !== 'paused'}
                                    className="px-10 py-3 bg-gradient-to-r from-accent-blue to-accent-cyan text-white rounded-xl font-bold uppercase tracking-wider flex items-center gap-2 disabled:opacity-50 disabled:cursor-not-allowed"
                                >
                                    {scanProgress?.status === 'paused' && currentStepData.step_type === 'capture' ? 'Retry Capture' : 'Continue'}
                                    <ArrowRight className="w-5 h-5" />
                                </button>
                            </div>
                        </div>
                    )}

                    {/* Step: SD Card Import */}
                    {step === 'sd_import' && (
                        <div className="space-y-8 animate-in fade-in duration-300 py-8">
                            <div className="text-center space-y-2">
                                <HardDrive className="w-16 h-16 text-accent-cyan mx-auto" />
                                <h3 className="text-2xl font-bold text-white mt-4">Import High-Quality Photos</h3>
                                <p className="text-white/40">Insert SD card from DSLR to import RAW files</p>
                            </div>

                            {importProgress > 0 ? (
                                <div className="max-w-md mx-auto space-y-4">
                                    <div className="h-3 bg-white/10 rounded-full overflow-hidden">
                                        <div
                                            className="h-full bg-gradient-to-r from-accent-blue to-accent-cyan transition-all"
                                            style={{ width: `${importProgress}%` }}
                                        />
                                    </div>
                                    <div className="text-center text-white/60">
                                        Importing... {importProgress}%
                                    </div>
                                </div>
                            ) : (
                                <div className="flex flex-col items-center gap-4">
                                    <div className="p-6 border-2 border-dashed border-white/20 rounded-xl text-center">
                                        <Upload className="w-8 h-8 text-white/40 mx-auto mb-2" />
                                        <div className="text-white/60">Insert SD card or drag files here</div>
                                    </div>
                                    <button
                                        onClick={importFromSD}
                                        className="px-8 py-3 bg-accent-cyan hover:bg-accent-cyan/80 text-black rounded-xl font-bold uppercase tracking-wider flex items-center gap-2"
                                    >
                                        <HardDrive className="w-5 h-5" />
                                        Import from SD Card
                                    </button>
                                    <button
                                        onClick={() => { setStep('processing'); runProcessing(); }}
                                        className="text-white/40 hover:text-white text-sm"
                                    >
                                        Skip (use live preview photos only)
                                    </button>
                                </div>
                            )}
                        </div>
                    )}

                    {/* Step: Processing */}
                    {step === 'processing' && (
                        <div className="space-y-8 animate-in fade-in duration-300 py-8">
                            <div className="text-center space-y-2">
                                <div className="text-6xl font-black text-white">{processingProgress}%</div>
                                <div className="text-accent-cyan uppercase tracking-widest font-bold">
                                    Building 3D Model...
                                </div>
                            </div>

                            <div className="max-w-2xl mx-auto">
                                <div className="h-3 bg-white/10 rounded-full overflow-hidden">
                                    <div
                                        className="h-full bg-gradient-to-r from-accent-blue to-accent-cyan transition-all"
                                        style={{ width: `${processingProgress}%` }}
                                    />
                                </div>
                            </div>

                            <div className="max-w-md mx-auto space-y-2 text-sm text-white/60">
                                {processingProgress < 20 && <div className="flex items-center gap-2"><Loader2 className="w-4 h-4 animate-spin" /> Loading images...</div>}
                                {processingProgress >= 20 && processingProgress < 40 && <div className="flex items-center gap-2"><Loader2 className="w-4 h-4 animate-spin" /> Extracting features...</div>}
                                {processingProgress >= 40 && processingProgress < 60 && <div className="flex items-center gap-2"><Loader2 className="w-4 h-4 animate-spin" /> Matching points...</div>}
                                {processingProgress >= 60 && processingProgress < 80 && <div className="flex items-center gap-2"><Loader2 className="w-4 h-4 animate-spin" /> Reconstructing geometry...</div>}
                                {processingProgress >= 80 && <div className="flex items-center gap-2"><Loader2 className="w-4 h-4 animate-spin" /> Finalizing model...</div>}
                            </div>
                        </div>
                    )}

                </div>

                {/* Footer */}
                <div className="p-5 border-t border-white/10 bg-white/5 flex justify-between items-center">
                    {step === 'review' ? (
                        <>
                            <button
                                onClick={() => setStep('place_object')}
                                className="px-6 py-3 rounded-xl font-bold text-white/60 hover:text-white hover:bg-white/5"
                            >
                                Back
                            </button>
                            <button
                                onClick={presetLocked ? startPresetTrial : startCapture}
                                disabled={presetLocked ? unlockBusy : (!scanPlan || planLoading)}
                                className="px-10 py-4 bg-gradient-to-r from-accent-blue to-accent-cyan text-white rounded-xl font-bold uppercase tracking-wider flex items-center gap-3 shadow-lg disabled:opacity-50 disabled:cursor-not-allowed"
                            >
                                <Play className="w-5 h-5" />
                                {presetLocked ? 'Unlock to Start' : 'Start Capture'}
                            </button>
                        </>
                    ) : step === 'capturing' ? (
                        <div className="w-full flex justify-center">
                            <button
                                onClick={async () => {
                                    try {
                                        await scan.stop();
                                    } catch (err) {
                                        console.error(err);
                                    } finally {
                                        onClose();
                                    }
                                }}
                                className="px-8 py-3 bg-red-500/10 hover:bg-red-500/20 text-red-500 rounded-xl font-bold uppercase tracking-wider border border-red-500/20"
                            >
                                Cancel
                            </button>
                        </div>
                    ) : (
                        <div className="w-full text-center text-white/40 text-sm">
                            {step === 'background' && !hasBackground && 'Capture empty turntable to begin'}
                            {step === 'place_object' && 'Waiting for object detection...'}
                            {step === 'analyzing' && 'Computing optimal parameters...'}
                            {step === 'sd_import' && 'Ready to import high-quality images'}
                            {step === 'processing' && 'Building your 3D model...'}
                        </div>
                    )}
                </div>

            </div>
        </div>
    );
};
