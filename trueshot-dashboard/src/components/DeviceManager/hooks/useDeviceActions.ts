/**
 * useDeviceActions Hook
 * Device action handlers (capture, enable, disable, etc.)
 */

import { useCallback } from 'react';
import toast from 'react-hot-toast';

type DeviceActionParams = Record<string, unknown> & {
    flash?: boolean;
    countdown_ms?: number;
    quality?: number;
    degrees?: number;
};

interface UseDeviceActionsResult {
    captureDevice: (deviceId: string, params?: DeviceActionParams) => Promise<void>;
    enableDevice: (deviceId: string) => Promise<void>;
    disableDevice: (deviceId: string) => Promise<void>;
    homeDevice: (deviceId: string) => Promise<void>;
    rotateDevice: (deviceId: string, degrees: number) => Promise<void>;
    captureAllPhones: () => Promise<void>;
    scanHardware: () => Promise<void>;
}

async function sendAction(deviceId: string, action: string, params: Record<string, unknown> = {}): Promise<unknown> {
    const response = await fetch(`/api/devices/${deviceId}/action`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action, params }),
    });

    if (!response.ok) {
        const text = await response.text();
        throw new Error(text || `Action failed: ${response.status}`);
    }

    return response.json();
}

async function sendBatchAction(action: string, params: Record<string, unknown> = {}): Promise<unknown> {
    const response = await fetch('/api/devices/batch', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action, params }),
    });

    if (!response.ok) {
        throw new Error(`Batch action failed: ${response.status}`);
    }

    return response.json();
}

export function useDeviceActions(): UseDeviceActionsResult {
    const captureDevice = useCallback(async (deviceId: string, params?: DeviceActionParams) => {
        try {
            await sendAction(deviceId, 'capture', params || {});
            toast.success('Capture triggered');
        } catch (e) {
            toast.error(e instanceof Error ? e.message : 'Capture failed');
        }
    }, []);

    const enableDevice = useCallback(async (deviceId: string) => {
        try {
            await sendAction(deviceId, 'enable');
            toast.success('Device enabled');
        } catch (e) {
            toast.error(e instanceof Error ? e.message : 'Enable failed');
        }
    }, []);

    const disableDevice = useCallback(async (deviceId: string) => {
        try {
            await sendAction(deviceId, 'disable');
            toast.success('Device disabled');
        } catch (e) {
            toast.error(e instanceof Error ? e.message : 'Disable failed');
        }
    }, []);

    const homeDevice = useCallback(async (deviceId: string) => {
        try {
            await sendAction(deviceId, 'home');
            toast.success('Homing...');
        } catch (e) {
            toast.error(e instanceof Error ? e.message : 'Home failed');
        }
    }, []);

    const rotateDevice = useCallback(async (deviceId: string, degrees: number) => {
        try {
            await sendAction(deviceId, 'rotate', { degrees });
            toast.success(`Rotating ${degrees}°`);
        } catch (e) {
            toast.error(e instanceof Error ? e.message : 'Rotate failed');
        }
    }, []);

    const captureAllPhones = useCallback(async () => {
        try {
            const result = await sendBatchAction('capture_all_phones') as { phones_ready?: number };
            toast.success(`Captured from ${result.phones_ready ?? 0} phones`);
        } catch (e) {
            toast.error(e instanceof Error ? e.message : 'Batch capture failed');
        }
    }, []);

    const scanHardware = useCallback(async () => {
        try {
            await sendBatchAction('scan');
            toast.success('Scanning for devices...');
        } catch (e) {
            toast.error(e instanceof Error ? e.message : 'Scan failed');
        }
    }, []);

    return {
        captureDevice,
        enableDevice,
        disableDevice,
        homeDevice,
        rotateDevice,
        captureAllPhones,
        scanHardware,
    };
}
