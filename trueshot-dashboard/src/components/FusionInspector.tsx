import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { AlertTriangle, CheckCircle2, Download, Gauge, Layers3, Loader2, ScanSearch, ShieldCheck, X } from 'lucide-react';
import toast from 'react-hot-toast';
import {
  createLicenseTrial,
  fetchFusionArtifact,
  getLicenseBundles,
  getLicenseStatus,
  listFusionReports,
  type FusionArtifactRef,
  type FusionReportInventory,
  type LicenseBundleInfo,
} from '../api/client';
import { FeatureUnlockPanel } from './FeatureUnlockPanel';

interface FusionInspectorProps {
  projectId: string | null;
  open: boolean;
  onClose: () => void;
}

type LayerKey = 'overlay' | 'flags' | 'frequency_flags' | 'glare' | 'boundary' | 'sensor_correction';

const LAYERS: Array<{ key: LayerKey; label: string; description: string; filter?: string }> = [
  { key: 'overlay', label: 'Source + Deghost', description: 'Measured-source provenance with alignment, fallback, clipping, and disocclusion states.' },
  { key: 'flags', label: 'Fusion Flags', description: 'Exact censoring, rejection, visibility, alignment, and fallback bitfield.', filter: 'contrast(4) brightness(1.8)' },
  { key: 'frequency_flags', label: 'Frequency Split', description: 'Measured low/detail source separation and envelope-clamping bitfield.', filter: 'contrast(8) brightness(2.5)' },
  { key: 'glare', label: 'Glare Guard', description: 'Glare excluded from focus evidence without changing measured radiance.', filter: 'contrast(2)' },
  { key: 'boundary', label: 'Aperture Boundary', description: 'Physical interior, PSF-support, and depth-crossing trimap.', filter: 'contrast(40) brightness(3)' },
  { key: 'sensor_correction', label: 'Sensor Corrections', description: 'Same-CFA flat-field and persistent-defect provenance.', filter: 'contrast(20) brightness(2)' },
];

const OVERLAY_LEGEND = [
  ['Disoccluded', '#eb37d2'], ['Source fallback', '#f53741'], ['Censor conflict', '#ff327d'],
  ['Detail reference', '#ff5f2d'], ['Split sources', '#37dc91'], ['Frequency separated', '#3cbef5'],
  ['Outlier rejected', '#ff7d23'], ['Censored', '#facd2d'], ['Bracket aligned', '#14cde6'],
  ['Visibility corrected', '#3778f5'],
] as const;

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
            </aside>

            <main className="flex min-h-[360px] min-w-0 flex-col bg-[color:var(--ts-preview-bg)]">
              <div className="flex items-start justify-between gap-4 border-b border-white/10 px-5 py-3">
                <div><h3 className="font-semibold text-white">{selectedLayer.label}</h3><p className="mt-0.5 text-xs text-white/55">{selectedLayer.description}</p></div>
                {artifact?.present && <button onClick={() => void download(artifact)} className="flex shrink-0 items-center gap-2 rounded-lg border border-white/15 px-3 py-2 text-xs text-white/70 hover:bg-white/10"><Download className="h-3.5 w-3.5" />Exact PNG</button>}
              </div>
              <div className="relative flex min-h-0 flex-1 items-center justify-center overflow-auto p-5">
                {imageLoading ? <Loader2 className="h-8 w-8 animate-spin text-accent-cyan" /> : imageUrl
                  ? <img src={imageUrl} alt={`${selectedLayer.label} for ${report.label}`} className="max-h-full max-w-full rounded-lg object-contain shadow-2xl" style={{ filter: selectedLayer.filter }} />
                  : <div className="text-sm text-white/50">Layer artifact unavailable.</div>}
              </div>
              <div className="flex flex-wrap gap-x-4 gap-y-2 border-t border-white/10 px-5 py-3">
                {layer === 'overlay' ? OVERLAY_LEGEND.map(([name, color]) => <span key={name} className="flex items-center gap-1.5 text-[10px] text-white/65"><i className="h-2.5 w-2.5 rounded-sm" style={{ backgroundColor: color }} />{name}</span>)
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
