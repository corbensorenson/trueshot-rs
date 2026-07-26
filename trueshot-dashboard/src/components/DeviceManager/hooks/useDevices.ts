/**
 * useDevices Hook
 * Fetches and manages device state from the unified /api/devices endpoint
 */

import { useState, useEffect, useCallback } from 'react';
import { Device, DeviceType } from '../types';

interface DeviceStats {
    total: number;
    connected: number;
    byType: Record<DeviceType, number>;
    byConnection: Record<string, number>;
}

interface UseDevicesResult {
    devices: Device[];
    stats: DeviceStats;
    isLoading: boolean;
    error: string | null;
    refresh: () => Promise<void>;
}

const POLL_INTERVAL_MS = 3000;

type DeviceApi = Omit<Device, 'lastSeen'> & { last_seen?: string; lastSeen?: string };

export function useDevices(): UseDevicesResult {
    const [devices, setDevices] = useState<Device[]>([]);
    const [isLoading, setIsLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);

    const fetchDevices = useCallback(async () => {
        try {
            const response = await fetch('/api/devices');
            if (!response.ok) {
                throw new Error(`HTTP ${response.status}`);
            }
            const data = await response.json() as DeviceApi[];

            // Parse lastSeen dates
            const parsed = data.map((d) => {
                const lastSeenRaw = d.last_seen ?? d.lastSeen;
                return {
                    ...d,
                    lastSeen: lastSeenRaw ? new Date(lastSeenRaw) : new Date(0),
                };
            });

            setDevices(parsed);
            setError(null);
        } catch (e) {
            setError(e instanceof Error ? e.message : 'Failed to fetch devices');
        } finally {
            setIsLoading(false);
        }
    }, []);

    // Initial fetch and polling
    useEffect(() => {
        fetchDevices();
        const interval = setInterval(fetchDevices, POLL_INTERVAL_MS);
        return () => clearInterval(interval);
    }, [fetchDevices]);

    // Compute stats
    const stats: DeviceStats = {
        total: devices.length,
        connected: devices.filter(d => d.status === 'connected' || d.status === 'ready').length,
        byType: devices.reduce((acc, d) => {
            acc[d.type] = (acc[d.type] || 0) + 1;
            return acc;
        }, {} as Record<DeviceType, number>),
        byConnection: devices.reduce((acc, d) => {
            acc[d.connection] = (acc[d.connection] || 0) + 1;
            return acc;
        }, {} as Record<string, number>),
    };

    return {
        devices,
        stats,
        isLoading,
        error,
        refresh: fetchDevices,
    };
}
