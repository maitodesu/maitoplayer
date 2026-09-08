import type { AnalyzedCueV1, AnalyzedTokenV1 } from '../../contracts/generated';

export type SubtitleTokenSelection = { cue: AnalyzedCueV1; token: AnalyzedTokenV1 };

export function isSameSelection(
  selection: SubtitleTokenSelection | null | undefined,
  cue: AnalyzedCueV1,
  token: AnalyzedTokenV1,
): boolean {
  return (
    selection?.cue.cue.cue_id === cue.cue.cue_id &&
    selection.token.token.token_id === token.token.token_id
  );
}

export function containsSelection(
  cues: AnalyzedCueV1[],
  selection: SubtitleTokenSelection | null | undefined,
): boolean {
  return (
    selection !== null &&
    selection !== undefined &&
    cues.some((cue) => cue.tokens.some((token) => isSameSelection(selection, cue, token)))
  );
}
