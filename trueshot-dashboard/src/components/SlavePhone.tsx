/**
 * Slave Phone - Server-Controlled Camera Portal
 * 
 * Mobile web app for mounting phones as controlled cameras.
 * Waits for CAPTURE commands from the TrueShot server.
 * 
 * Use cases:
 * - 3DGS turntable capture (mount phones around object)
 * - Room scanning (place phones at key positions)
 * - Synchronized multi-view capture
 */

import { useState, useEffect, useRef, useCallback } from 'react';
import {
    Camera,
    Wifi,
    WifiOff,
    Battery,
    Check,
    X,
    Settings,
    Volume2,
    VolumeX
} from 'lucide-react';
import { ThemeToggleFloating } from './ThemeToggleFloating';

// ============================================================================
// Types
// ============================================================================

interface PhoneConfig {
    name: string;
    resolution: { width: number; height: number };
    flash: boolean;
    quality: number;
    facingMode: 'user' | 'environment';
}

interface ServerConnection {
    connected: boolean;
    sessionId: string | null;
    serverUrl: string;
    lastPing: number;
}

interface CaptureState {
    isCapturing: boolean;
    captureCount: number;
    lastCaptureTime: number | null;
    countdown: number | null;
}

type BatteryManagerLike = {
    level: number;
    addEventListener: (type: 'levelchange', listener: () => void) => void;
};

type WakeLockSentinelLike = {
    release: () => Promise<void> | void;
};

type NavigatorWithBattery = Navigator & {
    getBattery?: () => Promise<BatteryManagerLike>;
    wakeLock?: { request: (type: 'screen') => Promise<WakeLockSentinelLike> };
};

const DEFAULT_CAMERA_NAME = `Camera ${Math.floor(Math.random() * 1000)}`;

type WsMessageType =
    | { type: 'capture'; capture_id: string; flash: boolean; countdown_ms: number; quality: number }
    | { type: 'set_resolution'; width: number; height: number }
    | { type: 'set_flash'; enabled: boolean }
    | { type: 'start_video' }
    | { type: 'stop_video' }
    | { type: 'ping' }
    | { type: 'registered'; session_id: string; server_time: number };

// ============================================================================
// Main Component
// ============================================================================

