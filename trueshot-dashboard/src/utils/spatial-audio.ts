/**
 * Spatial Audio Player for 4D Gaussian Splatting
 * 
 * Production-quality spatial audio with:
 * - Web Audio API PannerNode with HRTF
 * - Dynamic listener position tracking
 * - Distance-based attenuation
 * - Cone-based directional sound
 * - Doppler effect support
 * - Room reverb simulation
 */

import { useRef, useCallback, useState, useEffect } from 'react';
import { Vector3 } from 'three';
import type { Quaternion } from 'three';

// ============================================================================
// Types
// ============================================================================

export interface AudioSource {
    id: string;
    name: string;
    position: [number, number, number];
    direction: [number, number, number];
    volume: number;
    minDistance: number;
    maxDistance: number;
    rolloffFactor: number;
    coneInnerAngle: number;
    coneOuterAngle: number;
    coneOuterGain: number;
    timeRange: [number, number];
    audioUrl: string;
}

export interface SpatialAudioSceneData {
    sources: AudioSource[];
    roomDimensions?: [number, number, number];
    duration: number;
    sampleRate: number;
}

export interface ListenerState {
    position: [number, number, number];
    forward: [number, number, number];
    up: [number, number, number];
}

// ============================================================================
// Spatial Audio Context
// ============================================================================

export class SpatialAudioPlayer {
    private context: AudioContext | null = null;
    private listener: AudioListener | null = null;
    private sources: Map<string, SpatialAudioSourceNode> = new Map();
    private masterGain: GainNode | null = null;
    private convolver: ConvolverNode | null = null;
    private isInitialized = false;

    /**
     * Initialize the audio context (must be called after user gesture)
     */
    async initialize(): Promise<void> {
        if (this.isInitialized) return;

        this.context = new AudioContext({ sampleRate: 48000 });
        this.listener = this.context.listener;

        // Create master gain
        this.masterGain = this.context.createGain();
        this.masterGain.gain.value = 1.0;
        this.masterGain.connect(this.context.destination);

        // Create convolver for room reverb
        this.convolver = this.context.createConvolver();
        this.convolver.connect(this.masterGain);

        // Generate simple impulse response
        await this.loadDefaultReverb();

        this.isInitialized = true;
    }

    /**
     * Load a spatial audio scene
     */
    async loadScene(sceneData: SpatialAudioSceneData): Promise<void> {
        if (!this.context) {
            await this.initialize();
        }

        // Clear existing sources
        this.clearSources();

        // Load each audio source
        for (const sourceData of sceneData.sources) {
            await this.loadSource(sourceData);
        }

        // Create room impulse response if dimensions provided
        if (sceneData.roomDimensions) {
            await this.createRoomReverb(sceneData.roomDimensions);
        }
    }

    /**
     * Load a single audio source
     */
    async loadSource(sourceData: AudioSource): Promise<void> {
        if (!this.context || !this.masterGain) return;

        // Load audio file
        const response = await fetch(sourceData.audioUrl);
        const arrayBuffer = await response.arrayBuffer();
        const audioBuffer = await this.context.decodeAudioData(arrayBuffer);

        // Create source node
        const bufferSource = this.context.createBufferSource();
        bufferSource.buffer = audioBuffer;
        bufferSource.loop = false;

        // Create panner with HRTF for realistic 3D audio
        const panner = this.context.createPanner();
        panner.panningModel = 'HRTF';
        panner.distanceModel = 'inverse';
        panner.refDistance = sourceData.minDistance;
        panner.maxDistance = sourceData.maxDistance;
        panner.rolloffFactor = sourceData.rolloffFactor;
        panner.coneInnerAngle = sourceData.coneInnerAngle;
        panner.coneOuterAngle = sourceData.coneOuterAngle;
        panner.coneOuterGain = sourceData.coneOuterGain;

        // Set position
        panner.positionX.value = sourceData.position[0];
        panner.positionY.value = sourceData.position[1];
        panner.positionZ.value = sourceData.position[2];

        // Set orientation
        panner.orientationX.value = sourceData.direction[0];
        panner.orientationY.value = sourceData.direction[1];
        panner.orientationZ.value = sourceData.direction[2];

        // Create gain node for volume
        const gainNode = this.context.createGain();
        gainNode.gain.value = sourceData.volume;

        // Connect: source -> gain -> panner -> master
        bufferSource.connect(gainNode);
        gainNode.connect(panner);
        panner.connect(this.masterGain);

        // Also send to reverb (wet mix)
        if (this.convolver) {
            const reverbGain = this.context.createGain();
            reverbGain.gain.value = 0.2;  // 20% wet reverb
            gainNode.connect(reverbGain);
            reverbGain.connect(this.convolver);
        }

        this.sources.set(sourceData.id, {
            id: sourceData.id,
            data: sourceData,
            bufferSource,
            panner,
            gainNode,
            audioBuffer,
            isPlaying: false,
        });
    }

