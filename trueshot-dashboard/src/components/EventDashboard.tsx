/**
 * Event Dashboard - Organizer Control Panel for Guest Portal
 * 
 * Allows event organizers to:
 * - Create and configure events
 * - View connected guests and their recording status
 * - Trigger synchronized recording across all guests
 * - Monitor upload progress
 * - Download/process collected videos for 4DGS
 */

import { useState, useEffect } from 'react';
import {
    Users, Video, Play, Square, Download, QrCode, Mail,
    HardDrive, Wifi, Check, X, Loader2, Plus,
    Copy, Zap, Camera
} from 'lucide-react';
import toast from 'react-hot-toast';
import { ThemeToggleFloating } from './ThemeToggleFloating';

// ============================================================================
// Types
// ============================================================================

interface Event {
    id: string;
    name: string;
    description: string;
    createdAt: string;
    status: 'draft' | 'active' | 'completed';
    config: EventConfig;
    stats: EventStats;
}

interface EventConfig {
    collectEmail: boolean;
    allowLocalSave: boolean;
    maxRecordingDuration: number;
    preferredQuality: '720p' | '1080p' | '4K';
    syncEnabled: boolean;
}

interface EventStats {
    totalGuests: number;
    activeGuests: number;
    recordingGuests: number;
    totalRecordings: number;
    totalDataSize: number;
    emailsCollected: number;
}

interface GuestInfo {
    id: string;
    deviceInfo: string;
    connectedAt: string;
    email?: string;
    isRecording: boolean;
    uploadProgress?: number;
    recordingDuration: number;
}

interface Recording {
    id: string;
    guestId: string;
    startedAt: string;
    duration: number;
    fileSize: number;
    uploadComplete: boolean;
    quality: string;
}

const MOCK_EVENTS: Event[] = [
    {
        id: 'evt-001',
        name: "Sarah & Mike's Wedding",
        description: 'Ceremony and reception at Rosewood Gardens',
        createdAt: '2026-02-01T12:00:00Z',
        status: 'active',
        config: {
            collectEmail: true,
            allowLocalSave: true,
            maxRecordingDuration: 600,
            preferredQuality: '1080p',
            syncEnabled: true,
        },
        stats: {
            totalGuests: 47,
            activeGuests: 32,
            recordingGuests: 18,
            totalRecordings: 12,
            totalDataSize: 4.2 * 1024 * 1024 * 1024, // 4.2GB
            emailsCollected: 89,
        },
    },
    {
        id: 'evt-002',
        name: 'Tech Conference 2026',
        description: 'Annual developer conference main stage',
        createdAt: '2026-01-15T12:00:00Z',
        status: 'completed',
        config: {
            collectEmail: true,
            allowLocalSave: false,
            maxRecordingDuration: 1800,
            preferredQuality: '4K',
            syncEnabled: true,
        },
        stats: {
            totalGuests: 234,
            activeGuests: 0,
            recordingGuests: 0,
            totalRecordings: 156,
            totalDataSize: 45.6 * 1024 * 1024 * 1024,
            emailsCollected: 312,
        },
    },
];

// ============================================================================
// Event Dashboard Component
// ============================================================================

