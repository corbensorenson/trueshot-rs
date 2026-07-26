import { useState, useEffect, useRef } from 'react';
import { X, Camera, Cpu, ArrowLeft, ArrowRight, History, Zap } from 'lucide-react';
import { getStreamUrl } from '../api/client';
import { useHardwareStore } from '../store/hardwareStore';
import toast from 'react-hot-toast';
import { VirtualJoystick } from './VirtualJoystick';

interface DeviceManagerProps {
    isOpen: boolean;
    onClose: () => void;
}

export const DeviceManager = ({ isOpen, onClose }: DeviceManagerProps) => {
    const { cameras, turntable } = useHardwareStore(); // Read-only access, updates come from HardwareStatus/WS
    // const [cameras, setCameras] = useState<CameraProfile[]>([]); // REPLACED
    // const [turntable, setTurntable] = useState<any>({ connected: false, type: "Unknown", moving: false }); // REPLACED
    const [rotationDegrees, setRotationDegrees] = useState(45);
    const [ptzState, setPtzState] = useState<{ [id: string]: { p: number, t: number, z: number } }>({});
    const [enabledCameras, setEnabledCameras] = useState<Record<string, boolean>>({});

    // Joystick State
    const [activeJoystickCam, setActiveJoystickCam] = useState<string | null>(null);
    const joystickVelocity = useRef({ x: 0, y: 0 });
    const ptzStateRef = useRef(ptzState);

    useEffect(() => {
        ptzStateRef.current = ptzState;
    }, [ptzState]);

    // PTZ Loop
    useEffect(() => {
        if (!activeJoystickCam) return;

        const interval = setInterval(async () => {
            const { x, y } = joystickVelocity.current;
            if (Math.abs(x) < 0.01 && Math.abs(y) < 0.01) return;

            const curState = ptzStateRef.current;
            const cur = curState[activeJoystickCam] || { p: 0, t: 0, z: 0 };
            let { p, t } = cur;
            const { z } = cur;

            // Speed is now handled by Joystick Sensitivity Output?
            // Joystick returns (norm * sens).
            // If Sens is 1.0, output is -1 to 1.
            // We need "Degrees Per Tick".
            // Let's assume Joystick output IS the degrees delta.
            // If Sens is 3.0, delta is 3.0 deg/tick.

            const speedMultiplier = 1.0;
            p += x * speedMultiplier;
            t -= y * speedMultiplier; // Invert Y

            p = Math.max(-180, Math.min(180, p));
            t = Math.max(-90, Math.min(90, t));

            setPtzState(prev => ({ ...prev, [activeJoystickCam]: { p, t, z } }));

            try {
                const { setCameraPtz } = await import('../api/client');
                await setCameraPtz(activeJoystickCam, p, t, z);
            } catch (error) {
                console.error('PTZ update failed', error);
            }

        }, 50);

        return () => clearInterval(interval);
    }, [activeJoystickCam]);

    // Note: We no longer need local useEffect to load data or connect WS, 
    // because HardwareStatus component (always mounted) handles it and populates the Store.
    // However, if we want to force refresh when opening:
    useEffect(() => {
        if (isOpen) {
            // Trigger a refresh just in case
            import('../api/client').then(async client => {
                try {
                    const cams = await client.getCameras();
                    const tt = await client.getTurntableStatus();
                    useHardwareStore.getState().setCameras(cams);
                    useHardwareStore.getState().updateTurntable(tt);
                } catch (error) {
                    console.error(error);
                }
            });
        }
    }, [isOpen]);

    const activeCameras = cameras.filter(c => c.connected);
    const historyCameras = cameras.filter(c => !c.connected);

    if (!isOpen) return null;

    return (
        <div className="fixed inset-0 z-50 flex items-center justify-center p-12 bg-black/80 backdrop-blur-sm animate-in fade-in duration-200 pointer-events-auto">
            <div className="w-full max-w-5xl max-h-full flex flex-col glass-panel relative overflow-hidden bg-[#050505]">
                {/* Header */}
                <div className="p-6 border-b border-white/10 flex items-center justify-between bg-white/5">
                    <div className="flex items-center gap-4">
                        <div className="w-10 h-10 rounded-xl bg-accent-blue/20 flex items-center justify-center">
                            <Cpu className="w-6 h-6 text-accent-blue" />
                        </div>
                        <div>
                            <h2 className="text-xl font-bold text-white">Device Manager</h2>
                            <p className="text-white/40 text-xs uppercase tracking-widest">Connected Hardware Registry</p>
                        </div>
                    </div>
                    <button onClick={async () => {
                        const { scanHardware } = await import('../api/client');
                        await scanHardware();
                        toast.success("Scanning for devices...");
                    }} className="p-2 hover:bg-white/10 rounded-full transition-colors mr-2 text-xs font-bold uppercase tracking-wider text-accent-blue border border-accent-blue/20 px-4">
                        Scan Hardware
                    </button>
                    <button onClick={onClose} className="p-2 hover:bg-white/10 rounded-full transition-colors" title="Close Device Manager">
                        <X className="w-6 h-6 text-white/60" />
                    </button>
                </div>

                {/* Content */}
                <div className="flex-1 overflow-y-auto p-8 space-y-12">

                    {/* Active Devices Section */}
                    <div className="space-y-6">
                        <div className="flex items-center gap-2 mb-6">
                            <div className="w-2 h-2 rounded-full bg-green-500 animate-pulse" />
                            <h3 className="text-white font-bold uppercase tracking-widest text-sm">Active Connections</h3>
                        </div>

                        {/* Local Sensors (Webcam) - Frontend Only */}
                        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6 mb-6">
                            <div className="p-6 rounded-2xl bg-white/5 border border-white/10 hover:border-accent-blue/30 transition-all flex flex-col gap-4 group">
                                <div className="flex items-start justify-between">
                                    <div className="flex items-center gap-3">
                                        <div className="p-3 rounded-xl bg-black/40 border border-white/5">
                                            <Camera className="w-6 h-6 text-white/60" />
                                        </div>
                                        <div>
                                            <h3 className="font-bold text-lg text-white">Local User Cam</h3>
                                            <span className="text-[10px] uppercase tracking-wider text-white/40">Frontend Sensor</span>
                                        </div>
                                    </div>
                                    <div className="px-2 py-1 rounded text-[10px] font-bold uppercase bg-green-400/10 text-green-400">
                                        Tracking Ready
                                    </div>
                                </div>
                                <div className="aspect-video bg-black rounded-lg overflow-hidden relative border border-white/10">
                                    <video
                                        ref={(el) => {
                                            if (el && navigator.mediaDevices) {
                                                navigator.mediaDevices.getUserMedia({ video: true }).then(stream => {
                                                    el.srcObject = stream;
                                                    el.play();
                                                }).catch(e => console.error("Local cam access failed", e));
                                            }
                                        }}
                                        className="w-full h-full object-cover opacity-50 grayscale group-hover:grayscale-0 group-hover:opacity-100 transition-all"
                                        muted
                                        playsInline
                                    />
                                    <div className="absolute top-2 right-2 flex gap-1">
                                        <div className="w-2 h-2 bg-green-500 rounded-full animate-pulse" />
                                        <span className="text-[10px] text-white/60 font-mono">LOCAL</span>
                                    </div>
                                </div>
                                <div className="flex gap-2 text-xs text-white/40">
                                    <span className="bg-black/40 px-2 py-1 rounded border border-white/5">Face Tracking Source</span>
                                </div>
                            </div>
                        </div>

                        {/* Combined Grid for All Device Types */}
                        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">

                            {/* 1. Turntables */}
                            <div className={`p-6 rounded-2xl border transition-all flex flex-col gap-4 relative overflow-hidden group ${turntable.connected ? 'bg-accent-blue/5 border-accent-blue/50' : 'bg-white/5 border-white/10 opacity-50'}`}>
                                <div className="flex items-start justify-between">
                                    <div className="flex items-center gap-3">
                                        <div className="p-3 rounded-xl bg-black/40 border border-white/5">
                                            <div className={`w-6 h-6 rounded-full border-2 border-dashed ${turntable.connected ? 'border-accent-blue animate-spin-slow' : turntable.type === 'Scanning...' ? 'border-yellow-400 animate-spin' : turntable.type === 'Not Found' ? 'border-red-500/50' : 'border-white/20'}`} />
                                        </div>
                                        <div>
                                            <h3 className="font-bold text-lg text-white">{turntable.connected ? turntable.type : turntable.type === 'Scanning...' ? "Searching..." : turntable.type === 'Not Found' ? "No Device Found" : "Turntable"}</h3>
                                            <span className="text-[10px] uppercase tracking-wider text-white/40">Motion Control</span>
                                        </div>
                                    </div>
                                    <div className={`px-2 py-1 rounded text-[10px] font-bold uppercase ${turntable.connected ? 'text-green-400 bg-green-400/10' : turntable.type === 'Scanning...' ? 'text-yellow-400 bg-yellow-400/10' : 'text-red-400 bg-red-400/10'}`}>
                                        {turntable.connected ? 'Connected' : turntable.type === 'Scanning...' ? 'Scanning' : turntable.type === 'Not Found' ? 'Not Found' : 'Offline'}
                                    </div>
                                </div>

                                {turntable.connected && (
                                    <div className="mt-auto grid grid-cols-1 gap-2">
                                        <div className="grid grid-cols-2 gap-2">
                                            <button
                                                disabled={turntable.moving}
                                                onClick={async () => {
                                                    try {
                                                        const { homeTurntable } = await import('../api/client');
                                                        await homeTurntable();
                                                        toast.success("Turntable homing...");
                                                    } catch (error) {
                                                        console.error(error);
                                                        toast.error("Home failed");
                                                    }
                                                }}
                                                className={`py-2 rounded text-xs transition-colors flex items-center justify-center gap-2 col-span-2 ${turntable.moving ? 'bg-white/5 text-white/20 cursor-not-allowed' : 'bg-white/10 hover:bg-white/20'}`}
                                            >
                                                {turntable.moving ? <div className="w-3 h-3 border-2 border-white/20 border-t-white rounded-full animate-spin" /> : "Home"}
                                            </button>
                                        </div>
                                        <div className="flex items-stretch rounded overflow-hidden">
                                            <button
                                                title="Rotate Counter-Clockwise"
                                                disabled={turntable.moving}
                                                onClick={async () => {
                                                    try {
                                                        const { rotateTurntable } = await import('../api/client');
                                                        await rotateTurntable(-rotationDegrees);
                                                        toast.success(`Rotating -${rotationDegrees}°`);
                                                    } catch (error) {
                                                        console.error(error);
                                                        toast.error("Rotation failed");
                                                    }
                                                }}
                                                className={`py-2 px-3 rounded-l text-xs transition-colors flex items-center justify-center border-r border-white/5 ${turntable.moving ? 'bg-white/5 text-white/20 cursor-not-allowed' : 'bg-white/10 hover:bg-white/20'}`}
                                            >
                                                <ArrowLeft className="w-4 h-4" />
                                            </button>
                                            <div className="relative flex-1">
                                                <input
                                                    type="number"
                                                    value={rotationDegrees}
                                                    onChange={(e) => setRotationDegrees(Math.max(1, parseInt(e.target.value) || 0))}
                                                    className="w-full h-full bg-white/5 text-center text-xs font-bold text-white focus:outline-none focus:bg-white/10 transition-colors"
                                                    disabled={turntable.moving}
                                                />
                                                <span className="absolute right-2 top-1/2 -translate-y-1/2 text-[10px] text-white/40 pointer-events-none">DEG</span>
                                            </div>
                                            <button
                                                title="Rotate Clockwise"
                                                disabled={turntable.moving}
                                                onClick={async () => {
                                                    try {
                                                        const { rotateTurntable } = await import('../api/client');
                                                        await rotateTurntable(rotationDegrees);
                                                        toast.success(`Rotating +${rotationDegrees}°`);
                                                    } catch (error) {
                                                        console.error(error);
                                                        toast.error("Rotation failed");
                                                    }
                                                }}
                                                className={`py-2 px-3 rounded-r text-xs transition-colors flex items-center justify-center border-l border-white/5 ${turntable.moving ? 'bg-white/5 text-white/20 cursor-not-allowed' : 'bg-white/10 hover:bg-white/20'}`}
                                            >
                                                <ArrowRight className="w-4 h-4" />
                                            </button>
                                        </div>
                                    </div>
                                )}
                            </div>

                            {/* 2. Cameras */}
                            {activeCameras.map((cam) => (
                                <div key={cam.id} className="p-6 rounded-2xl bg-white/5 border border-white/10 hover:border-accent-blue/30 transition-all flex flex-col gap-4 group">
                                    <div className="flex items-start justify-between">
                                        <div className="flex items-center gap-3">
                                            <div className="p-3 rounded-xl bg-black/40 border border-white/5">
                                                <Camera className="w-6 h-6 text-accent-cyan" />
                                            </div>
                                            <div className="min-w-0">
                                                <h3 className="font-bold text-lg text-white truncate max-w-[150px]" title={cam.nickname || cam.name}>
                                                    {cam.nickname || cam.name}
                                                </h3>
                                                <span className="text-[10px] uppercase tracking-wider text-white/40 block truncate">{cam.id}</span>
                                            </div>
                                        </div>
                                        <div className="flex flex-col items-end gap-2">
                                            <div className={`px-2 py-1 rounded text-[10px] font-bold uppercase shrink-0 ${cam.capabilities.has_gimbal ? 'bg-accent-blue/20 text-accent-blue' : 'bg-white/5 text-white/40'}`}>
                                                {cam.capabilities.has_gimbal ? 'PTZ' : 'Fixed'}
                                            </div>
                                            {/* Use For Scan Toggle */}
                                            <button
                                                onClick={() => {
                                                    const newState = !enabledCameras[cam.id];
                                                    setEnabledCameras(prev => ({ ...prev, [cam.id]: newState }));
                                                    toast(newState ? "Camera Enabled for Scan" : "Camera Disabled for Scan");
                                                }}
                                                className={`relative w-10 h-5 rounded-full transition-colors duration-200 ease-in-out ${enabledCameras[cam.id] !== false ? 'bg-green-500' : 'bg-[#333]'}`}
                                                title={enabledCameras[cam.id] !== false ? "Active for Scan" : "Ignored"}
                                            >
                                                <div className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow-sm transition-transform duration-200 ease-in-out ${enabledCameras[cam.id] !== false ? 'translate-x-5' : 'translate-x-0'}`} />
                                            </button>
                                        </div>
                                    </div>

                                    {/* Feed */}
                                    <div className="aspect-video bg-black rounded-lg overflow-hidden relative border border-white/10 group">
                                        <img src={getStreamUrl(cam.id)} alt="Live" className="w-full h-full object-cover" onError={(e) => (e.target as HTMLImageElement).style.display = 'none'} />
                                        <div className="absolute top-2 right-2 flex gap-1"><div className="w-2 h-2 bg-red-500 rounded-full animate-pulse" /><span className="text-[10px] text-white/60 font-mono">LIVE</span></div>

                                        {/* Gimbal Overlay (Smaller) */}
                                        {cam.capabilities.has_gimbal && (
                                            <div className="absolute bottom-2 right-2 opacity-0 group-hover:opacity-100 transition-opacity bg-black/60 backdrop-blur rounded-full p-1">
                                                <VirtualJoystick size={60} onMove={(x, y) => { setActiveJoystickCam(cam.id); joystickVelocity.current = { x, y }; }} onStop={() => { setActiveJoystickCam(null); }} />
                                            </div>
                                        )}
                                    </div>

                                    {/* Storage Info if DSLR */}
                                    {cam.capabilities.storage_info && (
                                        <div className="flex items-center justify-between text-[10px] px-1">
                                            <span className="text-white/40">{cam.capabilities.storage_info.remaining_shots} shots left</span>
                                            <span className="text-white/40">{cam.capabilities.storage_info.free_gb?.toFixed(1)}GB</span>
                                        </div>
                                    )}
                                </div>
                            ))}

                            {/* 3. Empty State / Placeholders for Lights/Arms */}
                            {activeCameras.length === 0 && !turntable.connected && (
                                <div className="col-span-full py-12 text-center border-2 border-dashed border-white/10 rounded-2xl">
                                    <div className="w-12 h-12 bg-white/5 rounded-full flex items-center justify-center mx-auto mb-4 text-white/20">
                                        <Zap className="w-6 h-6" />
                                    </div>
                                    <h3 className="text-white/60 font-bold">No active devices</h3>
                                    <p className="text-white/40 text-sm mt-1">Connect USB or BLE hardware to begin</p>
                                </div>
                            )}

                        </div>
                    </div>

                    {/* Previous Devices Section */}
                    {historyCameras.length > 0 && (
                        <div className="space-y-6 pt-6 border-t border-white/10 opacity-60">
                            <div className="flex items-center gap-2 mb-6">
                                <History className="w-4 h-4 text-white/40" />
                                <h3 className="text-white/60 font-bold uppercase tracking-widest text-sm">Recently Connected</h3>
                            </div>

                            <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
                                {historyCameras.map((cam) => (
                                    <div key={cam.id} className="p-4 rounded-xl border border-white/5 bg-white/5 flex items-center justify-between grayscale opacity-50 hover:grayscale-0 hover:opacity-100 transition-all cursor-not-allowed">
                                        <div className="flex items-center gap-3">
                                            <div className="p-2 rounded bg-black/20">
                                                <Camera className="w-4 h-4" />
                                            </div>
                                            <div className="min-w-0">
                                                <div className="text-sm font-bold text-white truncate max-w-[120px]" title={cam.name}>{cam.nickname || cam.name}</div>
                                                <div className="text-[10px] text-white/40 truncate">{cam.id}</div>
                                            </div>
                                        </div>
                                        <span className="text-[10px] bg-white/10 px-2 py-1 rounded text-white/40">OFFLINE</span>
                                    </div>
                                ))}
                            </div>
                        </div>
                    )}

                </div>
            </div>
        </div>
    );
};
