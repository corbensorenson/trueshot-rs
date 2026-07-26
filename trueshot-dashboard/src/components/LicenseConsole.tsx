import { useEffect, useMemo, useState } from 'react';
import {
    createLicenseTrial,
    importLicense,
    activateLicenseKey,
    getLicenseBundles,
    getLicenseDevices,
    getLicenseStatus,
    getLicenseTiers,
    activateLicenseDevice,
    deactivateLicenseDevice,
    LicenseBundleInfo,
    LicenseDeviceInfo,
    LicenseStatusResponse,
    LicenseTierInfo,
} from '../api/client';
import { BadgeCheck, BadgeDollarSign, Clock, ShieldAlert } from 'lucide-react';

interface LicenseConsoleProps {
    isOpen: boolean;
    onClose: () => void;
}

const statusLabel = (status?: string) => {
    switch (status) {
        case 'valid':
            return 'Active';
        case 'development':
            return 'Development';
        case 'expired':
            return 'Expired';
        case 'not_activated':
            return 'Not Activated';
        case 'grace_period_expired':
            return 'Grace Expired';
        case 'unlicensed':
            return 'Unlicensed';
        default:
            return 'Unavailable';
    }
};

const statusTone = (status?: string) => {
    switch (status) {
        case 'valid':
        case 'development':
            return 'text-emerald-400';
        case 'expired':
        case 'grace_period_expired':
            return 'text-amber-300';
        case 'not_activated':
        case 'unlicensed':
            return 'text-red-400';
        default:
            return 'text-white/60';
    }
};

