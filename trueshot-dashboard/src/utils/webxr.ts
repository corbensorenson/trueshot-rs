/**
 * WebXR Scanning Module
 * 
 * Provides VR/AR headset scanning capabilities for TrueShot.
 * Supports three modes:
 * - Object Mode: Scan individual objects by walking around them
 * - Portion Mode: Scan a section of a room (table, desk, shelf)
 * - Room Mode: Full room walkthrough scanning
 */

import { useRef, useCallback, useState, useEffect } from 'react';

// ============================================================================
// Types
// ============================================================================

export type ScanMode = 'object' | 'portion' | 'room';
export type XRSessionState = 'idle' | 'requesting' | 'active' | 'scanning' | 'processing' | 'error';

export interface XRCapabilities {
    immersiveVR: boolean;
    immersiveAR: boolean;
    handTracking: boolean;
    depthSensing: boolean;
    rawCameraAccess: boolean;
    hitTest: boolean;
    anchors: boolean;
    meshDetection: boolean;
}

export interface ScanBounds {
    center: { x: number; y: number; z: number };
    size: { width: number; height: number; depth: number };
    rotation: number;
}

export interface CapturedFrame {
    timestamp: number;
    cameraPosition: { x: number; y: number; z: number };
    cameraRotation: { x: number; y: number; z: number; w: number };
    imageData?: ImageData;
    depthData?: Float32Array;
}

export interface ScanProgress {
    framesCapture: number;
    coveragePercent: number;
    quality: 'low' | 'medium' | 'high' | 'excellent';
    estimatedTimeRemaining: number;
}

export interface XRScanSession {
    id: string;
    mode: ScanMode;
    state: XRSessionState;
    bounds: ScanBounds | null;
    frames: CapturedFrame[];
    progress: ScanProgress;
    startedAt: Date;
}

// ============================================================================
// WebXR Session Manager
// ============================================================================

export class WebXRSessionManager {
    private xrSession: XRSession | null = null;
    private referenceSpace: XRReferenceSpace | null = null;
    private capabilities: XRCapabilities | null = null;
    private scanSession: XRScanSession | null = null;
    private frameCallbacks: ((frame: CapturedFrame) => void)[] = [];
    private stateCallbacks: ((state: XRSessionState) => void)[] = [];

    /**
     * Check WebXR support and capabilities
     */
    async checkCapabilities(): Promise<XRCapabilities> {
        const nav = navigator as Navigator & { xr?: XRSystem };
        const xr = nav.xr;

        if (!xr) {
            throw new Error('WebXR not supported in this browser');
        }

        const capabilities: XRCapabilities = {
            immersiveVR: await xr.isSessionSupported('immersive-vr').catch(() => false),
            immersiveAR: await xr.isSessionSupported('immersive-ar').catch(() => false),
            handTracking: false, // Checked after session start
            depthSensing: false,
            rawCameraAccess: false,
            hitTest: false,
            anchors: false,
            meshDetection: false,
        };

        this.capabilities = capabilities;
        return capabilities;
    }

