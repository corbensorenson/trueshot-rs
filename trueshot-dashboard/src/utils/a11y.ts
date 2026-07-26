// Accessibility Utilities
export const A11y = {
    // Check contrast ratio
    checkContrast: (bgHex: string, fgHex: string): number => {
        const parseHex = (hex: string): [number, number, number] | null => {
            const cleaned = hex.replace('#', '').trim();
            if (cleaned.length === 3) {
                const r = parseInt(cleaned[0] + cleaned[0], 16);
                const g = parseInt(cleaned[1] + cleaned[1], 16);
                const b = parseInt(cleaned[2] + cleaned[2], 16);
                return [r, g, b];
            }
            if (cleaned.length === 6) {
                const r = parseInt(cleaned.slice(0, 2), 16);
                const g = parseInt(cleaned.slice(2, 4), 16);
                const b = parseInt(cleaned.slice(4, 6), 16);
                return [r, g, b];
            }
            return null;
        };

        const toLinear = (value: number) => {
            const srgb = value / 255;
            return srgb <= 0.03928 ? srgb / 12.92 : Math.pow((srgb + 0.055) / 1.055, 2.4);
        };

        const bg = parseHex(bgHex);
        const fg = parseHex(fgHex);
        if (!bg || !fg) return 1;

        const bgLum = 0.2126 * toLinear(bg[0]) + 0.7152 * toLinear(bg[1]) + 0.0722 * toLinear(bg[2]);
        const fgLum = 0.2126 * toLinear(fg[0]) + 0.7152 * toLinear(fg[1]) + 0.0722 * toLinear(fg[2]);
        const lighter = Math.max(bgLum, fgLum);
        const darker = Math.min(bgLum, fgLum);
        return (lighter + 0.05) / (darker + 0.05);
    },

    // announce for screen readers
    announce: (msg: string) => {
        const d = document.createElement('div');
        d.setAttribute('aria-live', 'polite');
        d.style.position = 'absolute';
        d.style.left = '-9999px';
        d.innerText = msg;
        document.body.appendChild(d);
        setTimeout(() => document.body.removeChild(d), 1000);
    }
};
