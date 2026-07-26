import { Terminal, HardDrive } from 'lucide-react';
import { useSystemStats } from '../hooks/useSystemStats';

interface FooterProps {
    consoleOpen: boolean;
    setConsoleOpen: (open: boolean) => void;
    holographicMode: boolean;
    setHolographicMode: (mode: boolean) => void;
}

export const Footer = ({ consoleOpen, setConsoleOpen, holographicMode, setHolographicMode }: FooterProps) => {
    const { stats } = useSystemStats();

    return (
        <footer className="shrink-0 pointer-events-auto">
            <div className="ts-panel h-12 px-6 flex items-center justify-between text-[10px] uppercase font-bold tracking-widest text-[color:var(--ts-muted)]">
                <div className="flex gap-8">
                    <span className="flex items-center gap-2" title={`Memory: ${stats?.memory_used_mb ?? 0}MB / ${stats?.memory_total_mb ?? 0}MB`}>
                        <div className={`w-1.5 h-1.5 rounded-full ${stats ? 'bg-accent-blue shadow-[0_0_8px_#0070f3]' : 'bg-red-500'} `} />
                        CPU: {(stats?.cpu_usage ?? 0).toFixed(1)}%
                    </span>
                    <span className="flex items-center gap-2 text-accent-cyan">
                        <div className="w-1.5 h-1.5 rounded-full bg-accent-cyan shadow-[0_0_8px_#00dfd8]" /> METAL GRAPHICS ACCELERATED
                    </span>
                    <span className="flex items-center gap-2">
                        <HardDrive className="w-3 h-3" />
                        STORAGE: {stats?.disk_free_gb ?? 0}GB FREE
                    </span>
                    {/* Terminal Toggle */}
                    <button
                        onClick={() => setConsoleOpen(!consoleOpen)}
                        className={`flex items-center gap-2 px-3 py-1 rounded-lg ts-transition ${consoleOpen ? 'bg-[color:color-mix(in_srgb,var(--ts-surface)_75%,transparent)] text-[color:var(--ts-text)]' : 'hover:bg-[color:color-mix(in_srgb,var(--ts-surface)_65%,transparent)] text-[color:var(--ts-muted)]'}`}
                    >
                        <Terminal className="w-3 h-3" />
                        <span className="text-[10px] font-bold tracking-widest uppercase">
                            TERMINAL
                        </span>
                    </button>
                </div>
                <div className="flex items-center gap-4">
                    <button
                        onClick={() => setHolographicMode(!holographicMode)}
                        className={`flex items-center gap-2 px-3 py-1 rounded-lg ts-transition border ${holographicMode ? 'bg-accent-blue/20 border-accent-blue/50 text-accent-cyan' : 'bg-transparent border-transparent hover:bg-[color:color-mix(in_srgb,var(--ts-surface)_65%,transparent)] text-[color:color-mix(in_srgb,var(--ts-text)_25%,transparent)]'}`}
                        title="Toggle 2.5D Parallax Face Tracking"
                    >
                        <span className="italic uppercase tracking-widest text-[10px]">{holographicMode ? 'Holographic Active' : 'Enable Holographic'}</span>
                        <div className={`w-1.5 h-1.5 rounded-full ${holographicMode ? 'bg-accent-cyan animate-pulse shadow-[0_0_8px_cyan]' : 'bg-[color:color-mix(in_srgb,var(--ts-text)_20%,transparent)]'}`} />
                    </button>
                </div>
            </div>
        </footer>
    );
};