    /**
     * Start a WebXR session for scanning
     */
    async startSession(mode: ScanMode): Promise<XRScanSession> {
        if (!this.capabilities) {
            await this.checkCapabilities();
        }

        const nav = navigator as Navigator & { xr?: XRSystem };
        const xr = nav.xr;
        if (!xr) {
            throw new Error('WebXR not supported in this browser');
        }

        // Determine session type based on mode
        const sessionType = mode === 'room' ? 'immersive-ar' : 'immersive-vr';

        if (sessionType === 'immersive-ar' && !this.capabilities?.immersiveAR) {
            throw new Error('Immersive AR not supported');
        }
        if (sessionType === 'immersive-vr' && !this.capabilities?.immersiveVR) {
            throw new Error('Immersive VR not supported');
        }

        // Required features based on mode
        const requiredFeatures: string[] = ['local-floor'];
        const optionalFeatures: string[] = ['hand-tracking', 'anchors'];

        if (mode === 'room') {
            requiredFeatures.push('unbounded');
            optionalFeatures.push('depth-sensing', 'mesh-detection');
        }

        this.notifyState('requesting');

        try {
            this.xrSession = await xr.requestSession(sessionType, {
                requiredFeatures,
                optionalFeatures,
            });

            // Get reference space
            const refSpaceType = mode === 'room' ? 'unbounded' : 'local-floor';
            this.referenceSpace = await this.xrSession.requestReferenceSpace(refSpaceType)
                .catch(() => this.xrSession!.requestReferenceSpace('local-floor'));

            // Update capabilities based on session
            this.updateCapabilitiesFromSession();

            // Create scan session
            this.scanSession = {
                id: crypto.randomUUID(),
                mode,
                state: 'active',
                bounds: null,
                frames: [],
                progress: {
                    framesCapture: 0,
                    coveragePercent: 0,
                    quality: 'low',
                    estimatedTimeRemaining: 300,
                },
                startedAt: new Date(),
            };

            // Start frame loop
            this.xrSession.requestAnimationFrame(this.onXRFrame.bind(this));

            // Handle session end
            this.xrSession.addEventListener('end', () => {
                this.notifyState('idle');
                this.xrSession = null;
                this.referenceSpace = null;
            });

            this.notifyState('active');
            return this.scanSession;

        } catch (error) {
            this.notifyState('error');
            throw error;
        }
    }

    /**
     * Set scan bounds (for object/portion modes)
     */
    setBounds(bounds: ScanBounds): void {
        if (this.scanSession) {
            this.scanSession.bounds = bounds;
        }
    }

    /**
     * Start capturing frames
     */
    startCapture(): void {
        if (this.scanSession) {
            this.scanSession.state = 'scanning';
            this.notifyState('scanning');
        }
    }

    /**
     * Stop capturing and process
     */
    async stopCapture(): Promise<CapturedFrame[]> {
        if (!this.scanSession) {
            return [];
        }

        this.scanSession.state = 'processing';
        this.notifyState('processing');

        return this.scanSession.frames;
    }

    /**
     * End the XR session
     */
    async endSession(): Promise<void> {
        if (this.xrSession) {
            await this.xrSession.end();
        }
        this.scanSession = null;
        this.xrSession = null;
        this.referenceSpace = null;
        this.notifyState('idle');
    }

    /**
     * Subscribe to frame captures
     */
    onFrame(callback: (frame: CapturedFrame) => void): () => void {
        this.frameCallbacks.push(callback);
        return () => {
            this.frameCallbacks = this.frameCallbacks.filter(cb => cb !== callback);
        };
    }

    /**
     * Subscribe to state changes
     */
    onStateChange(callback: (state: XRSessionState) => void): () => void {
        this.stateCallbacks.push(callback);
        return () => {
            this.stateCallbacks = this.stateCallbacks.filter(cb => cb !== callback);
        };
    }

    /**
     * Get current session
     */
    getSession(): XRScanSession | null {
        return this.scanSession;
    }

    /**
     * Get capabilities
     */
    getCapabilities(): XRCapabilities | null {
        return this.capabilities;
    }

    // ============================================================================
    // Private Methods
    // ============================================================================

    private updateCapabilitiesFromSession(): void {
        if (!this.xrSession || !this.capabilities) return;

        // Check for hand tracking
        this.capabilities.handTracking = 'inputSources' in this.xrSession;

        // Check for depth sensing (if available)
        this.capabilities.depthSensing = !!this.xrSession.depthUsage;
    }

    private onXRFrame(time: DOMHighResTimeStamp, frame: XRFrame): void {
        if (!this.xrSession || !this.referenceSpace) return;

        // Continue the frame loop
        this.xrSession.requestAnimationFrame(this.onXRFrame.bind(this));

        // Only capture if in scanning state
        if (this.scanSession?.state !== 'scanning') return;

        // Get pose
        const pose = frame.getViewerPose(this.referenceSpace);
        if (!pose) return;

        const position = pose.transform.position;
        const orientation = pose.transform.orientation;

        // Create captured frame
        const capturedFrame: CapturedFrame = {
            timestamp: time,
            cameraPosition: { x: position.x, y: position.y, z: position.z },
            cameraRotation: {
                x: orientation.x,
                y: orientation.y,
                z: orientation.z,
                w: orientation.w
            },
        };

        // Store frame
        this.scanSession.frames.push(capturedFrame);
        this.scanSession.progress.framesCapture++;

        // Update coverage estimation
        this.updateCoverageEstimate();

        // Notify listeners
        this.frameCallbacks.forEach(cb => cb(capturedFrame));
    }

