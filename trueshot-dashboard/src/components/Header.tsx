import { Library, HelpCircle, Smartphone, Zap, Camera, Shield, Sun, Moon, BadgeDollarSign } from 'lucide-react';
import { HardwareStatus } from './HardwareStatus';
import { useHardwareStore } from '../store/hardwareStore';

export interface HeaderProps {
    onOpenLibrary: () => void;
    onOpenHelp: () => void;
    onOpenAR: () => void;
    onOpenDeviceManager: () => void;
    onOpenSequence: () => void;
    onOpenSecurity: () => void;
    onOpenLicense: () => void;
    booted: boolean;
    theme: 'dark' | 'light';
    onToggleTheme: () => void;
}

export const Header = ({
    onOpenLibrary,
    onOpenHelp,
    onOpenAR,
    onOpenDeviceManager,
    onOpenSequence,
    onOpenSecurity,
    onOpenLicense,
    booted,
    theme,
    onToggleTheme
}: HeaderProps) => {
    const { backendConnected } = useHardwareStore();

    return (
        <header className="flex items-center justify-between pointer-events-auto h-20">
            {/* Left: Logo & Status */}
            <div className="flex items-center gap-6">
                <div className="relative group cursor-pointer">
                    <div className="w-12 h-12 bg-accent-blue rounded-2xl flex items-center justify-center shadow-lg shadow-accent-blue/20 group-hover:scale-105 transition-transform duration-300">
                        <div className="absolute inset-0 bg-white/20 blur-lg rounded-full opacity-0 group-hover:opacity-100 transition-opacity" />
                        <Zap className="text-[color:var(--ts-text-on-accent)] w-7 h-7 fill-[color:var(--ts-text-on-accent)]" />
                    </div>
                </div>

                <div className="h-10 w-px bg-[color:var(--ts-border)]" />

                <div className="flex flex-col">
                    <h1 className="text-xl font-black italic tracking-tighter text-[color:var(--ts-text)] flex items-center gap-1">
                        TRUESHOT
                    </h1>
                    <div className="flex items-center gap-3 text-[10px] font-bold tracking-[0.2em] text-[color:color-mix(in_srgb,var(--ts-text)_30%,transparent)] uppercase">
                        <span>Operation Center V6.0</span>
                    </div>
                </div>

                <div className="h-10 w-px bg-[color:var(--ts-border)] mx-2" />

                <div className="flex flex-col">
                    <span className="text-[10px] uppercase font-bold text-[color:color-mix(in_srgb,var(--ts-text)_30%,transparent)] tracking-widest">Status</span>
                    <span className={`font-bold tracking-wider text-sm ${booted && backendConnected ? "text-accent-cyan shadow-accent-cyan drop-shadow-[0_0_8px_rgba(0,223,216,0.5)]" : "text-red-500 shadow-red-500 drop-shadow-[0_0_8px_rgba(239,68,68,0.5)]"}`}>
                        {booted && backendConnected ? "SYSTEM ONLINE" : "DISCONNECTED"}
                    </span>
                </div>
            </div>

            {/* Right: Controls & Hardware Status */}
            <div className="flex items-center gap-6">

                {/* Primary Action */}
                <button
                    onClick={onOpenSequence}
                    className="flex items-center gap-2 px-4 py-2 rounded-lg ts-button-primary"
                    title="Start a new scanning sequence"
                >
                    <Camera className="w-4 h-4 text-accent-blue group-hover:scale-110 transition-transform" />
                    <span className="text-xs font-bold text-accent-blue tracking-wider uppercase">New Scan</span>
                </button>

                <div className="h-8 w-px bg-[color:var(--ts-border)]" />

                {/* Icon Set */}
                <div className="flex items-center gap-1">
                    {/* Settings button removed as per user request */}
                    <button
                        onClick={onToggleTheme}
                        className="p-2.5 rounded-lg ts-icon-button"
                        title={`Switch to ${theme === 'dark' ? 'light' : 'dark'} mode`}
                    >
                        {theme === 'dark' ? <Sun className="w-5 h-5" /> : <Moon className="w-5 h-5" />}
                    </button>
                    <button onClick={onOpenSecurity} className="p-2.5 rounded-lg ts-icon-button" title="Access Control">
                        <Shield className="w-5 h-5" />
                    </button>
                    <button onClick={onOpenLicense} className="p-2.5 rounded-lg ts-icon-button" title="License & Plans">
                        <BadgeDollarSign className="w-5 h-5" />
                    </button>
                    <button onClick={onOpenLibrary} className="p-2.5 rounded-lg ts-icon-button" title="Project Library">
                        <Library className="w-5 h-5" />
                    </button>
                    <button onClick={onOpenHelp} className="p-2.5 rounded-lg ts-icon-button" title="Help & Documentation">
                        <HelpCircle className="w-5 h-5" />
                    </button>
                    <button onClick={onOpenAR} className="p-2.5 rounded-lg ts-icon-button relative overflow-hidden group" title="Open Mobile/AR View">
                        <div className="absolute inset-0 bg-[color:color-mix(in_srgb,var(--ts-surface)_70%,transparent)] translate-y-full group-hover:translate-y-0 transition-transform" />
                        <Smartphone className="w-5 h-5 relative z-10" />
                    </button>
                </div>

                {/* Hardware Pill Widget */}
                <div className="ts-panel rounded-xl overflow-hidden" title="Hardware Status & Device Manager">
                    <HardwareStatus onClick={onOpenDeviceManager} />
                </div>
            </div>
        </header>
    );
};