export const LicenseConsole = ({ isOpen, onClose }: LicenseConsoleProps) => {
    const [status, setStatus] = useState<LicenseStatusResponse | null>(null);
    const [bundles, setBundles] = useState<LicenseBundleInfo[]>([]);
    const [tiers, setTiers] = useState<LicenseTierInfo[]>([]);
    const [devices, setDevices] = useState<LicenseDeviceInfo[]>([]);
    const [loading, setLoading] = useState(false);
    const [actionBusy, setActionBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [licenseDraft, setLicenseDraft] = useState('');
    const [licenseKeyDraft, setLicenseKeyDraft] = useState('');
    const [deviceBusy, setDeviceBusy] = useState<string | null>(null);

    const refresh = async () => {
        setLoading(true);
        setError(null);
        try {
            const [statusResp, bundleResp, tierResp] = await Promise.all([
                getLicenseStatus(),
                getLicenseBundles(),
                getLicenseTiers(),
            ]);
            setStatus(statusResp);
            setBundles(bundleResp);
            setTiers(tierResp);
            try {
                const deviceResp = await getLicenseDevices();
                setDevices(deviceResp);
            } catch {
                setDevices([]);
            }
        } catch (err: unknown) {
            const message = err instanceof Error ? err.message : 'Failed to load license status';
            setError(message);
        } finally {
            setLoading(false);
        }
    };

    useEffect(() => {
        if (isOpen) {
            refresh();
        }
    }, [isOpen]);

    const missingBundles = useMemo(() => {
        if (!status) return [];
        return bundles
            .filter((bundle) => !status.bundles[bundle.key])
            .map((bundle) => bundle.key);
    }, [status, bundles]);

    const handleStartTrial = async () => {
        if (!status || !status.trial_available) return;
        setActionBusy(true);
        setError(null);
        try {
            await createLicenseTrial({
                duration_days: 14,
                bundles: missingBundles.length ? missingBundles : undefined,
            });
            await refresh();
        } catch (err: unknown) {
            const message = err instanceof Error ? err.message : 'Failed to start trial';
            setError(message);
        } finally {
            setActionBusy(false);
        }
    };

    const handleImportLicense = async () => {
        if (!licenseDraft.trim()) return;
        setActionBusy(true);
        setError(null);
        try {
            await importLicense({ license_json: licenseDraft.trim() });
            setLicenseDraft('');
            await refresh();
        } catch (err: unknown) {
            const message = err instanceof Error ? err.message : 'Failed to import license';
            setError(message);
        } finally {
            setActionBusy(false);
        }
    };

    const handleActivateKey = async () => {
        if (!licenseKeyDraft.trim()) return;
        setActionBusy(true);
        setError(null);
        try {
            await activateLicenseKey({ license_key: licenseKeyDraft.trim() });
            setLicenseKeyDraft('');
            await refresh();
        } catch (err: unknown) {
            const message = err instanceof Error ? err.message : 'Failed to activate license';
            setError(message);
        } finally {
            setActionBusy(false);
        }
    };

    const handleActivateDevice = async () => {
        setDeviceBusy('activate');
        setError(null);
        try {
            await activateLicenseDevice({});
            await refresh();
        } catch (err: unknown) {
            const message = err instanceof Error ? err.message : 'Failed to activate device';
            setError(message);
        } finally {
            setDeviceBusy(null);
        }
    };

    const handleDeactivateDevice = async (fingerprint: string) => {
        setDeviceBusy(fingerprint);
        setError(null);
        try {
            await deactivateLicenseDevice({ fingerprint_hash: fingerprint });
            await refresh();
        } catch (err: unknown) {
            const message = err instanceof Error ? err.message : 'Failed to deactivate device';
            setError(message);
        } finally {
            setDeviceBusy(null);
        }
    };

    const handlePurchase = (bundleName: string) => {
        const subject = encodeURIComponent(`TrueShot purchase: ${bundleName}`);
        const body = encodeURIComponent('Hi TrueShot team,\n\nI would like to purchase this add-on.\n\nThanks!');
        window.open(`mailto:sales@trueshot.ai?subject=${subject}&body=${body}`, '_blank');
    };

    if (!isOpen) return null;

    return (
        <div className="fixed inset-0 z-[120] flex items-center justify-center bg-black/70 backdrop-blur-xl">
            <div className="w-[860px] max-h-[90vh] overflow-hidden rounded-2xl border border-[color:var(--ts-border)] bg-[color:var(--ts-surface)] p-6 shadow-2xl text-[color:var(--ts-text)]">
                <div className="flex items-start justify-between">
                    <div>
                        <div className="text-[10px] uppercase tracking-[0.3em] text-[color:var(--ts-muted)]">License & Plans</div>
                        <div className="mt-2 text-xl font-semibold">Entitlements, trials, and add-ons</div>
                    </div>
                    <button
                        onClick={onClose}
                        className="rounded-lg border border-[color:var(--ts-border)] px-3 py-1 text-xs uppercase tracking-[0.2em] text-[color:var(--ts-muted)] hover:text-[color:var(--ts-text)]"
                    >
                        Close
                    </button>
                </div>

                <div className="mt-6 grid grid-cols-2 gap-6">
                    <div className="rounded-xl border border-[color:var(--ts-border)] bg-[color:color-mix(in_srgb,var(--ts-surface-strong)_80%,transparent)] p-4">
                        <div className="text-[10px] uppercase tracking-[0.25em] text-[color:var(--ts-muted)]">Current license</div>
                        <div className="mt-3 flex items-center justify-between">
                            <div>
                                <div className={`text-lg font-semibold ${statusTone(status?.status)}`}>
                                    {statusLabel(status?.status)}
                                </div>
                                <div className="text-sm text-[color:var(--ts-muted)]">
                                    Tier: {status?.tier || 'None'}
                                </div>
                            </div>
                            <BadgeCheck className="w-6 h-6 text-accent-cyan" />
                        </div>
                        <div className="mt-3 text-xs text-[color:var(--ts-muted)]">
                            Expires: {status?.expires_at ? new Date(status.expires_at).toLocaleString() : 'Never'}
                        </div>
                        {status?.trial_active && (
                            <div className="mt-2 text-xs text-amber-200">
                                Trial active: {status.trial_days_remaining ?? 'n/a'} day(s) remaining.
                            </div>
                        )}
                        {status?.trial_available ? (
                            <button
                                onClick={handleStartTrial}
                                disabled={actionBusy}
                                className="mt-4 w-full rounded-lg bg-accent-cyan px-4 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-black disabled:opacity-60"
                            >
                                Start Trial (All Add-ons)
                            </button>
                        ) : (
                            <div className="mt-4 flex items-center gap-2 text-xs text-[color:var(--ts-muted)]">
                                <Clock className="w-4 h-4" />
                                Trial not available ({status?.trial_reason || 'n/a'})
                            </div>
                        )}
                    </div>

                    <div className="rounded-xl border border-[color:var(--ts-border)] bg-[color:color-mix(in_srgb,var(--ts-surface-strong)_80%,transparent)] p-4">
                        <div className="text-[10px] uppercase tracking-[0.25em] text-[color:var(--ts-muted)]">Entitlements</div>
                        <div className="mt-3 grid grid-cols-2 gap-2">
                            {Object.entries(status?.bundles ?? {}).map(([key, enabled]) => (
                                <div
                                    key={key}
                                    className={`rounded-lg border px-3 py-2 text-xs font-semibold uppercase tracking-[0.18em] ${
                                        enabled
                                            ? 'border-accent-cyan/40 text-accent-cyan bg-accent-cyan/10'
                                            : 'border-[color:var(--ts-border)] text-[color:var(--ts-muted)]'
                                    }`}
                                >
                                    {key.replace('_', ' ')}
                                </div>
                            ))}
                            {!status && !loading && (
                                <div className="text-xs text-[color:var(--ts-muted)]">No entitlements available.</div>
                            )}
                        </div>
                        {error && (
                            <div className="mt-3 flex items-center gap-2 text-xs text-amber-300">
                                <ShieldAlert className="w-4 h-4" />
                                {error}
                            </div>
                        )}
                    </div>
                </div>

                <div className="mt-6 grid grid-cols-2 gap-4">
                    <div className="rounded-xl border border-[color:var(--ts-border)] bg-[color:color-mix(in_srgb,var(--ts-surface-strong)_70%,transparent)] p-4">
                        <div className="text-[10px] uppercase tracking-[0.25em] text-[color:var(--ts-muted)]">License activation</div>
                        <div className="mt-3 text-xs text-[color:var(--ts-muted)]">
                            Enter your license key to activate this device online.
                        </div>
                        <div className="mt-3 flex items-center gap-3">
                            <input
                                value={licenseKeyDraft}
                                onChange={(e) => setLicenseKeyDraft(e.target.value)}
                                className="flex-1 rounded-lg border border-[color:var(--ts-border)] bg-[color:var(--ts-surface)] px-3 py-2 text-xs text-[color:var(--ts-text)]"
                                placeholder="XXXX-XXXX-XXXX-XXXX"
                            />
                            <button
                                onClick={handleActivateKey}
                                disabled={actionBusy || !licenseKeyDraft.trim()}
                                className="rounded-lg bg-accent-cyan px-4 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-black disabled:opacity-60"
                            >
                                Activate Key
                            </button>
                        </div>
                        <div className="mt-3 text-xs text-[color:var(--ts-muted)]">
                            Paste the license JSON payload to activate this device. Activation consumes a seat.
                        </div>
                        <textarea
                            value={licenseDraft}
                            onChange={(e) => setLicenseDraft(e.target.value)}
                            className="mt-3 w-full min-h-[120px] rounded-lg border border-[color:var(--ts-border)] bg-[color:var(--ts-surface)] p-3 text-xs text-[color:var(--ts-text)]"
                            placeholder="Paste license JSON here"
                        />
                        <div className="mt-3 flex items-center gap-3">
                            <button
                                onClick={handleImportLicense}
                                disabled={actionBusy || !licenseDraft.trim()}
                                className="rounded-lg bg-accent-cyan px-4 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-black disabled:opacity-60"
                            >
                                Import & Activate
                            </button>
                            {status?.status === 'not_activated' && (
                                <button
                                    onClick={handleActivateDevice}
                                    disabled={deviceBusy === 'activate'}
                                    className="rounded-lg border border-[color:var(--ts-border)] px-4 py-2 text-xs uppercase tracking-[0.2em] text-[color:var(--ts-text)] disabled:opacity-60"
                                >
                                    Activate This Device
                                </button>
                            )}
                        </div>
                    </div>

                    <div className="rounded-xl border border-[color:var(--ts-border)] bg-[color:color-mix(in_srgb,var(--ts-surface-strong)_70%,transparent)] p-4">
                        <div className="text-[10px] uppercase tracking-[0.25em] text-[color:var(--ts-muted)]">Device seats</div>
                        <div className="mt-3 space-y-2 max-h-[220px] overflow-y-auto custom-scrollbar pr-2">
                            {devices.length === 0 && (
                                <div className="text-xs text-[color:var(--ts-muted)]">No activated devices.</div>
                            )}
                            {devices.map((device) => (
                                <div key={device.fingerprint_hash} className="rounded-lg border border-[color:var(--ts-border)] bg-[color:var(--ts-surface)] p-3">
                                    <div className="flex items-center justify-between">
                                        <div>
                                            <div className="text-sm font-semibold">{device.device_name}</div>
                                            <div className="text-xs text-[color:var(--ts-muted)]">
                                                Last seen: {new Date(device.last_seen).toLocaleString()}
                                            </div>
                                        </div>
                                        <button
                                            onClick={() => handleDeactivateDevice(device.fingerprint_hash)}
                                            disabled={deviceBusy === device.fingerprint_hash}
                                            className="rounded-lg border border-red-400/40 px-3 py-1 text-xs uppercase tracking-[0.2em] text-red-300 disabled:opacity-60"
                                        >
                                            Revoke
                                        </button>
                                    </div>
                                </div>
                            ))}
                        </div>
                    </div>
                </div>

                <div className="mt-6 rounded-xl border border-[color:var(--ts-border)] bg-[color:color-mix(in_srgb,var(--ts-surface-strong)_70%,transparent)] p-4">
                    <div className="flex items-center justify-between">
                        <div className="text-[10px] uppercase tracking-[0.25em] text-[color:var(--ts-muted)]">Add-on catalog</div>
                        <button
                            onClick={refresh}
                            className="text-xs uppercase tracking-[0.2em] text-[color:var(--ts-muted)] hover:text-[color:var(--ts-text)]"
                        >
                            Refresh
                        </button>
                    </div>

                    <div className="mt-4 grid grid-cols-2 gap-4">
                        {bundles.map((bundle) => {
                            const enabled = status?.bundles?.[bundle.key] ?? false;
                            return (
                                <div key={bundle.key} className="rounded-xl border border-[color:var(--ts-border)] bg-[color:var(--ts-surface)] p-4">
                                    <div className="flex items-center justify-between">
                                        <div className="text-sm font-semibold">{bundle.name}</div>
                                        <div className="text-xs text-[color:var(--ts-muted)]">
                                            {enabled ? 'Active' : `$${bundle.price_usd} ${bundle.billing}`}
                                        </div>
                                    </div>
                                    <div className="mt-2 text-xs text-[color:var(--ts-muted)]">{bundle.description}</div>
                                    <div className="mt-3 flex flex-wrap gap-2">
                                        {bundle.features.slice(0, 4).map((feature) => (
                                            <span
                                                key={feature}
                                                className="rounded-full border border-[color:var(--ts-border)] px-2 py-1 text-[10px] uppercase tracking-[0.18em] text-[color:var(--ts-muted)]"
                                            >
                                                {feature.replace('_', ' ')}
                                            </span>
                                        ))}
                                    </div>
                                    {!enabled && (
                                        <button
                                            onClick={() => handlePurchase(bundle.name)}
                                            className="mt-4 inline-flex items-center gap-2 rounded-lg border border-[color:var(--ts-border)] px-3 py-2 text-xs uppercase tracking-[0.2em] text-[color:var(--ts-text)] hover:bg-white/5"
                                        >
                                            <BadgeDollarSign className="w-4 h-4" />
                                            Buy Lifetime
                                        </button>
                                    )}
                                </div>
                            );
                        })}
                        {!bundles.length && !loading && (
                            <div className="text-xs text-[color:var(--ts-muted)]">No bundles available.</div>
                        )}
                    </div>
                </div>

                <div className="mt-6 rounded-xl border border-[color:var(--ts-border)] bg-[color:color-mix(in_srgb,var(--ts-surface-strong)_70%,transparent)] p-4">
                    <div className="text-[10px] uppercase tracking-[0.25em] text-[color:var(--ts-muted)]">Core license tiers</div>
                    <div className="mt-4 grid grid-cols-3 gap-4">
                        {tiers.map((tier) => (
                            <div key={tier.key} className="rounded-xl border border-[color:var(--ts-border)] bg-[color:var(--ts-surface)] p-4">
                                <div className="text-sm font-semibold">{tier.name}</div>
                                <div className="mt-1 text-xs text-[color:var(--ts-muted)]">
                                    {tier.max_devices} device{tier.max_devices === 1 ? '' : 's'}
                                </div>
                                <div className="mt-3 text-sm font-semibold">
                                    {tier.price_usd ? `$${tier.price_usd} ${tier.billing || ''}` : 'Contact sales'}
                                </div>
                            </div>
                        ))}
                        {!tiers.length && !loading && (
                            <div className="text-xs text-[color:var(--ts-muted)]">No tier catalog available.</div>
                        )}
                    </div>
                </div>
            </div>
        </div>
    );
};
