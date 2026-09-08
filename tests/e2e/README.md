# Desktop E2E

The intended deterministic lane uses in-memory storage, fake media providers,
and the Anki emulator from `crates/test-support`. No executable desktop E2E runner
is wired into the repository yet; unit tests must not be reported as E2E evidence.

The opt-in Windows lane uses generated media, the packaged app's custom range
protocol, trusted local FFmpeg tools, and a disposable Anki profile. Run every
case in `release-scenarios.md` against the exact installer candidate.
