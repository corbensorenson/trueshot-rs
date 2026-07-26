import { Bluetooth, Camera } from 'lucide-react';
import { useState, useEffect, useCallback } from 'react';
import { connectWebSocket } from '../api/client';
import { useHardwareStore } from '../store/hardwareStore';

interface HardwareStatusProps {
    onClick?: () => void;
}

export const HardwareStatus = ({ onClick }: HardwareStatusProps) => {
    const { cameras, turntable, setCameras, updateTurntable, setTurntableConnected, setBackendConnected } = useHardwareStore();
    const [status, setStatus] = useState("Offline"); // System/WS Status
    const [online, setOnline] = useState(false); // WS Connected

    // Computed from store
    const cameraCount = cameras.filter(c => c.connected).length;
    const turntableConnected = turntable.connected;

    const updateStatus = useCallback(async () => {
        try {
            const { getCameras, getTurntableStatus } = await import('../api/client');
            const cams = await getCameras();
            const tt = await getTurntableStatus();
            setCameras(cams);
            updateTurntable(tt);
        } catch (e) {
            console.error("Status check failed", e);
        }
    }, [setCameras, updateTurntable]);

    useEffect(() => {
        const { close } = connectWebSocket(
            (event) => {
                // Determine specific hardware status from events if needed
                if (event.type === 'DeviceConnected' || event.type === 'DeviceDisconnected') {
                    updateStatus();
                }
                if (event.type === 'TurntableStatus' && 'connected' in event) {
                    setTurntableConnected(Boolean(event.connected));
                }
            },
            () => { // On Connect
                setOnline(true);
                setBackendConnected(true);
                setStatus("System Online");
                updateStatus(); // Fetch initial state on connect
            },
            () => { // On Disconnect
                setOnline(false);
                setBackendConnected(false);
                setStatus("Disconnected");
                // Optional: clear store or mark as offline?
                // setCameras([]); 
                setTurntableConnected(false);
            }
        );

        return () => close();
    }, [setBackendConnected, setTurntableConnected, updateStatus]);

    return (
        <button
            type="button"
            onClick={onClick}
            className="flex items-center gap-6 px-6 py-3 glass-panel hover:bg-white/5 transition-colors cursor-pointer text-left"
        >
            <div className="flex items-center gap-2">
                <Camera className={`w-5 h-5 ${cameraCount > 0 ? 'neon-text-blue' : 'text-white/20'}`} />
                <div className="flex flex-col">
                    <span className="text-[10px] uppercase tracking-wider text-white/50">Camera</span>
                    <span className="text-sm font-semibold">{cameraCount > 0 ? `${cameraCount} Active` : "Searching..."}</span>
                </div>
            </div>

            <div className="w-px h-8 bg-white/10" />

            <div className="flex items-center gap-2">
                <Bluetooth className={`w-5 h-5 ${turntableConnected ? 'neon-text-cyan' : 'text-white/20'}`} />
                <div className="flex flex-col">
                    <span className="text-[10px] uppercase tracking-wider text-white/50">Turntable</span>
                    <span className={`text-sm font-semibold ${turntableConnected ? 'text-accent-cyan' : 'text-white/40'}`}>
                        {turntableConnected ? "Connected" : "Searching..."}
                    </span>
                </div>
            </div>

            <div className="ml-auto flex items-center gap-2">
                <div className={`w-2 h-2 rounded-full ${online ? 'bg-green-500 animate-pulse' : 'bg-red-500'}`} />
                <span className="text-[10px] uppercase tracking-widest font-bold hidden xl:block">{status}</span>
            </div>
        </button>
    );
};
