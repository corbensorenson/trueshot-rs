/**
 * WebXR Advanced Scanning - State-of-the-Art Implementation
 * 
 * Production-quality WebXR scanning with:
 * - Quest 3 Depth API integration for real-time depth sensing
 * - Vision Pro gaze+pinch input handling
 * - Hit testing for precise object placement
 * - Real-time point cloud preview
 * - Adaptive frame rate management
 */

import { useRef, useCallback, useState } from 'react';

// ============================================================================
// Advanced Types
// ============================================================================

export interface DepthData {
    width: number;
    height: number;
    data: Float32Array;
    normDepthBufferFromNormView: Float32Array;  // 4x4 matrix
}

export interface HitTestResult {
    position: [number, number, number];
    normal: [number, number, number];
    distance: number;
}

export interface HandJoint {
    position: [number, number, number];
    radius: number;
}

export interface HandData {
    wrist: HandJoint;
    thumb: HandJoint[];
    index: HandJoint[];
    middle: HandJoint[];
    ring: HandJoint[];
    pinky: HandJoint[];
    isPinching: boolean;
    pinchStrength: number;
}

export interface PointCloudPoint {
    x: number;
    y: number;
    z: number;
    r: number;
    g: number;
    b: number;
    confidence: number;
}

export interface AdaptiveConfig {
    targetFrameRate: number;
    minFrameRate: number;
    maxPointsPerFrame: number;
    qualityLevel: 'low' | 'medium' | 'high' | 'ultra';
}

// ============================================================================
// Quest 3 Depth API Integration
// ============================================================================

export class Quest3DepthSensor {
    private depthInfo: XRDepthInformation | null = null;
    private depthFormat: 'luminance-alpha' | 'float32' = 'float32';

    /**
     * Request depth sensing feature
     */
    static getSessionOptions(): XRSessionInit {
        return {
            requiredFeatures: ['local-floor'],
            optionalFeatures: [
                'depth-sensing',
                'hand-tracking',
                'hit-test',
                'anchors',
                'mesh-detection',
            ],
            depthSensing: {
                usagePreference: ['cpu-optimized', 'gpu-optimized'],
                dataFormatPreference: ['float32', 'luminance-alpha'],
            },
        } as XRSessionInit;
    }

    /**
     * Get depth data from current frame
     */
    getDepth(frame: XRFrame, view: XRView): DepthData | null {
        try {
            const depthInfo = frame.getDepthInformation?.(view);
            if (!depthInfo) return null;

            this.depthInfo = depthInfo;

            // Get depth values
            const width = depthInfo.width;
            const height = depthInfo.height;
            const data = new Float32Array(width * height);

            // Sample depth at each pixel
            for (let y = 0; y < height; y++) {
                for (let x = 0; x < width; x++) {
                    const u = x / (width - 1);
                    const v = y / (height - 1);
                    data[y * width + x] = depthInfo.getDepthInMeters(u, v);
                }
            }

            return {
                width,
                height,
                data,
                normDepthBufferFromNormView: new Float32Array(depthInfo.normDepthBufferFromNormView.matrix),
            };
        } catch (error) {
            console.warn('Depth read failed', error);
            return null;
        }
    }

    /**
     * Convert depth to point cloud
     */
    depthToPoints(
        depth: DepthData,
        view: XRView,
        maxPoints: number = 10000
    ): PointCloudPoint[] {
        const points: PointCloudPoint[] = [];
        const { width, height, data } = depth;

        // Get inverse projection
        const proj = view.projectionMatrix;

        // Sample every Nth pixel to limit points
        const step = Math.ceil(Math.sqrt((width * height) / maxPoints));

        for (let y = 0; y < height; y += step) {
            for (let x = 0; x < width; x += step) {
                const d = data[y * width + x];

                // Skip invalid depths
                if (d <= 0 || d > 10 || !isFinite(d)) continue;

                // Convert to normalized device coordinates
                const ndcX = (x / width) * 2 - 1;
                const ndcY = (y / height) * 2 - 1;

                // Unproject (simplified)
                const worldX = ndcX * d / proj[0];
                const worldY = -ndcY * d / proj[5];
                const worldZ = -d;

                points.push({
                    x: worldX,
                    y: worldY,
                    z: worldZ,
                    r: 128,
                    g: 128,
                    b: 128,
                    confidence: 1.0 - (d / 10),  // Confidence decreases with distance
                });
            }
        }

        return points;
    }
}

