import { useState, useRef, useEffect } from 'react';
import { FolderOpen, Upload, Box, HardDrive, Clock, Share2, Scissors } from 'lucide-react';
import { getProjects, createProject, openProjectFolder, importModel, getImuDiagnostics, getProjectLicense, type ImuDiagnostics, type ProjectLicense } from '../api/client';
import toast from 'react-hot-toast';
import { ShareAssetModal } from './ShareAssetModal';
import { EditAssetModal } from './EditAssetModal';

interface ProjectLibraryProps {
    isOpen: boolean;
    onClose: () => void;
}

interface ProjectSummary {
    name?: string;
    created?: string;
}

export const ProjectLibrary = ({ isOpen, onClose }: ProjectLibraryProps) => {
    const [projects, setProjects] = useState<ProjectSummary[]>([]);
    const [imuDiagnostics, setImuDiagnostics] = useState<Record<string, ImuDiagnostics | null>>({});
    const [licenseTerms, setLicenseTerms] = useState<Record<string, ProjectLicense | null>>({});
    const [shareProjectId, setShareProjectId] = useState<string | null>(null);
    const [editProjectId, setEditProjectId] = useState<string | null>(null);

    useEffect(() => {
        if (isOpen) {
            getProjects().then(setProjects).catch(e => {
                console.error(e);
                toast.error("Failed to load projects");
            });
        }
    }, [isOpen]);

    useEffect(() => {
        if (!isOpen || projects.length === 0) return;
        let cancelled = false;
        setImuDiagnostics({});
        setLicenseTerms({});
        const loadDiagnostics = async () => {
            for (const project of projects) {
                if (!project.name) continue;
                try {
                    const [diag, license] = await Promise.all([
                        getImuDiagnostics(project.name).catch(() => null),
                        getProjectLicense(project.name).catch(() => null),
                    ]);
                    if (cancelled) return;
                    setImuDiagnostics(prev => ({ ...prev, [project.name as string]: diag }));
                    setLicenseTerms(prev => ({ ...prev, [project.name as string]: license }));
                } catch (err) {
                    if (cancelled) return;
                    console.error(err);
                    setImuDiagnostics(prev => ({ ...prev, [project.name as string]: null }));
                    setLicenseTerms(prev => ({ ...prev, [project.name as string]: null }));
                }
            }
        };
        loadDiagnostics();
        return () => {
            cancelled = true;
        };
    }, [isOpen, projects]);

    const fileInputRef = useRef<HTMLInputElement>(null);
    const [activeImportId, setActiveImportId] = useState<string | null>(null);

    const handleImport = async (e: React.ChangeEvent<HTMLInputElement>) => {
        if (!e.target.files?.length || !activeImportId) return;
        try {
            await importModel(activeImportId, e.target.files[0]);
            toast.success("Imported successfully");
        } catch (err) {
            console.error(err);
            toast.error("Import failed");
        }
        setActiveImportId(null);
    };

    const handleCreateProject = async () => {
        try {
            const name = `Project_${Date.now()}`; // Simple auto-name for now or prompt?
            // Since user didn't ask for a modal form for create, I'll just auto-create for simplicity 
            // or I could prompt. Let's auto-create "Untitled Scan X"
            // Wait, createProject takes (name, desc).
            await createProject(name, "New Project");
            const list = await getProjects();
            setProjects(list);
            toast.success("New project created!");
        } catch (error) {
            console.error("Failed to create project:", error);
            toast.error("Create failed");
        }
    };

    return (
        <div className={`fixed inset-0 z-[60] bg-black/80 backdrop-blur-md flex items-center justify-center pointer-events-auto ${isOpen ? '' : 'hidden'}`}>
            <div className="bg-[#111] border border-white/10 rounded-2xl w-[800px] h-[600px] flex flex-col shadow-2xl overflow-hidden animate-in fade-in zoom-in-95 duration-200">
                <div className="p-6 border-b border-white/10 flex justify-between items-center bg-white/5">
                    <h2 className="text-2xl font-bold text-white tracking-tight">Project Library</h2>
                    <button onClick={onClose} className="text-white/40 hover:text-white px-4 py-2 rounded border border-white/10 hover:bg-white/10 text-xs font-bold uppercase tracking-widest">Close</button>
                </div>

                <div className="flex-1 overflow-y-auto p-6 grid grid-cols-3 gap-6 align-start content-start">
                    {/* Create New Card */}
                    <div onClick={handleCreateProject} className="aspect-square rounded-xl border-2 border-dashed border-white/10 hover:border-accent-blue/50 flex flex-col items-center justify-center cursor-pointer group transition-all bg-white/5 hover:bg-white/10">
                        <div className="w-12 h-12 rounded-full bg-white/5 group-hover:bg-accent-blue text-white/40 group-hover:text-black flex items-center justify-center transition-all mb-4">
                            <span className="text-2xl font-light">+</span>
                        </div>
                        <span className="text-white/40 font-bold uppercase text-xs tracking-widest group-hover:text-white">New Project</span>
                    </div>

                    {/* Project Cards */}
                    {projects.map((p, i) => (
                        <div key={i} className="aspect-square bg-white/5 rounded-xl border border-white/5 hover:border-white/20 p-4 flex flex-col group relative overflow-hidden transition-all">
                            <div className="flex-1 flex items-center justify-center">
                                <Box className="w-16 h-16 text-white/10 group-hover:text-accent-blue/50 transition-colors" />
                            </div>
                            <div className="mt-4">
                                <div className="flex justify-between items-start">
                                    <h3 className="text-white font-bold truncate pr-2">{p.name || `Project ${i}`}</h3>
                                </div>
                                <span className="text-white/40 text-xs flex items-center gap-1 mt-1">
                                    <Clock className="w-3 h-3" /> {p.created || 'Just now'}
                                </span>
                                {p.name && (
                                    <span
                                        className={`text-xs mt-1 inline-flex items-center gap-1 ${imuDiagnostics[p.name] === null ? 'text-red-300/80' : 'text-white/40'}`}
                                    >
                                        IMU:{' '}
                                        {imuDiagnostics[p.name]
                                            ? imuDiagnostics[p.name]?.status === 'ok'
                                                ? `${imuDiagnostics[p.name]?.samples} samples`
                                                : imuDiagnostics[p.name]?.status
                                            : 'loading'}
                                    </span>
                                )}
                                {p.name && (
                                    <span className="text-white/40 text-xs mt-1 inline-flex items-center gap-1">
                                        License:{' '}
                                        {licenseTerms[p.name]
                                            ? licenseTerms[p.name]?.title || 'Unspecified'
                                            : 'loading'}
                                        {licenseTerms[p.name]?.url && (
                                            <a
                                                className="text-accent-blue hover:text-white underline underline-offset-2"
                                                href={licenseTerms[p.name]?.url || undefined}
                                                target="_blank"
                                                rel="noreferrer"
                                            >
                                                View
                                            </a>
                                        )}
                                    </span>
                                )}
                            </div>

                            {/* Actions Overlay */}
                            <div className="absolute inset-0 bg-black/90 backdrop-blur-sm opacity-0 group-hover:opacity-100 transition-all duration-300 flex flex-col items-center justify-center gap-3 p-4 translate-y-4 group-hover:translate-y-0">
                                <button
                                    onClick={() => {
                                        if (!p.name) return;
                                        openProjectFolder(p.name);
                                    }}
                                    className="bg-white/10 hover:bg-white text-white hover:text-black px-4 py-3 rounded-xl text-xs font-bold uppercase tracking-widest flex items-center gap-2 transition-all w-full justify-center border border-white/10"
                                >
                                    <FolderOpen className="w-4 h-4" /> Open Folder
                                </button>
                                <button
                                    onClick={() => {
                                        if (!p.name) return;
                                        setActiveImportId(p.name);
                                        fileInputRef.current?.click();
                                    }}
                                    className="bg-accent-blue/10 hover:bg-accent-blue text-accent-blue hover:text-black px-4 py-3 rounded-xl text-xs font-bold uppercase tracking-widest flex items-center gap-2 transition-all w-full justify-center border border-accent-blue/20"
                                >
                                    <Upload className="w-4 h-4" /> Import Model
                                </button>
                                <button
                                    onClick={() => setShareProjectId(p.name || null)}
                                    className="bg-white/5 hover:bg-white text-white hover:text-black px-4 py-3 rounded-xl text-xs font-bold uppercase tracking-widest flex items-center gap-2 transition-all w-full justify-center border border-white/10"
                                >
                                    <Share2 className="w-4 h-4" /> Share Asset
                                </button>
                                <button
                                    onClick={() => setEditProjectId(p.name || null)}
                                    className="bg-white/5 hover:bg-white text-white hover:text-black px-4 py-3 rounded-xl text-xs font-bold uppercase tracking-widest flex items-center gap-2 transition-all w-full justify-center border border-white/10"
                                >
                                    <Scissors className="w-4 h-4" /> Edit Asset
                                </button>
                            </div>
                        </div>
                    ))}
                </div>

                <div className="p-4 border-t border-white/10 bg-black/20 text-[10px] text-white/30 flex justify-between">
                    <span>{projects.length} Projects</span>
                    <span className="flex items-center gap-1"><HardDrive className="w-3 h-3" /> Local Storage</span>
                </div>

                <input type="file" ref={fileInputRef} className="hidden" accept=".obj,.stl,.usdz,.ply,.glb,.gltf" onChange={handleImport} />
            </div>
            <ShareAssetModal
                projectId={shareProjectId}
                open={Boolean(shareProjectId)}
                onClose={() => setShareProjectId(null)}
            />
            <EditAssetModal
                projectId={editProjectId}
                open={Boolean(editProjectId)}
                onClose={() => setEditProjectId(null)}
            />
        </div>
    );
};
