# Windows release evidence

- Release commit:
- Application version:
- Installer filename / SHA-256:
- Authenticode status / signer / timestamp:
- Operator / date:
- Clean VM snapshot:
- Windows edition / build / architecture:
- WebView2 version:
- FFmpeg / FFprobe version and distribution:
- Anki / AnkiConnect version and disposable profile:
- Dictionary source version / SHA-256 / schema version:
- Pitch source / JLPT-estimate source / metadata database SHA-256:
- Noto Sans JP version / source commit / SHA-256:

## Packaging results

| Check | Pass/fail | Evidence |
|---|---|---|
| Standard-user install |  |  |
| WebView2 prerequisite |  |  |
| Offline install with WebView2 absent and network disconnected |  |  |
| Exact bundled FFmpeg/FFprobe hashes and licence notice |  |  |
| Full bundled JMdict hash, entry count, attribution, and sample lookups |  |  |
| UniDic pitch/JLPT-estimate hashes, row counts, licences, labels, and sample lookups |  |  |
| Bundled Noto Sans JP hash, licence, attribution, and subtitle rendering |  |  |
| Missing bundled tool fails closed without `PATH` fallback |  |  |
| First-launch health |  |  |
| Upgrade from prior release |  |  |
| Settings/history retained |  |  |
| Cache retention/cleanup matches policy |  |  |
| Uninstall behavior matches policy |  |  |
| Executable and installer signatures |  |  |
| Published checksum independently verified |  |  |

## Functional and nonfunctional results

Attach completed copies of `tests/e2e/release-scenarios.md`,
`docs/spikes/media-compatibility/measurement-template.md`, security scan output,
and all accepted waivers. A blank row is a failed release gate, not “not
applicable,” unless the release owner records a scoped waiver and rationale.
