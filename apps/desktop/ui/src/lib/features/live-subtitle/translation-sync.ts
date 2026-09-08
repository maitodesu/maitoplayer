import type { SubtitleCueV1 } from '../../contracts/generated';
import { isCueActive } from '../../shell/time';

export const MAX_VISIBLE_TRANSLATION_CUES = 3;
export const MAX_VISIBLE_TRANSLATION_CODEPOINTS = 600;

/**
 * Selects display-only translation cues against the same exact media timestamp
 * used by the Japanese track. We intentionally do not fuzzy-pair text: small
 * boundary skews are honored as authored, avoiding a translation from being
 * attached to the wrong Japanese line.
 */
export function visibleTranslationCues(cues: SubtitleCueV1[], positionUs: number): SubtitleCueV1[] {
  const seen = new Set<string>();
  const visible: SubtitleCueV1[] = [];
  const active = cues
    .filter((cue) => isCueActive(cue.start_us, cue.end_us, positionUs))
    .sort(
      (left, right) =>
        left.start_us - right.start_us ||
        left.track_order - right.track_order ||
        left.end_us - right.end_us ||
        left.cue_id.localeCompare(right.cue_id),
    );

  for (const cue of active) {
    const identity = normalizeVisibleText(cue.plain_text);
    if (!identity || seen.has(identity)) continue;
    seen.add(identity);
    visible.push({
      ...cue,
      plain_text: clipCodepoints(cue.plain_text, MAX_VISIBLE_TRANSLATION_CODEPOINTS),
    });
    if (visible.length === MAX_VISIBLE_TRANSLATION_CUES) break;
  }
  return visible;
}

function normalizeVisibleText(value: string): string {
  return value.replace(/\s+/gu, ' ').trim();
}

function clipCodepoints(value: string, limit: number): string {
  const codepoints = Array.from(value);
  if (codepoints.length <= limit) return value;
  return `${codepoints.slice(0, limit).join('').trimEnd()}…`;
}
