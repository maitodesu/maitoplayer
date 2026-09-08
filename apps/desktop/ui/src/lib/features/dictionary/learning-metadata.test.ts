import { describe, expect, it } from 'vitest';
import {
  contourPoints,
  describePitchAccent,
  extractLearningMetadata,
  followingParticleLevel,
  pitchAccentName,
  type PitchAccentPattern,
} from './learning-metadata';

const nakadaka: PitchAccentPattern = {
  reading: 'こころ',
  morae: ['こ', 'こ', 'ろ'],
  levels: ['low', 'high', 'low'],
  drop_after_mora: 2,
  source: 'test',
};

describe('learning metadata', () => {
  it('accepts valid pitch and JLPT metadata while dropping malformed patterns', () => {
    expect(
      extractLearningMetadata({
        pitch_accents: [nakadaka, { ...nakadaka, levels: ['low'] }],
        jlpt_level: 3,
        jlpt_source: 'test source',
      }),
    ).toEqual({ pitchAccents: [nakadaka], jlptLevel: 3, jlptSource: 'test source' });
  });

  it('keeps optional metadata absent instead of inventing a value', () => {
    expect(extractLearningMetadata({ pitch_accents: [], jlpt_level: 0 })).toEqual({
      pitchAccents: [],
      jlptLevel: null,
      jlptSource: null,
    });
  });

  it('names and describes common Tokyo accent classes accessibly', () => {
    expect(pitchAccentName(nakadaka)).toBe('Nakadaka');
    expect(describePitchAccent(nakadaka)).toBe(
      'こころ: low, high, low. Nakadaka accent. Pitch drops after mora 2; a following particle is low.',
    );
    const heiban: PitchAccentPattern = {
      ...nakadaka,
      levels: ['low', 'high', 'high'],
      drop_after_mora: null,
    };
    expect(pitchAccentName(heiban)).toBe('Heiban');
    expect(followingParticleLevel(heiban)).toBe('high');
    expect(followingParticleLevel(nakadaka)).toBe('low');
  });

  it('adds a following particle point so heiban and odaka remain distinguishable', () => {
    expect(contourPoints(nakadaka)).toBe('24,28 72,8 120,28 168,28');
    expect(
      contourPoints({ ...nakadaka, levels: ['low', 'high', 'high'], drop_after_mora: null }),
    ).toBe('24,28 72,8 120,8 168,8');
    expect(
      contourPoints({ ...nakadaka, levels: ['low', 'high', 'high'], drop_after_mora: 3 }),
    ).toBe('24,28 72,8 120,8 168,28');
  });
});
