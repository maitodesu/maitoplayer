<script lang="ts">
  import { onDestroy } from 'svelte';
  import type {
    AnalyzedCueV1,
    AnalyzedTokenV1,
    AppErrorV1,
    CardDraftV1,
    CreateCardResultV1,
    MediaSessionV1,
    SubtitleSourceV1,
  } from '../../contracts/generated';
  import { api, asAppError } from '../../shell/api';
  import { formatTime } from '../../shell/time';
  import { formatDictionaryDefinition, preferredReading } from './card-content';

  interface Props {
    session: MediaSessionV1;
    source?: SubtitleSourceV1 | null;
    selection?: { cue: AnalyzedCueV1; token: AnalyzedTokenV1 } | null;
    positionUs: number;
    onClose?: () => void;
    onError: (error: AppErrorV1) => void;
  }

  let {
    session,
    source = null,
    selection = null,
    positionUs,
    onClose = () => undefined,
    onError,
  }: Props = $props();
  let draft = $state<CardDraftV1 | null>(null);
  let result = $state<CreateCardResultV1 | null>(null);
  let expression = $state('');
  let reading = $state('');
  let definition = $state('');
  let busy = $state(false);
  let publishing = $state(false);
  let extractionActive = $state(false);
  let status = $state('Select a subtitle word to prepare a card.');
  let progressTimer: number | null = null;
  let progressPollInFlight = false;
  let publishGeneration = 0;
  let activePublishSessionId: MediaSessionV1['session_id'] | null = null;

  $effect(() => {
    if (selection) {
      draft = null;
      result = null;
      expression = selection.token.token.surface;
      const entry = selection.token.dictionary[0];
      reading = preferredReading(selection.token.token.reading, entry);
      definition = formatDictionaryDefinition(entry);
      status = entry
        ? 'Word and definition are ready. Save only when you want this card.'
        : 'Word context is ready, but the local dictionary has no definition for it.';
    }
  });

  async function prepareDraft(): Promise<void> {
    if (!selection || !source) return;
    busy = true;
    status = 'Freezing exact media and subtitle context…';
    try {
      draft = await api.createDraft({
        session_id: session.session_id,
        subtitle_source_id: source.subtitle_source_id,
        cue_id: selection.cue.cue.cue_id,
        token_id: selection.token.token.token_id,
        dictionary_entry_id: selection.token.dictionary[0]?.entry_id ?? null,
        observed_playback_time_us: positionUs,
      });
      status = 'Draft ready. Publishing locks this revision.';
    } catch (error) {
      onError(asAppError(error));
      status = 'Draft could not be prepared.';
    } finally {
      busy = false;
    }
  }

  async function publish(): Promise<void> {
    if (!draft) return;
    const operationGeneration = ++publishGeneration;
    const operationSessionId = session.session_id;
    activePublishSessionId = operationSessionId;
    busy = true;
    publishing = true;
    extractionActive = true;
    status = 'Extracting the bounded audio clip and frame…';
    if (progressTimer !== null) window.clearInterval(progressTimer);
    progressTimer = window.setInterval(async () => {
      if (!publishing || progressPollInFlight) return;
      progressPollInFlight = true;
      try {
        const progress = await api.miningProgress(operationSessionId);
        if (operationGeneration !== publishGeneration) return;
        extractionActive = progress !== null;
        status =
          progress === null
            ? 'Checking idempotency and contacting Anki…'
            : progress < 0
              ? 'Queued behind another media operation…'
              : `Extracting media assets… ${Math.round(progress * 100)}%`;
      } catch {
        // The publish result remains authoritative if progress polling is interrupted.
      } finally {
        progressPollInFlight = false;
      }
    }, 250);
    try {
      const publishResult = await api.publishCard({
        draft_id: draft.draft_id,
        expected_draft_revision: draft.revision,
        profile_id: 'default',
        editable_fields: {
          expression,
          reading,
          sentence: selection?.cue.cue.plain_text ?? draft.cue.plain_text,
          definition,
        },
        confirmed: true,
      });
      if (operationGeneration !== publishGeneration) return;
      result = publishResult;
      status =
        result.outcome === 'created'
          ? `Created Anki note ${result.note_id ?? ''}.`
          : result.outcome === 'already_exists'
            ? `This mining context already exists as note ${result.note_id ?? ''}.`
            : 'Anki did not complete the card.';
    } catch (error) {
      if (operationGeneration !== publishGeneration) return;
      const appError = asAppError(error);
      onError(appError);
      status =
        appError.code === 'PUBLISH_OUTCOME_UNCERTAIN'
          ? 'Outcome uncertain. Reconciliation is required before retrying.'
          : 'Publish failed. Review Anki health before retrying.';
    } finally {
      publishing = false;
      extractionActive = false;
      if (progressTimer !== null) window.clearInterval(progressTimer);
      progressTimer = null;
      if (operationGeneration === publishGeneration) {
        publishGeneration += 1;
        activePublishSessionId = null;
        busy = false;
      }
    }
  }

  async function cancelPublish(): Promise<void> {
    status = 'Cancellation requested. Finishing cleanup…';
    try {
      const cancelled = await api.cancelMining(activePublishSessionId ?? session.session_id);
      if (!cancelled) status = 'Extraction already finished; completing the Anki request…';
    } catch (error) {
      onError(asAppError(error));
    }
  }

  onDestroy(() => {
    publishGeneration += 1;
    if (progressTimer !== null) window.clearInterval(progressTimer);
    progressTimer = null;
  });
