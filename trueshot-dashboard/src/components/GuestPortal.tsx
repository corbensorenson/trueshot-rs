/**
 * Guest Portal - Crowd-Sourced Event Capture
 * 
 * Enables guests at events (weddings, concerts, graduations) to contribute
 * video recordings from their phones via browser. Zero app install required.
 * 
 * Features:
 * - QR code access to /guest/{event-id}
 * - Synchronized recording across all devices
 * - Chunked, resumable uploads
 * - Email collection for post-event 4DGS delivery
 */

import { useState, useEffect, useRef } from 'react';
import {
    Video, Camera, Mic, MicOff, Settings, Download, Mail,
    Wifi, Users, Play, Square, Check, X,
    RefreshCw, AlertCircle, Loader2,
    Volume2, VolumeX, RotateCw
} from 'lucide-react';
import toast from 'react-hot-toast';

// ============================================================================
// Types
// ============================================================================

interface EventConfig {
    id: string;
    name: string;
    organizer: string;
    collectEmail: boolean;
    allowLocalSave: boolean;
    maxRecordingDuration: number; // seconds
    preferredQuality: '720p' | '1080p' | '4K';
    syncEnabled: boolean;
    coverImage?: string;
    description?: string;
}

interface GuestSession {
    id: string;
    connected: boolean;
    synced: boolean;
    serverTimeOffset: number;
    guestCount: number;
    recordingGuestCount: number;
}

type RecordingState = 'idle' | 'preparing' | 'recording' | 'stopping' | 'uploading';

interface UploadProgress {
    bytesUploaded: number;
    totalBytes: number;
    chunksUploaded: number;
    totalChunks: number;
    speed: number; // bytes/sec
}

interface GuestPortalProps {
    eventId: string;
}

// ============================================================================
// Guest Portal Component
// ============================================================================

