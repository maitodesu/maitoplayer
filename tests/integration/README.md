# Integration verification

The normal workspace tests prove domain units, not the app-owned desktop flow.
Record each row independently and attach command output or a screen recording to
the release evidence. Never mark a row passed from compilation alone.

| Lane | Deterministic input | Required observation | Current automation |
|---|---|---|---|
| Contract composition | Checked-in JSON, SRT/ASS/VTT, JMdict sample | Rust/TypeScript parity, exact token spans, ranked lookup | Workspace unit tests plus `tests/contract/verify-contract-boundaries.ps1` |
| Scoped media session | Synthetic media | Unknown/closed IDs fail; valid ranges are capped; seek works | Rust unit coverage is partial; packaged protocol test required |
| Direct playback | H.264/AAC MP4 and VP9/Opus WebM | First frame, audio, seek, pause/resume, rate, callback, cleanup | Opt-in packaged Windows lane |
| Remux | H.264/AAC Matroska | Stream copy, no decode, canonical timeline, cache hit | Opt-in real-tool lane |
| Audio conversion | H.264/FLAC Matroska | Video copied, audio AAC, canonical timeline, cache hit | Opt-in real-tool lane |
| Video conversion | VP9/Opus or unsupported fixture | Explicit approval, progress, cancellation, partial cleanup | Opt-in packaged Windows lane |
| Embedded subtitle | Synthetic Matroska with UTF-8 SRT track | Extract, parse, select, seek storm | Opt-in real-tool lane |
| Mining | Known cue/audio marker/frame color | Canonical audio/image, hash names, immutable draft | Domain real-tool and publisher units pass; Tauri coordinator E2E required |
| Anki uncertainty | Emulator disconnect after commit | Retry/reconcile returns the same note ID | Workspace fault test proves one `addNote`; process-restart/real-Anki lane required |
| Restart recovery | Kill after each durable job stage | Resume without lost confirmation or duplicate | Fault-injection lane required |

Baseline commands:

```powershell
cargo test --workspace
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
pnpm check
pnpm test
pnpm build
pwsh -File tests/contract/verify-contract-boundaries.ps1
pwsh -File tests/security/verify-desktop-boundary.ps1
```

Generate only synthetic media with:

```powershell
./tools/media-fixtures/generate.ps1 -Ffmpeg C:/trusted/ffmpeg.exe -Ffprobe C:/trusted/ffprobe.exe
```

Then run the opt-in backend lane:

```powershell
pwsh -File tests/integration/run-real-media-tools.ps1 `
  -Ffmpeg C:/trusted/ffmpeg.exe `
  -Ffprobe C:/trusted/ffprobe.exe
```

The FFmpeg and FFprobe executables must come from the same trusted distribution.
Generated media and local evidence stay ignored by Git.
