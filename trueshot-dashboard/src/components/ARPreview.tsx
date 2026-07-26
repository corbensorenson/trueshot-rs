import { QRCodeSVG } from 'qrcode.react';
import { motion, AnimatePresence } from 'framer-motion';
import { X, Smartphone, Download, Ruler } from 'lucide-react';
import { useEffect, useState } from 'react';
import { wizard, ScaleAnchorStatus } from '../api/client';
import toast from 'react-hot-toast';

export const ARPreview = ({ modelUrl, isOpen, onClose }: { modelUrl: string, isOpen: boolean, onClose: () => void }) => {
    const [anchorStatus, setAnchorStatus] = useState<ScaleAnchorStatus | null>(null);
    const [knownDistance, setKnownDistance] = useState('');
    const [measuredUnits, setMeasuredUnits] = useState('');
    const [label, setLabel] = useState('');
    const [originLat, setOriginLat] = useState('');
    const [originLon, setOriginLon] = useState('');
    const [originAlt, setOriginAlt] = useState('');
    const [crs, setCrs] = useState('');
    const [saving, setSaving] = useState(false);

    useEffect(() => {
        if (!isOpen) return;
        wizard.getScaleAnchor()
            .then((status) => {
                setAnchorStatus(status);
                if (status.anchor) {
                    setKnownDistance(status.anchor.known_distance_m.toString());
                    setMeasuredUnits(status.anchor.measured_units.toString());
                    setLabel(status.anchor.label ?? '');
                    setOriginLat(status.anchor.origin_lat?.toString() ?? '');
                    setOriginLon(status.anchor.origin_lon?.toString() ?? '');
                    setOriginAlt(status.anchor.origin_alt?.toString() ?? '');
                    setCrs(status.anchor.crs ?? '');
                }
            })
            .catch((err) => {
                toast.error(err instanceof Error ? err.message : 'Failed to load scale anchor');
            });
    }, [isOpen]);

    const saveAnchor = async () => {
        const known = parseFloat(knownDistance);
        const measured = parseFloat(measuredUnits);
        if (!Number.isFinite(known) || !Number.isFinite(measured) || known <= 0 || measured <= 0) {
            toast.error('Enter valid known distance and measured units');
            return;
        }
        setSaving(true);
        try {
            const anchor = await wizard.setScaleAnchor({
                known_distance_m: known,
                measured_units: measured,
                label: label.trim() || null,
                origin_lat: originLat ? parseFloat(originLat) : null,
                origin_lon: originLon ? parseFloat(originLon) : null,
                origin_alt: originAlt ? parseFloat(originAlt) : null,
                crs: crs.trim() || null,
            });
            setAnchorStatus({ configured: true, anchor });
            toast.success('Scale anchor saved');
        } catch (err) {
            toast.error(err instanceof Error ? err.message : 'Failed to save scale anchor');
        } finally {
            setSaving(false);
        }
    };

    return (
        <AnimatePresence>
            {isOpen && (
                <motion.div
                    initial={{ opacity: 0 }}
                    animate={{ opacity: 1 }}
                    exit={{ opacity: 0 }}
                    className="fixed inset-0 z-[60] flex items-center justify-center bg-[color:var(--ts-overlay-strong)] backdrop-blur-sm pointer-events-auto"
                >
                    <motion.div
                        initial={{ scale: 0.9, y: 20 }}
                        animate={{ scale: 1, y: 0 }}
                        className="ts-panel-strong p-8 max-w-md w-full relative flex flex-col items-center gap-6"
                    >
                        <button onClick={onClose} className="absolute top-4 right-4 text-[color:var(--ts-muted)] hover:text-[color:var(--ts-text)]">
                            <X className="w-5 h-5" />
                        </button>

                        <div className="w-16 h-16 rounded-full bg-accent-cyan/10 flex items-center justify-center mb-2">
                            <Smartphone className="w-8 h-8 text-accent-cyan" />
                        </div>

                        <div className="text-center space-y-2">
                            <h2 className="text-xl font-bold text-[color:var(--ts-text)]">AR Mobile Preview</h2>
                            <p className="text-sm text-[color:color-mix(in_srgb,var(--ts-text)_60%,transparent)]">Scan to view this model in Augmented Reality on iOS or Android.</p>
                        </div>

                        <div className="p-4 bg-white rounded-xl">
                            <QRCodeSVG value={modelUrl} size={180} level="H" includeMargin={false} />
                        </div>

                        <a href={modelUrl} download className="flex items-center gap-2 text-xs text-accent-cyan hover:underline mt-2">
                            <Download className="w-3 h-3" />
                            Download .USDZ File
                        </a>

                        <div className="w-full border-t border-[color:var(--ts-border)] pt-4 space-y-3">
                            <div className="flex items-center gap-2 text-[11px] uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">
                                <Ruler className="w-3 h-3" /> Scale Anchor
                            </div>
                            {anchorStatus?.anchor && (
                                <div className="text-xs text-[color:color-mix(in_srgb,var(--ts-text)_70%,transparent)]">
                                    Current scale: 1 unit = {anchorStatus.anchor.meters_per_unit.toFixed(4)} m
                                </div>
                            )}
                            <div className="grid grid-cols-2 gap-3">
                                <div>
                                    <label className="text-[10px] uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">Known Distance (m)</label>
                                    <input
                                        value={knownDistance}
                                        onChange={(e) => setKnownDistance(e.target.value)}
                                        className="mt-1 w-full px-3 py-2 text-xs ts-input"
                                        placeholder="1.0"
                                    />
                                </div>
                                <div>
                                    <label className="text-[10px] uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">Measured Units</label>
                                    <input
                                        value={measuredUnits}
                                        onChange={(e) => setMeasuredUnits(e.target.value)}
                                        className="mt-1 w-full px-3 py-2 text-xs ts-input"
                                        placeholder="0.78"
                                    />
                                </div>
                                <div className="col-span-2">
                                    <label className="text-[10px] uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">Label (optional)</label>
                                    <input
                                        value={label}
                                        onChange={(e) => setLabel(e.target.value)}
                                        className="mt-1 w-full px-3 py-2 text-xs ts-input"
                                        placeholder="Tape measure on floor"
                                    />
                                </div>
                            </div>
                            <div className="grid grid-cols-2 gap-3">
                                <div>
                                    <label className="text-[10px] uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">Origin Lat</label>
                                    <input
                                        value={originLat}
                                        onChange={(e) => setOriginLat(e.target.value)}
                                        className="mt-1 w-full px-3 py-2 text-xs ts-input"
                                        placeholder="37.7749"
                                    />
                                </div>
                                <div>
                                    <label className="text-[10px] uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">Origin Lon</label>
                                    <input
                                        value={originLon}
                                        onChange={(e) => setOriginLon(e.target.value)}
                                        className="mt-1 w-full px-3 py-2 text-xs ts-input"
                                        placeholder="-122.4194"
                                    />
                                </div>
                                <div>
                                    <label className="text-[10px] uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">Origin Alt (m)</label>
                                    <input
                                        value={originAlt}
                                        onChange={(e) => setOriginAlt(e.target.value)}
                                        className="mt-1 w-full px-3 py-2 text-xs ts-input"
                                        placeholder="12.0"
                                    />
                                </div>
                                <div>
                                    <label className="text-[10px] uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">CRS (optional)</label>
                                    <input
                                        value={crs}
                                        onChange={(e) => setCrs(e.target.value)}
                                        className="mt-1 w-full px-3 py-2 text-xs ts-input"
                                        placeholder="EPSG:4326"
                                    />
                                </div>
                            </div>
                            <button
                                onClick={saveAnchor}
                                disabled={saving}
                                className="w-full rounded-lg py-2 text-[11px] font-semibold uppercase tracking-[0.2em] ts-button-primary disabled:opacity-40"
                            >
                                {saving ? 'Saving...' : 'Save Scale Anchor'}
                            </button>
                        </div>
                    </motion.div>
                </motion.div>
            )}
        </AnimatePresence>
    );
};
