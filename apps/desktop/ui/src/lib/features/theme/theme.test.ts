import { describe, expect, it } from 'vitest';
import {
  DEFAULT_THEME,
  THEME_STORAGE_KEY,
  applyTheme,
  isThemeId,
  persistTheme,
  resolveAppliedTheme,
  resolveInitialTheme,
  systemTheme,
} from './theme';

function storageWith(value: string | null) {
  return {
    getItem: () => value,
    setItem: () => undefined,
  };
}

describe('theme preference', () => {
  it('accepts only supported stable theme identifiers', () => {
    expect(isThemeId('shibui-cinema')).toBe(true);
    expect(isThemeId('tokyo-editorial')).toBe(true);
    expect(isThemeId('neon-night')).toBe(true);
    expect(isThemeId('neon')).toBe(false);
    expect(isThemeId(null)).toBe(false);
  });

  it('prefers a valid saved selection over the system fallback', () => {
    expect(resolveInitialTheme(storageWith('neon-night'), true)).toBe('neon-night');
    expect(resolveInitialTheme(storageWith('invalid'), true)).toBe('tokyo-editorial');
    expect(resolveInitialTheme(storageWith(null), false)).toBe(DEFAULT_THEME);
  });

  it('maps a light system to Editorial and dark to Shibui', () => {
    expect(systemTheme(true)).toBe('tokyo-editorial');
    expect(systemTheme(false)).toBe('shibui-cinema');
  });

  it('uses the currently applied theme when a picker remounts', () => {
    expect(resolveAppliedTheme('tokyo-editorial', DEFAULT_THEME)).toBe('tokyo-editorial');
    expect(resolveAppliedTheme('unknown', DEFAULT_THEME)).toBe('shibui-cinema');
  });

  it('applies both theme identity and native control color scheme', () => {
    const root = { dataset: {}, style: { colorScheme: '' } };
    applyTheme('tokyo-editorial', root);
    expect(root.dataset).toEqual({ theme: 'tokyo-editorial' });
    expect(root.style.colorScheme).toBe('light');
    applyTheme('neon-night', root);
    expect(root.style.colorScheme).toBe('dark');
  });

  it('persists under a versioned key and tolerates disabled storage', () => {
    const writes: [string, string][] = [];
    persistTheme('shibui-cinema', {
      getItem: () => null,
      setItem: (key, value) => writes.push([key, value]),
    });
    expect(writes).toEqual([[THEME_STORAGE_KEY, 'shibui-cinema']]);

    const blocked = {
      getItem: () => {
        throw new Error('blocked');
      },
      setItem: () => {
        throw new Error('blocked');
      },
    };
    expect(resolveInitialTheme(blocked, false)).toBe(DEFAULT_THEME);
    expect(() => persistTheme('neon-night', blocked)).not.toThrow();
  });
});
