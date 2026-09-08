import { describe, expect, it, vi } from 'vitest';
import { createCueBoundaryCache } from './cue-boundary-cache';

interface Cue {
  id: string;
  start_us: number;
  end_us: number;
}

describe('cue boundary cache', () => {
  it('reuses a selection between boundaries and refreshes exactly at half-open edges', () => {
    const cues: Cue[] = [
      { id: 'first', start_us: 10, end_us: 20 },
      { id: 'second', start_us: 20, end_us: 30 },
    ];
    const select = vi.fn((items: Cue[], positionUs: number) =>
      items.filter((cue) => cue.start_us <= positionUs && positionUs < cue.end_us),
    );
    const cached = createCueBoundaryCache((cue: Cue) => cue, select);

    const first = cached(cues, 12);
    expect(cached(cues, 19)).toBe(first);
    expect(select).toHaveBeenCalledTimes(1);

    const second = cached(cues, 20);
    expect(second.map((cue) => cue.id)).toEqual(['second']);
    expect(second).not.toBe(first);
    expect(select).toHaveBeenCalledTimes(2);
  });

  it('recomputes after backward seeks and source-window replacement', () => {
    const cues: Cue[] = [{ id: 'long', start_us: 10, end_us: 30 }];
    const select = vi.fn((items: Cue[], positionUs: number) =>
      items.filter((cue) => cue.start_us <= positionUs && positionUs < cue.end_us),
    );
    const cached = createCueBoundaryCache((cue: Cue) => cue, select);

    cached(cues, 25);
    cached(cues, 15);
    expect(select).toHaveBeenCalledTimes(1);
    cached(cues, 5);
    expect(select).toHaveBeenCalledTimes(2);
    cached([...cues], 5);
    expect(select).toHaveBeenCalledTimes(3);
  });
});
