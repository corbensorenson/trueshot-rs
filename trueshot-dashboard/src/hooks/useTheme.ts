import { useEffect, useState } from 'react';

export type ThemeMode = 'dark' | 'light';

const STORAGE_KEY = 'trueshot_theme';

const getInitialTheme = (): ThemeMode => {
  if (typeof window !== 'undefined') {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (stored === 'dark' || stored === 'light') {
      return stored;
    }
    if (window.matchMedia) {
      return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
    }
  }
  return 'dark';
};

export const useTheme = () => {
  const [theme, setTheme] = useState<ThemeMode>(() => getInitialTheme());

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    if (typeof window !== 'undefined') {
      window.localStorage.setItem(STORAGE_KEY, theme);
      window.dispatchEvent(new Event('trueshot-theme'));
    }
  }, [theme]);

  useEffect(() => {
    if (typeof window === 'undefined') return;
    const syncTheme = () => {
      const stored = window.localStorage.getItem(STORAGE_KEY);
      if (stored === 'dark' || stored === 'light') {
        setTheme(stored);
      }
    };
    window.addEventListener('storage', syncTheme);
    window.addEventListener('trueshot-theme', syncTheme);
    return () => {
      window.removeEventListener('storage', syncTheme);
      window.removeEventListener('trueshot-theme', syncTheme);
    };
  }, []);

  const toggleTheme = () => {
    setTheme((prev) => (prev === 'dark' ? 'light' : 'dark'));
  };

  return { theme, setTheme, toggleTheme };
};