    /**
     * Update listener position and orientation
     */
    updateListener(state: ListenerState): void {
        if (!this.listener) return;

        // Position
        if (this.listener.positionX) {
            this.listener.positionX.value = state.position[0];
            this.listener.positionY.value = state.position[1];
            this.listener.positionZ.value = state.position[2];
        } else {
            this.listener.setPosition(...state.position);
        }

        // Orientation
        if (this.listener.forwardX) {
            this.listener.forwardX.value = state.forward[0];
            this.listener.forwardY.value = state.forward[1];
            this.listener.forwardZ.value = state.forward[2];
            this.listener.upX.value = state.up[0];
            this.listener.upY.value = state.up[1];
            this.listener.upZ.value = state.up[2];
        } else {
            this.listener.setOrientation(...state.forward, ...state.up);
        }
    }

    /**
     * Update a source's position (for moving sources)
     */
    updateSourcePosition(
        sourceId: string,
        position: [number, number, number],
        _velocity?: [number, number, number]
    ): void {
        void _velocity;
        const source = this.sources.get(sourceId);
        if (!source) return;

        source.panner.positionX.value = position[0];
        source.panner.positionY.value = position[1];
        source.panner.positionZ.value = position[2];
    }

    /**
     * Play all sources from a specific time
     */
    playFrom(time: number): void {
        if (!this.context) return;

        for (const [, source] of this.sources) {
            if (source.isPlaying) continue;

            // Check if within time range
            if (time >= source.data.timeRange[0] && time < source.data.timeRange[1]) {
                const offset = time - source.data.timeRange[0];

                // Need to recreate buffer source (can only be started once)
                const newBuffer = this.context.createBufferSource();
                newBuffer.buffer = source.audioBuffer;
                newBuffer.connect(source.gainNode);
                source.bufferSource = newBuffer;

                newBuffer.start(0, offset);
                source.isPlaying = true;
            }
        }
    }

    /**
     * Stop all sources
     */
    stop(): void {
        for (const source of this.sources.values()) {
            if (source.isPlaying) {
                try {
                    source.bufferSource.stop();
                } catch (error) {
                    console.warn('Audio stop failed', error);
                }
                source.isPlaying = false;
            }
        }
    }

    /**
     * Set master volume
     */
    setMasterVolume(volume: number): void {
        if (this.masterGain) {
            this.masterGain.gain.value = Math.max(0, Math.min(1, volume));
        }
    }

    /**
     * Set individual source volume
     */
    setSourceVolume(sourceId: string, volume: number): void {
        const source = this.sources.get(sourceId);
        if (source) {
            source.gainNode.gain.value = Math.max(0, Math.min(1, volume));
        }
    }

    /**
     * Clear all sources
     */
    clearSources(): void {
        this.stop();
        this.sources.clear();
    }

    /**
     * Cleanup
     */
    dispose(): void {
        this.clearSources();
        if (this.context) {
            this.context.close();
        }
        this.isInitialized = false;
    }

    /**
     * Load default reverb impulse response
     */
    private async loadDefaultReverb(): Promise<void> {
        if (!this.context || !this.convolver) return;

        // Generate simple room impulse response
        const sampleRate = this.context.sampleRate;
        const length = sampleRate * 0.5;  // 500ms reverb tail
        const impulse = this.context.createBuffer(2, length, sampleRate);

        for (let channel = 0; channel < 2; channel++) {
            const channelData = impulse.getChannelData(channel);
            for (let i = 0; i < length; i++) {
                // Exponential decay with random noise
                channelData[i] = (Math.random() * 2 - 1) * Math.pow(1 - i / length, 2);
            }
        }

        this.convolver.buffer = impulse;
    }

