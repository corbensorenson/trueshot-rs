/**
 * Scene Reconstruction Mode
 * 
 * Crowd-sourced 4DGS reconstruction from heterogeneous video sources:
 * - Drag & drop video upload (phones, official, online)
 * - Audio-based temporal synchronization visualization
 * - Video quality assessment
 * - Confidence heatmap overlay
 * - Timeline with multi-source alignment
 */

import { useState, useCallback, useRef, useMemo, useEffect } from 'react';
import {
  Video, Upload, CheckCircle, AlertTriangle,
  Play, Pause, SkipBack, SkipForward, Layers, Zap,
  Settings, X, Film, Activity, Globe, Smartphone, Tv, Camera, Radio
} from 'lucide-react';
import toast from 'react-hot-toast';
import { createLicenseTrial, getLicenseBundles, getLicenseStatus, type LicenseBundleInfo, type LicenseStatusResponse } from '../api/client';
import { FeatureUnlockPanel } from './FeatureUnlockPanel';

// ============================================================================
// Types
// ============================================================================

export interface VideoSource {
  id: string;
  name: string;
  type: 'personal' | 'official' | 'online' | 'surveillance' | 'professional';
  file?: File;
  url?: string;
  duration: number;
  fps: number;
  resolution: [number, number];
  hasAudio: boolean;
  quality: QualityAssessment;
  alignment?: TemporalAlignment;
  state: SourceState;
  thumbnailUrl?: string;
}

export interface QualityAssessment {
  overall: number;
  resolution: number;
  stability: number;
  sharpness: number;
  exposure: number;
}

export interface TemporalAlignment {
  offsetSecs: number;
  confidence: number;
  method: 'audio' | 'visual' | 'manual' | 'metadata';
}

export type SourceState = 'pending' | 'analyzing' | 'aligning' | 'ready' | 'failed';

export type ConfidenceMode = 'none' | 'heatmap' | 'transparency' | 'wireframe';

// ============================================================================
// Main Component
// ============================================================================

interface SceneReconstructionProps {
  onClose: () => void;
}

const formatBundlePrice = (bundle?: LicenseBundleInfo | null) => {
  if (!bundle) return 'Pricing unavailable';
  if (!bundle.price_usd) return 'Contact sales';
  const billing = bundle.billing ? ` ${bundle.billing}` : '';
  return `$${bundle.price_usd}${billing}`;
};

