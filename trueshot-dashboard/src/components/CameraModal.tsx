import { useState, useRef, useEffect } from 'react';
import { X, ZoomIn, ZoomOut, Sliders, Zap, ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight, MoreHorizontal, Battery, BatteryLow, BatteryWarning, BatteryFull, BatteryMedium } from 'lucide-react';
import { CameraProfile, setCameraPtz, setCameraConfig, triggerAutofocus, setFocusPoint, driveFocus, capturePhoto, getStreamUrl } from '../api/client';
import { VirtualJoystick } from './VirtualJoystick';

interface CameraModalProps {
    camera: CameraProfile;
    onClose: () => void;
}

export const CameraModal = ({ camera, onClose }: CameraModalProps) => {
    const [ptzState, setPtzState] = useState<{ p: number, t: number, z: number }>({ p: 0, t: 0, z: 0 });
    const joystickVelocity = useRef({ x: 0, y: 0 });
    const ptzStateRef = useRef(ptzState);
    const buildInitialConfig = (profile: CameraProfile) => {
        const iso = profile.last_settings?.iso ?? profile.capabilities.iso_options?.[0] ?? '100';
        const shutter_speed = profile.last_settings?.shutter_speed ?? profile.capabilities.shutter_speed_options?.[0] ?? '1/125';
        const aperture = profile.last_settings?.aperture ?? profile.capabilities.aperture_options?.[0] ?? 'f/7.1';
        const wb = profile.last_settings?.wb ?? profile.capabilities.wb_options?.[0] ?? 'Auto';
        return {
            iso,
            shutter_speed,
            aperture,
            wb,
            mode: 'M',
        };
    };
    const [config, setConfig] = useState(() => buildInitialConfig(camera));
    const [focusPoint, setFocusPointState] = useState<{ x: number, y: number } | null>(null);
    const wbOptions = camera.capabilities.wb_options ?? [];

    useEffect(() => {
        ptzStateRef.current = ptzState;
    }, [ptzState]);

    useEffect(() => {
        setConfig(buildInitialConfig(camera));
    }, [camera.id]);

    // PTZ Loop
    useEffect(() => {
        if (!camera.capabilities.has_gimbal) return;
        const interval = setInterval(async () => {
            const { x, y } = joystickVelocity.current;
            if (Math.abs(x) < 0.01 && Math.abs(y) < 0.01) return;
            let { p, t } = ptzStateRef.current;
            const { z } = ptzStateRef.current;
            p += x * 1.5; t -= y * 1.5;
            p = Math.max(-180, Math.min(180, p)); t = Math.max(-90, Math.min(90, t));
            setPtzState({ p, t, z });
            try {
                await setCameraPtz(camera.id, p, t, z);
            } catch (err) {
                console.error(err);
            }
        }, 50);
        return () => clearInterval(interval);
    }, [camera]);

    const handleZoom = async (delta: number) => {
        const { p, t } = ptzState;
        let { z } = ptzState;
        z = Math.max(0, Math.min(100, z + delta));
        setPtzState({ p, t, z });
        await setCameraPtz(camera.id, p, t, z);
    };

    const updateConfig = async (key: string, value: string) => {
        setConfig(prev => ({ ...prev, [key]: value }));
        try {
            await setCameraConfig(camera.id, { [key]: value });
        } catch (err) {
            console.error(err);
        }
    };

    const handleDriveFocus = async (step: number) => {
        try {
            await driveFocus(camera.id, step);
        } catch (err) {
            console.error(err);
        }
    };

    return (
        <div className="fixed inset-0 z-[100] bg-[#1a1a1a] flex animate-in fade-in duration-200">
            {/* Main Viewport */}
            <div className="flex-1 relative bg-black flex flex-col">
                {/* Top Toolbar */}
                <div className="h-12 bg-[#2a2a2a] border-b border-[#3a3a3a] flex items-center px-4 gap-4">
                    <div className="bg-yellow-400 text-black font-bold px-2 py-0.5 text-xs rounded-sm">Lv</div>
                    <button className="p-1 text-yellow-400 hover:text-yellow-300"><Sliders className="w-5 h-5" /></button>
                    <div className="flex-1" />
                    <div className="text-white/40 text-xs font-mono">{camera.id}</div>
                    <button onClick={onClose} className="p-2 bg-red-500/10 hover:bg-red-500/20 text-red-500 rounded border border-red-500/20 transition-colors ml-4">
                        <X className="w-5 h-5" />
                    </button>
                </div>

                {/* Viewport */}
                <div className="flex-1 relative flex items-center justify-center overflow-hidden">
                    <img
                        src={getStreamUrl(camera.id)}
                        className="max-w-full max-h-full object-contain cursor-crosshair"
                        alt="Live Feed"
                        onClick={async (e) => {
                            // Allow click-to-focus for DSLRs (GPhoto) or cameras with autofocus
                            if (!camera.id.includes('GPhoto') && !camera.capabilities.has_autofocus) return;
                            const rect = e.currentTarget.getBoundingClientRect();
                            const x = (e.clientX - rect.left) / rect.width;
                            const y = (e.clientY - rect.top) / rect.height;
                            setFocusPointState({ x, y });
                            try {
                                await setFocusPoint(camera.id, x, y);
                                await triggerAutofocus(camera.id);
                            } catch (e) { console.error(e); }
                            // Clear focus indicator after 2 seconds
                            setTimeout(() => setFocusPointState(null), 2000);
                        }}
                    />
                    {/* Focus Point Indicator */}
                    {focusPoint && (
                        <div
                            className="absolute pointer-events-none border-2 border-yellow-400 w-12 h-12 animate-pulse"
                            style={{
                                left: `calc(${focusPoint.x * 100}% - 24px)`,
                                top: `calc(${focusPoint.y * 100}% - 24px)`,
                            }}
                        />
                    )}
                    {/* Grid Overlay */}
                    <div className="absolute inset-0 pointer-events-none opacity-20 border-white/20 border-2">
                        <div className="absolute top-1/3 w-full h-px bg-white/20" />
                        <div className="absolute top-2/3 w-full h-px bg-white/20" />
                        <div className="absolute left-1/3 h-full w-px bg-white/20" />
                        <div className="absolute left-2/3 h-full w-px bg-white/20" />
                    </div>
                </div>

                {/* Bottom Status Bar */}
                <div className="h-10 bg-[#2a2a2a] border-t border-[#3a3a3a] flex items-center px-4 gap-6 text-sm font-bold text-white/80">
                    <span>{config.mode}</span>
                    <span>{config.shutter_speed}</span>
                    <span>{config.aperture}</span>
                    <span>ISO {config.iso}</span>
                    {/* Battery Indicator */}
                    {camera.battery_level != null && (
                        <span className={`ml-auto flex items-center gap-1 ${camera.battery_level > 50 ? 'text-green-400' :
                            camera.battery_level > 20 ? 'text-yellow-400' : 'text-red-400'
                            }`}>
                            {camera.battery_level > 75 ? <BatteryFull className="w-5 h-5" /> :
                                camera.battery_level > 50 ? <BatteryMedium className="w-5 h-5" /> :
                                    camera.battery_level > 20 ? <BatteryLow className="w-5 h-5" /> :
                                        <BatteryWarning className="w-5 h-5" />}
                            {camera.battery_level}%
                        </span>
                    )}
                    {camera.battery_level == null && (
                        <span className="text-white/40 ml-auto flex items-center gap-1">
                            <Battery className="w-5 h-5" />
                            --
                        </span>
                    )}
                </div>
            </div>

            {/* Right Sidebar - NxTether Style */}
            <div className="w-80 bg-[#222] border-l border-[#333] flex flex-col font-sans">
                {/* Header Tabs - NxTether Style (Live View Only) */}
                <div className="flex border-b border-[#333]">
                    <div className="flex-1 py-3 text-center text-yellow-400 border-b-2 border-yellow-400 font-bold text-sm bg-[#2a2a2a]">Live View</div>
                </div>

                <div className="flex-1 overflow-y-auto p-4 space-y-2">
                    {/* AF Button */}
                    {camera.capabilities.has_autofocus && (
                        <div className="flex justify-center mb-6 mt-2 gap-4">
                            <div className="flex flex-col items-center gap-1">
                                <button onClick={() => triggerAutofocus(camera.id)} className="w-16 h-16 rounded-full border-2 border-white/20 flex items-center justify-center hover:border-yellow-400 hover:text-yellow-400 transition-colors">
                                    <span className="font-bold">AF</span>
                                </button>
                                <span className="text-[10px] text-white/40 uppercase">Autofocus</span>
                            </div>
                            <div className="flex flex-col items-center gap-1">
                                <button onClick={async () => { try { await capturePhoto(camera.id); } catch (e) { console.error(e); } }} className="w-16 h-16 rounded-full bg-white text-black flex items-center justify-center hover:bg-gray-200 transition-colors">
                                    <div className="w-12 h-12 border-2 border-black rounded-full" />
                                </button>
                                <span className="text-[10px] text-white/40 uppercase">Capture</span>
                            </div>
                        </div>
                    )}

                    {/* Settings Grid - Always show for DSLR Cameras */}
                    {(camera.id.includes('GPhoto') || camera.capabilities.iso_options?.length > 0 || camera.capabilities.shutter_speed_options?.length > 0) && (
                        <div className="bg-[#1a1a1a] rounded border border-[#333] p-1">
                            <div className="flex items-center justify-between px-2 py-1 border-b border-[#333]">
                                <span className="text-xs text-white/60 font-bold">Shooting Settings</span>
                            </div>
                            <div className="grid grid-cols-4 gap-1 p-1">
                                {/* Mode */}
                                <div className="bg-[#333] aspect-square flex flex-col items-center justify-center rounded cursor-pointer hover:bg-[#444]">
                                    <span className="text-xl font-bold text-white">{config.mode}</span>
                                </div>
                                {/* Shutter */}
                                <div className="bg-[#333] aspect-square flex flex-col items-center justify-center rounded cursor-pointer hover:bg-[#444]">
                                    <span className="text-[10px] text-white/60">1/</span>
                                    <span className="text-sm font-bold text-white">{config.shutter_speed.replace('1/', '')}</span>
                                </div>
                                {/* Aperture */}
                                <div className="bg-[#333] aspect-square flex flex-col items-center justify-center rounded cursor-pointer hover:bg-[#444]">
                                    <span className="text-sm font-bold text-white">{config.aperture.toUpperCase()}</span>
                                </div>
                                {/* ISO */}
                                <div className="bg-[#333] aspect-square flex flex-col items-center justify-center rounded cursor-pointer hover:bg-[#444]">
                                    <span className="text-[8px] text-white/60">ISO</span>
                                    <span className="text-sm font-bold text-white">{config.iso}</span>
                                </div>

                                {/* Row 2 */}
                                <div className="bg-[#333] aspect-square flex flex-col items-center justify-center rounded cursor-pointer hover:bg-[#444] col-span-1">
                                    <span className="text-[8px] text-white/60">WB</span>
                                    <span className="text-xs font-bold text-white">{config.wb}</span>
                                </div>
                                <div className="bg-[#333] aspect-square flex flex-col items-center justify-center rounded cursor-pointer hover:bg-[#444] col-span-1">
                                    <Zap className="w-4 h-4 text-white/60" />
                                    <span className="text-[8px] text-white/60">Flash</span>
                                </div>
                            </div>
                            {/* Functional Settings Dropdowns */}
                            <div className="p-2 space-y-2">
                                {/* Aperture */}
                                <div className="flex items-center gap-2">
                                    <span className="text-[10px] text-white/40 w-16">Aperture</span>
                                    <select
                                        className="flex-1 bg-black/40 border border-white/10 rounded px-2 py-1 text-xs text-white outline-none"
                                        value={config.aperture}
                                        onChange={(e) => updateConfig('aperture', e.target.value)}
                                    >
                                        {(camera.capabilities.aperture_options?.length > 0
                                            ? camera.capabilities.aperture_options
                                            : ['f/1.4', 'f/1.8', 'f/2', 'f/2.8', 'f/4', 'f/5.6', 'f/7.1', 'f/8', 'f/11', 'f/16', 'f/22']
                                        ).map(o => <option key={o} value={o}>{o}</option>)}
                                    </select>
                                </div>
                                {/* Shutter Speed */}
                                <div className="flex items-center gap-2">
                                    <span className="text-[10px] text-white/40 w-16">Shutter</span>
                                    <select
                                        className="flex-1 bg-black/40 border border-white/10 rounded px-2 py-1 text-xs text-white outline-none"
                                        value={config.shutter_speed}
                                        onChange={(e) => updateConfig('shutter_speed', e.target.value)}
                                    >
                                        {(camera.capabilities.shutter_speed_options?.length > 0
                                            ? camera.capabilities.shutter_speed_options
                                            : ['30', '15', '8', '4', '2', '1', '1/2', '1/4', '1/8', '1/15', '1/30', '1/60', '1/125', '1/250', '1/500', '1/1000', '1/2000', '1/4000', '1/8000']
                                        ).map(o => <option key={o} value={o}>{o}</option>)}
                                    </select>
                                </div>
                                {/* ISO */}
                                <div className="flex items-center gap-2">
                                    <span className="text-[10px] text-white/40 w-16">ISO</span>
                                    <select
                                        className="flex-1 bg-black/40 border border-white/10 rounded px-2 py-1 text-xs text-white outline-none"
                                        value={config.iso}
                                        onChange={(e) => updateConfig('iso', e.target.value)}
                                    >
                                        {(camera.capabilities.iso_options?.length > 0
                                            ? camera.capabilities.iso_options
                                            : ['64', '100', '125', '160', '200', '250', '320', '400', '500', '640', '800', '1000', '1250', '1600', '2000', '2500', '3200', '6400', '12800', '25600']
                                        ).map(o => <option key={o} value={o}>{o}</option>)}
                                    </select>
                                </div>
                                {/* White Balance */}
                                <div className="flex items-center gap-2">
                                    <span className="text-[10px] text-white/40 w-16">WB</span>
                                    <select
                                        className="flex-1 bg-black/40 border border-white/10 rounded px-2 py-1 text-xs text-white outline-none"
                                        value={config.wb}
                                        onChange={(e) => updateConfig('wb', e.target.value)}
                                        disabled={wbOptions.length === 0}
                                    >
                                        {(wbOptions.length > 0
                                            ? wbOptions
                                            : ['Auto']
                                        ).map(o => <option key={o} value={o}>{o}</option>)}
                                    </select>
                                </div>
                            </div>
                        </div>
                    )}

                    {/* Focus Control - Always show for DSLRs */}
                    {(camera.id.includes('GPhoto') || camera.capabilities.has_autofocus) && (
                        <div className="bg-[#1a1a1a] rounded border border-[#333] mt-2">
                            <div className="flex items-center justify-between px-2 py-1 border-b border-[#333]">
                                <span className="text-xs text-white/60 font-bold">Focus Drive</span>
                            </div>
                            <div className="p-2 flex flex-col gap-2">
                                <div className="flex gap-1 justify-center items-center">
                                    <button onClick={() => handleDriveFocus(-6)} className="p-2 bg-[#333] rounded hover:bg-[#444] text-white" title="Near (Large)"><ChevronsLeft className="w-5 h-5" /></button>
                                    <button onClick={() => handleDriveFocus(-3)} className="p-2 bg-[#333] rounded hover:bg-[#444] text-white" title="Near (Medium)"><ChevronsLeft className="w-4 h-4" /></button>
                                    <button onClick={() => handleDriveFocus(-1)} className="p-2 bg-[#333] rounded hover:bg-[#444] text-white" title="Near (Fine)"><ChevronLeft className="w-4 h-4" /></button>

                                    <div className="w-px h-6 bg-[#333] mx-1" />

                                    <button onClick={() => handleDriveFocus(1)} className="p-2 bg-[#333] rounded hover:bg-[#444] text-white" title="Far (Fine)"><ChevronRight className="w-4 h-4" /></button>
                                    <button onClick={() => handleDriveFocus(3)} className="p-2 bg-[#333] rounded hover:bg-[#444] text-white" title="Far (Medium)"><ChevronsRight className="w-4 h-4" /></button>
                                    <button onClick={() => handleDriveFocus(6)} className="p-2 bg-[#333] rounded hover:bg-[#444] text-white" title="Far (Large)"><ChevronsRight className="w-5 h-5" /></button>
                                </div>
                                <div className="text-[9px] text-center text-white/20 uppercase tracking-widest">Manual Override</div>
                            </div>
                        </div>
                    )}

                    {/* PTZ Control Panel */}
                    {camera.capabilities.has_gimbal && (
                        <div className="bg-[#1a1a1a] rounded border border-[#333] mt-2 p-3 flex flex-col gap-2">
                            <span className="text-xs text-white/60 font-bold border-b border-[#333] pb-1">Gimbal</span>
                            <div className="flex justify-center">
                                <VirtualJoystick size={80} onMove={(x, y) => { joystickVelocity.current = { x, y } }} onStop={() => { joystickVelocity.current = { x: 0, y: 0 } }} />
                            </div>
                            <div className="flex gap-2 justify-center">
                                <button onClick={() => handleZoom(-5)} title="Zoom Out" className="p-2 bg-[#333] rounded hover:bg-[#444]"><ZoomOut className="w-4 h-4 text-white" /></button>
                                <button onClick={() => handleZoom(5)} title="Zoom In" className="p-2 bg-[#333] rounded hover:bg-[#444]"><ZoomIn className="w-4 h-4 text-white" /></button>
                            </div>
                        </div>
                    )}

                </div>

                <div className="p-4 border-t border-[#333] flex justify-end items-center text-white/40">
                    <button className="text-white hover:text-yellow-400">
                        <MoreHorizontal className="w-5 h-5" />
                    </button>
                </div>
            </div>
        </div>
    );
};
