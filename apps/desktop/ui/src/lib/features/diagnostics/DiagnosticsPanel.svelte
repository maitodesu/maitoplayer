<script lang="ts">
  import type { AppHealthV1, MediaSessionV1, SubtitleSourceV1 } from '../../contracts/generated';
  import { buildDiagnosticsPreview, exportDiagnostics } from './redaction';

  interface Props {
    health?: AppHealthV1 | null;
    session?: MediaSessionV1 | null;
    subtitle?: SubtitleSourceV1 | null;
  }

  let { health = null, session = null, subtitle = null }: Props = $props();
  const report = $derived(buildDiagnosticsPreview(health, session, subtitle));
  const preview = $derived(JSON.stringify(report, null, 2));
</script>

<section class="diagnostics" aria-labelledby="diagnostics-title">
  <p class="eyebrow">Redacted preview</p>
  <h2 id="diagnostics-title">Diagnostics</h2>
  <p>Review before exporting. Full media and subtitle paths are excluded from this preview.</p>
  <button type="button" onclick={() => exportDiagnostics(report)}>Export redacted JSON</button>
  <pre>{preview}</pre>
</section>

<style>
  .diagnostics {
    max-width: 960px;
    margin: 2rem auto;
    padding: 1.5rem;
    border: 1px solid var(--line);
    border-top: 4px solid var(--accent);
    border-radius: 2px;
    background: #151612;
    box-shadow: 0 28px 80px rgb(0 0 0 / 25%);
  }
  .eyebrow {
    margin: 0;
    color: var(--accent-2);
    text-transform: uppercase;
    letter-spacing: 0.12em;
    font-size: 0.68rem;
    font-weight: 700;
  }
  h2 {
    margin: 0.25rem 0;
    font-family: Georgia, 'Times New Roman', serif;
    font-size: 1.7rem;
    font-weight: 500;
  }
  p {
    color: var(--muted);
    font-size: 0.8rem;
  }
  pre {
    max-height: 62vh;
    overflow: auto;
    padding: 1rem;
    border: 1px solid var(--line);
    border-radius: 1px;
    background: #0b0c0a;
    color: #d0ccc2;
    font:
      0.72rem/1.6 ui-monospace,
      monospace;
    white-space: pre-wrap;
  }
  button {
    margin: 0.35rem 0 0.75rem;
    padding: 0.6rem 0.85rem;
    border: 1px solid var(--line);
    border-radius: 1px;
    background: var(--accent);
    color: #fff;
    font-weight: 700;
    cursor: pointer;
  }
  button:focus-visible {
    outline: 3px solid var(--accent-2);
    outline-offset: 2px;
  }
</style>