export function SceneReconstruction({ onClose }: SceneReconstructionProps) {
  // State
  const [sources, setSources] = useState<VideoSource[]>([]);
  const [selectedSource, setSelectedSource] = useState<string | null>(null);
  const [isProcessing, setIsProcessing] = useState(false);
  const [currentTime] = useState(0);
  const [isPlaying, setIsPlaying] = useState(false);
  const [confidenceMode, setConfidenceMode] = useState<ConfidenceMode>('heatmap');
  const [showSettings, setShowSettings] = useState(false);
  const [licenseStatus, setLicenseStatus] = useState<LicenseStatusResponse | null>(null);
  const [licenseBundles, setLicenseBundles] = useState<LicenseBundleInfo[]>([]);
  const [unlockBusy, setUnlockBusy] = useState(false);
  const [unlockError, setUnlockError] = useState<string | null>(null);

  const fileInputRef = useRef<HTMLInputElement>(null);
  const dropZoneRef = useRef<HTMLDivElement>(null);

  // Computed values
  const totalDuration = useMemo(() => {
    return sources.reduce((max, s) => {
      const end = (s.alignment?.offsetSecs || 0) + s.duration;
      return Math.max(max, end);
    }, 0);
  }, [sources]);

  const readySources = sources.filter(s => s.state === 'ready');
  const averageConfidence = useMemo(() => {
    if (readySources.length === 0) return 0;
    return readySources.reduce((sum, s) => sum + (s.alignment?.confidence || 0), 0) / readySources.length;
  }, [readySources]);

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

  const dynamicLocked = licenseStatus ? !(licenseStatus.license_valid && licenseStatus.features?.['4dgs']) : false;
  const trialAvailable = licenseStatus?.trial_available ?? true;
  const dynamicBundle = licenseBundles.find(bundle => bundle.key === 'dynamic_4dgs') ?? null;
  const dynamicPriceLabel = formatBundlePrice(dynamicBundle);
  const dynamicBundleName = dynamicBundle?.name ?? 'Dynamic 4DGS';

  const startDynamicTrial = async () => {
    setUnlockBusy(true);
    setUnlockError(null);
    try {
      await createLicenseTrial({ duration_days: 14, bundles: ['dynamic_4dgs'] });
      await refreshLicensing();
      toast.success('Dynamic 4DGS trial activated.');
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Trial activation failed';
      setUnlockError(message);
      toast.error('Trial unavailable. Purchase required.');
    } finally {
      setUnlockBusy(false);
    }
  };

  const openDynamicPurchase = () => {
    const subject = encodeURIComponent(`TrueShot purchase: ${dynamicBundleName}`);
    const body = encodeURIComponent(`I want to buy the ${dynamicBundleName} lifetime add-on.`);
    window.open(`mailto:sales@trueshot.ai?subject=${subject}&body=${body}`, '_blank');
  };

  // Handlers
  async function analyzeSource(source: VideoSource) {
    setSources(prev => prev.map(s =>
      s.id === source.id ? { ...s, state: 'analyzing' as SourceState } : s
    ));

    // Simulate analysis
    await new Promise(resolve => setTimeout(resolve, 2000));

    const quality: QualityAssessment = {
      overall: 0.7 + Math.random() * 0.3,
      resolution: Math.random() > 0.5 ? 1.0 : 0.7,
      stability: 0.3 + Math.random() * 0.7,
      sharpness: 0.5 + Math.random() * 0.5,
      exposure: 0.6 + Math.random() * 0.4,
    };

    setSources(prev => prev.map(s =>
      s.id === source.id ? {
        ...s,
        state: 'aligning' as SourceState,
        duration: 30 + Math.random() * 300,
        quality,
      } : s
    ));

    // Simulate alignment
    await new Promise(resolve => setTimeout(resolve, 1500));

    const alignment: TemporalAlignment = {
      offsetSecs: Math.random() * 60,
      confidence: 0.7 + Math.random() * 0.3,
      method: 'audio',
    };

    setSources(prev => prev.map(s =>
      s.id === source.id ? {
        ...s,
        state: 'ready' as SourceState,
        alignment,
      } : s
    ));

    toast.success(`${source.name} analyzed and aligned`);
  }

  const handleFileDrop = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    const files = Array.from(e.dataTransfer.files).filter(f =>
      f.type.startsWith('video/') || f.type.startsWith('audio/')
    );

    files.forEach(file => {
      const newSource: VideoSource = {
        id: `source-${Date.now()}-${Math.random().toString(36).slice(2)}`,
        name: file.name,
        type: 'personal',
        file,
        duration: 0,
        fps: 30,
        resolution: [1920, 1080],
        hasAudio: true,
        quality: { overall: 0, resolution: 0, stability: 0, sharpness: 0, exposure: 0 },
        state: 'pending',
      };

      setSources(prev => [...prev, newSource]);
      analyzeSource(newSource);
    });
  }, []);

  const handleFileSelect = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    const files = Array.from(e.target.files || []);
    files.forEach(file => {
      const newSource: VideoSource = {
        id: `source-${Date.now()}-${Math.random().toString(36).slice(2)}`,
        name: file.name,
        type: 'personal',
        file,
        duration: 0,
        fps: 30,
        resolution: [1920, 1080],
        hasAudio: true,
        quality: { overall: 0, resolution: 0, stability: 0, sharpness: 0, exposure: 0 },
        state: 'pending',
      };

      setSources(prev => [...prev, newSource]);
      analyzeSource(newSource);
    });
  }, []);

  const handleRemoveSource = (id: string) => {
    setSources(prev => prev.filter(s => s.id !== id));
    if (selectedSource === id) setSelectedSource(null);
  };

  const handleStartReconstruction = async () => {
    if (readySources.length < 2) {
      toast.error('Need at least 2 aligned sources');
      return;
    }

    setIsProcessing(true);
    toast.loading('Starting reconstruction...', { id: 'reconstruct' });

    // This would trigger backend reconstruction
    await new Promise(resolve => setTimeout(resolve, 5000));

    toast.success('Reconstruction complete!', { id: 'reconstruct' });
    setIsProcessing(false);
  };

  const getTypeIcon = (type: VideoSource['type']) => {
    switch (type) {
      case 'personal': return <Smartphone size={16} />;
      case 'official': return <Tv size={16} />;
      case 'online': return <Globe size={16} />;
      case 'surveillance': return <Camera size={16} />;
      case 'professional': return <Film size={16} />;
    }
  };

  const formatTime = (secs: number) => {
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${s.toString().padStart(2, '0')}`;
  };

  return (
    <div className="scene-reconstruction">
      <style>{`
        .scene-reconstruction {
          min-height: 100vh;
          background: linear-gradient(
            180deg,
            color-mix(in srgb, var(--ts-background) 92%, var(--ts-accent-purple)) 0%,
            color-mix(in srgb, var(--ts-background) 88%, var(--ts-accent-blue)) 100%
          );
          color: var(--ts-text);
          display: flex;
          flex-direction: column;
        }
        
        .sr-header {
          padding: 1.5rem 2rem;
          border-bottom: 1px solid var(--ts-border);
          display: flex;
          align-items: center;
          justify-content: space-between;
        }
        
        .sr-title-group {
          display: flex;
          align-items: center;
          gap: 1rem;
        }
        
        .sr-title-icon {
          width: 48px;
          height: 48px;
          border-radius: 12px;
          background: linear-gradient(135deg, #f59e0b 0%, #ef4444 100%);
          display: flex;
          align-items: center;
          justify-content: center;
        }
        
        .sr-title {
          font-size: 1.5rem;
          font-weight: 700;
        }
        
        .sr-subtitle {
          font-size: 0.75rem;
          color: color-mix(in srgb, var(--ts-text) 55%, transparent);
          text-transform: uppercase;
          letter-spacing: 0.1em;
        }
        
        .sr-stats {
          display: flex;
          gap: 2rem;
        }
        
        .sr-stat {
          text-align: center;
        }
        
        .sr-stat-value {
          font-size: 1.5rem;
          font-weight: 700;
          color: var(--ts-accent-amber);
        }

        .sr-locked {
          flex: 1;
          display: flex;
          flex-direction: column;
          justify-content: center;
          gap: 2rem;
          padding: 3rem;
        }
        
        .sr-stat-label {
          font-size: 0.75rem;
          color: color-mix(in srgb, var(--ts-text) 55%, transparent);
          text-transform: uppercase;
        }
        
        .sr-content {
          flex: 1;
          display: flex;
          overflow: hidden;
        }
        
        .sr-sources-panel {
          width: 360px;
          border-right: 1px solid var(--ts-border);
          display: flex;
          flex-direction: column;
        }
        
        .sr-panel-header {
          padding: 1rem 1.5rem;
          border-bottom: 1px solid var(--ts-border);
          display: flex;
          align-items: center;
          justify-content: space-between;
        }
        
        .sr-panel-title {
          font-weight: 600;
          display: flex;
          align-items: center;
          gap: 0.5rem;
        }
        
        .sr-source-list {
          flex: 1;
          overflow-y: auto;
          padding: 1rem;
        }
        
        .sr-drop-zone {
          border: 2px dashed color-mix(in srgb, var(--ts-text) 20%, transparent);
          border-radius: 1rem;
          padding: 2rem;
          text-align: center;
          cursor: pointer;
          transition: all 0.2s;
          margin-bottom: 1rem;
        }
        
        .sr-drop-zone:hover, .sr-drop-zone.active {
          border-color: var(--ts-accent-amber);
          background: color-mix(in srgb, var(--ts-accent-amber) 12%, transparent);
        }
        
        .sr-drop-icon {
          width: 48px;
          height: 48px;
          border-radius: 50%;
          background: color-mix(in srgb, var(--ts-text) 6%, transparent);
          display: flex;
          align-items: center;
          justify-content: center;
          margin: 0 auto 1rem;
        }
        
        .sr-source-card {
          background: color-mix(in srgb, var(--ts-text) 4%, transparent);
          border: 1px solid var(--ts-border);
          border-radius: 0.75rem;
          padding: 1rem;
          margin-bottom: 0.75rem;
          cursor: pointer;
          transition: all 0.2s;
        }
        
        .sr-source-card:hover {
          background: color-mix(in srgb, var(--ts-text) 6%, transparent);
        }
        
        .sr-source-card.selected {
          border-color: var(--ts-accent-amber);
          background: color-mix(in srgb, var(--ts-accent-amber) 14%, transparent);
        }
        
        .sr-source-header {
          display: flex;
          align-items: start;
          justify-content: space-between;
          margin-bottom: 0.75rem;
        }
        
        .sr-source-info {
          display: flex;
          align-items: center;
          gap: 0.75rem;
        }
        
        .sr-source-icon {
          width: 36px;
          height: 36px;
          border-radius: 8px;
          display: flex;
          align-items: center;
          justify-content: center;
        }
        
        .sr-source-icon.personal { background: rgba(59, 130, 246, 0.2); color: #3b82f6; }
        .sr-source-icon.official { background: rgba(168, 85, 247, 0.2); color: #a855f7; }
        .sr-source-icon.online { background: rgba(34, 197, 94, 0.2); color: #22c55e; }
        .sr-source-icon.professional { background: rgba(245, 158, 11, 0.2); color: #f59e0b; }
        
        .sr-source-name {
          font-weight: 500;
          font-size: 0.875rem;
          white-space: nowrap;
          overflow: hidden;
          text-overflow: ellipsis;
          max-width: 180px;
        }
        
        .sr-source-meta {
          font-size: 0.75rem;
          color: color-mix(in srgb, var(--ts-text) 55%, transparent);
        }
        
        .sr-source-status {
          display: inline-flex;
          align-items: center;
          gap: 0.25rem;
          padding: 0.25rem 0.5rem;
          border-radius: 9999px;
          font-size: 0.7rem;
          font-weight: 500;
        }
        
        .sr-source-status.pending { background: rgba(107,114,128,0.2); color: #9ca3af; }
        .sr-source-status.analyzing { background: rgba(59,130,246,0.2); color: #3b82f6; }
        .sr-source-status.aligning { background: rgba(168,85,247,0.2); color: #a855f7; }
        .sr-source-status.ready { background: rgba(34,197,94,0.2); color: #22c55e; }
        .sr-source-status.failed { background: rgba(239,68,68,0.2); color: #ef4444; }
        
        .sr-quality-bar {
          height: 4px;
          background: color-mix(in srgb, var(--ts-text) 16%, transparent);
          border-radius: 2px;
          overflow: hidden;
          margin-top: 0.75rem;
        }
        
        .sr-quality-fill {
          height: 100%;
          border-radius: 2px;
          transition: width 0.3s;
        }
        
        .sr-quality-fill.low { background: #ef4444; }
        .sr-quality-fill.medium { background: #f59e0b; }
        .sr-quality-fill.high { background: #22c55e; }
        
        .sr-main {
          flex: 1;
          display: flex;
          flex-direction: column;
        }
        
        .sr-preview {
          flex: 1;
          background: var(--ts-preview-bg);
          position: relative;
          display: flex;
          align-items: center;
          justify-content: center;
        }
        
        .sr-preview-placeholder {
          text-align: center;
          color: color-mix(in srgb, var(--ts-text) 35%, transparent);
        }
        
        .sr-confidence-legend {
          position: absolute;
          top: 1rem;
          right: 1rem;
          background: color-mix(in srgb, var(--ts-overlay-strong) 90%, transparent);
          border-radius: 0.5rem;
          padding: 0.75rem;
        }
        
        .sr-legend-title {
          font-size: 0.75rem;
          font-weight: 600;
          margin-bottom: 0.5rem;
        }
        
        .sr-legend-gradient {
          width: 120px;
          height: 12px;
          border-radius: 6px;
          background: linear-gradient(90deg, #ef4444, #f59e0b, #22c55e);
          margin-bottom: 0.25rem;
        }
        
        .sr-legend-labels {
          display: flex;
          justify-content: space-between;
          font-size: 0.625rem;
          color: color-mix(in srgb, var(--ts-text) 55%, transparent);
        }
        
        .sr-timeline {
          height: 200px;
          border-top: 1px solid var(--ts-border);
          background: color-mix(in srgb, var(--ts-overlay) 70%, transparent);
          padding: 1rem;
        }
        
        .sr-timeline-header {
          display: flex;
          align-items: center;
          justify-content: space-between;
          margin-bottom: 1rem;
        }
        
        .sr-timeline-controls {
          display: flex;
          align-items: center;
          gap: 0.5rem;
        }
        
        .sr-timeline-btn {
          width: 32px;
          height: 32px;
          border-radius: 8px;
          background: color-mix(in srgb, var(--ts-text) 10%, transparent);
          display: flex;
          align-items: center;
          justify-content: center;
          cursor: pointer;
          transition: all 0.2s;
        }
        
        .sr-timeline-btn:hover {
          background: color-mix(in srgb, var(--ts-text) 18%, transparent);
        }
        
        .sr-timeline-time {
          font-family: monospace;
          font-size: 0.875rem;
        }
        
        .sr-timeline-tracks {
          position: relative;
          height: 100px;
          background: color-mix(in srgb, var(--ts-text) 4%, transparent);
          border-radius: 0.5rem;
          overflow: hidden;
        }
        
        .sr-timeline-ruler {
          position: absolute;
          top: 0;
          left: 0;
          right: 0;
          height: 20px;
          background: color-mix(in srgb, var(--ts-text) 6%, transparent);
          border-bottom: 1px solid var(--ts-border);
        }
        
        .sr-timeline-track {
          position: absolute;
          height: 24px;
          border-radius: 4px;
          display: flex;
          align-items: center;
          padding: 0 0.5rem;
          font-size: 0.7rem;
          font-weight: 500;
          white-space: nowrap;
          overflow: hidden;
          cursor: pointer;
        }
        
        .sr-playhead {
          position: absolute;
          top: 0;
          bottom: 0;
          width: 2px;
          background: var(--ts-accent-amber);
          z-index: 10;
        }
        
        .sr-playhead::before {
          content: '';
          position: absolute;
          top: 0;
          left: -4px;
          width: 10px;
          height: 10px;
          background: var(--ts-accent-amber);
          border-radius: 2px;
          transform: rotate(45deg);
        }
        
        .sr-btn {
          padding: 0.625rem 1.25rem;
          border-radius: 0.5rem;
          font-weight: 500;
          cursor: pointer;
          display: inline-flex;
          align-items: center;
          gap: 0.5rem;
          transition: all 0.2s;
          border: none;
          font-size: 0.875rem;
        }
        
        .sr-btn-primary {
          background: linear-gradient(135deg, var(--ts-accent-amber) 0%, #ef4444 100%);
          color: var(--ts-text-on-accent);
        }
        
        .sr-btn-primary:hover {
          transform: translateY(-1px);
          box-shadow: 0 4px 12px rgba(245, 158, 11, 0.3);
        }
        
        .sr-btn-primary:disabled {
          opacity: 0.5;
          cursor: not-allowed;
          transform: none;
        }
        
        .sr-btn-secondary {
          background: color-mix(in srgb, var(--ts-text) 10%, transparent);
          color: var(--ts-text);
        }
        
        .sr-confidence-toggle {
          display: flex;
          gap: 0.25rem;
          background: color-mix(in srgb, var(--ts-text) 6%, transparent);
          padding: 0.25rem;
          border-radius: 0.5rem;
        }
        
        .sr-confidence-btn {
          padding: 0.375rem 0.75rem;
          border-radius: 0.375rem;
          font-size: 0.75rem;
          cursor: pointer;
          transition: all 0.2s;
        }
        
        .sr-confidence-btn:hover {
          background: color-mix(in srgb, var(--ts-text) 10%, transparent);
        }
        
        .sr-confidence-btn.active {
          background: var(--ts-accent-amber);
          color: var(--ts-text-on-accent);
        }

        .sr-settings-overlay {
          position: fixed;
          inset: 0;
          background: rgba(0, 0, 0, 0.65);
          display: flex;
          align-items: center;
          justify-content: center;
          z-index: 60;
        }

        .sr-settings-panel {
          width: min(480px, 92vw);
          background: color-mix(in srgb, var(--ts-panel) 85%, transparent);
          border: 1px solid var(--ts-border);
          border-radius: 16px;
          padding: 1.5rem;
          box-shadow: 0 24px 48px rgba(0, 0, 0, 0.35);
        }
      `}</style>

      {dynamicLocked ? (
        <div className="sr-locked">
          <FeatureUnlockPanel
            title="Dynamic Scene Reconstruction"
            subtitle="Unlock multi‑source 4D Gaussian Splatting with temporal alignment, confidence overlays, and cinematic playback outputs."
            bundleName={dynamicBundleName}
            priceLabel={dynamicPriceLabel}
            capabilities={[
              'Multi-source 4DGS reconstruction',
              'Audio sync + confidence alignment',
              'Temporal playback + volumetric export',
              'High-fidelity dynamic scene outputs',
            ]}
            trialAvailable={trialAvailable}
            onStartTrial={startDynamicTrial}
            onBuy={openDynamicPurchase}
            busy={unlockBusy}
            errorMessage={unlockError}
          />
          <div className="flex justify-center">
            <button className="sr-btn sr-btn-secondary" onClick={onClose}>
              <X size={14} />
              Close
            </button>
          </div>
        </div>
      ) : (
        <>
          {/* Header */}
          <div className="sr-header">
        <div className="sr-title-group">
          <div className="sr-title-icon">
            <Layers size={24} />
          </div>
          <div>
            <div className="sr-title">Scene Reconstruction</div>
            <div className="sr-subtitle">Multi-Source 4DGS Builder</div>
          </div>
        </div>

        <div className="sr-stats">
          <div className="sr-stat">
            <div className="sr-stat-value">{sources.length}</div>
            <div className="sr-stat-label">Sources</div>
          </div>
          <div className="sr-stat">
            <div className="sr-stat-value">{readySources.length}</div>
            <div className="sr-stat-label">Aligned</div>
          </div>
          <div className="sr-stat">
            <div className="sr-stat-value">{formatTime(totalDuration)}</div>
            <div className="sr-stat-label">Duration</div>
          </div>
          <div className="sr-stat">
            <div className="sr-stat-value">{Math.round(averageConfidence * 100)}%</div>
            <div className="sr-stat-label">Confidence</div>
          </div>
        </div>

        <div style={{ display: 'flex', gap: '0.75rem' }}>
          <button
            className="sr-btn sr-btn-secondary"
            onClick={() => setShowSettings(true)}
          >
            <Settings size={16} />
            Settings
          </button>
          <button
            className="sr-btn sr-btn-primary"
            disabled={readySources.length < 2 || isProcessing}
            onClick={handleStartReconstruction}
          >
            <Zap size={16} />
            {isProcessing ? 'Processing...' : 'Reconstruct Scene'}
          </button>
        </div>
      </div>

      {/* Content */}
      <div className="sr-content">
        {/* Sources Panel */}
        <div className="sr-sources-panel">
          <div className="sr-panel-header">
            <div className="sr-panel-title">
              <Video size={18} />
              Video Sources
            </div>
            <span style={{ fontSize: '0.75rem', color: 'var(--ts-muted)' }}>
              {sources.length} files
            </span>
          </div>

          <div className="sr-source-list">
            <div
              ref={dropZoneRef}
              className="sr-drop-zone"
              onDragOver={(e) => e.preventDefault()}
              onDrop={handleFileDrop}
              onClick={() => fileInputRef.current?.click()}
            >
              <div className="sr-drop-icon">
                <Upload size={24} />
              </div>
              <div style={{ fontWeight: 500, marginBottom: '0.25rem' }}>
                Drop videos here
              </div>
              <div style={{ fontSize: '0.75rem', color: 'var(--ts-muted)' }}>
                or click to browse
              </div>
            </div>

            <input
              ref={fileInputRef}
              type="file"
              accept="video/*,audio/*"
              multiple
              style={{ display: 'none' }}
              onChange={handleFileSelect}
            />

            {sources.map(source => (
              <div
                key={source.id}
                className={`sr-source-card ${selectedSource === source.id ? 'selected' : ''}`}
                onClick={() => setSelectedSource(source.id)}
              >
                <div className="sr-source-header">
                  <div className="sr-source-info">
                    <div className={`sr-source-icon ${source.type}`}>
                      {getTypeIcon(source.type)}
                    </div>
                    <div>
                      <div className="sr-source-name" title={source.name}>{source.name}</div>
                      <div className="sr-source-meta">
                        {formatTime(source.duration)} • {source.resolution[0]}x{source.resolution[1]}
                      </div>
                    </div>
                  </div>
                  <button
                    onClick={(e) => { e.stopPropagation(); handleRemoveSource(source.id); }}
                    style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'color-mix(in srgb, var(--ts-text) 40%, transparent)' }}
                  >
                    <X size={14} />
                  </button>
                </div>

                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
                  <div className={`sr-source-status ${source.state}`}>
                    {source.state === 'pending' && 'Pending'}
                    {source.state === 'analyzing' && <><Activity size={10} /> Analyzing...</>}
                    {source.state === 'aligning' && <><Radio size={10} /> Aligning...</>}
                    {source.state === 'ready' && <><CheckCircle size={10} /> Ready</>}
                    {source.state === 'failed' && <><AlertTriangle size={10} /> Failed</>}
                  </div>

                  {source.alignment && (
                    <span style={{ fontSize: '0.7rem', color: 'var(--ts-muted)' }}>
                      {Math.round(source.alignment.confidence * 100)}% sync
                    </span>
                  )}
                </div>

                <div className="sr-quality-bar">
                  <div
                    className={`sr-quality-fill ${source.quality.overall > 0.7 ? 'high' :
                      source.quality.overall > 0.4 ? 'medium' : 'low'
                      }`}
                    style={{ width: `${source.quality.overall * 100}%` }}
                  />
                </div>
              </div>
            ))}
          </div>
        </div>

        {/* Main View */}
        <div className="sr-main">
          {/* Preview */}
          <div className="sr-preview">
            {sources.length === 0 ? (
              <div className="sr-preview-placeholder">
                <Layers size={64} style={{ marginBottom: '1rem', opacity: 0.3 }} />
                <div>Add video sources to begin</div>
              </div>
            ) : (
              <>
                {/* Placeholder for 4DGS preview */}
                <div style={{ color: 'var(--ts-muted)' }}>
                  4DGS Preview
                </div>

                {confidenceMode !== 'none' && (
                  <div className="sr-confidence-legend">
                    <div className="sr-legend-title">Confidence</div>
                    <div className="sr-legend-gradient" />
                    <div className="sr-legend-labels">
                      <span>Low</span>
                      <span>High</span>
                    </div>
                  </div>
                )}
              </>
            )}
          </div>

          {/* Timeline */}
          <div className="sr-timeline">
            <div className="sr-timeline-header">
              <div className="sr-timeline-controls">
                <button className="sr-timeline-btn">
                  <SkipBack size={14} />
                </button>
                <button
                  className="sr-timeline-btn"
                  onClick={() => setIsPlaying(!isPlaying)}
                >
                  {isPlaying ? <Pause size={14} /> : <Play size={14} />}
                </button>
                <button className="sr-timeline-btn">
                  <SkipForward size={14} />
                </button>
                <div className="sr-timeline-time">
                  {formatTime(currentTime)} / {formatTime(totalDuration)}
                </div>
              </div>

              <div className="sr-confidence-toggle">
                {(['none', 'heatmap', 'transparency', 'wireframe'] as ConfidenceMode[]).map(mode => (
                  <button
                    key={mode}
                    className={`sr-confidence-btn ${confidenceMode === mode ? 'active' : ''}`}
                    onClick={() => setConfidenceMode(mode)}
                  >
                    {mode === 'none' ? 'Off' : mode.charAt(0).toUpperCase() + mode.slice(1)}
                  </button>
                ))}
              </div>
            </div>

            <div className="sr-timeline-tracks">
              <div className="sr-timeline-ruler" />

              {sources.filter(s => s.state === 'ready').map((source, i) => {
                const left = totalDuration > 0
                  ? ((source.alignment?.offsetSecs || 0) / totalDuration) * 100
                  : 0;
                const width = totalDuration > 0
                  ? (source.duration / totalDuration) * 100
                  : 0;

                return (
                  <div
                    key={source.id}
                    className="sr-timeline-track"
                    style={{
                      left: `${left}%`,
                      width: `${width}%`,
                      top: 24 + i * 28,
                      background: source.type === 'official'
                        ? 'color-mix(in srgb, var(--ts-accent-purple) 70%, transparent)'
                        : 'color-mix(in srgb, var(--ts-accent-blue) 70%, transparent)',
                    }}
                    title={source.name}
                  >
                    {source.name}
                  </div>
                );
              })}

              <div
                className="sr-playhead"
                style={{ left: `${(currentTime / totalDuration) * 100}%` }}
              />
            </div>
          </div>
        </div>
      </div>
          {showSettings && (
            <div className="sr-settings-overlay">
              <div className="sr-settings-panel">
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: '1rem' }}>
                  <div style={{ fontWeight: 700 }}>Scene Reconstruction Settings</div>
                  <button className="sr-btn sr-btn-secondary" onClick={() => setShowSettings(false)}>
                    <X size={14} />
                    Close
                  </button>
                </div>
                <div style={{ color: 'var(--ts-muted)', fontSize: '0.85rem', lineHeight: 1.5 }}>
                  Configure reconstruction preferences, sync tolerances, and export presets here.
                  We will wire this panel into persistent settings next.
                </div>
              </div>
            </div>
          )}
        </>
      )}
    </div>
  );
}

export default SceneReconstruction;
