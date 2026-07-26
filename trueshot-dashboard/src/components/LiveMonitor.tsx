import { useEffect, useState, useRef } from 'react';
import { Target, Maximize2 } from 'lucide-react'; // X, ZoomIn, ZoomOut, Crosshair removed
import { getCameras, CameraProfile, connectWebSocket, getStreamUrl } from '../api/client';

interface LiveMonitorProps {
    onSelectCam: (cam: CameraProfile) => void;
}

export const LiveMonitor = ({ onSelectCam }: LiveMonitorProps) => {
    const [cameras, setCameras] = useState<CameraProfile[]>([]);
    const [hoveredCam, setHoveredCam] = useState<CameraProfile | null>(null); // For Long Hover

    const hoverTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

    useEffect(() => {
        const fetchCams = () => getCameras()
            .then(cams => setCameras(cams.filter(c => c.connected)))
            .catch(console.error);

        fetchCams();
        const { close } = connectWebSocket((event) => {
            if (event.type === 'DeviceConnected' || event.type === 'DeviceDisconnected') {
                fetchCams();
            }
        });

        return () => close();
    }, []);

    const handleMouseEnter = (cam: CameraProfile) => {
        hoverTimer.current = setTimeout(() => {
            setHoveredCam(cam);
        }, 800); // 800ms threshold
    };

    const handleMouseLeave = () => {
        if (hoverTimer.current) clearTimeout(hoverTimer.current);
        setHoveredCam(null);
    };

    return (
        <>
            <div className="h-full flex flex-col gap-3 overflow-y-auto no-scrollbar mask-gradient-b pb-4 relative">
                {cameras.length === 0 && (
                    <div className="flex-1 flex flex-col items-center justify-center text-white/20 min-h-[100px] border border-white/5 rounded-lg border-dashed">
                        <div className="w-8 h-8 rounded-full border-2 border-dashed border-current animate-spin-slow mb-2" />
                        <span className="text-[10px] uppercase tracking-widest text-center">Scanning<br />Inputs...</span>
                    </div>
                )}

                {cameras.map((cam, idx) => (
                    <button
                        key={cam.id}
                        onClick={() => {
                            onSelectCam(cam);
                            setHoveredCam(null);
                        }}
                        onMouseEnter={() => handleMouseEnter(cam)}
                        onMouseLeave={handleMouseLeave}
                        className="relative aspect-video bg-black/50 rounded-lg overflow-hidden border border-white/5 group shrink-0 hover:scale-105 transition-transform duration-300 shadow-lg"
                    >
                        <img
                            src={getStreamUrl(cam.id)}
                            className="w-full h-full object-cover opacity-80 group-hover:opacity-100 transition-opacity"
                            alt={cam.name}
                            onError={(e) => {
                                e.currentTarget.style.display = 'none';
                                e.currentTarget.nextElementSibling?.classList.remove('hidden');
                            }}
                        />
                        <div className="hidden absolute inset-0 flex items-center justify-center text-white/10 text-[9px] font-mono uppercase tracking-widest">
                            NO SIGNAL
                        </div>

                        <div className="absolute inset-x-0 bottom-0 p-2 app-gradient-scrim flex justify-between items-end opacity-0 group-hover:opacity-100 transition-opacity">
                            <span className="text-[9px] font-mono text-white/70 uppercase truncate max-w-[80px]">
                                {cam.nickname || `CAM ${idx + 1}`}
                            </span>
                            {cam.capabilities.has_gimbal ? (
                                <Target className="w-3 h-3 text-accent-cyan" />
                            ) : (
                                <Maximize2 className="w-3 h-3 text-white/50" />
                            )}
                        </div>
                    </button>
                ))}
            </div>

            {/* Long Hover Preview (Pop up next to sidebar) */}
            {hoveredCam && (
                <div className="fixed left-72 top-32 w-80 aspect-video bg-black/90 backdrop-blur-xl rounded-xl border border-white/20 shadow-[0_0_30px_rgba(0,0,0,0.5)] z-50 overflow-hidden animate-in fade-in zoom-in-95 duration-200 pointer-events-none">
                    <div className="absolute top-2 left-2 bg-black/60 px-2 py-0.5 rounded text-[9px] text-white/80 font-bold tracking-widest uppercase border border-white/10">
                        Quick Look
                    </div>
                    <img
                        src={getStreamUrl(hoveredCam.id)}
                        className="w-full h-full object-cover"
                        alt="Preview"
                    />
                </div>
            )}
        </>
    );
};
