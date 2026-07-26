import { toast } from 'react-hot-toast';
import Joyride, { Step } from 'react-joyride';

const steps: Step[] = [
    {
        target: 'header',
        content: 'Welcome to TrueShot v5.4! This is your mission control for hybrid photogrammetry.',
        disableBeacon: true,
    },
    {
        target: '.hardware-status',
        content: 'Check here to verify your Camera and Turntable connection status.',
    },
    {
        target: '.live-monitor',
        content: 'This area shows live feeds from all connected sensors. Use the toggle to switch to 3D view.',
    },
    {
        target: '.sequence-control',
        content: 'Configure your scan presets here (Matte, Shiny, etc.) and START the capture sequence.',
    },
    {
        target: '.project-library-btn',
        content: 'Access your previous scans and manage disk space from the Library.',
    }
];

export const OnboardingTour = ({ run, onFinish }: { run: boolean, onFinish: () => void }) => {
    return (
        <Joyride
            steps={steps}
            run={run}
            continuous
            showSkipButton
            showProgress
                styles={{
                    options: {
                    arrowColor: 'var(--ts-surface-elevated)',
                    backgroundColor: 'var(--ts-surface-elevated)',
                    overlayColor: 'var(--ts-overlay-strong)',
                    primaryColor: 'var(--ts-accent-cyan)',
                    textColor: 'var(--ts-text)',
                    zIndex: 1000,
                    },
                    tooltipContainer: {
                    textAlign: 'left',
                    border: '1px solid var(--ts-border)'
                    },
                    buttonNext: {
                    backgroundColor: 'var(--ts-accent-cyan)',
                    color: 'var(--ts-text-on-accent)',
                    fontWeight: 800,
                    borderRadius: '8px',
                    fontSize: '12px'
                    },
                    buttonBack: {
                    color: 'var(--ts-text)',
                    marginRight: 10,
                    }
                }}
            callback={(data) => {
                if (data.status === 'finished' || data.status === 'skipped') {
                    onFinish();
                    toast.success("Ready for Takeoff!");
                }
            }}
        />
    );
};
