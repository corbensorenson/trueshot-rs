/**
 * DeviceCard Component
 * Individual device card for grid view
 */

import type { ComponentType, CSSProperties } from 'react';
import { Camera, Mic, Lightbulb, RotateCw, HardDrive, Zap, Bot, Smartphone, Eye, Wifi, Usb, Cloud } from 'lucide-react';
import { Device, DeviceType, ConnectionType, STATUS_INFO } from './types';
import './DeviceCard.css';

interface DeviceCardProps {
    device: Device;
    isSelected: boolean;
    nickname?: string;
    onSelect: () => void;
    onToggle: () => void;
    onCapture?: () => void;
}

type IconProps = { size?: number; className?: string; color?: string; style?: CSSProperties };
type IconComponent = ComponentType<IconProps>;

const DEVICE_ICONS: Record<DeviceType, IconComponent> = {
    camera: Camera,
    depth_camera: Eye,
    microphone: Mic,
    turntable: RotateCw,
    light: Lightbulb,
    robot_arm: Bot,
    storage: HardDrive,
    sensor: Zap,
    phone: Smartphone,
};

const CONNECTION_ICONS: Record<ConnectionType, IconComponent> = {
    usb: Usb,
    network: Wifi,
    bluetooth: ({ size }) => <span style={{ fontSize: size ? `${size}px` : undefined }}>📶</span>,
    serial: ({ size }) => <span style={{ fontSize: size ? `${size}px` : undefined }}>🔌</span>,
    cloud: Cloud,
};

export function DeviceCard({ device, isSelected, nickname, onSelect, onToggle, onCapture }: DeviceCardProps) {
    const Icon = DEVICE_ICONS[device.type] || Camera;
    const ConnectionIcon = CONNECTION_ICONS[device.connection] || Wifi;
    const statusInfo = STATUS_INFO[device.status];

    return (
        <div
            className={`device-card ${isSelected ? 'selected' : ''} ${!device.enabled ? 'disabled' : ''}`}
            onClick={onSelect}
        >
            <div className="device-card-header">
                <div className="device-icon" style={{ backgroundColor: statusInfo.color + '20' }}>
                    <Icon size={24} style={{ color: statusInfo.color }} />
                </div>
                <div className="device-connection">
                    <ConnectionIcon size={14} />
                </div>
            </div>

            <div className="device-card-body">
                <h4 className="device-name">{nickname || device.name}</h4>
                <p className="device-model">{device.manufacturer} {device.model}</p>

                <div className="device-status">
                    <span className="status-dot" style={{ backgroundColor: statusInfo.color }} />
                    <span className="status-label">{statusInfo.label}</span>
                </div>
            </div>

            <div className="device-card-actions">
                <button
                    className="device-toggle"
                    onClick={(e) => { e.stopPropagation(); onToggle(); }}
                    title={device.enabled ? 'Disable' : 'Enable'}
                >
                    {device.enabled ? '✓' : '○'}
                </button>
                {onCapture && (device.type === 'camera' || device.type === 'phone') && (
                    <button
                        className="device-capture"
                        onClick={(e) => { e.stopPropagation(); onCapture(); }}
                        title="Capture"
                    >
                        📷
                    </button>
                )}
            </div>
        </div>
    );
}
