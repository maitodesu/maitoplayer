import type { MediaSessionV1, SubtitleSourceV1 } from '../contracts/generated';

export type PlaylistScanModeV1 = 'auto_import' | 'preview';

export type PlaylistWatchStatusV1 = 'watching' | 'unavailable';

export type SubtitleRoleV1 = 'japanese' | 'translation';

export type PlaylistSubtitleKindV1 = 'auto' | 'override' | 'ambiguous' | 'none';

export interface PlaylistSubtitleV1 {
  kind: PlaylistSubtitleKindV1;
  display_name: string | null;
  confidence: number | null;
  reason: string | null;
}

export interface PlaylistItemV1 {
  item_id: string;
  display_name: string;
  ordinal: number;
  japanese_subtitle: PlaylistSubtitleV1;
  translation_subtitle: PlaylistSubtitleV1;
}

export interface PlaylistWatchStateV1 {
  status: PlaylistWatchStatusV1;
  last_scan_unix_ms: number;
}

export interface PlaylistV1 {
  playlist_id: string;
  display_name: string;
  revision: number;
  scan_revision: number;
  change_id: string;
  items: PlaylistItemV1[];
  current_item_id: string | null;
  watch_state: PlaylistWatchStateV1;
}

export interface PlaylistDiscoveryItemV1 {
  item_id: string;
  display_name: string;
}

export interface PlaylistScanResultV1 {
  playlist: PlaylistV1;
  pending_items: PlaylistDiscoveryItemV1[];
}

export interface PlaylistSelectionV1 {
  playlist: PlaylistV1;
  session: MediaSessionV1;
  japanese_subtitle: SubtitleSourceV1 | null;
  translation_subtitle: SubtitleSourceV1 | null;
}

export interface PlaylistSubtitleMutationV1 {
  playlist: PlaylistV1;
  source: SubtitleSourceV1 | null;
}

export interface PlaylistViewState {
  playlist: PlaylistV1;
  pendingItems: PlaylistDiscoveryItemV1[];
}

export type PlaylistMoveDirection = -1 | 1;

export function playlistMoveTarget(
  items: PlaylistItemV1[],
  itemId: string,
  direction: PlaylistMoveDirection,
): number | null {
  const index = items.findIndex((item) => item.item_id === itemId);
  if (index < 0) return null;
  const target = index + direction;
  return target >= 0 && target < items.length ? target : null;
}

export function subtitleSummary(subtitle: PlaylistSubtitleV1): string {
  if (subtitle.kind === 'override') {
    return subtitle.display_name ? `Override · ${subtitle.display_name}` : 'Override';
  }
  if (subtitle.kind === 'auto') {
    return subtitle.display_name ? `Auto · ${subtitle.display_name}` : 'Automatic match';
  }
  if (subtitle.kind === 'ambiguous') return 'Ambiguous match · choose a file';
  return subtitle.reason ? `Missing · ${subtitle.reason}` : 'Missing';
}

export const PLAYLIST_SCAN_MODE_STORAGE_KEY = 'migaku.playlist-scan-mode.v1';

interface StorageReader {
  getItem(key: string): string | null;
}

interface StorageWriter {
  setItem(key: string, value: string): void;
}

export function loadPlaylistScanMode(storage: StorageReader): PlaylistScanModeV1 {
  try {
    return storage.getItem(PLAYLIST_SCAN_MODE_STORAGE_KEY) === 'auto_import'
      ? 'auto_import'
      : 'preview';
  } catch {
    return 'preview';
  }
}

export function savePlaylistScanMode(storage: StorageWriter, scanMode: PlaylistScanModeV1): void {
  try {
    storage.setItem(PLAYLIST_SCAN_MODE_STORAGE_KEY, scanMode);
  } catch {
    // A denied storage write must not disable folder playback.
  }
}

export function retainPendingDiscoveryIds(
  selectedIds: string[],
  pendingItems: PlaylistDiscoveryItemV1[],
): string[] {
  const available = new Set(pendingItems.map((item) => item.item_id));
  return selectedIds.filter((id) => available.has(id));
}

export function reconcilePlaylistScan(
  current: PlaylistV1 | null,
  result: PlaylistScanResultV1,
): PlaylistViewState {
  const unchanged =
    current !== null &&
    current.change_id === result.playlist.change_id &&
    current.watch_state.status === result.playlist.watch_state.status;
  return {
    playlist: unchanged ? current : result.playlist,
    pendingItems: result.pending_items,
  };
}

export function remainingPlaylistDiscoveries(
  pendingItems: PlaylistDiscoveryItemV1[],
  importedItemIds: string[],
): PlaylistDiscoveryItemV1[] {
  const imported = new Set(importedItemIds);
  return pendingItems.filter((item) => !imported.has(item.item_id));
}

export function playlistSelectionPlayback(selection: PlaylistSelectionV1) {
  return {
    session: selection.session,
    subtitles: {
      japanese: selection.japanese_subtitle,
      translation: selection.translation_subtitle,
    },
  };
}
