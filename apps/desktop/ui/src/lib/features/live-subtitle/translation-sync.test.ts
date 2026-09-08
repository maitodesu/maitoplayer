import { describe, expect, it } from 'vitest';
import type { SubtitleCueV1 } from '../../contracts/generated';
import {
  MAX_VISIBLE_TRANSLATION_CODEPOINTS,
  MAX_VISIBLE_TRANSLATION_CUES,
  visibleTranslationCues,
} from './translation-sync';

function cue(id: string, startUs: number, endUs: number, text = id, order = 0): SubtitleCueV1 {
  return {
    cue_id: id,
    start_us: startUs,
    end_us: endUs,
    plain_text: text,
    source_text: text,
    track_order: order,
    style_hint: { alignment: null, actor: null },
  };
}

describe('translation synchronization', () => {
  it('uses exact half-open cue endpoints', () => {
    const cues = [cue('first', 10, 20), cue('second', 20, 30)];
    expect(visibleTranslationCues(cues, 10).map((item) => item.cue_id)).toEqual(['first']);
    expect(visibleTranslationCues(cues, 20).map((item) => item.cue_id)).toEqual(['second']);
    expect(visibleTranslationCues(cues, 30)).toEqual([]);
  });

  it('honors boundary skew without fuzzy cross-pairing', () => {
    const slightlyLateAndEarly = [cue('en', 1_020_000, 2_980_000, 'A translation')];
    expect(visibleTranslationCues(slightlyLateAndEarly, 1_000_000)).toEqual([]);
    expect(visibleTranslationCues(slightlyLateAndEarly, 1_020_000)).toHaveLength(1);
    expect(visibleTranslationCues(slightlyLateAndEarly, 2_980_000)).toEqual([]);
  });

  it('switches multiple English cues during one long Japanese cue', () => {
    // The Japanese cue spans [0, 10); only the shared media clock is needed to
    // move from the first authored English segment to the second.
    const english = [cue('en-1', 0, 5, 'First half'), cue('en-2', 5, 10, 'Second half')];
    expect(visibleTranslationCues(english, 4).map((item) => item.plain_text)).toEqual([
      'First half',
    ]);
    expect(visibleTranslationCues(english, 5).map((item) => item.plain_text)).toEqual([
      'Second half',
    ]);
  });

  it('keeps deterministic overlapping cues and removes duplicate text', () => {
    const english = [
      cue('later', 2, 9, 'Second speaker', 2),
      cue('first', 0, 10, 'Same line', 1),
      cue('duplicate', 1, 8, ' Same\nline ', 0),
    ];
    expect(visibleTranslationCues(english, 3).map((item) => item.cue_id)).toEqual([
      'first',
      'later',
    ]);
  });

  it('returns no translation in authored gaps', () => {
    expect(visibleTranslationCues([cue('before', 0, 10), cue('after', 20, 30)], 15)).toEqual([]);
  });

  it('bounds pathological overlaps and oversized cue text', () => {
    const english = Array.from({ length: 12 }, (_, index) =>
      cue(`cue-${index}`, 0, 10, `${index}-${'x'.repeat(800)}`, index),
    );
    const visible = visibleTranslationCues(english, 5);
    expect(visible).toHaveLength(MAX_VISIBLE_TRANSLATION_CUES);
    const first = visible[0];
    if (!first) throw new Error('Expected a bounded translation cue.');
    expect(Array.from(first.plain_text)).toHaveLength(MAX_VISIBLE_TRANSLATION_CODEPOINTS + 1);
    expect(first.plain_text.endsWith('…')).toBe(true);
  });
});
