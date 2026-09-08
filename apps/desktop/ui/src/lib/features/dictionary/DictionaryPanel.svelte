<script lang="ts">
  import { tick } from 'svelte';
  import type { AnalyzedCueV1, AnalyzedTokenV1 } from '../../contracts/generated';
  import { readingToHiragana } from '../live-subtitle/reading';
  import LearningMetadata from './LearningMetadata.svelte';
  import { calculatePopoverLayout } from './popover-layout';

  type Selection = { cue: AnalyzedCueV1; token: AnalyzedTokenV1 };

  interface Props {
    selection?: Selection | null;
    anchor?: HTMLElement | null;
    onDismiss?: () => void;
    onInteractStart?: () => void;
    onInteractEnd?: () => void;
  }

  let {
    selection = null,
    anchor = null,
    onDismiss = () => undefined,
    onInteractStart = () => undefined,
    onInteractEnd = () => undefined,
  }: Props = $props();
  let popover = $state<HTMLElement | null>(null);
  let left = $state(16);
  let top = $state(16);
  let width = $state(480);
  let maxHeight = $state(448);
  let positionedSelectionKey: string | null = null;

  $effect(() => {
    const selectionKey = selection
      ? `${selection.cue.cue.cue_id}:${selection.token.token.token_id}`
      : null;
    anchor;
    if (!selection || !anchor) {
      positionedSelectionKey = null;
      return;
    }
    const resetScroll = selectionKey !== positionedSelectionKey;
    positionedSelectionKey = selectionKey;
    void positionPopover(resetScroll);
  });

  async function positionPopover(resetScroll = false): Promise<void> {
    await tick();
    if (!anchor?.isConnected || !popover) return;

    const anchorRect = anchor.getBoundingClientRect();
    width = Math.min(480, Math.max(0, window.innerWidth - 32));
    maxHeight = Math.min(448, Math.max(0, window.innerHeight - 32));
    await tick();
    if (!anchor?.isConnected || !popover) return;
    if (resetScroll) popover.scrollTop = 0;
    const borderHeight = Math.max(0, popover.offsetHeight - popover.clientHeight);
    const layout = calculatePopoverLayout({
      viewportWidth: window.innerWidth,
      viewportHeight: window.innerHeight,
      anchor: anchorRect,
      desiredHeight: popover.scrollHeight + borderHeight,
    });
    left = layout.left;
    top = layout.top;
    width = layout.width;
    maxHeight = layout.maxHeight;
  }

  function repositionPopover(): void {
    void positionPopover();
  }

  function handleWindowKeydown(event: KeyboardEvent): void {
    if (selection && event.key === 'Escape') {
      event.preventDefault();
      onDismiss();
    }
  }

  function isCommon(selectionEntry: AnalyzedTokenV1['dictionary'][number]): boolean {
    return selectionEntry.senses.some((sense) =>
      sense.misc.some((tag) => tag.toLocaleLowerCase().includes('common')),
    );
  }
</script>

<svelte:window
  onresize={repositionPopover}
  onscroll={repositionPopover}
  onfullscreenchange={repositionPopover}
  onkeydown={handleWindowKeydown}
/>

