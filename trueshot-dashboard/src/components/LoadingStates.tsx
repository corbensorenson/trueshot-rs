/**
 * Skeleton Loader - Animated placeholder for loading states
 */

interface SkeletonProps {
    className?: string;
}

export const Skeleton = ({ className = '' }: SkeletonProps) => (
    <div
        className={`animate-pulse bg-white/10 rounded ${className}`}
    />
);

interface SkeletonBoxProps {
    width?: string;
    height?: string;
    className?: string;
}

export const SkeletonBox = ({ width = 'w-full', height = 'h-8', className = '' }: SkeletonBoxProps) => (
    <Skeleton className={`${width} ${height} ${className}`} />
);

export const SkeletonText = ({ lines = 3 }: { lines?: number }) => (
    <div className="space-y-2">
        {Array.from({ length: lines }).map((_, i) => (
            <Skeleton
                key={i}
                className={`h-3 ${i === lines - 1 ? 'w-3/4' : 'w-full'}`}
            />
        ))}
    </div>
);

export const SkeletonCard = () => (
    <div className="p-4 bg-white/5 rounded-xl border border-white/10 space-y-3">
        <div className="flex items-center gap-3">
            <Skeleton className="w-10 h-10 rounded-full" />
            <div className="flex-1 space-y-2">
                <Skeleton className="h-4 w-1/2" />
                <Skeleton className="h-3 w-1/3" />
            </div>
        </div>
        <SkeletonText lines={2} />
    </div>
);

export const SkeletonDeviceList = () => (
    <div className="space-y-3">
        {Array.from({ length: 4 }).map((_, i) => (
            <div key={i} className="flex items-center gap-3 p-3 bg-white/5 rounded-lg">
                <Skeleton className="w-8 h-8 rounded-lg" />
                <div className="flex-1">
                    <Skeleton className="h-4 w-24 mb-1" />
                    <Skeleton className="h-3 w-16" />
                </div>
                <Skeleton className="w-16 h-6 rounded-full" />
            </div>
        ))}
    </div>
);

/**
 * Loading Spinner Component
 */
export const LoadingSpinner = ({ size = 'md' }: { size?: 'sm' | 'md' | 'lg' }) => {
    const sizeClasses = {
        sm: 'w-4 h-4 border-2',
        md: 'w-8 h-8 border-3',
        lg: 'w-12 h-12 border-4',
    };

    return (
        <div className={`${sizeClasses[size]} border-white/20 border-t-accent-cyan rounded-full animate-spin`} />
    );
};

/**
 * Full-area loading overlay
 */
export const LoadingOverlay = ({ message = 'Loading...' }: { message?: string }) => (
    <div className="absolute inset-0 bg-black/60 backdrop-blur-sm flex flex-col items-center justify-center z-50">
        <LoadingSpinner size="lg" />
        <p className="text-white/60 mt-4 text-sm">{message}</p>
    </div>
);
