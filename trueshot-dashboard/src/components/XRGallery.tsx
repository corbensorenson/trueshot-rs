/**
 * XR Scan Gallery Component
 * 
 * Browse and interact with previous scans in an immersive VR/AR environment.
 * Features:
 * - Place scans in current environment
 * - Scale and position adjustment  
 * - Side-by-side comparison
 * - Merge multiple scans
 */

import { useState, useCallback } from 'react';
import {
    Eye,
    Move,
    Merge,
    Trash2,
    Download,
    Grid3x3,
    List,
    Search,
    X,
    Check,
} from 'lucide-react';

interface Scan {
    id: string;
    name: string;
    createdAt: Date;
    thumbnailUrl: string;
    type: '3dgs' | 'mesh' | 'pointcloud';
    fileSize: number;
    dimensions: { width: number; height: number; depth: number };
}

interface PlacedScan {
    scanId: string;
    position: { x: number; y: number; z: number };
    rotation: { x: number; y: number; z: number };
    scale: number;
}

interface XRGalleryProps {
    scans: Scan[];
    isInXR: boolean;
    onPlaceScan: (scan: Scan, position: { x: number; y: number; z: number }) => void;
    onMergeScans: (scanIds: string[]) => void;
    onDeleteScan: (scanId: string) => void;
    onExportScan: (scanId: string) => void;
}

export function XRGallery({
    scans,
    isInXR,
    onPlaceScan,
    onMergeScans,
    onDeleteScan,
    onExportScan,
}: XRGalleryProps) {
    const [viewMode, setViewMode] = useState<'grid' | 'list'>('grid');
    const [searchQuery, setSearchQuery] = useState('');
    const [selectedScans, setSelectedScans] = useState<string[]>([]);
    const [placedScans, setPlacedScans] = useState<PlacedScan[]>([]);
    const [editingScan, setEditingScan] = useState<string | null>(null);

    // Filter scans by search
    const filteredScans = scans.filter(scan =>
        scan.name.toLowerCase().includes(searchQuery.toLowerCase())
    );

    // Toggle scan selection
    const toggleSelect = useCallback((scanId: string) => {
        setSelectedScans(prev =>
            prev.includes(scanId)
                ? prev.filter(id => id !== scanId)
                : [...prev, scanId]
        );
    }, []);

    // Place scan in XR environment
    const handlePlace = useCallback((scan: Scan) => {
        const placed: PlacedScan = {
            scanId: scan.id,
            position: { x: 0, y: 0, z: -2 }, // 2m in front
            rotation: { x: 0, y: 0, z: 0 },
            scale: 1.0,
        };
        setPlacedScans(prev => [...prev, placed]);
        onPlaceScan(scan, placed.position);
    }, [onPlaceScan]);

    // Remove placed scan
    const handleRemovePlaced = useCallback((scanId: string) => {
        setPlacedScans(prev => prev.filter(p => p.scanId !== scanId));
    }, []);

    // Adjust scale
    const handleScaleChange = useCallback((scanId: string, delta: number) => {
        setPlacedScans(prev =>
            prev.map(p =>
                p.scanId === scanId
                    ? { ...p, scale: Math.max(0.1, Math.min(10, p.scale + delta)) }
                    : p
            )
        );
    }, []);

    // Handle merge
    const handleMerge = useCallback(() => {
        if (selectedScans.length >= 2) {
            onMergeScans(selectedScans);
            setSelectedScans([]);
        }
    }, [selectedScans, onMergeScans]);

    return (
        <div className="flex flex-col h-full bg-gray-900">
            {/* Header */}
            <div className="p-4 border-b border-gray-800">
                <div className="flex items-center justify-between mb-4">
                    <h2 className="text-white text-lg font-medium">Scan Gallery</h2>
                    <div className="flex items-center gap-2">
                        <button
                            onClick={() => setViewMode('grid')}
                            className={`p-2 rounded ${viewMode === 'grid' ? 'bg-blue-500' : 'bg-gray-800'} text-white`}
                        >
                            <Grid3x3 className="w-4 h-4" />
                        </button>
                        <button
                            onClick={() => setViewMode('list')}
                            className={`p-2 rounded ${viewMode === 'list' ? 'bg-blue-500' : 'bg-gray-800'} text-white`}
                        >
                            <List className="w-4 h-4" />
                        </button>
                    </div>
                </div>

                {/* Search */}
                <div className="relative">
                    <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-500" />
                    <input
                        type="text"
                        value={searchQuery}
                        onChange={e => setSearchQuery(e.target.value)}
                        placeholder="Search scans..."
                        className="w-full pl-10 pr-4 py-2 bg-gray-800 rounded-lg text-white placeholder-gray-500 text-sm"
                    />
                </div>
            </div>

            {/* Selection actions */}
            {selectedScans.length > 0 && (
                <div className="p-3 bg-blue-500/20 border-b border-blue-500/50 flex items-center justify-between">
                    <span className="text-blue-400 text-sm">
                        {selectedScans.length} scan{selectedScans.length > 1 ? 's' : ''} selected
                    </span>
                    <div className="flex gap-2">
                        {selectedScans.length >= 2 && (
                            <button
                                onClick={handleMerge}
                                className="flex items-center gap-1 px-3 py-1 bg-blue-500 rounded text-white text-sm"
                            >
                                <Merge className="w-3 h-3" />
                                Merge
                            </button>
                        )}
                        <button
                            onClick={() => setSelectedScans([])}
                            className="px-3 py-1 bg-gray-700 rounded text-white text-sm"
                        >
                            Clear
                        </button>
                    </div>
                </div>
            )}

            {/* Scan grid/list */}
            <div className="flex-1 overflow-auto p-4">
                {viewMode === 'grid' ? (
                    <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-4">
                        {filteredScans.map(scan => (
                            <ScanCard
                                key={scan.id}
                                scan={scan}
                                isSelected={selectedScans.includes(scan.id)}
                                isPlaced={placedScans.some(p => p.scanId === scan.id)}
                                isInXR={isInXR}
                                onSelect={() => toggleSelect(scan.id)}
                                onPlace={() => handlePlace(scan)}
                                onView={() => setEditingScan(scan.id)}
                                onDelete={() => onDeleteScan(scan.id)}
                                onExport={() => onExportScan(scan.id)}
                            />
                        ))}
                    </div>
                ) : (
                    <div className="space-y-2">
                        {filteredScans.map(scan => (
                            <ScanListItem
                                key={scan.id}
                                scan={scan}
                                isSelected={selectedScans.includes(scan.id)}
                                isPlaced={placedScans.some(p => p.scanId === scan.id)}
                                isInXR={isInXR}
                                onSelect={() => toggleSelect(scan.id)}
                                onPlace={() => handlePlace(scan)}
                                onDelete={() => onDeleteScan(scan.id)}
                                onExport={() => onExportScan(scan.id)}
                            />
                        ))}
                    </div>
                )}
            </div>

            {/* Placed scans panel (when in XR) */}
            {isInXR && placedScans.length > 0 && (
                <div className="p-4 border-t border-gray-800 bg-gray-900/80 backdrop-blur">
                    <h3 className="text-white text-sm font-medium mb-3">Placed in Scene</h3>
                    <div className="flex gap-2 overflow-x-auto pb-2">
                        {placedScans.map(placed => {
                            const scan = scans.find(s => s.id === placed.scanId);
                            if (!scan) return null;
                            return (
                                <PlacedScanChip
                                    key={placed.scanId}
                                    scan={scan}
                                    placed={placed}
                                    isEditing={editingScan === placed.scanId}
                                    onEdit={() => setEditingScan(placed.scanId)}
                                    onRemove={() => handleRemovePlaced(placed.scanId)}
                                    onScaleUp={() => handleScaleChange(placed.scanId, 0.1)}
                                    onScaleDown={() => handleScaleChange(placed.scanId, -0.1)}
                                />
                            );
                        })}
                    </div>
                </div>
            )}
        </div>
    );
}