// ============================================================================
// Vision Pro Input Handler
// ============================================================================

export class VisionProInputHandler {
    private lastGazeTarget: [number, number, number] | null = null;
    private isPinching = false;
    private pinchStartTime = 0;

    /**
     * Process Vision Pro gaze+pinch input
     */
    processInput(
        frame: XRFrame,
        referenceSpace: XRReferenceSpace
    ): { gazePoint: [number, number, number] | null; action: 'none' | 'select' | 'hold' } {
        const gazeResults = (frame as unknown as { getHitTestResultsForTransientInput?: () => XRTransientInputHitTestResult[] })
            .getHitTestResultsForTransientInput?.();

        let gazePoint: [number, number, number] | null = null;

        if (gazeResults && gazeResults.length > 0) {
            const result = gazeResults[0];
            const pose = (result as unknown as { getPose?: (space: XRReferenceSpace) => XRPose | undefined; results?: Array<{ getPose?: (space: XRReferenceSpace) => XRPose | undefined }> })
                .getPose?.(referenceSpace) ?? result.results?.[0]?.getPose?.(referenceSpace);
            if (pose) {
                gazePoint = [
                    pose.transform.position.x,
                    pose.transform.position.y,
                    pose.transform.position.z,
                ];
                this.lastGazeTarget = gazePoint;
            }
        }

        // Check for pinch gesture via input sources
        let action: 'none' | 'select' | 'hold' = 'none';

        const session = frame.session;
        for (const source of session.inputSources) {
            if (source.hand) {
                const handData = this.getHandData(source.hand, frame, referenceSpace);
                if (handData.isPinching && !this.isPinching) {
                    this.isPinching = true;
                    this.pinchStartTime = performance.now();
                    action = 'select';
                } else if (handData.isPinching && this.isPinching) {
                    if (performance.now() - this.pinchStartTime > 500) {
                        action = 'hold';
                    }
                } else if (!handData.isPinching && this.isPinching) {
                    this.isPinching = false;
                }
            }
        }

        return { gazePoint, action };
    }

    /**
     * Extract hand joint data
     */
    getHandData(
        hand: XRHand,
        frame: XRFrame,
        referenceSpace: XRReferenceSpace
    ): HandData {
        const getJoint = (jointName: string): HandJoint => {
            const joint = hand.get(jointName as XRHandJoint);
            if (!joint) return { position: [0, 0, 0], radius: 0 };

            const pose = frame.getJointPose?.(joint, referenceSpace);
            if (!pose) return { position: [0, 0, 0], radius: 0 };

            return {
                position: [
                    pose.transform.position.x,
                    pose.transform.position.y,
                    pose.transform.position.z,
                ],
                radius: pose.radius ?? 0.01,
            };
        };

        const thumbTip = getJoint('thumb-tip');
        const indexTip = getJoint('index-finger-tip');

        // Calculate pinch
        const pinchDistance = Math.sqrt(
            Math.pow(thumbTip.position[0] - indexTip.position[0], 2) +
            Math.pow(thumbTip.position[1] - indexTip.position[1], 2) +
            Math.pow(thumbTip.position[2] - indexTip.position[2], 2)
        );

        const isPinching = pinchDistance < 0.03;  // 3cm threshold
        const pinchStrength = Math.max(0, 1 - pinchDistance / 0.05);

        return {
            wrist: getJoint('wrist'),
            thumb: ['thumb-metacarpal', 'thumb-phalanx-proximal', 'thumb-phalanx-distal', 'thumb-tip'].map(getJoint),
            index: ['index-finger-metacarpal', 'index-finger-phalanx-proximal', 'index-finger-phalanx-intermediate', 'index-finger-phalanx-distal', 'index-finger-tip'].map(getJoint),
            middle: ['middle-finger-metacarpal', 'middle-finger-phalanx-proximal', 'middle-finger-phalanx-intermediate', 'middle-finger-phalanx-distal', 'middle-finger-tip'].map(getJoint),
            ring: ['ring-finger-metacarpal', 'ring-finger-phalanx-proximal', 'ring-finger-phalanx-intermediate', 'ring-finger-phalanx-distal', 'ring-finger-tip'].map(getJoint),
            pinky: ['pinky-finger-metacarpal', 'pinky-finger-phalanx-proximal', 'pinky-finger-phalanx-intermediate', 'pinky-finger-phalanx-distal', 'pinky-finger-tip'].map(getJoint),
            isPinching,
            pinchStrength,
        };
    }
}

