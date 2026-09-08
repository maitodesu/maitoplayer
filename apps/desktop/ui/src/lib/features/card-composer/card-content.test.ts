import { describe, expect, it } from 'vitest';
import type { DictionaryEntrySummaryV1 } from '../../contracts/generated';
import { formatDictionaryDefinition, preferredReading } from './card-content';

const entry: DictionaryEntrySummaryV1 = {
  entry_id: 'entry-1',
  headwords: ['見る'],
  readings: ['みる'],
  pitch_accents: [],
  jlpt_level: null,
  jlpt_source: null,
  match_reason: 'exact lemma',
  priority_score: 100,
  senses: [
    {
      glosses: ['to see', 'to look'],
      parts_of_speech: ['Ichidan verb'],
      restrictions: [],
      fields: [],
      dialects: [],
      misc: ['common'],
    },
    {
      glosses: ['to examine'],
      parts_of_speech: ['transitive verb'],
      restrictions: [],
      fields: [],
      dialects: [],
      misc: [],
    },
  ],
};

describe('word card content', () => {
  it('keeps every numbered dictionary sense and its useful labels', () => {
    expect(formatDictionaryDefinition(entry)).toBe(
      '1. (Ichidan verb, common) to see; to look\n2. (transitive verb) to examine',
    );
  });

  it('uses the dictionary reading and falls back to the tokenizer reading', () => {
    expect(preferredReading('ミル', entry)).toBe('みる');
    expect(preferredReading('ミル', null)).toBe('みる');
    expect(preferredReading('ミエル', { ...entry, readings: ['みる', 'みえる'] })).toBe('みえる');
  });

  it('leaves missing definitions explicit instead of inventing one', () => {
    expect(formatDictionaryDefinition(null)).toBe('');
  });
});
