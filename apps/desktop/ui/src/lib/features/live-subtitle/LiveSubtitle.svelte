<script lang="ts">
  import { onDestroy } from 'svelte';
  import type { AnalyzedCueV1, AnalyzedTokenV1, SubtitleCueV1 } from '../../contracts/generated';
  import DictionaryPanel from '../dictionary/DictionaryPanel.svelte';
  import { readingToHiragana } from './reading';
  import { containsSelection, isSameSelection, type SubtitleTokenSelection } from './selection';

  interface Props {
    cues: AnalyzedCueV1[];
    translationCues?: SubtitleCueV1[];
    selection?: SubtitleTokenSelection | null;
    showFurigana?: boolean;
    onSelect: (selection: SubtitleTokenSelection) => void;
    onDismiss?: () => void;
  }

  let {
    cues,
    translationCues = [],
    selection = null,
    showFurigana = true,
    onSelect,
    onDismiss = () => undefined,
  }: Props = $props();
  let previewSelection = $state<SubtitleTokenSelection | null>(null);
  let previewAnchor = $state<HTMLElement | null>(null);
  let selectedAnchor = $state<HTMLElement | null>(null);
  let closeTimer: ReturnType<typeof setTimeout> | null = null;
  let suppressedFocusAnchor: HTMLElement | null = null;

  const popoverSelection = $derived(previewSelection ?? selection);
  const popoverAnchor = $derived(previewAnchor ?? selectedAnchor);

  $effect(() => {
    if (!containsSelection(cues, selection)) selectedAnchor = null;
    if (!containsSelection(cues, previewSelection)) {
      previewSelection = null;
      previewAnchor = null;
    }
  });

  function showReading(token: AnalyzedTokenV1): boolean {
    return (
      showFurigana &&
      token.token.reading !== token.token.surface &&
      /[\u3400-\u9fff]/u.test(token.token.surface)
    );
  }

  function clearCloseTimer(): void {
    if (closeTimer !== null) clearTimeout(closeTimer);
    closeTimer = null;
  }

  function preview(value: SubtitleTokenSelection, event: PointerEvent | FocusEvent): void {
    const eventAnchor = event.currentTarget as HTMLElement;
    if (event.type === 'focus' && suppressedFocusAnchor === eventAnchor) {
      suppressedFocusAnchor = null;
      return;
    }
    clearCloseTimer();
    previewSelection = value;
    previewAnchor = eventAnchor;
  }

  function select(value: SubtitleTokenSelection, event: MouseEvent): void {
    clearCloseTimer();
    selectedAnchor = event.currentTarget as HTMLElement;
    previewSelection = null;
    previewAnchor = null;
    if (isSameSelection(selection, value.cue, value.token)) return;
    onSelect(value);
  }

  function schedulePreviewClose(): void {
    clearCloseTimer();
    closeTimer = setTimeout(() => {
      previewSelection = null;
      previewAnchor = null;
      closeTimer = null;
    }, 250);
  }

  function dismiss(): void {
    const anchorToRestore = popoverAnchor;
    const focusWasInPopover =
      typeof document !== 'undefined' &&
      document.activeElement instanceof Element &&
      document.activeElement.closest('#subtitle-dictionary-popover') !== null;
    clearCloseTimer();
    previewSelection = null;
    previewAnchor = null;
    selectedAnchor = null;
    onDismiss();
    if (focusWasInPopover && anchorToRestore?.isConnected) {
      suppressedFocusAnchor = anchorToRestore;
      queueMicrotask(() => anchorToRestore.focus({ preventScroll: true }));
    }
  }

  onDestroy(clearCloseTimer);
</script>

<section
  class="subtitle-stage"
  aria-label="Interactive Japanese subtitles with optional English translation"
  aria-live="polite"