export default function GuestPortal({ eventId }: GuestPortalProps) {
    // -- State --
    const [event, setEvent] = useState<EventConfig | null>(null);
    const [session, setSession] = useState<GuestSession | null>(null);
    const [recordingState, setRecordingState] = useState<RecordingState>('idle');
    const [recordingDuration, setRecordingDuration] = useState(0);
    const [uploadProgress, setUploadProgress] = useState<UploadProgress | null>(null);

    // Settings
    const [facingMode, setFacingMode] = useState<'user' | 'environment'>('environment');
    const [quality, setQuality] = useState<'720p' | '1080p' | '4K'>('1080p');
    const [audioEnabled, setAudioEnabled] = useState(true);
    const [saveToDevice, setSaveToDevice] = useState(false);
    const [email, setEmail] = useState('');
    const [showSettings, setShowSettings] = useState(false);

    // Error/loading states
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);
    const [cameraReady, setCameraReady] = useState(false);

    // Refs
    const videoRef = useRef<HTMLVideoElement>(null);
    const streamRef = useRef<MediaStream | null>(null);
    const mediaRecorderRef = useRef<MediaRecorder | null>(null);
    const chunksRef = useRef<Blob[]>([]);
    const wsRef = useRef<WebSocket | null>(null);
    const recordingTimerRef = useRef<number | null>(null);

    // ========================================================================
    // Initialize
    // ========================================================================

    useEffect(() => {
        loadEventConfig();
        return () => {
            cleanup();
        };
    }, [eventId]); // eslint-disable-line react-hooks/exhaustive-deps

    const loadEventConfig = async () => {
        try {
            setLoading(true);
            // In production, fetch from /api/guest/{eventId}/config
            // For now, use mock data
            await new Promise(r => setTimeout(r, 500));

            const mockEvent: EventConfig = {
                id: eventId,
                name: "Sarah & Mike's Wedding",
                organizer: "TrueShot Events",
                collectEmail: true,
                allowLocalSave: true,
                maxRecordingDuration: 600, // 10 minutes
                preferredQuality: '1080p',
                syncEnabled: true,
                description: "Help us capture every magical moment! Your recordings will be combined into a beautiful 4D memory.",
            };

            setEvent(mockEvent);
            setQuality(mockEvent.preferredQuality);

            // Initialize camera after loading config
            await initializeCamera();

            // Connect WebSocket for sync
            connectWebSocket();

        } catch (err) {
            setError('Failed to load event. Please try again.');
            console.error(err);
        } finally {
            setLoading(false);
        }
    };

    const cleanup = () => {
        if (streamRef.current) {
            streamRef.current.getTracks().forEach(track => track.stop());
        }
        if (wsRef.current) {
            wsRef.current.close();
        }
        if (recordingTimerRef.current) {
            clearInterval(recordingTimerRef.current);
        }
    };

    // ========================================================================
    // Camera Setup
    // ========================================================================

    const initializeCamera = async () => {
        try {
            setCameraReady(false);

            // Request camera permissions
            const constraints: MediaStreamConstraints = {
                video: {
                    facingMode: facingMode,
                    width: { ideal: quality === '4K' ? 3840 : quality === '1080p' ? 1920 : 1280 },
                    height: { ideal: quality === '4K' ? 2160 : quality === '1080p' ? 1080 : 720 },
                },
                audio: audioEnabled,
            };

            const stream = await navigator.mediaDevices.getUserMedia(constraints);
            streamRef.current = stream;

            if (videoRef.current) {
                videoRef.current.srcObject = stream;
            }

            setCameraReady(true);
        } catch (err) {
            console.error('Camera access failed:', err);
            setError('Camera access denied. Please allow camera access to record.');
        }
    };

    const switchCamera = async () => {
        const newMode = facingMode === 'user' ? 'environment' : 'user';
        setFacingMode(newMode);

        // Reinitialize with new camera
        if (streamRef.current) {
            streamRef.current.getTracks().forEach(track => track.stop());
        }
        await initializeCamera();
    };

    const toggleAudio = () => {
        setAudioEnabled(!audioEnabled);
        if (streamRef.current) {
            streamRef.current.getAudioTracks().forEach(track => {
                track.enabled = !audioEnabled;
            });
        }
    };

    // ========================================================================
    // WebSocket Sync
    // ========================================================================

    const connectWebSocket = () => {
        // In production: wss://server/api/guest/{eventId}/connect
        // Mock session for demo
        setSession({
            id: `guest-${Date.now()}`,
            connected: true,
            synced: true,
            serverTimeOffset: 0,
            guestCount: 12,
            recordingGuestCount: 5,
        });

        // In production, would establish real WebSocket
        // const ws = new WebSocket(wsUrl);
        // ws.onopen = () => { ... };
        // ws.onmessage = (e) => { ... };
        // wsRef.current = ws;
    };

    // ========================================================================
    // Recording
    // ========================================================================

    const startRecording = async () => {
        if (!streamRef.current || recordingState !== 'idle') return;

        try {
            setRecordingState('preparing');
            chunksRef.current = [];

            // Create MediaRecorder
            const mimeType = MediaRecorder.isTypeSupported('video/webm;codecs=vp9')
                ? 'video/webm;codecs=vp9'
                : 'video/webm';

            const recorder = new MediaRecorder(streamRef.current, {
                mimeType,
                videoBitsPerSecond: quality === '4K' ? 8000000 : quality === '1080p' ? 4000000 : 2500000,
            });

            recorder.ondataavailable = (e) => {
                if (e.data.size > 0) {
                    chunksRef.current.push(e.data);
                }
            };

            recorder.onstop = () => {
                handleRecordingComplete();
            };

            mediaRecorderRef.current = recorder;
            recorder.start(1000); // Collect data every second

            setRecordingState('recording');
            setRecordingDuration(0);

            // Start timer
            recordingTimerRef.current = window.setInterval(() => {
                setRecordingDuration(prev => {
                    const next = prev + 1;
                    if (event && next >= event.maxRecordingDuration) {
                        stopRecording();
                    }
                    return next;
                });
            }, 1000);

            // Update mock session
            setSession(prev => prev ? { ...prev, recordingGuestCount: prev.recordingGuestCount + 1 } : null);

            toast.success('Recording started!');

        } catch (err) {
            console.error('Failed to start recording:', err);
            toast.error('Failed to start recording');
            setRecordingState('idle');
        }
    };

    const stopRecording = () => {
        if (recordingState !== 'recording') return;

        setRecordingState('stopping');

        if (recordingTimerRef.current) {
            clearInterval(recordingTimerRef.current);
            recordingTimerRef.current = null;
        }

        if (mediaRecorderRef.current && mediaRecorderRef.current.state !== 'inactive') {
            mediaRecorderRef.current.stop();
        }
    };

    const handleRecordingComplete = async () => {
        const blob = new Blob(chunksRef.current, { type: 'video/webm' });
        console.log(`Recording complete: ${(blob.size / 1024 / 1024).toFixed(2)} MB`);

        // Save to device if enabled
        if (saveToDevice) {
            const url = URL.createObjectURL(blob);
            const a = document.createElement('a');
            a.href = url;
            a.download = `${event?.name || 'recording'}_${Date.now()}.webm`;
            a.click();
            URL.revokeObjectURL(url);
            toast.success('Video saved to device!');
        }

        // Upload
        await uploadRecording(blob);
    };

    // ========================================================================
    // Upload
    // ========================================================================

    const uploadRecording = async (blob: Blob) => {
        setRecordingState('uploading');

        const CHUNK_SIZE = 1024 * 1024; // 1MB chunks
        const totalChunks = Math.ceil(blob.size / CHUNK_SIZE);

        setUploadProgress({
            bytesUploaded: 0,
            totalBytes: blob.size,
            chunksUploaded: 0,
            totalChunks,
            speed: 0,
        });

        // Simulate chunked upload
        for (let i = 0; i < totalChunks; i++) {
            const start = i * CHUNK_SIZE;
            const end = Math.min(start + CHUNK_SIZE, blob.size);

            // In production, POST to /api/guest/{eventId}/upload/chunk
            await new Promise(r => setTimeout(r, 100)); // Simulate upload

            setUploadProgress(prev => prev ? {
                ...prev,
                bytesUploaded: end,
                chunksUploaded: i + 1,
                speed: CHUNK_SIZE * 10, // Mock speed
            } : null);
        }

        toast.success('Upload complete! Thank you for contributing!');
        setRecordingState('idle');
        setUploadProgress(null);

        // Update mock session
        setSession(prev => prev ? { ...prev, recordingGuestCount: prev.recordingGuestCount - 1 } : null);
    };

    // ========================================================================
    // Email Registration
    // ========================================================================

    const registerEmail = () => {
        if (!email.includes('@')) {
            toast.error('Please enter a valid email');
            return;
        }

        // In production, POST to /api/guest/{eventId}/register
        toast.success("Email registered! You'll receive the 4D memory after the event.");
    };

    // ========================================================================
    // Helpers
    // ========================================================================

    const formatDuration = (seconds: number): string => {
        const mins = Math.floor(seconds / 60);
        const secs = seconds % 60;
        return `${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
    };

    const formatBytes = (bytes: number): string => {
        if (bytes < 1024) return `${bytes} B`;
        if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
        return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
    };

    // ========================================================================
    // Render
    // ========================================================================

    if (loading) {
        return (
            <div className="guest-portal guest-portal--loading">
                <div className="guest-portal__loader">
                    <Loader2 className="spin" size={48} />
                    <p>Loading event...</p>
                </div>
            </div>
        );
    }

    if (error) {
        return (
            <div className="guest-portal guest-portal--error">
                <AlertCircle size={48} />
                <p>{error}</p>
                <button onClick={() => window.location.reload()}>
                    <RefreshCw size={16} /> Try Again
                </button>
            </div>
        );
    }

    if (!event) return null;

    const isRecording = recordingState === 'recording';
    const isUploading = recordingState === 'uploading';
    const isBusy = recordingState !== 'idle';

    return (
        <div className={`guest-portal ${isRecording ? 'guest-portal--recording' : ''}`}>
            {/* Header */}
            <header className="guest-portal__header">
                <div className="guest-portal__brand">
                    <Video size={24} />
                    <span>TrueShot</span>
                </div>
                <div className="guest-portal__event-name">{event.name}</div>
                <button
                    className="guest-portal__settings-btn"
                    onClick={() => setShowSettings(!showSettings)}
                    disabled={isBusy}
                >
                    <Settings size={20} />
                </button>
            </header>

            {/* Camera Preview */}
            <div className="guest-portal__preview">
                <video
                    ref={videoRef}
                    autoPlay
                    playsInline
                    muted
                    className="guest-portal__video"
                />

                {!cameraReady && (
                    <div className="guest-portal__preview-placeholder">
                        <Camera size={48} />
                        <p>Setting up camera...</p>
                    </div>
                )}

                {/* Recording indicator */}
                {isRecording && (
                    <div className="guest-portal__recording-indicator">
                        <span className="guest-portal__rec-dot"></span>
                        <span>{formatDuration(recordingDuration)}</span>
                    </div>
                )}

                {/* Upload progress overlay */}
                {isUploading && uploadProgress && (
                    <div className="guest-portal__upload-overlay">
                        <Loader2 className="spin" size={32} />
                        <div className="guest-portal__upload-info">
                            <p>Uploading...</p>
                            <div className="guest-portal__upload-bar">
                                <div
                                    className="guest-portal__upload-fill"
                                    style={{ width: `${(uploadProgress.bytesUploaded / uploadProgress.totalBytes) * 100}%` }}
                                />
                            </div>
                            <span>{formatBytes(uploadProgress.bytesUploaded)} / {formatBytes(uploadProgress.totalBytes)}</span>
                        </div>
                    </div>
                )}

                {/* Camera controls */}
                <div className="guest-portal__camera-controls">
                    <button onClick={switchCamera} disabled={isBusy} title="Switch camera">
                        <RotateCw size={20} />
                    </button>
                    <button onClick={toggleAudio} disabled={isBusy} title={audioEnabled ? 'Mute' : 'Unmute'}>
                        {audioEnabled ? <Mic size={20} /> : <MicOff size={20} />}
                    </button>
                </div>
            </div>

            {/* Status Bar */}
            <div className="guest-portal__status-bar">
                <div className="guest-portal__status-item">
                    <Wifi size={14} className={session?.connected ? 'connected' : 'disconnected'} />
                    <span>{session?.connected ? 'Connected' : 'Connecting...'}</span>
                </div>
                <div className="guest-portal__status-item">
                    <Users size={14} />
                    <span>{session?.guestCount || 0} Guests • {session?.recordingGuestCount || 0} Recording</span>
                </div>
            </div>

            {/* Main Button */}
            <div className="guest-portal__main-action">
                {!isRecording && !isUploading ? (
                    <button
                        className="guest-portal__record-btn guest-portal__record-btn--start"
                        onClick={startRecording}
                        disabled={!cameraReady || isBusy}
                    >
                        <Play size={32} />
                        <span>START RECORDING</span>
                    </button>
                ) : isRecording ? (
                    <button
                        className="guest-portal__record-btn guest-portal__record-btn--stop"
                        onClick={stopRecording}
                    >
                        <Square size={32} />
                        <span>STOP RECORDING</span>
                    </button>
                ) : (
                    <div className="guest-portal__uploading">
                        <Loader2 className="spin" size={32} />
                        <span>Uploading...</span>
                    </div>
                )}
            </div>

            {/* Save to Device Toggle */}
            {event.allowLocalSave && (
                <label className="guest-portal__toggle">
                    <input
                        type="checkbox"
                        checked={saveToDevice}
                        onChange={(e) => setSaveToDevice(e.target.checked)}
                        disabled={isBusy}
                    />
                    <span>
                        <Download size={14} /> Save copy to my device
                    </span>
                </label>
            )}

            {/* Email Collection */}
            {event.collectEmail && (
                <div className="guest-portal__email-section">
                    <div className="guest-portal__section-title">
                        <Mail size={16} />
                        <span>Get the 4D memory after the event</span>
                    </div>
                    <div className="guest-portal__email-input">
                        <input
                            type="email"
                            placeholder="your@email.com"
                            value={email}
                            onChange={(e) => setEmail(e.target.value)}
                        />
                        <button onClick={registerEmail}>
                            <Check size={16} />
                        </button>
                    </div>
                </div>
            )}

            {/* Settings Panel */}
            {showSettings && (
                <div className="guest-portal__settings-panel">
                    <div className="guest-portal__settings-header">
                        <h3>Settings</h3>
                        <button onClick={() => setShowSettings(false)}>
                            <X size={20} />
                        </button>
                    </div>

                    <div className="guest-portal__setting">
                        <label>Camera</label>
                        <select
                            value={facingMode}
                            onChange={(e) => setFacingMode(e.target.value as 'user' | 'environment')}
                            disabled={isBusy}
                        >
                            <option value="environment">Back Camera</option>
                            <option value="user">Front Camera</option>
                        </select>
                    </div>

                    <div className="guest-portal__setting">
                        <label>Quality</label>
                        <select
                            value={quality}
                            onChange={(e) => setQuality(e.target.value as '720p' | '1080p' | '4K')}
                            disabled={isBusy}
                        >
                            <option value="720p">720p (HD)</option>
                            <option value="1080p">1080p (Full HD)</option>
                            <option value="4K">4K (Ultra HD)</option>
                        </select>
                    </div>

                    <div className="guest-portal__setting">
                        <label>Audio</label>
                        <button
                            className={`guest-portal__toggle-btn ${audioEnabled ? 'active' : ''}`}
                            onClick={toggleAudio}
                            disabled={isBusy}
                        >
                            {audioEnabled ? <Volume2 size={16} /> : <VolumeX size={16} />}
                            {audioEnabled ? 'On' : 'Off'}
                        </button>
                    </div>
                </div>
            )}

            {/* Event Description */}
            {event.description && (
                <div className="guest-portal__description">
                    <p>{event.description}</p>
                </div>
            )}

            {/* Footer */}
            <footer className="guest-portal__footer">
                <p>Powered by <strong>TrueShot</strong></p>
            </footer>
        </div>
    );
}

// ============================================================================
// CSS - Add to index.css
// ============================================================================

/*
.guest-portal {
    display: flex;
    flex-direction: column;
    min-height: 100vh;
    background: linear-gradient(135deg, #0f0f23 0%, #1a1a2e 50%, #16213e 100%);
    color: white;
    font-family: 'Inter', system-ui, sans-serif;
    padding: 0;
    max-width: 100%;
    overflow-x: hidden;
}

.guest-portal--loading,
.guest-portal--error {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 1rem;
}

.guest-portal__loader {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 1rem;
}

.guest-portal__loader .spin {
    animation: spin 1s linear infinite;
}

@keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
}

.guest-portal__header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 1rem;
    background: rgba(0,0,0,0.3);
}

.guest-portal__brand {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-weight: 700;
    color: #818cf8;
}

.guest-portal__event-name {
    font-size: 0.9rem;
    opacity: 0.8;
    text-align: center;
    flex: 1;
}

.guest-portal__settings-btn {
    background: transparent;
    border: none;
    color: white;
    opacity: 0.7;
    cursor: pointer;
    padding: 0.5rem;
}

.guest-portal__preview {
    position: relative;
    width: 100%;
    aspect-ratio: 16/9;
    background: #000;
    overflow: hidden;
}

.guest-portal__video {
    width: 100%;
    height: 100%;
    object-fit: cover;
}

.guest-portal__preview-placeholder {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    background: rgba(0,0,0,0.8);
    gap: 1rem;
}

.guest-portal__recording-indicator {
    position: absolute;
    top: 1rem;
    left: 1rem;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: rgba(239, 68, 68, 0.9);
    padding: 0.5rem 1rem;
    border-radius: 2rem;
    font-weight: 600;
}

.guest-portal__rec-dot {
    width: 10px;
    height: 10px;
    background: white;
    border-radius: 50%;
    animation: pulse 1s infinite;
}

@keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.5; }
}

.guest-portal__camera-controls {
    position: absolute;
    bottom: 1rem;
    right: 1rem;
    display: flex;
    gap: 0.5rem;
}

.guest-portal__camera-controls button {
    width: 40px;
    height: 40px;
    border-radius: 50%;
    background: rgba(0,0,0,0.6);
    border: none;
    color: white;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
}

.guest-portal__status-bar {
    display: flex;
    justify-content: center;
    gap: 2rem;
    padding: 0.75rem;
    background: rgba(0,0,0,0.3);
    font-size: 0.8rem;
}

.guest-portal__status-item {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    opacity: 0.8;
}

.guest-portal__status-item .connected {
    color: #34d399;
}

.guest-portal__status-item .disconnected {
    color: #f87171;
}

.guest-portal__main-action {
    padding: 1.5rem;
    display: flex;
    justify-content: center;
}

.guest-portal__record-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.75rem;
    width: 100%;
    max-width: 300px;
    padding: 1rem 2rem;
    border: none;
    border-radius: 3rem;
    font-size: 1.1rem;
    font-weight: 700;
    cursor: pointer;
    transition: all 0.2s;
}

.guest-portal__record-btn--start {
    background: linear-gradient(135deg, #10b981, #059669);
    color: white;
}

.guest-portal__record-btn--start:hover:not(:disabled) {
    transform: scale(1.02);
    box-shadow: 0 4px 20px rgba(16, 185, 129, 0.4);
}

.guest-portal__record-btn--stop {
    background: linear-gradient(135deg, #ef4444, #dc2626);
    color: white;
}

.guest-portal__record-btn--stop:hover {
    transform: scale(1.02);
    box-shadow: 0 4px 20px rgba(239, 68, 68, 0.4);
}

.guest-portal__record-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
}

.guest-portal__toggle {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    padding: 0.5rem;
    font-size: 0.9rem;
    opacity: 0.8;
    cursor: pointer;
}

.guest-portal__toggle input {
    width: 1rem;
    height: 1rem;
}

.guest-portal__toggle span {
    display: flex;
    align-items: center;
    gap: 0.4rem;
}

.guest-portal__email-section {
    padding: 1rem 1.5rem;
    border-top: 1px solid rgba(255,255,255,0.1);
}

.guest-portal__section-title {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.85rem;
    opacity: 0.7;
    margin-bottom: 0.75rem;
}

.guest-portal__email-input {
    display: flex;
    gap: 0.5rem;
}

.guest-portal__email-input input {
    flex: 1;
    padding: 0.75rem 1rem;
    border-radius: 0.5rem;
    border: 1px solid rgba(255,255,255,0.2);
    background: rgba(255,255,255,0.1);
    color: white;
    font-size: 1rem;
}

.guest-portal__email-input input::placeholder {
    color: rgba(255,255,255,0.4);
}

.guest-portal__email-input button {
    width: 44px;
    height: 44px;
    border-radius: 0.5rem;
    background: #6366f1;
    border: none;
    color: white;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
}

.guest-portal__upload-overlay {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    background: rgba(0,0,0,0.8);
    gap: 1rem;
}

.guest-portal__upload-info {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.5rem;
}

.guest-portal__upload-bar {
    width: 200px;
    height: 8px;
    background: rgba(255,255,255,0.2);
    border-radius: 4px;
    overflow: hidden;
}

.guest-portal__upload-fill {
    height: 100%;
    background: #10b981;
    transition: width 0.2s;
}

.guest-portal__settings-panel {
    position: fixed;
    bottom: 0;
    left: 0;
    right: 0;
    background: #1a1a2e;
    border-top-left-radius: 1rem;
    border-top-right-radius: 1rem;
    padding: 1.5rem;
    box-shadow: 0 -4px 20px rgba(0,0,0,0.5);
    z-index: 100;
}

.guest-portal__settings-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 1rem;
}

.guest-portal__settings-header button {
    background: transparent;
    border: none;
    color: white;
    opacity: 0.7;
    cursor: pointer;
}

.guest-portal__setting {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.75rem 0;
    border-bottom: 1px solid rgba(255,255,255,0.1);
}

.guest-portal__setting label {
    font-size: 0.9rem;
    opacity: 0.8;
}

.guest-portal__setting select {
    padding: 0.5rem;
    border-radius: 0.25rem;
    background: rgba(255,255,255,0.1);
    border: 1px solid rgba(255,255,255,0.2);
    color: white;
}

.guest-portal__toggle-btn {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.5rem 0.75rem;
    border-radius: 0.25rem;
    border: 1px solid rgba(255,255,255,0.2);
    background: transparent;
    color: white;
    opacity: 0.7;
    cursor: pointer;
}

.guest-portal__toggle-btn.active {
    background: #10b981;
    opacity: 1;
}

.guest-portal__description {
    padding: 1rem 1.5rem;
    text-align: center;
    font-size: 0.85rem;
    opacity: 0.6;
    font-style: italic;
}

.guest-portal__footer {
    margin-top: auto;
    padding: 1rem;
    text-align: center;
    font-size: 0.75rem;
    opacity: 0.5;
}
*/
