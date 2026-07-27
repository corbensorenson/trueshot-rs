import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from 'react';
import { AlertTriangle, CheckCircle2, Copy, Crosshair, Download, Gauge, Layers3, Loader2, Save, ScanSearch, ShieldCheck, Trash2, X } from 'lucide-react';
import toast from 'react-hot-toast';
import {
  createLicenseTrial,
  createFusionEdit,
  fetchFusionArtifact,
  getLicenseBundles,
  getLicenseStatus,
  listFusionReports,
  type FusionArtifactRef,
  type FusionEditOperation,
  type FusionEditReason,
  type FusionEditReceipt,
  type FusionReportInventory,
  type LicenseBundleInfo,
} from '../api/client';
import { FeatureUnlockPanel } from './FeatureUnlockPanel';

interface FusionInspectorProps {
  projectId: string | null;
  open: boolean;
  onClose: () => void;
}

type LayerKey = 'overlay' | 'flags' | 'frequency_flags' | 'glare' | 'boundary' | 'sensor_correction' | 'edit';

const LAYERS: Array<{ key: LayerKey; label: string; description: string; filter?: string }> = [
  { key: 'overlay', label: 'Source + Deghost', description: 'Measured-source provenance with alignment, fallback, clipping, and disocclusion states.' },
  { key: 'flags', label: 'Fusion Flags', description: 'Exact censoring, rejection, visibility, alignment, and fallback bitfield.', filter: 'contrast(4) brightness(1.8)' },
  { key: 'frequency_flags', label: 'Frequency Split', description: 'Measured low/detail source separation and envelope-clamping bitfield.', filter: 'contrast(8) brightness(2.5)' },
  { key: 'glare', label: 'Glare Guard', description: 'Glare excluded from focus evidence without changing measured radiance.', filter: 'contrast(2)' },
  { key: 'boundary', label: 'Aperture Boundary', description: 'Physical interior, PSF-support, and depth-crossing trimap.', filter: 'contrast(40) brightness(3)' },
  { key: 'sensor_correction', label: 'Sensor Corrections', description: 'Same-CFA flat-field and persistent-defect provenance.', filter: 'contrast(20) brightness(2)' },
  { key: 'edit', label: 'Operator Revision', description: 'Exact pixels rebound to a selected aligned, uncensored measured RAW frame.', filter: 'contrast(20) brightness(2)' },
];

const OVERLAY_LEGEND = [
  ['Disoccluded', '#eb37d2'], ['Source fallback', '#f53741'], ['Censor conflict', '#ff327d'],
  ['Detail reference', '#ff5f2d'], ['Split sources', '#37dc91'], ['Frequency separated', '#3cbef5'],
  ['Outlier rejected', '#ff7d23'], ['Censored', '#facd2d'], ['Bracket aligned', '#14cde6'],
  ['Visibility corrected', '#3778f5'],
] as const;

const EDIT_REASONS: Array<{ value: FusionEditReason; label: string }> = [
  { value: 'motion', label: 'Motion' },
  { value: 'disocclusion', label: 'Disocclusion' },
  { value: 'focus', label: 'Focus selection' },
  { value: 'glare', label: 'Glare evidence' },
  { value: 'boundary', label: 'Depth boundary' },
  { value: 'other', label: 'Other measured correction' },
];

type EditRect = { x: number; y: number; width: number; height: number };

const rectanglesOverlap = (left: EditRect, right: EditRect) =>
  left.x < right.x + right.width && right.x < left.x + left.width
  && left.y < right.y + right.height && right.y < left.y + left.height;

