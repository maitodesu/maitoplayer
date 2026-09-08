import { describe, expect, it } from 'vitest';
import { formatTime, isCueActive, secondsToMicroseconds } from './time';

describe('media time', () => {
  it('converts browser seconds only at the IPC boundary', () => {
    expect(secondsToMicroseconds(1.234567)).toBe(1_234_567);
    expect(secondsToMicroseconds(Number.NaN)).toBe(0);
  });

  it('uses half-open subtitle intervals', () => {
    expect(isCueActive(10, 20, 10)).toBe(true);
    expect(isCueActive(10, 20, 20)).toBe(false);
  });

  it('formats long media positions', () => {
    expect(formatTime(3_661_000_000)).toBe('1:01:01');
  });
});
