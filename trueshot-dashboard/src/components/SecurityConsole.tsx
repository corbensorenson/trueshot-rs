import { useEffect, useMemo, useState } from 'react';
import {
    ApiTokenResponse,
    ApiTokenSummary,
    createApiToken,
    listApiTokens,
    logoutAll,
    revokeApiToken,
} from '../api/client';

interface SecurityConsoleProps {
    isOpen: boolean;
    onClose: () => void;
    onLoggedOut: () => void;
}

const expiryOptions = [
    { value: 'never', label: 'Never', seconds: undefined },
    { value: '1d', label: '24 hours', seconds: 60 * 60 * 24 },
    { value: '7d', label: '7 days', seconds: 60 * 60 * 24 * 7 },
    { value: '30d', label: '30 days', seconds: 60 * 60 * 24 * 30 },
    { value: '90d', label: '90 days', seconds: 60 * 60 * 24 * 90 },
];

const formatTimestamp = (value?: number | null) => {
    if (!value) return 'Never';
    return new Date(value * 1000).toLocaleString();
};

export const SecurityConsole = ({ isOpen, onClose, onLoggedOut }: SecurityConsoleProps) => {
    const [tokens, setTokens] = useState<ApiTokenSummary[]>([]);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [createName, setCreateName] = useState('');
    const [createScopes, setCreateScopes] = useState('read');
    const [createExpiry, setCreateExpiry] = useState('30d');
    const [createdToken, setCreatedToken] = useState<ApiTokenResponse | null>(null);

    const selectedExpiry = useMemo(
        () => expiryOptions.find((option) => option.value === createExpiry),
        [createExpiry]
    );

    const refreshTokens = async () => {
        setLoading(true);
        setError(null);
        try {
            const list = await listApiTokens();
            setTokens(list);
        } catch (err: unknown) {
            const message = err instanceof Error ? err.message : 'Failed to load API tokens';
            setError(message);
        } finally {
            setLoading(false);
        }
    };

    useEffect(() => {
        if (isOpen) {
            refreshTokens();
        }
    }, [isOpen]);

    const handleCreate = async () => {
        setError(null);
        if (!createName.trim()) {
            setError('Token name is required');
            return;
        }
        const scopes = createScopes
            .split(',')
            .map((scope) => scope.trim())
            .filter((scope) => scope.length > 0);
        try {
            const token = await createApiToken({
                name: createName.trim(),
                scopes: scopes.length ? scopes : ['read'],
                expires_in_seconds: selectedExpiry?.seconds,
            });
            setCreatedToken(token);
            setCreateName('');
            setCreateScopes('read');
            setCreateExpiry('30d');
            refreshTokens();
        } catch (err: unknown) {
            const message = err instanceof Error ? err.message : 'Failed to create token';
            setError(message);
        }
    };

    const handleRevoke = async (tokenId: string) => {
        setError(null);
        try {
            await revokeApiToken(tokenId);
            refreshTokens();
        } catch (err: unknown) {
            const message = err instanceof Error ? err.message : 'Failed to revoke token';
            setError(message);
        }
    };

    const handleLogoutAll = async () => {
        setError(null);
        try {
            await logoutAll();
            onLoggedOut();
        } catch (err: unknown) {
            const message = err instanceof Error ? err.message : 'Failed to logout all sessions';
            setError(message);
        }
    };

    const handleCopy = async () => {
        if (!createdToken?.token) return;
        try {
            await navigator.clipboard.writeText(createdToken.token);
        } catch {
            // ignore clipboard errors silently
        }
    };

    if (!isOpen) {
        return null;
    }

    return (
        <div className="fixed inset-0 z-[110] flex items-center justify-center bg-black/80 backdrop-blur-xl">
            <div className="w-[760px] max-h-[90vh] overflow-hidden rounded-2xl border border-white/10 bg-[#0b0b0b] p-6 shadow-2xl text-white">
                <div className="flex items-center justify-between">
                    <div>
                        <div className="text-xs uppercase tracking-[0.3em] text-white/40">Access Control</div>
                        <div className="mt-2 text-xl font-semibold">API Tokens & Sessions</div>
                    </div>
                    <button
                        onClick={onClose}
                        className="rounded-lg border border-white/10 px-3 py-1 text-xs uppercase tracking-[0.2em] text-white/60 hover:text-white"
                    >
                        Close
                    </button>
                </div>

                <div className="mt-6 grid grid-cols-2 gap-6">
                    <div className="rounded-xl border border-white/10 bg-white/5 p-4">
                        <div className="text-[10px] uppercase tracking-[0.25em] text-white/40">Create token</div>
                        <div className="mt-3 space-y-3">
                            <input
                                value={createName}
                                onChange={(e) => setCreateName(e.target.value)}
                                placeholder="Token name (e.g., CI export)"
                                className="w-full rounded-lg border border-white/10 bg-black/60 px-3 py-2 text-sm text-white outline-none focus:border-white/40"
                            />
                            <input
                                value={createScopes}
                                onChange={(e) => setCreateScopes(e.target.value)}
                                placeholder="Scopes: read, capture, process, export, license, admin"
                                className="w-full rounded-lg border border-white/10 bg-black/60 px-3 py-2 text-sm text-white outline-none focus:border-white/40"
                            />
                            <div className="text-[11px] leading-relaxed text-white/40">
                                Use the minimum access needed. Wildcard <span className="font-mono">*</span> grants every capability and must be used alone.
                            </div>
                            <select
                                value={createExpiry}
                                onChange={(e) => setCreateExpiry(e.target.value)}
                                className="w-full rounded-lg border border-white/10 bg-black/60 px-3 py-2 text-sm text-white outline-none focus:border-white/40"
                            >
                                {expiryOptions.map((option) => (
                                    <option key={option.value} value={option.value}>
                                        {option.label}
                                    </option>
                                ))}
                            </select>
                            <button
                                onClick={handleCreate}
                                className="w-full rounded-lg bg-white/90 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-black"
                            >
                                Create Token
                            </button>
                        </div>

                        {createdToken && (
                            <div className="mt-4 rounded-lg border border-white/10 bg-black/60 p-3 text-xs">
                                <div className="flex items-center justify-between text-white/60">
                                    <span>New token</span>
                                    <button onClick={handleCopy} className="text-white/80 hover:text-white">
                                        Copy
                                    </button>
                                </div>
                                <div className="mt-2 break-all font-mono text-white/80">{createdToken.token}</div>
                                <div className="mt-2 text-white/40">Save this token now. It will not be shown again.</div>
                            </div>
                        )}
                    </div>

                    <div className="rounded-xl border border-white/10 bg-white/5 p-4">
                        <div className="flex items-center justify-between">
                            <div className="text-[10px] uppercase tracking-[0.25em] text-white/40">Active tokens</div>
                            <button
                                onClick={refreshTokens}
                                className="text-xs uppercase tracking-[0.2em] text-white/50 hover:text-white"
                            >
                                Refresh
                            </button>
                        </div>
                        <div className="mt-3 max-h-[320px] space-y-2 overflow-y-auto pr-1">
                            {loading && <div className="text-xs text-white/50">Loading tokens...</div>}
                            {!loading && tokens.length === 0 && (
                                <div className="text-xs text-white/40">No tokens created yet.</div>
                            )}
                            {tokens.map((token) => (
                                <div key={token.token_id} className="rounded-lg border border-white/10 bg-black/60 p-3">
                                    <div className="flex items-center justify-between">
                                        <div>
                                            <div className="text-sm font-semibold">{token.name}</div>
                                            <div className="text-[10px] uppercase tracking-[0.2em] text-white/40">
                                                {token.token_id}
                                            </div>
                                        </div>
                                        <button
                                            onClick={() => handleRevoke(token.token_id)}
                                            className="rounded-lg border border-red-500/40 px-2 py-1 text-[10px] uppercase tracking-[0.2em] text-red-400 hover:text-red-200"
                                        >
                                            Revoke
                                        </button>
                                    </div>
                                    <div className="mt-2 text-[11px] text-white/50">
                                        Scopes: {token.scopes.length ? token.scopes.join(', ') : '*'}
                                    </div>
                                    <div className="mt-1 text-[11px] text-white/40">
                                        Created: {formatTimestamp(token.created_at)}
                                    </div>
                                    <div className="text-[11px] text-white/40">
                                        Expires: {formatTimestamp(token.expires_at)}
                                    </div>
                                    <div className="text-[11px] text-white/40">
                                        Last used: {formatTimestamp(token.last_used)}
                                    </div>
                                    {token.revoked && (
                                        <div className="mt-1 text-[10px] uppercase tracking-[0.2em] text-red-400">Revoked</div>
                                    )}
                                </div>
                            ))}
                        </div>
                    </div>
                </div>

                <div className="mt-6 flex items-center justify-between">
                    <div className="text-xs text-white/40">
                        Logout-all revokes refresh tokens for the current operator.
                    </div>
                    <button
                        onClick={handleLogoutAll}
                        className="rounded-lg border border-white/10 px-4 py-2 text-xs uppercase tracking-[0.2em] text-white/70 hover:text-white"
                    >
                        Logout All Sessions
                    </button>
                </div>

                {error && <div className="mt-4 text-xs text-red-400">{error}</div>}
            </div>
        </div>
    );
};
