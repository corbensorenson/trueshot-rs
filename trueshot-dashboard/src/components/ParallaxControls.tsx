import { useEffect, useRef, useCallback } from 'react';
import { useThree } from '@react-three/fiber';
import { FaceLandmarker, FilesetResolver } from '@mediapipe/tasks-vision';
import * as THREE from 'three';

// --- 1 Euro Filter Implementation ---
// Adaptive low-pass filter for jitter reduction
class OneEuroFilter {
    minCutoff: number;
    beta: number;
    dCutoff: number;
    x: number | null;
    dx: number;
    lastTime: number | null;

    constructor(minCutoff = 1.0, beta = 0.0, dCutoff = 1.0) {
        this.minCutoff = minCutoff;
        this.beta = beta;
        this.dCutoff = dCutoff;
        this.x = null;
        this.dx = 0;
        this.lastTime = null;
    }

    alpha(cutoff: number, dt: number) {
        const tau = 1.0 / (2 * Math.PI * cutoff);
        return 1.0 / (1.0 + tau / dt);
    }

    filter(x: number, timestamp: number) {
        if (this.lastTime === null) {
            this.lastTime = timestamp;
            this.x = x;
            this.dx = 0;
            return x;
        }

        const dt = (timestamp - this.lastTime) / 1000;
        this.lastTime = timestamp;

        if (dt <= 0) return this.x!; // Should not happen

        const cutoff = this.minCutoff + this.beta * Math.abs(this.dx);
        const a = this.alpha(cutoff, dt);

        // Simple exponential smoothing
        const dxRaw = (x - this.x!) / dt;
        const a_d = this.alpha(this.dCutoff, dt);
        this.dx = this.dx + a_d * (dxRaw - this.dx);

        this.x = this.x! + a * (x - this.x!);
        return this.x!;
    }
}

export const ParallaxControls = ({ enabled = true, sensitivity = 2.0 }: { enabled?: boolean, sensitivity?: number }) => {
    const { camera } = useThree();
    const videoRef = useRef<HTMLVideoElement>(document.createElement("video"));
    const lastVideoTimeRef = useRef(-1);
    const landmarkerRef = useRef<FaceLandmarker | null>(null);
    const requestRef = useRef<number | null>(null);

    // Filters for X, Y, Z
    const filterX = useRef(new OneEuroFilter(0.5, 0.05)); // Low cutoff for stability
    const filterY = useRef(new OneEuroFilter(0.5, 0.05));

    // Screen dimensions in "virtual meters" (assumed)
    // We calibrate to standard monitor size relative to viewing distance.
    const SCREEN_WIDTH = 0.5; // 50cm
    const SCREEN_HEIGHT = 0.3; // 30cm

    const updateCameraProjection = useCallback((headX: number, headY: number, headZ: number) => {
        if (!(camera instanceof THREE.PerspectiveCamera)) return;

        // "Generalized Perspective Projection" (Kooima)
        // Screen is at Z=0. Dimensions defined above. 
        // Frustum planes:
        // left   = (pa.x - pe.x) * near / pe.z
        // right  = (pb.x - pe.x) * near / pe.z
        // bottom = (pa.y - pe.y) * near / pe.z
        // top    = (pc.y - pe.y) * near / pe.z

        const near = camera.near;
        const far = camera.far;

        // Define Screen Corners in World Space (assuming camera parent is at origin looking -Z)
        // Wait, standard Kooima assumes screen is fixed and user moves.
        // In ThreeJS, we move the camera.
        // If we move camera physically to `headX, headY, headZ`:
        // We look at (0,0,0).
        // But `lookAt` rotates the view plane. We want the view plane to stay PARALLEL to Z=0.
        // So we do NOT rotate. Rotation = Identity.
        // We only Translate.

        camera.rotation.set(0, 0, 0);
        camera.position.set(headX, headY, headZ);
        camera.updateMatrixWorld();

        // Calculate Frustum Offsets relative to camera position
        // Screen half-sizes
        const hw = SCREEN_WIDTH / 2.0; // 0.25
        const hh = SCREEN_HEIGHT / 2.0; // 0.15

        // Distance from eye to screen plane (Screen is at Z=0)
        // Eye is at headZ. dist = headZ - 0 = headZ.
        const dist = headZ;

        // Frustum bounds at Near plane
        const scale = near / dist;

        const left = (-hw - headX) * scale;
        const right = (hw - headX) * scale;
        const bottom = (-hh - headY) * scale;
        const top = (hh - headY) * scale;

        camera.projectionMatrix.makePerspective(left, right, top, bottom, near, far);
    }, [camera]);

    useEffect(() => {
        let isActive = true;
        const videoEl = videoRef.current;

        const runLoop = () => {
            if (!enabled || !isActive) return;

            if (videoEl && videoEl.currentTime !== lastVideoTimeRef.current) {
                lastVideoTimeRef.current = videoEl.currentTime;

                if (landmarkerRef.current) {
                    const now = performance.now();
                    const result = landmarkerRef.current.detectForVideo(videoEl, now);

                    if (result.faceLandmarks && result.faceLandmarks.length > 0) {
                        const landmarks = result.faceLandmarks[0];
                        const nose = landmarks[1];

                        const rawX = (0.5 - nose.x) * sensitivity;
                        const rawY = (0.5 - nose.y) * sensitivity * (9 / 16);

                        const x = filterX.current.filter(rawX, now);
                        const y = filterY.current.filter(rawY, now);

                        updateCameraProjection(x, y, 2.0);
                        camera.position.set(x, y, 2.0);
                    }
                }
            }
            requestRef.current = requestAnimationFrame(runLoop);
        };

        const init = async () => {
            try {
                const vision = await FilesetResolver.forVisionTasks(
                    "https://cdn.jsdelivr.net/npm/@mediapipe/tasks-vision@0.10.3/wasm"
                );

                landmarkerRef.current = await FaceLandmarker.createFromOptions(vision, {
                    baseOptions: {
                        modelAssetPath: `https://storage.googleapis.com/mediapipe-models/face_landmarker/face_landmarker/float16/1/face_landmarker.task`,
                        delegate: "GPU"
                    },
                    outputFaceBlendshapes: false,
                    outputFacialTransformationMatrixes: false,
                    runningMode: "VIDEO",
                    numFaces: 1
                });

                const stream = await navigator.mediaDevices.getUserMedia({ video: { width: 640, height: 480 } });
                videoEl.srcObject = stream;
                await videoEl.play();

                runLoop();
            } catch (error) {
                console.error(error);
            }
        };

        if (enabled) init();

        return () => {
            isActive = false;
            if (videoEl.srcObject) {
                const stream = videoEl.srcObject as MediaStream;
                stream.getTracks().forEach(t => t.stop());
            }
            if (requestRef.current) cancelAnimationFrame(requestRef.current);
        };
    }, [enabled, sensitivity, camera, updateCameraProjection]);

    return (
        <group>
            {/* Invisible HTML video element is managed by Ref */}
            {/* Debug Indicator */}
            <mesh position={[0, -0.25, 0]}>
                {/* <textGeometry args={[status, { size: 0.02, height: 0 }]} /> */}
                {/* Text rendering is hard in bare threejs, using HTML overlay instead */}
            </mesh>

            {/* HTML Overlay for Feedback */}
            {/* We can teleport this out to UI later */}
        </group>
    );
};
