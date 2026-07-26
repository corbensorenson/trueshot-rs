import { useEffect, useMemo, useState } from 'react';
import toast from 'react-hot-toast';
import {
  createLicenseTrial,
  createShareLink,
  getLicenseBundles,
  getLicenseStatus,
  getShareAnalytics,
  listProjectAssets,
  setSharePublic,
  type ProjectAsset,
  type LicenseBundleInfo,
  type ShareAnalytics,
  type SharePublicResponse,
  type ShareLinkResponse
} from '../api/client';
import { FeatureUnlockPanel } from './FeatureUnlockPanel';

interface ShareAssetModalProps {
  projectId: string | null;
  open: boolean;
  onClose: () => void;
}

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

const formatBundlePrice = (bundle?: LicenseBundleInfo | null) => {
  if (!bundle) return 'Pricing unavailable';
  if (!bundle.price_usd) return 'Contact sales';
  const billing = bundle.billing ? ` ${bundle.billing}` : '';
  return `$${bundle.price_usd}${billing}`;
};

export const ShareAssetModal = ({ projectId, open, onClose }: ShareAssetModalProps) => {
  const [assets, setAssets] = useState<ProjectAsset[]>([]);
  const [scope, setScope] = useState<'output' | 'processed' | 'all'>('output');
  const [search, setSearch] = useState('');
  const [assetPath, setAssetPath] = useState('');
  const [expiresDays, setExpiresDays] = useState(7);
  const [maxUses, setMaxUses] = useState('');
  const [allowDownload, setAllowDownload] = useState(true);
  const [allowEmbed, setAllowEmbed] = useState(true);
  const [publishPublic, setPublishPublic] = useState(false);
  const [publicTitle, setPublicTitle] = useState('');
  const [publicDescription, setPublicDescription] = useState('');
  const [publicTags, setPublicTags] = useState('');
  const [publicShortCode, setPublicShortCode] = useState('');
  const [loading, setLoading] = useState(false);
  const [share, setShare] = useState<ShareLinkResponse | null>(null);
  const [analytics, setAnalytics] = useState<ShareAnalytics | null>(null);
  const [analyticsLoading, setAnalyticsLoading] = useState(false);
  const [publicMeta, setPublicMeta] = useState<SharePublicResponse | null>(null);
  const [collabLocked, setCollabLocked] = useState(false);
  const [bundleName, setBundleName] = useState('Team Collaboration');
  const [bundlePrice, setBundlePrice] = useState('Pricing unavailable');
  const [trialAvailable, setTrialAvailable] = useState(true);
  const [unlockBusy, setUnlockBusy] = useState(false);
  const [unlockError, setUnlockError] = useState<string | null>(null);

  const decodeEntitlementError = (err: unknown): { locked: boolean; message: string | null } => {
    if (!(err instanceof Error)) return { locked: false, message: null };
    const raw = err.message || '';
    if (!raw.length) return { locked: false, message: null };
    try {
      const payload = JSON.parse(raw) as { error?: string; capability?: string; message?: string };
      if (payload?.error === 'feature_not_entitled') {
        return {
          locked: true,
          message: payload.message || 'This feature requires the Team Collaboration add-on.',
        };
      }
    } catch {
      // ignore parse errors and fall through to string matching
    }
    if (raw.includes('feature_not_entitled') || raw.includes('Payment Required')) {
      return { locked: true, message: 'This feature requires the Team Collaboration add-on.' };
    }
    return { locked: false, message: null };
  };

  const refreshEntitlement = async () => {
    try {
      const [status, bundles] = await Promise.all([getLicenseStatus(), getLicenseBundles()]);
      setCollabLocked(!(status.license_valid && status.features?.team_collaboration));
      setTrialAvailable(status.trial_available ?? true);
      const matched = bundles.find((bundle) => bundle.key === 'team_collaboration');
      if (matched?.name) setBundleName(matched.name);
      setBundlePrice(formatBundlePrice(matched));
    } catch {
      // If status lookup fails (e.g. non-admin token), defer to server-side 402 handling.
      setCollabLocked(false);
    }
  };

  useEffect(() => {
    if (!open || !projectId) return;
    setShare(null);
    setPublicMeta(null);
    setPublishPublic(false);
    setPublicTitle('');
    setPublicDescription('');
    setPublicTags('');
    setPublicShortCode('');
    setAssetPath('');
    setSearch('');
    setUnlockError(null);
    void refreshEntitlement();
    setLoading(true);
    listProjectAssets(projectId, scope)
      .then((data) => setAssets(data))
      .catch((err) => {
        console.error(err);
        const entitlement = decodeEntitlementError(err);
        if (entitlement.locked) {
          setCollabLocked(true);
          setUnlockError(entitlement.message);
          return;
        }
        toast.error('Failed to load project assets');
      })
      .finally(() => setLoading(false));
  }, [open, projectId, scope]);

  useEffect(() => {
    if (!share?.token) {
      setAnalytics(null);
      return;
    }
    let active = true;
    setAnalyticsLoading(true);
    getShareAnalytics(share.token)
      .then((data) => {
        if (!active) return;
        setAnalytics(data);
      })
      .catch((err) => {
        console.error(err);
        if (!active) return;
        setAnalytics(null);
      })
      .finally(() => {
        if (!active) return;
        setAnalyticsLoading(false);
      });
    return () => {
      active = false;
    };
  }, [share?.token]);

  const filteredAssets = useMemo(() => {
    if (!search.trim()) return assets;
    const needle = search.toLowerCase();
    return assets.filter((asset) => asset.path.toLowerCase().includes(needle));
  }, [assets, search]);

  const handleCreate = async () => {
    if (!projectId) return;
    if (!assetPath.trim()) {
      toast.error('Select or enter an asset path');
      return;
    }
    const expiresInSeconds = Math.max(1, Math.round(expiresDays * 24 * 3600));
    const maxUsesValue = maxUses.trim().length ? Number(maxUses) : undefined;
    if (maxUsesValue !== undefined && (Number.isNaN(maxUsesValue) || maxUsesValue <= 0)) {
      toast.error('Max uses must be a positive number');
      return;
    }
    setLoading(true);
    try {
      const link = await createShareLink({
        project_id: projectId,
        asset_path: assetPath,
        expires_in_seconds: expiresInSeconds,
        max_uses: maxUsesValue,
        allow_download: allowDownload,
        allow_embed: allowEmbed,
      });
      setShare(link);
      if (publishPublic && link.token) {
        try {
          const meta = await setSharePublic(link.token, {
            public: true,
            title: publicTitle.trim().length ? publicTitle.trim() : undefined,
            description: publicDescription.trim().length ? publicDescription.trim() : undefined,
            tags: publicTags.trim().length ? publicTags.split(',').map((t) => t.trim()).filter(Boolean) : undefined,
            short_code: publicShortCode.trim().length ? publicShortCode.trim() : undefined,
          });
          setPublicMeta(meta);
        } catch (err) {
          console.error(err);
          setPublicMeta(null);
          toast.error('Share created, but publishing failed');
        }
      } else {
        setPublicMeta(null);
      }
      toast.success('Share link created');
    } catch (err) {
      console.error(err);
      const entitlement = decodeEntitlementError(err);
      if (entitlement.locked) {
        setCollabLocked(true);
        setUnlockError(entitlement.message);
      } else {
        toast.error('Failed to create share link');
      }
    } finally {
      setLoading(false);
    }
  };

  const handleStartTrial = async () => {
    setUnlockBusy(true);
    setUnlockError(null);
    try {
      await createLicenseTrial({ duration_days: 14, bundles: ['team_collaboration'] });
      await refreshEntitlement();
      toast.success('Team Collaboration trial activated.');
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Trial activation failed';
      setUnlockError(message);
      toast.error('Trial unavailable. Purchase required.');
    } finally {
      setUnlockBusy(false);
    }
  };

  const handleBuy = () => {
    const subject = encodeURIComponent(`TrueShot purchase: ${bundleName}`);
    const body = encodeURIComponent(`I want to buy the ${bundleName} lifetime add-on.`);
    window.open(`mailto:sales@trueshot.ai?subject=${subject}&body=${body}`, '_blank');
  };

  const copy = async (value: string, label: string) => {
    try {
      await navigator.clipboard.writeText(value);
      toast.success(`${label} copied`);
    } catch (err) {
      console.error(err);
      toast.error('Copy failed');
    }
  };

  const embedSnippet = share
    ? `<iframe src="${share.viewer_url}" width="960" height="540" style="border:0;" allow="fullscreen"></iframe>`
    : '';

  const lastAccessLabel = useMemo(() => {
    if (!analytics?.last_access) return '—';
    return new Date(analytics.last_access * 1000).toLocaleString();
  }, [analytics?.last_access]);

  if (!open || !projectId) return null;

  return (
    <div className="fixed inset-0 z-[80] bg-black/70 backdrop-blur-md flex items-center justify-center">
      <div className="w-[900px] max-w-[95vw] max-h-[90vh] overflow-hidden rounded-2xl ts-panel border border-white/10 shadow-2xl">
        <div className="p-5 border-b border-white/10 flex items-center justify-between">
          <div>
            <div className="text-xs uppercase tracking-[0.3em] text-[color:var(--ts-muted)]">Share Asset</div>
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
          {collabLocked && (
            <div className="col-span-2 p-5">
              <FeatureUnlockPanel
                title="Team Collaboration"
                subtitle="Create secure share links, publish to the public gallery, and track viewer analytics for each asset."
                bundleName={bundleName}
                priceLabel={bundlePrice}
                capabilities={[
                  'Signed share links with download/embed controls',
                  'Public gallery publishing and short links',
                  'Share analytics, usage tracking, and referrer insights',
                  'Review-ready collaboration workflows',
                ]}
                trialAvailable={trialAvailable}
                onStartTrial={handleStartTrial}
                onBuy={handleBuy}
                busy={unlockBusy}
                errorMessage={unlockError}
              />
            </div>
          )}
          {!collabLocked && (
            <>
          <div className="p-5 border-r border-white/10 space-y-4">
            <div className="flex items-center gap-3">
              <div className="text-xs text-[color:var(--ts-muted)] uppercase tracking-[0.2em]">Scope</div>
              <select
                value={scope}
                onChange={(e) => setScope(e.target.value as 'output' | 'processed' | 'all')}
                className="px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
              >
                <option value="output">Output</option>
                <option value="processed">Processed</option>
                <option value="all">All</option>
              </select>
            </div>

            <div>
              <div className="text-xs text-[color:var(--ts-muted)] uppercase tracking-[0.2em] mb-2">Asset</div>
              <input
                value={assetPath}
                onChange={(e) => setAssetPath(e.target.value)}
                placeholder="output/model.glb"
                className="w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
              />
              <input
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Filter assets..."
                className="w-full mt-2 px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-xs text-[color:var(--ts-text)]"
              />
              <div className="mt-2 max-h-[240px] overflow-auto rounded-lg border border-white/10">
                {loading && <div className="p-3 text-xs text-[color:var(--ts-muted)]">Loading assets…</div>}
                {!loading && filteredAssets.length === 0 && (
                  <div className="p-3 text-xs text-[color:var(--ts-muted)]">No assets found.</div>
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

            <div className="grid grid-cols-2 gap-3">
              <div>
                <div className="text-xs text-[color:var(--ts-muted)] uppercase tracking-[0.2em] mb-2">Expires (days)</div>
                <input
                  type="number"
                  min={1}
                  max={365}
                  value={expiresDays}
                  onChange={(e) => setExpiresDays(Number(e.target.value))}
                  className="w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
                />
              </div>
              <div>
                <div className="text-xs text-[color:var(--ts-muted)] uppercase tracking-[0.2em] mb-2">Max uses</div>
                <input
                  value={maxUses}
                  onChange={(e) => setMaxUses(e.target.value)}
                  placeholder="Unlimited"
                  className="w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
                />
              </div>
            </div>

            <div className="flex items-center gap-4 text-xs text-[color:var(--ts-muted)]">
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={allowDownload}
                  onChange={(e) => setAllowDownload(e.target.checked)}
                />
                Allow download
              </label>
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={allowEmbed}
                  onChange={(e) => setAllowEmbed(e.target.checked)}
                />
                Allow embed
              </label>
            </div>

            <div className="space-y-3 rounded-xl border border-white/10 p-4 bg-white/5">
              <div className="flex items-center justify-between">
                <div className="text-xs uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">Public gallery</div>
                <label className="flex items-center gap-2 text-xs text-[color:var(--ts-text)]">
                  <input
                    type="checkbox"
                    checked={publishPublic}
                    onChange={(e) => setPublishPublic(e.target.checked)}
                  />
                  Publish
                </label>
              </div>
              {publishPublic && (
                <div className="space-y-2">
                  <input
                    value={publicTitle}
                    onChange={(e) => setPublicTitle(e.target.value)}
                    placeholder="Public title"
                    className="w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-xs text-[color:var(--ts-text)]"
                  />
                  <textarea
                    value={publicDescription}
                    onChange={(e) => setPublicDescription(e.target.value)}
                    placeholder="Short description"
                    className="w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-xs text-[color:var(--ts-text)]"
                    rows={3}
                  />
                  <input
                    value={publicTags}
                    onChange={(e) => setPublicTags(e.target.value)}
                    placeholder="Tags (comma separated)"
                    className="w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-xs text-[color:var(--ts-text)]"
                  />
                  <input
                    value={publicShortCode}
                    onChange={(e) => setPublicShortCode(e.target.value)}
                    placeholder="Short code (optional)"
                    className="w-full px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-xs text-[color:var(--ts-text)]"
                  />
                </div>
              )}
            </div>

            <button
              onClick={handleCreate}
              disabled={loading}
              className="w-full py-3 rounded-xl text-xs font-semibold uppercase tracking-[0.2em] bg-accent-blue text-black hover:opacity-90 transition disabled:opacity-50"
            >
              {loading ? 'Creating…' : 'Create Share Link'}
            </button>
          </div>

          <div className="p-5 space-y-4">
            <div className="text-xs uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">Share Output</div>
            {!share && (
              <div className="text-sm text-[color:var(--ts-muted)]">Create a share link to view or embed the asset.</div>
            )}
            {share && (
              <div className="space-y-4">
                <div className="ts-panel p-3 rounded-xl border border-white/10">
                  <div className="text-xs text-[color:var(--ts-muted)] uppercase tracking-[0.2em] mb-2">Viewer URL</div>
                  <div className="flex items-center gap-2">
                    <input
                      readOnly
                      value={share.viewer_url}
                      className="flex-1 px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-xs text-[color:var(--ts-text)]"
                    />
                    <button
                      onClick={() => copy(share.viewer_url, 'Viewer URL')}
                      className="px-3 py-2 rounded-lg border border-white/10 text-xs text-[color:var(--ts-muted)] hover:text-[color:var(--ts-text)]"
                    >
                      Copy
                    </button>
                  </div>
                </div>

                {publicMeta && (
                  <div className="ts-panel p-3 rounded-xl border border-white/10 space-y-2">
                    <div className="text-xs text-[color:var(--ts-muted)] uppercase tracking-[0.2em] mb-2">Public Links</div>
                    {publicMeta.short_url && (
                      <div className="flex items-center gap-2">
                        <input
                          readOnly
                          value={publicMeta.short_url}
                          className="flex-1 px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-xs text-[color:var(--ts-text)]"
                        />
                        <button
                          onClick={() => copy(publicMeta.short_url || '', 'Short URL')}
                          className="px-3 py-2 rounded-lg border border-white/10 text-xs text-[color:var(--ts-muted)] hover:text-[color:var(--ts-text)]"
                        >
                          Copy
                        </button>
                      </div>
                    )}
                    <div className="flex items-center gap-2">
                      <input
                        readOnly
                        value={publicMeta.card_url}
                        className="flex-1 px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-xs text-[color:var(--ts-text)]"
                      />
                      <button
                        onClick={() => copy(publicMeta.card_url, 'Social card')}
                        className="px-3 py-2 rounded-lg border border-white/10 text-xs text-[color:var(--ts-muted)] hover:text-[color:var(--ts-text)]"
                      >
                        Copy
                      </button>
                    </div>
                  </div>
                )}

                <div className="ts-panel p-3 rounded-xl border border-white/10 space-y-2">
                  <div className="flex items-center justify-between">
                    <div className="text-xs text-[color:var(--ts-muted)] uppercase tracking-[0.2em]">Share Analytics</div>
                    <button
                      onClick={() => {
                        if (!share?.token) return;
                        setAnalyticsLoading(true);
                        getShareAnalytics(share.token)
                          .then((data) => setAnalytics(data))
                          .catch((err) => {
                            console.error(err);
                            setAnalytics(null);
                          })
                          .finally(() => setAnalyticsLoading(false));
                      }}
                      className="px-2 py-1 rounded-lg border border-white/10 text-[10px] uppercase tracking-[0.2em] text-[color:var(--ts-muted)] hover:text-[color:var(--ts-text)]"
                    >
                      Refresh
                    </button>
                  </div>
                  {analyticsLoading && (
                    <div className="text-xs text-[color:var(--ts-muted)]">Loading analytics…</div>
                  )}
                  {!analyticsLoading && !analytics && (
                    <div className="text-xs text-[color:var(--ts-muted)]">Analytics unavailable.</div>
                  )}
                  {analytics && (
                    <div className="grid grid-cols-2 gap-3 text-xs text-[color:var(--ts-muted)]">
                      <div className="flex flex-col gap-1">
                        <span className="uppercase tracking-[0.2em] text-[10px]">Views</span>
                        <span className="text-[color:var(--ts-text)] text-sm">{analytics.views}</span>
                      </div>
                      <div className="flex flex-col gap-1">
                        <span className="uppercase tracking-[0.2em] text-[10px]">Asset Requests</span>
                        <span className="text-[color:var(--ts-text)] text-sm">{analytics.asset_requests}</span>
                      </div>
                      <div className="flex flex-col gap-1">
                        <span className="uppercase tracking-[0.2em] text-[10px]">Downloads</span>
                        <span className="text-[color:var(--ts-text)] text-sm">{analytics.downloads}</span>
                      </div>
                      <div className="flex flex-col gap-1">
                        <span className="uppercase tracking-[0.2em] text-[10px]">Embeds</span>
                        <span className="text-[color:var(--ts-text)] text-sm">{analytics.embeds}</span>
                      </div>
                      <div className="col-span-2 flex items-center justify-between text-[10px] uppercase tracking-[0.2em]">
                        <span className="text-[color:var(--ts-muted)]">Last Access</span>
                        <span className="text-[color:var(--ts-text)]">{lastAccessLabel}</span>
                      </div>
                      {analytics.top_referrers.length > 0 && (
                        <div className="col-span-2 text-[10px] uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">
                          Top referrers:
                          <div className="mt-2 space-y-1 text-xs text-[color:var(--ts-text)]">
                            {analytics.top_referrers.map((ref) => (
                              <div key={ref.referrer} className="flex items-center justify-between gap-3">
                                <span className="truncate">{ref.referrer}</span>
                                <span className="text-[color:var(--ts-muted)]">{ref.count}</span>
                              </div>
                            ))}
                          </div>
                        </div>
                      )}
                    </div>
                  )}
                </div>

                {share.allow_download && (
                  <div className="ts-panel p-3 rounded-xl border border-white/10">
                    <div className="text-xs text-[color:var(--ts-muted)] uppercase tracking-[0.2em] mb-2">Download URL</div>
                    <div className="flex items-center gap-2">
                      <input
                        readOnly
                        value={share.download_url}
                        className="flex-1 px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-xs text-[color:var(--ts-text)]"
                      />
                      <button
                        onClick={() => copy(share.download_url, 'Download URL')}
                        className="px-3 py-2 rounded-lg border border-white/10 text-xs text-[color:var(--ts-muted)] hover:text-[color:var(--ts-text)]"
                      >
                        Copy
                      </button>
                    </div>
                  </div>
                )}

                {share.allow_embed && (
                  <div className="ts-panel p-3 rounded-xl border border-white/10">
                    <div className="text-xs text-[color:var(--ts-muted)] uppercase tracking-[0.2em] mb-2">Embed Snippet</div>
                    <textarea
                      readOnly
                      value={embedSnippet}
                      className="w-full min-h-[120px] px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-xs text-[color:var(--ts-text)]"
                    />
                    <button
                      onClick={() => copy(embedSnippet, 'Embed snippet')}
                      className="mt-2 px-3 py-2 rounded-lg border border-white/10 text-xs text-[color:var(--ts-muted)] hover:text-[color:var(--ts-text)]"
                    >
                      Copy Embed
                    </button>
                  </div>
                )}
              </div>
            )}
          </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
};
