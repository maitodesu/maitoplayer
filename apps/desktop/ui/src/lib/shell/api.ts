import { invoke } from '@tauri-apps/api/core';
import type {
  AppErrorV1,
  AppHealthV1,
  CardDraftV1,
  CreateCardRequestV1,
  CreateCardResultV1,
  DecodeTestOutcomeV1,
  MediaSessionId,
  MediaSessionV1,
  PlaybackCapabilityReportV1,
  PlaybackCheckpointV1,
  RecentMediaV1,
  SubtitleDiscoveryV1,
  SubtitleSourceId,
  SubtitleSourceV1,
  SubtitleWindowV1,
  TranslationWindowV1,
  UserSettingsV1,
} from '../contracts/generated';
import type {
  PlaylistScanModeV1,
  PlaylistScanResultV1,
  PlaylistSelectionV1,
  PlaylistSubtitleMutationV1,
  PlaylistV1,
  SubtitleRoleV1,
} from './playlist';

export interface DraftSelection {
  session_id: MediaSessionId;
  subtitle_source_id: SubtitleSourceId;
  cue_id: string;
  token_id: string;
  dictionary_entry_id?: string | null;
  observed_playback_time_us: number;
}

export function isDesktop(): boolean {
  return typeof window !== 'undefined' && window.__TAURI_INTERNALS__ !== undefined;
}

export function asAppError(error: unknown): AppErrorV1 {
  if (typeof error === 'object' && error !== null && 'code' in error && 'message' in error) {
    const candidate = error as Partial<AppErrorV1>;
    return {
      code: String(candidate.code),
      message: String(candidate.message),
      retryable: Boolean(candidate.retryable),
      diagnostics: candidate.diagnostics ?? null,
    };
  }
  return {
    code: 'UNEXPECTED_ERROR',
    message: error instanceof Error ? error.message : 'An unexpected error occurred.',
    retryable: true,
    diagnostics: null,
  };
}

async function desktopInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isDesktop()) {
    throw {
      code: 'DESKTOP_REQUIRED',
      message: 'Run the Tauri desktop application to access local files and media services.',
      retryable: false,
      diagnostics: null,
    } satisfies AppErrorV1;
  }
  return invoke<T>(command, args);
}

