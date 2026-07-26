/**
 * Avatar Capture Mode - Complete Interactive UI
 * 
 * Multi-camera avatar scanning with:
 * - Guided capture workflow (T-pose, expressions, voice)
 * - Real-time pose preview
 * - Clothing layer management
 * - Voice recording for TTS cloning
 * - Blendshape editor
 * - Accessory/outfit customization
 */

import { useState, useCallback, useRef, useEffect } from 'react';
import {
    User,
    Camera,
    Mic,
    MicOff,
    Smile,
    Play,
    Save,
    Undo,
    Redo,
    Eye,
    EyeOff,
    Shirt,
    RefreshCw,
    Volume2,
    Wand2,
    ChevronRight,
    Check,
    AlertCircle,
} from 'lucide-react';
import toast from 'react-hot-toast';
import { createLicenseTrial, getLicenseBundles, getLicenseStatus, type LicenseBundleInfo, type LicenseStatusResponse } from '../api/client';
import { FeatureUnlockPanel } from './FeatureUnlockPanel';

// ============================================================================
// Types
// ============================================================================

export interface AvatarSource {
    id: string;
    name: string;
    position: [number, number, number];
    direction: [number, number, number];
    volume: number;
}

export interface ClothingLayer {
    id: string;
    name: string;
    type: 'top' | 'bottom' | 'fullBody' | 'shoes' | 'hat' | 'glasses' | 'accessory';
    visible: boolean;
    thumbnailUrl?: string;
}

export interface Blendshape {
    name: string;
    displayName: string;
    weight: number;
    category: 'mouth' | 'eyes' | 'brows' | 'nose' | 'other';
}

export interface AvatarData {
    id: string;
    name: string;
    thumbnailUrl?: string;
    clothing: ClothingLayer[];
    blendshapes: Blendshape[];
    hasVoiceProfile: boolean;
}

export type CaptureStep =
    | 'intro'
    | 'calibrating'
    | 'tpose'
    | 'expressions'
    | 'motion'
    | 'voice'
    | 'processing'
    | 'complete';

const CAPTURE_STEPS: { id: CaptureStep; label: string; description: string }[] = [
    { id: 'intro', label: 'Introduction', description: 'Get ready to create your avatar' },
    { id: 'tpose', label: 'T-Pose', description: 'Stand in a T-pose for body scan' },
    { id: 'expressions', label: 'Expressions', description: 'Make various facial expressions' },
    { id: 'motion', label: 'Motion', description: 'Perform natural movements' },
    { id: 'voice', label: 'Voice', description: 'Record your voice for cloning' },
    { id: 'processing', label: 'Processing', description: 'Creating your avatar...' },
];

const formatBundlePrice = (bundle?: LicenseBundleInfo | null) => {
    if (!bundle) return 'Pricing unavailable';
    if (!bundle.price_usd) return 'Contact sales';
    const billing = bundle.billing ? ` ${bundle.billing}` : '';
    return `$${bundle.price_usd}${billing}`;
};

// ============================================================================
// Avatar Capture Workflow
// ============================================================================

interface AvatarCaptureProps {
    onComplete: (avatarId: string) => void;
    onCancel: () => void;
}

