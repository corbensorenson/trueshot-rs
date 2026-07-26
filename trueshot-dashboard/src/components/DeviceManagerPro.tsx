/**
 * Professional Device Manager v2
 * 
 * Enterprise-grade device management for large 4DGS setups:
 * - Table view for 20+ devices with search and filtering
 * - Multiple device types: cameras, mics, lights, robot arms, turntables
 * - Persistent nicknames with localStorage
 * - External storage: NAS, S3, Google Cloud
 * - Bulk actions and device groups
 * - Detail panel for individual device configuration
 */

import { useState, useEffect, useMemo, useCallback } from 'react';
import {
    X, Camera, Mic, Lightbulb, Cog, HardDrive, Search,
    Filter, MoreVertical, Edit2, Save, CheckCircle,
    RefreshCw, Cloud, Server, Wifi, Usb, Settings,
    Eye, EyeOff, Grid3X3, List, Plus, FolderOpen,
    RotateCw, Zap, Bot
} from 'lucide-react';
import toast from 'react-hot-toast';
import { useHardwareStore } from '../store/hardwareStore';
import {
    createLicenseTrial,
    getLicenseBundles,
    getLicenseStatus,
    type LicenseBundleInfo,
    type LicenseStatusResponse,
} from '../api/client';
import { FeatureUnlockPanel } from './FeatureUnlockPanel';

// ============================================================================
// Types
// ============================================================================

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

type CameraCapabilityInfo = {
    manufacturer?: string;
    model?: string;
};

// ============================================================================
// Main Component
// ============================================================================

interface DeviceManagerProps {
    isOpen: boolean;
    onClose: () => void;
}

