// LiveMonitor is now inside Sidebar
import { PixelCollapseFeed } from './components/PixelCollapseFeed';
import { UnifiedViewer } from './components/UnifiedViewer';
import { Toaster } from 'react-hot-toast';
import { BootSequence } from './components/BootSequence';
import { CalibrationWizard } from './components/CalibrationWizard';
import { ProjectLibrary } from './components/ProjectLibrary';
import { ScanWizard } from './components/ScanWizard';
import { OnboardingTour } from './components/OnboardingTour';
import { ARPreview } from './components/ARPreview';
import DeviceManagerPro from './components/DeviceManagerPro';
import { ErrorBoundary } from './components/ErrorBoundary';
import { useState, useEffect, useMemo, useRef } from 'react';
import { clearAuthToken, establishSession } from './api/client';
import toast from 'react-hot-toast';

import { CameraModal } from './components/CameraModal';
import { CameraProfile } from './api/client';
import { Header } from './components/Header';
import { Footer } from './components/Footer';
import { Sidebar } from './components/Sidebar';
import { AuthGate } from './components/AuthGate';
import { SecurityConsole } from './components/SecurityConsole';
import { LicenseConsole } from './components/LicenseConsole';
import { useTheme } from './hooks/useTheme';
import { ShareViewer } from './components/ShareViewer';
import { PublicGallery } from './components/PublicGallery';
import { AvatarCapture } from './components/AvatarCapture';
import { SceneReconstruction } from './components/SceneReconstruction';
import { XRScanner } from './components/XRScanner';


