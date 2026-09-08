import { describe, expect, it } from 'vitest';
import { readingToHiragana } from './reading';

describe('readingToHiragana', () => {
  it('converts tokenizer katakana readings to hiragana', () => {
    expect(readingToHiragana('ニホンゴ')).toBe('にほんご');
    expect(readingToHiragana('ミル')).toBe('みる');
  });

  it('preserves punctuation, long vowel marks, latin text, and existing hiragana', () => {
    expect(readingToHiragana('スーパー・でABC')).toBe('すーぱー・でABC');
  });

  it('supports voiced kana and iteration marks', () => {
    expect(readingToHiragana('ヴヽヾ')).toBe('ゔゝゞ');
  });

  it('normalizes half-width and uncommon phonetic katakana', () => {
    expect(readingToHiragana('ｶﾞｯｺｳ')).toBe('がっこう');
    expect(readingToHiragana('ヷヸヹヺヿ')).toBe('わ゙ゐ゙ゑ゙を゙こと');
    expect(readingToHiragana('ㇰㇷ゚')).toBe('くぷ');
  });
});