{#if selection && anchor}
  <div
    bind:this={popover}
    id="subtitle-dictionary-popover"
    class="popover"
    role="dialog"
    aria-label={`Dictionary definition for ${selection.token.token.surface}`}
    tabindex="-1"
    style={`--popover-left: ${left}px; --popover-top: ${top}px; --popover-width: ${width}px; --popover-max-height: ${maxHeight}px`}
    onpointerenter={onInteractStart}
    onpointerleave={onInteractEnd}
    onfocusin={onInteractStart}
    onfocusout={onInteractEnd}
  >
    <header>
      <div>
        <p class="eyebrow">Local JMdict</p>
        <div class="word-line">
          <h2 lang="ja">{selection.token.token.surface}</h2>
          <span class="reading" lang="ja">{readingToHiragana(selection.token.token.reading)}</span>
        </div>
      </div>
      <button type="button" class="close" onclick={onDismiss} aria-label="Close dictionary"
        >×</button
      >
    </header>
    <div class="word-meta">
      {#if selection.token.token.lemma !== selection.token.token.surface}
        <span lang="ja">{selection.token.token.lemma}</span>
      {/if}
      {#each selection.token.token.part_of_speech as part}
        <span>{part}</span>
      {/each}
    </div>
    {#if selection.token.dictionary.length === 0}
      <div class="empty">
        <strong>No local definition found</strong>
        <span>The reading and sentence context are still available.</span>
      </div>
    {:else}
      <ol class="entries">
        {#each selection.token.dictionary.slice(0, 3) as entry (entry.entry_id)}
          <li>
            <div class="entry-title">
              <strong lang="ja"
                >{entry.headwords.join('・') ||
                  entry.readings.map(readingToHiragana).join('・')}</strong
              >
              {#if isCommon(entry)}
                <span class="common">Common</span>
              {/if}
            </div>
            <p class="entry-reading" lang="ja">
              {entry.readings.map(readingToHiragana).join('・')}
            </p>
            <LearningMetadata metadata={entry} />
            {#each entry.senses as sense, index}
              {@const tags = [
                ...sense.parts_of_speech,
                ...sense.fields.map((value) => `field: ${value}`),
                ...sense.dialects.map((value) => `dialect: ${value}`),
                ...sense.misc,
              ]}
              <div class="sense">
                <span class="sense-number">{index + 1}</span>
                <div>
                  <p>{sense.glosses.join('; ')}</p>
                  {#if tags.length > 0}
                    <div class="sense-tags" aria-label="Sense tags">
                      {#each tags as tag}
                        <span>{tag}</span>
                      {/each}
                    </div>
                  {/if}
                </div>
              </div>
            {/each}
          </li>
        {/each}
      </ol>
    {/if}
    <footer>Hover another word or use Tab to explore the subtitle.</footer>
  </div>
{/if}

<style>
  .popover {
    --text: #1b1a17;
    --muted: #706c64;
    --line: #cbc5b8;
    --chip: #e4dfd4;
    --accent: #cf4037;
    --accent-2: #a23730;
    position: fixed;
    z-index: 100;
    top: var(--popover-top);
    left: var(--popover-left);
    width: var(--popover-width);
    max-height: min(28rem, var(--popover-max-height), calc(100vh - 2rem));
    overscroll-behavior: contain;
    scrollbar-gutter: stable;
    padding: 1rem 1.05rem;
    overflow: auto;
    border: 1px solid var(--line);
    border-top: 4px solid var(--accent);
    border-radius: 2px;
    background: rgb(248 245 237 / 98%);
    color: var(--text);
    box-shadow: 0 28px 80px rgb(0 0 0 / 56%);
    backdrop-filter: blur(14px);
  }
  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
    border-bottom: 1px solid var(--line);
    padding-bottom: 0.8rem;
  }
  .word-line {
    display: flex;
    align-items: baseline;
    gap: 0.65rem;
  }
  .eyebrow {
    color: var(--accent);
    margin: 0 0 0.22rem;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    font-size: 0.6rem;
    font-weight: 850;
  }
  h2 {
    margin: 0;
    font-family: 'Noto Sans JP', sans-serif;
    font-size: 1.7rem;
  }
  .reading {
    color: var(--accent-2);
    font-size: 0.9rem;
  }
  .close {
    display: grid;
    place-items: center;
    width: 2rem;
    height: 2rem;
    padding: 0;
    border: 1px solid var(--line);
    border-radius: 50%;
    background: transparent;
    color: var(--muted);
    font-size: 1.15rem;
  }
  .word-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0.45rem;
    margin: 0.9rem 0;
  }
  .word-meta span {
    padding: 0.25rem 0.5rem;
    border-radius: 1px;
    background: var(--chip);
    color: var(--muted);
    font-size: 0.75rem;
  }
  .entries {
    display: grid;
    gap: 0.8rem;
    padding: 0;
    margin: 0.7rem 0;
    list-style: none;
  }
  .entries li {
    padding: 0.8rem 0.85rem;
    border: 1px solid var(--line);
    border-radius: 1px;
    background: rgb(255 255 255 / 45%);
  }
  .entry-title {
    display: flex;
    justify-content: space-between;
  }
  .common {
    align-self: center;
    padding: 0.16rem 0.45rem;
    border-radius: 1px;
    background: var(--accent);
    color: #fff;
    font-size: 0.67rem;
    font-weight: 700;
  }
  .entry-reading {
    margin: 0.15rem 0 0.55rem;
    color: var(--accent-2);
    font-size: 0.8rem;
  }
  .sense {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 0.35rem;
    font-size: 0.85rem;
  }
  .sense p {
    margin: 0 0 0.35rem;
  }
  .sense-number {
    display: grid;
    place-items: center;
    width: 1.25rem;
    height: 1.25rem;
    border: 0;
    border-radius: 0;
    color: var(--accent);
    font-size: 0.68rem;
  }
  .sense-tags {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
    margin-bottom: 0.55rem;
  }
  .sense-tags span {
    padding: 0.12rem 0.38rem;
    border: 1px solid var(--line);
    border-radius: 1px;
    color: var(--muted);
    font-size: 0.64rem;
  }
  .empty {
    display: grid;
    gap: 0.3rem;
    padding: 1rem 0.2rem;
    color: var(--muted);
    font-size: 0.85rem;
    line-height: 1.6;
  }
  .empty strong {
    color: var(--text);
  }
  footer {
    margin: 0.8rem -1rem -1rem;
    padding: 0.65rem 1rem;
    border-top: 1px solid var(--line);
    color: var(--muted);
    background: #efebe1;
    font-family: Georgia, 'Times New Roman', serif;
    font-size: 0.68rem;
    font-style: italic;
  }
  @media (max-width: 520px) {
    .popover {
      max-height: min(24rem, calc(100vh - 1rem));
    }
    header {
      gap: 0.5rem;
    }
    .word-line {
      display: grid;
      gap: 0.1rem;
      min-width: 0;
    }
    h2,
    .reading {
      overflow-wrap: anywhere;
    }
    h2 {
      font-size: 1.45rem;
    }
    .close {
      flex: 0 0 auto;
    }
  }
</style>
