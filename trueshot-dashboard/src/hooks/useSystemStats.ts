import { useState, useEffect } from 'react';
import { getSystemStats } from '../api/client';

export interface SystemStats {
    cpu_usage: number;
    memory_used_mb: number;
    memory_total_mb: number;
    disk_free_gb: number;
}

export function useSystemStats(intervalMs: number = 3000) {
    const [stats, setStats] = useState<SystemStats | null>(null);
    const [loading, setLoading] = useState(true);

    useEffect(() => {
        let mounted = true;

        const fetchStats = async () => {
            try {
                const data = await getSystemStats();
                if (mounted) {
                    setStats(data);
                    setLoading(false);
                }
            } catch (e) {
                console.error("Failed to fetch system stats", e);
            }
        };

        fetchStats();
        const interval = setInterval(fetchStats, intervalMs);

        return () => {
            mounted = false;
            clearInterval(interval);
        };
    }, [intervalMs]);

    return { stats, loading };
}
