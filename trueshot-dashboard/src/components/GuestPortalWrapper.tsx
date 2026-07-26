import GuestPortal from './GuestPortal';
import { ThemeToggleFloating } from './ThemeToggleFloating';

export const GuestPortalWrapper = () => {
    const eventId = window.location.pathname.split('/guest/')[1] || 'default-event';
    return (
        <>
            <ThemeToggleFloating />
            <GuestPortal eventId={eventId} />
        </>
    );
};