export function DeviceManagerPro({ isOpen, onClose }: DeviceManagerProps) {
    // State
    const [view, setView] = useState<'table' | 'grid'>('table');
    const [search, setSearch] = useState('');
    const [typeFilter, setTypeFilter] = useState<DeviceType | 'all'>('all');
    const [statusFilter, setStatusFilter] = useState<DeviceStatus | 'all'>('all');
    const [selectedDevices, setSelectedDevices] = useState<Set<string>>(new Set());
    const [, setSelectedDevice] = useState<Device | null>(null);
    const [showStorageModal, setShowStorageModal] = useState(false);
    const [isScanning, setIsScanning] = useState(false);
    const [editingNickname, setEditingNickname] = useState<string | null>(null);
    const [nicknameValue, setNicknameValue] = useState('');
    const [activeTab, setActiveTab] = useState<'devices' | 'storage' | 'groups'>('devices');
    const [licenseStatus, setLicenseStatus] = useState<LicenseStatusResponse | null>(null);
    const [licenseBundles, setLicenseBundles] = useState<LicenseBundleInfo[]>([]);
    const [unlockBusy, setUnlockBusy] = useState(false);
    const [unlockError, setUnlockError] = useState<string | null>(null);

    // Mock devices - in production, this comes from backend
    const [devices, setDevices] = useState<Device[]>([]);
    const [storages, setStorages] = useState<ExternalStorage[]>([]);

    // Load nicknames from localStorage
    const [nicknames, setNicknames] = useState<Record<string, string>>(() => {
        try {
            return JSON.parse(localStorage.getItem('deviceNicknames') || '{}');
        } catch {
            return {};
        }
    });

    // Persist nicknames
    useEffect(() => {
        localStorage.setItem('deviceNicknames', JSON.stringify(nicknames));
    }, [nicknames]);

    // Load devices from store
    const { cameras, turntable } = useHardwareStore();

    useEffect(() => {
        if (!isOpen) return;
        refreshEntitlement();

        // Convert cameras to unified device format
        const cameraDevices: Device[] = cameras.map(cam => {
            const capabilities = cam.capabilities as CameraCapabilityInfo | undefined;
            return ({
            id: cam.id,
            type: 'camera' as DeviceType,
            name: cam.name,
            nickname: nicknames[cam.id] || cam.nickname || null,
            status: cam.connected ? 'connected' as DeviceStatus : 'disconnected' as DeviceStatus,
            connection: 'usb' as ConnectionType,
            manufacturer: capabilities?.manufacturer,
            model: capabilities?.model,
            lastSeen: new Date(),
            enabled: true,
            metadata: (cam.capabilities ?? {}) as Record<string, unknown>,
        });
        });

        // Add turntable
        const turntableDevice: Device = {
            id: 'turntable-main',
            type: 'turntable',
            name: turntable.type || 'Turntable',
            nickname: nicknames['turntable-main'] || null,
            status: turntable.connected ? 'connected' : 'disconnected',
            connection: 'serial',
            lastSeen: new Date(),
            enabled: true,
            metadata: { moving: turntable.moving },
        };

        setDevices([turntableDevice, ...cameraDevices]);
    }, [isOpen, cameras, turntable, nicknames]);

    const refreshEntitlement = async () => {
        try {
            const [status, bundles] = await Promise.all([getLicenseStatus(), getLicenseBundles()]);
            setLicenseStatus(status);
            setLicenseBundles(bundles);
        } catch {
            setLicenseStatus(null);
            setLicenseBundles([]);
        }
    };

    const formatBundlePrice = (bundle?: LicenseBundleInfo | null) => {
        if (!bundle) return 'Pricing unavailable';
        if (!bundle.price_usd) return 'Contact sales';
        const billing = bundle.billing ? ` ${bundle.billing}` : '';
        return `$${bundle.price_usd}${billing}`;
    };

    const storageLocked = licenseStatus ? !(licenseStatus.license_valid && licenseStatus.features?.cloud_sync_backup) : false;
    const storageBundle = licenseBundles.find(bundle => bundle.key === 'cloud_sync_backup') ?? null;
    const storagePriceLabel = formatBundlePrice(storageBundle);
    const storageBundleName = storageBundle?.name ?? 'Cloud Sync + Backup';
    const trialAvailable = licenseStatus?.trial_available ?? true;

    const startStorageTrial = async () => {
        setUnlockBusy(true);
        setUnlockError(null);
        try {
            await createLicenseTrial({ duration_days: 14, bundles: ['cloud_sync_backup'] });
            await refreshEntitlement();
            toast.success('Cloud Sync + Backup trial activated.');
        } catch (err) {
            const message = err instanceof Error ? err.message : 'Trial activation failed';
            setUnlockError(message);
            toast.error('Trial unavailable. Purchase required.');
        } finally {
            setUnlockBusy(false);
        }
    };

    const openStoragePurchase = () => {
        const subject = encodeURIComponent(`TrueShot purchase: ${storageBundleName}`);
        const body = encodeURIComponent(`I want to buy the ${storageBundleName} lifetime add-on.`);
        window.open(`mailto:sales@trueshot.ai?subject=${subject}&body=${body}`, '_blank');
    };

    // Filtering
    const filteredDevices = useMemo(() => {
        return devices.filter(device => {
            // Search
            const searchLower = search.toLowerCase();
            const matchesSearch =
                device.name.toLowerCase().includes(searchLower) ||
                (device.nickname?.toLowerCase().includes(searchLower)) ||
                device.id.toLowerCase().includes(searchLower) ||
                device.manufacturer?.toLowerCase().includes(searchLower) ||
                device.model?.toLowerCase().includes(searchLower);

            // Type filter
            const matchesType = typeFilter === 'all' || device.type === typeFilter;

            // Status filter
            const matchesStatus = statusFilter === 'all' || device.status === statusFilter;

            return matchesSearch && matchesType && matchesStatus;
        });
    }, [devices, search, typeFilter, statusFilter]);

    // Stats
    const deviceStats = useMemo(() => {
        const connected = devices.filter(d => d.status === 'connected' || d.status === 'ready').length;
        const total = devices.length;
        const byType: Record<DeviceType, number> = {
            camera: 0, depth_camera: 0, microphone: 0, turntable: 0,
            light: 0, robot_arm: 0, storage: 0, sensor: 0, phone: 0,
        };
        devices.forEach(d => byType[d.type]++);
        return { connected, total, byType };
    }, [devices]);

    // Actions
    const handleScan = useCallback(async () => {
        setIsScanning(true);
        try {
            const { scanHardware } = await import('../api/client');
            await scanHardware();
            toast.success('Scanning for devices...');
        } catch (error) {
            console.error(error);
            toast.error('Scan failed');
        } finally {
            setTimeout(() => setIsScanning(false), 2000);
        }
    }, []);

    const handleSaveNickname = useCallback(async (deviceId: string, nickname: string) => {
        setNicknames(prev => ({ ...prev, [deviceId]: nickname }));
        setEditingNickname(null);

        // Also save to backend if it's a camera
        try {
            const { updateCameraNickname } = await import('../api/client');
            await updateCameraNickname(deviceId, nickname);
        } catch (error) {
            console.warn('Failed to persist nickname', error);
        }

        toast.success('Nickname saved');
    }, []);

    const handleToggleDevice = useCallback((deviceId: string) => {
        setDevices(prev => prev.map(d =>
            d.id === deviceId ? { ...d, enabled: !d.enabled } : d
        ));
    }, []);

    const handleBulkEnable = useCallback((enable: boolean) => {
        setDevices(prev => prev.map(d =>
            selectedDevices.has(d.id) ? { ...d, enabled: enable } : d
        ));
        setSelectedDevices(new Set());
        toast.success(`${enable ? 'Enabled' : 'Disabled'} ${selectedDevices.size} devices`);
    }, [selectedDevices]);

    const handleSelectAll = useCallback(() => {
        if (selectedDevices.size === filteredDevices.length) {
            setSelectedDevices(new Set());
        } else {
            setSelectedDevices(new Set(filteredDevices.map(d => d.id)));
        }
    }, [selectedDevices, filteredDevices]);

    if (!isOpen) return null;

    return (
        <div className="device-manager-pro">
            <style>{`
        .device-manager-pro {
          position: fixed;
          inset: 0;
          z-index: 50;
          background: color-mix(in srgb, var(--ts-overlay-strong) 90%, transparent);
          backdrop-filter: blur(12px);
          display: flex;
          flex-direction: column;
          color: var(--ts-text);
          font-family: inherit;
        }
        
        .dm-header {
          display: flex;
          align-items: center;
          justify-content: space-between;
          padding: 1rem 1.5rem;
          border-bottom: 1px solid var(--ts-border);
          background: color-mix(in srgb, var(--ts-text) 4%, transparent);
        }
        
        .dm-title-group {
          display: flex;
          align-items: center;
          gap: 1rem;
        }
        
        .dm-title-icon {
          width: 40px;
          height: 40px;
          border-radius: 10px;
          background: linear-gradient(135deg, #8b5cf6 0%, #6366f1 100%);
          display: flex;
          align-items: center;
          justify-content: center;
        }
        
        .dm-title {
          font-size: 1.25rem;
          font-weight: 600;
        }
        
        .dm-subtitle {
          font-size: 0.75rem;
          color: color-mix(in srgb, var(--ts-text) 55%, transparent);
          text-transform: uppercase;
          letter-spacing: 0.1em;
        }
        
        .dm-tabs {
          display: flex;
          gap: 0.25rem;
          background: color-mix(in srgb, var(--ts-text) 6%, transparent);
          padding: 0.25rem;
          border-radius: 0.5rem;
        }
        
        .dm-tab {
          padding: 0.5rem 1rem;
          border-radius: 0.375rem;
          font-size: 0.875rem;
          cursor: pointer;
          transition: all 0.2s;
          display: flex;
          align-items: center;
          gap: 0.5rem;
        }
        
        .dm-tab:hover {
          background: color-mix(in srgb, var(--ts-text) 10%, transparent);
        }
        
        .dm-tab.active {
          background: var(--ts-accent-purple);
          color: var(--ts-text-on-accent);
        }
        
        .dm-toolbar {
          display: flex;
          align-items: center;
          gap: 1rem;
          padding: 1rem 1.5rem;
          border-bottom: 1px solid var(--ts-border);
        }
        
        .dm-search {
          display: flex;
          align-items: center;
          gap: 0.5rem;
          background: color-mix(in srgb, var(--ts-text) 6%, transparent);
          border: 1px solid var(--ts-border);
          border-radius: 0.5rem;
          padding: 0.5rem 1rem;
          flex: 1;
          max-width: 400px;
        }
        
        .dm-search input {
          background: transparent;
          border: none;
          outline: none;
          color: var(--ts-text);
          flex: 1;
          font-size: 0.875rem;
        }
        
        .dm-search input::placeholder {
          color: color-mix(in srgb, var(--ts-text) 40%, transparent);
        }
        
        .dm-filter {
          display: flex;
          align-items: center;
          gap: 0.5rem;
          background: color-mix(in srgb, var(--ts-text) 6%, transparent);
          border: 1px solid var(--ts-border);
          border-radius: 0.5rem;
          padding: 0.5rem 0.75rem;
          cursor: pointer;
          font-size: 0.875rem;
        }
        
        .dm-filter:hover {
          background: color-mix(in srgb, var(--ts-text) 10%, transparent);
        }
        
        .dm-filter select {
          background: transparent;
          border: none;
          outline: none;
          color: var(--ts-text);
          cursor: pointer;
        }
        
        .dm-stats {
          display: flex;
          gap: 1.5rem;
          margin-left: auto;
        }
        
        .dm-stat {
          display: flex;
          align-items: center;
          gap: 0.5rem;
          font-size: 0.875rem;
        }
        
        .dm-stat-value {
          font-weight: 600;
        }
        
        .dm-stat-label {
          color: color-mix(in srgb, var(--ts-text) 55%, transparent);
        }
        
        .dm-content {
          flex: 1;
          overflow: auto;
          padding: 1.5rem;
        }
        
        .dm-table {
          width: 100%;
          border-collapse: collapse;
        }
        
        .dm-table th {
          text-align: left;
          padding: 0.75rem 1rem;
          font-size: 0.75rem;
          font-weight: 600;
          text-transform: uppercase;
          letter-spacing: 0.05em;
          color: color-mix(in srgb, var(--ts-text) 55%, transparent);
          border-bottom: 1px solid var(--ts-border);
          position: sticky;
          top: 0;
          background: color-mix(in srgb, var(--ts-background) 92%, transparent);
          backdrop-filter: blur(8px);
        }
        
        .dm-table td {
          padding: 0.75rem 1rem;
          border-bottom: 1px solid var(--ts-border);
          font-size: 0.875rem;
        }
        
        .dm-table tr:hover {
          background: color-mix(in srgb, var(--ts-text) 4%, transparent);
        }
        
        .dm-table tr.selected {
          background: color-mix(in srgb, var(--ts-accent-purple) 16%, transparent);
        }
        
        .dm-checkbox {
          width: 16px;
          height: 16px;
          border-radius: 4px;
          border: 2px solid var(--ts-border-strong);
          background: transparent;
          cursor: pointer;
          display: flex;
          align-items: center;
          justify-content: center;
        }
        
        .dm-checkbox.checked {
          background: #8b5cf6;
          border-color: #8b5cf6;
        }
        
        .dm-device-info {
          display: flex;
          align-items: center;
          gap: 0.75rem;
        }
        
        .dm-device-icon {
          width: 32px;
          height: 32px;
          border-radius: 8px;
          display: flex;
          align-items: center;
          justify-content: center;
        }
        
        .dm-device-icon.camera { background: rgba(59, 130, 246, 0.2); color: #3b82f6; }
        .dm-device-icon.depth_camera { background: rgba(6, 182, 212, 0.2); color: #06b6d4; }
        .dm-device-icon.microphone { background: rgba(249, 115, 22, 0.2); color: #f97316; }
        .dm-device-icon.turntable { background: rgba(168, 85, 247, 0.2); color: #a855f7; }
        .dm-device-icon.light { background: rgba(234, 179, 8, 0.2); color: #eab308; }
        .dm-device-icon.robot_arm { background: rgba(239, 68, 68, 0.2); color: #ef4444; }
        .dm-device-icon.storage { background: rgba(34, 197, 94, 0.2); color: #22c55e; }
        .dm-device-icon.sensor { background: rgba(14, 165, 233, 0.2); color: #0ea5e9; }
        
        .dm-device-name {
          font-weight: 500;
        }
        
        .dm-device-nickname {
          font-weight: 600;
          color: var(--ts-accent-purple);
        }
        
        .dm-device-id {
          font-size: 0.75rem;
          color: color-mix(in srgb, var(--ts-text) 40%, transparent);
          font-family: monospace;
        }
        
        .dm-status {
          display: inline-flex;
          align-items: center;
          gap: 0.375rem;
          padding: 0.25rem 0.625rem;
          border-radius: 9999px;
          font-size: 0.75rem;
          font-weight: 500;
        }
        
        .dm-status.connected { background: rgba(34,197,94,0.1); color: #22c55e; }
        .dm-status.disconnected { background: rgba(107,114,128,0.1); color: #6b7280; }
        .dm-status.error { background: rgba(239,68,68,0.1); color: #ef4444; }
        .dm-status.busy { background: rgba(234,179,8,0.1); color: #eab308; }
        
        .dm-status-dot {
          width: 6px;
          height: 6px;
          border-radius: 50%;
          background: currentColor;
        }
        
        .dm-status.connected .dm-status-dot {
          animation: pulse 2s infinite;
        }
        
        @keyframes pulse {
          0%, 100% { opacity: 1; }
          50% { opacity: 0.5; }
        }
        
        .dm-connection {
          display: flex;
          align-items: center;
          gap: 0.375rem;
          font-size: 0.75rem;
          color: color-mix(in srgb, var(--ts-text) 55%, transparent);
        }
        
        .dm-toggle {
          width: 36px;
          height: 20px;
          border-radius: 10px;
          background: color-mix(in srgb, var(--ts-text) 10%, transparent);
          cursor: pointer;
          position: relative;
          transition: all 0.2s;
        }
        
        .dm-toggle.on {
          background: #22c55e;
        }
        
        .dm-toggle-handle {
          position: absolute;
          top: 2px;
          left: 2px;
          width: 16px;
          height: 16px;
          border-radius: 50%;
          background: var(--ts-text);
          transition: all 0.2s;
        }
        
        .dm-toggle.on .dm-toggle-handle {
          left: 18px;
        }
        
        .dm-actions {
          display: flex;
          gap: 0.25rem;
        }
        
        .dm-action-btn {
          width: 28px;
          height: 28px;
          border-radius: 6px;
          display: flex;
          align-items: center;
          justify-content: center;
          cursor: pointer;
          transition: all 0.2s;
          color: color-mix(in srgb, var(--ts-text) 55%, transparent);
        }
        
        .dm-action-btn:hover {
          background: color-mix(in srgb, var(--ts-text) 10%, transparent);
          color: var(--ts-text);
        }
        
        .dm-bulk-bar {
          position: fixed;
          bottom: 2rem;
          left: 50%;
          transform: translateX(-50%);
          background: var(--ts-surface-elevated);
          border: 1px solid var(--ts-border);
          border-radius: 0.75rem;
          padding: 0.75rem 1.5rem;
          display: flex;
          align-items: center;
          gap: 1rem;
          box-shadow: var(--ts-shadow-panel);
        }
        
        .dm-bulk-count {
          font-weight: 600;
          color: var(--ts-accent-purple);
        }
        
        .dm-btn {
          padding: 0.5rem 1rem;
          border-radius: 0.5rem;
          font-size: 0.875rem;
          font-weight: 500;
          cursor: pointer;
          display: flex;
          align-items: center;
          gap: 0.5rem;
          transition: all 0.2s;
          border: none;
        }
        
        .dm-btn-primary {
          background: var(--ts-accent-purple);
          color: var(--ts-text-on-accent);
        }
        
        .dm-btn-primary:hover {
          background: color-mix(in srgb, var(--ts-accent-purple) 85%, #4f46e5);
        }
        
        .dm-btn-secondary {
          background: color-mix(in srgb, var(--ts-text) 10%, transparent);
          color: var(--ts-text);
        }
        
        .dm-btn-secondary:hover {
          background: color-mix(in srgb, var(--ts-text) 18%, transparent);
        }
        
        .dm-btn-danger {
          background: rgba(239,68,68,0.1);
          color: #ef4444;
        }
        
        .dm-nickname-input {
          background: color-mix(in srgb, var(--ts-text) 10%, transparent);
          border: 1px solid var(--ts-accent-purple);
          border-radius: 4px;
          padding: 0.25rem 0.5rem;
          color: var(--ts-text);
          font-size: 0.875rem;
          width: 120px;
        }
        
        .dm-empty {
          display: flex;
          flex-direction: column;
          align-items: center;
          justify-content: center;
          padding: 4rem;
          text-align: center;
        }
        
        .dm-empty-icon {
          width: 64px;
          height: 64px;
          border-radius: 16px;
          background: color-mix(in srgb, var(--ts-text) 6%, transparent);
          display: flex;
          align-items: center;
          justify-content: center;
          margin-bottom: 1rem;
        }
        
        .dm-storage-grid {
          display: grid;
          grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
          gap: 1rem;
        }
        
        .dm-storage-card {
          background: color-mix(in srgb, var(--ts-text) 4%, transparent);
          border: 1px solid var(--ts-border);
          border-radius: 0.75rem;
          padding: 1.25rem;
          cursor: pointer;
          transition: all 0.2s;
        }
        
        .dm-storage-card:hover {
          background: color-mix(in srgb, var(--ts-text) 6%, transparent);
          border-color: var(--ts-border-strong);
        }
        
        .dm-storage-header {
          display: flex;
          align-items: start;
          justify-content: space-between;
          margin-bottom: 1rem;
        }
        
        .dm-storage-icon {
          width: 40px;
          height: 40px;
          border-radius: 10px;
          display: flex;
          align-items: center;
          justify-content: center;
        }
        
        .dm-storage-icon.nas { background: rgba(59,130,246,0.2); color: #3b82f6; }
        .dm-storage-icon.s3 { background: rgba(249,115,22,0.2); color: #f97316; }
        .dm-storage-icon.gcs { background: rgba(34,197,94,0.2); color: #22c55e; }
        .dm-storage-icon.local { background: rgba(139,92,246,0.2); color: #8b5cf6; }
        
        .dm-storage-bar {
          height: 4px;
          background: color-mix(in srgb, var(--ts-text) 18%, transparent);
          border-radius: 2px;
          overflow: hidden;
          margin-top: 0.75rem;
        }
        
        .dm-storage-fill {
          height: 100%;
          background: #8b5cf6;
          border-radius: 2px;
        }
        
        .dm-add-storage {
          border: 2px dashed color-mix(in srgb, var(--ts-text) 20%, transparent);
          background: transparent;
          display: flex;
          flex-direction: column;
          align-items: center;
          justify-content: center;
          min-height: 140px;
          color: var(--ts-muted);
        }
        
        .dm-add-storage:hover {
          border-color: var(--ts-accent-purple);
          color: var(--ts-accent-purple);
        }
      `}</style>

            {/* Header */}
            <div className="dm-header">
                <div className="dm-title-group">
                    <div className="dm-title-icon">
                        <Cog size={22} />
                    </div>
                    <div>
                        <div className="dm-title">Device Manager</div>
                        <div className="dm-subtitle">Hardware Registry & Configuration</div>
                    </div>
                </div>

                <div className="dm-tabs">
                    <div
                        className={`dm-tab ${activeTab === 'devices' ? 'active' : ''}`}
                        onClick={() => setActiveTab('devices')}
                    >
                        <Grid3X3 size={16} />
                        Devices
                    </div>
                    <div
                        className={`dm-tab ${activeTab === 'storage' ? 'active' : ''}`}
                        onClick={() => setActiveTab('storage')}
                    >
                        <HardDrive size={16} />
                        Storage
                    </div>
                    <div
                        className={`dm-tab ${activeTab === 'groups' ? 'active' : ''}`}
                        onClick={() => setActiveTab('groups')}
                    >
                        <FolderOpen size={16} />
                        Groups
                    </div>
                </div>

                <div style={{ display: 'flex', gap: '0.5rem' }}>
                    <button className="dm-btn dm-btn-secondary" onClick={handleScan}>
                        <RefreshCw size={16} className={isScanning ? 'animate-spin' : ''} />
                        Scan
                    </button>
                    <button className="dm-btn dm-btn-secondary" onClick={onClose}>
                        <X size={16} />
                    </button>
                </div>
            </div>

            {/* Toolbar */}
            {activeTab === 'devices' && (
                <div className="dm-toolbar">
                    <div className="dm-search">
                        <Search size={16} color="var(--ts-muted)" />
                        <input
                            type="text"
                            placeholder="Search devices by name, ID, or manufacturer..."
                            value={search}
                            onChange={(e) => setSearch(e.target.value)}
                        />
                    </div>

                    <div className="dm-filter">
                        <Filter size={14} />
                        <select
                            value={typeFilter}
                            onChange={(e) => setTypeFilter(e.target.value as DeviceType | 'all')}
                        >
                            <option value="all">All Types</option>
                            <option value="camera">Cameras</option>
                            <option value="microphone">Microphones</option>
                            <option value="turntable">Turntables</option>
                            <option value="light">Lights</option>
                            <option value="robot_arm">Robot Arms</option>
                        </select>
                    </div>

                    <div className="dm-filter">
                        <select
                            value={statusFilter}
                            onChange={(e) => setStatusFilter(e.target.value as DeviceStatus | 'all')}
                        >
                            <option value="all">All Status</option>
                            <option value="connected">Connected</option>
                            <option value="disconnected">Disconnected</option>
                            <option value="error">Error</option>
                        </select>
                    </div>

                    <div className="dm-stats">
                        <div className="dm-stat">
                            <span className="dm-stat-value" style={{ color: '#22c55e' }}>{deviceStats.connected}</span>
                            <span className="dm-stat-label">Connected</span>
                        </div>
                        <div className="dm-stat">
                            <span className="dm-stat-value">{deviceStats.total}</span>
                            <span className="dm-stat-label">Total</span>
                        </div>
                    </div>

                    <div style={{ display: 'flex', gap: '0.25rem' }}>
                            <button
                                className={`dm-action-btn ${view === 'table' ? 'active' : ''}`}
                                onClick={() => setView('table')}
                                style={{ background: view === 'table' ? 'color-mix(in srgb, var(--ts-accent-purple) 20%, transparent)' : undefined }}
                            >
                            <List size={16} />
                        </button>
                            <button
                                className={`dm-action-btn ${view === 'grid' ? 'active' : ''}`}
                                onClick={() => setView('grid')}
                                style={{ background: view === 'grid' ? 'color-mix(in srgb, var(--ts-accent-purple) 20%, transparent)' : undefined }}
                            >
                            <Grid3X3 size={16} />
                        </button>
                    </div>
                </div>
            )}

            {/* Content */}
            <div className="dm-content">
                {activeTab === 'devices' && (
                    <>
                        {filteredDevices.length === 0 ? (
                            <div className="dm-empty">
                                <div className="dm-empty-icon">
                                    <Zap size={32} color="var(--ts-muted)" />
                                </div>
                                <h3 style={{ marginBottom: '0.5rem' }}>No devices found</h3>
                                <p style={{ color: 'var(--ts-muted)', marginBottom: '1.5rem' }}>
                                    {search ? 'Try a different search term' : 'Connect USB or network devices'}
                                </p>
                                <button className="dm-btn dm-btn-primary" onClick={handleScan}>
                                    <RefreshCw size={16} />
                                    Scan for Devices
                                </button>
                            </div>
                        ) : (
                            <table className="dm-table">
                                <thead>
                                    <tr>
                                        <th style={{ width: 40 }}>
                                            <div
                                                className={`dm-checkbox ${selectedDevices.size === filteredDevices.length ? 'checked' : ''}`}
                                                onClick={handleSelectAll}
                                            >
                                                {selectedDevices.size === filteredDevices.length && (
                                                    <CheckCircle size={12} />
                                                )}
                                            </div>
                                        </th>
                                        <th>Device</th>
                                        <th>Status</th>
                                        <th>Connection</th>
                                        <th>Enabled</th>
                                        <th style={{ width: 120 }}>Actions</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {filteredDevices.map(device => (
                                        <tr
                                            key={device.id}
                                            className={selectedDevices.has(device.id) ? 'selected' : ''}
                                        >
                                            <td>
                                                <div
                                                    className={`dm-checkbox ${selectedDevices.has(device.id) ? 'checked' : ''}`}
                                                    onClick={() => {
                                                        const newSet = new Set(selectedDevices);
                                                        if (newSet.has(device.id)) {
                                                            newSet.delete(device.id);
                                                        } else {
                                                            newSet.add(device.id);
                                                        }
                                                        setSelectedDevices(newSet);
                                                    }}
                                                >
                                                    {selectedDevices.has(device.id) && <CheckCircle size={12} />}
                                                </div>
                                            </td>
                                            <td>
                                                <div className="dm-device-info">
                                                    <div className={`dm-device-icon ${device.type}`}>
                                                        {device.type === 'camera' && <Camera size={16} />}
                                                        {device.type === 'microphone' && <Mic size={16} />}
                                                        {device.type === 'turntable' && <RotateCw size={16} />}
                                                        {device.type === 'light' && <Lightbulb size={16} />}
                                                        {device.type === 'robot_arm' && <Bot size={16} />}
                                                    </div>
                                                    <div>
                                                        {editingNickname === device.id ? (
                                                            <div style={{ display: 'flex', gap: '0.25rem' }}>
                                                                <input
                                                                    className="dm-nickname-input"
                                                                    value={nicknameValue}
                                                                    onChange={(e) => setNicknameValue(e.target.value)}
                                                                    placeholder="Nickname"
                                                                    autoFocus
                                                                    onKeyDown={(e) => {
                                                                        if (e.key === 'Enter') handleSaveNickname(device.id, nicknameValue);
                                                                        if (e.key === 'Escape') setEditingNickname(null);
                                                                    }}
                                                                />
                                                                <button
                                                                    className="dm-action-btn"
                                                                    onClick={() => handleSaveNickname(device.id, nicknameValue)}
                                                                >
                                                                    <Save size={14} />
                                                                </button>
                                                            </div>
                                                        ) : (
                                                            <>
                                                                <div
                                                                    className={device.nickname ? 'dm-device-nickname' : 'dm-device-name'}
                                                                    onClick={() => {
                                                                        setEditingNickname(device.id);
                                                                        setNicknameValue(device.nickname || '');
                                                                    }}
                                                                    style={{ cursor: 'pointer' }}
                                                                >
                                                                    {device.nickname || device.name}
                                                                </div>
                                                                <div className="dm-device-id">{device.id}</div>
                                                            </>
                                                        )}
                                                    </div>
                                                </div>
                                            </td>
                                            <td>
                                                <div className={`dm-status ${device.status}`}>
                                                    <div className="dm-status-dot" />
                                                    {device.status}
                                                </div>
                                            </td>
                                            <td>
                                                <div className="dm-connection">
                                                    {device.connection === 'usb' && <Usb size={12} />}
                                                    {device.connection === 'network' && <Wifi size={12} />}
                                                    {device.connection === 'bluetooth' && <Wifi size={12} />}
                                                    {device.connection === 'serial' && <Server size={12} />}
                                                    {device.connection}
                                                </div>
                                            </td>
                                            <td>
                                                <div
                                                    className={`dm-toggle ${device.enabled ? 'on' : ''}`}
                                                    onClick={() => handleToggleDevice(device.id)}
                                                >
                                                    <div className="dm-toggle-handle" />
                                                </div>
                                            </td>
                                            <td>
                                                <div className="dm-actions">
                                                    <button
                                                        className="dm-action-btn"
                                                        onClick={() => {
                                                            setEditingNickname(device.id);
                                                            setNicknameValue(device.nickname || '');
                                                        }}
                                                        title="Edit nickname"
                                                    >
                                                        <Edit2 size={14} />
                                                    </button>
                                                    <button
                                                        className="dm-action-btn"
                                                        onClick={() => setSelectedDevice(device)}
                                                        title="Device settings"
                                                    >
                                                        <Settings size={14} />
                                                    </button>
                                                    <button className="dm-action-btn" title="More options">
                                                        <MoreVertical size={14} />
                                                    </button>
                                                </div>
                                            </td>
                                        </tr>
                                    ))}
                                </tbody>
                            </table>
                        )}
                    </>
                )}

                {activeTab === 'storage' && (
                    <>
                        {storageLocked && (
                            <FeatureUnlockPanel
                                title="Cloud Sync + Backup"
                                subtitle="Connect NAS and cloud storage, validate sync integrity, and manage automated backups."
                                bundleName={storageBundleName}
                                priceLabel={storagePriceLabel}
                                capabilities={[
                                    'NAS / S3 / GCS / Azure connectors',
                                    'Sync validation with integrity checks',
                                    'Provider-backed backups and restores',
                                    'Storage health monitoring and alerts',
                                ]}
                                trialAvailable={trialAvailable}
                                onStartTrial={startStorageTrial}
                                onBuy={openStoragePurchase}
                                busy={unlockBusy}
                                errorMessage={unlockError}
                            />
                        )}
                        {!storageLocked && (
                            <div className="dm-storage-grid">
                                {storages.map(storage => (
                                    <div key={storage.id} className="dm-storage-card">
                                        <div className="dm-storage-header">
                                            <div style={{ display: 'flex', alignItems: 'center', gap: '0.75rem' }}>
                                                <div className={`dm-storage-icon ${storage.type}`}>
                                                    {storage.type === 'nas' && <Server size={20} />}
                                                    {storage.type === 's3' && <Cloud size={20} />}
                                                    {storage.type === 'gcs' && <Cloud size={20} />}
                                                    {storage.type === 'local' && <HardDrive size={20} />}
                                                </div>
                                                <div>
                                                    <div style={{ fontWeight: 600 }}>{storage.name}</div>
                                                    <div style={{ fontSize: '0.75rem', color: 'var(--ts-muted)' }}>
                                                        {storage.type.toUpperCase()}
                                                    </div>
                                                </div>
                                            </div>
                                            <div className={`dm-status ${storage.status}`}>
                                                <div className="dm-status-dot" />
                                                {storage.status}
                                            </div>
                                        </div>
                                        {storage.endpoint && (
                                            <div style={{ fontSize: '0.75rem', color: 'color-mix(in srgb, var(--ts-text) 45%, transparent)', fontFamily: 'monospace' }}>
                                                {storage.endpoint}
                                            </div>
                                        )}
                                        {storage.totalBytes && (
                                            <>
                                                <div className="dm-storage-bar">
                                                    <div
                                                        className="dm-storage-fill"
                                                        style={{ width: `${((storage.usedBytes || 0) / storage.totalBytes) * 100}%` }}
                                                    />
                                                </div>
                                                <div style={{ fontSize: '0.75rem', color: 'var(--ts-muted)', marginTop: '0.5rem' }}>
                                                    {((storage.usedBytes || 0) / 1e9).toFixed(1)} / {(storage.totalBytes / 1e9).toFixed(0)} GB
                                                </div>
                                            </>
                                        )}
                                    </div>
                                ))}

                                {/* Add Storage Button */}
                                <div
                                    className="dm-storage-card dm-add-storage"
                                    onClick={() => setShowStorageModal(true)}
                                >
                                    <Plus size={24} />
                                    <div style={{ marginTop: '0.5rem', fontWeight: 500 }}>Add External Storage</div>
                                    <div style={{ fontSize: '0.75rem', marginTop: '0.25rem' }}>NAS, S3, Google Cloud</div>
                                </div>
                            </div>
                        )}
                    </>
                )}

                {activeTab === 'groups' && (
                    <div className="dm-empty">
                        <div className="dm-empty-icon">
                            <FolderOpen size={32} color="var(--ts-muted)" />
                        </div>
                        <h3 style={{ marginBottom: '0.5rem' }}>Device Groups</h3>
                        <p style={{ color: 'var(--ts-muted)', marginBottom: '1.5rem' }}>
                            Organize devices into groups for batch operations
                        </p>
                        <button className="dm-btn dm-btn-primary">
                            <Plus size={16} />
                            Create Group
                        </button>
                    </div>
                )}
            </div>

            {/* Bulk Actions Bar */}
            {selectedDevices.size > 0 && (
                <div className="dm-bulk-bar">
                    <span className="dm-bulk-count">{selectedDevices.size} selected</span>
                    <button className="dm-btn dm-btn-secondary" onClick={() => handleBulkEnable(true)}>
                        <Eye size={16} />
                        Enable All
                    </button>
                    <button className="dm-btn dm-btn-secondary" onClick={() => handleBulkEnable(false)}>
                        <EyeOff size={16} />
                        Disable All
                    </button>
                    <button className="dm-btn dm-btn-danger" onClick={() => setSelectedDevices(new Set())}>
                        <X size={16} />
                        Clear
                    </button>
                </div>
            )}

            {/* Storage Modal */}
            {showStorageModal && !storageLocked && (
                <AddStorageModal onClose={() => setShowStorageModal(false)} onAdd={(storage) => {
                    setStorages(prev => [...prev, storage]);
                    setShowStorageModal(false);
                    toast.success('Storage added');
                }} />
            )}
        </div>
    );
}

