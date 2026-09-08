import { describe, expect, it } from 'vitest';
import { buildDiagnosticsPreview, redactHealth } from './redaction';

describe('diagnostics redaction', () => {
  it('never exposes dependency diagnostics', () => {
    const canary = 'remote-secret-canary';
    const redacted = redactHealth({
      checks: [
        {
          component: 'AnkiConnect',
          available: false,
          version: null,
          action: 'Retry',
          error: {
            code: 'ANKI_OFFLINE',
            message: 'Anki is unavailable.',
            retryable: true,
            diagnostics: canary,
          },
        },
      ],
    });
    expect(JSON.stringify(redacted)).not.toContain(canary);
    expect(redacted?.checks[0]?.error?.diagnostics).toBe('[redacted]');
  });

  it('builds the exported preview from redacted health only', () => {
    const canary = 'C:\\Users\\private-user\\secret-video.mkv';
    const preview = buildDiagnosticsPreview(
      {
        checks: [
          {
            component: 'FFmpeg',
            available: false,
            version: null,
            action: 'Repair or reinstall the application.',
            error: {
              code: 'FFMPEG_NOT_FOUND',
              message: 'FFmpeg is unavailable.',
              retryable: true,
              diagnostics: canary,
            },
          },
        ],
      },
      null,
      null,
    );
    const exported = JSON.stringify(preview);
    expect(exported).not.toContain(canary);
    expect(exported).toContain('paths omitted');
    expect(exported).toContain('[redacted]');
  });
});