// ============================================================================
// Hit Testing for Object Placement
// ============================================================================

export class HitTester {
    private hitTestSource: XRHitTestSource | null = null;
    private transientHitTestSource: XRTransientInputHitTestSource | null = null;

    /**
     * Initialize hit test sources
     */
    async initialize(session: XRSession): Promise<void> {
        try {
            // Viewer-space hit testing (Quest 3 Depth API)
            this.hitTestSource = (await session.requestHitTestSource?.({
                space: await session.requestReferenceSpace('viewer'),
            })) ?? null;

            // Transient hit testing for pointer/gaze input
            this.transientHitTestSource = (await session.requestHitTestSourceForTransientInput?.({
                profile: 'generic-touchscreen',
            })) ?? null;
        } catch (e) {
            console.warn('Hit testing not available:', e);
        }
    }

    /**
     * Perform hit test
     */
    hitTest(frame: XRFrame, referenceSpace: XRReferenceSpace): HitTestResult[] {
        const results: HitTestResult[] = [];

        // Viewer-based hit test (center of view)
        if (this.hitTestSource) {
            const hitResults = frame.getHitTestResults?.(this.hitTestSource);
            if (hitResults) {
                for (const result of hitResults) {
                    const pose = result.getPose(referenceSpace);
                    if (pose) {
                        const p = pose.transform.position;
                        const m = pose.transform.matrix;

                        results.push({
                            position: [p.x, p.y, p.z],
                            normal: [m[4], m[5], m[6]],  // Up vector from matrix
                            distance: Math.sqrt(p.x * p.x + p.y * p.y + p.z * p.z),
                        });
                    }
                }
            }
        }

        return results;
    }

    /**
     * Cleanup
     */
    destroy(): void {
        this.hitTestSource?.cancel();
        this.transientHitTestSource?.cancel();
        this.hitTestSource = null;
        this.transientHitTestSource = null;
    }
}

// ============================================================================
// Real-Time Point Cloud Preview
// ============================================================================

export class PointCloudAccumulator {
    private points: PointCloudPoint[] = [];
    private maxPoints: number;
    private voxelSize: number;
    private voxelGrid: Map<string, PointCloudPoint> = new Map();

    constructor(maxPoints: number = 500000, voxelSize: number = 0.01) {
        this.maxPoints = maxPoints;
        this.voxelSize = voxelSize;
    }

