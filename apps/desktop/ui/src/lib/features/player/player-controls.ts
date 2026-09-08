export const PLAYER_CONTROLS_HIDE_DELAY_MS = 2_700;

export interface PlayerControlsContext {
  active: boolean;
  playing: boolean;
  controlsHovered: boolean;
  controlsFocused: boolean;
  scrubbing: boolean;
  menuOpen: boolean;
  blockingStatus: boolean;
}

export type MediaShortcutAction =
  'toggle-playback' | 'seek-backward' | 'seek-forward' | 'toggle-mute' | 'toggle-fullscreen';

export type ShortcutTargetKind = 'plain' | 'editing' | 'range' | 'player-control' | 'other-control';

interface MediaShortcutInput {
  code: string;
  key: string;
  fullscreen: boolean;
  targetKind: ShortcutTargetKind;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
}

export function canAutoHidePlayerControls(context: PlayerControlsContext): boolean {
  return (
    context.active &&
    context.playing &&
    !context.controlsHovered &&
    !context.controlsFocused &&
    !context.scrubbing &&
    !context.menuOpen &&
    !context.blockingStatus
  );
}

export function mediaShortcutAction(input: MediaShortcutInput): MediaShortcutAction | null {
  if (input.ctrlKey || input.metaKey || input.altKey) return null;
  if (input.targetKind === 'editing' || input.targetKind === 'range') return null;
  if (input.targetKind === 'other-control') return null;
  if (input.targetKind === 'player-control' && !input.fullscreen && input.code === 'Space') {
    return null;
  }

  if (input.code === 'Space' || input.code === 'KeyK') return 'toggle-playback';
  if (input.code === 'ArrowLeft') return 'seek-backward';
  if (input.code === 'ArrowRight') return 'seek-forward';
  if (input.code === 'KeyM') return 'toggle-mute';
  if (input.code === 'KeyF' || input.key.toLocaleLowerCase() === 'f') return 'toggle-fullscreen';
  return null;
}

export function shortcutTargetKind(target: EventTarget | null): ShortcutTargetKind {
  if (!(target instanceof Element)) return 'plain';
  if (target.closest('textarea, select, [contenteditable="true"], input:not([type="range"])')) {
    return 'editing';
  }
  if (target.closest('input[type="range"], [role="slider"]')) return 'range';
  if (target.closest('[data-player-control]')) return 'player-control';
  if (target.closest('button, summary, a[href], [role="button"]')) return 'other-control';
  return 'plain';
}
