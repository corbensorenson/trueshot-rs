/**
 * Device Manager - Modular Component Library
 * 
 * Barrel export for all DeviceManager components and hooks
 */

// Types
export * from './types';

// Hooks
export { useDevices } from './hooks/useDevices';
export { useDeviceActions } from './hooks/useDeviceActions';

// Components
export { DeviceCard } from './DeviceCard';

// Re-export the full manager for backward compatibility
// (Will be created after we refactor the main file)