export function AvatarCapture({ onComplete, onCancel }: AvatarCaptureProps) {
    const [step, setStep] = useState<CaptureStep>('intro');
    const [progress, setProgress] = useState(0);
    const [isRecording, setIsRecording] = useState(false);
    const audioRef = useRef<MediaRecorder | null>(null);
    const [licenseStatus, setLicenseStatus] = useState<LicenseStatusResponse | null>(null);
    const [licenseBundles, setLicenseBundles] = useState<LicenseBundleInfo[]>([]);
    const [unlockBusy, setUnlockBusy] = useState(false);
    const [unlockError, setUnlockError] = useState<string | null>(null);

    const currentStepIndex = CAPTURE_STEPS.findIndex(s => s.id === step);

    const refreshLicensing = useCallback(async () => {
        try {
            const [status, bundles] = await Promise.all([
                getLicenseStatus(),
                getLicenseBundles(),
            ]);
            setLicenseStatus(status);
            setLicenseBundles(bundles);
        } catch {
            setLicenseStatus(null);
            setLicenseBundles([]);
        }
    }, []);

    useEffect(() => {
        refreshLicensing();
    }, [refreshLicensing]);

    const avatarLocked = licenseStatus ? !(licenseStatus.license_valid && licenseStatus.features?.avatar_reconstruction) : false;
    const trialAvailable = licenseStatus?.trial_available ?? true;
    const avatarBundle = licenseBundles.find(bundle => bundle.key === 'avatar_studio') ?? null;
    const avatarPriceLabel = formatBundlePrice(avatarBundle);
    const avatarBundleName = avatarBundle?.name ?? 'Avatar Studio';

    const startAvatarTrial = async () => {
        setUnlockBusy(true);
        setUnlockError(null);
        try {
            await createLicenseTrial({ duration_days: 14, bundles: ['avatar_studio'] });
            await refreshLicensing();
            toast.success('Avatar Studio trial activated.');
        } catch (err) {
            const message = err instanceof Error ? err.message : 'Trial activation failed';
            setUnlockError(message);
            toast.error('Trial unavailable. Purchase required.');
        } finally {
            setUnlockBusy(false);
        }
    };

    const openAvatarPurchase = () => {
        const subject = encodeURIComponent(`TrueShot purchase: ${avatarBundleName}`);
        const body = encodeURIComponent(`I want to buy the ${avatarBundleName} lifetime add-on.`);
        window.open(`mailto:sales@trueshot.ai?subject=${subject}&body=${body}`, '_blank');
    };

    const handleStartCapture = useCallback(() => {
        setStep('tpose');
    }, []);

    const handleNextStep = useCallback(() => {
        const nextIndex = currentStepIndex + 1;
        if (nextIndex < CAPTURE_STEPS.length) {
            setStep(CAPTURE_STEPS[nextIndex].id);
            setProgress(0);
        }
    }, [currentStepIndex]);

    const handleStartVoiceRecording = useCallback(async () => {
        try {
            const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
            const recorder = new MediaRecorder(stream);
            const chunks: Blob[] = [];

            recorder.ondataavailable = (e) => chunks.push(e.data);
            recorder.onstop = () => {
                const blob = new Blob(chunks, { type: 'audio/webm' });
                // Upload voice sample
                console.log('Voice sample recorded:', blob.size, 'bytes');
            };

            recorder.start();
            audioRef.current = recorder;
            setIsRecording(true);
        } catch (err) {
            console.error('Failed to start recording:', err);
        }
    }, []);

    const handleStopVoiceRecording = useCallback(() => {
        if (audioRef.current) {
            audioRef.current.stop();
            setIsRecording(false);
        }
    }, []);

    // Simulate progress during capture steps
    useEffect(() => {
        if (['tpose', 'expressions', 'motion'].includes(step)) {
            const interval = setInterval(() => {
                setProgress(p => {
                    if (p >= 100) {
                        clearInterval(interval);
                        setTimeout(handleNextStep, 500);
                        return 100;
                    }
                    return p + 2;
                });
            }, 100);

            return () => clearInterval(interval);
        }
    }, [step, handleNextStep]);

    // Process avatar after voice recording
    useEffect(() => {
        if (step === 'processing') {
            const timer = setTimeout(() => {
                setStep('complete');
                onComplete('avatar-' + Date.now());
            }, 3000);

            return () => clearTimeout(timer);
        }
    }, [step, onComplete]);

    return (
        <div className="avatar-capture">
            <style>{`
        .avatar-capture {
          min-height: 100vh;
          background: linear-gradient(
            135deg,
            color-mix(in srgb, var(--ts-background) 90%, var(--ts-accent-purple)) 0%,
            color-mix(in srgb, var(--ts-background) 88%, var(--ts-accent-blue)) 100%
          );
          color: var(--ts-text);
          padding: 2rem;
        }

        .avatar-locked {
          max-width: 720px;
          margin: 0 auto;
          display: flex;
          flex-direction: column;
          gap: 1.5rem;
          padding-top: 4rem;
        }
        
        .capture-header {
          display: flex;
          justify-content: space-between;
          align-items: center;
          margin-bottom: 2rem;
        }
        
        .capture-title {
          display: flex;
          align-items: center;
          gap: 1rem;
          font-size: 1.5rem;
          font-weight: 600;
        }
        
        .step-indicator {
          display: flex;
          gap: 0.5rem;
          margin-bottom: 2rem;
        }
        
        .step-dot {
          width: 40px;
          height: 4px;
          border-radius: 2px;
          background: color-mix(in srgb, var(--ts-text) 20%, transparent);
          transition: all 0.3s ease;
        }
        
        .step-dot.active {
          background: #8b5cf6;
        }
        
        .step-dot.complete {
          background: #10b981;
        }
        
        .capture-content {
          display: flex;
          flex-direction: column;
          align-items: center;
          justify-content: center;
          min-height: 60vh;
          text-align: center;
        }
        
        .capture-icon {
          width: 120px;
          height: 120px;
          background: rgba(139, 92, 246, 0.2);
          border-radius: 50%;
          display: flex;
          align-items: center;
          justify-content: center;
          margin-bottom: 2rem;
        }
        
        .capture-instruction {
          font-size: 1.25rem;
          margin-bottom: 1rem;
          color: color-mix(in srgb, var(--ts-text) 90%, transparent);
        }
        
        .capture-description {
          color: var(--ts-muted);
          margin-bottom: 2rem;
          max-width: 400px;
        }
        
        .progress-ring {
          width: 200px;
          height: 200px;
          position: relative;
          margin-bottom: 2rem;
        }
        
        .progress-ring svg {
          transform: rotate(-90deg);
        }
        
        .progress-ring-bg {
          stroke: color-mix(in srgb, var(--ts-text) 18%, transparent);
        }
        
        .progress-ring-fg {
          stroke: #8b5cf6;
          stroke-linecap: round;
          transition: stroke-dashoffset 0.1s ease;
        }
        
        .progress-text {
          position: absolute;
          top: 50%;
          left: 50%;
          transform: translate(-50%, -50%);
          font-size: 2rem;
          font-weight: 600;
        }
        
        .voice-panel {
          background: color-mix(in srgb, var(--ts-text) 6%, transparent);
          border-radius: 1rem;
          padding: 2rem;
          max-width: 500px;
          width: 100%;
        }
        
        .voice-waveform {
          height: 60px;
          background: color-mix(in srgb, var(--ts-overlay) 60%, transparent);
          border-radius: 0.5rem;
          margin-bottom: 1rem;
          display: flex;
          align-items: center;
          justify-content: center;
          gap: 2px;
          overflow: hidden;
        }
        
        .wave-bar {
          width: 3px;
          background: #8b5cf6;
          border-radius: 1.5px;
          animation: wave 0.5s ease-in-out infinite;
        }
        
        @keyframes wave {
          0%, 100% { height: 10px; }
          50% { height: 40px; }
        }
        
        .btn {
          padding: 0.75rem 1.5rem;
          border-radius: 0.5rem;
          font-weight: 500;
          cursor: pointer;
          border: none;
          display: inline-flex;
          align-items: center;
          gap: 0.5rem;
          transition: all 0.2s ease;
        }
        
        .btn-primary {
          background: var(--ts-accent-purple);
          color: var(--ts-text-on-accent);
        }
        
        .btn-primary:hover {
          background: color-mix(in srgb, var(--ts-accent-purple) 85%, #4f46e5);
        }
        
        .btn-secondary {
          background: color-mix(in srgb, var(--ts-text) 10%, transparent);
          color: var(--ts-text);
        }
        
        .btn-danger {
          background: #ef4444;
          color: var(--ts-text-on-accent);
        }
        
        .btn-group {
          display: flex;
          gap: 1rem;
          margin-top: 1rem;
        }
        
        .transcript-box {
          background: color-mix(in srgb, var(--ts-overlay) 60%, transparent);
          border-radius: 0.5rem;
          padding: 1rem;
          min-height: 100px;
          text-align: left;
          font-family: monospace;
          color: color-mix(in srgb, var(--ts-text) 85%, transparent);
        }
        
        .transcript-prompt {
          color: var(--ts-muted);
          font-style: italic;
        }
      `}</style>

            {avatarLocked ? (
                <div className="avatar-locked">
                    <FeatureUnlockPanel
                        title="Avatar Studio"
                        subtitle="Unlock full‑body avatar reconstruction, facial expression capture, and voice cloning."
                        bundleName={avatarBundleName}
                        priceLabel={avatarPriceLabel}
                        capabilities={[
                            'SMPL‑X based body reconstruction',
                            'Facial expression and motion capture',
                            'Voice profile cloning workflow',
                            'Rigged export for DCC/engine pipelines',
                        ]}
                        trialAvailable={trialAvailable}
                        onStartTrial={startAvatarTrial}
                        onBuy={openAvatarPurchase}
                        busy={unlockBusy}
                        errorMessage={unlockError}
                    />
                    <div className="flex justify-center">
                        <button className="btn btn-secondary" onClick={onCancel}>
                            Close
                        </button>
                    </div>
                </div>
            ) : (
                <>
            <div className="capture-header">
                <div className="capture-title">
                    <User size={28} />
                    Avatar Capture
                </div>
                <button className="btn btn-secondary" onClick={onCancel}>
                    Cancel
                </button>
            </div>

            <div className="step-indicator">
                {CAPTURE_STEPS.map((s, i) => (
                    <div
                        key={s.id}
                        className={`step-dot ${i < currentStepIndex ? 'complete' : ''} ${i === currentStepIndex ? 'active' : ''}`}
                    />
                ))}
            </div>

            <div className="capture-content">
                {step === 'intro' && (
                    <>
                        <div className="capture-icon">
                            <User size={60} />
                        </div>
                        <h2 className="capture-instruction">Create Your Avatar</h2>
                        <p className="capture-description">
                            Stand in front of the cameras. We'll capture your body, face, expressions,
                            and voice to create a fully animated avatar version of you.
                        </p>
                        <button className="btn btn-primary" onClick={handleStartCapture}>
                            <Camera size={20} />
                            Start Capture
                        </button>
                    </>
                )}

                {step === 'tpose' && (
                    <>
                        <div className="progress-ring">
                            <svg width="200" height="200">
                                <circle
                                    className="progress-ring-bg"
                                    cx="100"
                                    cy="100"
                                    r="90"
                                    fill="none"
                                    strokeWidth="8"
                                />
                                <circle
                                    className="progress-ring-fg"
                                    cx="100"
                                    cy="100"
                                    r="90"
                                    fill="none"
                                    strokeWidth="8"
                                    strokeDasharray={565}
                                    strokeDashoffset={565 - (progress / 100) * 565}
                                />
                            </svg>
                            <div className="progress-text">{Math.round(progress)}%</div>
                        </div>
                        <h2 className="capture-instruction">Hold T-Pose</h2>
                        <p className="capture-description">
                            Stand with arms extended horizontally, palms facing forward.
                            Stay still while we scan your body shape.
                        </p>
                    </>
                )}

                {step === 'expressions' && (
                    <>
                        <div className="progress-ring">
                            <svg width="200" height="200">
                                <circle className="progress-ring-bg" cx="100" cy="100" r="90" fill="none" strokeWidth="8" />
                                <circle
                                    className="progress-ring-fg"
                                    cx="100"
                                    cy="100"
                                    r="90"
                                    fill="none"
                                    strokeWidth="8"
                                    strokeDasharray={565}
                                    strokeDashoffset={565 - (progress / 100) * 565}
                                />
                            </svg>
                            <div className="progress-text"><Smile size={40} /></div>
                        </div>
                        <h2 className="capture-instruction">Make Expressions</h2>
                        <p className="capture-description">
                            Smile, frown, raise eyebrows, open mouth wide, and make various
                            facial expressions. This creates realistic face animations.
                        </p>
                    </>
                )}

                {step === 'motion' && (
                    <>
                        <div className="progress-ring">
                            <svg width="200" height="200">
                                <circle className="progress-ring-bg" cx="100" cy="100" r="90" fill="none" strokeWidth="8" />
                                <circle
                                    className="progress-ring-fg"
                                    cx="100"
                                    cy="100"
                                    r="90"
                                    fill="none"
                                    strokeWidth="8"
                                    strokeDasharray={565}
                                    strokeDashoffset={565 - (progress / 100) * 565}
                                />
                            </svg>
                            <div className="progress-text"><RefreshCw size={40} /></div>
                        </div>
                        <h2 className="capture-instruction">Natural Movement</h2>
                        <p className="capture-description">
                            Walk in place, turn around slowly, move your arms naturally.
                            This captures how your body and clothing move.
                        </p>
                    </>
                )}

                {step === 'voice' && (
                    <div className="voice-panel">
                        <h2 className="capture-instruction">Voice Recording</h2>
                        <p className="capture-description">
                            Read the text below naturally. This creates a voice clone for your avatar.
                        </p>

                        <div className="voice-waveform">
                            {isRecording ? (
                                [...Array(20)].map((_, i) => (
                                    <div
                                        key={i}
                                        className="wave-bar"
                                        style={{ animationDelay: `${i * 0.05}s` }}
                                    />
                                ))
                            ) : (
                                <Mic size={24} style={{ opacity: 0.5 }} />
                            )}
                        </div>

                        <div className="transcript-box">
                            <p className="transcript-prompt">
                                "Hello, my name is [your name]. I'm excited to create my digital avatar today.
                                This recording will help capture the unique qualities of my voice,
                                including my tone, pace, and natural speaking patterns.
                                I hope you enjoy meeting my avatar!"
                            </p>
                        </div>

                        <div className="btn-group">
                            {!isRecording ? (
                                <button className="btn btn-primary" onClick={handleStartVoiceRecording}>
                                    <Mic size={20} />
                                    Start Recording
                                </button>
                            ) : (
                                <button className="btn btn-danger" onClick={handleStopVoiceRecording}>
                                    <MicOff size={20} />
                                    Stop Recording
                                </button>
                            )}
                            <button
                                className="btn btn-secondary"
                                onClick={() => setStep('processing')}
                                disabled={isRecording}
                            >
                                Skip Voice <ChevronRight size={20} />
                            </button>
                        </div>
                    </div>
                )}

                {step === 'processing' && (
                    <>
                        <div className="capture-icon">
                            <Wand2 size={60} className="animate-spin" />
                        </div>
                        <h2 className="capture-instruction">Creating Your Avatar</h2>
                        <p className="capture-description">
                            Processing multi-view scans, fitting body model, extracting expressions,
                            and preparing your digital twin...
                        </p>
                    </>
                )}

                {step === 'complete' && (
                    <>
                        <div className="capture-icon" style={{ background: 'rgba(16, 185, 129, 0.2)' }}>
                            <Check size={60} color="#10b981" />
                        </div>
                        <h2 className="capture-instruction">Avatar Created!</h2>
                        <p className="capture-description">
                            Your avatar is ready. You can now customize clothing, accessories,
                            and expressions in the editor.
                        </p>
                        <button className="btn btn-primary" onClick={() => onComplete('avatar-' + Date.now())}>
                            Open Editor
                        </button>
                    </>
                )}
            </div>
                </>
            )}
        </div>
    );
}

