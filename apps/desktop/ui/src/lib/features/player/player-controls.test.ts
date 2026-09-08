import { describe, expect, it } from 'vitest';
import { canAutoHidePlayerControls, mediaShortcutAction } from './player-controls';

const idleContext = {
  active: true,
  playing: true,
  controlsHovered: false,
  controlsFocused: false,
  scrubbing: false,
  menuOpen: false,
  blockingStatus: false,
};

describe('player control visibility', () => {
  it('auto-hides only during uninterrupted active playback', () => {
    expect(canAutoHidePlayerControls(idleContext)).toBe(true);
    for (const override of [
      { active: false },
      { playing: false },
      { controlsHovered: true },
      { controlsFocused: true },
      { scrubbing: true },
      { menuOpen: true },
      { blockingStatus: true },
    ]) {
      expect(canAutoHidePlayerControls({ ...idleContext, ...override })).toBe(false);
    }
  });
});

describe('global media keyboard semantics', () => {
  it('maps the global playback shortcuts', () => {
    expect(
      mediaShortcutAction({ code: 'Space', key: ' ', fullscreen: false, targetKind: 'plain' }),
    ).toBe('toggle-playback');
    expect(
      mediaShortcutAction({ code: 'KeyK', key: 'k', fullscreen: false, targetKind: 'plain' }),
    ).toBe('toggle-playback');
    expect(
      mediaShortcutAction({
        code: 'ArrowLeft',
        key: 'ArrowLeft',
        fullscreen: false,
        targetKind: 'plain',
      }),
    ).toBe('seek-backward');
    expect(
      mediaShortcutAction({
        code: 'ArrowRight',
        key: 'ArrowRight',
        fullscreen: false,
        targetKind: 'plain',
      }),
    ).toBe('seek-forward');
    expect(
      mediaShortcutAction({ code: 'KeyM', key: 'm', fullscreen: false, targetKind: 'plain' }),
    ).toBe('toggle-mute');
    expect(
      mediaShortcutAction({ code: 'KeyF', key: 'f', fullscreen: false, targetKind: 'plain' }),
    ).toBe('toggle-fullscreen');
  });

  it('lets fullscreen shortcuts win while a player button is focused', () => {
    expect(
      mediaShortcutAction({
        code: 'Space',
        key: ' ',
        fullscreen: true,
        targetKind: 'player-control',
      }),
    ).toBe('toggle-playback');
    expect(
      mediaShortcutAction({
        code: 'KeyF',
        key: 'f',
        fullscreen: true,
        targetKind: 'player-control',
      }),
    ).toBe('toggle-fullscreen');
  });

  it('keeps normal button activation outside fullscreen', () => {
    expect(
      mediaShortcutAction({
        code: 'Space',
        key: ' ',
        fullscreen: false,
        targetKind: 'player-control',
      }),
    ).toBeNull();
    expect(
      mediaShortcutAction({
        code: 'KeyK',
        key: 'k',
        fullscreen: false,
        targetKind: 'player-control',
      }),
    ).toBe('toggle-playback');
  });

  it('protects editing, sliders, other controls, and modified shortcuts', () => {
    for (const targetKind of ['editing', 'range', 'other-control'] as const) {
      expect(
        mediaShortcutAction({ code: 'Space', key: ' ', fullscreen: true, targetKind }),
      ).toBeNull();
    }
    expect(
      mediaShortcutAction({
        code: 'Space',
        key: ' ',
        fullscreen: true,
        targetKind: 'plain',
        ctrlKey: true,
      }),
    ).toBeNull();
  });
});
