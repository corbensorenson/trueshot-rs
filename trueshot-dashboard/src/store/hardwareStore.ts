import { create } from 'zustand';
import { CameraProfile, TurntableStatus } from '../api/client';

interface HardwareState {
    cameras: CameraProfile[];
    turntable: TurntableStatus;

    // Actions
    setCameras: (cameras: CameraProfile[]) => void;
    updateTurntable: (status: Partial<TurntableStatus>) => void;
    setTurntableConnected: (connected: boolean) => void;
    setBackendConnected: (connected: boolean) => void;

    // State
    backendConnected: boolean;
}

export const useHardwareStore = create<HardwareState>((set) => ({
    cameras: [],
    turntable: {
        connected: false,
        type: 'Unknown',
        angle: 0,
        moving: false,
    },
    backendConnected: false,

    setCameras: (cameras) => set({ cameras }),
    updateTurntable: (status) => set((state) => ({
        turntable: { ...state.turntable, ...status }
    })),
    setTurntableConnected: (connected) => set((state) => ({
        turntable: { ...state.turntable, connected }
    })),
    setBackendConnected: (connected) => set({ backendConnected: connected }),
}));