// ============================================================================
// Avatar Editor
// ============================================================================

interface AvatarEditorProps {
    avatarId: string;
    onSave: () => void;
    onClose: () => void;
}

export function AvatarEditor({ avatarId, onSave, onClose }: AvatarEditorProps) {
    const [avatar, setAvatar] = useState<AvatarData>({
        id: avatarId,
        name: 'My Avatar',
        clothing: [
            { id: 'shirt', name: 'T-Shirt', type: 'top', visible: true },
            { id: 'pants', name: 'Jeans', type: 'bottom', visible: true },
            { id: 'shoes', name: 'Sneakers', type: 'shoes', visible: true },
        ],
        blendshapes: [
            { name: 'smile', displayName: 'Smile', weight: 0, category: 'mouth' },
            { name: 'frown', displayName: 'Frown', weight: 0, category: 'mouth' },
            { name: 'mouthOpen', displayName: 'Mouth Open', weight: 0, category: 'mouth' },
            { name: 'eyebrowsUp', displayName: 'Eyebrows Up', weight: 0, category: 'brows' },
            { name: 'eyebrowsDown', displayName: 'Eyebrows Down', weight: 0, category: 'brows' },
            { name: 'eyesClosed', displayName: 'Eyes Closed', weight: 0, category: 'eyes' },
        ],
        hasVoiceProfile: true,
    });

    const [activeTab, setActiveTab] = useState<'clothing' | 'expressions' | 'voice'>('clothing');
    const [canUndo, setCanUndo] = useState(false);
    const canRedo = false;

    const toggleClothing = (id: string) => {
        setAvatar(prev => ({
            ...prev,
            clothing: prev.clothing.map(c =>
                c.id === id ? { ...c, visible: !c.visible } : c
            ),
        }));
        setCanUndo(true);
    };

    const setBlendshapeWeight = (name: string, weight: number) => {
        setAvatar(prev => ({
            ...prev,
            blendshapes: prev.blendshapes.map(b =>
                b.name === name ? { ...b, weight } : b
            ),
        }));
    };

    const stripToBase = () => {
        setAvatar(prev => ({
            ...prev,
            clothing: prev.clothing.map(c => ({ ...c, visible: false })),
        }));
        setCanUndo(true);
    };

    const restoreAll = () => {
        setAvatar(prev => ({
            ...prev,
            clothing: prev.clothing.map(c => ({ ...c, visible: true })),
        }));
    };

    return (
        <div className="avatar-editor">
            <style>{`
        .avatar-editor {
          display: flex;
          height: 100vh;
          background: var(--ts-background);
          color: var(--ts-text);
        }
        
        .editor-sidebar {
          width: 320px;
          background: color-mix(in srgb, var(--ts-text) 6%, transparent);
          border-right: 1px solid var(--ts-border);
          display: flex;
          flex-direction: column;
        }
        
        .editor-header {
          padding: 1rem;
          border-bottom: 1px solid var(--ts-border);
          display: flex;
          justify-content: space-between;
          align-items: center;
        }
        
        .editor-tabs {
          display: flex;
          border-bottom: 1px solid var(--ts-border);
        }
        
        .editor-tab {
          flex: 1;
          padding: 0.75rem;
          text-align: center;
          cursor: pointer;
          transition: all 0.2s;
          border-bottom: 2px solid transparent;
        }
        
        .editor-tab:hover {
          background: color-mix(in srgb, var(--ts-text) 6%, transparent);
        }
        
        .editor-tab.active {
          border-bottom-color: var(--ts-accent-purple);
          color: var(--ts-accent-purple);
        }
        
        .editor-content {
          flex: 1;
          padding: 1rem;
          overflow-y: auto;
        }
        
        .clothing-item {
          display: flex;
          align-items: center;
          justify-content: space-between;
          padding: 0.75rem;
          background: color-mix(in srgb, var(--ts-text) 4%, transparent);
          border-radius: 0.5rem;
          margin-bottom: 0.5rem;
          cursor: pointer;
          transition: all 0.2s;
        }
        
        .clothing-item:hover {
          background: color-mix(in srgb, var(--ts-text) 8%, transparent);
        }
        
        .clothing-info {
          display: flex;
          align-items: center;
          gap: 0.75rem;
        }
        
        .clothing-icon {
          width: 40px;
          height: 40px;
          background: rgba(139, 92, 246, 0.2);
          border-radius: 0.5rem;
          display: flex;
          align-items: center;
          justify-content: center;
        }
        
        .blendshape-item {
          margin-bottom: 1rem;
        }
        
        .blendshape-header {
          display: flex;
          justify-content: space-between;
          margin-bottom: 0.25rem;
        }
        
        .blendshape-slider {
          width: 100%;
          height: 6px;
          border-radius: 3px;
          appearance: none;
          background: color-mix(in srgb, var(--ts-text) 12%, transparent);
          cursor: pointer;
        }
        
        .blendshape-slider::-webkit-slider-thumb {
          appearance: none;
          width: 16px;
          height: 16px;
          border-radius: 50%;
          background: #8b5cf6;
          cursor: pointer;
        }
        
        .editor-preview {
          flex: 1;
          display: flex;
          align-items: center;
          justify-content: center;
          background: radial-gradient(
            circle at center,
            color-mix(in srgb, var(--ts-accent-purple) 25%, var(--ts-background)) 0%,
            var(--ts-background) 100%
          );
        }
        
        .preview-placeholder {
          width: 300px;
          height: 500px;
          background: color-mix(in srgb, var(--ts-text) 6%, transparent);
          border-radius: 1rem;
          display: flex;
          align-items: center;
          justify-content: center;
          color: color-mix(in srgb, var(--ts-text) 35%, transparent);
        }
        
        .action-bar {
          padding: 1rem;
          border-top: 1px solid var(--ts-border);
          display: flex;
          gap: 0.5rem;
        }
        
        .special-actions {
          padding: 1rem;
          border-top: 1px solid var(--ts-border);
        }
        
        .special-btn {
          width: 100%;
          margin-bottom: 0.5rem;
        }
      `}</style>

            <div className="editor-sidebar">
                <div className="editor-header">
                    <h2>{avatar.name}</h2>
                    <div style={{ display: 'flex', gap: '0.5rem' }}>
                        <button className="btn btn-secondary" disabled={!canUndo}>
                            <Undo size={16} />
                        </button>
                        <button className="btn btn-secondary" disabled={!canRedo}>
                            <Redo size={16} />
                        </button>
                    </div>
                </div>

                <div className="editor-tabs">
                    <div
                        className={`editor-tab ${activeTab === 'clothing' ? 'active' : ''}`}
                        onClick={() => setActiveTab('clothing')}
                    >
                        <Shirt size={18} />
                    </div>
                    <div
                        className={`editor-tab ${activeTab === 'expressions' ? 'active' : ''}`}
                        onClick={() => setActiveTab('expressions')}
                    >
                        <Smile size={18} />
                    </div>
                    <div
                        className={`editor-tab ${activeTab === 'voice' ? 'active' : ''}`}
                        onClick={() => setActiveTab('voice')}
                    >
                        <Volume2 size={18} />
                    </div>
                </div>

                <div className="editor-content">
                    {activeTab === 'clothing' && (
                        <>
                            {avatar.clothing.map(item => (
                                <div
                                    key={item.id}
                                    className="clothing-item"
                                    onClick={() => toggleClothing(item.id)}
                                >
                                    <div className="clothing-info">
                                        <div className="clothing-icon">
                                            <Shirt size={20} />
                                        </div>
                                        <span>{item.name}</span>
                                    </div>
                                    {item.visible ? (
                                        <Eye size={20} color="#10b981" />
                                    ) : (
                                        <EyeOff size={20} color="#6b7280" />
                                    )}
                                </div>
                            ))}

                            <div className="special-actions">
                                <button className="btn btn-secondary special-btn" onClick={stripToBase}>
                                    <EyeOff size={16} />
                                    Strip to Base Layer
                                </button>
                                <button className="btn btn-secondary special-btn" onClick={restoreAll}>
                                    <Eye size={16} />
                                    Restore All Clothing
                                </button>
                            </div>
                        </>
                    )}

                    {activeTab === 'expressions' && (
                        <>
                            {avatar.blendshapes.map(bs => (
                                <div key={bs.name} className="blendshape-item">
                                    <div className="blendshape-header">
                                        <span>{bs.displayName}</span>
                                        <span>{Math.round(bs.weight * 100)}%</span>
                                    </div>
                                    <input
                                        type="range"
                                        className="blendshape-slider"
                                        min="0"
                                        max="1"
                                        step="0.01"
                                        value={bs.weight}
                                        onChange={(e) => setBlendshapeWeight(bs.name, parseFloat(e.target.value))}
                                    />
                                </div>
                            ))}
                        </>
                    )}

                    {activeTab === 'voice' && (
                        <div style={{ textAlign: 'center', padding: '2rem' }}>
                            {avatar.hasVoiceProfile ? (
                                <>
                                    <div style={{
                                        width: 80,
                                        height: 80,
                                        borderRadius: '50%',
                                        background: 'rgba(16, 185, 129, 0.2)',
                                        display: 'flex',
                                        alignItems: 'center',
                                        justifyContent: 'center',
                                        margin: '0 auto 1rem'
                                    }}>
                                        <Volume2 size={32} color="#10b981" />
                                    </div>
                                    <p>Voice profile active</p>
                                    <button className="btn btn-secondary" style={{ marginTop: '1rem' }}>
                                        <Play size={16} />
                                        Test Voice
                                    </button>
                                </>
                            ) : (
                                <>
                                    <AlertCircle size={48} style={{ opacity: 0.5, marginBottom: '1rem' }} />
                                    <p>No voice profile</p>
                                    <button className="btn btn-primary" style={{ marginTop: '1rem' }}>
                                        <Mic size={16} />
                                        Record Voice
                                    </button>
                                </>
                            )}
                        </div>
                    )}
                </div>

                <div className="action-bar">
                    <button className="btn btn-secondary" onClick={onClose} style={{ flex: 1 }}>
                        Cancel
                    </button>
                    <button className="btn btn-primary" onClick={onSave} style={{ flex: 1 }}>
                        <Save size={16} />
                        Save
                    </button>
                </div>
            </div>

            <div className="editor-preview">
                <div className="preview-placeholder">
                    <User size={64} />
                </div>
            </div>
        </div>
    );
}

export default AvatarCapture;