    /**
     * Add points from a frame
     */
    addPoints(newPoints: PointCloudPoint[]): number {
        let addedCount = 0;

        for (const point of newPoints) {
            // Voxel grid filtering for uniform density
            const voxelKey = this.getVoxelKey(point.x, point.y, point.z);

            if (!this.voxelGrid.has(voxelKey)) {
                if (this.points.length < this.maxPoints) {
                    this.points.push(point);
                    this.voxelGrid.set(voxelKey, point);
                    addedCount++;
                } else {
                    // Replace low-confidence point
                    const replaceIdx = this.findLowConfidencePoint();
                    if (replaceIdx >= 0 && point.confidence > this.points[replaceIdx].confidence) {
                        const oldPoint = this.points[replaceIdx];
                        const oldKey = this.getVoxelKey(oldPoint.x, oldPoint.y, oldPoint.z);
                        this.voxelGrid.delete(oldKey);

                        this.points[replaceIdx] = point;
                        this.voxelGrid.set(voxelKey, point);
                        addedCount++;
                    }
                }
            }
        }

        return addedCount;
    }

    private getVoxelKey(x: number, y: number, z: number): string {
        const vx = Math.floor(x / this.voxelSize);
        const vy = Math.floor(y / this.voxelSize);
        const vz = Math.floor(z / this.voxelSize);
        return `${vx},${vy},${vz}`;
    }

    private findLowConfidencePoint(): number {
        let minConfidence = 1.0;
        let minIdx = -1;

        // Sample 100 random points to find low confidence
        for (let i = 0; i < 100 && i < this.points.length; i++) {
            const idx = Math.floor(Math.random() * this.points.length);
            if (this.points[idx].confidence < minConfidence) {
                minConfidence = this.points[idx].confidence;
                minIdx = idx;
            }
        }

        return minIdx;
    }

    /**
     * Get all points
     */
    getPoints(): PointCloudPoint[] {
        return this.points;
    }

    /**
     * Get point count
     */
    getCount(): number {
        return this.points.length;
    }

    /**
     * Clear all points
     */
    clear(): void {
        this.points = [];
        this.voxelGrid.clear();
    }

    /**
     * Export to PLY format
     */
    toPLY(): Blob {
        let ply = 'ply\nformat ascii 1.0\n';
        ply += `element vertex ${this.points.length}\n`;
        ply += 'property float x\n';
        ply += 'property float y\n';
        ply += 'property float z\n';
        ply += 'property uchar red\n';
        ply += 'property uchar green\n';
        ply += 'property uchar blue\n';
        ply += 'end_header\n';

        for (const p of this.points) {
            ply += `${p.x} ${p.y} ${p.z} ${p.r} ${p.g} ${p.b}\n`;
        }

        return new Blob([ply], { type: 'text/plain' });
    }
}

// ============================================================================
// Adaptive Frame Rate Manager
// ============================================================================

export class AdaptiveFrameRateManager {
    private config: AdaptiveConfig;
    private frameTimes: number[] = [];
    private maxSamples = 30;

    constructor(config: Partial<AdaptiveConfig> = {}) {
        this.config = {
            targetFrameRate: 72,
            minFrameRate: 60,
            maxPointsPerFrame: 10000,
            qualityLevel: 'high',
            ...config,
        };
    }

    /**
     * Record frame time
     */
    recordFrame(frameTimeMs: number): void {
        this.frameTimes.push(frameTimeMs);
        if (this.frameTimes.length > this.maxSamples) {
            this.frameTimes.shift();
        }
    }

    /**
     * Get current FPS
     */
    getCurrentFPS(): number {
        if (this.frameTimes.length < 2) return this.config.targetFrameRate;
        const avgFrameTime = this.frameTimes.reduce((a, b) => a + b, 0) / this.frameTimes.length;
        return 1000 / avgFrameTime;
    }

    /**
     * Get recommended settings based on performance
     */
    getRecommendedSettings(): { maxPoints: number; skipFrames: number } {
        const fps = this.getCurrentFPS();

        // If struggling to hit target, reduce quality
        if (fps < this.config.minFrameRate) {
            return {
                maxPoints: Math.floor(this.config.maxPointsPerFrame * 0.5),
                skipFrames: 2,
            };
        } else if (fps < this.config.targetFrameRate * 0.9) {
            return {
                maxPoints: Math.floor(this.config.maxPointsPerFrame * 0.75),
                skipFrames: 1,
            };
        }

        return {
            maxPoints: this.config.maxPointsPerFrame,
            skipFrames: 0,
        };
    }

