/**
 * Device Manager Types
 * Shared type definitions for device management
 */

export type DeviceType =
    | 'camera'
    | 'depth_camera'
    | 'microphone'
    | 'turntable'
    | 'light'
    | 'robot_arm'
    | 'storage'
    | 'sensor'
    | 'phone';

export type ConnectionType = 'usb' | 'network' | 'bluetooth' | 'serial' | 'cloud';

export type DeviceStatus = 'connected' | 'disconnected' | 'error' | 'busy' | 'initializing' | 'ready';

export interface Device {
    id: string;
    type: DeviceType;
    name: string;
    nickname: string | null;
    status: DeviceStatus;
    connection: ConnectionType;
    manufacturer?: string;
    model?: string;
    serialNumber?: string;
    firmwareVersion?: string;
    lastSeen: Date;
    enabled: boolean;
    groupId?: string;
    metadata: Record<string, unknown>;
}

export interface DeviceGroup {
    id: string;
    name: string;
    color: string;
    deviceIds: string[];
}

export interface ExternalStorage {
    id: string;
    name: string;
    type: 'nas' | 's3' | 'gcs' | 'azure' | 'local' | 'google_drive' | 'dropbox' | 'onedrive' | 'icloud';
    status: 'connected' | 'disconnected' | 'syncing' | 'error' | 'needs_reauth';
    endpoint?: string;
    bucket?: string;
    path?: string;
    email?: string;
    usedBytes?: number;
    totalBytes?: number;
    lastSync?: Date;
}

export interface DeviceManagerProps {
    isOpen: boolean;
    onClose: () => void;
}

// Device type display info
export const DEVICE_TYPE_INFO: Record<DeviceType, { icon: string; label: string; color: string }> = {
    camera: { icon: 'Camera', label: 'Camera', color: '#10b981' },
    depth_camera: { icon: 'Eye', label: 'Depth Camera', color: '#8b5cf6' },
    microphone: { icon: 'Mic', label: 'Microphone', color: '#f59e0b' },
    turntable: { icon: 'RotateCw', label: 'Turntable', color: '#3b82f6' },
    light: { icon: 'Lightbulb', label: 'Light', color: '#eab308' },
    robot_arm: { icon: 'Bot', label: 'Robot Arm', color: '#ec4899' },
    storage: { icon: 'HardDrive', label: 'Storage', color: '#6366f1' },
    sensor: { icon: 'Zap', label: 'Sensor', color: '#14b8a6' },
    phone: { icon: 'Smartphone', label: 'Phone', color: '#f97316' },
};

// Connection type display info
export const CONNECTION_TYPE_INFO: Record<ConnectionType, { icon: string; label: string }> = {
    usb: { icon: 'Usb', label: 'USB' },
    network: { icon: 'Wifi', label: 'Network' },
    bluetooth: { icon: 'Bluetooth', label: 'Bluetooth' },
    serial: { icon: 'Cable', label: 'Serial' },
    cloud: { icon: 'Cloud', label: 'Cloud' },
};

// Status display info
export const STATUS_INFO: Record<DeviceStatus, { color: string; label: string }> = {
    connected: { color: '#10b981', label: 'Connected' },
    disconnected: { color: '#6b7280', label: 'Disconnected' },
    error: { color: '#ef4444', label: 'Error' },
    busy: { color: '#f59e0b', label: 'Busy' },
    initializing: { color: '#3b82f6', label: 'Initializing' },
    ready: { color: '#22c55e', label: 'Ready' },
};
