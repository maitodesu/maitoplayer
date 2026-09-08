<script lang="ts">
  import { onMount } from 'svelte';
  import {
    applyTheme,
    chooseTheme,
    initialTheme,
    isThemeId,
    resolveAppliedTheme,
    THEME_STORAGE_KEY,
    themeOptions,
    type ThemeId,
  } from './theme';

  interface Props {
    mode?: 'compact' | 'panel';
    label?: string;
  }

  let { mode = 'panel', label = 'Appearance' }: Props = $props();
  let selected = $state<ThemeId>(
    resolveAppliedTheme(
      typeof document === 'undefined' ? undefined : document.documentElement.dataset.theme,
      initialTheme,
    ),
  );

  function selectTheme(theme: ThemeId): void {
    selected = theme;
    chooseTheme(theme);
  }

  onMount(() => {
    const handleThemeChange = (event: Event) => {
      const theme = (event as CustomEvent<unknown>).detail;
      if (isThemeId(theme)) selected = theme;
    };
    const handleStorage = (event: StorageEvent) => {
      if (event.key === THEME_STORAGE_KEY && isThemeId(event.newValue)) {
        selected = event.newValue;
        applyTheme(event.newValue, document.documentElement);
      }
    };
    window.addEventListener('maitoplayer-theme-change', handleThemeChange);
    window.addEventListener('storage', handleStorage);
    return () => {
      window.removeEventListener('maitoplayer-theme-change', handleThemeChange);
      window.removeEventListener('storage', handleStorage);
    };
  });
</script>

<fieldset class="theme-picker" class:compact={mode === 'compact'}>
  <legend class:visually-hidden={mode === 'compact'}>{label}</legend>
  <div class="theme-options" aria-label={mode === 'compact' ? label : undefined}>
    {#each themeOptions as theme (theme.id)}
      <label class="theme-option" class:selected={selected === theme.id}>
        <input
          type="radio"
          name="maitoplayer-theme"
          value={theme.id}
          checked={selected === theme.id}
          onchange={() => selectTheme(theme.id)}
        />
        <span class="swatches" aria-hidden="true">
          {#each theme.swatches as swatch}
            <span style:background={swatch}></span>
          {/each}
        </span>
        <span class="theme-copy">
          <strong>{mode === 'compact' ? theme.shortLabel : theme.label}</strong>
          {#if mode === 'panel'}<small>{theme.description}</small>{/if}
        </span>
        <span class="selection-mark" aria-hidden="true">✓</span>
      </label>
    {/each}
  </div>
</fieldset>

<style>
  .theme-picker {
    min-width: 0;
    margin: 0;
    padding: 0;
    border: 0;
    color: var(--text);
  }
  legend {
    margin-bottom: 0.7rem;
    color: var(--text);
    font-size: 0.72rem;
    font-weight: 800;
    letter-spacing: 0.09em;
    text-transform: uppercase;
  }
  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }
  .theme-options {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 0.65rem;
  }
  .theme-option {
    position: relative;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 0.7rem;
    min-width: 0;
    min-height: 4.4rem;
    padding: 0.72rem;
    border: 1px solid var(--line);
    border-radius: var(--theme-control-radius, 3px);
    background: var(--panel);
    color: var(--text);
    cursor: pointer;
    transition:
      border-color 160ms ease,
      background 160ms ease,
      transform 160ms ease;
  }
  .theme-option:hover {
    border-color: var(--accent);
    transform: translateY(-1px);
  }
  .theme-option.selected {
    border-color: var(--accent);
    background: var(--theme-selected-surface, var(--chip));
    box-shadow: var(--theme-picker-selected-shadow, none);
  }
  input {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    opacity: 0;
  }
  input:focus-visible + .swatches {
    outline: 2px solid var(--accent-strong, var(--accent));
    outline-offset: 4px;
  }
  .swatches {
    display: flex;
    overflow: hidden;
    width: 2.7rem;
    height: 1.7rem;
    border: 1px solid rgb(127 127 127 / 35%);
    border-radius: var(--theme-control-radius, 3px);
  }
  .swatches span {
    flex: 1;
  }
  .theme-copy {
    min-width: 0;
  }
  .theme-copy strong,
  .theme-copy small {
    display: block;
  }
  .theme-copy strong {
    overflow: hidden;
    font-size: 0.72rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .theme-copy small {
    margin-top: 0.25rem;
    color: var(--muted);
    font-size: 0.66rem;
    line-height: 1.35;
  }
  .selection-mark {
    display: grid;
    width: 1.25rem;
    height: 1.25rem;
    place-items: center;
    border: 1px solid var(--line);
    border-radius: 50%;
    color: transparent;
    font-size: 0.68rem;
  }
  .selected .selection-mark {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--theme-on-accent, #fff);
  }
  .compact .theme-options {
    display: flex;
    gap: 0.28rem;
    padding: 0.22rem;
    border: 1px solid var(--line);
    border-radius: var(--theme-control-radius, 3px);
    background: var(--theme-picker-track, transparent);
  }
  .compact .theme-option {
    display: block;
    min-height: 0;
    padding: 0.42rem 0.58rem;
    border: 0;
    background: transparent;
    text-align: center;
  }
  .compact .theme-option:hover {
    transform: none;
  }
  .compact .theme-option.selected {
    background: var(--theme-selected-surface, var(--chip));
    box-shadow: none;
  }
  .compact .swatches,
  .compact .selection-mark {
    display: none;
  }
  .compact .theme-copy strong {
    font-size: 0.64rem;
    letter-spacing: 0.025em;
  }
  .compact input:focus-visible ~ .theme-copy {
    outline: 2px solid var(--accent-strong, var(--accent));
    outline-offset: 4px;
  }
  @media (max-width: 720px) {
    .theme-options {
      grid-template-columns: 1fr;
    }
    .compact .theme-option {
      padding-inline: 0.44rem;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .theme-option {
      transition: none;
    }
  }
</style>
