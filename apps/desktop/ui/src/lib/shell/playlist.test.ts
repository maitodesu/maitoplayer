import { describe, expect, it } from 'vitest';
import {
  loadPlaylistScanMode,
  playlistMoveTarget,
  playlistSelectionPlayback,
  reconcilePlaylistScan,
  remainingPlaylistDiscoveries,
  retainPendingDiscoveryIds,
  savePlaylistScanMode,
  subtitleSummary,
  type PlaylistItemV1,
} from './playlist';
import type { MediaSessionV1 } from '../contracts/generated';

const subtitle = {
  kind: 'none' as const,
  display_name: null,
  confidence: null,
  reason: 'No adjacent match',
};
const items: PlaylistItemV1[] = ['a', 'b', 'c'].map((itemId, ordinal) => ({
  item_id: itemId,
  display_name: `${itemId}.mkv`,
  ordinal,
  japanese_subtitle: subtitle,
  translation_subtitle: subtitle,
}));

describe('playlist view state', () => {
  it('returns zero-based reorder targets and rejects boundary moves', () => {
    expect(playlistMoveTarget(items, 'b', -1)).toBe(0);
    expect(playlistMoveTarget(items, 'b', 1)).toBe(2);
    expect(playlistMoveTarget(items, 'a', -1)).toBeNull();
    expect(playlistMoveTarget(items, 'c', 1)).toBeNull();
    expect(playlistMoveTarget(items, 'missing', 1)).toBeNull();
  });

  it('describes automatic, override, and missing subtitle choices', () => {
    expect(
      subtitleSummary({
        kind: 'auto',
        display_name: 'episode.ja.srt',
        confidence: 96,
        reason: 'Exact episode match',
      }),
    ).toBe('Auto · episode.ja.srt');
    expect(
      subtitleSummary({
        kind: 'override',
        display_name: 'picked.ass',
        confidence: 100,
        reason: 'Manual override',
      }),
    ).toBe('Override · picked.ass');
    expect(
      subtitleSummary({
        kind: 'ambiguous',
        display_name: null,
        confidence: 70,
        reason: 'Two equal episode matches',
      }),
    ).toBe('Ambiguous match · choose a file');
    expect(
      subtitleSummary({
        kind: 'none',
        display_name: null,
        confidence: null,
        reason: 'No adjacent match',
      }),
    ).toBe('Missing · No adjacent match');
  });

  it('drops selected discoveries that disappeared during reconciliation', () => {
    expect(
      retainPendingDiscoveryIds(
        ['gone', 'new-2'],
        [
          { item_id: 'new-1', display_name: 'One.mp4' },
          { item_id: 'new-2', display_name: 'Two.mp4' },
        ],
      ),
    ).toEqual(['new-2']);
  });

  it('persists auto-import preference and defaults invalid values to preview', () => {
    let stored: string | null = null;
    const storage = {
      getItem: () => stored,
      setItem: (_key: string, value: string) => (stored = value),
    };
    expect(loadPlaylistScanMode(storage)).toBe('preview');
    savePlaylistScanMode(storage, 'auto_import');
    expect(loadPlaylistScanMode(storage)).toBe('auto_import');
    stored = 'unexpected';
    expect(loadPlaylistScanMode(storage)).toBe('preview');
  });

  it('keeps a preview-only folder actionable until selecting an imported item starts playback', () => {
    const emptyPlaylist = {
      playlist_id: 'folder-1',
      display_name: 'Season 1',
      revision: 1,
      scan_revision: 1,
      change_id: 'empty',
      items: [],
      current_item_id: null,
      watch_state: { status: 'watching' as const, last_scan_unix_ms: 10 },
    };
    const preview = reconcilePlaylistScan(null, {
      playlist: emptyPlaylist,
      pending_items: [{ item_id: 'episode-1', display_name: 'Episode 01.mkv' }],
    });
    expect(preview.playlist).toBe(emptyPlaylist);
    expect(preview.pendingItems).toHaveLength(1);

    const importedPlaylist = {
      ...emptyPlaylist,
      revision: 2,
      change_id: 'episode-added',
      items: [{ ...items[0]!, item_id: 'episode-1', display_name: 'Episode 01.mkv' }],
    };
    const pendingAfterImport = remainingPlaylistDiscoveries(preview.pendingItems, ['episode-1']);
    expect(importedPlaylist.items).toHaveLength(1);
    expect(pendingAfterImport).toEqual([]);

    const session = { session_id: 'session-episode-1' } as MediaSessionV1;
    const playback = playlistSelectionPlayback({
      playlist: { ...importedPlaylist, current_item_id: 'episode-1' },
      session,
      japanese_subtitle: null,
      translation_subtitle: null,
    });
    expect(playback.session.session_id).toBe('session-episode-1');
    expect(playback.subtitles).toEqual({ japanese: null, translation: null });
  });
});
