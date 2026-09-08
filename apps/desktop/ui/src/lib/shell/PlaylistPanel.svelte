<script lang="ts">
  import {
    playlistMoveTarget,
    retainPendingDiscoveryIds,
    subtitleSummary,
    type PlaylistDiscoveryItemV1,
    type PlaylistScanModeV1,
    type PlaylistV1,
    type SubtitleRoleV1,
  } from './playlist';

  interface Props {
    playlist: PlaylistV1;
    pendingItems?: PlaylistDiscoveryItemV1[];
    scanMode: PlaylistScanModeV1;
    busy?: boolean;
    onSelect: (itemId: string) => void | Promise<void>;
    onReorder: (itemId: string, newIndex: number) => void | Promise<void>;
    onRemove: (itemId: string) => void | Promise<void>;
    onChooseSubtitle: (itemId: string, role: SubtitleRoleV1) => void | Promise<void>;
    onClearSubtitle: (itemId: string, role: SubtitleRoleV1) => void | Promise<void>;
    onScanMode: (mode: PlaylistScanModeV1) => void;
    onRescan: () => void | Promise<void>;
    onImportDiscoveries: (itemIds: string[]) => void | Promise<void>;
  }

  let {
    playlist,
    pendingItems = [],
    scanMode,
    busy = false,
    onSelect,
    onReorder,
    onRemove,
    onChooseSubtitle,
    onClearSubtitle,
    onScanMode,
    onRescan,
    onImportDiscoveries,
  }: Props = $props();

  let selectedDiscoveries = $state<string[]>([]);

  $effect(() => {
    const retained = retainPendingDiscoveryIds(selectedDiscoveries, pendingItems);
    if (
      retained.length !== selectedDiscoveries.length ||
      retained.some((id, index) => id !== selectedDiscoveries[index])
    ) {
      selectedDiscoveries = retained;
    }
  });

  function setDiscoverySelected(itemId: string, selected: boolean): void {
    selectedDiscoveries = selected
      ? [...new Set([...selectedDiscoveries, itemId])]
      : selectedDiscoveries.filter((id) => id !== itemId);
  }

  function move(itemId: string, direction: -1 | 1): void {
    const target = playlistMoveTarget(playlist.items, itemId, direction);
    if (target !== null) void onReorder(itemId, target);
  }

  function subtitleActionLabel(
    kind: 'auto' | 'override' | 'ambiguous' | 'none',
    role: 'JP' | 'EN',
  ): string {
    return kind === 'override' ? `Change ${role}` : `Set ${role}`;
  }
</script>

