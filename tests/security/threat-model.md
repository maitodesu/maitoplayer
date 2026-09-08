# Security threat model

Status reflects the 2026-09-04 source candidate. “Static pass” means the
deterministic script found the declared control; it is not packaged runtime proof.

| Threat | Intended control | Evidence/status | Owner/action |
|---|---|---|---|
| Arbitrary webview file read | Opaque session registry; no path-taking command | Static pass; closed/unknown-ID packaged test pending | A1/Q4 |
| Whole-file application copy | 8 MiB protocol response cap | Static pass; working-set and browser-allocation proof pending | A1/A5 |
| Process argument injection | Direct executable and argument array; no shell | Static pass; malicious filename lane pending | A3/Q4 |
| Probe/output bomb | Time and stdout/stderr caps | Unit shape coverage; hang/output-bomb process tests pending | A2/A3/Q4 |
| Process escape after cancel | Kill entire Windows child tree and contain it in a kill-on-close Job Object | Static pass for direct kill, `taskkill /T /F`, and Job Object assignment; cancel/close/app-crash packaged proof is missing | A3/Q3 |
| Cache poisoning | Validate artifact identity/content before atomic promotion | Atomic rename plus FFprobe container/video/duration validation and WebView-rejection deletion exist; crafted-cache test is missing | A3/Q4 |
| Unbounded cache retention | LRU size/age budgets with reference protection | Source unit coverage exists for budget, age markers, active playback references, partials, and orphan markers; packaged churn/10,000-entry boundary proof is missing | A3/F3/Q3 |
| Subtitle HTML/script | Plain-text normalization; no `{@html}` | Static pass and parser units; malicious corpus pending | B/Q4 |
| Image subtitle confusion | Explicit text/image classification and actionable rejection | Classification unit passes; desktop selection path pending | A2/B3/G4 |
| Stale seek/window race | Increasing revisions and latest-wins UI | Unit/static code exists; seek-storm desktop test pending | B3/G1/Q2 |
| Duplicate note after timeout | Mining ID, remote marker, durable reconciliation | Domain post-commit disconnect and eight-thread same-marker tests prove one add; composed process-restart/real-Anki proof is missing | E3/F3/Q2 |
| Crafted Anki response | Exact envelope, response-size cap, timeout | Transport units cover success/API error/extra key; delay/truncation/chunking corpus pending | E1/E4/Q4 |
| Overbroad Tauri ACL | Core-only window capability; strict CSP | `verify-desktop-boundary.ps1` passes; packaged effective ACL inspection pending | Master/Q4 |
| Persisted-scope abuse | Explicit recent-file consent and revocation | Desktop Remember/reopen/Forget composition exists and subtitle bindings are stored only for approved persistent sessions; packaged revocation proof is missing | F2/Master/Q2 |
| Anki markup injection | Escape user text; trust only validated content-addressed audio/image markup | Static gate and forged-markup regression reject replacement of protected media fields; real-Anki proof is required | E2/G3/Q4 |
| Sensitive diagnostics | Omit paths, secrets, and provider-supplied diagnostic detail from the reviewed preview | Pure redaction canary and static gate pass; no export exists and packaged export-canary proof is missing | G5/Q4 |
| Dictionary supply-chain substitution | Immutable English JMdict-Simplified release URL; source, licence, and deterministic schema-3 database SHA-256; exact staged-file allowlist | 218,672-entry source and two byte-identical database builds verified; CC BY-SA notice/licence staged; clean-installer proof pending | C2/R1 |
| Learning-metadata substitution or false authority | Pinned official UniDic source and BSD attribution; pinned CC BY-SA JLPT estimate; exact spelling/reading joins; UI and notice say estimate/non-official | 893,228 pitch rows and 8,113 JLPT rows built byte-identically; exact staged-file allowlist passes; clean-installer proof pending | C2/R1/Q4 |
| Bundled executable substitution | Immutable BtbN LGPL release URL; archive/tool/licence SHA-256; version/configuration/encoder audit; exact staged-file allowlist | Pinned FFmpeg/FFprobe staging and real-tool tests pass; release legal review, signing, and clean-installer proof pending | A3/R1/Q4 |

Release-blocking security evidence still includes packaged ACL/CSP inspection,
malicious metadata/subtitle/Anki corpora, Unicode and long-path argument tests,
post-commit disconnect recovery, cache corruption, cancellation/exit child-tree cleanup,
packaged same-marker publication, and diagnostics export canary scanning.
