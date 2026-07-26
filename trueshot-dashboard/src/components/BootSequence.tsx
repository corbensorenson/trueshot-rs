import { useState, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { Radio, CheckCircle2 } from 'lucide-react';

export const BootSequence = ({ onComplete }: { onComplete: () => void }) => {
    const [steps, setSteps] = useState([
        { id: 1, text: "Initializing TrueShot Kernel v6.0...", status: "pending" },
        { id: 2, text: "Establishing WebSocket Uplink...", status: "pending" },
        { id: 3, text: "Verifying GPU Acceleration (METAL)...", status: "pending" },
        { id: 4, text: "Calibrating Sensors...", status: "pending" },
        { id: 5, text: "System Ready via Localhost:3000", status: "pending" },
    ]);

    useEffect(() => {
        let currentStep = 0;
        const interval = setInterval(() => {
            if (currentStep >= steps.length) {
                clearInterval(interval);
                setTimeout(onComplete, 800);
                return;
            }

            setSteps(prev => prev.map((s, i) =>
                i === currentStep ? { ...s, status: "complete" } : s
            ));
            currentStep++;
        }, 300); // Speed up boot slightly

        return () => clearInterval(interval);
    }, [onComplete, steps.length]);

    return (
        <div className="fixed inset-0 bg-[#050505] z-[100] flex flex-col items-center justify-center font-mono text-accent-cyan">
            {/* Background Grid */}
            <div className="absolute inset-0 opacity-10" style={{ backgroundImage: 'linear-gradient(0deg, transparent 24%, rgba(0, 223, 216, .3) 25%, rgba(0, 223, 216, .3) 26%, transparent 27%, transparent 74%, rgba(0, 223, 216, .3) 75%, rgba(0, 223, 216, .3) 76%, transparent 77%, transparent), linear-gradient(90deg, transparent 24%, rgba(0, 223, 216, .3) 25%, rgba(0, 223, 216, .3) 26%, transparent 27%, transparent 74%, rgba(0, 223, 216, .3) 75%, rgba(0, 223, 216, .3) 76%, transparent 77%, transparent)', backgroundSize: '50px 50px' }} />

            <div className="w-96 space-y-4 relative z-10">
                <div className="flex items-center gap-4 mb-8">
                    <Radio className="w-8 h-8 animate-pulse text-accent-cyan" />
                    <h1 className="text-2xl font-black tracking-tighter uppercase italic">TrueShot<span className="text-white/40">OS</span></h1>
                </div>

                <div className="space-y-3">
                    {steps.map((step) => (
                        <div key={step.id} className="flex items-center gap-3 text-xs tracking-wider">
                            <AnimatePresence mode='wait'>
                                {step.status === 'complete' ? (
                                    <motion.div initial={{ scale: 0 }} animate={{ scale: 1 }}>
                                        <CheckCircle2 className="w-4 h-4 text-green-500" />
                                    </motion.div>
                                ) : (
                                    <div className="w-4 h-4 flex items-center justify-center">
                                        <div className="w-1.5 h-1.5 bg-accent-cyan/50 rounded-full animate-ping" />
                                    </div>
                                )}
                            </AnimatePresence>
                            <span className={step.status === 'complete' ? 'text-white' : 'text-white/40'}>{step.text}</span>
                        </div>
                    ))}
                </div>
            </div>

            <div className="absolute bottom-8 left-8 text-[10px] text-white/20 uppercase tracking-[0.3em]">
                Augment Technologies • Proprietary • v6.0.0-rc1
            </div>
        </div>
    );
};
