import { useRef, useState, useEffect, useCallback } from 'react';

interface VirtualJoystickProps {
    onMove: (x: number, y: number) => void;
    onStop: () => void;
    size?: number;
}

export const VirtualJoystick = ({ onMove, onStop, size = 100 }: VirtualJoystickProps) => {
    const containerRef = useRef<HTMLDivElement>(null);
    const knobRef = useRef<HTMLDivElement>(null);
    const [active, setActive] = useState(false);
    const [position, setPosition] = useState({ x: 0, y: 0 });
    const [sensitivity, setSensitivity] = useState(1.0); // 0.2 to 3.0

    const updatePosition = useCallback((clientX: number, clientY: number, sens: number) => {
        if (!containerRef.current) return;
        const rect = containerRef.current.getBoundingClientRect();
        const centerX = rect.left + rect.width / 2;
        const centerY = rect.top + rect.height / 2;

        const maxDist = size / 2;

        let dx = clientX - centerX;
        let dy = clientY - centerY;

        const dist = Math.sqrt(dx * dx + dy * dy);

        if (dist > maxDist) {
            dx = (dx / dist) * maxDist;
            dy = (dy / dist) * maxDist;
        }

        setPosition({ x: dx, y: dy });

        // Output with sensitivity
        // Normalize (-1 to 1) * sensitivity
        onMove((dx / maxDist) * sens, (dy / maxDist) * sens);
    }, [onMove, size]);

    const handleStart = useCallback((clientX: number, clientY: number) => {
        if (!containerRef.current) return;
        setActive(true);
        updatePosition(clientX, clientY, sensitivity);
    }, [sensitivity, updatePosition]);

    const handleMove = useCallback((clientX: number, clientY: number) => {
        if (!active || !containerRef.current) return;
        updatePosition(clientX, clientY, sensitivity);
    }, [active, sensitivity, updatePosition]);

    const handleEnd = useCallback(() => {
        setActive(false);
        setPosition({ x: 0, y: 0 });
        onStop();
    }, [onStop]);

    useEffect(() => {
        const onMouseMove = (e: MouseEvent) => handleMove(e.clientX, e.clientY);
        const onMouseUp = () => handleEnd();
        const onTouchMove = (e: TouchEvent) => handleMove(e.touches[0].clientX, e.touches[0].clientY);
        const onTouchEnd = () => handleEnd();

        if (active) {
            window.addEventListener('mousemove', onMouseMove);
            window.addEventListener('mouseup', onMouseUp);
            window.addEventListener('touchmove', onTouchMove);
            window.addEventListener('touchend', onTouchEnd);
        }

        return () => {
            window.removeEventListener('mousemove', onMouseMove);
            window.removeEventListener('mouseup', onMouseUp);
            window.removeEventListener('touchmove', onTouchMove);
            window.removeEventListener('touchend', onTouchEnd);
        };
    }, [active, handleMove, handleEnd]);

    return (
        <div className="flex flex-col items-center gap-2">
            <div
                ref={containerRef}
                className="rounded-full bg-black/40 border border-white/10 relative shadow-inner backdrop-blur-sm touch-none"
                style={{ width: size, height: size }}
                onMouseDown={(e) => handleStart(e.clientX, e.clientY)}
                onTouchStart={(e) => handleStart(e.touches[0].clientX, e.touches[0].clientY)}
            >
                <div
                    ref={knobRef}
                    className={`absolute w-1/3 h-1/3 rounded-full shadow-xl border border-white/20 flex items-center justify-center transition-transform duration-75 ${active ? 'bg-accent-blue cursor-grabbing scale-95' : 'bg-white/10 cursor-grab hover:bg-white/20'}`}
                    style={{
                        left: '50%',
                        top: '50%',
                        transform: `translate(calc(-50% + ${position.x}px), calc(-50% + ${position.y}px))`
                    }}
                >
                    <div className="w-2 h-2 rounded-full bg-white/50" />
                </div>
            </div>

            {/* Sensitivity Slider */}
            <div className="flex flex-col items-center w-full max-w-[80px] gap-1 group/slider">
                <input
                    type="range"
                    min="0.2"
                    max="5.0"
                    step="0.1"
                    value={sensitivity}
                    onChange={(e) => {
                        const newSens = parseFloat(e.target.value);
                        setSensitivity(newSens);
                        // If holding, update immediately
                        if (active) {
                            // We need current clientX/Y. Hard to get here without Ref.
                            // If user is just dragging slider, they aren't dragging joystick.
                            // If user is dragging joystick AND adjusting slider? Impossible with mouse.
                            // Possible with Multi-touch, but rare.
                            // Just setting state is fine. Next move will pick it up.
                        }
                    }}
                    className="w-full h-1 bg-white/10 rounded-full appearance-none cursor-pointer [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:w-3 [&::-webkit-slider-thumb]:h-3 [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-accent-blue hover:[&::-webkit-slider-thumb]:bg-accent-blue/80 transition-all opacity-50 hover:opacity-100"
                />
                <span className="text-[9px] text-white/20 font-mono uppercase tracking-widest group-hover/slider:text-white/40 transition-colors">Speed</span>
            </div>
        </div>
    );
};
