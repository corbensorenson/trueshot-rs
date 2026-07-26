import { Sun, Moon } from 'lucide-react';
import { useTheme } from '../hooks/useTheme';

interface ThemeToggleFloatingProps {
  className?: string;
}

export const ThemeToggleFloating = ({ className }: ThemeToggleFloatingProps) => {
  const { theme, toggleTheme } = useTheme();

  return (
    <button
      onClick={toggleTheme}
      className={`fixed top-4 right-4 z-[200] flex items-center gap-2 rounded-full border border-[color:var(--ts-border)] bg-[color:var(--ts-panel)] px-3 py-2 text-[11px] font-semibold uppercase tracking-widest text-[color:color-mix(in_srgb,var(--ts-text)_80%,transparent)] shadow-lg backdrop-blur ts-transition ${className ?? ''}`}
      aria-label={`Switch to ${theme === 'dark' ? 'light' : 'dark'} mode`}
      title={`Switch to ${theme === 'dark' ? 'light' : 'dark'} mode`}
    >
      {theme === 'dark' ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
      <span>{theme === 'dark' ? 'Light' : 'Dark'}</span>
    </button>
  );
};
