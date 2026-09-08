# ADR 0001: App-owned playback and canonical source time

Status: accepted

## Decision

The Tauri webview owns playback through one `HTMLVideoElement`. Rust imports the
user-authorized file into an opaque media session and serves bounded byte ranges
through the `migaku-media` protocol. The UI never receives a reusable filesystem
path.

The backend chooses the cheapest verified path in this order: direct playback,
stream-copy remux, copied video with converted audio, explicitly approved video
transcode, then an actionable unsupported result. Every proxy uses a monotonic,
constant-offset map back to canonical source microseconds. Subtitles, drafts,
audio clips, and frames always use source time.

## Consequences

- Container names alone never prove compatibility; the webview reports codec
  capability and a first-frame result.
- A range response is capped at 8 MiB, preventing accidental whole-file IPC.
- Image subtitles remain visible as unsupported text sources; OCR is not in MVP.
- Direct/remux/audio conversion may run automatically. Video transcode requires
  confirmation.

