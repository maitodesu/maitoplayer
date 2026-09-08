import { describe, expect, it, vi } from 'vitest';
import {
  DEFAULT_SUBTITLE_VISIBILITY,
  loadSubtitleVisibility,
  parseSubtitleVisibility,
  saveSubtitleVisibility,
  SUBTITLE_VISIBILITY_STORAGE_KEY,
  subtitleVisibilityShortcut,
} from './subtitle-visibility';

describe('subtitle visibility preferences', () => {
  it('uses visible defaults when no persisted value exists', () => {
    expect(parseSubtitleVisibility(null)).toEqual(DEFAULT_SUBTITLE_VISIBILITY);
  });

  it('restores independent persisted controls', () => {
    expect(parseSubtitleVisibility('{"furigana":false,"japanese":true,"english":false}')).toEqual({
      furigana: false,
      japanese: true,
      english: false,
    });
  });

  it('keeps safe defaults for malformed and partial data', () => {
    expect(parseSubtitleVisibility('{bad json')).toEqual(DEFAULT_SUBTITLE_VISIBILITY);
    expect(parseSubtitleVisibility('{"japanese":false}')).toEqual({
      furigana: true,
      japanese: false,
      english: true,
    });
  });

  it('maps discoverable shortcuts without case sensitivity', () => {
    expect(subtitleVisibilityShortcut('J')).toBe('japanese');
    expect(subtitleVisibilityShortcut('e')).toBe('english');
    expect(subtitleVisibilityShortcut('R')).toBe('furigana');
    expect(subtitleVisibilityShortcut('f')).toBeNull();
  });

  it('loads and saves through the stable storage key', () => {
    const getItem = vi.fn(() => '{"furigana":false,"japanese":true,"english":true}');
    expect(loadSubtitleVisibility({ getItem })).toEqual({
      furigana: false,
      japanese: true,
      english: true,
    });
    expect(getItem).toHaveBeenCalledWith(SUBTITLE_VISIBILITY_STORAGE_KEY);

    const setItem = vi.fn();
    saveSubtitleVisibility({ setItem }, { furigana: true, japanese: false, english: true });
    expect(setItem).toHaveBeenCalledWith(
      SUBTITLE_VISIBILITY_STORAGE_KEY,
      '{"furigana":true,"japanese":false,"english":true}',
    );
  });

  it('fails open when storage access is denied', () => {
    expect(
      loadSubtitleVisibility({
        getItem: () => {
          throw new Error('denied');
        },
      }),
    ).toEqual(DEFAULT_SUBTITLE_VISIBILITY);

    expect(() =>
      saveSubtitleVisibility(
        {
          setItem: () => {
            throw new Error('denied');
          },
        },
        DEFAULT_SUBTITLE_VISIBILITY,
      ),
    ).not.toThrow();
  });
});