const formatBytes = (bytes?: number | null) => {
  if (bytes == null) return 'Unavailable';
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KiB', 'MiB', 'GiB'];
  let value = bytes / 1024;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(value >= 10 ? 1 : 2)} ${units[index]}`;
};

const humanize = (value: string) => value.split('_').filter(Boolean)
  .map(word => word.charAt(0).toUpperCase() + word.slice(1)).join(' ');

const priceLabel = (bundle: LicenseBundleInfo | null) =>
  bundle?.price_usd ? `$${bundle.price_usd} ${bundle.billing ?? 'lifetime'}` : 'Contact sales';

const entitlementMessage = (error: unknown) => {
  if (!(error instanceof Error)) return null;
  try {
    const payload = JSON.parse(error.message) as { error?: string; message?: string };
    if (payload.error === 'feature_not_entitled') return payload.message ?? 'Advanced Capture add-on required.';
  } catch {
    // Ordinary server errors are shown without treating them as entitlement failures.
  }
  return error.message.includes('feature_not_entitled') ? 'Advanced Capture add-on required.' : null;
};

export const FusionInspector = ({ projectId, open, onClose }: FusionInspectorProps) => {
  const [inventory, setInventory] = useState<FusionReportInventory | null>(null);
  const [selectedPath, setSelectedPath] = useState('');
  const [layer, setLayer] = useState<LayerKey>('overlay');
  const [imageUrl, setImageUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [imageLoading, setImageLoading] = useState(false);
  const [locked, setLocked] = useState(false);
  const [trialAvailable, setTrialAvailable] = useState(true);
  const [bundle, setBundle] = useState<LicenseBundleInfo | null>(null);
  const [unlockBusy, setUnlockBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [editMode, setEditMode] = useState(false);
  const [draftStart, setDraftStart] = useState<{ x: number; y: number } | null>(null);
  const [draftRect, setDraftRect] = useState<EditRect | null>(null);
  const [editOperations, setEditOperations] = useState<FusionEditOperation[]>([]);
  const [editFrame, setEditFrame] = useState(0);
  const [editReason, setEditReason] = useState<FusionEditReason>('motion');
  const [editNote, setEditNote] = useState('');
  const [savingEdit, setSavingEdit] = useState(false);
  const [editReceipt, setEditReceipt] = useState<FusionEditReceipt | null>(null);
  const imageRef = useRef<HTMLImageElement | null>(null);

  const report = useMemo(
    () => inventory?.reports.find(item => item.report_path === selectedPath) ?? inventory?.reports[0] ?? null,
    [inventory, selectedPath],
  );
  const artifact = report?.artifacts[layer] ?? null;

  const loadReports = useCallback(async () => {
    if (!projectId) return;
    setLoading(true);
    setError(null);
    try {
      const result = await listFusionReports(projectId);
      setInventory(result);
      setSelectedPath(current => result.reports.some(item => item.report_path === current)
        ? current : result.reports[0]?.report_path ?? '');
      setLocked(false);
    } catch (requestError) {
      const entitlement = entitlementMessage(requestError);
      if (entitlement) setLocked(true);
      setError(entitlement ?? (requestError instanceof Error ? requestError.message : 'Failed to load reports.'));
      setInventory(null);
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    if (!open || !projectId) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    setLayer('overlay');
    Promise.all([getLicenseStatus(), getLicenseBundles()])
      .then(([status, bundles]) => {
        if (cancelled) return;
        setBundle(bundles.find(item => item.key === 'advanced_capture') ?? null);
        setTrialAvailable(status.trial_available ?? false);
        const isLocked = !(status.license_valid && status.features?.advanced_capture_automation);
        setLocked(isLocked);
        if (isLocked) setLoading(false);
        else void loadReports();
      })
      .catch(() => { if (!cancelled) void loadReports(); });
    return () => { cancelled = true; };
  }, [open, projectId, loadReports]);

  useEffect(() => {
    if (!open || !projectId || !artifact?.present) {
      setImageUrl(null);
      return;
    }
    let cancelled = false;
    let objectUrl: string | null = null;
    setImageUrl(null);
    setImageLoading(true);
    fetchFusionArtifact(projectId, artifact.path)
      .then(blob => {
        if (cancelled) return;
        objectUrl = URL.createObjectURL(blob);
        setImageUrl(objectUrl);
      })
      .catch(fetchError => {
        if (!cancelled) toast.error(fetchError instanceof Error ? fetchError.message : 'Layer load failed.');
      })
      .finally(() => { if (!cancelled) setImageLoading(false); });
    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [open, projectId, artifact?.path, artifact?.present]);

  useEffect(() => {
    setEditMode(false);
    setDraftStart(null);
    setDraftRect(null);
    setEditOperations([]);
    setEditFrame(0);
    setEditReason('motion');
    setEditNote('');
    setEditReceipt(null);
  }, [selectedPath]);

  const startTrial = async () => {
    setUnlockBusy(true);
    setError(null);
    try {
      await createLicenseTrial({ duration_days: 14, bundles: ['advanced_capture'] });
      toast.success('Advanced Capture Automation trial activated.');
      await loadReports();
    } catch (trialError) {
      setError(trialError instanceof Error ? trialError.message : 'Trial activation failed.');
    } finally {
      setUnlockBusy(false);
    }
  };

  const buy = () => {
    const name = bundle?.name ?? 'Advanced Capture Automation';
    window.open(`mailto:sales@trueshot.ai?subject=${encodeURIComponent(`TrueShot purchase: ${name}`)}&body=${encodeURIComponent(`I want to buy the ${name} lifetime add-on.`)}`, '_blank');
  };

  const download = async (item: FusionArtifactRef) => {
    if (!projectId || !item.present) return;
    try {
      const blob = await fetchFusionArtifact(projectId, item.path);
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = item.path.split('/').pop() ?? 'fusion-map.png';
      anchor.click();
      URL.revokeObjectURL(url);
    } catch (downloadError) {
      toast.error(downloadError instanceof Error ? downloadError.message : 'Download failed.');
    }
  };

  const imagePoint = (event: ReactPointerEvent<HTMLDivElement>) => {
    const image = imageRef.current;
    if (!image || !report) return null;
    const bounds = image.getBoundingClientRect();
    if (bounds.width <= 0 || bounds.height <= 0) return null;
    return {
      x: Math.max(0, Math.min(report.width - 1, Math.floor((event.clientX - bounds.left) / bounds.width * report.width))),
      y: Math.max(0, Math.min(report.height - 1, Math.floor((event.clientY - bounds.top) / bounds.height * report.height))),
    };
  };

  const rectFromPoints = (start: { x: number; y: number }, end: { x: number; y: number }): EditRect => ({
    x: Math.min(start.x, end.x),
    y: Math.min(start.y, end.y),
    width: Math.abs(end.x - start.x) + 1,
    height: Math.abs(end.y - start.y) + 1,
  });

  const beginEditRect = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!editMode || !report?.editable_base) return;
    const point = imagePoint(event);
    if (!point) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    setDraftStart(point);
    setDraftRect({ ...point, width: 1, height: 1 });
  };

  const moveEditRect = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!draftStart) return;
    const point = imagePoint(event);
    if (point) setDraftRect(rectFromPoints(draftStart, point));
  };

  const finishEditRect = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!draftStart) return;
    const point = imagePoint(event);
    const next = point ? rectFromPoints(draftStart, point) : draftRect;
    setDraftStart(null);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (!next) return;
    if (editOperations.some(operation => rectanglesOverlap(operation.rect, next))) {
      toast.error('Revision regions cannot overlap; each output pixel must have one unambiguous source.');
      setDraftRect(null);
      return;
    }
    setDraftRect(next);
  };

  const addEditRegion = () => {
    if (!draftRect || !report?.frame_count) return;
    const frameCount = report.frame_count;
    setEditReceipt(null);
    const id = `${editReason}-${draftRect.x}-${draftRect.y}-${draftRect.width}-${draftRect.height}-f${editFrame}`;
    setEditOperations(current => [...current, {
      id,
      rect: draftRect,
      source_frame: Math.max(0, Math.min(frameCount - 1, editFrame)),
      reason: editReason,
      ...(editNote.trim() ? { note: editNote.trim() } : {}),
    }]);
    setDraftRect(null);
    setEditNote('');
  };

  const saveEditDocument = async () => {
    if (!projectId || !report || editOperations.length === 0) return;
    setSavingEdit(true);
    setError(null);
    try {
      const receipt = await createFusionEdit(projectId, report.report_path, editOperations);
      setEditReceipt(receipt);
      toast.success('Immutable measured-source revision saved.');
    } catch (saveError) {
      setError(saveError instanceof Error ? saveError.message : 'Revision save failed.');
    } finally {
      setSavingEdit(false);
    }
  };

  const copyRefusionArgument = async () => {
    if (!editReceipt?.cli_argument) return;
    try {
      await navigator.clipboard.writeText(editReceipt.cli_argument);
      toast.success('Refusion argument copied.');
    } catch {
      toast.error('Clipboard access was denied. The immutable revision path remains visible above.');
    }
  };

  const downloadEditDocument = () => {
    if (!editReceipt) return;
    const blob = new Blob([`${JSON.stringify(editReceipt.document, null, 2)}\n`], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = editReceipt.download_filename;
    anchor.click();
    URL.revokeObjectURL(url);
  };

  if (!open || !projectId) return null;
  const selectedLayer = LAYERS.find(item => item.key === layer) ?? LAYERS[0];
  const pixelCount = report ? report.width * report.height : 0;
  const flags = layer === 'frequency_flags'
    ? Object.entries(report?.frequency_flags ?? {}) : Object.entries(report?.flags ?? {});

  return (
    <div className="fixed inset-0 z-[95] flex items-center justify-center bg-[color:var(--ts-overlay-strong)]/90 p-3 backdrop-blur-xl sm:p-6" role="dialog" aria-modal="true">
      <div className="ts-panel-strong flex h-[94vh] w-full max-w-[1500px] flex-col overflow-hidden">
        <header className="flex items-center justify-between border-b border-[color:var(--ts-border)] px-5 py-4">
          <div className="flex min-w-0 items-center gap-3">
            <div className="rounded-xl border border-accent-cyan/30 bg-accent-cyan/10 p-2.5"><ScanSearch className="h-5 w-5 text-accent-cyan" /></div>
            <div className="min-w-0">
              <div className="text-[10px] font-bold uppercase tracking-[0.28em] text-accent-cyan">Measured Fusion Provenance</div>
              <h2 className="truncate text-xl font-semibold text-[color:var(--ts-text)]">Fusion Inspector · {projectId}</h2>
            </div>
          </div>
          <button className="ts-icon-button h-10 w-10" onClick={onClose} aria-label="Close Fusion Inspector"><X className="h-5 w-5" /></button>
        </header>

        {locked ? (
          <div className="flex-1 overflow-y-auto p-6 sm:p-10">
            <FeatureUnlockPanel
              title="Fusion Inspector"
              subtitle="Audit every HDR and focus-stack decision instead of trusting a black box."
              bundleName={bundle?.name ?? 'Advanced Capture Automation'}
              priceLabel={priceLabel(bundle)}
              capabilities={['Measured source provenance', 'Deghost and disocclusion maps', 'Physical aperture boundaries', 'Exact archival downloads', 'Calibration disclosure', 'Encrypted local-only reads']}
              trialAvailable={trialAvailable}
              onStartTrial={startTrial}
              onBuy={buy}
              busy={unlockBusy}
              errorMessage={error}
            />
          </div>
        ) : loading ? (
          <div className="flex flex-1 items-center justify-center gap-3 text-[color:var(--ts-muted)]"><Loader2 className="h-5 w-5 animate-spin text-accent-cyan" />Validating fusion provenance…</div>
        ) : !report ? (
          <div className="flex flex-1 flex-col items-center justify-center px-6 text-center">
            <Layers3 className="mb-4 h-12 w-12 text-[color:var(--ts-muted)]" />
            <h3 className="text-lg font-semibold text-[color:var(--ts-text)]">No fusion reports yet</h3>
            <p className="mt-2 max-w-xl text-sm text-[color:var(--ts-muted)]">Run native HDR or focus fusion into this project. Validated provenance reports appear automatically.</p>
            {error && <p className="mt-4 text-sm text-red-400">{error}</p>}
          </div>
        ) : (
          <div className="grid min-h-0 flex-1 grid-cols-1 xl:grid-cols-[250px_minmax(0,1fr)_330px]">
            <aside className="min-h-0 overflow-y-auto border-b border-[color:var(--ts-border)] p-4 xl:border-b-0 xl:border-r">
              <label className="mb-2 block text-[10px] font-bold uppercase tracking-[0.22em] text-[color:var(--ts-muted)]">Fusion result</label>
              <select className="ts-input w-full px-3 py-2.5 text-sm" value={report.report_path} onChange={event => setSelectedPath(event.target.value)}>
                {inventory?.reports.map(item => <option key={item.report_path} value={item.report_path}>{item.label}</option>)}
              </select>
              <div className="mt-2 text-xs text-[color:var(--ts-muted)]">{report.width.toLocaleString()} × {report.height.toLocaleString()} · {pixelCount.toLocaleString()} px</div>
              <div className="mt-6 space-y-2">
                <div className="text-[10px] font-bold uppercase tracking-[0.22em] text-[color:var(--ts-muted)]">Evidence layers</div>
                {LAYERS.map(item => (
                  <button key={item.key} onClick={() => setLayer(item.key)} disabled={!report.artifacts[item.key]?.present}
                    className={`flex w-full items-center justify-between rounded-xl border px-3 py-2.5 text-left text-sm ${layer === item.key ? 'border-accent-cyan/50 bg-accent-cyan/10 text-[color:var(--ts-text)]' : 'border-[color:var(--ts-border)] text-[color:var(--ts-muted)] hover:bg-[color:var(--ts-surface-muted)]'} disabled:opacity-35`}>
                    {item.label}<Layers3 className="h-3.5 w-3.5" />
                  </button>
                ))}
              </div>
              <div className="mt-6 space-y-2">
                <div className="text-[10px] font-bold uppercase tracking-[0.22em] text-[color:var(--ts-muted)]">Exact source maps</div>
                {(['source', 'detail_source'] as const).map(key => {
                  const item = report.artifacts[key];
                  return <button key={key} onClick={() => item && void download(item)} disabled={!item?.present}
                    className="flex w-full items-center justify-between rounded-lg border border-[color:var(--ts-border)] px-3 py-2 text-left text-xs text-[color:var(--ts-muted)] hover:text-[color:var(--ts-text)] disabled:opacity-35">
                    <span>{key === 'source' ? 'Low-frequency source' : 'Detail source'} <span className="opacity-60">{formatBytes(item?.bytes)}</span></span><Download className="h-3.5 w-3.5" />
                  </button>;
                })}
              </div>
              <div className="mt-6 border-t border-[color:var(--ts-border)] pt-5">
                <div className="flex items-center justify-between gap-2">
                  <div className="text-[10px] font-bold uppercase tracking-[0.22em] text-[color:var(--ts-muted)]">Measured revision</div>
                  <button
                    onClick={() => {
                      setEditMode(current => !current);
                      setLayer('overlay');
                      setDraftRect(null);
                    }}
                    disabled={!report.editable_base}
                    className={`rounded-lg border px-2.5 py-1.5 text-[10px] font-bold uppercase tracking-wider ${editMode ? 'border-amber-400/50 bg-amber-400/10 text-amber-300' : 'border-[color:var(--ts-border)] text-[color:var(--ts-muted)]'} disabled:opacity-35`}
                  >
                    <span className="flex items-center gap-1.5"><Crosshair className="h-3.5 w-3.5" />{editMode ? 'Drawing' : 'Author'}</span>
                  </button>
                </div>
                {!report.editable_base ? (
                  <p className="mt-2 text-xs leading-relaxed text-amber-400">This report predates source-bound revisions. Rerun native fusion with the current build.</p>
                ) : (
                  <p className="mt-2 text-xs leading-relaxed text-[color:var(--ts-muted)]">Draw non-overlapping regions, then bind each to one real frame. Refusion rejects clipped or disoccluded measurements.</p>
                )}
                {editMode && report.frame_count && (
                  <div className="mt-3 space-y-2">
                    <label className="block text-[10px] uppercase tracking-wider text-[color:var(--ts-muted)]">Measured frame</label>
                    <select className="ts-input w-full px-3 py-2 text-xs" value={editFrame} onChange={event => setEditFrame(Number(event.target.value))}>
                      {Array.from({ length: report.frame_count }, (_, index) => <option key={index} value={index}>Frame {index}</option>)}
                    </select>
                    <label className="block text-[10px] uppercase tracking-wider text-[color:var(--ts-muted)]">Reason</label>
                    <select className="ts-input w-full px-3 py-2 text-xs" value={editReason} onChange={event => setEditReason(event.target.value as FusionEditReason)}>
                      {EDIT_REASONS.map(reason => <option key={reason.value} value={reason.value}>{reason.label}</option>)}
                    </select>
                    <input className="ts-input w-full px-3 py-2 text-xs" value={editNote} maxLength={512} onChange={event => setEditNote(event.target.value)} placeholder="Optional audit note" />
                    <button onClick={addEditRegion} disabled={!draftRect} className="w-full rounded-lg bg-amber-400 px-3 py-2 text-xs font-black uppercase tracking-wider text-black disabled:opacity-35">
                      Add drawn region
                    </button>
                  </div>
                )}
              </div>
            </aside>

            <main className="flex min-h-[360px] min-w-0 flex-col bg-[color:var(--ts-preview-bg)]">
              <div className="flex items-start justify-between gap-4 border-b border-white/10 px-5 py-3">
                <div><h3 className="font-semibold text-white">{selectedLayer.label}</h3><p className="mt-0.5 text-xs text-white/55">{selectedLayer.description}</p></div>
                {artifact?.present && <button onClick={() => void download(artifact)} className="flex shrink-0 items-center gap-2 rounded-lg border border-white/15 px-3 py-2 text-xs text-white/70 hover:bg-white/10"><Download className="h-3.5 w-3.5" />Exact PNG</button>}
              </div>
              <div className="relative flex min-h-0 flex-1 items-center justify-center overflow-auto p-5">
                {imageLoading ? <Loader2 className="h-8 w-8 animate-spin text-accent-cyan" /> : imageUrl
                  ? <div
                    className={`relative inline-flex max-h-full max-w-full touch-none ${editMode ? 'cursor-crosshair ring-2 ring-amber-400/50' : ''}`}
                    onPointerDown={beginEditRect}
                    onPointerMove={moveEditRect}
                    onPointerUp={finishEditRect}
                    onPointerCancel={finishEditRect}
                  >
                    <img ref={imageRef} draggable={false} src={imageUrl} alt={`${selectedLayer.label} for ${report.label}`} className="max-h-full max-w-full select-none rounded-lg object-contain shadow-2xl" style={{ filter: selectedLayer.filter }} />
                    {editOperations.map(operation => <div key={operation.id} className="pointer-events-none absolute border border-cyan-300 bg-cyan-300/15" style={{
                      left: `${operation.rect.x / report.width * 100}%`,
                      top: `${operation.rect.y / report.height * 100}%`,
                      width: `${operation.rect.width / report.width * 100}%`,
                      height: `${operation.rect.height / report.height * 100}%`,
                    }} />)}
                    {draftRect && <div className="pointer-events-none absolute border-2 border-amber-300 bg-amber-300/20" style={{
                      left: `${draftRect.x / report.width * 100}%`,
                      top: `${draftRect.y / report.height * 100}%`,
                      width: `${draftRect.width / report.width * 100}%`,
                      height: `${draftRect.height / report.height * 100}%`,
                    }} />}
                  </div>
                  : <div className="text-sm text-white/50">Layer artifact unavailable.</div>}
              </div>
              <div className="flex flex-wrap gap-x-4 gap-y-2 border-t border-white/10 px-5 py-3">
                {layer === 'edit' ? <span className="text-[10px] text-white/65">255 · Exact operator-selected measured source</span>
                  : layer === 'overlay' ? OVERLAY_LEGEND.map(([name, color]) => <span key={name} className="flex items-center gap-1.5 text-[10px] text-white/65"><i className="h-2.5 w-2.5 rounded-sm" style={{ backgroundColor: color }} />{name}</span>)
                  : flags.map(([name, value]) => <span key={name} className="text-[10px] text-white/65">{humanize(name)} · {value.pixels.toLocaleString()} ({pixelCount ? ((value.pixels / pixelCount) * 100).toFixed(3) : '0.000'}%)</span>)}
                {layer === 'boundary' && Object.entries(report.boundary_trimap_legend).map(([name, value]) => <span key={name} className="text-[10px] text-white/65">{humanize(name)} · {value}</span>)}
              </div>
            </main>

            <aside className="min-h-0 overflow-y-auto border-t border-[color:var(--ts-border)] p-5 xl:border-l xl:border-t-0">
              <div className={`rounded-xl border p-4 ${report.integrity_complete ? 'border-emerald-500/25 bg-emerald-500/8' : 'border-amber-500/30 bg-amber-500/8'}`}>
                <div className="flex items-center gap-2">{report.integrity_complete ? <CheckCircle2 className="h-4 w-4 text-emerald-400" /> : <AlertTriangle className="h-4 w-4 text-amber-400" />}<b className="text-sm text-[color:var(--ts-text)]">{report.integrity_complete ? 'Artifact set complete' : 'Artifact set incomplete'}</b></div>
                <div className="mt-2 flex items-center gap-2 text-xs text-[color:var(--ts-muted)]"><ShieldCheck className="h-3.5 w-3.5" />Measured only · Generative off</div>
              </div>
              {report.warnings.map(warning => <div key={warning} className="mt-2 rounded-lg border border-amber-500/20 bg-amber-500/8 px-3 py-2 text-xs text-amber-300">{warning}</div>)}
              {(editMode || editOperations.length > 0 || editReceipt) && (
                <Section title="Measured revision" icon={<Crosshair className="h-3.5 w-3.5" />}>
                  <div className="space-y-2">
                    {editOperations.map(operation => (
                      <div key={operation.id} className="rounded-lg border border-cyan-400/20 bg-cyan-400/5 p-2.5">
                        <div className="flex items-start justify-between gap-2">
                          <div>
                            <div className="text-xs font-semibold text-[color:var(--ts-text)]">{humanize(operation.reason)} · Frame {operation.source_frame}</div>
                            <div className="mt-0.5 text-[10px] tabular-nums text-[color:var(--ts-muted)]">x{operation.rect.x} y{operation.rect.y} · {operation.rect.width}×{operation.rect.height}</div>
                          </div>
                          <button onClick={() => {
                            setEditReceipt(null);
                            setEditOperations(current => current.filter(item => item.id !== operation.id));
                          }} className="text-[color:var(--ts-muted)] hover:text-red-400" aria-label={`Remove ${operation.id}`}><Trash2 className="h-3.5 w-3.5" /></button>
                        </div>
                      </div>
                    ))}
                    {editOperations.length === 0 && <p className="text-xs text-[color:var(--ts-muted)]">No committed regions. Draw on the provenance image, then add the region.</p>}
                    <button onClick={() => void saveEditDocument()} disabled={savingEdit || editOperations.length === 0} className="flex w-full items-center justify-center gap-2 rounded-lg bg-accent-cyan px-3 py-2.5 text-xs font-black uppercase tracking-wider text-black disabled:opacity-35">
                      {savingEdit ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}Save revision document
                    </button>
                    {editReceipt && (
                      <div className="rounded-lg border border-emerald-400/25 bg-emerald-400/8 p-3">
                        <div className="flex items-center gap-2 text-xs font-semibold text-emerald-300"><CheckCircle2 className="h-4 w-4" />Immutable revision ready</div>
                        <div className="mt-2 break-all font-mono text-[10px] text-[color:var(--ts-muted)]">{editReceipt.path}</div>
                        <div className="mt-2 flex flex-wrap gap-3">
                          <button onClick={downloadEditDocument} className="flex items-center gap-1.5 text-[10px] font-bold uppercase tracking-wider text-accent-cyan"><Download className="h-3.5 w-3.5" />Download revision JSON</button>
                          {editReceipt.cli_argument && <button onClick={() => void copyRefusionArgument()} className="flex items-center gap-1.5 text-[10px] font-bold uppercase tracking-wider text-accent-cyan"><Copy className="h-3.5 w-3.5" />Copy exact refusion argument</button>}
                        </div>
                        {editReceipt.encrypted && <div className="mt-2 text-[10px] leading-relaxed text-amber-300">The project copy is encrypted. Download this in-memory JSON, then pass the downloaded file to <code>--fusion-edits</code>.</div>}
                        <div className="mt-2 text-[10px] text-[color:var(--ts-muted)]">{editReceipt.edited_pixels.toLocaleString()} pixels · {editReceipt.encrypted ? 'encrypted at rest' : 'local project storage'}</div>
                      </div>
                    )}
                    {error && <p className="text-xs text-red-400">{error}</p>}
                  </div>
                </Section>
              )}
              <Section title="Calibration">
                <div className="grid grid-cols-2 gap-2">
                  <Status label="RAW noise" pass={report.calibration.noise_model_calibrated} />
                  <Status label="Lens PSF" pass={report.calibration.lens_psf_calibrated} />
                  <Status label="Demosaic" pass={!report.demosaic.fallback} yes="Native" no="Fallback" />
                  <Status label="Generative" pass={!report.demosaic.generative_reconstruction} yes="Off" no="On" />
                </div>
                <p className="mt-2 truncate text-xs text-[color:var(--ts-muted)]">{report.demosaic.backend ?? 'Backend unreported'} · {report.demosaic.adapter ?? 'Adapter unreported'}</p>
              </Section>
              <Section title="Performance" icon={<Gauge className="h-3.5 w-3.5" />}>
                <div className="grid grid-cols-2 gap-2">
                  <Metric label="Decode" value={report.performance.decode_seconds} />
                  <Metric label="Fusion" value={report.performance.fusion_seconds} />
                  <Metric label="Demosaic" value={report.performance.demosaic_and_postprocess_seconds} />
                  <Metric label="Peak admitted" text={formatBytes(report.performance.admitted_peak_memory_bytes)} />
                </div>
              </Section>
              <Section title="Measured interventions">
                {Object.entries(report.metrics).map(([name, value]) => <div key={name} className="flex justify-between gap-4 py-0.5 text-xs"><span className="text-[color:var(--ts-muted)]">{humanize(name)}</span><b className="tabular-nums text-[color:var(--ts-text)]">{value.toLocaleString()}</b></div>)}
              </Section>
              <Section title="Policies">
                {Object.entries(report.policy).map(([name, value]) => <div key={name} className="mb-2"><div className="text-[10px] uppercase tracking-wider text-[color:var(--ts-muted)]">{name}</div><div className="text-xs leading-relaxed text-[color:var(--ts-text)]">{humanize(value)}</div></div>)}
              </Section>
              {(inventory?.rejected_reports ?? 0) > 0 && <p className="mt-4 text-[10px] text-amber-400">{inventory?.rejected_reports} malformed or non-archival report(s) excluded.</p>}
            </aside>
          </div>
        )}
      </div>
    </div>
  );
};

const Section = ({ title, icon, children }: { title: string; icon?: ReactNode; children: ReactNode }) => (
  <section className="mt-5"><h4 className="mb-2 flex items-center gap-2 text-[10px] font-bold uppercase tracking-[0.22em] text-[color:var(--ts-muted)]">{icon}{title}</h4>{children}</section>
);

const Status = ({ label, pass, yes = 'Calibrated', no = 'Fallback' }: { label: string; pass: boolean; yes?: string; no?: string }) => (
  <div className="rounded-lg border border-[color:var(--ts-border)] bg-[color:var(--ts-surface-muted)] p-2.5"><div className="text-[10px] text-[color:var(--ts-muted)]">{label}</div><div className={`mt-1 text-xs font-semibold ${pass ? 'text-emerald-400' : 'text-amber-400'}`}>{pass ? yes : no}</div></div>
);

const Metric = ({ label, value, text }: { label: string; value?: number | null; text?: string }) => (
  <div className="rounded-lg border border-[color:var(--ts-border)] bg-[color:var(--ts-surface-muted)] p-2.5"><div className="text-[10px] text-[color:var(--ts-muted)]">{label}</div><div className="mt-1 text-xs font-semibold tabular-nums text-[color:var(--ts-text)]">{text ?? (value == null ? '—' : `${value.toFixed(3)}s`)}</div></div>
);
