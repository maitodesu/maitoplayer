import { describe, expect, it } from 'vitest';
import type { AnalyzedCueV1, AnalyzedTokenV1 } from '../../contracts/generated';
import { containsSelection, isSameSelection, type SubtitleTokenSelection } from './selection';

function analyzedCue(cueId: string, tokenId: string): AnalyzedCueV1 {
  return {
    cue: {
      cue_id: cueId,
      start_us: 0,
      end_us: 1_000_000,
      plain_text: '見る',
      source_text: '見る',
      track_order: 0,
      style_hint: {
        alignment: null,
        actor: null,
      },
    },
    tokens: [analyzedToken(tokenId)],
  };
}

function analyzedToken(tokenId: string): AnalyzedTokenV1 {
  return {
    token: {
      token_id: tokenId,
      surface: '見る',
      byte_start: 0,
      byte_end: 6,
      lemma: '見る',
      reading: 'みる',
      pronunciation: null,
      part_of_speech: ['verb'],
      lookup_candidate: true,
    },
    dictionary: [],
  };
}

describe('subtitle selection identity', () => {
  it('requires both cue and token identity because repeated cues can reuse token ids', () => {
    const firstCue = analyzedCue('cue-1', 'shared-token');
    const repeatedCue = analyzedCue('cue-2', 'shared-token');
    const selection: SubtitleTokenSelection = { cue: firstCue, token: firstCue.tokens[0]! };

    expect(isSameSelection(selection, firstCue, firstCue.tokens[0]!)).toBe(true);
    expect(isSameSelection(selection, repeatedCue, repeatedCue.tokens[0]!)).toBe(false);
    expect(containsSelection([repeatedCue], selection)).toBe(false);
  });
});
