import { LiveMonitor } from './LiveMonitor';
import { User, Film, Headphones } from 'lucide-react';
import { CameraProfile } from '../api/client';

interface SidebarProps {
    onSelectCam: (cam: CameraProfile) => void;
    onOpenAvatar: () => void;
    onOpenScene: () => void;
    onOpenXR: () => void;
}

export const Sidebar = ({ onSelectCam, onOpenAvatar, onOpenScene, onOpenXR }: SidebarProps) => {
    return (
        <aside className="w-64 flex flex-col gap-4 pointer-events-auto z-20 pb-4 pl-6 pt-4">
            <div className="flex-1 ts-panel p-4 flex flex-col gap-3 overflow-hidden min-h-0">
                <div className="flex items-center justify-between mb-2">
                    <h3 className="text-[10px] uppercase font-bold text-[color:var(--ts-muted)] tracking-widest pl-1">Live Feeds</h3>
                    <div className="w-2 h-2 bg-red-500 rounded-full animate-pulse shadow-[0_0_8px_red]" />
                </div>
                <div className="flex-1 min-h-0 overflow-y-auto custom-scrollbar -mr-2 pr-2">
                    <LiveMonitor onSelectCam={onSelectCam} />
                </div>
            </div>
            <div className="ts-panel p-4 flex flex-col gap-3">
                <div className="flex items-center justify-between mb-1">
                    <h3 className="text-[10px] uppercase font-bold text-[color:var(--ts-muted)] tracking-widest pl-1">Experiences</h3>
                </div>
                <button
                    onClick={onOpenAvatar}
                    className="flex items-center gap-2 rounded-lg border border-[color:var(--ts-border)] px-3 py-2 text-[10px] uppercase tracking-[0.2em] text-[color:var(--ts-text)] hover:bg-[color:var(--ts-surface-strong)] transition-colors"
                >
                    <User className="w-4 h-4 text-accent-cyan" />
                    Avatar Studio
                </button>
                <button
                    onClick={onOpenScene}
                    className="flex items-center gap-2 rounded-lg border border-[color:var(--ts-border)] px-3 py-2 text-[10px] uppercase tracking-[0.2em] text-[color:var(--ts-text)] hover:bg-[color:var(--ts-surface-strong)] transition-colors"
                >
                    <Film className="w-4 h-4 text-accent-blue" />
                    Scene Reconstruction
                </button>
                <button
                    onClick={onOpenXR}
                    className="flex items-center gap-2 rounded-lg border border-[color:var(--ts-border)] px-3 py-2 text-[10px] uppercase tracking-[0.2em] text-[color:var(--ts-text)] hover:bg-[color:var(--ts-surface-strong)] transition-colors"
                >
                    <Headphones className="w-4 h-4 text-accent-purple" />
                    XR Scanner
                </button>
            </div>
        </aside>
    );
};
