<script lang="ts">
  import { onMount } from 'svelte';
  import type {
    AppErrorV1,
    AppHealthV1,
    MediaSessionV1,
    RecentMediaV1,
    SubtitleCandidateV1,
    SubtitleSourceV1,
    SubtitleWindowV1,
    TranslationWindowV1,
  } from './lib/contracts/generated';
  import CardComposer from './lib/features/card-composer/CardComposer.svelte';
  import DiagnosticsPanel from './lib/features/diagnostics/DiagnosticsPanel.svelte';
  import LiveSubtitle from './lib/features/live-subtitle/LiveSubtitle.svelte';
  import { createCueBoundaryCache } from './lib/features/live-subtitle/cue-boundary-cache';
  import {
    containsSelection,
    type SubtitleTokenSelection,
  } from './lib/features/live-subtitle/selection';
  import { visibleTranslationCues } from './lib/features/live-subtitle/translation-sync';
  import {
    isEditableOrMediaControl,
    loadSubtitleVisibility,
    saveSubtitleVisibility,
    subtitleVisibilityShortcut,
    type SubtitleVisibilityControl,
  } from './lib/features/live-subtitle/subtitle-visibility';
  import Player from './lib/features/player/Player.svelte';
  import SettingsPanel from './lib/features/settings/SettingsPanel.svelte';
  import ThemePicker from './lib/features/theme/ThemePicker.svelte';
  import { api, asAppError, isDesktop } from './lib/shell/api';
  import PlaylistPanel from './lib/shell/PlaylistPanel.svelte';
  import {
    loadPlaylistScanMode,
    playlistSelectionPlayback,
    reconcilePlaylistScan,
    remainingPlaylistDiscoveries,
    savePlaylistScanMode,
    type PlaylistDiscoveryItemV1,
    type PlaylistScanModeV1,
    type PlaylistSubtitleMutationV1,
    type PlaylistV1,
    type SubtitleRoleV1,
  } from './lib/shell/playlist';
  import { createPlaylistWatcher, type PlaylistWatcher } from './lib/shell/playlist-watch';
  import { isCueActive } from './lib/shell/time';

  type Tab = 'watch' | 'settings' | 'diagnostics';
  let tab = $state<Tab>('watch');
  let session = $state<MediaSessionV1 | null>(null);
  let subtitleSource = $state<SubtitleSourceV1 | null>(null);
  let subtitleWindow = $state<SubtitleWindowV1 | null>(null);
  let translationSource = $state<SubtitleSourceV1 | null>(null);
  let translationWindow = $state<TranslationWindowV1 | null>(null);
  let selection = $state<SubtitleTokenSelection | null>(null);
  let health = $state<AppHealthV1 | null>(null);
  let recents = $state<RecentMediaV1[]>([]);
  let playlist = $state<PlaylistV1 | null>(null);
  let pendingPlaylistItems = $state<PlaylistDiscoveryItemV1[]>([]);
  let playlistScanMode = $state<PlaylistScanModeV1>('preview');
  let playlistScanModeReady = $state(false);
  let playlistActionBusy = $state(false);
  let playlistScanBusy = $state(false);
  let playlistWatcher = $state<PlaylistWatcher | null>(null);
  let adjacentCandidates = $state<SubtitleCandidateV1[]>([]);
  let currentTimeUs = $state(0);
  let error = $state<AppErrorV1 | null>(null);
  let busy = $state(false);
  let windowBusy = false;
  let windowRevision = 0;
  let pendingWindowPositionUs: number | null = null;
  let translationWindowBusy = false;
  let translationWindowRevision = 0;
  let pendingTranslationWindowPositionUs: number | null = null;
  let playbackExperience = $state<HTMLElement | null>(null);
  let showFurigana = $state(true);
  let showJapanese = $state(true);
  let showEnglish = $state(true);
  let visibilityReady = $state(false);

  const emptyAnalyzedCues: SubtitleWindowV1['cues'] = [];
  const emptyTranslationCues: TranslationWindowV1['cues'] = [];
  const selectActiveCues = createCueBoundaryCache(
    (item: SubtitleWindowV1['cues'][number]) => item.cue,
    (items, positionUs) =>
      items.filter((item) => isCueActive(item.cue.start_us, item.cue.end_us, positionUs)),
  );
  const selectActiveTranslationCues = createCueBoundaryCache(
    (item: TranslationWindowV1['cues'][number]) => item,
    visibleTranslationCues,
  );

  const activeCues = $derived(
    selectActiveCues(subtitleWindow?.cues ?? emptyAnalyzedCues, currentTimeUs),
  );
  const activeTranslationCues = $derived(
    selectActiveTranslationCues(translationWindow?.cues ?? emptyTranslationCues, currentTimeUs),
  );

  $effect(() => {
    if (!selection) return;
    if (!containsSelection(activeCues, selection)) selection = null;
  });

  $effect(() => {
    if (!showJapanese && selection) selection = null;
  });

  $effect(() => {
    if (!visibilityReady) return;
    saveSubtitleVisibility(localStorage, {
      furigana: showFurigana,
      japanese: showJapanese,
      english: showEnglish,
    });
  });

  $effect(() => {
    playlistWatcher?.setActive(tab === 'watch' && playlist !== null);
  });

  $effect(() => {
    if (playlistScanModeReady) savePlaylistScanMode(localStorage, playlistScanMode);
  });

  onMount(() => {
    const visibility = loadSubtitleVisibility(localStorage);
    showFurigana = visibility.furigana;
    showJapanese = visibility.japanese;
    showEnglish = visibility.english;
    visibilityReady = true;
    playlistScanMode = loadPlaylistScanMode(localStorage);
    playlistScanModeReady = true;
    const watcher = createPlaylistWatcher({ scan: scanPlaylist });
    playlistWatcher = watcher;
    if (isDesktop()) {
      void Promise.all([api.health(), api.recentMedia(), api.currentPlaylist()])
        .then(([healthValue, recentValue, playlistValue]) => {
          health = healthValue;
          recents = recentValue;
          playlist = playlistValue;
        })
        .catch((reason) => (error = asAppError(reason)));
    }
    return () => watcher.stop();
  });

  async function importVideo(): Promise<void> {
    busy = true;
    error = null;
    try {
      const imported = await api.importVideo();
      if (!imported) return;
      await activateSession(imported);
      if (playlist) playlist = { ...playlist, current_item_id: null };
    } catch (reason) {
      error = asAppError(reason);
    } finally {
      busy = false;
    }
  }

  async function importPlaylistFolder(): Promise<void> {
    playlistActionBusy = true;
    error = null;
    try {
      const imported = await api.importPlaylistFolder(playlistScanMode);
      if (!imported) return;
      const next = reconcilePlaylistScan(null, imported);
      playlist = next.playlist;
      pendingPlaylistItems = next.pendingItems;
      tab = 'watch';
    } catch (reason) {
      error = asAppError(reason);
    } finally {
      playlistActionBusy = false;
    }
  }

  async function activateSession(
    imported: MediaSessionV1,
    playlistSubtitles?: {
      japanese: SubtitleSourceV1 | null;
      translation: SubtitleSourceV1 | null;
    },
  ): Promise<void> {
    if (session) await api.closeSession(session.session_id);
    session = imported;
    subtitleSource = null;
    subtitleWindow = null;
    translationSource = null;
    translationWindow = null;
    selection = null;
    adjacentCandidates = [];
    currentTimeUs = 0;
    pendingWindowPositionUs = null;
    pendingTranslationWindowPositionUs = null;
    windowRevision += 1;
    translationWindowRevision += 1;
    tab = 'watch';
    if (playlistSubtitles) {
      subtitleSource = playlistSubtitles.japanese;
      translationSource = playlistSubtitles.translation;
      await Promise.all([
        subtitleSource ? loadSubtitleWindow(0) : Promise.resolve(),
        translationSource ? loadTranslationWindow(0) : Promise.resolve(),
      ]);
      return;
    }
    const [restored, restoredTranslation] = await Promise.all([
      api.restoreSubtitleBinding(imported.session_id),
      api.restoreTranslationSubtitleBinding(imported.session_id),
    ]);
    if (restoredTranslation) translationSource = restoredTranslation;
    if (restored) {
      subtitleSource = restored;
      await Promise.all([loadSubtitleWindow(0), loadTranslationWindow(0)]);
      return;
    }
    if (restoredTranslation) await loadTranslationWindow(0);
    const discovery = await api.discoverSubtitles(imported.session_id);
    adjacentCandidates = discovery.candidates;
    if (discovery.recommended_candidate_id) {
      await useDiscoveredSubtitle(discovery.recommended_candidate_id);
    }
  }

  async function scanPlaylist(): Promise<void> {
    if (!playlist || playlistActionBusy || playlistScanBusy) return;
    playlistScanBusy = true;
    try {
      const result = await api.rescanPlaylist(playlistScanMode);
      const next = reconcilePlaylistScan(playlist, result);
      playlist = next.playlist;
      pendingPlaylistItems = next.pendingItems;
    } catch (reason) {
      error = asAppError(reason);
    } finally {
      playlistScanBusy = false;
    }
  }

  function setPlaylistScanMode(mode: PlaylistScanModeV1): void {
    playlistScanMode = mode;
    pendingPlaylistItems = [];
    void playlistWatcher?.scanNow();
  }

  async function selectPlaylistItem(itemId: string): Promise<void> {
    playlistActionBusy = true;
    error = null;
    try {
      const selected = await api.selectPlaylistItem(itemId);
      playlist = selected.playlist;
      const playback = playlistSelectionPlayback(selected);
      await activateSession(playback.session, playback.subtitles);
    } catch (reason) {
      error = asAppError(reason);
    } finally {
      playlistActionBusy = false;
    }
  }

  async function reorderPlaylistItem(itemId: string, newIndex: number): Promise<void> {
    playlistActionBusy = true;
    error = null;
    try {
      playlist = await api.reorderPlaylistItem(itemId, newIndex);
    } catch (reason) {
      error = asAppError(reason);
    } finally {
      playlistActionBusy = false;
    }
  }

  async function removePlaylistItem(itemId: string): Promise<void> {
    playlistActionBusy = true;
    error = null;
    try {
      playlist = await api.removePlaylistItem(itemId);
    } catch (reason) {
      error = asAppError(reason);
    } finally {
      playlistActionBusy = false;
    }
  }

  async function importPlaylistDiscoveries(itemIds: string[]): Promise<void> {
    if (itemIds.length === 0) return;
    playlistActionBusy = true;
    error = null;
    try {
      playlist = await api.importPlaylistDiscoveries(itemIds);
      pendingPlaylistItems = remainingPlaylistDiscoveries(pendingPlaylistItems, itemIds);
    } catch (reason) {
      error = asAppError(reason);
    } finally {
      playlistActionBusy = false;
    }
  }

  async function choosePlaylistSubtitleOverride(
    itemId: string,
    role: SubtitleRoleV1,
  ): Promise<void> {
    playlistActionBusy = true;
    error = null;
    const active = playlist?.current_item_id === itemId && session !== null;
    try {
      const mutation = await api.choosePlaylistSubtitleOverride(
        itemId,
        role,
        active ? (session?.session_id ?? null) : null,
      );
      if (mutation) await applyPlaylistSubtitleMutation(mutation, role, active);
    } catch (reason) {
      error = asAppError(reason);
    } finally {
      playlistActionBusy = false;
    }
  }

  async function clearPlaylistSubtitleOverride(
    itemId: string,
    role: SubtitleRoleV1,
  ): Promise<void> {
    playlistActionBusy = true;
    error = null;
    const active = playlist?.current_item_id === itemId && session !== null;
    try {
      const mutation = await api.clearPlaylistSubtitleOverride(
        itemId,
        role,
        active ? (session?.session_id ?? null) : null,
      );
      await applyPlaylistSubtitleMutation(mutation, role, active);
    } catch (reason) {
      error = asAppError(reason);
    } finally {
      playlistActionBusy = false;
    }
  }

  async function applyPlaylistSubtitleMutation(
    mutation: PlaylistSubtitleMutationV1,
    role: SubtitleRoleV1,
    active: boolean,
  ): Promise<void> {
    playlist = mutation.playlist;
    if (!active) return;
    if (role === 'japanese') {
      subtitleSource = mutation.source;
      subtitleWindow = null;
      selection = null;
      pendingWindowPositionUs = null;
      windowRevision += 1;
      if (subtitleSource && showJapanese) await loadSubtitleWindow(currentTimeUs);
      return;
    }
    translationSource = mutation.source;
    translationWindow = null;
    pendingTranslationWindowPositionUs = null;
    translationWindowRevision += 1;
    if (translationSource && showEnglish) await loadTranslationWindow(currentTimeUs);
  }

  async function useDiscoveredSubtitle(candidateId: string): Promise<void> {
    if (!session) return;
    try {
      subtitleSource = await api.useDiscoveredSubtitle(session.session_id, candidateId);
      adjacentCandidates = [];
      subtitleWindow = null;
      selection = null;
      windowRevision += 1;
      await loadSubtitleWindow(currentTimeUs);
    } catch (reason) {
      error = asAppError(reason);
    }
  }

  async function rememberCurrentMedia(): Promise<void> {
    if (!session) return;
    try {
      session = await api.rememberMedia(
        session.session_id,
        subtitleSource?.subtitle_source_id,
        translationSource?.subtitle_source_id,
      );
      recents = await api.recentMedia();
    } catch (reason) {
      error = asAppError(reason);
    }
  }

  async function reopenRecent(sourceFingerprint: string): Promise<void> {
    busy = true;
    try {
      await activateSession(await api.reopenRecentMedia(sourceFingerprint));
      recents = await api.recentMedia();
    } catch (reason) {
      error = asAppError(reason);
    } finally {
      busy = false;
    }
  }

  async function revokeRecent(sourceFingerprint: string): Promise<void> {
    try {
      await api.revokeRecentMedia(sourceFingerprint);
      recents = await api.recentMedia();
    } catch (reason) {
      error = asAppError(reason);
    }
  }

  async function chooseSubtitle(): Promise<void> {
    if (!session) return;
    error = null;
    try {
      const selected = await api.chooseSubtitle(session.session_id);
      if (!selected) return;
      subtitleSource = selected;
      subtitleWindow = null;
      selection = null;
      windowRevision += 1;
      await loadSubtitleWindow(currentTimeUs);
    } catch (reason) {
      error = asAppError(reason);
    }
  }

  async function chooseTranslationSubtitle(): Promise<void> {
    if (!session) return;
    error = null;
    try {
      const selected = await api.chooseTranslationSubtitle(session.session_id);
      if (!selected) return;
      translationSource = selected;
      translationWindow = null;
      translationWindowRevision += 1;
      await loadTranslationWindow(currentTimeUs);
    } catch (reason) {
      error = asAppError(reason);
    }
  }

  async function removeTranslationSubtitle(): Promise<void> {
    if (!session || !translationSource) return;
    error = null;
    try {
      await api.removeTranslationSubtitle(session.session_id, translationSource.subtitle_source_id);
      translationSource = null;
      translationWindow = null;
      pendingTranslationWindowPositionUs = null;
      translationWindowRevision += 1;
    } catch (reason) {
      error = asAppError(reason);
    }
  }

  async function useEmbeddedSubtitle(streamId: string): Promise<void> {
    if (!session) return;
    error = null;
    try {
      const selected = await api.useEmbeddedSubtitle(session.session_id, streamId);
      subtitleSource = selected;
      subtitleWindow = null;
      selection = null;
      windowRevision += 1;
      await loadSubtitleWindow(currentTimeUs);
    } catch (reason) {
      error = asAppError(reason);
    }
  }

  async function useEmbeddedTranslationSubtitle(streamId: string): Promise<void> {
    if (!session) return;
    error = null;
    try {
      const selected = await api.useEmbeddedTranslationSubtitle(session.session_id, streamId);
      translationSource = selected;
      translationWindow = null;
      translationWindowRevision += 1;
      await loadTranslationWindow(currentTimeUs);
    } catch (reason) {
      error = asAppError(reason);
    }
  }

  function handleTime(positionUs: number): void {
    currentTimeUs = positionUs;
    if (!session) return;
    if (subtitleSource && showJapanese) {
      if (needsSubtitleWindow(positionUs)) {
        if (windowBusy) pendingWindowPositionUs = positionUs;
        else void loadSubtitleWindow(positionUs);
      }
    }
    if (translationSource && showEnglish) {
      if (needsTranslationWindow(positionUs)) {
        if (translationWindowBusy) pendingTranslationWindowPositionUs = positionUs;
        else void loadTranslationWindow(positionUs);
      }
    }
  }

  function needsSubtitleWindow(positionUs: number): boolean {
    return (
      !subtitleWindow ||
      positionUs < subtitleWindow.window_start_us ||
      positionUs >= subtitleWindow.window_end_us ||
      positionUs >= subtitleWindow.recommended_refresh_at_us - 2_000_000
    );
  }

  function needsTranslationWindow(positionUs: number): boolean {
    return (
      !translationWindow ||
      positionUs < translationWindow.window_start_us ||
      positionUs >= translationWindow.window_end_us ||
      positionUs >= translationWindow.recommended_refresh_at_us - 2_000_000
    );
  }

  async function loadSubtitleWindow(positionUs: number): Promise<void> {
    if (!session || !subtitleSource || !showJapanese) return;
    if (windowBusy) {
      pendingWindowPositionUs = positionUs;
      return;
    }
    windowBusy = true;
    const requestedRevision = ++windowRevision;
    try {
      const next = await api.subtitleWindow(
        session.session_id,
        subtitleSource.subtitle_source_id,
        positionUs,
        requestedRevision,
      );
      if (next.revision === windowRevision) subtitleWindow = next;
    } catch (reason) {
      error = asAppError(reason);
    } finally {
      windowBusy = false;
      if (pendingWindowPositionUs !== null) {
        const pending = pendingWindowPositionUs;
        pendingWindowPositionUs = null;
        void loadSubtitleWindow(pending);
      }
    }
  }

  async function loadTranslationWindow(positionUs: number): Promise<void> {
    if (!session || !translationSource || !showEnglish) return;
    if (translationWindowBusy) {
      pendingTranslationWindowPositionUs = positionUs;
      return;
    }
    translationWindowBusy = true;
    const requestedRevision = ++translationWindowRevision;
    try {
      const next = await api.translationWindow(
        session.session_id,
        translationSource.subtitle_source_id,
        positionUs,
        requestedRevision,
      );
      if (next.revision === translationWindowRevision) translationWindow = next;
    } catch (reason) {
      error = asAppError(reason);
    } finally {
      translationWindowBusy = false;
      if (pendingTranslationWindowPositionUs !== null) {
        const pending = pendingTranslationWindowPositionUs;
        pendingTranslationWindowPositionUs = null;
        void loadTranslationWindow(pending);
      }
    }
  }

  function remediation(appError: AppErrorV1): string {
    const actions: Record<string, string> = {
      FFMPEG_NOT_FOUND: 'Repair or reinstall the app to restore its bundled media tools.',
      MEDIA_SCOPE_DENIED: 'Locate the media file again.',
      MEDIA_PROBE_FAILED: 'Check FFprobe health or try another local file.',
      CONVERSION_REQUIRED: 'Review the conversion cost and approve it explicitly.',
      CONVERSION_FAILED: 'Check FFmpeg health and free disk space, then retry.',
      CONVERSION_CANCELLED: 'Restart the compatibility conversion when you are ready.',
      MEDIA_UNSUPPORTED: 'Review the codec details and media compatibility guide.',
      INVALID_REQUEST: 'Refresh the current media state and retry the action.',
      STALE_REVISION: 'Review the refreshed subtitle or card draft before retrying.',
      STORAGE_FAILED: 'Check free disk space and application data permissions.',
      SUBTITLE_AMBIGUOUS: 'Choose one of the tied subtitle sources explicitly.',
      SUBTITLE_IMAGE_UNSUPPORTED: 'Choose an embedded or external text subtitle track.',
      DICTIONARY_UNAVAILABLE: 'Repair or reinstall the app to restore its bundled JMdict data.',
      ANKI_OFFLINE: 'Start Anki Desktop and confirm AnkiConnect is installed.',
      ANKI_SCHEMA_MISMATCH: 'Review the selected deck, note type, and mapped fields.',
      PUBLISH_OUTCOME_UNCERTAIN: 'Reconcile the mining marker before retrying.',
    };
    return actions[appError.code] ?? 'Review diagnostics and retry the last safe action.';
  }

  function toggleSubtitleVisibility(control: SubtitleVisibilityControl): void {
    if (control === 'furigana') showFurigana = !showFurigana;
    else if (control === 'japanese') {
      showJapanese = !showJapanese;
      if (showJapanese && subtitleSource && needsSubtitleWindow(currentTimeUs)) {
        void loadSubtitleWindow(currentTimeUs);
      } else if (!showJapanese) {
        pendingWindowPositionUs = null;
      }
    } else {
      showEnglish = !showEnglish;
      if (showEnglish && translationSource && needsTranslationWindow(currentTimeUs)) {
        void loadTranslationWindow(currentTimeUs);
      } else if (!showEnglish) {
        pendingTranslationWindowPositionUs = null;
      }
    }
  }

  function handleVisibilityShortcut(event: KeyboardEvent): void {
    if (
      tab !== 'watch' ||
      event.defaultPrevented ||
      event.repeat ||
      event.ctrlKey ||
      event.metaKey ||
      event.altKey ||
      isEditableOrMediaControl(event.target)
    )
      return;
    const control = subtitleVisibilityShortcut(event.key);
    if (!control) return;
    event.preventDefault();
    toggleSubtitleVisibility(control);
  }

  function closeCardComposer(): void {
    const tokenId = selection?.token.token.token_id;
    const anchor = tokenId
      ? Array.from(document.querySelectorAll<HTMLElement>('[data-subtitle-token-id]')).find(
          (element) => element.dataset.subtitleTokenId === tokenId,
        )
      : null;
    selection = null;
    queueMicrotask(() => {
      if (anchor?.isConnected) anchor.focus({ preventScroll: true });
    });
  }
