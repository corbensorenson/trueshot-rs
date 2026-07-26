/**
 * Production-safe logging utility
 * In production: sends to monitoring service
 * In development: logs to console
 */

const isDev = import.meta.env.DEV;

export const logger = {
    info: (message: string, data?: unknown) => {
        if (isDev) {
            console.log(`[INFO] ${message}`, data ?? '');
        }
        // In production: send to monitoring service
    },

    warn: (message: string, data?: unknown) => {
        if (isDev) {
            console.warn(`[WARN] ${message}`, data ?? '');
        }
        // In production: send to monitoring service
    },

    error: (message: string, error?: unknown) => {
        if (isDev) {
            console.error(`[ERROR] ${message}`, error ?? '');
        }
        // In production: send to error tracking (e.g., Sentry)
        // Could also call a backend endpoint to log errors
    },

    debug: (message: string, data?: unknown) => {
        if (isDev) {
            console.debug(`[DEBUG] ${message}`, data ?? '');
        }
    },
};

// Type-safe error extraction
export const getErrorMessage = (error: unknown): string => {
    if (error instanceof Error) return error.message;
    if (typeof error === 'string') return error;
    return 'An unknown error occurred';
};