export const api = {
  importVideo: () => desktopInvoke<MediaSessionV1 | null>('choose_and_import_media'),
  importPlaylistFolder: (scanMode: PlaylistScanModeV1) =>
    desktopInvoke<PlaylistScanResultV1 | null>('choose_and_import_playlist_folder', { scanMode }),
  currentPlaylist: () => desktopInvoke<PlaylistV1 | null>('current_playlist'),
  rescanPlaylist: (scanMode: PlaylistScanModeV1) =>
    desktopInvoke<PlaylistScanResultV1>('rescan_playlist', { scanMode }),
  selectPlaylistItem: (itemId: string) =>
    desktopInvoke<PlaylistSelectionV1>('select_playlist_item', { itemId }),
  reorderPlaylistItem: (itemId: string, newIndex: number) =>
    desktopInvoke<PlaylistV1>('reorder_playlist_item', { itemId, newIndex }),
  removePlaylistItem: (itemId: string) =>
    desktopInvoke<PlaylistV1>('remove_playlist_item', { itemId }),
  importPlaylistDiscoveries: (itemIds: string[]) =>
    desktopInvoke<PlaylistV1>('import_playlist_discoveries', { itemIds }),
  choosePlaylistSubtitleOverride: (
    itemId: string,
    role: SubtitleRoleV1,
    sessionId: MediaSessionId | null,
  ) =>
    desktopInvoke<PlaylistSubtitleMutationV1 | null>('choose_playlist_subtitle_override', {
      itemId,
      role,
      sessionId,
    }),
  clearPlaylistSubtitleOverride: (
    itemId: string,
    role: SubtitleRoleV1,
    sessionId: MediaSessionId | null,
  ) =>
    desktopInvoke<PlaylistSubtitleMutationV1>('clear_playlist_subtitle_override', {
      itemId,
      role,
      sessionId,
    }),
  reportCapabilities: (sessionId: MediaSessionId, report: PlaybackCapabilityReportV1) =>
    desktopInvoke<MediaSessionV1>('report_playback_capabilities', {
      sessionId,
      report,
    }),
  reportDecodeOutcome: (sessionId: MediaSessionId, outcome: DecodeTestOutcomeV1) =>
    desktopInvoke<MediaSessionV1>('report_decode_outcome', { sessionId, outcome }),
  preparePlayback: (sessionId: MediaSessionId, approvedVideoTranscode: boolean) =>
    desktopInvoke<MediaSessionV1>('prepare_playback', {
      sessionId,
      approvedVideoTranscode,
    }),
  cancelConversion: (sessionId: MediaSessionId) =>
    desktopInvoke<boolean>('cancel_conversion', { sessionId }),
  conversionProgress: (sessionId: MediaSessionId) =>
    desktopInvoke<number | null>('conversion_progress', { sessionId }),
  miningProgress: (sessionId: MediaSessionId) =>
    desktopInvoke<number | null>('mining_progress', { sessionId }),
  cancelMining: (sessionId: MediaSessionId) =>
    desktopInvoke<boolean>('cancel_mining', { sessionId }),
  selectAudioStream: (sessionId: MediaSessionId, streamId: string) =>
    desktopInvoke<MediaSessionV1>('select_audio_stream', { sessionId, streamId }),
  checkpoint: (checkpoint: PlaybackCheckpointV1) =>
    desktopInvoke<boolean>('send_playback_checkpoint', { checkpoint }),
  chooseSubtitle: (sessionId: MediaSessionId) =>
    desktopInvoke<SubtitleSourceV1 | null>('choose_subtitle', { sessionId }),
  chooseTranslationSubtitle: (sessionId: MediaSessionId) =>
    desktopInvoke<SubtitleSourceV1 | null>('choose_translation_subtitle', { sessionId }),
  discoverSubtitles: (sessionId: MediaSessionId) =>
    desktopInvoke<SubtitleDiscoveryV1>('discover_subtitles', { sessionId }),
  useDiscoveredSubtitle: (sessionId: MediaSessionId, candidateId: string) =>
    desktopInvoke<SubtitleSourceV1>('use_discovered_subtitle', { sessionId, candidateId }),
  restoreSubtitleBinding: (sessionId: MediaSessionId) =>
    desktopInvoke<SubtitleSourceV1 | null>('restore_subtitle_binding', { sessionId }),
  restoreTranslationSubtitleBinding: (sessionId: MediaSessionId) =>
    desktopInvoke<SubtitleSourceV1 | null>('restore_translation_subtitle_binding', { sessionId }),
  useEmbeddedSubtitle: (sessionId: MediaSessionId, streamId: string) =>
    desktopInvoke<SubtitleSourceV1>('use_embedded_subtitle', { sessionId, streamId }),
  useEmbeddedTranslationSubtitle: (sessionId: MediaSessionId, streamId: string) =>
    desktopInvoke<SubtitleSourceV1>('use_embedded_translation_subtitle', {
      sessionId,
      streamId,
    }),
  removeTranslationSubtitle: (sessionId: MediaSessionId, sourceId: SubtitleSourceId) =>
    desktopInvoke<void>('remove_translation_subtitle', { sessionId, sourceId }),
  subtitleWindow: (
    sessionId: MediaSessionId,
    sourceId: SubtitleSourceId,
    positionUs: number,
    revision: number,
  ) =>
    desktopInvoke<SubtitleWindowV1>('subtitle_window', {
      sessionId,
      sourceId,
      positionUs,
      revision,
    }),
  translationWindow: (
    sessionId: MediaSessionId,
    sourceId: SubtitleSourceId,
    positionUs: number,
    revision: number,
  ) =>
    desktopInvoke<TranslationWindowV1>('translation_window', {
      sessionId,
      sourceId,
      positionUs,
      revision,
    }),
  createDraft: (selection: DraftSelection) =>
    desktopInvoke<CardDraftV1>('create_draft', { selection }),
  publishCard: (request: CreateCardRequestV1) =>
    desktopInvoke<CreateCardResultV1>('publish_card', { request }),
  closeSession: (sessionId: MediaSessionId) =>
    desktopInvoke<void>('close_media_session', { sessionId }),
  rememberMedia: (
    sessionId: MediaSessionId,
    subtitleSourceId?: SubtitleSourceId | null,
    translationSourceId?: SubtitleSourceId | null,
  ) =>
    desktopInvoke<MediaSessionV1>('remember_media', {
      sessionId,
      subtitleSourceId,
      translationSourceId,
    }),
  recentMedia: () => desktopInvoke<RecentMediaV1[]>('recent_media'),
  reopenRecentMedia: (sourceFingerprint: string) =>
    desktopInvoke<MediaSessionV1>('reopen_recent_media', { sourceFingerprint }),
  revokeRecentMedia: (sourceFingerprint: string) =>
    desktopInvoke<boolean>('revoke_recent_media', { sourceFingerprint }),
  health: () => desktopInvoke<AppHealthV1>('health'),
  settings: () => desktopInvoke<UserSettingsV1>('settings'),
  saveSettings: (settings: UserSettingsV1) =>
    desktopInvoke<UserSettingsV1>('save_settings', { settings }),
};