</script>

<section class="composer" aria-label="Card composer">
  <div class="heading">
    <div>
      <p class="eyebrow">Word card</p>
      <h2>Keep it for later</h2>
    </div>
    <div class="heading-actions">
      <span class="time">{formatTime(positionUs)}</span>
      <button
        type="button"
        class="close"
        onclick={onClose}
        disabled={busy}
        aria-label="Close word card composer"
        title="Close word card composer">×</button
      >
    </div>
  </div>

  {#if selection}
    <label>
      Expression
      <input bind:value={expression} maxlength="256" />
    </label>
    <label>
      Reading
      <input bind:value={reading} maxlength="256" lang="ja" />
    </label>
    <label>
      Definition
      <textarea
        bind:value={definition}
        rows="5"
        maxlength="4096"
        placeholder="No local definition found for this word."></textarea>
    </label>
    <div class="context" lang="ja">{selection.cue.cue.plain_text}</div>
    <div class="actions">
      {#if !draft}
        <button type="button" class="primary" onclick={prepareDraft} disabled={busy || !source}>
          Prepare word card
        </button>
      {:else}
        <button
          type="button"
          class="primary"
          onclick={publish}
          disabled={busy || result?.outcome === 'created'}
        >
          Create in Anki
        </button>
        {#if publishing && extractionActive}
          <button type="button" class="secondary" onclick={cancelPublish}>Cancel extraction</button>
        {/if}
        <button type="button" class="secondary" onclick={() => (draft = null)} disabled={busy}
          >Cancel draft</button
        >
      {/if}
    </div>
  {:else}
    <div class="placeholder">
      Select a subtitle word to preview its reading and full local definition. Saving to Anki is
      optional and stays out of the way while you watch.
    </div>
  {/if}
  <p class="status" aria-live="polite">{status}</p>
</section>

<style>
  .composer {
    padding: 1.1rem;
    border: 1px solid var(--line);
    border-radius: 0;
    background: var(--panel);
  }
  .heading {
    display: flex;
    justify-content: space-between;
    align-items: start;
    margin-bottom: 1rem;
  }
  .heading-actions {
    display: flex;
    align-items: center;
    gap: 0.55rem;
  }
  .eyebrow {
    margin: 0 0 0.2rem;
    color: var(--accent-2);
    text-transform: uppercase;
    letter-spacing: 0.12em;
    font-size: 0.6rem;
    font-weight: 850;
  }
  h2 {
    margin: 0;
    font-family: Georgia, 'Times New Roman', serif;
    font-size: 1.25rem;
    font-weight: 500;
  }
  .time {
    color: var(--muted);
    font:
      0.72rem ui-monospace,
      monospace;
  }
  .close {
    display: grid;
    place-items: center;
    width: 1.75rem;
    height: 1.75rem;
    padding: 0;
    border: 1px solid var(--line);
    border-radius: 50%;
    background: transparent;
    color: var(--muted);
    font-size: 1rem;
    line-height: 1;
  }
  .close:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }
  label {
    display: grid;
    gap: 0.3rem;
    margin-bottom: 0.75rem;
    color: var(--muted);
    font-size: 0.65rem;
    font-weight: 750;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }
  input,
  textarea {
    width: 100%;
    box-sizing: border-box;
    border-radius: 1px;
    background: #f9f6ef;
    color: var(--text);
    font-size: 0.78rem;
    letter-spacing: normal;
    resize: vertical;
    text-transform: none;
  }
  .context {
    margin: 0.75rem 0;
    padding: 0.7rem;
    border-left: 3px solid var(--accent);
    background: rgb(202 63 54 / 6%);
    color: var(--text);
    font-size: 0.95rem;
    line-height: 1.5;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }
  .primary,
  .secondary {
    padding: 0.55rem 0.8rem;
    border-radius: 1px;
    font-weight: 650;
  }
  .primary {
    border: 0;
    background: var(--accent);
    color: #fff;
  }
  .secondary {
    border: 1px solid var(--line);
    background: transparent;
    color: var(--text);
  }
  button:disabled {
    opacity: 0.45;
  }
  .placeholder {
    padding: 1rem;
    border: 1px dashed var(--line);
    border-radius: 1px;
    background: rgb(255 255 255 / 28%);
    color: var(--muted);
    font-family: Georgia, 'Times New Roman', serif;
    font-size: 0.8rem;
    font-style: italic;
    line-height: 1.5;
  }
  .status {
    min-height: 1.2rem;
    margin: 0.75rem 0 0;
    color: var(--muted);
    font-size: 0.72rem;
  }
</style>
