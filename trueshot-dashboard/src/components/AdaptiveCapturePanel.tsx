import { useEffect, useMemo, useState } from 'react';
import {
    AlertTriangle,
    BrainCircuit,
    Check,
    CircleStop,
    Crosshair,
    Gauge,
    Loader2,
    RefreshCw,
    Sparkles,
} from 'lucide-react';
import toast from 'react-hot-toast';
import {
    captureNextAdaptive,
    getAdaptiveCapture,
    getProjects,
    listProjectAssets,
    startAdaptiveCapture,
    terminateAdaptiveCapture,
    type AdaptiveCaptureSession,
    type AdaptiveCaptureStep,
    type ProjectAsset,
    type ProjectSummary,
} from '../api/client';

interface AdaptiveCapturePanelProps {
    cameraId: string;
    locked: boolean;
}

const isNef = (path: string) => /\.nef$/i.test(path);
const isNoiseProfile = (path: string) =>
    /\.json$/i.test(path) && /(noise|sensor|calibrat)/i.test(path);

const focusGrid = (minimum: number, maximum: number, count: number) => {
    const bounded = Math.max(1, Math.min(256, Math.floor(count)));
    if (bounded === 1) return [minimum];
    return Array.from({ length: bounded }, (_, index) =>
        minimum + ((maximum - minimum) * index) / (bounded - 1));
};

const maxOrZero = (values: number[]) => values.reduce((maximum, value) => Math.max(maximum, value), 0);