<aside class="playlist-panel" aria-label="Folder playlist">
  <header>
    <div>
      <p class="kicker">Folder playlist</p>
      <h2>{playlist.display_name}</h2>
    </div>
    <span class:attention={playlist.watch_state.status !== 'watching'} class="watch-status">
      {playlist.watch_state.status === 'watching' ? 'Watching' : 'Unavailable'}
    </span>
  </header>

  <div class="scan-controls">
    <label>
      <span>New videos</span>
      <select
        value={scanMode}
        onchange={(event) => onScanMode(event.currentTarget.value as PlaylistScanModeV1)}
        disabled={busy}
      >
        <option value="preview">Preview before adding</option>
        <option value="auto_import">Add automatically</option>
      </select>
    </label>
    <button type="button" class="refresh" onclick={() => void onRescan()} disabled={busy}>
      {busy ? 'Scanning…' : 'Refresh'}
    </button>
  </div>

  {#if scanMode === 'preview' && pendingItems.length > 0}
    <section class="discoveries" aria-label="New videos found">
      <div class="discoveries-heading">
        <strong>{pendingItems.length} new {pendingItems.length === 1 ? 'video' : 'videos'}</strong>
        <button
          type="button"
          onclick={() => (selectedDiscoveries = pendingItems.map((item) => item.item_id))}
          disabled={busy}>Select all</button
        >
      </div>
      {#each pendingItems as item (item.item_id)}
        <label>
          <input
            type="checkbox"
            checked={selectedDiscoveries.includes(item.item_id)}
            onchange={(event) => setDiscoverySelected(item.item_id, event.currentTarget.checked)}
            disabled={busy}
          />
          <span>{item.display_name}</span>
        </label>
      {/each}
      <button
        type="button"
        class="add-selected"
        onclick={() => void onImportDiscoveries(selectedDiscoveries)}
        disabled={busy || selectedDiscoveries.length === 0}
      >
        Add selected
      </button>
    </section>
  {/if}

  {#if playlist.items.length > 0}
    <ol class="playlist-items">
      {#each playlist.items as item, index (item.item_id)}
        {@const current = item.item_id === playlist.current_item_id}
        <li class:current>
          <div class="item-heading">
            <button
              type="button"
              class="select-item"
              aria-current={current ? 'true' : undefined}
              aria-label={`${current ? 'Currently playing' : 'Play'} ${item.display_name}`}
              onclick={() => void onSelect(item.item_id)}
              disabled={busy}
            >
              <span class="ordinal">{String(index + 1).padStart(2, '0')}</span>
              <span class="filename">{item.display_name}</span>
              <span class="play-mark" aria-hidden="true">{current ? '●' : '▶'}</span>
            </button>
            <div class="item-order" aria-label={`Reorder ${item.display_name}`}>
              <button
                type="button"
                aria-label={`Move ${item.display_name} up`}
                onclick={() => move(item.item_id, -1)}
                disabled={busy || index === 0}>↑</button
              >
              <button
                type="button"
                aria-label={`Move ${item.display_name} down`}
                onclick={() => move(item.item_id, 1)}
                disabled={busy || index === playlist.items.length - 1}>↓</button
              >
              <button
                type="button"
                class="remove"
                aria-label={`Remove ${item.display_name} from playlist${current ? '; playback continues' : ''}`}
                title={current
                  ? 'Remove from playlist; current playback continues'
                  : 'Remove from playlist'}
                onclick={() => void onRemove(item.item_id)}
                disabled={busy}>×</button
              >
            </div>
          </div>

          <div class="subtitle-row">
            <div>
              <span class="role">JP</span>
              <span
                class="subtitle-copy"
                class:unresolved={['ambiguous', 'none'].includes(item.japanese_subtitle.kind)}
              >
                <span>{subtitleSummary(item.japanese_subtitle)}</span>
                {#if item.japanese_subtitle.reason}
                  <small>
                    {item.japanese_subtitle.confidence !== null
                      ? `${item.japanese_subtitle.confidence}% · `
                      : ''}{item.japanese_subtitle.reason}
                  </small>
                {/if}
              </span>
            </div>
            <span class="subtitle-actions">
              <button
                type="button"
                onclick={() => void onChooseSubtitle(item.item_id, 'japanese')}
                disabled={busy}>{subtitleActionLabel(item.japanese_subtitle.kind, 'JP')}</button
              >
              {#if item.japanese_subtitle.kind === 'override'}
                <button
                  type="button"
                  aria-label={`Clear Japanese subtitle override for ${item.display_name}`}
                  onclick={() => void onClearSubtitle(item.item_id, 'japanese')}
                  disabled={busy}>Clear</button
                >
              {/if}
            </span>
          </div>
          <div class="subtitle-row">
            <div>
              <span class="role translation">EN</span>
              <span
                class="subtitle-copy"
                class:unresolved={['ambiguous', 'none'].includes(item.translation_subtitle.kind)}
              >
                <span>{subtitleSummary(item.translation_subtitle)}</span>
                {#if item.translation_subtitle.reason}
                  <small>
                    {item.translation_subtitle.confidence !== null
                      ? `${item.translation_subtitle.confidence}% · `
                      : ''}{item.translation_subtitle.reason}
                  </small>
                {/if}
              </span>
            </div>
            <span class="subtitle-actions">
              <button
                type="button"
                onclick={() => void onChooseSubtitle(item.item_id, 'translation')}
                disabled={busy}>{subtitleActionLabel(item.translation_subtitle.kind, 'EN')}</button
              >
              {#if item.translation_subtitle.kind === 'override'}
                <button
                  type="button"
                  aria-label={`Clear English subtitle override for ${item.display_name}`}
                  onclick={() => void onClearSubtitle(item.item_id, 'translation')}
                  disabled={busy}>Clear</button
                >
              {/if}
            </span>
          </div>
        </li>
      {/each}
    </ol>
  {:else}
    <p class="empty">
      {pendingItems.length > 0
        ? 'Choose the new videos above to add them to this playlist.'
        : 'No supported videos are in this playlist yet. Refresh after adding files.'}
    </p>
  {/if}
</aside>

<style>
  .playlist-panel {
    position: sticky;
    top: 92px;
    min-width: 0;
    max-height: calc(100vh - 118px);
    overflow: auto;
    border: 1px solid var(--line);
    border-top: 4px solid var(--accent);
    border-radius: var(--theme-control-radius);
    background: var(--panel);
    color: var(--text);
  }

  header {
    display: flex;
    justify-content: space-between;
    align-items: start;
    gap: 0.75rem;
    padding: 0.9rem;
    border-bottom: 1px solid var(--line);
  }

  .kicker {
    margin: 0 0 0.24rem;
    color: var(--accent-strong);
    font-size: 0.56rem;
    font-weight: 850;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  h2 {
    margin: 0;
    overflow: hidden;
    font-family: Georgia, 'Times New Roman', serif;
    font-size: 1.05rem;
    font-weight: 520;
    line-height: 1.15;
    text-overflow: ellipsis;
  }

  .watch-status {
    flex: 0 0 auto;
    padding: 0.25rem 0.38rem;
    border: 1px solid var(--line);
    color: var(--muted);
    font-size: 0.52rem;
    font-weight: 800;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }

  .watch-status.attention {
    border-color: var(--accent);
    color: var(--accent-strong);
  }

  .scan-controls {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: end;
    gap: 0.45rem;
    padding: 0.65rem 0.75rem;
    border-bottom: 1px solid var(--line);
  }

  .scan-controls label {
    display: grid;
    gap: 0.3rem;
    min-width: 0;
  }

  .scan-controls label > span {
    color: var(--muted);
    font-size: 0.55rem;
    font-weight: 800;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  select {
    min-width: 0;
    width: 100%;
    min-height: 2rem;
    padding: 0.35rem 0.42rem;
    background: var(--ink-raised);
    color: var(--text);
    font-size: 0.65rem;
  }

  button {
    min-height: 1.75rem;
    border: 1px solid var(--line);
    border-radius: var(--theme-control-radius);
    background: transparent;
    color: var(--text);
    font-size: 0.6rem;
    font-weight: 700;
  }

  button:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent-strong);
  }

  button:disabled {
    opacity: 0.42;
  }

  .refresh {
    padding-inline: 0.55rem;
  }

  .discoveries {
    display: grid;
    gap: 0.5rem;
    padding: 0.7rem 0.75rem;
    border-bottom: 1px solid var(--line);
    background: var(--theme-selected-surface);
  }

  .discoveries-heading {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.5rem;
  }

  .discoveries-heading strong {
    font-size: 0.65rem;
  }

  .discoveries-heading button,
  .add-selected {
    padding-inline: 0.45rem;
  }

  .discoveries label {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 0.4rem;
    color: var(--muted);
    font-size: 0.65rem;
  }

  .discoveries label span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .add-selected {
    justify-self: start;
    border-color: var(--accent);
  }

  .playlist-items {
    display: grid;
    gap: 0;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .playlist-items li {
    display: grid;
    gap: 0.28rem;
    padding: 0.55rem 0.6rem;
    border-bottom: 1px solid var(--line);
  }

  .playlist-items li.current {
    background: var(--theme-selected-surface);
    box-shadow: inset 3px 0 0 var(--accent);
  }

  .item-heading {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: 0.35rem;
  }

  .select-item {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 0.42rem;
    min-width: 0;
    padding: 0.25rem 0.3rem;
    border-color: transparent;
    text-align: left;
  }

  .ordinal {
    color: var(--muted);
    font-family: Georgia, 'Times New Roman', serif;
    font-size: 0.62rem;
  }

  .filename {
    overflow: hidden;
    font-size: 0.69rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .play-mark {
    color: var(--accent);
    font-size: 0.5rem;
  }

  .item-order {
    display: inline-flex;
    gap: 0.18rem;
  }

  .item-order button {
    width: 1.65rem;
    padding: 0;
  }

  .item-order .remove {
    color: var(--muted);
    font-size: 0.85rem;
  }

  .subtitle-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: 0.35rem;
    padding-left: 0.3rem;
  }

  .subtitle-row > div {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 0.32rem;
    min-width: 0;
    color: var(--muted);
    font-size: 0.56rem;
  }

  .subtitle-copy,
  .subtitle-copy > span,
  .subtitle-copy small {
    display: block;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .subtitle-copy.unresolved > span {
    color: var(--accent-strong);
    font-weight: 800;
  }

  .subtitle-copy small {
    margin-top: 0.08rem;
    color: var(--muted);
    font-size: 0.49rem;
  }

  .role {
    display: grid;
    place-items: center;
    min-width: 1.55rem;
    min-height: 1.05rem;
    background: var(--text);
    color: var(--ink);
    font-size: 0.47rem;
    font-weight: 900;
  }

  .role.translation {
    background: var(--accent);
    color: var(--theme-on-accent);
  }

  .subtitle-actions {
    display: inline-flex;
    gap: 0.18rem;
  }

  .subtitle-actions button {
    min-height: 1.35rem;
    padding: 0.12rem 0.3rem;
    font-size: 0.5rem;
  }

  .empty {
    margin: 0;
    padding: 1rem;
    color: var(--muted);
    font-family: Georgia, 'Times New Roman', serif;
    font-size: 0.78rem;
    line-height: 1.5;
  }

  @media (max-width: 1180px) {
    .playlist-panel {
      position: static;
      max-height: none;
    }
  }
</style>