    private updateCoverageEstimate(): void {
        if (!this.scanSession) return;

        const frames = this.scanSession.frames;

        // Simple coverage estimation based on unique viewing angles
        const angleResolution = 15; // degrees
        const uniqueAngles = new Set<string>();

        for (const frame of frames) {
            const { x, y, z, w } = frame.cameraRotation;
            // Convert quaternion to euler angles (simplified)
            const yaw = Math.atan2(2 * (w * y + x * z), 1 - 2 * (y * y + z * z));
            const pitch = Math.asin(2 * (w * x - z * y));

            const yawBucket = Math.floor((yaw * 180 / Math.PI) / angleResolution);
            const pitchBucket = Math.floor((pitch * 180 / Math.PI) / angleResolution);
            uniqueAngles.add(`${yawBucket},${pitchBucket}`);
        }

        // Assume 360° horizontal, 90° vertical coverage needed
        const totalBuckets = (360 / angleResolution) * (90 / angleResolution);
        const coverage = Math.min((uniqueAngles.size / totalBuckets) * 100, 100);

        this.scanSession.progress.coveragePercent = coverage;
        this.scanSession.progress.quality =
            coverage < 25 ? 'low' :
                coverage < 50 ? 'medium' :
                    coverage < 75 ? 'high' : 'excellent';
    }

    private notifyState(state: XRSessionState): void {
        this.stateCallbacks.forEach(cb => cb(state));
    }
}

// ============================================================================
// React Hooks
// ============================================================================

/**
 * Hook for WebXR scanning functionality
 */
export function useWebXRScanning() {
    const managerRef = useRef<WebXRSessionManager | null>(null);
    const [state, setState] = useState<XRSessionState>('idle');
    const [capabilities, setCapabilities] = useState<XRCapabilities | null>(null);
    const [session, setSession] = useState<XRScanSession | null>(null);
    const [error, setError] = useState<string | null>(null);

    // Initialize manager
    useEffect(() => {
        const manager = new WebXRSessionManager();
        managerRef.current = manager;

        // Subscribe to state changes
        const unsubscribe = manager.onStateChange(setState);

        // Check capabilities
        manager.checkCapabilities()
            .then(setCapabilities)
            .catch(err => setError(err instanceof Error ? err.message : 'WebXR capability check failed'));

        return () => {
            unsubscribe();
            manager.endSession();
        };
    }, []);

    const startScan = useCallback(async (mode: ScanMode) => {
        if (!managerRef.current) return;

        try {
            setError(null);
            const newSession = await managerRef.current.startSession(mode);
            setSession(newSession);
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Failed to start XR session');
        }
    }, []);

    const setBounds = useCallback((bounds: ScanBounds) => {
        managerRef.current?.setBounds(bounds);
    }, []);

    const startCapture = useCallback(() => {
        managerRef.current?.startCapture();
    }, []);

    const stopCapture = useCallback(async () => {
        return managerRef.current?.stopCapture() || [];
    }, []);

    const endSession = useCallback(async () => {
        await managerRef.current?.endSession();
        setSession(null);
    }, []);

    const onFrame = useCallback((callback: (frame: CapturedFrame) => void) => {
        return managerRef.current?.onFrame(callback) || (() => { });
    }, []);

    return {
        state,
        capabilities,
        session,
        error,
        isSupported: capabilities?.immersiveVR || capabilities?.immersiveAR,
        startScan,
        setBounds,
        startCapture,
        stopCapture,
        endSession,
        onFrame,
    };
}

// ============================================================================
// Singleton for global access
// ============================================================================

let globalManager: WebXRSessionManager | null = null;

export function getWebXRManager(): WebXRSessionManager {
    if (!globalManager) {
        globalManager = new WebXRSessionManager();
    }
    return globalManager;
}
