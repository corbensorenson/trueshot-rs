import { Suspense, useCallback, useEffect, useMemo, useState } from 'react';
import { Canvas, type ThreeEvent, useLoader, useThree } from '@react-three/fiber';
import { Html, Line, OrbitControls, Splat, useGLTF } from '@react-three/drei';
import { PLYLoader } from 'three/examples/jsm/loaders/PLYLoader.js';
import { OBJLoader } from 'three/examples/jsm/loaders/OBJLoader.js';
import { STLLoader } from 'three/examples/jsm/loaders/STLLoader.js';
import { USDZLoader } from 'three/examples/jsm/loaders/USDZLoader.js';
import { ParallaxControls } from './ParallaxControls';
import * as THREE from 'three';

// Supported file types
export type ViewerType = 'auto' | 'splat' | 'points' | 'mesh';

export interface AnnotationPoint {
    id: string;
    label: string;
    position: [number, number, number];
    created_at?: number;
    author?: string | null;
}

interface UnifiedViewerProps {
    url?: string;
    type?: ViewerType;
    holographicMode?: boolean;
    className?: string;
    annotations?: AnnotationPoint[];
    annotationsReadOnly?: boolean;
    annotationStorageKey?: string;
    onAnnotationsChange?: (annotations: AnnotationPoint[]) => void;
}

const DefaultCube = () => (
    <mesh rotation={[0.5, 0.5, 0]}>
        <boxGeometry args={[0.5, 0.5, 0.5]} />
        <meshNormalMaterial wireframe />
        <meshStandardMaterial color="#00ffff" wireframe opacity={0.2} transparent />
    </mesh>
);

const PointsAsset = ({ url }: { url: string }) => {
    const geometry = useLoader(PLYLoader, url);
    return (
        <points>
            <primitive object={geometry} attach="geometry" />
            <pointsMaterial vertexColors size={0.01} sizeAttenuation={true} />
        </points>
    );
};

const ObjAsset = ({ url }: { url: string }) => {
    const object = useLoader(OBJLoader, url);
    return <primitive object={object} />;
};

const StlAsset = ({ url }: { url: string }) => {
    const geom = useLoader(STLLoader, url);
    return (
        <mesh geometry={geom}>
            <meshStandardMaterial color="gray" />
        </mesh>
    );
};

const UsdzAsset = ({ url }: { url: string }) => {
    const object = useLoader(USDZLoader, url);
    return <primitive object={object} />;
};

const GltfAsset = ({ url }: { url: string }) => {
    const gltf = useGLTF(url);
    return <primitive object={gltf.scene} />;
};

const MeshAsset = ({ url, ext }: { url: string; ext: string | undefined }) => {
    if (ext === 'obj') {
        return <ObjAsset url={url} />;
    }
    if (ext === 'stl') {
        return <StlAsset url={url} />;
    }
    if (ext === 'usdz') {
        return <UsdzAsset url={url} />;
    }
    if (ext === 'glb' || ext === 'gltf') {
        return <GltfAsset url={url} />;
    }
    return <DefaultCube />;
};

const AssetLoader = ({ url, type }: { url: string, type: ViewerType }) => {
    // Determine type if auto
    const actualType = useMemo(() => {
        if (type !== 'auto') return type;
        const ext = url.split('.').pop()?.toLowerCase();
        if (ext === 'splat') return 'splat';
        if (ext === 'ply') return 'points'; // Can also be splat, but usually points in this context unless specified
        if (['obj', 'stl', 'usdz', 'glb', 'gltf'].includes(ext || '')) return 'mesh';
        return 'mesh';
    }, [url, type]);

    const ext = url.split('.').pop()?.toLowerCase();

    // 1. Gaussian Splats
    if (actualType === 'splat') {
        return <Splat src={url} />;
    }

    // 2. Point Clouds (PLY)
    if (actualType === 'points') {
        return <PointsAsset url={url} />;
    }

    // 3. Meshes
    if (actualType === 'mesh') {
        return <MeshAsset url={url} ext={ext} />;
    }

    return <DefaultCube />;
};

