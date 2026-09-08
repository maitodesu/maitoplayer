export type PitchLevel = 'low' | 'high';

export interface PitchAccentPattern {
  reading: string;
  morae: string[];
  levels: PitchLevel[];
  drop_after_mora: number | null;
  source: string;
}

export interface LearningMetadata {
  pitchAccents: PitchAccentPattern[];
  jlptLevel: number | null;
  jlptSource: string | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function pitchAccentPattern(value: unknown): PitchAccentPattern | null {
  if (!isRecord(value)) return null;

  const morae = Array.isArray(value.morae)
    ? value.morae.filter((mora): mora is string => typeof mora === 'string' && mora.length > 0)
    : [];
  const levels = Array.isArray(value.levels)
    ? value.levels.filter((level): level is PitchLevel => level === 'low' || level === 'high')
    : [];
  const dropAfterMora =
    value.drop_after_mora === null || value.drop_after_mora === undefined
      ? null
      : typeof value.drop_after_mora === 'number' &&
          Number.isInteger(value.drop_after_mora) &&
          value.drop_after_mora >= 1 &&
          value.drop_after_mora <= morae.length
        ? value.drop_after_mora
        : undefined;

  if (
    typeof value.reading !== 'string' ||
    morae.length === 0 ||
    morae.length !== levels.length ||
    dropAfterMora === undefined
  ) {
    return null;
  }

  return {
    reading: value.reading,
    morae,
    levels,
    drop_after_mora: dropAfterMora,
    source: typeof value.source === 'string' ? value.source : '',
  };
}

export function extractLearningMetadata(value: unknown): LearningMetadata {
  const record = isRecord(value) ? value : {};
  const pitchAccents = Array.isArray(record.pitch_accents)
    ? record.pitch_accents
        .map(pitchAccentPattern)
        .filter((pattern): pattern is PitchAccentPattern => pattern !== null)
    : [];
  const jlptLevel =
    typeof record.jlpt_level === 'number' &&
    Number.isInteger(record.jlpt_level) &&
    record.jlpt_level >= 1 &&
    record.jlpt_level <= 5
      ? record.jlpt_level
      : null;

  return {
    pitchAccents,
    jlptLevel,
    jlptSource: typeof record.jlpt_source === 'string' ? record.jlpt_source : null,
  };
}

export function pitchAccentName(pattern: PitchAccentPattern): string {
  if (pattern.drop_after_mora === null) return 'Heiban';
  if (pattern.drop_after_mora === 1) return 'Atamadaka';
  if (pattern.drop_after_mora === pattern.morae.length) return 'Odaka';
  return 'Nakadaka';
}

export function describePitchAccent(pattern: PitchAccentPattern): string {
  const tones = pattern.levels.map((level) => (level === 'high' ? 'high' : 'low')).join(', ');
  const drop =
    pattern.drop_after_mora === null
      ? 'No lexical pitch drop; a following particle stays high.'
      : `Pitch drops after mora ${pattern.drop_after_mora}; a following particle is low.`;
  return `${pattern.reading}: ${tones}. ${pitchAccentName(pattern)} accent. ${drop}`;
}

export function followingParticleLevel(pattern: PitchAccentPattern): PitchLevel {
  return pattern.drop_after_mora === null ? 'high' : 'low';
}

export function contourPoints(pattern: PitchAccentPattern): string {
  return [...pattern.levels, followingParticleLevel(pattern)]
    .map((level, index) => `${24 + index * 48},${level === 'high' ? 8 : 28}`)
    .join(' ');
}
