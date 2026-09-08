# Desktop runtime performance and memory audit (2026-09-05)

This is a code-level audit and targeted benchmark, not a substitute for the
packaged two-hour release soak in `release-measurements.md`.

## Verified bounds and lifecycle behavior

- The media clock uses `requestVideoFrameCallback` when available and falls back
  to `requestAnimationFrame`. Frame callbacks are cancelled on pause and destroy.
- Playback checkpoints are bounded to approximately one invoke per second while
  playing, plus explicit state transitions.
- Japanese and English subtitle requests use separate latest-wins revisions and
  in-flight/pending collapse. Each backend window is 60 seconds and capped at 200
  cues, including pathological overlap.
- Hidden subtitle tracks stop requesting new windows. Turning a track back on
  immediately refreshes it only when its current window is missing or stale.
- The media clock remains frame-accurate, while cue selection is memoized until
  the next authored cue boundary. Rendering work therefore follows subtitle
  changes instead of the display frame rate.
- The tokenizer cache is bounded to 512 entries and approximately 4 MiB.
- The dictionary is an indexed, read-only SQLite database; the 88 MiB JMdict file
  is not deserialized into application memory. Query results are capped at 20.
- Media protocol responses are capped at 8 MiB. `read_range` now copies only the
  authorized path and size while holding the session registry lock and performs
  file open/read after releasing the lock.
- Media sessions and subtitle timelines are removed by `close_session`. Player
  frame callbacks, conversion polling, video source, and diagnostic download
  object URLs have explicit cleanup paths.
- Conversion and card-publication polling are single-flight, generation scoped,
  and session scoped. Component teardown clears timers and late results cannot
  mutate a replacement session.
- Translation windows have the same 200-cue overlap cap as Japanese windows and
  never enter the tokenizer/dictionary pipeline.

## Changes made by this audit

1. `dictionary::SqliteDictionary` now has an immutable-result cache keyed by
   surface, lemma, and reading. It caches misses, is FIFO bounded to 2,048 entries
   and approximately 32 MiB, and releases the SQLite connection before inserting.
2. `MiningAssetService`'s completed-request map is FIFO bounded to 1,024 entries.
   Invalidating a missing asset also removes its order entry, preventing metadata
   growth during repeated cache-file deletion.
3. `AppCore` now releases session/subtitle read locks before tokenization,
   dictionary lookup, and draft storage. Long analysis can no longer block media
   session close or subtitle-source mutation for its whole duration.
4. The media session registry lock no longer covers an up-to-8-MiB file read.
5. `dictionary/examples/lookup_probe.rs` provides a repeatable benchmark against
   any supplied JMdict SQLite file.
6. The Tauri `subtitle_window` command now dispatches tokenization and SQLite
   enrichment through `spawn_blocking`, so a cold analysis does not occupy an
   async command worker.
7. Subtitle cue selection now uses a boundary-aware memoization cache. The
   per-frame clock remains precise without repeatedly filtering unchanged cue
   arrays or rewriting equivalent DOM props.
8. Hidden JP/EN tracks suspend rolling-window work. Player and composer polling
   now enforce one request in flight, discard stale generations, and capture the
   initiating session for cancellation.
9. The player is keyed only by media-session identity: settings navigation keeps
   the same paused video node and position, while a genuinely new import gets a
   fresh capability and playback lifecycle.

## Measured dictionary result

Command (release mode, bundled 88 MiB dictionary, 10 representative tokens, 20
passes / 200 lookups):

```powershell
cargo run --release -p dictionary --example lookup_probe -- `
  apps/desktop/src-tauri/release-resources/data/jmdict.sqlite 20
```

| Measurement | Before cache | After cache |
|---|---:|---:|
| Lookup total | 88.862 ms | 7.079 ms |
| Mean lookup | 444.312 us | 35.393 us |
| Result count | 880 | 880 |

Repeated-window lookup time improved by about 12.6x in this deterministic loop.
The after-cache 100-pass run completed 1,000 lookups in 18.230 ms (18.230 us
mean). Dictionary open remained expensive and variable: 2.281-3.398 seconds in
these runs because startup validation includes SQLite integrity, foreign-key,
JSON-shape, and count scans. A warm-cache benchmark does not waive the plan's
50 ms p95 composed tokenization/dictionary gate.

## Validation

```text
cargo test --workspace --all-targets
108 passed; 0 failed; 5 ignored (explicit real-tool fixture tests)

cargo clippy --workspace --all-targets -- -D warnings
passed

cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets
12 passed; 0 failed

cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml \
  --all-targets -- -D warnings
passed

cargo fmt --all -- --check
passed

pnpm format:check && pnpm check && pnpm test && pnpm build
9 test files / 33 tests passed; 0 Svelte errors; 0 Svelte warnings;
137 production modules built
```

The production browser journey also completed 25 mixed interaction cycles after
forced-GC warm-up. It reported zero page/console errors, unchanged DOM node and
event-listener counts (1,264 nodes / 86 listeners), and 84,100 bytes of retained
JavaScript heap growth. This is a short browser diagnostic, not the packaged
two-hour memory gate; see `../e2e/immersive-reader-browser-result-2026-09-05.md`.

## Remaining release risks and exact follow-ups

1. Full JMdict startup validation performs database-wide work on every launch.
   The bundled dependency lock pins the database size, SHA-256, schema, and entry
   count, and imported dictionaries are fully validated before atomic promotion,
   but runtime does not yet bind an app-owned copy to a durable database-file
   digest. Preserve full validation until import records that digest and startup
   verifies it (plus schema/application IDs); only then move integrity/foreign-key/
   JSON scans to build, import, repair, and explicit health checks.
2. The bundled Noto Sans JP variable TTF is 9,590,732 bytes; FFmpeg is
   113,536,512 bytes, FFprobe 113,330,688 bytes, and JMdict 88,133,632 bytes.
   The verified self-contained NSIS artifact is 369,305,747 bytes. These dominate
   disk/package cost; none are imported into the 95,793-byte JavaScript bundle or
   retained wholesale in JavaScript heap.
3. A packaged soak is still required: after warm-up, aggregate desktop plus
   descendant-process memory growth must stay below 25 MiB over two hours.

## Release artifact produced

- Installer: `Maito Player_0.1.0_x64-setup.exe`
- Size: 369,305,747 bytes (352.20 MiB)
- SHA-256: `08c68880f2c557b0ed434c61a158ac7592634502bbcba16132d090a18dbef804`
- Authenticode: `NotSigned` (code signing remains a distribution gate)
- Pinned runtime dependency guard: passed after the final build