// ============================================================================
// Add Storage Modal
// ============================================================================

interface AddStorageModalProps {
    onClose: () => void;
    onAdd: (storage: ExternalStorage) => void;
}

function AddStorageModal({ onClose, onAdd }: AddStorageModalProps) {
    const [storageType, setStorageType] = useState<'nas' | 's3' | 'gcs' | 'local'>('nas');
    const [name, setName] = useState('');
    const [endpoint, setEndpoint] = useState('');
    const [bucket, setBucket] = useState('');
    const [accessKey, setAccessKey] = useState('');
    const [secretKey, setSecretKey] = useState('');

    const handleSubmit = () => {
        const storage: ExternalStorage = {
            id: `storage-${Date.now()}`,
            name: name || `${storageType.toUpperCase()} Storage`,
            type: storageType,
            status: 'connected',
            endpoint,
            bucket,
        };
        onAdd(storage);
    };

    return (
        <div style={{
            position: 'fixed',
            inset: 0,
            background: 'var(--ts-overlay-strong)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            zIndex: 100,
        }}>
            <div style={{
                background: 'var(--ts-surface)',
                border: '1px solid var(--ts-border)',
                borderRadius: '1rem',
                padding: '2rem',
                width: '100%',
                maxWidth: 480,
            }}>
                <h2 style={{ fontSize: '1.25rem', fontWeight: 600, marginBottom: '1.5rem' }}>
                    Add External Storage
                </h2>

                <div style={{ display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)', gap: '0.5rem', marginBottom: '1.5rem' }}>
                    {(['nas', 's3', 'gcs', 'local'] as const).map(type => (
                        <button
                            key={type}
                            onClick={() => setStorageType(type)}
                            style={{
                                padding: '1rem',
                                borderRadius: '0.5rem',
                                border: storageType === type ? '2px solid var(--ts-accent-purple)' : '1px solid var(--ts-border)',
                                background: storageType === type
                                    ? 'color-mix(in srgb, var(--ts-accent-purple) 14%, transparent)'
                                    : 'color-mix(in srgb, var(--ts-text) 4%, transparent)',
                                display: 'flex',
                                flexDirection: 'column',
                                alignItems: 'center',
                                gap: '0.5rem',
                                cursor: 'pointer',
                            }}
                        >
                            {type === 'nas' && <Server size={20} />}
                            {type === 's3' && <Cloud size={20} />}
                            {type === 'gcs' && <Cloud size={20} />}
                            {type === 'local' && <HardDrive size={20} />}
                            <span style={{ fontSize: '0.75rem', textTransform: 'uppercase' }}>{type}</span>
                        </button>
                    ))}
                </div>

                <div style={{ display: 'flex', flexDirection: 'column', gap: '1rem' }}>
                    <div>
                        <label style={{ fontSize: '0.75rem', color: 'var(--ts-muted)', display: 'block', marginBottom: '0.25rem' }}>
                            Display Name
                        </label>
                        <input
                            type="text"
                            value={name}
                            onChange={(e) => setName(e.target.value)}
                            placeholder="My Storage"
                            style={{
                                width: '100%',
                                padding: '0.75rem',
                                borderRadius: '0.5rem',
                                border: '1px solid var(--ts-border)',
                                background: 'color-mix(in srgb, var(--ts-text) 6%, transparent)',
                                color: 'var(--ts-text)',
                            }}
                        />
                    </div>

                    {storageType === 'nas' && (
                        <div>
                            <label style={{ fontSize: '0.75rem', color: 'var(--ts-muted)', display: 'block', marginBottom: '0.25rem' }}>
                                Network Path (SMB/NFS)
                            </label>
                            <input
                                type="text"
                                value={endpoint}
                                onChange={(e) => setEndpoint(e.target.value)}
                                placeholder="//192.168.1.100/share"
                                style={{
                                    width: '100%',
                                    padding: '0.75rem',
                                    borderRadius: '0.5rem',
                                    border: '1px solid var(--ts-border)',
                                    background: 'color-mix(in srgb, var(--ts-text) 6%, transparent)',
                                    color: 'var(--ts-text)',
                                }}
                            />
                        </div>
                    )}

                    {(storageType === 's3' || storageType === 'gcs') && (
                        <>
                            <div>
                                <label style={{ fontSize: '0.75rem', color: 'var(--ts-muted)', display: 'block', marginBottom: '0.25rem' }}>
                                    {storageType === 's3' ? 'Endpoint URL' : 'Project ID'}
                                </label>
                                <input
                                    type="text"
                                    value={endpoint}
                                    onChange={(e) => setEndpoint(e.target.value)}
                                    placeholder={storageType === 's3' ? 's3.amazonaws.com' : 'my-project-123'}
                                    style={{
                                        width: '100%',
                                        padding: '0.75rem',
                                        borderRadius: '0.5rem',
                                        border: '1px solid var(--ts-border)',
                                        background: 'color-mix(in srgb, var(--ts-text) 6%, transparent)',
                                        color: 'var(--ts-text)',
                                    }}
                                />
                            </div>
                            <div>
                                <label style={{ fontSize: '0.75rem', color: 'var(--ts-muted)', display: 'block', marginBottom: '0.25rem' }}>
                                    Bucket Name
                                </label>
                                <input
                                    type="text"
                                    value={bucket}
                                    onChange={(e) => setBucket(e.target.value)}
                                    placeholder="my-bucket"
                                    style={{
                                        width: '100%',
                                        padding: '0.75rem',
                                        borderRadius: '0.5rem',
                                        border: '1px solid var(--ts-border)',
                                        background: 'color-mix(in srgb, var(--ts-text) 6%, transparent)',
                                        color: 'var(--ts-text)',
                                    }}
                                />
                            </div>
                            <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '1rem' }}>
                                <div>
                                    <label style={{ fontSize: '0.75rem', color: 'var(--ts-muted)', display: 'block', marginBottom: '0.25rem' }}>
                                        Access Key
                                    </label>
                                    <input
                                        type="password"
                                        value={accessKey}
                                        onChange={(e) => setAccessKey(e.target.value)}
                                        style={{
                                            width: '100%',
                                            padding: '0.75rem',
                                            borderRadius: '0.5rem',
                                            border: '1px solid var(--ts-border)',
                                            background: 'color-mix(in srgb, var(--ts-text) 6%, transparent)',
                                            color: 'var(--ts-text)',
                                        }}
                                    />
                                </div>
                                <div>
                                    <label style={{ fontSize: '0.75rem', color: 'var(--ts-muted)', display: 'block', marginBottom: '0.25rem' }}>
                                        Secret Key
                                    </label>
                                    <input
                                        type="password"
                                        value={secretKey}
                                        onChange={(e) => setSecretKey(e.target.value)}
                                        style={{
                                            width: '100%',
                                            padding: '0.75rem',
                                            borderRadius: '0.5rem',
                                            border: '1px solid var(--ts-border)',
                                            background: 'color-mix(in srgb, var(--ts-text) 6%, transparent)',
                                            color: 'var(--ts-text)',
                                        }}
                                    />
                                </div>
                            </div>
                        </>
                    )}
                </div>

                <div style={{ display: 'flex', justifyContent: 'flex-end', gap: '0.75rem', marginTop: '2rem' }}>
                    <button className="dm-btn dm-btn-secondary" onClick={onClose}>
                        Cancel
                    </button>
                    <button className="dm-btn dm-btn-primary" onClick={handleSubmit}>
                        <Plus size={16} />
                        Add Storage
                    </button>
                </div>
            </div>
        </div>
    );
}

export default DeviceManagerPro;
