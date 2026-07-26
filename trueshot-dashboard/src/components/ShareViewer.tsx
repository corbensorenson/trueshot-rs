import { useEffect, useMemo, useState } from 'react';
import { API_Base, getShareAnnotations, type AnnotationPoint } from '../api/client';
import { UnifiedViewer } from './UnifiedViewer';

interface ShareMetadata {
  asset_url: string;
  download_url: string;
  viewer_url: string;
  lods?: {
    level: number;
    asset_url: string;
    bytes: number;
  }[];
  expires_at: number;
  max_uses?: number | null;
  remaining_uses?: number | null;
  allow_download: boolean;
  allow_embed: boolean;
  project_id: string;
  asset_path: string;
}

interface ShareViewerProps {
  token: string;
}

export const ShareViewer = ({ token }: ShareViewerProps) => {
  const [metadata, setMetadata] = useState<ShareMetadata | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [activeUrl, setActiveUrl] = useState<string | null>(null);
  const [activeLod, setActiveLod] = useState<number | null>(null);
  const [annotations, setAnnotations] = useState<AnnotationPoint[]>([]);

  useEffect(() => {
    let active = true;
    setError(null);
    fetch(`${API_Base}/share/${token}`)
      .then(async (res) => {
        if (!res.ok) {
          throw new Error(await res.text());
        }
        return res.json();
      })
      .then((data) => {
        if (!active) return;
        setMetadata(data);
      })
      .catch((err) => {
        if (!active) return;
        setError(err.message || 'Failed to load share link');
      });
    return () => {
      active = false;
    };
  }, [token]);

  useEffect(() => {
    if (!metadata?.asset_url) {
      setActiveUrl(null);
      setActiveLod(null);
      return;
    }
    const lods = (metadata.lods || []).slice().sort((a, b) => a.level - b.level);
    if (lods.length === 0) {
      setActiveUrl(metadata.asset_url);
      setActiveLod(null);
      return;
    }
    let cancelled = false;
    setActiveUrl(lods[0].asset_url);
    setActiveLod(lods[0].level);
    const timeouts: number[] = [];
    for (let i = 1; i < lods.length; i += 1) {
      const delay = 1200 * i;
      const handle = window.setTimeout(() => {
        if (cancelled) return;
        setActiveUrl(lods[i].asset_url);
        setActiveLod(lods[i].level);
      }, delay);
      timeouts.push(handle);
    }
    return () => {
      cancelled = true;
      timeouts.forEach((handle) => window.clearTimeout(handle));
    };
  }, [metadata]);

  useEffect(() => {
    if (!token) return;
    getShareAnnotations(token)
      .then((layer) => setAnnotations(layer.annotations || []))
      .catch((err) => {
        console.error(err);
        setAnnotations([]);
      });
  }, [token]);

  const expiresLabel = useMemo(() => {
    if (!metadata?.expires_at) return 'Unknown';
    const date = new Date(metadata.expires_at * 1000);
    return date.toLocaleString();
  }, [metadata?.expires_at]);

  return (
    <div className="min-h-screen w-full px-6 py-8 app-shell text-[color:var(--ts-text)]">
      <div className="max-w-6xl mx-auto space-y-6">
        <div className="ts-panel p-6 flex flex-col gap-2">
          <div className="text-xs uppercase tracking-[0.3em] text-[color:var(--ts-muted)]">
            TrueShot Share
          </div>
          <div className="text-2xl font-semibold text-[color:var(--ts-text)]">
            {metadata?.asset_path || 'Shared Asset'}
          </div>
          <div className="text-sm text-[color:var(--ts-muted)]">
            Project: {metadata?.project_id || 'Unknown'} • Expires: {expiresLabel}
          </div>
          {metadata?.max_uses !== undefined && metadata?.max_uses !== null && (
            <div className="text-xs text-[color:var(--ts-muted)]">
              Remaining views: {metadata.remaining_uses ?? 0} / {metadata.max_uses}
            </div>
          )}
          {metadata?.allow_download && metadata?.download_url && (
            <a
              className="ts-chip w-fit text-xs mt-2"
              href={metadata.download_url}
              rel="noreferrer"
            >
              Download Asset
            </a>
          )}
        </div>

        <div className="ts-panel-strong p-4 min-h-[500px]">
          {error && (
            <div className="text-sm text-red-400">{error}</div>
          )}
          {!error && !metadata && (
            <div className="text-sm text-[color:var(--ts-muted)]">Loading share…</div>
          )}
          {activeUrl && (
            <UnifiedViewer url={activeUrl} annotations={annotations} annotationsReadOnly />
          )}
        </div>

        {metadata?.lods && metadata.lods.length > 0 && (
          <div className="flex items-center justify-between text-[11px] uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">
            <span>Streaming LOD</span>
            <span className="text-[color:var(--ts-text)]">
              {activeLod !== null ? `LOD ${activeLod}` : 'Base'}
            </span>
          </div>
        )}
      </div>
    </div>
  );
};
