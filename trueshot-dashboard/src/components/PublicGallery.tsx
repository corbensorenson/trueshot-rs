import { useEffect, useMemo, useState } from 'react';
import { listPublicShares, type PublicShareSummary } from '../api/client';

export const PublicGallery = () => {
  const [shares, setShares] = useState<PublicShareSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState('');
  const [sort, setSort] = useState<'recent' | 'popular'>('recent');

  useEffect(() => {
    setLoading(true);
    listPublicShares({ limit: 48, sort })
      .then((data) => {
        setShares(data);
        setError(null);
      })
      .catch((err) => {
        console.error(err);
        setError('Failed to load gallery');
      })
      .finally(() => setLoading(false));
  }, [sort]);

  const filtered = useMemo(() => {
    if (!search.trim()) return shares;
    const needle = search.toLowerCase();
    return shares.filter((share) => {
      const title = share.title?.toLowerCase() || '';
      const desc = share.description?.toLowerCase() || '';
      const tags = share.tags.join(' ').toLowerCase();
      return title.includes(needle) || desc.includes(needle) || tags.includes(needle);
    });
  }, [shares, search]);

  return (
    <div className="min-h-screen w-full px-6 py-10 app-shell text-[color:var(--ts-text)]">
      <div className="max-w-6xl mx-auto space-y-6">
        <div className="flex flex-col gap-3">
          <div className="text-xs uppercase tracking-[0.3em] text-[color:var(--ts-muted)]">TrueShot Public Gallery</div>
          <div className="flex flex-wrap items-center gap-3">
            <input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search by title, description, or tag"
              className="flex-1 min-w-[240px] px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
            />
            <select
              value={sort}
              onChange={(e) => setSort(e.target.value as 'recent' | 'popular')}
              className="px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-[color:var(--ts-text)]"
            >
              <option value="recent">Recent</option>
              <option value="popular">Popular</option>
            </select>
          </div>
        </div>

        {loading && <div className="text-sm text-[color:var(--ts-muted)]">Loading gallery…</div>}
        {error && <div className="text-sm text-red-400">{error}</div>}

        {!loading && !error && filtered.length === 0 && (
          <div className="text-sm text-[color:var(--ts-muted)]">No public shares yet.</div>
        )}

        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
          {filtered.map((share) => (
            <button
              key={share.token}
              onClick={() => window.location.assign(share.viewer_url)}
              className="group text-left rounded-2xl border border-white/10 bg-white/5 hover:bg-white/10 transition p-4 space-y-3"
            >
              <div className="flex items-center justify-between text-[10px] uppercase tracking-[0.2em] text-[color:var(--ts-muted)]">
                <span>{share.tags.slice(0, 1).join(' ') || 'Share'}</span>
                <span>{share.views} views</span>
              </div>
              <div className="text-lg font-semibold text-[color:var(--ts-text)] truncate">
                {share.title || share.asset_url.split('/').pop()}
              </div>
              <div className="text-xs text-[color:var(--ts-muted)] line-clamp-3">
                {share.description || 'Shared 3D asset'}
              </div>
              <div className="flex flex-wrap gap-2 text-[10px] text-[color:var(--ts-muted)] uppercase tracking-[0.2em]">
                {share.tags.slice(0, 3).map((tag) => (
                  <span key={tag} className="px-2 py-1 rounded-full border border-white/10">{tag}</span>
                ))}
              </div>
              <div className="text-[10px] text-[color:var(--ts-muted)] uppercase tracking-[0.2em]">
                {new Date(share.updated_at * 1000).toLocaleDateString()}
              </div>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
};
