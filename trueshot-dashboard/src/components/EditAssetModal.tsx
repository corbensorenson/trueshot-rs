import { useEffect, useMemo, useState } from 'react';
import toast from 'react-hot-toast';
import {
  applyMeshEdits,
  applySplatEdits,
  getEditHistory,
  listProjectAssets,
  type EditHistoryEntry,
  type MeshEditOp,
  type ProjectAsset,
  type SplatEditOp,
} from '../api/client';

interface EditAssetModalProps {
  projectId: string | null;
  open: boolean;
  onClose: () => void;
}

type AssetType = 'mesh' | 'splat';

const formatBytes = (bytes: number) => {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${value.toFixed(1)} ${units[unitIndex]}`;
};

const isMeshAsset = (path: string) => {
  const lower = path.toLowerCase();
  return lower.endsWith('.obj') || lower.endsWith('.ply');
};

const isSplatAsset = (path: string) => path.toLowerCase().endsWith('.splat');

const normalizeOutputName = (type: AssetType, value: string) => {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  if (type === 'mesh') {
    return trimmed.toLowerCase().endsWith('.ply') ? trimmed : `${trimmed}.ply`;
  }
  return trimmed.toLowerCase().endsWith('.splat') ? trimmed : `${trimmed}.splat`;
};

const parseVec3 = (value: string, fallback: [number, number, number]) => {
  const parts = value.split(',').map((part) => parseFloat(part.trim()));
  if (parts.length !== 3 || parts.some((part) => Number.isNaN(part))) {
    return fallback;
  }
  const parsed: [number, number, number] = [parts[0], parts[1], parts[2]];
  return parsed;
};

export const EditAssetModal = ({ projectId, open, onClose }: EditAssetModalProps) => {
  const [assets, setAssets] = useState<ProjectAsset[]>([]);
  const [assetType, setAssetType] = useState<AssetType>('mesh');
  const [assetPath, setAssetPath] = useState('');
  const [search, setSearch] = useState('');
  const [outputName, setOutputName] = useState('');
  const [loading, setLoading] = useState(false);
  const [history, setHistory] = useState<EditHistoryEntry[]>([]);
  const [historyLoading, setHistoryLoading] = useState(false);

  const [smoothEnabled, setSmoothEnabled] = useState(true);
  const [smoothIterations, setSmoothIterations] = useState(2);
  const [smoothLambda, setSmoothLambda] = useState(0.35);
  const [decimateEnabled, setDecimateEnabled] = useState(false);
  const [targetTriangles, setTargetTriangles] = useState(50000);
  const [preserveBoundaries, setPreserveBoundaries] = useState(true);
  const [preserveUvSeams, setPreserveUvSeams] = useState(true);
  const [uvSeamThreshold, setUvSeamThreshold] = useState(0.6);
  const [fillHolesEnabled, setFillHolesEnabled] = useState(false);
  const [maxHoleVertices, setMaxHoleVertices] = useState(64);
  const [recomputeNormalsEnabled, setRecomputeNormalsEnabled] = useState(true);

  const [pruneOpacityEnabled, setPruneOpacityEnabled] = useState(true);
  const [minAlpha, setMinAlpha] = useState(16);
  const [boundsEnabled, setBoundsEnabled] = useState(false);
  const [boundsMin, setBoundsMin] = useState<[number, number, number]>([-1, -1, -1]);
  const [boundsMax, setBoundsMax] = useState<[number, number, number]>([1, 1, 1]);
  const [sphereEnabled, setSphereEnabled] = useState(false);
  const [sphereCenter, setSphereCenter] = useState<[number, number, number]>([0, 0, 0]);
  const [sphereRadius, setSphereRadius] = useState(1);
  const [densityEnabled, setDensityEnabled] = useState(false);
  const [densityTarget, setDensityTarget] = useState(100000);
  const [writeSpz, setWriteSpz] = useState(true);

  useEffect(() => {
    if (!open || !projectId) return;
    setLoading(true);
    listProjectAssets(projectId, 'output')
      .then((data) => setAssets(data))
      .catch((err) => {
        console.error(err);
        toast.error('Failed to load project assets');
      })
      .finally(() => setLoading(false));
  }, [open, projectId]);

  useEffect(() => {
    if (!open || !projectId) return;
    setHistoryLoading(true);
    getEditHistory(projectId)
      .then((data) => setHistory(data))
      .catch((err) => {
        console.error(err);
        setHistory([]);
      })
      .finally(() => setHistoryLoading(false));
  }, [open, projectId]);

  useEffect(() => {
    setAssetPath('');
  }, [assetType]);

  const filteredAssets = useMemo(() => {
    const scopeAssets = assets.filter((asset) => (assetType === 'mesh' ? isMeshAsset(asset.path) : isSplatAsset(asset.path)));
    if (!search.trim()) return scopeAssets;
    const needle = search.toLowerCase();
    return scopeAssets.filter((asset) => asset.path.toLowerCase().includes(needle));
  }, [assets, assetType, search]);

  const refreshHistory = async () => {
    if (!projectId) return;
    setHistoryLoading(true);
    try {
      const data = await getEditHistory(projectId);
      setHistory(data);
    } catch (err) {
      console.error(err);
      toast.error('Failed to load edit history');
    } finally {
      setHistoryLoading(false);
    }
  };

  const handleApply = async () => {
    if (!projectId) return;
    if (!assetPath.trim()) {
      toast.error('Select an asset to edit');
      return;
    }

    if (assetType === 'mesh') {
      const ops: MeshEditOp[] = [];
      if (smoothEnabled) {
        ops.push({ op: 'smooth', iterations: Math.max(1, smoothIterations), lambda: smoothLambda });
      }
      if (decimateEnabled) {
        ops.push({
          op: 'decimate',
          target_triangles: Math.max(100, targetTriangles),
          preserve_boundaries: preserveBoundaries,
          preserve_uv_seams: preserveUvSeams,
          uv_seam_threshold: uvSeamThreshold,
        });
      }
      if (fillHolesEnabled) {
        ops.push({ op: 'fill_holes', max_hole_vertices: Math.max(4, maxHoleVertices) });
      }
      if (recomputeNormalsEnabled) {
        ops.push({ op: 'recompute_normals' });
      }
      if (ops.length === 0) {
        toast.error('Select at least one mesh operation');
        return;
      }
      setLoading(true);
      try {
        const response = await applyMeshEdits(projectId, {
          input_path: assetPath,
          output_name: normalizeOutputName('mesh', outputName),
          output_format: 'ply',
          ops,
        });
        toast.success(`Mesh saved to ${response.output_path}`);
        await refreshHistory();
      } catch (err) {
        console.error(err);
        toast.error('Mesh edit failed');
      } finally {
        setLoading(false);
      }
    } else {
      const ops: SplatEditOp[] = [];
      if (pruneOpacityEnabled) {
        ops.push({ op: 'prune_opacity', min_alpha: Math.max(0, Math.min(255, minAlpha)) });
      }
      if (boundsEnabled) {
        ops.push({ op: 'bounds', min: boundsMin, max: boundsMax });
      }
      if (sphereEnabled) {
        ops.push({ op: 'sphere', center: sphereCenter, radius: Math.max(0.001, sphereRadius) });
      }
      if (densityEnabled) {
        ops.push({ op: 'density', target: Math.max(1000, densityTarget) });
      }
      if (ops.length === 0) {
        toast.error('Select at least one splat operation');
        return;
      }
      setLoading(true);
      try {
        const response = await applySplatEdits(projectId, {
          input_path: assetPath,
          output_name: normalizeOutputName('splat', outputName),
          write_spz: writeSpz,
          ops,
        });
        toast.success(`Splat saved to ${response.output_path}`);
        await refreshHistory();
      } catch (err) {
        console.error(err);
        toast.error('Splat edit failed');
      } finally {
        setLoading(false);
      }
    }
  };

  if (!open || !projectId) return null;

  return (
    <div className="fixed inset-0 z-[80] bg-black/70 backdrop-blur-md flex items-center justify-center">
      <div className="w-[980px] max-w-[96vw] max-h-[92vh] overflow-hidden rounded-2xl ts-panel border border-white/10 shadow-2xl">
        <div className="p-5 border-b border-white/10 flex items-center justify-between">
          <div>
            <div className="text-xs uppercase tracking-[0.3em] text-[color:var(--ts-muted)]">Edit Assets</div>
            <div className="text-lg font-semibold text-[color:var(--ts-text)]">{projectId}</div>
          </div>
          <button
            onClick={onClose}
            className="text-xs uppercase tracking-[0.2em] px-3 py-2 rounded-lg border border-white/10 text-[color:var(--ts-muted)] hover:text-[color:var(--ts-text)] hover:bg-white/5"
          >
            Close
          </button>
        </div>

        <div className="grid grid-cols-2 gap-0">
          <div className="p-5 border-r border-white/10 space-y-4 overflow-y-auto max-h-[78vh]">
            <div className="flex items-center gap-3">
              <div className="text-xs text-[color:var(--ts-muted)] uppercase tracking-[0.2em]">Asset type</div>
              <div className="flex items-center gap-2">
                <button
                  onClick={() => setAssetType('mesh')}
                  className={`px-3 py-2 rounded-lg text-xs uppercase tracking-[0.2em] border ${
                    assetType === 'mesh' ? 'bg-white/15 text-[color:var(--ts-text)] border-white/20' : 'text-[color:var(--ts-muted)] border-white/10'
                  }`}
                >
                  Mesh
                </button>
                <button
                  onClick={() => setAssetType('splat')}
                  className={`px-3 py-2 rounded-lg text-xs uppercase tracking-[0.2em] border ${
                    assetType === 'splat' ? 'bg-white/15 text-[color:var(--ts-text)] border-white/20' : 'text-[color:var(--ts-muted)] border-white/10'
                  }`}
                >
                  Splat
                </button>
              </div>
            </div>

            <div>
              <div className="text-xs text-[color:var(--ts-muted)] uppercase tracking-[0.2em] mb-2">Asset (output only)</div>
              <input
                value={assetPath}
                onChange={(e) => setAssetPath(e.target.value)}
                placeholder={assetType === 'mesh' ? 'output/model.obj' : 'output/model.splat'}
                className="w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
              />
              <input
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Filter assets..."
                className="w-full mt-2 px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-xs text-[color:var(--ts-text)]"
              />
              <div className="mt-2 max-h-[220px] overflow-auto rounded-lg border border-white/10">
                {loading && <div className="p-3 text-xs text-[color:var(--ts-muted)]">Loading assets…</div>}
                {!loading && filteredAssets.length === 0 && (
                  <div className="p-3 text-xs text-[color:var(--ts-muted)]">No compatible assets found.</div>
                )}
                {!loading && filteredAssets.map((asset) => (
                  <button
                    key={asset.path}
                    onClick={() => setAssetPath(asset.path)}
                    className={`w-full text-left px-3 py-2 text-xs border-b border-white/5 hover:bg-white/5 ${
                      asset.path === assetPath ? 'bg-white/10 text-[color:var(--ts-text)]' : 'text-[color:var(--ts-muted)]'
                    }`}
                  >
                    <div className="flex items-center justify-between gap-4">
                      <span className="truncate">{asset.path}</span>
                      <span className="text-[10px] uppercase">{formatBytes(asset.bytes)}</span>
                    </div>
                  </button>
                ))}
              </div>
            </div>

            <div>
              <div className="text-xs text-[color:var(--ts-muted)] uppercase tracking-[0.2em] mb-2">Output name (optional)</div>
              <input
                value={outputName}
                onChange={(e) => setOutputName(e.target.value)}
                placeholder={assetType === 'mesh' ? 'mesh_cleaned.ply' : 'splat_cleaned.splat'}
                className="w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
              />
              <div className="text-[11px] text-[color:var(--ts-muted)] mt-1">
                Output will be saved in `output/edits/...` and appended with the correct extension if missing.
              </div>
            </div>

            {assetType === 'mesh' ? (
              <div className="space-y-3">
                <div className="text-xs uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">Mesh operations</div>
                <label className="flex items-center gap-2 text-xs text-[color:var(--ts-text)]">
                  <input type="checkbox" checked={smoothEnabled} onChange={(e) => setSmoothEnabled(e.target.checked)} />
                  Smooth
                </label>
                {smoothEnabled && (
                  <div className="grid grid-cols-2 gap-3">
                    <label className="text-xs text-[color:var(--ts-muted)]">
                      Iterations
                      <input
                        type="number"
                        min={1}
                        max={20}
                        value={smoothIterations}
                        onChange={(e) => setSmoothIterations(Number(e.target.value))}
                        className="mt-1 w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
                      />
                    </label>
                    <label className="text-xs text-[color:var(--ts-muted)]">
                      Lambda
                      <input
                        type="number"
                        min={0}
                        max={1}
                        step={0.05}
                        value={smoothLambda}
                        onChange={(e) => setSmoothLambda(Number(e.target.value))}
                        className="mt-1 w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
                      />
                    </label>
                  </div>
                )}

                <label className="flex items-center gap-2 text-xs text-[color:var(--ts-text)]">
                  <input type="checkbox" checked={decimateEnabled} onChange={(e) => setDecimateEnabled(e.target.checked)} />
                  Decimate
                </label>
                {decimateEnabled && (
                  <div className="space-y-2">
                    <label className="text-xs text-[color:var(--ts-muted)]">
                      Target triangles
                      <input
                        type="number"
                        min={100}
                        step={100}
                        value={targetTriangles}
                        onChange={(e) => setTargetTriangles(Number(e.target.value))}
                        className="mt-1 w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
                      />
                    </label>
                    <label className="text-xs text-[color:var(--ts-muted)]">
                      UV seam threshold
                      <input
                        type="number"
                        min={0}
                        max={2}
                        step={0.05}
                        value={uvSeamThreshold}
                        onChange={(e) => setUvSeamThreshold(Number(e.target.value))}
                        className="mt-1 w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
                      />
                    </label>
                    <div className="flex flex-wrap gap-3 text-xs text-[color:var(--ts-muted)]">
                      <label className="flex items-center gap-2">
                        <input type="checkbox" checked={preserveBoundaries} onChange={(e) => setPreserveBoundaries(e.target.checked)} />
                        Preserve boundaries
                      </label>
                      <label className="flex items-center gap-2">
                        <input type="checkbox" checked={preserveUvSeams} onChange={(e) => setPreserveUvSeams(e.target.checked)} />
                        Preserve UV seams
                      </label>
                    </div>
                  </div>
                )}

                <label className="flex items-center gap-2 text-xs text-[color:var(--ts-text)]">
                  <input type="checkbox" checked={fillHolesEnabled} onChange={(e) => setFillHolesEnabled(e.target.checked)} />
                  Fill holes
                </label>
                {fillHolesEnabled && (
                  <label className="text-xs text-[color:var(--ts-muted)]">
                    Max hole vertices
                    <input
                      type="number"
                      min={4}
                      max={1024}
                      step={4}
                      value={maxHoleVertices}
                      onChange={(e) => setMaxHoleVertices(Number(e.target.value))}
                      className="mt-1 w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
                    />
                  </label>
                )}

                <label className="flex items-center gap-2 text-xs text-[color:var(--ts-text)]">
                  <input
                    type="checkbox"
                    checked={recomputeNormalsEnabled}
                    onChange={(e) => setRecomputeNormalsEnabled(e.target.checked)}
                  />
                  Recompute normals
                </label>
              </div>
            ) : (
              <div className="space-y-3">
                <div className="text-xs uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">Splat operations</div>
                <label className="flex items-center gap-2 text-xs text-[color:var(--ts-text)]">
                  <input type="checkbox" checked={pruneOpacityEnabled} onChange={(e) => setPruneOpacityEnabled(e.target.checked)} />
                  Prune by opacity
                </label>
                {pruneOpacityEnabled && (
                  <label className="text-xs text-[color:var(--ts-muted)]">
                    Min alpha (0-255)
                    <input
                      type="number"
                      min={0}
                      max={255}
                      value={minAlpha}
                      onChange={(e) => setMinAlpha(Number(e.target.value))}
                      className="mt-1 w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
                    />
                  </label>
                )}

                <label className="flex items-center gap-2 text-xs text-[color:var(--ts-text)]">
                  <input type="checkbox" checked={boundsEnabled} onChange={(e) => setBoundsEnabled(e.target.checked)} />
                  Crop by bounds
                </label>
                {boundsEnabled && (
                  <div className="grid grid-cols-2 gap-3">
                    <label className="text-xs text-[color:var(--ts-muted)]">
                      Min (x,y,z)
                      <input
                        value={boundsMin.join(',')}
                        onChange={(e) => setBoundsMin(parseVec3(e.target.value, boundsMin))}
                        className="mt-1 w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
                      />
                    </label>
                    <label className="text-xs text-[color:var(--ts-muted)]">
                      Max (x,y,z)
                      <input
                        value={boundsMax.join(',')}
                        onChange={(e) => setBoundsMax(parseVec3(e.target.value, boundsMax))}
                        className="mt-1 w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
                      />
                    </label>
                  </div>
                )}

                <label className="flex items-center gap-2 text-xs text-[color:var(--ts-text)]">
                  <input type="checkbox" checked={sphereEnabled} onChange={(e) => setSphereEnabled(e.target.checked)} />
                  Crop by sphere
                </label>
                {sphereEnabled && (
                  <div className="grid grid-cols-2 gap-3">
                    <label className="text-xs text-[color:var(--ts-muted)]">
                      Center (x,y,z)
                      <input
                        value={sphereCenter.join(',')}
                        onChange={(e) => setSphereCenter(parseVec3(e.target.value, sphereCenter))}
                        className="mt-1 w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
                      />
                    </label>
                    <label className="text-xs text-[color:var(--ts-muted)]">
                      Radius
                      <input
                        type="number"
                        min={0.001}
                        step={0.05}
                        value={sphereRadius}
                        onChange={(e) => setSphereRadius(Number(e.target.value))}
                        className="mt-1 w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
                      />
                    </label>
                  </div>
                )}

                <label className="flex items-center gap-2 text-xs text-[color:var(--ts-text)]">
                  <input type="checkbox" checked={densityEnabled} onChange={(e) => setDensityEnabled(e.target.checked)} />
                  Density cap
                </label>
                {densityEnabled && (
                  <label className="text-xs text-[color:var(--ts-muted)]">
                    Target count
                    <input
                      type="number"
                      min={1000}
                      step={1000}
                      value={densityTarget}
                      onChange={(e) => setDensityTarget(Number(e.target.value))}
                      className="mt-1 w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
                    />
                  </label>
                )}

                <label className="flex items-center gap-2 text-xs text-[color:var(--ts-text)]">
                  <input type="checkbox" checked={writeSpz} onChange={(e) => setWriteSpz(e.target.checked)} />
                  Write SPZ alongside output
                </label>
              </div>
            )}

            <button
              onClick={handleApply}
              disabled={loading}
              className="w-full mt-3 px-4 py-3 rounded-xl text-xs font-bold uppercase tracking-[0.2em] border border-white/10 bg-accent-blue/20 text-[color:var(--ts-text)] hover:bg-accent-blue/40 disabled:opacity-60"
            >
              {loading ? 'Applying…' : 'Apply edits'}
            </button>
          </div>

          <div className="p-5 space-y-4 overflow-y-auto max-h-[78vh]">
            <div className="flex items-center justify-between">
              <div className="text-xs uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">Edit history</div>
              <button
                onClick={refreshHistory}
                className="text-xs uppercase tracking-[0.2em] px-3 py-2 rounded-lg border border-white/10 text-[color:var(--ts-muted)] hover:text-[color:var(--ts-text)] hover:bg-white/5"
              >
                Refresh
              </button>
            </div>
            {historyLoading && <div className="text-xs text-[color:var(--ts-muted)]">Loading history…</div>}
            {!historyLoading && history.length === 0 && (
              <div className="text-xs text-[color:var(--ts-muted)]">No edits recorded yet.</div>
            )}
            {!historyLoading && history.map((entry) => (
              <div key={entry.id} className="border border-white/10 rounded-xl p-4 bg-white/5 space-y-2">
                <div className="flex items-center justify-between">
                  <div className="text-xs uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">{entry.asset_type}</div>
                  <div className="text-[11px] text-[color:var(--ts-muted)]">{new Date(entry.created_at).toLocaleString()}</div>
                </div>
                <div className="text-xs text-[color:var(--ts-text)] truncate">{entry.output_path}</div>
                <div className="text-[11px] text-[color:var(--ts-muted)]">
                  {JSON.stringify(entry.operations)}
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
};