</script>

<svelte:head>
  <meta
    name="description"
    content="A private local Japanese immersion player with interactive subtitles and Anki mining."
  />
</svelte:head>

<svelte:window onkeydown={handleVisibilityShortcut} />

<div class="app-shell">
  <header class="topbar">
    <button class="brand" type="button" onclick={() => (tab = 'watch')} aria-label="Open player">
      <span class="brand-mark">見</span>
      <span><strong>Migaku</strong><small>Cinema · 映画と言葉</small></span>
    </button>
    <nav aria-label="Primary navigation">
      <button
        class:active={tab === 'watch'}
        type="button"
        aria-current={tab === 'watch' ? 'page' : undefined}
        onclick={() => (tab = 'watch')}>Watch</button
      >
      <button
        class:active={tab === 'settings'}
        type="button"
        aria-current={tab === 'settings' ? 'page' : undefined}
        onclick={() => (tab = 'settings')}>Settings</button
      >
      <button
        class:active={tab === 'diagnostics'}
        type="button"
        aria-current={tab === 'diagnostics' ? 'page' : undefined}
        onclick={() => (tab = 'diagnostics')}>Diagnostics</button
      >
    </nav>
    <div class="import-actions">
      <label class="import-mode" title="Folder import mode">
        <span class="sr-only">Folder import mode</span>
        <select
          value={playlistScanMode}
          onchange={(event) => setPlaylistScanMode(event.currentTarget.value as PlaylistScanModeV1)}
          disabled={playlistActionBusy}
        >
          <option value="preview">Preview new</option>
          <option value="auto_import">Auto-add</option>
        </select>
      </label>
      <button
        class="import-folder"
        type="button"
        onclick={importPlaylistFolder}
        disabled={playlistActionBusy || !isDesktop()}
      >
        {playlistActionBusy ? 'Working…' : playlist ? 'Change folder' : 'Import folder'}
      </button>
      <button class="import" type="button" onclick={importVideo} disabled={busy}>
        {busy ? 'Importing…' : session ? 'Change video' : 'Import video'}
      </button>
    </div>
  </header>

  {#if error}
    <aside class="error-banner" role="alert">
      <div>
        <strong>{error.code}</strong>
        <p>{error.message} {remediation(error)}</p>
      </div>
      <button type="button" onclick={() => (error = null)} aria-label="Dismiss error">×</button>
    </aside>
  {/if}

  <main>
    {#if session}
      <div class="watch-tab" hidden={tab !== 'watch'}>
        <div class="watch-layout" class:has-playlist={playlist !== null}>
          {#if playlist}
            <PlaylistPanel
              {playlist}
              pendingItems={pendingPlaylistItems}
              scanMode={playlistScanMode}
              busy={playlistActionBusy || playlistScanBusy}
              onSelect={selectPlaylistItem}
              onReorder={reorderPlaylistItem}
              onRemove={removePlaylistItem}
              onChooseSubtitle={choosePlaylistSubtitleOverride}
              onClearSubtitle={clearPlaylistSubtitleOverride}
              onScanMode={setPlaylistScanMode}
              onRescan={() => playlistWatcher?.scanNow()}
              onImportDiscoveries={importPlaylistDiscoveries}
            />
          {/if}
          <div class="workspace" class:study-open={selection !== null}>
            <section class="watch-column">
              <div class="media-heading">
                <div>
                  <p class="eyebrow">Now playing · 上映中</p>
                  <h1>{session.display_name}</h1>
                </div>
                <div class="media-actions">
                  <span class="plan">{session.playback_plan.kind.replace('_', ' ')}</span>
                  {#if session.source_scope_persistence === 'session'}
                    <button type="button" class="secondary" onclick={rememberCurrentMedia}
                      >Remember file</button
                    >
                  {/if}
                  <button type="button" class="secondary" onclick={chooseSubtitle}>
                    {subtitleSource ? 'Change JP subtitles' : 'Add JP subtitles'}
                  </button>
                </div>
              </div>

              <div class="playback-experience" bind:this={playbackExperience}>
                {#key session.session_id}
                  <Player
                    {session}
                    active={tab === 'watch'}
                    fullscreenTarget={playbackExperience}
                    onTime={handleTime}
                    onSessionUpdate={(updated) => (session = updated)}
                    onError={(value) => (error = value)}
                  />
                {/key}

                {#if showJapanese || showEnglish}
                  <LiveSubtitle
                    cues={showJapanese ? activeCues : []}
                    translationCues={showEnglish ? activeTranslationCues : []}
                    {selection}
                    {showFurigana}
                    onSelect={(value) => (selection = value)}
                    onDismiss={() => (selection = null)}
                  />
                {/if}
              </div>
              <section class="track-board" aria-label="Subtitle tracks">
                <header class="track-board-heading">
                  <span class="track-board-title"
                    >Subtitle tracks <small>Dual cue timeline</small></span
                  >
                  <div
                    class="subtitle-visibility-controls"
                    role="group"
                    aria-label="Subtitle display"
                  >
                    <button
                      type="button"
                      class:enabled={showJapanese}
                      aria-pressed={showJapanese}
                      aria-keyshortcuts="J"
                      aria-label="Toggle Japanese subtitles"
                      title="Toggle Japanese subtitles (J)"
                      onclick={() => toggleSubtitleVisibility('japanese')}
                    >
                      JP <kbd>J</kbd>
                    </button>
                    <button
                      type="button"
                      class:enabled={showFurigana}
                      aria-pressed={showFurigana}
                      aria-keyshortcuts="R"
                      aria-label="Toggle furigana readings"
                      title="Toggle furigana readings (R)"
                      onclick={() => toggleSubtitleVisibility('furigana')}
                    >
                      Furigana <kbd>R</kbd>
                    </button>
                    <button
                      type="button"
                      class:enabled={showEnglish}
                      aria-pressed={showEnglish}
                      aria-keyshortcuts="E"
                      aria-label="Toggle English subtitles"
                      title="Toggle English subtitles (E)"
                      onclick={() => toggleSubtitleVisibility('english')}
                    >
                      EN <kbd>E</kbd>
                    </button>
                  </div>
                </header>
                {#if subtitleSource}
                  <div class="subtitle-source track-source">
                    <span class="track-role">JP</span>
                    <span class="track-purpose">Interactive study</span>
                    <span class="track-filename"
                      >{subtitleSource.display_name} · {subtitleSource.format.toUpperCase()}</span
                    >
                  </div>
                {:else}
                  <div class="subtitle-choices">
                    {#if adjacentCandidates.length > 0}
                      <span>Multiple adjacent subtitles matched. Choose one:</span>
                      {#each adjacentCandidates as candidate}
                        <button
                          type="button"
                          class="secondary"
                          onclick={() => useDiscoveredSubtitle(candidate.candidate_id)}
                        >
                          {candidate.display_name}{candidate.language
                            ? ` · ${candidate.language}`
                            : ''}
                        </button>
                      {/each}
                    {/if}
                    <button type="button" class="subtitle-prompt" onclick={chooseSubtitle}>
                      Choose a Japanese SRT, ASS/SSA, or WebVTT subtitle file
                    </button>
                  </div>
                {/if}

                <div class="translation-source track-source">
                  <span class="track-role">EN</span>
                  <span class="track-purpose">Translation</span>
                  {#if translationSource}
                    <span class="track-filename"
                      >{translationSource.display_name} ·
                      {translationSource.format.toUpperCase()}</span
                    >
                    <span class="track-actions">
                      <button type="button" class="secondary" onclick={chooseTranslationSubtitle}
                        >Change</button
                      >
                      <button
                        type="button"
                        class="secondary remove"
                        onclick={removeTranslationSubtitle}>Remove</button
                      >
                    </span>
                  {:else}
                    <span class="track-empty">Optional display-only English subtitles</span>
                    <button type="button" class="secondary" onclick={chooseTranslationSubtitle}
                      >Add English</button
                    >
                  {/if}
                </div>

                {#if session.subtitle_streams.length > 0}
                  <div class="embedded-tracks" aria-label="Embedded subtitle tracks">
                    <span>Embedded tracks</span>
                    {#each session.subtitle_streams as stream}
                      <button
                        type="button"
                        class="secondary"
                        disabled={stream.subtitle_kind !== 'text'}
                        title={stream.subtitle_kind === 'text'
                          ? 'Use this embedded text subtitle track'
                          : 'Image subtitles are not interactive in this release'}
                        onclick={() => useEmbeddedSubtitle(stream.stream_id)}
                      >
                        JP · {stream.language ?? 'und'} · {stream.codec}
                        {stream.subtitle_kind === 'image' ? ' (image)' : ''}
                      </button>
                      <button
                        type="button"
                        class="secondary"
                        disabled={stream.subtitle_kind !== 'text'}
                        title={stream.subtitle_kind === 'text'
                          ? 'Use this embedded text track as the English translation'
                          : 'Image subtitles cannot be used as a translation track'}
                        onclick={() => useEmbeddedTranslationSubtitle(stream.stream_id)}
                      >
                        EN · {stream.language ?? 'und'} · {stream.codec}
                        {stream.subtitle_kind === 'image' ? ' (image)' : ''}
                      </button>
                    {/each}
                  </div>
                {/if}
              </section>
            </section>

            {#if selection}
              <aside class="study-column">
                <div class="study-intro">
                  <span>Optional study layer</span>
                  <small>Opened from your selected word</small>
                </div>
                <CardComposer
                  {session}
                  source={subtitleSource}
                  {selection}
                  positionUs={currentTimeUs}
                  onClose={closeCardComposer}
                  onError={(value) => (error = value)}
                />
              </aside>
            {/if}
          </div>
        </div>
      </div>
    {:else if tab === 'watch'}
      <div class="watch-layout" class:has-playlist={playlist !== null}>
        {#if playlist}
          <PlaylistPanel
            {playlist}
            pendingItems={pendingPlaylistItems}
            scanMode={playlistScanMode}
            busy={playlistActionBusy || playlistScanBusy}
            onSelect={selectPlaylistItem}
            onReorder={reorderPlaylistItem}
            onRemove={removePlaylistItem}
            onChooseSubtitle={choosePlaylistSubtitleOverride}
            onClearSubtitle={clearPlaylistSubtitleOverride}
            onScanMode={setPlaylistScanMode}
            onRescan={() => playlistWatcher?.scanNow()}
            onImportDiscoveries={importPlaylistDiscoveries}
          />
        {/if}
        <section class="landing">
          <div class="landing-copy">
            <p class="eyebrow">Private · local · focused</p>
            <h1>Watch Japanese.<br /><span>Keep the moment.</span></h1>
            <p>
              Play a local video, follow frame-accurate subtitles, inspect words, and create an
              idempotent Anki note without sending your media off-device.
            </p>
            <div class="hero-actions">
              <button
                class="hero-action"
                type="button"
                onclick={importVideo}
                disabled={busy || !isDesktop()}
              >
                {isDesktop() ? 'Choose a local video' : 'Open the desktop app'}
              </button>
              <button
                class="folder-action"
                type="button"
                onclick={importPlaylistFolder}
                disabled={playlistActionBusy || !isDesktop()}>Choose a folder</button
              >
            </div>
            {#if recents.length > 0}
              <div class="recent-list" aria-label="Approved recent files">
                <strong>Approved recent files</strong>
                {#each recents as recent}
                  <div>
                    <button
                      type="button"
                      onclick={() => reopenRecent(recent.source_fingerprint)}
                      disabled={busy}>{recent.display_name}</button
                    >
                    <button
                      type="button"
                      class="forget"
                      onclick={() => revokeRecent(recent.source_fingerprint)}>Forget</button
                    >
                  </div>
                {/each}
              </div>
            {/if}
          </div>
          <div class="landing-card" aria-label="Workflow preview">
            <div class="fake-video"><span>映画とことばが、ひとつになる。</span></div>
            <div class="workflow">
              <span>01 Play</span><span>02 Understand</span><span>03 Remember</span>
            </div>
            <div class="privacy"><span class="pulse"></span> Media stays on this device</div>
          </div>
        </section>
      </div>
    {/if}

    {#if tab === 'settings'}
      <section class="theme-settings" aria-label="Appearance">
        <ThemePicker mode="panel" label="Visual theme" />
      </section>
      <SettingsPanel
        {health}
        onError={(value) => (error = value)}
        onHealth={(value) => (health = value)}
      />
    {:else if tab === 'diagnostics'}
      <DiagnosticsPanel {health} {session} subtitle={subtitleSource} />
    {/if}
  </main>
</div>

<style>
  .import-actions,
  .hero-actions {
    display: flex;
    align-items: center;
    gap: 0.45rem;
  }

  .import-actions {
    justify-self: end;
  }

  .import-mode select {
    min-height: 2.45rem;
    max-width: 7.6rem;
    padding: 0 0.45rem;
    border-radius: var(--theme-control-radius);
    background: var(--ink-raised);
    color: var(--muted);
    font-size: 0.62rem;
    font-weight: 700;
  }

  .import-folder,
  .folder-action {
    min-height: 2.45rem;
    padding: 0 0.8rem;
    border: 1px solid var(--line);
    border-radius: var(--theme-control-radius);
    background: transparent;
    color: var(--text);
    font-size: 0.66rem;
    font-weight: 800;
    letter-spacing: 0.055em;
    text-transform: uppercase;
  }

  .import-folder:hover:not(:disabled),
  .folder-action:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent-strong);
  }

  .watch-layout {
    max-width: 1740px;
    margin: 0 auto;
  }

  .watch-layout.has-playlist {
    display: grid;
    grid-template-columns: minmax(245px, 285px) minmax(0, 1fr);
    align-items: start;
    gap: clamp(1rem, 1.5vw, 1.5rem);
  }

  .watch-layout.has-playlist .landing {
    width: 100%;
    min-height: calc(100vh - 142px);
    padding-inline: clamp(1.5rem, 3.5vw, 4rem);
  }

  .workspace:not(.study-open) {
    grid-template-columns: minmax(0, 1fr);
  }

  .track-board-heading {
    flex-wrap: wrap;
    gap: 0.45rem 1rem;
    padding-block: 0.35rem;
  }

  .track-board-title {
    display: inline-flex;
    align-items: baseline;
    gap: 0.65rem;
  }

  .subtitle-visibility-controls {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 0.3rem;
  }

  .subtitle-visibility-controls button {
    display: inline-flex;
    align-items: center;
    gap: 0.38rem;
    min-height: 1.72rem;
    padding: 0.2rem 0.38rem 0.2rem 0.5rem;
    border: 1px solid var(--line);
    border-radius: 2px;
    background: transparent;
    color: var(--muted);
    font-size: 0.58rem;
    font-weight: 800;
    letter-spacing: 0.045em;
  }

  .subtitle-visibility-controls button.enabled {
    border-color: rgb(217 72 63 / 65%);
    background: rgb(217 72 63 / 12%);
    color: var(--text);
  }

  .subtitle-visibility-controls kbd {
    display: grid;
    place-items: center;
    min-width: 1.15rem;
    height: 1.05rem;
    border: 1px solid rgb(240 236 227 / 22%);
    border-radius: 1px;
    color: inherit;
    font: inherit;
    font-size: 0.5rem;
  }

  @media (max-width: 620px) {
    .import-actions {
      gap: 0.25rem;
    }

    .import-folder,
    .import {
      padding-inline: 0.55rem;
      font-size: 0.58rem;
    }

    .track-board-heading {
      align-items: flex-start;
      flex-direction: column;
    }

    .subtitle-visibility-controls {
      justify-content: flex-start;
      width: 100%;
    }
  }

  @media (max-width: 1180px) {
    .watch-layout.has-playlist {
      grid-template-columns: minmax(0, 1fr);
    }
  }

  @media (max-width: 720px) {
    .hero-actions {
      align-items: stretch;
      flex-direction: column;
    }

    .folder-action,
    .hero-action {
      justify-content: center;
      width: 100%;
    }
  }
</style>
