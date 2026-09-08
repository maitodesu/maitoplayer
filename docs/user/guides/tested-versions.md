# Tested versions and certification status

No public release is certified yet. Development checks through 2026-09-05 used:

- Windows kernel version 10.0.26200 x64;
- Microsoft Edge WebView2 152.0.4191.62;
- Rust/Cargo 1.96.0;
- Node.js 24.14.0 and pnpm 11.24.0;
- Tauri Rust crate 2.11.5 and Tauri CLI 2.11.4;
- the bundled BtbN LGPLv3 FFmpeg/FFprobe
  `n8.1.2-34-g9b6c8969e0-20260731` for all five backend media-tool tests;
- the full English JMdict-Simplified `3.6.2+20260831182826` source transformed
  into a deterministic 218,672-entry schema-3 SQLite database;
- Microsoft's validly signed 258,510,544-byte offline WebView2 installer inside
  the 364,090,381-byte self-contained NSIS candidate.

These are development observations, not a supported-version promise. No exact
Anki/AnkiConnect pair, clean Windows VM, signed installer, or packaged media
matrix has completed certification. The release evidence must replace this status
with the exact tested OS, WebView2, FFmpeg, FFprobe, Anki, AnkiConnect, dictionary,
and installer versions.
