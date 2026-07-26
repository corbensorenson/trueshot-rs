import { useEffect, useState, useRef } from 'react';
import { Terminal, Activity } from 'lucide-react';
import { connectWebSocket, SystemEvent } from '../api/client';

export const PixelCollapseFeed = () => {
    const [logs, setLogs] = useState<string[]>([]);
    const scrollRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
        const ws = connectWebSocket((event: SystemEvent) => {
            // Extract log message
            let msg = "";
            if (typeof event === 'object') {
                if ('SystemMessage' in event) {
                    // @ts-expect-error - legacy event payload
                    const [text, level] = event.SystemMessage;
                    msg = `${level.toUpperCase()}: ${text}`;
                }
                else if ('CaptureStarted' in event) msg = "CAPTURE: Sequence Started";
                else if ('CaptureFinished' in event) msg = "CAPTURE: Sequence Complete";
                else if ('CaptureProgress' in event) return; // Skip progress spam
            }

            if (msg) {
                setLogs(prev => [...prev.slice(-40), msg]);
            }
        });
        return () => ws.close();
    }, []);

    useEffect(() => {
        if (scrollRef.current) {
            scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
        }
    }, [logs]);

    return (
        <div className="flex flex-col gap-2 p-6 glass-panel h-full font-mono text-xs overflow-hidden">
            <div className="flex items-center justify-between mb-2">
                <h2 className="text-[10px] font-bold uppercase tracking-widest text-white/40 flex items-center gap-2">
                    <Terminal className="w-3 h-3" />
                    TrueShot Kernel Feed
                </h2>
                <Activity className="w-3 h-3 text-accent-cyan animate-pulse" />
            </div>

            <div ref={scrollRef} className="flex-1 overflow-y-auto space-y-1 scrollbar-hide">
                {logs.map((log, i) => (
                    <div key={i} className="flex gap-3 text-white/60 animate-in fade-in slide-in-from-left-2 duration-300">
                        <span className="text-white/20 whitespace-nowrap">[{new Date().toLocaleTimeString()}]</span>
                        <span className={log.includes('FFT') || log.includes('locked') ? 'text-accent-cyan' : log.includes('ERROR') ? 'text-red-500' : ''}>{log}</span>
                    </div>
                ))}
                {logs.length === 0 && <div className="text-white/20">Waiting for kernel sequence...</div>}
            </div>
        </div>
    );
};