>
  {#if cues.length === 0 && translationCues.length === 0}
    <p class="empty">No subtitle cue at this moment.</p>
  {:else}
    {#if cues.length > 0}
      <div class="japanese-cues" aria-label="Japanese study subtitles">
        {#each cues as cue (cue.cue.cue_id)}
          <p lang="ja">
            {#each cue.tokens as token (token.token.token_id)}
              {#if token.token.lookup_candidate}
                <button
                  type="button"
                  class:active={isSameSelection(selection, cue, token)}
                  class="token"
                  data-subtitle-token-id={token.token.token_id}
                  aria-pressed={isSameSelection(selection, cue, token)}
                  aria-haspopup="dialog"
                  aria-controls={isSameSelection(popoverSelection, cue, token)
                    ? 'subtitle-dictionary-popover'
                    : undefined}
                  aria-expanded={isSameSelection(popoverSelection, cue, token)}
                  onpointerenter={(event) => preview({ cue, token }, event)}
                  onpointerleave={schedulePreviewClose}
                  onfocus={(event) => preview({ cue, token }, event)}
                  onblur={schedulePreviewClose}
                  onclick={(event) => select({ cue, token }, event)}
                >
                  {#if showReading(token)}
                    <ruby
                      >{token.token.surface}<rt>{readingToHiragana(token.token.reading)}</rt></ruby
                    >
                  {:else}
                    {token.token.surface}
                  {/if}
                </button>
              {:else}
                <span>{token.token.surface}</span>
              {/if}
            {/each}
          </p>
        {/each}
      </div>
    {/if}
    {#if translationCues.length > 0}
      <div class="translation-cues" aria-label="English translation subtitles">
        {#each translationCues as cue (cue.cue_id)}
          <p class="translation" lang="en">{cue.plain_text}</p>
        {/each}
      </div>
    {/if}
  {/if}

  <DictionaryPanel
    selection={popoverAnchor ? popoverSelection : null}
    anchor={popoverAnchor}
    onDismiss={dismiss}
    onInteractStart={clearCloseTimer}
    onInteractEnd={schedulePreviewClose}
  />
</section>

<style>
  .subtitle-stage {
    position: relative;
    /* Do not create a windowed stacking context here. The fixed dictionary
       must escape above both the player HUD and the sticky application bar.
       Fullscreen establishes its own explicit subtitle layer below. */
    min-height: 7.25rem;
    display: grid;
    align-content: center;
    justify-items: center;
    gap: 0.42rem;
    padding: 1.15rem 1.3rem 1.25rem;
    border: 0;
    border-top: 1px solid rgb(240 236 227 / 10%);
    border-radius: 0;
    background: #151612;
    box-shadow: inset 3px 0 0 var(--accent);
  }
  .japanese-cues {
    display: grid;
    justify-items: center;
    gap: 0.25rem;
    max-width: 100%;
  }
  .japanese-cues p {
    margin: 0;
    text-align: center;
    color: #f8f4eb;
    font-family: 'Noto Sans JP', sans-serif;
    font-weight: 620;
    font-size: clamp(1.35rem, 2.7vw, 2rem);
    line-height: 1.75;
  }
  .translation-cues {
    display: grid;
    justify-items: center;
    gap: 0.18rem;
    width: min(100%, 72rem);
    padding-top: 0.46rem;
    border-top: 1px solid rgb(240 236 227 / 13%);
  }
  .translation {
    display: -webkit-box;
    max-width: 100%;
    margin: 0;
    overflow: hidden;
    color: rgb(240 236 227 / 84%);
    font-size: clamp(0.92rem, 1.65vw, 1.18rem);
    font-weight: 500;
    line-height: 1.42;
    text-align: center;
    overflow-wrap: anywhere;
    white-space: pre-line;
    text-shadow:
      0 1px 2px #000,
      0 2px 9px #000;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 3;
    line-clamp: 3;
  }
  .empty {
    margin: 0;
    color: var(--muted);
    font:
      0.9rem/1.5 system-ui,
      sans-serif;
  }
  .token {
    margin: 0;
    padding: 0.08rem 0.12rem;
    border: 0;
    border-radius: 2px;
    background: transparent;
    color: inherit;
    font: inherit;
    line-height: inherit;
    cursor: pointer;
    text-decoration: underline solid rgb(217 72 63 / 45%) 0.08em;
    text-underline-offset: 0.2em;
  }
  .token:hover,
  .token.active {
    background: rgb(217 72 63 / 22%);
    color: #fff6ee;
  }
  .token:focus-visible {
    background: rgb(217 72 63 / 22%);
    color: #fff6ee;
    outline: 2px solid #fffdf7;
    outline-offset: 2px;
  }
  rt {
    color: #fffdf7;
    font-size: 0.45em;
    font-weight: 780;
    text-shadow:
      -1px -1px 2px #000,
      1px -1px 2px #000,
      -1px 1px 2px #000,
      1px 1px 2px #000,
      0 0 10px #000,
      0 2px 4px #000;
  }
  :global(.playback-experience:fullscreen) .subtitle-stage {
    position: absolute;
    z-index: 8;
    right: max(1rem, 4vw);
    bottom: 4.7rem;
    left: max(1rem, 4vw);
    max-height: calc(100vh - 10.5rem);
    min-height: 5.5rem;
    width: min(calc(100vw - max(2rem, 8vw)), 78rem);
    margin-inline: auto;
    padding: 0.85rem 1.15rem 0.95rem;
    border: 1px solid rgb(255 253 247 / 18%);
    border-radius: 2px;
    background: rgb(7 8 6 / 84%);
    box-shadow:
      inset 3px 0 0 var(--accent),
      0 16px 50px rgb(0 0 0 / 46%);
    /* A backdrop filter makes this element the containing block for the
       viewport-positioned dictionary, pushing that popover off-screen in
       fullscreen. The 84% plate and text outlines provide the contrast. */
    backdrop-filter: none;
    pointer-events: none;
    transition: bottom 220ms ease;
  }
  :global(.playback-experience:fullscreen:has(> .player.controls-idle)) .subtitle-stage {
    bottom: max(0.5rem, env(safe-area-inset-bottom));
  }
  :global(.playback-experience:fullscreen) .subtitle-stage p,
  :global(.playback-experience:fullscreen) .subtitle-stage button,
  :global(.playback-experience:fullscreen) .subtitle-stage :global(.popover) {
    pointer-events: auto;
  }
  :global(.playback-experience:fullscreen) .japanese-cues p {
    font-size: clamp(1.65rem, 3.1vw, 2.8rem);
    text-shadow:
      0 2px 14px #000,
      0 1px 3px #000;
  }
  :global(.playback-experience:fullscreen) .translation-cues {
    width: min(94vw, 76rem);
    max-height: min(9rem, 24vh);
    overflow: hidden;
    border-top-color: rgb(255 253 247 / 18%);
  }
  :global(.playback-experience:fullscreen) .translation {
    font-size: clamp(1rem, 1.75vw, 1.5rem);
    text-shadow:
      0 2px 12px #000,
      0 1px 3px #000;
  }

  @media (prefers-reduced-motion: reduce) {
    :global(.playback-experience:fullscreen) .subtitle-stage {
      transition: none;
    }
  }

  @media (max-width: 720px) {
    .subtitle-stage {
      padding-inline: 0.8rem;
    }
    .japanese-cues p {
      font-size: clamp(1.15rem, 5vw, 1.5rem);
    }
    .translation {
      font-size: 0.88rem;
      line-height: 1.35;
    }
  }
</style>