// ============================================================================
// Sub-components
// ============================================================================

function ScanCard({
    scan,
    isSelected,
    isPlaced,
    isInXR,
    onSelect,
    onPlace,
    onView,
    onDelete,
    onExport,
}: {
    scan: Scan;
    isSelected: boolean;
    isPlaced: boolean;
    isInXR: boolean;
    onSelect: () => void;
    onPlace: () => void;
    onView: () => void;
    onDelete: () => void;
    onExport: () => void;
}) {
    return (
        <div
            className={`
        relative rounded-xl overflow-hidden bg-gray-800 group cursor-pointer
        ${isSelected ? 'ring-2 ring-blue-500' : ''}
        ${isPlaced ? 'ring-2 ring-green-500' : ''}
      `}
            onClick={onSelect}
        >
            {/* Thumbnail */}
            <div className="aspect-square bg-gray-700">
                <img
                    src={scan.thumbnailUrl}
                    alt={scan.name}
                    className="w-full h-full object-cover"
                />
            </div>

            {/* Type badge */}
            <div className="absolute top-2 right-2">
                <span className={`
          px-2 py-0.5 rounded text-xs font-medium
          ${scan.type === '3dgs' ? 'bg-purple-500' : ''}
          ${scan.type === 'mesh' ? 'bg-blue-500' : ''}
          ${scan.type === 'pointcloud' ? 'bg-green-500' : ''}
        `}>
                    {scan.type.toUpperCase()}
                </span>
            </div>

            {/* Selection indicator */}
            {isSelected && (
                <div className="absolute top-2 left-2 w-6 h-6 bg-blue-500 rounded-full flex items-center justify-center">
                    <Check className="w-4 h-4 text-white" />
                </div>
            )}

            {/* Info */}
            <div className="p-3">
                <h4 className="text-white text-sm font-medium truncate">{scan.name}</h4>
                <p className="text-gray-500 text-xs">
                    {new Date(scan.createdAt).toLocaleDateString()}
                </p>
            </div>

            {/* Hover actions */}
            <div className="absolute inset-0 bg-black/60 opacity-0 group-hover:opacity-100 transition-opacity flex items-center justify-center gap-2">
                <button
                    onClick={e => { e.stopPropagation(); onView(); }}
                    className="p-2 bg-white/20 rounded-lg hover:bg-white/30 text-white"
                >
                    <Eye className="w-4 h-4" />
                </button>
                {isInXR && (
                    <button
                        onClick={e => { e.stopPropagation(); onPlace(); }}
                        className="p-2 bg-green-500/80 rounded-lg hover:bg-green-500 text-white"
                    >
                        <Move className="w-4 h-4" />
                    </button>
                )}
                <button
                    onClick={e => { e.stopPropagation(); onExport(); }}
                    className="p-2 bg-white/20 rounded-lg hover:bg-white/30 text-white"
                >
                    <Download className="w-4 h-4" />
                </button>
                <button
                    onClick={e => { e.stopPropagation(); onDelete(); }}
                    className="p-2 bg-red-500/80 rounded-lg hover:bg-red-500 text-white"
                >
                    <Trash2 className="w-4 h-4" />
                </button>
            </div>
        </div>
    );
}