export function SlavePhone() {
    // Refs
    const videoRef = useRef<HTMLVideoElement>(null);
    const canvasRef = useRef<HTMLCanvasElement>(null);
    const wsRef = useRef<WebSocket | null>(null);
    const streamRef = useRef<MediaStream | null>(null);
    const connectToServerRef = useRef<() => void>(() => {});

    // State
    const [config, setConfig] = useState<PhoneConfig>({
        name: DEFAULT_CAMERA_NAME,
        resolution: { width: 1920, height: 1080 },
        flash: false,
        quality: 90,
        facingMode: 'environment'
    });

    const [connection, setConnection] = useState<ServerConnection>({
        connected: false,
        sessionId: null,
        serverUrl: window.location.origin.replace('3000', '8080'),
        lastPing: 0
    });

    const [capture, setCapture] = useState<CaptureState>({
        isCapturing: false,
        captureCount: 0,
        lastCaptureTime: null,
        countdown: null
    });

    const [battery, setBattery] = useState(100);
    const [isReady, setIsReady] = useState(false);
    const [showSettings, setShowSettings] = useState(false);
    const [audioEnabled, setAudioEnabled] = useState(true);
    const [error, setError] = useState<string | null>(null);

    // ========================================================================
    // Camera Setup
    // ========================================================================

    const initCamera = useCallback(async () => {
        try {
            if (streamRef.current) {
                streamRef.current.getTracks().forEach(t => t.stop());
            }

            const stream = await navigator.mediaDevices.getUserMedia({
                video: {
                    facingMode: config.facingMode,
                    width: { ideal: config.resolution.width },
                    height: { ideal: config.resolution.height }
                },
                audio: false
            });

            streamRef.current = stream;
            if (videoRef.current) {
                videoRef.current.srcObject = stream;
            }

            setError(null);
        } catch (err) {
            setError('Camera access denied');
            console.error('Camera init failed:', err);
        }
    }, [config.facingMode, config.resolution]);

    useEffect(() => {
        const timer = window.setTimeout(() => {
            initCamera();
        }, 0);
        return () => {
            clearTimeout(timer);
            streamRef.current?.getTracks().forEach(t => t.stop());
        };
    }, [initCamera]);

    // ========================================================================
    // Capture
    // ========================================================================

    const sendCaptureResult = useCallback((captureId: string, success: boolean, error?: string, fileSize?: number) => {
        if (wsRef.current?.readyState === WebSocket.OPEN) {
            wsRef.current.send(JSON.stringify({
                type: 'capture_complete',
                capture_id: captureId,
                timestamp: Date.now(),
                file_size: fileSize || 0,
                success,
                error
            }));
        }
    }, []);

    const performCapture = useCallback(async (
        captureId: string,
        useFlash: boolean,
        countdownMs: number,
        quality: number
    ) => {
        // Countdown
        if (countdownMs > 0) {
            setCapture(prev => ({ ...prev, countdown: Math.ceil(countdownMs / 1000) }));
            await new Promise(r => setTimeout(r, countdownMs));
        }
        setCapture(prev => ({ ...prev, countdown: null, isCapturing: true }));

        // Flash effect
        if (useFlash && audioEnabled) {
            // Play shutter sound
            try {
                const audio = new Audio('/shutter.mp3');
                audio.volume = 0.5;
                audio.play().catch((error) => {
                    console.warn('Shutter sound failed', error);
                });
            } catch (error) {
                console.warn('Shutter sound error', error);
            }
        }

        // Capture from video
        const video = videoRef.current;
        const canvas = canvasRef.current;

        if (!video || !canvas) {
            sendCaptureResult(captureId, false, 'No video stream');
            return;
        }

        canvas.width = video.videoWidth;
        canvas.height = video.videoHeight;
        const ctx = canvas.getContext('2d');

        if (!ctx) {
            sendCaptureResult(captureId, false, 'Canvas error');
            return;
        }

        ctx.drawImage(video, 0, 0);

        // Convert to JPEG blob
        canvas.toBlob(async (blob) => {
            if (!blob) {
                sendCaptureResult(captureId, false, 'Capture failed');
                return;
            }

            // Send capture complete message
            sendCaptureResult(captureId, true, undefined, blob.size);

            // Upload image
            if (wsRef.current?.readyState === WebSocket.OPEN) {
                const buffer = await blob.arrayBuffer();
                wsRef.current.send(buffer);
            }

            setCapture(prev => ({
                ...prev,
                isCapturing: false,
                captureCount: prev.captureCount + 1,
                lastCaptureTime: Date.now()
            }));
        }, 'image/jpeg', quality / 100);
    }, [audioEnabled, sendCaptureResult]);

    // ========================================================================
    // Message Handling
    // ========================================================================

    const handleServerMessage = useCallback((msg: WsMessageType) => {
        switch (msg.type) {
            case 'registered':
                setConnection(prev => ({
                    ...prev,
                    sessionId: msg.session_id,
                    lastPing: msg.server_time
                }));
                break;

            case 'capture':
                performCapture(msg.capture_id, msg.flash, msg.countdown_ms, msg.quality);
                break;

            case 'set_resolution':
                setConfig(prev => ({
                    ...prev,
                    resolution: { width: msg.width, height: msg.height }
                }));
                break;

            case 'set_flash':
                setConfig(prev => ({ ...prev, flash: msg.enabled }));
                break;

            case 'ping':
                setConnection(prev => ({ ...prev, lastPing: Date.now() }));
                wsRef.current?.send(JSON.stringify({ type: 'pong' }));
                break;
        }
    }, [performCapture]);

    // ========================================================================
    // WebSocket Connection
    // ========================================================================

    const connectToServer = useCallback(() => {
        const wsUrl = connection.serverUrl.replace('http', 'ws') + '/api/phones/ws';

        try {
            const ws = new WebSocket(wsUrl);
            wsRef.current = ws;

            ws.onopen = () => {
                setConnection(prev => ({ ...prev, connected: true }));

                // Register with server
                ws.send(JSON.stringify({
                    type: 'register',
                    name: config.name,
                    device_info: navigator.userAgent,
                    resolution: [config.resolution.width, config.resolution.height],
                    battery: battery
                }));
            };

            ws.onclose = () => {
                setConnection(prev => ({ ...prev, connected: false, sessionId: null }));
                // Auto-reconnect after 3 seconds
                setTimeout(() => connectToServerRef.current(), 3000);
            };

            ws.onerror = () => {
                setError('Connection failed');
            };

            ws.onmessage = (event) => {
                try {
                    const msg = JSON.parse(event.data) as WsMessageType;
                    handleServerMessage(msg);
                } catch (e) {
                    console.error('Invalid message:', e);
                }
            };
        } catch (e) {
            console.error('WebSocket error:', e);
            setTimeout(() => connectToServerRef.current(), 3000);
        }
    }, [connection.serverUrl, config.name, config.resolution, battery, handleServerMessage]);

    useEffect(() => {
        connectToServerRef.current = connectToServer;
    }, [connectToServer]);

    useEffect(() => {
        connectToServer();
        return () => {
            wsRef.current?.close();
        };
    }, [connectToServer]);

    // ========================================================================
    // Ready Toggle
    // ========================================================================

    const toggleReady = () => {
        const newReady = !isReady;
        setIsReady(newReady);

        if (wsRef.current?.readyState === WebSocket.OPEN) {
            wsRef.current.send(JSON.stringify({
                type: 'ready',
                ready: newReady
            }));
        }
    };

    // ========================================================================
    // Battery Monitor
    // ========================================================================

    useEffect(() => {
        const updateBattery = async () => {
            const nav = navigator as NavigatorWithBattery;
            if (nav.getBattery) {
                try {
                    const batt = await nav.getBattery();
                    setBattery(Math.round(batt.level * 100));

                    batt.addEventListener('levelchange', () => {
                        setBattery(Math.round(batt.level * 100));
                    });
                } catch (error) {
                    console.warn('Battery API unavailable', error);
                }
            }
        };
        updateBattery();
    }, []);

    // ========================================================================
    // Wake Lock
    // ========================================================================

    useEffect(() => {
        let wakeLock: WakeLockSentinelLike | null = null;

        const requestWakeLock = async () => {
            const nav = navigator as NavigatorWithBattery;
            if (nav.wakeLock?.request) {
                try {
                    wakeLock = await nav.wakeLock.request('screen');
                } catch (error) {
                    console.warn('Wake lock request failed', error);
                }
            }
        };

        requestWakeLock();

        return () => {
            wakeLock?.release();
        };
    }, []);

    // ========================================================================
    // Render
    // ========================================================================

    return (
        <div className="slave-phone">
            <ThemeToggleFloating />
            {/* Header */}
            <header className="slave-phone__header">
                <div className="slave-phone__status">
                    {connection.connected ? (
                        <Wifi className="text-green-500" size={20} />
                    ) : (
                        <WifiOff className="text-red-500" size={20} />
                    )}
                    <span className="text-sm">
                        {connection.connected ? 'Connected' : 'Disconnected'}
                    </span>
                </div>

                <div className="slave-phone__name">{config.name}</div>

                <div className="slave-phone__battery">
                    <Battery size={20} />
                    <span>{battery}%</span>
                </div>
            </header>

            {/* Camera Preview */}
            <div className="slave-phone__preview">
                <video
                    ref={videoRef}
                    autoPlay
                    playsInline
                    muted
                    className="slave-phone__video"
                />
                <canvas ref={canvasRef} style={{ display: 'none' }} />

                {/* Countdown Overlay */}
                {capture.countdown !== null && (
                    <div className="slave-phone__countdown">
                        {capture.countdown}
                    </div>
                )}

                {/* Capture Flash */}
                {capture.isCapturing && (
                    <div className="slave-phone__flash" />
                )}

                {/* Session ID */}
                {connection.sessionId && (
                    <div className="slave-phone__session">
                        ID: {connection.sessionId.slice(0, 8)}
                    </div>
                )}
            </div>

            {/* Stats */}
            <div className="slave-phone__stats">
                <div className="stat">
                    <Camera size={16} />
                    <span>{capture.captureCount} captures</span>
                </div>
                <div className="stat">
                    <span>{config.resolution.width}×{config.resolution.height}</span>
                </div>
            </div>

            {/* Controls */}
            <div className="slave-phone__controls">
                <button
                    className="slave-phone__btn secondary"
                    onClick={() => setShowSettings(!showSettings)}
                >
                    <Settings size={24} />
                </button>

                <button
                    className={`slave-phone__ready-btn ${isReady ? 'ready' : 'not-ready'}`}
                    onClick={toggleReady}
                    disabled={!connection.connected}
                >
                    {isReady ? (
                        <>
                            <Check size={32} />
                            <span>READY</span>
                        </>
                    ) : (
                        <>
                            <X size={32} />
                            <span>NOT READY</span>
                        </>
                    )}
                </button>

                <button
                    className="slave-phone__btn secondary"
                    onClick={() => setAudioEnabled(!audioEnabled)}
                >
                    {audioEnabled ? <Volume2 size={24} /> : <VolumeX size={24} />}
                </button>
            </div>

            {/* Settings Panel */}
            {showSettings && (
                <div className="slave-phone__settings">
                    <h3>Settings</h3>

                    <label>
                        Camera Name
                        <input
                            type="text"
                            value={config.name}
                            onChange={(e) => setConfig(prev => ({ ...prev, name: e.target.value }))}
                        />
                    </label>

                    <label>
                        Camera
                        <select
                            value={config.facingMode}
                            onChange={(e) => setConfig(prev => ({
                                ...prev,
                                facingMode: e.target.value as 'user' | 'environment'
                            }))}
                        >
                            <option value="environment">Back Camera</option>
                            <option value="user">Front Camera</option>
                        </select>
                    </label>

                    <label>
                        Resolution
                        <select
                            value={`${config.resolution.width}x${config.resolution.height}`}
                            onChange={(e) => {
                                const [w, h] = e.target.value.split('x').map(Number);
                                setConfig(prev => ({ ...prev, resolution: { width: w, height: h } }));
                            }}
                        >
                            <option value="3840x2160">4K (3840×2160)</option>
                            <option value="1920x1080">1080p (1920×1080)</option>
                            <option value="1280x720">720p (1280×720)</option>
                        </select>
                    </label>

                    <label>
                        Quality: {config.quality}%
                        <input
                            type="range"
                            min={50}
                            max={100}
                            value={config.quality}
                            onChange={(e) => setConfig(prev => ({ ...prev, quality: Number(e.target.value) }))}
                        />
                    </label>

                    <label>
                        Server URL
                        <input
                            type="text"
                            value={connection.serverUrl}
                            onChange={(e) => setConnection(prev => ({ ...prev, serverUrl: e.target.value }))}
                        />
                    </label>

                    <button onClick={() => setShowSettings(false)}>Close</button>
                </div>
            )}

            {/* Error */}
            {error && (
                <div className="slave-phone__error">
                    {error}
                </div>
            )}

            <style>{`
                .slave-phone {
                    display: flex;
                    flex-direction: column;
                    min-height: 100vh;
                    background: linear-gradient(
                      180deg,
                      color-mix(in srgb, var(--ts-background) 92%, var(--ts-accent-blue)) 0%,
                      color-mix(in srgb, var(--ts-background) 88%, var(--ts-accent-purple)) 100%
                    );
                    color: var(--ts-text);
                }
                
                .slave-phone__header {
                    display: flex;
                    justify-content: space-between;
                    align-items: center;
                    padding: 1rem;
                    background: color-mix(in srgb, var(--ts-overlay) 70%, transparent);
                }
                
                .slave-phone__status {
                    display: flex;
                    align-items: center;
                    gap: 0.5rem;
                }
                
                .slave-phone__name {
                    font-weight: 600;
                }
                
                .slave-phone__battery {
                    display: flex;
                    align-items: center;
                    gap: 0.25rem;
                    font-size: 0.875rem;
                }
                
                .slave-phone__preview {
                    flex: 1;
                    position: relative;
                    background: var(--ts-preview-bg);
                    display: flex;
                    align-items: center;
                    justify-content: center;
                }
                
                .slave-phone__video {
                    width: 100%;
                    height: 100%;
                    object-fit: cover;
                }
                
                .slave-phone__countdown {
                    position: absolute;
                    inset: 0;
                    display: flex;
                    align-items: center;
                    justify-content: center;
                    font-size: 8rem;
                    font-weight: 700;
                    color: var(--ts-text-on-accent);
                    background: color-mix(in srgb, var(--ts-overlay-strong) 90%, transparent);
                    animation: pulse 1s ease-in-out infinite;
                }
                
                .slave-phone__flash {
                    position: absolute;
                    inset: 0;
                    background: var(--ts-text-on-accent);
                    animation: flash 0.15s ease-out;
                }
                
                @keyframes flash {
                    from { opacity: 1; }
                    to { opacity: 0; }
                }
                
                @keyframes pulse {
                    0%, 100% { transform: scale(1); opacity: 1; }
                    50% { transform: scale(1.1); opacity: 0.8; }
                }
                
                .slave-phone__session {
                    position: absolute;
                    bottom: 1rem;
                    left: 1rem;
                    font-size: 0.75rem;
                    background: color-mix(in srgb, var(--ts-overlay-strong) 85%, transparent);
                    padding: 0.25rem 0.5rem;
                    border-radius: 4px;
                    font-family: monospace;
                }
                
                .slave-phone__stats {
                    display: flex;
                    justify-content: center;
                    gap: 2rem;
                    padding: 0.75rem;
                    background: color-mix(in srgb, var(--ts-overlay) 60%, transparent);
                }
                
                .slave-phone__stats .stat {
                    display: flex;
                    align-items: center;
                    gap: 0.5rem;
                    font-size: 0.875rem;
                    opacity: 0.8;
                }
                
                .slave-phone__controls {
                    display: flex;
                    justify-content: center;
                    align-items: center;
                    gap: 1rem;
                    padding: 1.5rem;
                    background: linear-gradient(to top, var(--ts-background), transparent);
                }
                
                .slave-phone__btn {
                    width: 48px;
                    height: 48px;
                    border-radius: 50%;
                    border: none;
                    display: flex;
                    align-items: center;
                    justify-content: center;
                    cursor: pointer;
                    transition: all 0.2s;
                }
                
                .slave-phone__btn.secondary {
                    background: color-mix(in srgb, var(--ts-text) 10%, transparent);
                    color: var(--ts-text);
                }
                
                .slave-phone__ready-btn {
                    width: 140px;
                    height: 70px;
                    border-radius: 12px;
                    border: none;
                    display: flex;
                    flex-direction: column;
                    align-items: center;
                    justify-content: center;
                    gap: 0.25rem;
                    font-weight: 600;
                    cursor: pointer;
                    transition: all 0.3s;
                }
                
                .slave-phone__ready-btn.ready {
                    background: linear-gradient(135deg, #10b981, #059669);
                    color: var(--ts-text-on-accent);
                    box-shadow: 0 4px 20px rgba(16,185,129,0.4);
                }
                
                .slave-phone__ready-btn.not-ready {
                    background: color-mix(in srgb, var(--ts-text) 10%, transparent);
                    color: color-mix(in srgb, var(--ts-text) 60%, transparent);
                }
                
                .slave-phone__ready-btn:disabled {
                    opacity: 0.5;
                    cursor: not-allowed;
                }
                
                .slave-phone__settings {
                    position: fixed;
                    inset: 0;
                    background: var(--ts-background);
                    padding: 2rem;
                    overflow-y: auto;
                    z-index: 100;
                }
                
                .slave-phone__settings h3 {
                    margin: 0 0 1.5rem;
                    font-size: 1.5rem;
                }
                
                .slave-phone__settings label {
                    display: block;
                    margin-bottom: 1rem;
                    font-size: 0.875rem;
                    opacity: 0.7;
                }
                
                .slave-phone__settings input,
                .slave-phone__settings select {
                    display: block;
                    width: 100%;
                    margin-top: 0.5rem;
                    padding: 0.75rem;
                    border-radius: 8px;
                    border: 1px solid var(--ts-border-strong);
                    background: color-mix(in srgb, var(--ts-text) 10%, transparent);
                    color: var(--ts-text);
                    font-size: 1rem;
                }
                
                .slave-phone__settings button {
                    width: 100%;
                    margin-top: 2rem;
                    padding: 1rem;
                    border-radius: 8px;
                    border: none;
                    background: var(--ts-accent-blue);
                    color: var(--ts-text-on-accent);
                    font-size: 1rem;
                    font-weight: 600;
                    cursor: pointer;
                }
                
                .slave-phone__error {
                    position: fixed;
                    bottom: 100px;
                    left: 50%;
                    transform: translateX(-50%);
                    background: #ef4444;
                    padding: 0.75rem 1.5rem;
                    border-radius: 8px;
                    font-size: 0.875rem;
                }
            `}</style>
        </div>
    );
}

export default SlavePhone;
