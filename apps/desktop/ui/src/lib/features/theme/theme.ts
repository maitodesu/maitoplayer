export const THEME_STORAGE_KEY = 'migaku.ui-theme.v1';

export const themeOptions = [
  {
    id: 'shibui-cinema',
    label: 'Shibui Cinema',
    shortLabel: 'Shibui',
    description: 'Quiet ink, ivory, and vermillion for focused viewing.',
    swatches: ['#0d0e0c', '#f0ece3', '#d9483f'],
  },
  {
    id: 'tokyo-editorial',
    label: 'Tokyo Editorial',
    shortLabel: 'Editorial',
    description: 'Crisp paper, black rules, and red editorial marks.',
    swatches: ['#f4f1e9', '#181817', '#db2f2f'],
  },
  {
    id: 'neon-night',
    label: 'Neon Night',
    shortLabel: 'Neon',
    description: 'Glassy midnight surfaces with indigo and cyan light.',
    swatches: ['#080a11', '#8d83ff', '#75e4ee'],
  },
] as const;

export type ThemeId = (typeof themeOptions)[number]['id'];

export const DEFAULT_THEME: ThemeId = 'shibui-cinema';

interface ThemeStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

interface ThemeRoot {
  dataset: DOMStringMap;
  style: Pick<CSSStyleDeclaration, 'colorScheme'>;
}

export function isThemeId(value: unknown): value is ThemeId {
  return themeOptions.some((theme) => theme.id === value);
}

export function resolveAppliedTheme(value: unknown, fallback: ThemeId): ThemeId {
  return isThemeId(value) ? value : fallback;
}

export function systemTheme(prefersLight: boolean): ThemeId {
  return prefersLight ? 'tokyo-editorial' : DEFAULT_THEME;
}

export function resolveInitialTheme(storage: ThemeStorage | null, prefersLight = false): ThemeId {
  try {
    const stored = storage?.getItem(THEME_STORAGE_KEY);
    if (isThemeId(stored)) return stored;
  } catch {
    // Privacy settings and hardened webviews may reject localStorage access.
  }
  return systemTheme(prefersLight);
}

export function applyTheme(theme: ThemeId, root: ThemeRoot): void {
  root.dataset.theme = theme;
  root.style.colorScheme = theme === 'tokyo-editorial' ? 'light' : 'dark';
}

export function persistTheme(theme: ThemeId, storage: ThemeStorage | null): void {
  try {
    storage?.setItem(THEME_STORAGE_KEY, theme);
  } catch {
    // Applying the theme still succeeds when persistence is unavailable.
  }
}

export function initializeTheme(): ThemeId {
  if (typeof document === 'undefined') return DEFAULT_THEME;
  let storage: Storage | null = null;
  let prefersLight = false;
  try {
    storage = window.localStorage;
    prefersLight = window.matchMedia?.('(prefers-color-scheme: light)').matches ?? false;
  } catch {
    // Use the dark Shibui default in restricted/non-browser environments.
  }
  const theme = resolveInitialTheme(storage, prefersLight);
  applyTheme(theme, document.documentElement);
  return theme;
}

export function chooseTheme(theme: ThemeId): void {
  if (typeof document === 'undefined') return;
  applyTheme(theme, document.documentElement);
  try {
    persistTheme(theme, window.localStorage);
  } catch {
    // The visual selection remains active for this session.
  }
  window.dispatchEvent(new CustomEvent<ThemeId>('migaku-theme-change', { detail: theme }));
}

// ThemePicker is imported with the application module graph, so this runs
// before Svelte mounts and avoids a post-mount theme flash in normal startup.
export const initialTheme = initializeTheme();