export default function EventDashboard() {
    // State
    const [events, setEvents] = useState<Event[]>(() => MOCK_EVENTS);
    const [selectedEvent, setSelectedEvent] = useState<Event | null>(null);
    const [guests, setGuests] = useState<GuestInfo[]>([]);
    const [recordings, setRecordings] = useState<Recording[]>([]);
    const [loading] = useState(false);
    const [showCreateModal, setShowCreateModal] = useState(false);
    const [showQRModal, setShowQRModal] = useState(false);

    // Create event form
    const [newEventName, setNewEventName] = useState('');
    const [newEventDesc, setNewEventDesc] = useState('');

    // Mock data for demo
    useEffect(() => {
        if (selectedEvent) {
            // Simulate guest updates
            const interval = setInterval(() => {
                setGuests(prev => prev.map(g => ({
                    ...g,
                    recordingDuration: g.isRecording ? g.recordingDuration + 1 : g.recordingDuration,
                    uploadProgress: g.uploadProgress ? Math.min(100, g.uploadProgress + Math.random() * 5) : undefined,
                })));
            }, 1000);
            return () => clearInterval(interval);
        }
    }, [selectedEvent]);

    const selectEvent = (event: Event) => {
        setSelectedEvent(event);

        // Load mock guests
        const mockGuests: GuestInfo[] = Array.from({ length: event.stats.activeGuests }, (_, i) => ({
            id: `guest-${i}`,
            deviceInfo: ['iPhone 15', 'Pixel 8', 'Galaxy S24', 'iPhone 14'][i % 4],
            connectedAt: new Date(Date.now() - Math.random() * 3600000).toISOString(),
            email: Math.random() > 0.3 ? `guest${i}@email.com` : undefined,
            isRecording: i < event.stats.recordingGuests,
            recordingDuration: Math.floor(Math.random() * 180),
            uploadProgress: undefined,
        }));
        setGuests(mockGuests);

        // Load mock recordings
        const mockRecordings: Recording[] = Array.from({ length: event.stats.totalRecordings }, (_, i) => ({
            id: `rec-${i}`,
            guestId: `guest-${i % event.stats.activeGuests}`,
            startedAt: new Date(Date.now() - Math.random() * 3600000).toISOString(),
            duration: Math.floor(30 + Math.random() * 300),
            fileSize: Math.floor(50 + Math.random() * 500) * 1024 * 1024,
            uploadComplete: Math.random() > 0.2,
            quality: ['720p', '1080p', '4K'][Math.floor(Math.random() * 3)],
        }));
        setRecordings(mockRecordings);
    };

    const createEvent = async () => {
        if (!newEventName.trim()) {
            toast.error('Please enter an event name');
            return;
        }

        const newEvent: Event = {
            id: `evt-${Date.now()}`,
            name: newEventName,
            description: newEventDesc,
            createdAt: new Date().toISOString(),
            status: 'draft',
            config: {
                collectEmail: true,
                allowLocalSave: true,
                maxRecordingDuration: 600,
                preferredQuality: '1080p',
                syncEnabled: true,
            },
            stats: {
                totalGuests: 0,
                activeGuests: 0,
                recordingGuests: 0,
                totalRecordings: 0,
                totalDataSize: 0,
                emailsCollected: 0,
            },
        };

        setEvents(prev => [newEvent, ...prev]);
        setShowCreateModal(false);
        setNewEventName('');
        setNewEventDesc('');
        toast.success('Event created!');
    };

    const triggerAllRecording = (start: boolean) => {
        if (!selectedEvent) return;

        // In production, send WebSocket message to all guests
        setGuests(prev => prev.map(g => ({
            ...g,
            isRecording: start,
            recordingDuration: start ? 0 : g.recordingDuration,
        })));

        toast.success(start ? 'All guests started recording!' : 'All guests stopped recording');
    };

    const copyEventLink = () => {
        if (!selectedEvent) return;
        const link = `${window.location.origin}/guest/${selectedEvent.id}`;
        navigator.clipboard.writeText(link);
        toast.success('Link copied to clipboard!');
    };

    const formatBytes = (bytes: number): string => {
        if (bytes < 1024) return `${bytes} B`;
        if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
        if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
        return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
    };

    const formatDuration = (seconds: number): string => {
        const mins = Math.floor(seconds / 60);
        const secs = seconds % 60;
        return `${mins}:${secs.toString().padStart(2, '0')}`;
    };

    // ========================================================================
    // Render
    // ========================================================================

    if (loading) {
        return (
            <div className="event-dashboard event-dashboard--loading">
                <ThemeToggleFloating />
                <Loader2 className="spin" size={48} />
                <p>Loading events...</p>
            </div>
        );
    }

    return (
        <div className="event-dashboard">
            <ThemeToggleFloating />
            {/* Sidebar - Event List */}
            <aside className="event-dashboard__sidebar">
                <div className="event-dashboard__sidebar-header">
                    <h2>
                        <Video size={20} />
                        Events
                    </h2>
                    <button onClick={() => setShowCreateModal(true)}>
                        <Plus size={18} />
                    </button>
                </div>

                <div className="event-dashboard__event-list">
                    {events.map(event => (
                        <div
                            key={event.id}
                            className={`event-dashboard__event-item ${selectedEvent?.id === event.id ? 'selected' : ''}`}
                            onClick={() => selectEvent(event)}
                        >
                            <div className="event-dashboard__event-name">{event.name}</div>
                            <div className="event-dashboard__event-meta">
                                <span className={`event-dashboard__status event-dashboard__status--${event.status}`}>
                                    {event.status}
                                </span>
                                <span>
                                    <Users size={12} /> {event.stats.activeGuests}
                                </span>
                            </div>
                        </div>
                    ))}
                </div>
            </aside>

            {/* Main Content */}
            <main className="event-dashboard__main">
                {selectedEvent ? (
                    <>
                        {/* Event Header */}
                        <header className="event-dashboard__header">
                            <div className="event-dashboard__header-info">
                                <h1>{selectedEvent.name}</h1>
                                <p>{selectedEvent.description}</p>
                            </div>
                            <div className="event-dashboard__header-actions">
                                <button onClick={() => setShowQRModal(true)}>
                                    <QrCode size={18} /> QR Code
                                </button>
                                <button onClick={copyEventLink}>
                                    <Copy size={18} /> Copy Link
                                </button>
                            </div>
                        </header>

                        {/* Stats Grid */}
                        <div className="event-dashboard__stats">
                            <div className="event-dashboard__stat">
                                <div className="event-dashboard__stat-value">{selectedEvent.stats.activeGuests}</div>
                                <div className="event-dashboard__stat-label">
                                    <Wifi size={14} /> Connected
                                </div>
                            </div>
                            <div className="event-dashboard__stat event-dashboard__stat--recording">
                                <div className="event-dashboard__stat-value">{selectedEvent.stats.recordingGuests}</div>
                                <div className="event-dashboard__stat-label">
                                    <Video size={14} /> Recording
                                </div>
                            </div>
                            <div className="event-dashboard__stat">
                                <div className="event-dashboard__stat-value">{selectedEvent.stats.totalRecordings}</div>
                                <div className="event-dashboard__stat-label">
                                    <Camera size={14} /> Recordings
                                </div>
                            </div>
                            <div className="event-dashboard__stat">
                                <div className="event-dashboard__stat-value">{formatBytes(selectedEvent.stats.totalDataSize)}</div>
                                <div className="event-dashboard__stat-label">
                                    <HardDrive size={14} /> Total Data
                                </div>
                            </div>
                            <div className="event-dashboard__stat">
                                <div className="event-dashboard__stat-value">{selectedEvent.stats.emailsCollected}</div>
                                <div className="event-dashboard__stat-label">
                                    <Mail size={14} /> Emails
                                </div>
                            </div>
                        </div>

                        {/* Master Controls */}
                        <div className="event-dashboard__controls">
                            <button
                                className="event-dashboard__control-btn event-dashboard__control-btn--start"
                                onClick={() => triggerAllRecording(true)}
                            >
                                <Play size={20} /> Start All Recording
                            </button>
                            <button
                                className="event-dashboard__control-btn event-dashboard__control-btn--stop"
                                onClick={() => triggerAllRecording(false)}
                            >
                                <Square size={20} /> Stop All
                            </button>
                            <button
                                className="event-dashboard__control-btn event-dashboard__control-btn--process"
                            >
                                <Zap size={20} /> Process 4DGS
                            </button>
                        </div>

                        {/* Guest List */}
                        <section className="event-dashboard__section">
                            <h3>
                                <Users size={18} />
                                Connected Guests ({guests.length})
                            </h3>
                            <div className="event-dashboard__guest-list">
                                <table>
                                    <thead>
                                        <tr>
                                            <th>Device</th>
                                            <th>Status</th>
                                            <th>Duration</th>
                                            <th>Email</th>
                                            <th>Upload</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {guests.slice(0, 10).map(guest => (
                                            <tr key={guest.id} className={guest.isRecording ? 'recording' : ''}>
                                                <td>{guest.deviceInfo}</td>
                                                <td>
                                                    {guest.isRecording ? (
                                                        <span className="recording-badge">
                                                            <span className="rec-dot"></span>
                                                            Recording
                                                        </span>
                                                    ) : (
                                                        <span className="connected-badge">Connected</span>
                                                    )}
                                                </td>
                                                <td>{formatDuration(guest.recordingDuration)}</td>
                                                <td>{guest.email || '—'}</td>
                                                <td>
                                                    {guest.uploadProgress !== undefined ? (
                                                        <div className="upload-bar">
                                                            <div
                                                                className="upload-fill"
                                                                style={{ width: `${guest.uploadProgress}%` }}
                                                            />
                                                            <span>{Math.round(guest.uploadProgress)}%</span>
                                                        </div>
                                                    ) : '—'}
                                                </td>
                                            </tr>
                                        ))}
                                    </tbody>
                                </table>
                                {guests.length > 10 && (
                                    <div className="event-dashboard__more">
                                        + {guests.length - 10} more guests
                                    </div>
                                )}
                            </div>
                        </section>

                        {/* Recordings */}
                        <section className="event-dashboard__section">
                            <h3>
                                <Video size={18} />
                                Recordings ({recordings.length})
                            </h3>
                            <div className="event-dashboard__recordings">
                                {recordings.slice(0, 5).map(rec => (
                                    <div key={rec.id} className="event-dashboard__recording">
                                        <div className="event-dashboard__recording-icon">
                                            {rec.uploadComplete ? (
                                                <Check size={16} className="text-green" />
                                            ) : (
                                                <Loader2 size={16} className="spin" />
                                            )}
                                        </div>
                                        <div className="event-dashboard__recording-info">
                                            <div>Recording from {rec.guestId}</div>
                                            <div className="meta">
                                                {formatDuration(rec.duration)} • {rec.quality} • {formatBytes(rec.fileSize)}
                                            </div>
                                        </div>
                                        <button className="event-dashboard__recording-download">
                                            <Download size={16} />
                                        </button>
                                    </div>
                                ))}
                            </div>
                        </section>
                    </>
                ) : (
                    <div className="event-dashboard__empty">
                        <Video size={64} />
                        <h2>Select an Event</h2>
                        <p>Choose an event from the sidebar or create a new one</p>
                        <button onClick={() => setShowCreateModal(true)}>
                            <Plus size={18} /> Create Event
                        </button>
                    </div>
                )}
            </main>

            {/* Create Event Modal */}
            {showCreateModal && (
                <div className="event-dashboard__modal-overlay" onClick={() => setShowCreateModal(false)}>
                    <div className="event-dashboard__modal" onClick={e => e.stopPropagation()}>
                        <div className="event-dashboard__modal-header">
                            <h2>Create New Event</h2>
                            <button onClick={() => setShowCreateModal(false)}>
                                <X size={20} />
                            </button>
                        </div>
                        <div className="event-dashboard__modal-body">
                            <label>
                                Event Name
                                <input
                                    type="text"
                                    placeholder="e.g., Sarah & Mike's Wedding"
                                    value={newEventName}
                                    onChange={e => setNewEventName(e.target.value)}
                                />
                            </label>
                            <label>
                                Description
                                <textarea
                                    placeholder="Brief description of the event..."
                                    value={newEventDesc}
                                    onChange={e => setNewEventDesc(e.target.value)}
                                />
                            </label>
                        </div>
                        <div className="event-dashboard__modal-footer">
                            <button
                                className="btn-secondary"
                                onClick={() => setShowCreateModal(false)}
                            >
                                Cancel
                            </button>
                            <button
                                className="btn-primary"
                                onClick={createEvent}
                            >
                                <Plus size={16} /> Create Event
                            </button>
                        </div>
                    </div>
                </div>
            )}

            {/* QR Code Modal */}
            {showQRModal && selectedEvent && (
                <div className="event-dashboard__modal-overlay" onClick={() => setShowQRModal(false)}>
                    <div className="event-dashboard__modal event-dashboard__modal--qr" onClick={e => e.stopPropagation()}>
                        <div className="event-dashboard__modal-header">
                            <h2>Guest Portal QR Code</h2>
                            <button onClick={() => setShowQRModal(false)}>
                                <X size={20} />
                            </button>
                        </div>
                        <div className="event-dashboard__modal-body event-dashboard__qr-body">
                            <div className="event-dashboard__qr-placeholder">
                                {/* In production, generate real QR code */}
                                <QrCode size={200} />
                            </div>
                            <p className="event-dashboard__qr-link">
                                {window.location.origin}/guest/{selectedEvent.id}
                            </p>
                            <button onClick={copyEventLink}>
                                <Copy size={16} /> Copy Link
                            </button>
                        </div>
                    </div>
                </div>
            )}
        </div>
    );
}
