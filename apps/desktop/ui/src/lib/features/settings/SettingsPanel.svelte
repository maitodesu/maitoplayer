<script lang="ts">
  import { onMount } from 'svelte';
  import type { AppErrorV1, AppHealthV1, UserSettingsV1 } from '../../contracts/generated';
  import { api, asAppError } from '../../shell/api';

  interface Props {
    health?: AppHealthV1 | null;
    onError: (error: AppErrorV1) => void;
    onHealth: (health: AppHealthV1) => void;
  }

  let { health = null, onError, onHealth }: Props = $props();
  let loaded = $state<UserSettingsV1 | null>(null);
  let languages = $state('jpn, ja, und');
  let cacheGiB = $state(20);
  let cacheAgeDays = $state(30);
  let leadMs = $state(250);
  let trailMs = $state(500);
  let deckName = $state('Default');
  let modelName = $state('Kiku');
  let ankiPort = $state(8765);
  let ankiTimeoutMs = $state(5000);
  let tags = $state('kiku, immersion');
  let mapping = $state<Record<string, string>>({});
  let busy = $state(false);
  let status = $state('');

  const logicalFields = [
    'expression',
    'reading',
    'sentence',
    'definition',
    'audio',
    'image',
    'source',
    'timestamp',
    'mining_id',
  ];

  onMount(() => {
    void load();
  });

  async function load(): Promise<void> {
    try {
      apply(await api.settings());
    } catch (error) {
      onError(asAppError(error));
    }
  }

  function apply(settings: UserSettingsV1): void {
    loaded = settings;
    languages = settings.preferred_audio_languages.join(', ');
    cacheGiB = settings.playback_cache_max_bytes / 1024 ** 3;
    cacheAgeDays = settings.playback_cache_max_age_days;
    leadMs = settings.clip_leading_padding_us / 1000;
    trailMs = settings.clip_trailing_padding_us / 1000;
    deckName = settings.card_profile.deck_name;
    modelName = settings.card_profile.model_name;
    ankiPort = settings.anki_port;
    ankiTimeoutMs = settings.anki_timeout_ms;
    tags = settings.card_profile.tags.join(', ');
    mapping = { ...settings.card_profile.field_mapping };
  }

  function list(value: string): string[] {
    return value
      .split(',')
      .map((item) => item.trim())
      .filter(Boolean);
  }

  async function save(): Promise<void> {
    if (!loaded) return;
    busy = true;
    status = 'Validating and saving preferences…';
    const field_mapping = Object.fromEntries(
      logicalFields
        .map((logical) => [logical, mapping[logical]?.trim() ?? ''])
        .filter(([, value]) => value),
    );
    try {
      const saved = await api.saveSettings({
        preferred_audio_languages: list(languages),
        playback_cache_max_bytes: Math.round(cacheGiB * 1024 ** 3),
        playback_cache_max_age_days: Math.round(cacheAgeDays),
        clip_leading_padding_us: Math.round(leadMs * 1000),
        clip_trailing_padding_us: Math.round(trailMs * 1000),
        anki_port: Math.round(ankiPort),
        anki_timeout_ms: Math.round(ankiTimeoutMs),
        card_profile: {
          profile_id: 'default',
          deck_name: deckName.trim(),
          model_name: modelName.trim(),
          field_mapping,
          tags: list(tags),
        },
      });
      apply(saved);
      onHealth(await api.health());
      status = 'Saved. New imports and card publishes use these preferences.';
    } catch (error) {
      onError(asAppError(error));
      status = 'Settings were not changed.';
    } finally {
      busy = false;
    }
  }
</script>

