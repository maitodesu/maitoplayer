import { describe, expect, it } from 'vitest';
import { calculatePopoverLayout } from './popover-layout';

describe('calculatePopoverLayout', () => {
  it('keeps a tall windowed-player popover inside the viewport above the subtitle', () => {
    const layout = calculatePopoverLayout({
      viewportWidth: 849,
      viewportHeight: 566,
      anchor: { left: 405, top: 490, bottom: 530, width: 180 },
      desiredHeight: 900,
    });

    expect(layout).toEqual({
      left: 255,
      top: 30,
      width: 480,
      maxHeight: 448,
      placement: 'above',
    });
    expect(layout.top + layout.maxHeight).toBeLessThanOrEqual(490 - 12);
  });

  it('uses and sizes to the larger side when neither side fits', () => {
    const layout = calculatePopoverLayout({
      viewportWidth: 800,
      viewportHeight: 600,
      anchor: { left: 320, top: 220, bottom: 248, width: 160 },
      desiredHeight: 500,
    });

    expect(layout.placement).toBe('below');
    expect(layout.top).toBe(260);
    expect(layout.maxHeight).toBe(324);
    expect(layout.top + layout.maxHeight).toBe(584);
  });

  it('shrinks on narrow viewports without crossing the safe margins', () => {
    const layout = calculatePopoverLayout({
      viewportWidth: 300,
      viewportHeight: 500,
      anchor: { left: 120, top: 400, bottom: 430, width: 60 },
      desiredHeight: 250,
    });

    expect(layout.width).toBe(268);
    expect(layout.left).toBe(16);
    expect(layout.left + layout.width).toBe(284);
  });
});
