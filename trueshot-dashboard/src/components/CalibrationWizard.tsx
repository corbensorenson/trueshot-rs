import { useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { Crosshair, Camera, Check, RefreshCw, X } from 'lucide-react';
import { captureCalibrationFrame, computeCalibration, clearCalibrationSession, STREAM_BASE } from '../api/client';
import toast from 'react-hot-toast';

export const CalibrationWizard = ({ onClose }: { onClose: () => void }) => {
    const [step, setStep] = useState<"intro" | "capture" | "compute" | "result">("intro");
    const [frames, setFrames] = useState<CalibrationFrame[]>([]);
    const [result, setResult] = useState<CalibrationResult | null>(null);
    const [loading, setLoading] = useState(false);

    const handleCapture = async () => {
        setLoading(true);
        try {
            const res = await captureCalibrationFrame();
            setFrames(prev => [...prev, res]);
            toast.success(`Frame ${res.frame_id} Captured`);
        } catch {
            toast.error("Capture Failed");
        } finally {
            setLoading(false);
        }
    };

    const handleCompute = async () => {
        setStep("compute");
        try {
            const res = await computeCalibration();
            setResult(res);
            setStep("result");
        } catch {
            toast.error("Calibration Failed");
            setStep("capture");
        }
    };

    const reset = async () => {
        await clearCalibrationSession();
        setFrames([]);
        setStep("intro");
    };

    return (
        <div className="fixed inset-0 z-50 bg-black/80 backdrop-blur-md flex items-center justify-center p-6">
            <motion.div
                initial={{ opacity: 0, scale: 0.9 }}
                animate={{ opacity: 1, scale: 1 }}
                className="w-full max-w-2xl bg-[#0a0a0a] border border-white/10 rounded-2xl overflow-hidden shadow-2xl flex flex-col"
            >
                {/* Header */}
                <div className="p-6 border-b border-white/10 flex items-center justify-between bg-white/5">
                    <div className="flex items-center gap-3">
                        <Crosshair className="w-5 h-5 text-accent-cyan" />
                        <h2 className="font-bold tracking-wide">Sensor Calibration Protocol</h2>
                    </div>
                    <button onClick={onClose}><X className="w-5 h-5 text-white/50 hover:text-white" /></button>
                </div>

                {/* Body */}
                <div className="p-8 min-h-[400px] flex flex-col">
                    <AnimatePresence mode="wait">
                        {step === "intro" && (
                            <motion.div key="intro" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="flex-1 flex flex-col items-center justify-center text-center gap-6">
                                <div className="w-20 h-20 rounded-full bg-accent-cyan/10 flex items-center justify-center">
                                    <Crosshair className="w-10 h-10 text-accent-cyan" />
                                </div>
                                <div className="space-y-2 max-w-md">
                                    <h3 className="text-xl font-bold">Intrinsic Parameter Estimation</h3>
                                    <p className="text-white/60 text-sm">This process will compute the focal length, principal point, and distortion coefficients for the connected Nikon Z9.</p>
                                </div>
                                <div className="p-4 bg-white/5 rounded-lg text-left text-xs space-y-2 border border-white/10">
                                    <p className="font-bold text-white/80">Requirements:</p>
                                    <ul className="list-disc pl-4 space-y-1 text-white/50">
                                        <li>9x6 Checkerboard Printed Target</li>
                                        <li>Flat surface with even lighting</li>
                                        <li>Approx 5-10 minutes</li>
                                    </ul>
                                </div>
                                <button onClick={() => setStep("capture")} className="px-8 py-3 bg-white hover:bg-white/90 text-black font-bold rounded-lg transition-colors">
                                    Initialize Sequence
                                </button>
                            </motion.div>
                        )}

                        {step === "capture" && (
                            <motion.div key="capture" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="flex-1 flex flex-col gap-6">
                                <div className="flex items-center justify-between">
                                    <h3 className="text-lg font-bold">Acquisition Phase</h3>
                                    <span className="text-sm font-mono text-accent-cyan">{frames.length} / 10 Frames</span>
                                </div>

                                {/* Placeholder for Last Frame Preview */}
                                <div className="flex-1 bg-black rounded-xl border border-white/10 relative overflow-hidden group">
                                    {frames.length > 0 ? (
                                        <img src={`${STREAM_BASE}/${frames[frames.length - 1].path}`} className="w-full h-full object-contain" />
                                    ) : (
                                        <div className="absolute inset-0 flex items-center justify-center text-white/20 text-sm">
                                            No Frames Acquired
                                        </div>
                                    )}

                                    {/* Virtual Overlay */}
                                    <div className="absolute inset-0 pointer-events-none border-2 border-white/5 m-8 rounded-lg border-dashed opacity-20" />
                                </div>

                                <div className="flex gap-4">
                                    <button
                                        disabled={loading}
                                        onClick={handleCapture}
                                        className="flex-1 py-4 bg-accent-blue hover:bg-accent-blue/80 rounded-xl font-bold flex items-center justify-center gap-2 transition-all"
                                    >
                                        <Camera className="w-5 h-5" />
                                        {loading ? "Capturing..." : "Capture Frame"}
                                    </button>
                                    <button
                                        onClick={handleCompute}
                                        disabled={frames.length < 5}
                                        className="px-8 py-4 bg-white/10 hover:bg-white/20 disabled:opacity-50 disabled:cursor-not-allowed rounded-xl font-bold transition-all"
                                    >
                                        Compute
                                    </button>
                                </div>
                            </motion.div>
                        )}

                        {step === "compute" && (
                            <motion.div key="compute" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="flex-1 flex flex-col items-center justify-center text-center gap-6">
                                <RefreshCw className="w-16 h-16 text-accent-cyan animate-spin" />
                                <h3 className="text-xl font-bold">Solving...</h3>
                            </motion.div>
                        )}

                        {step === "result" && (
                            <motion.div key="result" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="flex-1 flex flex-col items-center justify-center text-center gap-6">
                                {result?.success ? (
                                    <>
                                        <div className="w-20 h-20 rounded-full bg-green-500/20 flex items-center justify-center">
                                            <Check className="w-10 h-10 text-green-500" />
                                        </div>
                                        <h3 className="text-xl font-bold text-green-400">Calibration Successful</h3>
                                        <div className="bg-white/5 p-6 rounded-xl border border-white/10 font-mono text-sm w-full max-w-sm">
                                            <div className="flex justify-between mb-2">
                                                <span className="text-white/50">RMS Error:</span>
                                                <span className="text-white font-bold">{result.rms_error?.toFixed(4)}px</span>
                                            </div>
                                            <div className="flex justify-between">
                                                <span className="text-white/50">Status:</span>
                                                <span className="text-green-500">SAVED</span>
                                            </div>
                                        </div>
                                    </>
                                ) : (
                                    <>
                                        <div className="w-20 h-20 rounded-full bg-red-500/20 flex items-center justify-center">
                                            <X className="w-10 h-10 text-red-500" />
                                        </div>
                                        <h3 className="text-xl font-bold text-red-400">Calibration Failed</h3>
                                        <p className="text-white/60">{result?.message}</p>
                                    </>
                                )}
                                <div className="flex gap-4 w-full">
                                    <button onClick={reset} className="flex-1 py-3 bg-white/10 rounded-lg font-bold">Restart</button>
                                    <button onClick={onClose} className="flex-1 py-3 bg-white text-black rounded-lg font-bold">Finish</button>
                                </div>
                            </motion.div>
                        )}
                    </AnimatePresence>
                </div>
            </motion.div>
        </div>
    );
};

interface CalibrationFrame {
    frame_id: number;
    path: string;
}

interface CalibrationResult {
    success: boolean;
    rms_error?: number;
    message?: string;
}