export function AdaptiveCapturePanel({ cameraId, locked }: AdaptiveCapturePanelProps) {
    const [projects, setProjects] = useState<ProjectSummary[]>([]);
    const [projectId, setProjectId] = useState('');
    const [assets, setAssets] = useState<ProjectAsset[]>([]);
    const [referencePath, setReferencePath] = useState('');
    const [profilePath, setProfilePath] = useState('');
    const [roi, setRoi] = useState({ x: 0, y: 0, width: 1024, height: 1024 });
    const [minimumFocus, setMinimumFocus] = useState(0.25);
    const [maximumFocus, setMaximumFocus] = useState(4);
    const [focusSteps, setFocusSteps] = useState(16);
    const [timeBudgetSeconds, setTimeBudgetSeconds] = useState(30);
    const [readoutMs, setReadoutMs] = useState(22);
    const [settleMs, setSettleMs] = useState(80);
    const [motion, setMotion] = useState(0);
    const [thermal, setThermal] = useState(0);
    const [focusConfirmed, setFocusConfirmed] = useState(false);
    const [session, setSession] = useState<AdaptiveCaptureSession | null>(null);
    const [lastStep, setLastStep] = useState<AdaptiveCaptureStep | null>(null);
    const [busy, setBusy] = useState(false);
    const [loadingAssets, setLoadingAssets] = useState(false);

    const storageKey = `trueshot.adaptive-session.${cameraId}`;
    const rawAssets = useMemo(() => assets.filter(asset => isNef(asset.path)), [assets]);
    const profileAssets = useMemo(
        () => assets.filter(asset => isNoiseProfile(asset.path)),
        [assets],
    );
    const selected = session?.status.decision.selected ?? null;
    const complete = Boolean(session?.status.termination || !selected);
    const radianceWorst = maxOrZero(
        session?.status.posterior.radiance.map(probe => probe.variance) ?? [],
    );
    const focusWorst = Math.sqrt(maxOrZero(
        session?.status.posterior.focus.map(probe => probe.variance_diopters2) ?? [],
    ));

    useEffect(() => {
        let cancelled = false;
        getProjects()
            .then(items => {
                if (cancelled) return;
                const visible = items.filter(item => !item.name.startsWith('_'));
                setProjects(visible);
                setProjectId(current => current || visible[0]?.name || '');
            })
            .catch(error => {
                console.error(error);
                toast.error('Could not load adaptive capture projects');
            });
        const remembered = localStorage.getItem(storageKey);
        if (remembered) {
            getAdaptiveCapture(remembered)
                .then(restored => {
                    if (!cancelled && restored.camera_id === cameraId) {
                        setSession(restored);
                        setProjectId(restored.project_id ?? '');
                    }
                })
                .catch(() => localStorage.removeItem(storageKey));
        }
        return () => {
            cancelled = true;
        };
    }, [cameraId, storageKey]);

    useEffect(() => {
        if (!projectId) {
            setAssets([]);
            return;
        }
        let cancelled = false;
        setLoadingAssets(true);
        Promise.all([
            listProjectAssets(projectId, 'raw'),
            listProjectAssets(projectId, 'processed'),
        ])
            .then(([raw, processed]) => {
                if (cancelled) return;
                const next = [...raw, ...processed];
                setAssets(next);
                setReferencePath(current =>
                    next.some(asset => asset.path === current)
                        ? current
                        : next.find(asset => isNef(asset.path))?.path ?? '');
                setProfilePath(current =>
                    next.some(asset => asset.path === current)
                        ? current
                        : next.find(asset => isNoiseProfile(asset.path))?.path ?? '');
            })
            .catch(error => {
                console.error(error);
                if (!cancelled) toast.error('Could not load project capture assets');
            })
            .finally(() => {
                if (!cancelled) setLoadingAssets(false);
            });
        return () => {
            cancelled = true;
        };
    }, [projectId]);

    const beginSession = async () => {
        if (locked) {
            toast.error('Adaptive capture requires Advanced Capture Automation');
            return;
        }
        if (!projectId || !referencePath || !profilePath) {
            toast.error('Select a project, reference NEF, and calibrated sensor profile');
            return;
        }
        if (minimumFocus < 0 || maximumFocus <= minimumFocus || focusSteps < 1) {
            toast.error('Focus range must increase from minimum to maximum diopters');
            return;
        }
        setBusy(true);
        setLastStep(null);
        try {
            const created = await startAdaptiveCapture({
                camera_id: cameraId,
                project_id: projectId,
                reference_raw_path: referencePath,
                sensor_profile_path: profilePath,
                roi,
                focus_diopters: focusGrid(minimumFocus, maximumFocus, focusSteps),
                readout_ms: readoutMs,
                settle_ms: settleMs,
                planner: { remaining_time_ms: timeBudgetSeconds * 1000 },
            });
            setSession(created);
            setFocusConfirmed(false);
            localStorage.setItem(storageKey, created.session_id);
            toast.success('Measured adaptive session started');
        } catch (error) {
            toast.error(error instanceof Error ? error.message : 'Adaptive session failed');
        } finally {
            setBusy(false);
        }
    };

    const captureNext = async () => {
        if (!session || !selected || !focusConfirmed) return;
        setBusy(true);
        try {
            const step = await captureNextAdaptive(session.session_id, {
                confirmed_focus_diopters: selected.candidate.focus_diopters,
                motion_pixels_per_second: motion,
                thermal_load: thermal,
            });
            setLastStep(step);
            setSession(current => current ? {
                ...current,
                generation: step.generation,
                status: step.status,
            } : current);
            setFocusConfirmed(false);
            toast.success(`Retained measured RAW ${step.capture_path}`);
        } catch (error) {
            toast.error(error instanceof Error ? error.message : 'Adaptive capture failed');
        } finally {
            setBusy(false);
        }
    };

    const stopSession = async () => {
        if (!session) return;
        setBusy(true);
        try {
            const stopped = await terminateAdaptiveCapture(session.session_id, 'operator_stopped');
            setSession(stopped);
            localStorage.removeItem(storageKey);
            toast.success('Adaptive session stopped with provenance retained');
        } catch (error) {
            toast.error(error instanceof Error ? error.message : 'Could not stop adaptive session');
        } finally {
            setBusy(false);
        }
    };

    const clearCompleted = () => {
        localStorage.removeItem(storageKey);
        setSession(null);
        setLastStep(null);
        setFocusConfirmed(false);
    };

    if (!session) {
        return (
            <section className="adaptive-capture">
                <div className="adaptive-capture__hero">
                    <div>
                        <span className="adaptive-capture__eyebrow">
                            <Sparkles size={14} /> MEASURED ACQUISITION
                        </span>
                        <h3>Spend each shutter actuation where it adds evidence.</h3>
                        <p>
                            TrueShot ranks exact shutter, ISO, and physical focus candidates by
                            expected information gain, then validates every retained NEF.
                        </p>
                    </div>
                    <BrainCircuit size={42} />
                </div>
                <div className="adaptive-capture__grid">
                    <label>
                        <span>Project</span>
                        <select value={projectId} onChange={event => setProjectId(event.target.value)}>
                            <option value="">Select a project</option>
                            {projects.map(project => (
                                <option key={project.name} value={project.name}>{project.name}</option>
                            ))}
                        </select>
                    </label>
                    <label>
                        <span>Reference NEF</span>
                        <select
                            value={referencePath}
                            onChange={event => setReferencePath(event.target.value)}
                            disabled={loadingAssets}
                        >
                            <option value="">Select retained RAW</option>
                            {rawAssets.map(asset => (
                                <option key={asset.path} value={asset.path}>{asset.path}</option>
                            ))}
                        </select>
                    </label>
                    <label>
                        <span>Sensor calibration</span>
                        <select
                            value={profilePath}
                            onChange={event => setProfilePath(event.target.value)}
                            disabled={loadingAssets}
                        >
                            <option value="">Select exact-ISO profile</option>
                            {profileAssets.map(asset => (
                                <option key={asset.path} value={asset.path}>{asset.path}</option>
                            ))}
                        </select>
                    </label>
                    <label>
                        <span>Time budget</span>
                        <div className="adaptive-capture__unit-input">
                            <input
                                type="number"
                                min="1"
                                max="3600"
                                value={timeBudgetSeconds}
                                onChange={event => setTimeBudgetSeconds(Number(event.target.value))}
                            />
                            <b>s</b>
                        </div>
                    </label>
                </div>
                <div className="adaptive-capture__subhead">Selective RAW evidence region</div>
                <div className="adaptive-capture__numbers adaptive-capture__numbers--four">
                    {(['x', 'y', 'width', 'height'] as const).map(key => (
                        <label key={key}>
                            <span>{key.toUpperCase()}</span>
                            <input
                                type="number"
                                min="0"
                                value={roi[key]}
                                onChange={event => setRoi(current => ({
                                    ...current,
                                    [key]: Math.max(0, Number(event.target.value)),
                                }))}
                            />
                        </label>
                    ))}
                </div>
                <div className="adaptive-capture__subhead">Physical focus candidate envelope</div>
                <div className="adaptive-capture__numbers">
                    <label>
                        <span>Far limit / min D</span>
                        <input type="number" min="0" step="0.01" value={minimumFocus}
                            onChange={event => setMinimumFocus(Number(event.target.value))} />
                    </label>
                    <label>
                        <span>Near limit / max D</span>
                        <input type="number" min="0" step="0.01" value={maximumFocus}
                            onChange={event => setMaximumFocus(Number(event.target.value))} />
                    </label>
                    <label>
                        <span>Planes</span>
                        <input type="number" min="1" max="256" value={focusSteps}
                            onChange={event => setFocusSteps(Number(event.target.value))} />
                    </label>
                    <label>
                        <span>Readout ms</span>
                        <input type="number" min="0" value={readoutMs}
                            onChange={event => setReadoutMs(Number(event.target.value))} />
                    </label>
                    <label>
                        <span>Settle ms</span>
                        <input type="number" min="0" value={settleMs}
                            onChange={event => setSettleMs(Number(event.target.value))} />
                    </label>
                </div>
                <button
                    className="adaptive-capture__primary"
                    onClick={beginSession}
                    disabled={busy || locked || loadingAssets}
                >
                    {busy ? <Loader2 className="spin" size={18} /> : <BrainCircuit size={18} />}
                    Initialize measured planner
                </button>
                {!profileAssets.length && projectId && !loadingAssets && (
                    <p className="adaptive-capture__warning">
                        <AlertTriangle size={15} />
                        No calibrated sensor noise JSON was found in this project's RAW or processed
                        assets. Adaptive capture fails closed without exact calibration.
                    </p>
                )}
            </section>
        );
    }

    return (
        <section className="adaptive-capture adaptive-capture--active">
            <div className="adaptive-capture__run-head">
                <div>
                    <span className="adaptive-capture__eyebrow">SESSION {session.session_id.slice(0, 8)}</span>
                    <h3>{complete ? 'Evidence target reached' : 'Next measured action'}</h3>
                </div>
                <span className="adaptive-capture__generation">
                    G{session.generation} / {session.status.retained_frame_count} RAWs
                </span>
            </div>
            <div className="adaptive-capture__metrics">
                <div><span>Radiance worst variance</span><strong>{radianceWorst.toExponential(2)}</strong></div>
                <div><span>Focus worst std dev</span><strong>{focusWorst.toFixed(4)} D</strong></div>
                <div><span>Elapsed</span><strong>{session.status.posterior.elapsed_ms.toFixed(0)} ms</strong></div>
                <div><span>Thermal load</span><strong>{session.status.posterior.thermal_load.toFixed(3)}</strong></div>
            </div>
            {selected && !complete ? (
                <>
                    <div className="adaptive-capture__recommendation">
                        <div className="adaptive-capture__dial">
                            <Crosshair size={24} />
                            <span>FOCUS</span>
                            <strong>{selected.candidate.focus_diopters.toFixed(3)} D</strong>
                            <small>
                                {selected.candidate.focus_diopters > 0
                                    ? `${(1 / selected.candidate.focus_diopters).toFixed(3)} m`
                                    : 'infinity'}
                            </small>
                        </div>
                        <div><span>SHUTTER</span><strong>{selected.candidate.shutter_seconds.toFixed(6)} s</strong></div>
                        <div><span>ISO</span><strong>{selected.candidate.iso}</strong></div>
                        <div><span>HDR gain</span><strong>{selected.hdr_information_nats.toFixed(3)} nat</strong></div>
                        <div><span>Focus gain</span><strong>{selected.focus_information_nats.toFixed(3)} nat</strong></div>
                        <div><span>Cost</span><strong>{selected.capture_cost_ms.toFixed(0)} ms</strong></div>
                    </div>
                    <div className="adaptive-capture__telemetry">
                        <label>
                            <span>Measured motion px/s</span>
                            <input type="number" min="0" step="0.01" value={motion}
                                onChange={event => setMotion(Number(event.target.value))} />
                        </label>
                        <label>
                            <span>Measured thermal load</span>
                            <input type="number" min="0" step="0.01" value={thermal}
                                onChange={event => setThermal(Number(event.target.value))} />
                        </label>
                    </div>
                    <label className="adaptive-capture__focus-confirm">
                        <input
                            type="checkbox"
                            checked={focusConfirmed}
                            onChange={event => setFocusConfirmed(event.target.checked)}
                        />
                        <span>
                            Lens is physically set to {selected.candidate.focus_diopters.toFixed(3)} D.
                            TrueShot will verify this against the completed NEF.
                        </span>
                    </label>
                    <div className="adaptive-capture__actions">
                        <button onClick={captureNext} disabled={busy || !focusConfirmed || locked}>
                            {busy ? <Loader2 className="spin" size={18} /> : <Gauge size={18} />}
                            Capture, verify, assimilate
                        </button>
                        <button onClick={stopSession} disabled={busy}>
                            <CircleStop size={18} /> Stop safely
                        </button>
                    </div>
                </>
            ) : (
                <div className="adaptive-capture__complete">
                    <Check size={30} />
                    <div>
                        <strong>{session.status.termination?.replaceAll('_', ' ') ?? 'No useful candidate remains'}</strong>
                        <span>Measured provenance and every retained RAW remain in the project.</span>
                    </div>
                    <button onClick={clearCompleted}><RefreshCw size={16} /> New session</button>
                </div>
            )}
            <div className="adaptive-capture__explain">
                <span>Planner exclusions</span>
                <b>{session.status.decision.rejected_motion} motion</b>
                <b>{session.status.decision.rejected_budget} resource</b>
                <b>{session.status.decision.rejected_calibration} uncalibrated</b>
                <b>HDR {session.status.decision.stop_hdr ? 'complete' : 'active'}</b>
                <b>Focus {session.status.decision.stop_focus ? 'complete' : 'active'}</b>
            </div>
            {lastStep && (
                <div className="adaptive-capture__last-step">
                    <span>LAST RETAINED</span>
                    <strong>{lastStep.capture_path}</strong>
                    <small>
                        {lastStep.measured_capture_elapsed_ms.toFixed(0)} ms /
                        {' '}{lastStep.report.radiance_updates} radiance updates /
                        {' '}{lastStep.report.focus_updates} focus updates /
                        {' '}{lastStep.report.censored_constraints} censored constraints
                    </small>
                </div>
            )}
        </section>
    );
}
