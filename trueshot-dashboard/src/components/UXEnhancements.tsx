export const TrafficLightStatus = ({ status }: { status: 'idle' | 'processing' | 'error' }) => {
    const colors = {
        idle: 'bg-green-500 shadow-[0_0_20px_rgba(34,197,94,0.6)]',
        processing: 'bg-yellow-500 shadow-[0_0_20px_rgba(234,179,8,0.6)]',
        error: 'bg-red-500 shadow-[0_0_20px_rgba(239,68,68,0.6)]',
    };

    return (
        <div className={`w-16 h-16 rounded-full transition-all duration-500 ${colors[status]} border-4 border-black/20`} />
    );
};

// Safe Zone / Sweet Spot Overlay
export const SweetSpotOverlay = ({ show }: { show: boolean }) => {
    if (!show) return null;
    return (
        <div className="absolute inset-0 pointer-events-none" style={{
            background: 'radial-gradient(circle, transparent 40%, rgba(255, 0, 0, 0.1) 60%, rgba(255, 0, 0, 0.3) 100%)',
            border: '2px dashed rgba(0, 255, 0, 0.3)'
        }}>
            <div className="absolute top-1/2 left-1/2 transform -translate-x-1/2 -translate-y-1/2 text-green-500/50 text-xs tracking-widest uppercase">
                Optic Sweet Spot
            </div>
        </div>
    );
};

// History Scrubber
export const HistoryScrubber = ({ history, onSelect }: { history: unknown[]; onSelect: (idx: number) => void }) => {
    return (
        <div className="flex space-x-1 overflow-x-auto h-16 bg-black/50 p-2 rounded w-full backdrop-blur-md">
            {history.map((item, i) => (
                <div
                    key={i}
                    onClick={() => onSelect(i)}
                    className="min-w-[40px] h-full bg-gray-800 hover:bg-white/20 cursor-pointer border-r border-white/10 relative group"
                >
                    {/* Tooltip */}
                    <div className="hidden group-hover:block absolute bottom-full mb-1 bg-black text-xs p-1 whitespace-nowrap z-50">
                        Capture {i + 1}
                    </div>
                </div>
            ))}
        </div>
    );
};
