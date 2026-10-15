// Light/dark, persisted in the same `settings` table as everything else.
//
// The palette is CSS custom properties on `:root` (see `app.css`); this module only flips the
// `data-theme` attribute on `<html>` and keeps the chosen value in the backend, so a restart
// agrees with the window that set it.

import { getSettings, setSettings } from './ipc';

export type Theme = 'dark' | 'light';

/** `localStorage` is the only thing readable before the first paint; the backend copy wins after. */
const STORAGE_KEY = '9drive:theme';

function readStoredTheme(): Theme {
  try {
    return localStorage.getItem(STORAGE_KEY) === 'light' ? 'light' : 'dark';
  } catch {
    return 'dark';
  }
}

/** Applies the palette and mirrors the choice into `localStorage` for the next launch. */
export function paint(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
  try {
    localStorage.setItem(STORAGE_KEY, theme);
  } catch {
    // A profile with storage disabled still gets the theme for this session.
  }
}

/** What the shell applies before any backend call answers, so the first paint is not a flash. */
export const initialTheme: Theme = readStoredTheme();

export class ThemeState {
  current = $state<Theme>(initialTheme);

  /** Paints immediately, then tells the backend. A failed write must not interrupt boot. */
  async apply(next: Theme): Promise<void> {
    this.current = next;
    paint(next);
    try {
      await setSettings({ theme: next });
    } catch {
      // Losing the write only costs the preference on the next launch.
    }
  }

  async toggle(): Promise<void> {
    await this.apply(this.current === 'dark' ? 'light' : 'dark');
  }

  /** Reads the stored value on boot. Anything but an explicit `light` stays dark. */
  async load(): Promise<void> {
    try {
      const values = await getSettings();
      this.current = values.theme === 'light' ? 'light' : 'dark';
    } catch {
      this.current = readStoredTheme();
    }
    paint(this.current);
  }
}

export const theme = new ThemeState();