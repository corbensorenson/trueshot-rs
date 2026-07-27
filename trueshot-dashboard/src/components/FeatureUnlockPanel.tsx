import { Sparkles, Zap, BadgeDollarSign, CheckCircle2 } from 'lucide-react';

interface FeatureUnlockPanelProps {
    title: string;
    subtitle: string;
    bundleName: string;
    priceLabel: string;
    capabilities: string[];
    trialAvailable: boolean;
    trialCtaLabel?: string;
    onStartTrial: () => void;
    onBuy: () => void;
    busy?: boolean;
    errorMessage?: string | null;
}

export const FeatureUnlockPanel = ({
    title,
    subtitle,
    bundleName,
    priceLabel,
    capabilities,
    trialAvailable,
    trialCtaLabel = 'Start Trial',
    onStartTrial,
    onBuy,
    busy = false,
    errorMessage,
}: FeatureUnlockPanelProps) => {
    return (
        <div className="rounded-2xl border border-accent-cyan/30 bg-gradient-to-br from-accent-cyan/10 via-accent-blue/10 to-white/5 p-6 space-y-5">
            <div className="flex items-start gap-4">
                <div className="p-3 rounded-xl bg-accent-cyan/20 border border-accent-cyan/40">
                    <Sparkles className="w-6 h-6 text-accent-cyan" />
                </div>
                <div className="flex-1">
                    <div className="text-xs uppercase tracking-[0.18em] text-accent-cyan font-bold">Upgrade Available</div>
                    <h4 className="text-2xl font-black text-[color:var(--ts-text)] mt-1">{title}</h4>
                    <p className="text-[color:var(--ts-muted)] mt-1">{subtitle}</p>
                </div>
                <div className="text-right">
                    <div className="text-xs uppercase tracking-widest text-[color:var(--ts-muted)]">Bundle</div>
                    <div className="font-bold text-[color:var(--ts-text)]">{bundleName}</div>
                    <div className="text-accent-cyan font-black mt-1">{priceLabel}</div>
                </div>
            </div>

            <div className="grid grid-cols-2 gap-3">
                {capabilities.map(item => (
                    <div key={item} className="flex items-center gap-2 text-sm text-[color:var(--ts-text)] bg-[color:var(--ts-surface-muted)] border border-[color:var(--ts-border)] rounded-lg px-3 py-2">
                        <CheckCircle2 className="w-4 h-4 text-accent-cyan" />
                        {item}
                    </div>
                ))}
            </div>

            {errorMessage && (
                <div className="text-sm text-yellow-300 bg-yellow-500/10 border border-yellow-500/20 rounded-lg px-3 py-2">
                    {errorMessage}
                </div>
            )}

            <div className="flex items-center gap-3">
                <button
                    onClick={onStartTrial}
                    disabled={!trialAvailable || busy}
                    className="px-5 py-3 rounded-xl bg-accent-cyan text-black font-bold uppercase tracking-wider flex items-center gap-2 disabled:opacity-50 disabled:cursor-not-allowed"
                >
                    <Zap className="w-4 h-4" />
                    {trialCtaLabel}
                </button>
                <button
                    onClick={onBuy}
                    className="px-5 py-3 rounded-xl bg-[color:var(--ts-surface-muted)] hover:bg-[color:var(--ts-surface-elevated)] text-[color:var(--ts-text)] font-bold uppercase tracking-wider border border-[color:var(--ts-border)] flex items-center gap-2"
                >
                    <BadgeDollarSign className="w-4 h-4" />
                    Buy Lifetime
                </button>
            </div>
        </div>
    );
};