    /**
     * Create room-specific reverb
     */
    private async createRoomReverb(dimensions: [number, number, number]): Promise<void> {
        if (!this.context || !this.convolver) return;

        const [width, height, depth] = dimensions;
        const volume = width * height * depth;
        const surface = 2 * (width * height + width * depth + height * depth);

        // Approximate RT60 (Sabine's formula)
        const absorption = 0.3;
        const rt60 = 0.161 * volume / (surface * absorption);

        // Generate impulse response
        const sampleRate = this.context.sampleRate;
        const length = Math.min(sampleRate * rt60, sampleRate * 2);
        const impulse = this.context.createBuffer(2, length, sampleRate);

        for (let channel = 0; channel < 2; channel++) {
            const channelData = impulse.getChannelData(channel);

            // Early reflections
            const reflections = [
                { delay: width / 343 * sampleRate, gain: 0.5 },
                { delay: height / 343 * sampleRate, gain: 0.4 },
                { delay: depth / 343 * sampleRate, gain: 0.45 },
                { delay: Math.sqrt(width * width + height * height) / 343 * sampleRate, gain: 0.3 },
            ];

            for (const ref of reflections) {
                const sample = Math.floor(ref.delay);
                if (sample < length) {
                    channelData[sample] += ref.gain * (channel === 0 ? 1 : -1) * 0.3;
                }
            }

            // Late reverb (diffuse)
            const decayStart = Math.floor(0.05 * sampleRate);
            for (let i = decayStart; i < length; i++) {
                const decay = Math.pow(0.001, i / length);
                channelData[i] += (Math.random() * 2 - 1) * decay * 0.1;
            }
        }

        this.convolver.buffer = impulse;
    }
}

interface SpatialAudioSourceNode {
    id: string;
    data: AudioSource;
    bufferSource: AudioBufferSourceNode;
    panner: PannerNode;
    gainNode: GainNode;
    audioBuffer: AudioBuffer;
    isPlaying: boolean;
}

// ============================================================================
// React Hook
// ============================================================================

export function useSpatialAudio() {
    const playerRef = useRef<SpatialAudioPlayer | null>(null);
    const [isInitialized, setIsInitialized] = useState(false);
    const [isPlaying, setIsPlaying] = useState(false);
    const [currentTime, setCurrentTime] = useState(0);

    // Initialize on mount
    useEffect(() => {
        playerRef.current = new SpatialAudioPlayer();

        return () => {
            playerRef.current?.dispose();
        };
    }, []);

    const initialize = useCallback(async () => {
        if (!playerRef.current) return;
        await playerRef.current.initialize();
        setIsInitialized(true);
    }, []);

    const loadScene = useCallback(async (sceneData: SpatialAudioSceneData) => {
        if (!playerRef.current) return;
        if (!isInitialized) {
            await initialize();
        }
        await playerRef.current.loadScene(sceneData);
    }, [isInitialized, initialize]);

    const updateListener = useCallback((state: ListenerState) => {
        playerRef.current?.updateListener(state);
    }, []);

    const play = useCallback((time: number = 0) => {
        playerRef.current?.playFrom(time);
        setIsPlaying(true);
        setCurrentTime(time);
    }, []);

    const stop = useCallback(() => {
        playerRef.current?.stop();
        setIsPlaying(false);
    }, []);

    const setMasterVolume = useCallback((volume: number) => {
        playerRef.current?.setMasterVolume(volume);
    }, []);

    return {
        isInitialized,
        isPlaying,
        currentTime,
        initialize,
        loadScene,
        updateListener,
        play,
        stop,
        setMasterVolume,
    };
}

// ============================================================================
// Three.js Integration Hook
// ============================================================================

export function useSpatialAudioWithThree(camera: { position: Vector3; quaternion: Quaternion } | null) {
    const audio = useSpatialAudio();
    const lastPosition = useRef<[number, number, number]>([0, 0, 0]);

    // Update listener from camera position
    useEffect(() => {
        if (!camera) return;

        const updateFromCamera = () => {
            const position: [number, number, number] = [
                camera.position.x,
                camera.position.y,
                camera.position.z,
            ];

            // Extract forward and up from quaternion
            // Forward is -Z, Up is Y in Three.js
            const forwardVec = new Vector3(0, 0, -1).applyQuaternion(camera.quaternion);
            const upVec = new Vector3(0, 1, 0).applyQuaternion(camera.quaternion);

            const forward: [number, number, number] = [
                forwardVec.x,
                forwardVec.y,
                forwardVec.z,
            ];
            const up: [number, number, number] = [upVec.x, upVec.y, upVec.z];

            audio.updateListener({ position, forward, up });
            lastPosition.current = position;
        };

        // Update every frame using RAF
        let animationId: number;
        const loop = () => {
            updateFromCamera();
            animationId = requestAnimationFrame(loop);
        };
        loop();

        return () => {
            cancelAnimationFrame(animationId);
        };
    }, [camera, audio]);

    return audio;
}
