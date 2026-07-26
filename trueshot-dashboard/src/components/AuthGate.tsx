import { useEffect, useState } from 'react';
import { bootstrapAdmin, getBootstrapStatus, loginWithPassword } from '../api/client';

interface AuthGateProps {
    onAuthenticated: () => void;
}

export const AuthGate = ({ onAuthenticated }: AuthGateProps) => {
    const [mode, setMode] = useState<'loading' | 'bootstrap' | 'login'>('loading');
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [email, setEmail] = useState('');
    const [name, setName] = useState('');
    const [password, setPassword] = useState('');
    const [passwordConfirm, setPasswordConfirm] = useState('');

    useEffect(() => {
        let mounted = true;
        getBootstrapStatus()
            .then((status) => {
                if (!mounted) return;
                setMode(status.required ? 'bootstrap' : 'login');
            })
            .catch((err: unknown) => {
                if (!mounted) return;
                const message = err instanceof Error ? err.message : 'Failed to check bootstrap status';
                setError(message);
                setMode('login');
            });
        return () => {
            mounted = false;
        };
    }, []);

    const handleBootstrap = async () => {
        setError(null);
        setBusy(true);
        try {
            if (!email.trim() || !name.trim()) {
                throw new Error('Name and email are required');
            }
            if (password.length < 12) {
                throw new Error('Password must be at least 12 characters');
            }
            if (password !== passwordConfirm) {
                throw new Error('Passwords do not match');
            }
            await bootstrapAdmin({
                email: email.trim(),
                name: name.trim(),
                password,
            });
            onAuthenticated();
        } catch (err: unknown) {
            const message = err instanceof Error ? err.message : 'Failed to bootstrap admin';
            setError(message);
        } finally {
            setBusy(false);
        }
    };

    const handleLogin = async () => {
        setError(null);
        setBusy(true);
        try {
            if (!email.trim() || !password) {
                throw new Error('Email and password are required');
            }
            await loginWithPassword({
                email: email.trim(),
                password,
            });
            onAuthenticated();
        } catch (err: unknown) {
            const message = err instanceof Error ? err.message : 'Failed to authenticate';
            setError(message);
        } finally {
            setBusy(false);
        }
    };

    return (
        <div className="fixed inset-0 z-[120] flex items-center justify-center bg-[color:var(--ts-overlay-strong)] backdrop-blur-xl">
            <div className="w-[440px] ts-panel-strong p-6">
                <div className="text-xs uppercase tracking-[0.3em] text-[color:var(--ts-muted)]">TrueShot Secure Setup</div>
                <div className="mt-2 text-xl font-semibold">
                    {mode === 'bootstrap' ? 'Create the first admin' : 'Sign in'}
                </div>
                <p className="mt-2 text-xs text-[color:color-mix(in_srgb,var(--ts-text)_60%,transparent)]">
                    {mode === 'bootstrap'
                        ? 'Bootstrap requires an owner account. This will disable the API key.'
                        : 'Sign in with your admin credentials.'}
                </p>

                <div className="mt-6 space-y-4">
                    {mode === 'loading' && (
                        <div className="rounded-lg border border-[color:var(--ts-border)] bg-[color:color-mix(in_srgb,var(--ts-surface)_75%,transparent)] p-4 text-xs text-[color:color-mix(in_srgb,var(--ts-text)_70%,transparent)]">
                            Checking bootstrap status...
                        </div>
                    )}

                    {mode !== 'loading' && (
                        <>
                            <div className="space-y-3">
                                <div>
                                    <label className="text-[10px] uppercase tracking-[0.25em] text-[color:var(--ts-muted)]">Email</label>
                                    <input
                                        value={email}
                                        onChange={(e) => setEmail(e.target.value)}
                                        placeholder="admin@studio.com"
                                        className="mt-2 w-full px-3 py-2 text-sm ts-input"
                                        type="email"
                                        autoComplete="email"
                                    />
                                </div>

                                {mode === 'bootstrap' && (
                                    <div>
                                        <label className="text-[10px] uppercase tracking-[0.25em] text-[color:var(--ts-muted)]">Name</label>
                                        <input
                                            value={name}
                                            onChange={(e) => setName(e.target.value)}
                                            placeholder="Studio Admin"
                                            className="mt-2 w-full px-3 py-2 text-sm ts-input"
                                            type="text"
                                            autoComplete="name"
                                        />
                                    </div>
                                )}

                                <div>
                                    <label className="text-[10px] uppercase tracking-[0.25em] text-[color:var(--ts-muted)]">
                                        {mode === 'bootstrap' ? 'Create password' : 'Password'}
                                    </label>
                                    <input
                                        value={password}
                                        onChange={(e) => setPassword(e.target.value)}
                                        placeholder={mode === 'bootstrap' ? 'Minimum 12 characters' : 'Your password'}
                                        className="mt-2 w-full px-3 py-2 text-sm ts-input"
                                        type="password"
                                        autoComplete={mode === 'bootstrap' ? 'new-password' : 'current-password'}
                                    />
                                </div>

                                {mode === 'bootstrap' && (
                                    <div>
                                        <label className="text-[10px] uppercase tracking-[0.25em] text-[color:var(--ts-muted)]">Confirm password</label>
                                        <input
                                            value={passwordConfirm}
                                            onChange={(e) => setPasswordConfirm(e.target.value)}
                                            placeholder="Re-enter password"
                                            className="mt-2 w-full px-3 py-2 text-sm ts-input"
                                            type="password"
                                            autoComplete="new-password"
                                        />
                                    </div>
                                )}
                            </div>

                            <button
                                onClick={mode === 'bootstrap' ? handleBootstrap : handleLogin}
                                disabled={busy}
                                className="mt-2 w-full rounded-lg py-2 text-xs font-semibold uppercase tracking-[0.2em] ts-button-primary disabled:opacity-40"
                            >
                                {busy
                                    ? mode === 'bootstrap'
                                        ? 'Creating admin...'
                                        : 'Signing in...'
                                    : mode === 'bootstrap'
                                        ? 'Create Admin'
                                        : 'Sign In'}
                            </button>
                        </>
                    )}

                    {error && <div className="text-xs text-red-500">{error}</div>}
                </div>
            </div>
        </div>
    );
};
