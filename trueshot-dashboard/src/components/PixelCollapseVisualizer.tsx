import { motion } from 'framer-motion';

export const PixelCollapseVisualizer = () => {
    return (
        <div className="flex flex-col gap-2 p-6 glass-panel h-full overflow-hidden relative">
            <h2 className="text-[10px] font-bold uppercase tracking-widest text-white/40 mb-4">Hierarchical Pixel Collapse (Bayer Domain)</h2>

            <div className="flex-1 flex items-center justify-center">
                <div className="relative w-48 h-48">
                    {/* Central Point */}
                    <motion.div
                        animate={{
                            scale: [1, 1.2, 1],
                            opacity: [0.5, 1, 0.5],
                            boxShadow: [
                                '0 0 20px rgba(0, 112, 243, 0.2)',
                                '0 0 40px rgba(0, 112, 243, 0.6)',
                                '0 0 20px rgba(0, 112, 243, 0.2)'
                            ]
                        }}
                        transition={{ duration: 2, repeat: Infinity }}
                        className="absolute inset-0 m-auto w-4 h-4 bg-accent-blue rounded-full z-20"
                    />

                    {/* Orbiting Particles */}
                    {[...Array(12)].map((_, i) => (
                        <motion.div
                            key={i}
                            animate={{
                                rotate: 360,
                                scale: [1, 0.8, 1],
                            }}
                            transition={{
                                duration: 3 + (i % 5) * 0.4,
                                repeat: Infinity,
                                ease: "linear",
                                delay: i * 0.2
                            }}
                            style={{
                                position: 'absolute',
                                top: '50%',
                                left: '50%',
                                width: '100%',
                                height: '2px',
                                transformOrigin: '0% 50%',
                            }}
                        >
                            <div className="absolute right-0 w-1.5 h-1.5 bg-accent-cyan rounded-full shadow-[0_0_10px_#00dfd8]" />
                        </motion.div>
                    ))}

                    {/* Hexagonal Grid Overlay */}
                    <div className="absolute inset-[-20px] border border-white/5 rounded-full scale-150 opacity-20" />
                    <div className="absolute inset-[-40px] border border-white/5 rounded-full scale-110 opacity-10" />
                </div>
            </div>

            <div className="mt-4 grid grid-cols-3 gap-2">
                <div className="flex flex-col">
                    <span className="text-[9px] text-white/40 uppercase">Entropy</span>
                    <span className="text-xs font-mono font-bold text-accent-cyan tabular-nums">0.024bit</span>
                </div>
                <div className="flex flex-col">
                    <span className="text-[9px] text-white/40 uppercase">Phase</span>
                    <span className="text-xs font-mono font-bold text-accent-cyan tabular-nums">Φc=0.99</span>
                </div>
                <div className="flex flex-col">
                    <span className="text-[9px] text-white/40 uppercase">SNR</span>
                    <span className="text-xs font-mono font-bold text-accent-cyan tabular-nums">48.2dB</span>
                </div>
            </div>
        </div>
    );
};
