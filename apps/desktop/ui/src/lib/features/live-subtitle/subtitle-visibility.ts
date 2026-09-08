export type SubtitleVisibilityControl = 'furigana' | 'japanese' | 'english';

export interface SubtitleVisibility {
  furigana: boolean;
  japanese: boolean;
  english: boolean;
}

export const SUBTITLE_VISIBILITY_STORAGE_KEY = 'migaku.subtitle-visibility.v1';

export const DEFAULT_SUBTITLE_VISIBILITY: SubtitleVisibility = {
  furigana: true,
  japanese: true,
  english: true,
};

const shortcutMap: Readonly<Record<string, SubtitleVisibilityControl>> = {
  r: 'furigana',
  j: 'japanese',
  e: 'english',
};

export function parseSubtitleVisibility(value: string | null): SubtitleVisibility {
  if (!value) return { ...DEFAULT_SUBTITLE_VISIBILITY };
  try {
    const parsed = JSON.parse(value) as Partial<Record<SubtitleVisibilityControl, unknown>>;
    return {
      furigana:
        typeof parsed.furigana === 'boolean'
          ? parsed.furigana
          : DEFAULT_SUBTITLE_VISIBILITY.furigana,
      japanese:
        typeof parsed.japanese === 'boolean'
          ? parsed.japanese
          : DEFAULT_SUBTITLE_VISIBILITY.japanese,
      english:
        typeof parsed.english === 'boolean' ? parsed.english : DEFAULT_SUBTITLE_VISIBILITY.english,
    };
  } catch {
    return { ...DEFAULT_SUBTITLE_VISIBILITY };
  }
}

export function loadSubtitleVisibility(storage: Pick<Storage, 'getItem'>): SubtitleVisibility {
  try {
    return parseSubtitleVisibility(storage.getItem(SUBTITLE_VISIBILITY_STORAGE_KEY));
  } catch {
    return { ...DEFAULT_SUBTITLE_VISIBILITY };
  }
}

export function saveSubtitleVisibility(
  storage: Pick<Storage, 'setItem'>,
  visibility: SubtitleVisibility,
): void {
  try {
    storage.setItem(SUBTITLE_VISIBILITY_STORAGE_KEY, JSON.stringify(visibility));
  } catch {
    // Playback settings remain usable even when browser storage is unavailable.
  }
}

export function subtitleVisibilityShortcut(key: string): SubtitleVisibilityControl | null {
  return shortcutMap[key.toLocaleLowerCase()] ?? null;
}

export function isEditableOrMediaControl(target: EventTarget | null): boolean {
  return (
    target instanceof Element &&
    target.closest('input, select, textarea, button, summary, video, [contenteditable="true"]') !==
      null
  );
}
