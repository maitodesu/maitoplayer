# Verification layout

- `contract/`: serialization parity, stable errors, stale revisions, payload caps
- `integration/`: real domain composition and opt-in external tools
- `e2e/`: packaged desktop flows and disposable Anki lane
- `performance/`: playback/conversion/cache/lookup measurement harnesses
- `security/`: capability/CSP/path/process/subtitle/Anki abuse cases

Quick deterministic audit:

```powershell
pwsh -File tests/run-deterministic.ps1
```

The runner checks both the root Rust workspace and the separately rooted Tauri
crate, then frontend, contract, security, and repository guards. It stops at the
first failure.

Normal CI does not require user media or a running Anki. It is not release
certification: real-tool, packaged-WebView2, disposable-Anki, performance-soak,
installer, upgrade, and uninstall lanes are explicit gates. See
`tests/e2e/release-scenarios.md` and
`packaging/windows/clean-machine-runbook.md`.