function ScanListItem({
    scan,
    isSelected,
    isPlaced,
    isInXR,
    onSelect,
    onPlace,
    onDelete,
    onExport,
}: {
    scan: Scan;
    isSelected: boolean;
    isPlaced: boolean;
    isInXR: boolean;
    onSelect: () => void;
    onPlace: () => void;
    onDelete: () => void;
    onExport: () => void;
}) {
    const formatSize = (bytes: number) => {
        if (bytes < 1024) return `${bytes} B`;
        if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
        return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    };

    return (
        <div
            className={`
        flex items-center gap-3 p-3 rounded-lg bg-gray-800 cursor-pointer
        ${isSelected ? 'ring-2 ring-blue-500' : ''}
        ${isPlaced ? 'ring-2 ring-green-500' : ''}
      `}
            onClick={onSelect}
        >
            {/* Thumbnail */}
            <div className="w-12 h-12 rounded bg-gray-700 overflow-hidden flex-shrink-0">
                <img src={scan.thumbnailUrl} alt="" className="w-full h-full object-cover" />
            </div>

            {/* Info */}
            <div className="flex-1 min-w-0">
                <h4 className="text-white text-sm font-medium truncate">{scan.name}</h4>
                <p className="text-gray-500 text-xs">
                    {formatSize(scan.fileSize)} • {scan.type.toUpperCase()}
                </p>
            </div>

            {/* Actions */}
            <div className="flex gap-1">
                {isInXR && (
                    <button
                        onClick={e => { e.stopPropagation(); onPlace(); }}
                        className="p-1.5 bg-green-500/20 rounded hover:bg-green-500/40 text-green-400"
                    >
                        <Move className="w-4 h-4" />
                    </button>
                )}
                <button
                    onClick={e => { e.stopPropagation(); onExport(); }}
                    className="p-1.5 bg-gray-700 rounded hover:bg-gray-600 text-gray-400"
                >
                    <Download className="w-4 h-4" />
                </button>
                <button
                    onClick={e => { e.stopPropagation(); onDelete(); }}
                    className="p-1.5 bg-red-500/20 rounded hover:bg-red-500/40 text-red-400"
                >
                    <Trash2 className="w-4 h-4" />
                </button>
            </div>
        </div>
    );
}

function PlacedScanChip({
    scan,
    placed,
    isEditing,
    onEdit,
    onRemove,
    onScaleUp,
    onScaleDown,
}: {
    scan: Scan;
    placed: PlacedScan;
    isEditing: boolean;
    onEdit: () => void;
    onRemove: () => void;
    onScaleUp: () => void;
    onScaleDown: () => void;
}) {
    return (
        <div className={`
      flex items-center gap-2 px-3 py-2 rounded-lg flex-shrink-0
      ${isEditing ? 'bg-blue-500/30 ring-1 ring-blue-500' : 'bg-gray-800'}
    `}>
            <div className="w-8 h-8 rounded bg-gray-700 overflow-hidden">
                <img src={scan.thumbnailUrl} alt="" className="w-full h-full object-cover" />
            </div>
            <span className="text-white text-sm">{scan.name}</span>

            {/* Scale controls */}
            <div className="flex items-center gap-1 ml-2">
                <button
                    onClick={onScaleDown}
                    className="w-6 h-6 flex items-center justify-center bg-gray-700 rounded text-gray-400 hover:text-white"
                >
                    -
                </button>
                <span className="text-gray-400 text-xs w-12 text-center">
                    {(placed.scale * 100).toFixed(0)}%
                </span>
                <button
                    onClick={onScaleUp}
                    className="w-6 h-6 flex items-center justify-center bg-gray-700 rounded text-gray-400 hover:text-white"
                >
                    +
                </button>
            </div>

            <button
                onClick={onEdit}
                className="p-1 text-gray-400 hover:text-white"
            >
                <Move className="w-4 h-4" />
            </button>

            <button
                onClick={onRemove}
                className="p-1 text-red-400 hover:text-red-300"
            >
                <X className="w-4 h-4" />
            </button>
        </div>
    );
}

export default XRGallery;
