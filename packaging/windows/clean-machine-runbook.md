# Clean-machine release runbook

Start from a reverted Windows 11 x64 VM snapshot with no developer toolchain,
FFmpeg, WebView2, Anki, or application data. Copy in only the candidate installer,
published checksum, `verify-artifact.ps1`, this runbook and its evidence template,
synthetic fixtures, and test notes. Do not copy separate media tools or dictionary
data; dependency discovery outside the installed bundle is a failure. Use
`release-evidence-template.md` for the record.

1. Verify the installer checksum and Authenticode chain with
   `verify-artifact.ps1 -RequireTrustedSignature`.
2. Disconnect the VM from the network and install as a standard user. Confirm the
   bundled offline WebView2 installer handles an absent runtime; repeat with the
   supported runtime already present.
3. Launch without FFmpeg, FFprobe, or dictionary data anywhere outside the app.
   Media-tool and dictionary health must pass from installed resources. Record
   executable versions, dictionary provenance, and installed notices.
4. Temporarily remove each required installed resource in a disposable snapshot.
   FFmpeg, FFprobe, and JMdict must fail closed with a repair/reinstall action and
   must not execute a same-name program from `PATH`. Removing only the optional
   lexical-metadata database must hide pitch/JLPT information while leaving
   definitions usable. Restore the snapshot before functional testing.
5. Create a disposable Anki profile and install the tested AnkiConnect version.
   Never point fault injection at a personal collection.
6. Copy generated fixtures into a path containing spaces, Japanese characters,
   and a long nested component. Execute every case in
   `tests/e2e/release-scenarios.md`.
7. Run the two-hour direct-play soak and all path-specific measurements from
   `tests/performance/release-measurements.md`.
8. Inspect the installed CSP/effective capabilities. Preview diagnostics, export
   them, and scan the result for canary path, subtitle, field, and endpoint
   values. A missing export action is a failed release gate.
9. Install the previous supported version, create settings/history/drafts, then
   upgrade with the candidate. Confirm schema migration and documented retention.
10. Uninstall. Record exactly which app-data, dictionary, mining assets, and cache
    paths remain; compare the result with the published data-retention policy.
11. Revert the VM and repeat the happy path as a non-developer using only the user
    documentation. The operator must import a supported video and create one
    complete card without repository access or undocumented intervention.

Any blank measurement, unsigned artifact, unreviewed bundled binary, duplicate
note, unbounded child/cache growth, unredacted canary, or undocumented retained
data blocks release.