    /**
     * Update quality level
     */
    setQualityLevel(level: 'low' | 'medium' | 'high' | 'ultra'): void {
        this.config.qualityLevel = level;

        switch (level) {
            case 'low':
                this.config.maxPointsPerFrame = 2500;
                break;
            case 'medium':
                this.config.maxPointsPerFrame = 5000;
                break;
            case 'high':
                this.config.maxPointsPerFrame = 10000;
                break;
            case 'ultra':
                this.config.maxPointsPerFrame = 25000;
                break;
        }
    }
}

// ============================================================================
// Enhanced WebXR Hook
// ============================================================================

export function useAdvancedWebXRScanning() {
    const depthSensorRef = useRef<Quest3DepthSensor | null>(null);
    const inputHandlerRef = useRef<VisionProInputHandler | null>(null);
    const hitTesterRef = useRef<HitTester | null>(null);
    const pointCloudRef = useRef<PointCloudAccumulator | null>(null);
    const frameRateRef = useRef<AdaptiveFrameRateManager | null>(null);

    const [pointCount, setPointCount] = useState(0);
    const [fps, setFps] = useState(0);
    const [isDepthAvailable, setIsDepthAvailable] = useState(false);
    const [isHandTrackingAvailable, setIsHandTrackingAvailable] = useState(false);

    const initialize = useCallback((session: XRSession) => {
        depthSensorRef.current = new Quest3DepthSensor();
        inputHandlerRef.current = new VisionProInputHandler();
        hitTesterRef.current = new HitTester();
        pointCloudRef.current = new PointCloudAccumulator();
        frameRateRef.current = new AdaptiveFrameRateManager();

        hitTesterRef.current.initialize(session);

        // Check available features
        setIsDepthAvailable(!!session.depthUsage);
        setIsHandTrackingAvailable(Array.from(session.inputSources).some(s => s.hand));
    }, []);

    const processFrame = useCallback((frame: XRFrame, view: XRView, referenceSpace: XRReferenceSpace) => {
        const startTime = performance.now();

        // Get depth and convert to points
        if (depthSensorRef.current && isDepthAvailable) {
            const depth = depthSensorRef.current.getDepth(frame, view);
            if (depth) {
                const settings = frameRateRef.current?.getRecommendedSettings();
                const points = depthSensorRef.current.depthToPoints(depth, view, settings?.maxPoints);
                pointCloudRef.current?.addPoints(points);
                setPointCount(pointCloudRef.current?.getCount() ?? 0);
            }
        }

        // Process input
        const input = inputHandlerRef.current?.processInput(frame, referenceSpace);

        // Hit test
        const hits = hitTesterRef.current?.hitTest(frame, referenceSpace);

        // Track frame time
        const frameTime = performance.now() - startTime;
        frameRateRef.current?.recordFrame(frameTime);
        setFps(Math.round(frameRateRef.current?.getCurrentFPS() ?? 0));

        return { input, hits };
    }, [isDepthAvailable]);

    const getPointCloud = useCallback(() => {
        return pointCloudRef.current?.getPoints() ?? [];
    }, []);

    const exportPointCloud = useCallback(() => {
        return pointCloudRef.current?.toPLY() ?? new Blob();
    }, []);

    const cleanup = useCallback(() => {
        hitTesterRef.current?.destroy();
        pointCloudRef.current?.clear();
    }, []);

    return {
        initialize,
        processFrame,
        getPointCloud,
        exportPointCloud,
        cleanup,
        pointCount,
        fps,
        isDepthAvailable,
        isHandTrackingAvailable,
    };
}
