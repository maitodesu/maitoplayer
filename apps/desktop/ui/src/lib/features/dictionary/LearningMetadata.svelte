<script lang="ts">
  import {
    contourPoints,
    describePitchAccent,
    extractLearningMetadata,
    followingParticleLevel,
    pitchAccentName,
  } from './learning-metadata';

  interface Props {
    metadata: unknown;
  }

  let { metadata }: Props = $props();
  const learning = $derived(extractLearningMetadata(metadata));
  const visiblePitchAccents = $derived(learning.pitchAccents.slice(0, 3));
</script>

{#key metadata}
  {#if visiblePitchAccents.length > 0 || learning.jlptLevel !== null}
    <section class="learning" aria-label="Japanese learning metadata">
      {#if visiblePitchAccents.length > 0}
        <details class="pitch-accordion">
          <summary>
            <span class="summary-copy">
              <span class="summary-title">Pitch accent</span>
              <small
                >{visiblePitchAccents.length}
                {visiblePitchAccents.length === 1 ? 'pattern' : 'patterns'}</small
              >
            </span>
            <span class="summary-end">
              {#if learning.jlptLevel !== null}
                <span
                  class="jlpt"
                  aria-label={`Estimated Japanese Language Proficiency Test vocabulary level N${learning.jlptLevel}`}
                  title={learning.jlptSource
                    ? `Estimated JLPT vocabulary level · ${learning.jlptSource}`
                    : 'Estimated JLPT vocabulary level'}
                  >JLPT N{learning.jlptLevel} <small>estimate</small></span
                >
              {/if}
              <span class="chevron" aria-hidden="true"></span>
            </span>
          </summary>
          <div class="patterns" aria-label="Tokyo Japanese pitch-accent patterns">
            {#each visiblePitchAccents as pattern, index (`${pattern.reading}-${pattern.drop_after_mora}-${index}`)}
              <figure class="pattern">
                <figcaption>
                  <span>{pitchAccentName(pattern)}</span>
                  {#if pattern.drop_after_mora !== null}
                    <span class="drop">drop {pattern.drop_after_mora}</span>
                  {/if}
                </figcaption>
                <div class="contour" style={`--mora-count: ${pattern.morae.length + 1}`}>
                  <svg
                    viewBox={`0 0 ${(pattern.morae.length + 1) * 48} 36`}
                    preserveAspectRatio="none"
                    role="img"
                    aria-label={describePitchAccent(pattern)}
                  >
                    {#if pattern.levels.length > 1}
                      <polyline points={contourPoints(pattern)} />
                    {/if}
                    {#each pattern.levels as level, moraIndex}
                      <line
                        class="point"
                        x1={24 + moraIndex * 48}
                        x2={24 + moraIndex * 48}
                        y1={level === 'high' ? 8 : 28}
                        y2={level === 'high' ? 8 : 28}
                      />
                    {/each}
                    <line
                      class="point particle-point"
                      x1={24 + pattern.morae.length * 48}
                      x2={24 + pattern.morae.length * 48}
                      y1={followingParticleLevel(pattern) === 'high' ? 8 : 28}
                      y2={followingParticleLevel(pattern) === 'high' ? 8 : 28}
                    />
                  </svg>
                  <div class="morae" aria-hidden="true">
                    {#each pattern.morae as mora}
                      <span lang="ja">{mora}</span>
                    {/each}
                    <span class="particle" lang="ja">が</span>
                  </div>
                </div>
                {#if pattern.source}
                  <span class="source" title={`Pitch-accent source: ${pattern.source}`}
                    >Tokyo · {pattern.source}</span
                  >
                {/if}
              </figure>
            {/each}
          </div>
        </details>
      {:else if learning.jlptLevel !== null}
        <div class="metadata-row">
          <span class="summary-title">Learning level</span>
          <span
            class="jlpt"
            aria-label={`Estimated Japanese Language Proficiency Test vocabulary level N${learning.jlptLevel}`}
            title={learning.jlptSource
              ? `Estimated JLPT vocabulary level · ${learning.jlptSource}`
              : 'Estimated JLPT vocabulary level'}
            >JLPT N{learning.jlptLevel} <small>estimate</small></span
          >
        </div>
      {/if}
    </section>
  {/if}
{/key}

<style>
  .learning {
    padding: 0;
    margin: 0.85rem 0 0.65rem;
    border: 1px solid var(--line);
    border-radius: 2px;
    background: rgb(255 255 255 / 38%);
    overflow: hidden;
  }
  summary,
  .metadata-row,
  figcaption {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.6rem;
  }
  summary,
  .metadata-row {
    min-height: 2.3rem;
    padding: 0.45rem 0.7rem;
  }
  summary {
    cursor: pointer;
    list-style: none;
    outline: none;
    transition: background 150ms ease;
  }
  summary::-webkit-details-marker {
    display: none;
  }
  summary:hover {
    background: color-mix(in srgb, var(--accent) 5%, transparent);
  }
  summary:focus-visible {
    box-shadow: inset 0 0 0 2px color-mix(in srgb, var(--accent) 72%, transparent);
  }
  .summary-copy,
  .summary-end {
    display: flex;
    align-items: center;
  }
  .summary-copy {
    min-width: 0;
    gap: 0.45rem;
  }
  .summary-copy > small {
    color: var(--muted);
    font-size: 0.62rem;
    font-weight: 650;
  }
  .summary-end {
    flex: 0 0 auto;
    gap: 0.55rem;
  }
  .summary-title {
    color: var(--muted);
    font-size: 0.65rem;
    font-weight: 800;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }
  .chevron {
    width: 0.48rem;
    height: 0.48rem;
    border-right: 1.5px solid var(--muted);
    border-bottom: 1.5px solid var(--muted);
    transform: rotate(45deg) translateY(-0.12rem);
    transition: transform 150ms ease;
  }
  details[open] .chevron {
    transform: rotate(225deg) translate(-0.08rem, -0.02rem);
  }
  .jlpt {
    padding: 0.18rem 0.45rem;
    border: 1px solid color-mix(in srgb, var(--accent) 40%, var(--line));
    border-radius: 999px;
    color: var(--accent-2);
    background: color-mix(in srgb, var(--accent) 8%, transparent);
    font-size: 0.65rem;
    font-weight: 800;
    letter-spacing: 0.03em;
  }
  .jlpt small {
    font: inherit;
    font-size: 0.56rem;
    font-weight: 650;
    opacity: 0.72;
  }
  .patterns {
    display: grid;
    gap: 0.65rem;
    padding: 0.7rem;
    border-top: 1px solid color-mix(in srgb, var(--line) 75%, transparent);
  }
  .pattern {
    min-width: 0;
    margin: 0;
  }
  figcaption {
    margin-bottom: 0.18rem;
    color: var(--text);
    font-size: 0.72rem;
    font-weight: 750;
  }
  .drop,
  .source {
    color: var(--muted);
    font-size: 0.62rem;
    font-weight: 600;
  }
  .contour {
    min-width: 0;
  }
  svg {
    display: block;
    width: 100%;
    height: 1.8rem;
    overflow: visible;
  }
  polyline {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
    vector-effect: non-scaling-stroke;
  }
  .point {
    stroke: var(--accent);
    stroke-width: 6;
    stroke-linecap: round;
    vector-effect: non-scaling-stroke;
  }
  .morae {
    display: grid;
    grid-template-columns: repeat(var(--mora-count), minmax(0, 1fr));
    border-top: 1px solid color-mix(in srgb, var(--line) 75%, transparent);
  }
  .morae span {
    min-width: 0;
    padding-top: 0.15rem;
    overflow: hidden;
    color: var(--text);
    font-family: 'Noto Sans JP', sans-serif;
    font-size: 0.72rem;
    text-align: center;
    text-overflow: ellipsis;
  }
  .source {
    display: block;
    margin-top: 0.18rem;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .particle-point,
  .morae .particle {
    opacity: 0.4;
  }
  @media (max-width: 520px) {
    summary,
    .metadata-row,
    .patterns {
      padding-inline: 0.6rem;
    }
    svg {
      height: 1.55rem;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    summary,
    .chevron {
      transition: none;
    }
  }
</style>
