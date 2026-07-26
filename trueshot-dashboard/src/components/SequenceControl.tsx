import { useMachine } from '@xstate/react';
import { scanMachine } from '../machines/scanMachine';
import { Play, Square, Settings2 } from 'lucide-react';
import { motion } from 'framer-motion';
import { createProject, connectWebSocket, SystemEvent } from '../api/client';
import toast from 'react-hot-toast';
import { useState, useEffect } from 'react';

export const SequenceControl = () => {
    const [state, send] = useMachine(scanMachine);
    const [progress, setProgress] = useState(0);
    const [projectName, setProjectName] = useState("New Scan");
    const [mode, setMode] = useState<'auto' | 'schedule'>('auto');

    const isProcessing = state.matches('scanning');

    useEffect(() => {
        const ws = connectWebSocket((event: SystemEvent) => {
            if (typeof event === 'object') {
                const progressEvent = (event as { CaptureProgress?: [number, number] }).CaptureProgress;
                if (progressEvent) {
                    const p = progressEvent[1];
                    if (typeof p === 'number') setProgress(p);
                }
                const systemMessage = (event as { SystemMessage?: [string, string] }).SystemMessage;
                if (systemMessage) {
                    const [msg, level] = systemMessage;
                    if (level === 'Error') {
                        toast.error(msg);
                        send({ type: 'ERROR' });
                    }
                    else toast.success(msg);
                }
                if ('CaptureFinished' in event) {
                    send({ type: 'COMPLETE' });
                    toast.success("Capture Complete!");
                }
            }
        });
        return () => ws.close();
    }, [send]);

    const handleStart = async () => {
        send({ type: 'START' });
        try {
            await createProject(projectName, "Auto-generated scan");
            toast.success("Project Started");
        } catch (e) {
            console.error(e);
            toast.error("Failed to start project");
            send({ type: 'ERROR' });
        }
    };

    const handleStop = async () => {
        try {
            const { stopScan } = await import('../api/client');
            await stopScan();
            toast("Sequence Aborted by User", { icon: "🛑" });
            send({ type: 'STOP' });
        } catch (e) {
            toast.error("Failed to stop: " + e);
        }
    };

    return (
        <div className="flex flex-col gap-4 p-6 glass-panel h-full">
            <div className="flex items-center justify-between">
                <h2 className="text-xs font-bold uppercase tracking-widest text-white/40 flex items-center gap-2">
                    <Settings2 className="w-4 h-4" />
                    Sequence Configuration
                </h2>
                <div className="flex bg-white/5 rounded-lg p-1">
                    <button
                        onClick={() => setMode('auto')}
                        className={`px-3 py-1 rounded text-[10px] font-bold uppercase transition-all ${mode === 'auto' ? 'bg-accent-blue text-white shadow-lg' : 'text-white/40 hover:text-white'}`}
                    >
                        Auto
                    </button>
                    <button
                        onClick={() => setMode('schedule')}
                        className={`px-3 py-1 rounded text-[10px] font-bold uppercase transition-all ${mode === 'schedule' ? 'bg-accent-blue text-white shadow-lg' : 'text-white/40 hover:text-white'}`}
                    >
                        Schedule
                    </button>
                </div>
            </div>

            <div className="space-y-4 flex-1 overflow-y-auto pr-2">
                <div className="space-y-2">
                    <label className="text-[11px] uppercase tracking-wider text-white/60">Project Name</label>
                    <input
                        type="text"
                        value={projectName}
                        onChange={(e) => setProjectName(e.target.value)}
                        className="w-full bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-sm outline-none focus:border-accent-blue transition-colors"
                    />
                </div>

                {/* Progress Bar */}
                {isProcessing && (
                    <div className="space-y-1">
                        <div className="flex justify-between text-[10px] text-white/60">
                            <span>Progress</span>
                            <span>{Math.round(progress * 100)}%</span>
                        </div>
                        <div className="h-2 bg-white/10 rounded-full overflow-hidden">
                            <motion.div
                                className="h-full bg-accent-blue"
                                initial={{ width: 0 }}
                                animate={{ width: `${progress * 100}%` }}
                            />
                        </div>
                    </div>
                )}

                {mode === 'auto' ? (
                    <div className="grid grid-cols-2 gap-4">
                        <div className="space-y-2">
                            <label className="text-[11px] uppercase tracking-wider text-white/60">Preset</label>
                            <select className="w-full bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-sm outline-none focus:border-accent-blue appearance-none">
                                <option>Matte (Diffusion)</option>
                                <option>Shiny (Cross-Polarized)</option>
                                <option>Dark (High Exposure)</option>
                            </select>
                        </div>
                        <div className="space-y-2">
                            <label className="text-[11px] uppercase tracking-wider text-white/60">Quality</label>
                            <select className="w-full bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-sm outline-none focus:border-accent-blue appearance-none">
                                <option>Balanced (24 Steps)</option>
                                <option>High (48 Steps)</option>
                                <option>Ultra (72 Steps)</option>
                            </select>
                        </div>
                    </div>
                ) : (
                    <div className="space-y-4 border-t border-white/10 pt-4">
                        <div className="grid grid-cols-2 gap-4">
                            <div className="space-y-2">
                                <label className="text-[11px] uppercase tracking-wider text-white/60">Object Orientations</label>
                                <input type="number" defaultValue={2} className="w-full bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-sm outline-none focus:border-accent-blue" />
                            </div>
                            <div className="space-y-2">
                                <label className="text-[11px] uppercase tracking-wider text-white/60">Camera Positions</label>
                                <input type="number" defaultValue={3} className="w-full bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-sm outline-none focus:border-accent-blue" />
                            </div>
                        </div>
                        <div className="grid grid-cols-2 gap-4">
                            <div className="space-y-2">
                                <label className="text-[11px] uppercase tracking-wider text-white/60">Turntable Steps</label>
                                <input type="number" defaultValue={24} className="w-full bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-sm outline-none focus:border-accent-blue" />
                            </div>
                        </div>
                        <div className="space-y-2">
                            <label className="text-[11px] uppercase tracking-wider text-white/60">Focus Stacking</label>
                            <div className="flex gap-2">
                                <select className="flex-1 bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-sm outline-none focus:border-accent-blue">
                                    <option>Disabled</option>
                                    <option>3 Images</option>
                                    <option>5 Images</option>
                                </select>
                                <input type="number" placeholder="Step Size" className="w-20 bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-sm outline-none focus:border-accent-blue" />
                            </div>
                        </div>
                        <div className="space-y-2">
                            <label className="text-[11px] uppercase tracking-wider text-white/60">Exposure Bracketing</label>
                            <div className="flex gap-2">
                                <select className="flex-1 bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-sm outline-none focus:border-accent-blue">
                                    <option>Disabled</option>
                                    <option>HDR (3 Exposures)</option>
                                </select>
                                <input type="text" placeholder="-2, 0, +2" className="w-24 bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-sm outline-none focus:border-accent-blue" />
                            </div>
                        </div>
                    </div>
                )}
            </div>

            <motion.button
                whileHover={{ scale: 1.02 }}
                whileTap={{ scale: 0.98 }}
                onClick={isProcessing ? handleStop : handleStart}
                className={`w-full py-4 rounded-xl flex items-center justify-center gap-3 font-bold transition-all ${isProcessing
                    ? 'bg-red-500/20 text-red-500 border border-red-500/30 hover:bg-red-500/30'
                    : 'bg-accent-blue text-white shadow-[0_0_20px_rgba(0,112,243,0.4)] hover:shadow-[0_0_30px_rgba(0,112,243,0.6)]'
                    }`}
            >
                {isProcessing ? (
                    <>
                        <Square className="w-5 h-5 fill-current" />
                        ABORT SEQUENCE
                    </>
                ) : (
                    <>
                        <Play className="w-5 h-5 fill-current" />
                        START {mode === 'auto' ? 'AUTO' : 'SCHEDULED'} CAPTURE
                    </>
                )}
            </motion.button>
        </div>
    );
};