<section class="settings" aria-labelledby="settings-title">
  <div>
    <p class="eyebrow">First-run readiness</p>
    <h2 id="settings-title">App health and preferences</h2>
  </div>
  <div class="health-grid">
    {#each health?.checks ?? [] as check (check.component)}
      <article class:healthy={check.available}>
        <span class="dot" aria-hidden="true"></span>
        <div>
          <strong>{check.component}</strong>
          <p>
            {check.available ? (check.version ?? 'Ready') : (check.action ?? 'Needs attention')}
          </p>
        </div>
      </article>
    {/each}
  </div>

  {#if loaded}
    <div class="form-grid">
      <label>Preferred audio languages<input bind:value={languages} maxlength="160" /></label>
      <label
        >Playback cache budget (GiB)<input
          type="number"
          min="1"
          max="200"
          step="1"
          bind:value={cacheGiB}
        /></label
      >
      <label
        >Cache maximum age (days)<input
          type="number"
          min="1"
          max="365"
          bind:value={cacheAgeDays}
        /></label
      >
      <label
        >Audio lead padding (ms)<input
          type="number"
          min="0"
          max="5000"
          bind:value={leadMs}
        /></label
      >
      <label
        >Audio trail padding (ms)<input
          type="number"
          min="0"
          max="5000"
          bind:value={trailMs}
        /></label
      >
      <label
        >AnkiConnect port<input type="number" min="1" max="65535" bind:value={ankiPort} /></label
      >
      <label
        >Anki timeout (ms)<input
          type="number"
          min="250"
          max="30000"
          bind:value={ankiTimeoutMs}
        /></label
      >
      <label>Deck name<input bind:value={deckName} maxlength="255" /></label>
      <label>Note type<input bind:value={modelName} maxlength="255" /></label>
      <label class="wide">Tags (comma-separated)<input bind:value={tags} maxlength="1024" /></label>
    </div>

    <fieldset>
      <legend>Existing Anki note field mapping</legend>
      <div class="mapping-grid">
        {#each logicalFields as logical}
          <label>
            {logical}
            <input
              value={mapping[logical] ?? ''}
              oninput={(event) => (mapping[logical] = event.currentTarget.value)}
              maxlength="255"
            />
          </label>
        {/each}
      </div>
    </fieldset>
    <button type="button" class="save" onclick={save} disabled={busy}
      >{busy ? 'Working…' : 'Save settings'}</button
    >
  {/if}
  <p class="note">
    Video transcode always requires explicit approval. Bundled dependency errors require repair or
    reinstall; local paths are never included in diagnostics.
  </p>
  <p class="status" aria-live="polite">{status}</p>
</section>

<style>
  .settings {
    display: grid;
    gap: 1.25rem;
    max-width: 1040px;
    margin: 2rem auto;
    padding: 1.5rem;
    border: 1px solid var(--line);
    border-top: 4px solid var(--accent);
    border-radius: 2px;
    background: #151612;
    box-shadow: 0 28px 80px rgb(0 0 0 / 25%);
  }
  .eyebrow {
    margin: 0 0 0.25rem;
    color: var(--accent);
    text-transform: uppercase;
    letter-spacing: 0.12em;
    font-size: 0.68rem;
    font-weight: 700;
  }
  h2 {
    margin: 0;
    font-family: Georgia, 'Times New Roman', serif;
    font-size: 1.65rem;
    font-weight: 500;
  }
  .health-grid,
  .form-grid,
  .mapping-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 0.75rem;
  }
  article {
    display: flex;
    gap: 0.7rem;
    padding: 0.8rem;
    border: 1px solid #5a3935;
    border-radius: 1px;
    background: #211714;
  }
  article.healthy {
    border-color: #3e4c42;
    background: #18201b;
  }
  .dot {
    flex: 0 0 auto;
    width: 0.55rem;
    height: 0.55rem;
    margin-top: 0.25rem;
    border-radius: 999px;
    background: #e76f74;
  }
  .healthy .dot {
    background: var(--accent);
    box-shadow: 0 0 12px rgb(96 215 163 / 50%);
  }
  article p {
    margin: 0.2rem 0 0;
    color: var(--muted);
    font-size: 0.75rem;
  }
  label {
    display: grid;
    gap: 0.35rem;
    color: var(--muted);
    font-size: 0.75rem;
  }
  .wide {
    grid-column: 1 / -1;
  }
  fieldset {
    border: 1px solid var(--line);
    border-radius: 1px;
    padding: 1rem;
  }
  legend {
    color: var(--muted);
    font-size: 0.78rem;
    padding: 0 0.35rem;
  }
  button {
    padding: 0.55rem 0.8rem;
    border: 1px solid var(--line);
    border-radius: 1px;
    background: transparent;
    color: var(--text);
    font-weight: 650;
  }
  .save {
    border: 0;
    background: var(--accent);
    color: #fff;
    justify-self: start;
  }
  button:disabled {
    opacity: 0.5;
  }
  .note,
  .status {
    margin: 0;
    color: var(--muted);
    font-size: 0.75rem;
  }
  @media (max-width: 700px) {
    .health-grid,
    .form-grid,
    .mapping-grid {
      grid-template-columns: 1fr;
    }
    .wide {
      grid-column: auto;
    }
  }
</style>
