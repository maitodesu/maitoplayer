# Maito Player

A fast, local-first Windows video player for studying Japanese from your own media.

Play a video, follow interactive Japanese subtitles, reveal furigana, inspect offline
dictionary definitions, compare optional English subtitles, and capture the moment for
Anki—all without uploading your media.

## Highlights

- Native desktop playback with automatic compatibility conversion when required
- Interactive SRT, ASS/SSA, WebVTT, and embedded text subtitles
- Independent Japanese, furigana, and English subtitle toggles
- Full offline English JMdict dictionary with hiragana readings
- Collapsible Tokyo pitch-accent diagrams and estimated JLPT vocabulary tags
- Folder playlists with automatic subtitle matching and per-video overrides
- Fullscreen dictionary lookup and controls that stay out of the way during playback
- Optional screenshot, audio-clip, and AnkiConnect workflow
- Multiple visual themes and bundled Noto Sans JP typography
- Local processing with no analytics or remote media upload

## Install

Download the latest Windows installer from the repository's **Releases** page and run it.

The self-contained installer includes FFmpeg, FFprobe, the English JMdict database,
pitch-accent and JLPT metadata, Noto Sans JP, and the offline WebView2 runtime. End users
do not need to install or configure those dependencies. Anki Desktop with AnkiConnect is
optional and only required when publishing cards.

> The current pre-release installer is unsigned, so Windows may show a SmartScreen
> warning. Verify the published SHA-256 checksum before installing.

## Quick start

1. Choose **Import video**, or use **Import folder** to create a playlist.
2. Select Japanese subtitles if no confident adjacent or embedded match is found.
3. Optionally add a separate English subtitle track.
4. Hover or focus a Japanese word for definitions; click it to open the card composer.
5. Expand **Pitch accent** only when you want the full contour view.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| Space or K | Play or pause |
| Left / Right | Seek backward or forward five seconds |
| M | Mute or restore volume |
| F | Enter or leave fullscreen |
| J | Toggle Japanese subtitles |
| R | Toggle furigana |
| E | Toggle English subtitles |

## Development

Requirements:

- 64-bit Windows 11
- Rust 1.96.0
- Node.js 24.14 or newer
- pnpm 11.24 or newer

Install dependencies and run the development UI:

```powershell
pnpm install --frozen-lockfile
pnpm dev
```

Run the standard checks:

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm format:check
pnpm check
pnpm test
pnpm build
```

Build the self-contained Windows installer:

```powershell
./packaging/windows/build-self-contained.ps1
```

The packaging script downloads pinned runtime sources, verifies their sizes and hashes,
builds the local language databases, and produces an NSIS installer under
`apps/desktop/src-tauri/target/release/bundle/nsis/`. Use `-Offline` after the verified
artifacts have been cached.

## Project layout

```text
apps/desktop/ui/          Svelte interface
apps/desktop/src-tauri/   Windows desktop shell and command boundary
crates/                   Rust media, subtitle, dictionary, storage, and Anki modules
docs/user/                User guides and troubleshooting
fixtures/                 Synthetic test fixtures
packaging/windows/        Reproducible Windows packaging scripts
tests/                    Contract, integration, and security checks
tools/                    Build, fixture, and data-generation utilities
```

## Privacy and third-party data

Media playback, subtitle parsing, tokenization, and dictionary lookup run locally. Card
assets are sent only to AnkiConnect on the local loopback address after confirmation.

Bundled runtime and language resources are pinned and accompanied by their applicable
licences and attribution notices. See [Windows packaging](packaging/windows/README.md)
for exact versions, checksums, and update procedures.

More help is available in [the user guides](docs/user/).