const SceneClipping = ({ enabled, offset }: { enabled: boolean; offset: number }) => {
    const { gl } = useThree();
    useEffect(() => {
        gl.localClippingEnabled = enabled;
        if (enabled) {
            const plane = new THREE.Plane(new THREE.Vector3(0, 1, 0), -offset);
            gl.clippingPlanes = [plane];
        } else {
            gl.clippingPlanes = [];
        }
        return () => {
            gl.clippingPlanes = [];
            gl.localClippingEnabled = false;
        };
    }, [gl, enabled, offset]);
    return null;
};

export const UnifiedViewer = ({
    url = "/assets/demo.ply",
    type = 'auto',
    holographicMode = false,
    className = "w-full h-full relative",
    annotations: annotationsProp,
    annotationsReadOnly = false,
    annotationStorageKey,
    onAnnotationsChange,
}: UnifiedViewerProps) => {
    const [measureEnabled, setMeasureEnabled] = useState(false);
    const [measurePoints, setMeasurePoints] = useState<[THREE.Vector3, THREE.Vector3] | null>(null);
    const [annotationEnabled, setAnnotationEnabled] = useState(false);
    const [annotations, setAnnotations] = useState<AnnotationPoint[]>([]);
    const [sectionEnabled, setSectionEnabled] = useState(false);
    const [sectionOffset, setSectionOffset] = useState(0.0);
    const storageKey = annotationStorageKey || (url ? `trueshot_annotations_${url}` : '');
    const useLocalStorage = !annotationsProp && Boolean(storageKey);

    const handlePointerDown = useCallback((event: ThreeEvent<PointerEvent>) => {
        if (!measureEnabled && !annotationEnabled) return;
        if (!event?.point) return;
        const point = event.point as THREE.Vector3;
        if (annotationEnabled && !annotationsReadOnly) {
            const label = window.prompt('Annotation label')?.trim();
            if (!label) return;
            const next = [
                ...annotations,
                {
                    id: `${Date.now()}_${Math.random().toString(36).slice(2)}`,
                    label,
                    position: [point.x, point.y, point.z] as [number, number, number],
                },
            ];
            setAnnotations(next);
            onAnnotationsChange?.(next);
            return;
        }
        setMeasurePoints((prev) => {
            if (!prev) return [point.clone(), point.clone()];
            return [prev[1], point.clone()];
        });
    }, [measureEnabled, annotationEnabled, annotationsReadOnly, annotations, onAnnotationsChange]);

    useEffect(() => {
        if (!useLocalStorage || !storageKey) return;
        const stored = window.localStorage.getItem(storageKey);
        if (!stored) {
            setAnnotations([]);
            return;
        }
        try {
            const parsed = JSON.parse(stored) as AnnotationPoint[];
            setAnnotations(parsed);
        } catch {
            setAnnotations([]);
        }
    }, [storageKey, useLocalStorage]);

    useEffect(() => {
        if (!useLocalStorage || !storageKey) return;
        window.localStorage.setItem(storageKey, JSON.stringify(annotations));
    }, [annotations, storageKey, useLocalStorage]);

    useEffect(() => {
        if (annotationsProp) {
            setAnnotations(annotationsProp);
        }
    }, [annotationsProp]);

    const distanceLabel = useMemo(() => {
        if (!measurePoints) return null;
        const dist = measurePoints[0].distanceTo(measurePoints[1]);
        return `${dist.toFixed(4)} m`;
    }, [measurePoints]);

    return (
        <div className={`group overflow-hidden ${className}`}>
            <Canvas
                camera={{ position: [0, 0, 2], fov: 45 }}
                gl={{ antialias: true, toneMapping: THREE.ACESFilmicToneMapping }}
                dpr={[1, 2]} // Quality for high-dpi screens
            >
                {/* Scene Environment */}
                <color attach="background" args={['#050505']} />
                <ambientLight intensity={0.5} />
                <pointLight position={[10, 10, 10]} intensity={1.0} />
                <pointLight position={[-10, -10, -10]} intensity={0.5} color="blue" />

                {/* Content */}
                <Suspense fallback={<DefaultCube />}>
                    {url ? (
                        <group position={[0, -0.2, 0]} onPointerDown={handlePointerDown}> {/* Slight offset for centering */}
                            <AssetLoader url={url} type={type} />
                        </group>
                    ) : (
                        <DefaultCube />
                    )}
                </Suspense>

                <SceneClipping enabled={sectionEnabled} offset={sectionOffset} />

                {measurePoints && (
                    <Line
                        points={[measurePoints[0], measurePoints[1]]}
                        color="#37b8ff"
                        lineWidth={2}
                        dashed={false}
                    />
                )}

                {annotations.map((annotation) => (
                    <group
                        key={annotation.id}
                        position={new THREE.Vector3(annotation.position[0], annotation.position[1], annotation.position[2])}
                    >
                        <mesh>
                            <sphereGeometry args={[0.01, 12, 12]} />
                            <meshBasicMaterial color="#ff7a2f" />
                        </mesh>
                        <Html distanceFactor={8}>
                            <div className="px-2 py-1 text-[10px] rounded bg-black/70 text-white/90 border border-white/10">
                                {annotation.label}
                            </div>
                        </Html>
                    </group>
                ))}

                {/* Controls */}
                {holographicMode ? (
                    <ParallaxControls enabled={true} sensitivity={2.5} />
                ) : (
                    <OrbitControls
                        autoRotate={!measureEnabled}
                        autoRotateSpeed={0.5}
                        enableDamping
                        dampingFactor={0.05}
                    />
                )}

                {/* Helpers */}
                <gridHelper args={[10, 10, 0x333333, 0x111111]} position={[0, -0.5, 0]} />
            </Canvas>

            <div className="absolute top-4 left-4 flex flex-col gap-2 text-xs text-white/70">
                <button
                    onClick={() => {
                        setMeasureEnabled((prev) => !prev);
                        setAnnotationEnabled(false);
                    }}
                    className={`px-3 py-2 rounded-lg border ${
                        measureEnabled ? 'border-accent-cyan/50 bg-accent-cyan/10 text-accent-cyan' : 'border-white/10 bg-black/40'
                    }`}
                >
                    Measure
                </button>
                <button
                    onClick={() => {
                        if (annotationsReadOnly) return;
                        setAnnotationEnabled((prev) => !prev);
                        setMeasureEnabled(false);
                    }}
                    className={`px-3 py-2 rounded-lg border ${
                        annotationEnabled ? 'border-accent-cyan/50 bg-accent-cyan/10 text-accent-cyan' : 'border-white/10 bg-black/40'
                    } ${annotationsReadOnly ? 'opacity-50 cursor-not-allowed' : ''}`}
                >
                    Annotate
                </button>
                <button
                    onClick={() => setSectionEnabled((prev) => !prev)}
                    className={`px-3 py-2 rounded-lg border ${
                        sectionEnabled ? 'border-accent-cyan/50 bg-accent-cyan/10 text-accent-cyan' : 'border-white/10 bg-black/40'
                    }`}
                >
                    Section
                </button>
                {sectionEnabled && (
                    <div className="px-3 py-2 rounded-lg border border-white/10 bg-black/40">
                        <div className="text-[10px] uppercase tracking-[0.2em] text-white/50">Plane Y</div>
                        <input
                            type="range"
                            min={-2}
                            max={2}
                            step={0.01}
                            value={sectionOffset}
                            onChange={(e) => setSectionOffset(Number(e.target.value))}
                            className="w-full"
                        />
                    </div>
                )}
                {measureEnabled && (
                    <button
                        onClick={() => setMeasurePoints(null)}
                        className="px-3 py-2 rounded-lg border border-white/10 bg-black/40"
                    >
                        Clear Measure
                    </button>
                )}
                {annotationEnabled && !annotationsReadOnly && (
                    <button
                        onClick={() => {
                            setAnnotations([]);
                            onAnnotationsChange?.([]);
                        }}
                        className="px-3 py-2 rounded-lg border border-white/10 bg-black/40"
                    >
                        Clear Annotations
                    </button>
                )}
            </div>

            {distanceLabel && (
                <div className="absolute bottom-4 left-1/2 -translate-x-1/2 px-4 py-2 rounded-full bg-black/60 border border-white/10 text-xs text-white/80">
                    Distance: {distanceLabel}
                </div>
            )}

            {/* Mode Indicator */}
            {holographicMode && (
                <div className="absolute bottom-8 left-1/2 -translate-x-1/2 px-4 py-2 bg-black/60 backdrop-blur-md rounded-full border border-accent-cyan/30 text-accent-cyan text-xs font-mono animate-in fade-in slide-in-from-bottom-4 pointer-events-none select-none">
                    HOLOGRAPHIC MODE ACTIVE
                </div>
            )}
        </div>
    );
};