function App() {
  const [booted, setBooted] = useState(false);
  const [calibrating, setCalibrating] = useState(false);
  const [libraryOpen, setLibraryOpen] = useState(false);
  const [deviceManagerOpen, setDeviceManagerOpen] = useState(false);
  const [runTour, setRunTour] = useState(() => {
    const visited = localStorage.getItem('trueshot_v6_visited');
    if (!visited) {
      localStorage.setItem('trueshot_v6_visited', 'true');
      return true;
    }
    return false;
  });
  const [arOpen, setArOpen] = useState(false);
  const [holographicMode, setHolographicMode] = useState(false);

  const [sequenceOpen, setSequenceOpen] = useState(false);
  const [consoleOpen, setConsoleOpen] = useState(false);
  const [selectedCam, setSelectedCam] = useState<CameraProfile | null>(null);
  const [authReady, setAuthReady] = useState(false);
  const [securityOpen, setSecurityOpen] = useState(false);
  const [licenseOpen, setLicenseOpen] = useState(false);
  const [avatarOpen, setAvatarOpen] = useState(false);
  const [sceneOpen, setSceneOpen] = useState(false);
  const [xrOpen, setXrOpen] = useState(false);
  const { theme, toggleTheme } = useTheme();
  const shareToken = useMemo(() => {
    const path = window.location.pathname || '';
    if (path.startsWith('/share/')) {
      return path.replace('/share/', '');
    }
    return null;
  }, []);
  const galleryMode = useMemo(() => {
    const path = window.location.pathname || '';
    return path === '/gallery' || path.startsWith('/gallery/');
  }, []);

  // Establish session cookie if token already exists
  useEffect(() => {
    if (shareToken) return;
    establishSession().then((token) => {
      if (token) {
        setAuthReady(true);
        return;
      }
      clearAuthToken();
      setAuthReady(false);
    }).catch(() => {
      toast.error('Auth session failed. Check server logs for details.');
      clearAuthToken();
      setAuthReady(false);
    });
  }, []);

  // Keyboard Shortcuts
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === '`') setConsoleOpen(prev => !prev);
      if (e.key === 'Escape') {
        setLibraryOpen(false);
        setDeviceManagerOpen(false);
        setSequenceOpen(false);
        setArOpen(false);
        setSelectedCam(null);
        setLicenseOpen(false);
        setAvatarOpen(false);
        setSceneOpen(false);
        setXrOpen(false);
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, []);

  // Drag State for Terminal
  const [terminalPos, setTerminalPos] = useState({ x: 0, y: 0 });
  const isDragging = useRef(false);
  const dragOffset = useRef({ x: 0, y: 0 });

  const handleDragStart = (e: React.MouseEvent) => {
    isDragging.current = true;
    dragOffset.current = {
      x: e.clientX - terminalPos.x,
      y: e.clientY - terminalPos.y
    };
  };

  const handleDrag = (e: React.MouseEvent) => {
    if (!isDragging.current) return;
    setTerminalPos({
      x: e.clientX - dragOffset.current.x,
      y: e.clientY - dragOffset.current.y
    });
  };

  const handleDragEnd = () => {
    isDragging.current = false;
  };

  if (shareToken) {
    return <ShareViewer token={shareToken} />;
  }
  if (galleryMode) {
    return <PublicGallery />;
  }

  return (
    <div className="min-h-screen app-shell overflow-hidden relative" onMouseMove={handleDrag} onMouseUp={handleDragEnd}>
      <div className="absolute inset-0 z-0">
        <UnifiedViewer holographicMode={holographicMode} url="/assets/demo.ply" />
        <div className="absolute inset-0 app-vignette pointer-events-none" />
      </div>

      {!authReady && <AuthGate onAuthenticated={() => setAuthReady(true)} />}

      {/* Camera Modal (Top Level) */}
      {selectedCam && <CameraModal camera={selectedCam} onClose={() => setSelectedCam(null)} />}
      <SecurityConsole
        isOpen={securityOpen}
        onClose={() => setSecurityOpen(false)}
        onLoggedOut={() => {
          setSecurityOpen(false);
          setAuthReady(false);
        }}
      />
      <LicenseConsole
        isOpen={licenseOpen}
        onClose={() => setLicenseOpen(false)}
      />

      {/* UI Layer */}
      <div className="relative z-10 flex flex-col h-screen pointer-events-none px-6 pb-6 pt-4 gap-4">
        {!booted && <BootSequence onComplete={() => setBooted(true)} />}

        {/* Modals - Wrapped in ErrorBoundary to prevent crashes */}
        <ErrorBoundary>
          <DeviceManagerPro isOpen={deviceManagerOpen} onClose={() => setDeviceManagerOpen(false)} />
        </ErrorBoundary>
        {avatarOpen && (
          <div className="fixed inset-0 z-[140] pointer-events-auto">
            <AvatarCapture onComplete={() => setAvatarOpen(false)} onCancel={() => setAvatarOpen(false)} />
          </div>
        )}
        {sceneOpen && (
          <div className="fixed inset-0 z-[140] pointer-events-auto">
            <SceneReconstruction onClose={() => setSceneOpen(false)} />
          </div>
        )}
        <XRScanner
          isOpen={xrOpen}
          onClose={() => setXrOpen(false)}
          onScanComplete={() => setXrOpen(false)}
        />
        <ErrorBoundary>
          <ProjectLibrary isOpen={libraryOpen} onClose={() => setLibraryOpen(false)} />
        </ErrorBoundary>
        {calibrating && <CalibrationWizard onClose={() => setCalibrating(false)} />}
        <ARPreview isOpen={arOpen} onClose={() => setArOpen(false)} modelUrl="/assets/demo.ply" />

        {runTour && <OnboardingTour run={runTour} onFinish={() => setRunTour(false)} />}

        <Header
          onOpenLibrary={() => setLibraryOpen(true)}
          onOpenHelp={() => setRunTour(true)}
          onOpenAR={() => setArOpen(true)}
          onOpenDeviceManager={() => setDeviceManagerOpen(true)}
          onOpenSequence={() => setSequenceOpen(true)}
          onOpenSecurity={() => setSecurityOpen(true)}
          onOpenLicense={() => setLicenseOpen(true)}
          booted={booted}
          theme={theme}
          onToggleTheme={toggleTheme}
        />

        {/* Floating Workspace Controls */}
        <div className="flex-1 flex gap-6 min-h-0 relative">

          {/* Left Dock (Sidebar) */}
          <Sidebar
            onSelectCam={setSelectedCam}
            onOpenAvatar={() => setAvatarOpen(true)}
            onOpenScene={() => setSceneOpen(true)}
            onOpenXR={() => setXrOpen(true)}
          />

          {/* Main Area (Transparent, just holds floating elements) */}
          <main className="flex-1 min-h-0 relative pointer-events-none">

            {/* Sequence Wizard (Replaces old Drawer) */}
            <ScanWizard isOpen={sequenceOpen} onClose={() => setSequenceOpen(false)} />

            {/* Console Panel (Draggable Overlay) */}
            <div
              style={{ transform: `translate(${terminalPos.x}px, ${terminalPos.y}px)` }}
              className={`fixed bottom-16 left-6 right-6 h-[300px] ts-panel-strong backdrop-blur-2xl rounded-2xl z-40 transition-opacity duration-300 shadow-2xl flex flex-col pointer-events-auto overflow-hidden ${consoleOpen ? 'opacity-100 scale-100' : 'opacity-0 pointer-events-none scale-95'}`}
            >
              <div
                onMouseDown={handleDragStart}
                className="bg-[color:color-mix(in_srgb,var(--ts-surface)_75%,transparent)] p-3 flex justify-between items-center px-4 border-b border-[color:var(--ts-border)] cursor-move select-none"
              >
                <span className="text-[10px] uppercase font-bold tracking-widest text-[color:var(--ts-muted)] flex items-center gap-2">
                  <div className="w-2 h-2 bg-green-500 rounded-full shadow-[0_0_8px_rgba(34,197,94,0.6)]" />
                  System Terminal
                </span>
                <button onClick={() => setConsoleOpen(false)} className="text-xs text-[color:var(--ts-muted)] hover:text-[color:var(--ts-text)] transition-colors">Close</button>
              </div>
              <div className="flex-1 min-h-0 overflow-hidden bg-[color:color-mix(in_srgb,var(--ts-surface)_60%,transparent)]">
                <PixelCollapseFeed />
              </div>
            </div>

          </main>
        </div>

        <Footer consoleOpen={consoleOpen} setConsoleOpen={setConsoleOpen} holographicMode={holographicMode} setHolographicMode={setHolographicMode} />

        <Toaster
          position="bottom-right"
          toastOptions={{
            style: {
              background: 'var(--ts-panel)',
              backdropFilter: 'blur(16px)',
              border: '1px solid var(--ts-border)',
              color: 'var(--ts-text)',
              fontSize: '12px',
              borderRadius: '12px',
              boxShadow: 'var(--ts-shadow-panel)',
            }
          }}
        />
      </div>
    </div>
  );
}

export default App;
