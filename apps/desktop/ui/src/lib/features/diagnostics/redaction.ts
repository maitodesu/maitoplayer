import type { AppHealthV1, MediaSessionV1, SubtitleSourceV1 } from '../../contracts/generated';

export interface DiagnosticsPreview {
  redaction: 'paths omitted';
  media: {
    display_name: string;
    source_fingerprint_prefix: string;
    container: string;
    duration_us: number;
    playback_plan: MediaSessionV1['playback_plan'];
    stream_counts: { video: number; audio: number; subtitle: number };
  } | null;
  subtitle: {
    display_name: string;
    format: SubtitleSourceV1['format'];
    source_version_prefix: string;
  } | null;
  health: AppHealthV1 | null;
}

export function redactHealth(health: AppHealthV1 | null): AppHealthV1 | null {
  if (!health) return null;
  return {
    checks: health.checks.map((check) => ({
      component: check.component,
      available: check.available,
      version: check.version,
      action: check.action,
      error: check.error
        ? {
            code: check.error.code,
            message: check.error.message,
            retryable: check.error.retryable,
            diagnostics: '[redacted]',
          }
        : null,
    })),
  };
}

export function buildDiagnosticsPreview(
  health: AppHealthV1 | null,
  session: MediaSessionV1 | null,
  subtitle: SubtitleSourceV1 | null,
): DiagnosticsPreview {
  return {
    redaction: 'paths omitted',
    media: session
      ? {
          display_name: session.display_name,
          source_fingerprint_prefix: session.source_fingerprint.slice(0, 12),
          container: session.container,
          duration_us: session.duration_us,
          playback_plan: session.playback_plan,
          stream_counts: {
            video: session.video_streams.length,
            audio: session.audio_streams.length,
            subtitle: session.subtitle_streams.length,
          },
        }
      : null,
    subtitle: subtitle
      ? {
          display_name: subtitle.display_name,
          format: subtitle.format,
          source_version_prefix: subtitle.source_version.slice(0, 28),
        }
      : null,
    health: redactHealth(health),
  };
}

export function exportDiagnostics(preview: DiagnosticsPreview): void {
  const contents = JSON.stringify(preview, null, 2);
  const url = URL.createObjectURL(new Blob([contents], { type: 'application/json' }));
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = 'migaku-diagnostics-redacted.json';
  anchor.hidden = true;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}
