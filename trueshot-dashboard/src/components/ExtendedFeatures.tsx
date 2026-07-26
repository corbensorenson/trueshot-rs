import React, { useState } from 'react';

// Reference Overlay (Drag & Drop)
export const ReferenceOverlay = ({ opacity = 0.5 }: { opacity?: number }) => {
    const [refImage, setRefImage] = useState<string | null>(null);

    const onDrop = (e: React.DragEvent) => {
        e.preventDefault();
        if (e.dataTransfer.files && e.dataTransfer.files[0]) {
            const file = e.dataTransfer.files[0];
            const reader = new FileReader();
            reader.onload = (ev) => setRefImage(ev.target?.result as string);
            reader.readAsDataURL(file);
        }
    };

    if (!refImage) {
        return (
            <div
                className="absolute top-4 left-4 w-32 h-32 border-2 border-dashed border-white/20 rounded-lg flex items-center justify-center text-xs text-white/40 pointer-events-auto"
                onDragOver={(e) => e.preventDefault()}
                onDrop={onDrop}
            >
                Drop Ref Here
            </div>
        );
    }

    return (
        <div className="absolute inset-0 pointer-events-none z-10" style={{ opacity }}>
            <img src={refImage} className="w-full h-full object-cover" alt="Reference" />
            <button
                className="absolute top-2 right-2 bg-red-500/50 text-white rounded-full w-6 h-6 flex items-center justify-center pointer-events-auto"
                onClick={() => setRefImage(null)}
            >
                ×
            </button>
        </div>
    );
};

// Issue Reporter
export const IssueReporter = ({ onReport }: { onReport: () => void }) => (
    <button
        className="bg-yellow-600 hover:bg-yellow-500 text-black font-bold py-1 px-3 rounded shadow-lg flex items-center gap-2"
        onClick={onReport}
    >
        <span>⚠️</span> Report Issue
    </button>
);

// Jog Wheel (SVG based)
export const JogWheel = ({ onRotate }: { onRotate: (delta: number) => void }) => {
    const [isDragging, setIsDragging] = useState(false);
    const [lastY, setLastY] = useState(0);

    const startDrag = (e: React.MouseEvent) => {
        setIsDragging(true);
        setLastY(e.clientY);
    };

    const doDrag = (e: React.MouseEvent) => {
        if (!isDragging) return;
        const delta = lastY - e.clientY;
        onRotate(delta);
        setLastY(e.clientY);
    };

    const endDrag = () => setIsDragging(false);

    return (
        <div
            className="w-20 h-20 rounded-full border-4 border-gray-600 bg-gray-800 relative cursor-ns-resize shadow-[inset_0_2px_10px_rgba(0,0,0,0.5)] flex items-center justify-center"
            onMouseDown={startDrag}
            onMouseMove={doDrag}
            onMouseUp={endDrag}
            onMouseLeave={endDrag}
        >
            <div className="w-2 h-2 bg-blue-500 rounded-full" />
            <div className="absolute inset-0 rounded-full border-t-2 border-white/20" style={{ transform: `rotate(${lastY}deg)` }} />
        </div>
    );
};

// Voice Memo
export const VoiceMemo = () => {
    const [recording, setRecording] = useState(false);

    const toggle = () => {
        if (!recording) {
            // Start recording logic
            setRecording(true);
        } else {
            // Stop logic
            setRecording(false);
        }
    };

    return (
        <button
            onClick={toggle}
            className={`p-3 rounded-full ${recording ? 'bg-red-600 animate-pulse' : 'bg-gray-700'} text-white shadow-lg`}
        >
            {recording ? '🎙️ Rec' : '🎙️ Memo'}
        </button>
    );
};
