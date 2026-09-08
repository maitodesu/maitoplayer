# Windows packaging

Tauri owns the NSIS bundle configuration in
`apps/desktop/src-tauri/tauri.conf.json`. The release process must:

1. Build and test the exact commit from a clean checkout.
2. Stage the exact locked runtime dependencies and generate the NSIS installer
   with the offline WebView2 installer.
3. Install, upgrade, and uninstall on a clean Windows 11 VM.
4. Verify app-data retention, scoped media access, and cache cleanup.
5. Sign the executable and installer with the release certificate.
6. Publish SHA-256 checksums and record signing status.

Build from an installed frozen dependency graph:

```powershell
pnpm install --frozen-lockfile
./packaging/windows/build-self-contained.ps1
```

`stage-runtime-dependencies.ps1` downloads only immutable HTTPS release assets,
verifies their pinned sizes and SHA-256 values, audits the FFmpeg identity,
configuration, and required encoders, deterministically builds the full English
JMdict SQLite database and the UniDic/JLPT lexical-metadata database, verifies all
source licences plus the pinned Noto Sans JP font, and creates an ignored Tauri
resource overlay. Use `-Offline` after the verified artifacts are cached. The
staged media tools and databases are never committed; the font is a reviewed
application asset so subtitles render consistently without an operating-system
font install.

Then run `verify-artifact.ps1` against the installer. Use
`-RequireTrustedSignature` for a release build. Fill in
`release-evidence-template.md`; blank rows are failed gates.

The selected media tools are the BtbN win64 LGPL 8.1 build of FFmpeg
`n8.1.2-34-g9b6c8969e0-20260731`. The lock records the archive, FFmpeg source
commit, build-scripts commit, executable hashes, LGPLv3 licence hash, and required
non-GPL configuration. FFmpeg and FFprobe add 226,867,200 uncompressed bytes.
The JMdict database adds 88,133,632 bytes, the lexical-metadata database adds
95,924,224 bytes, and Noto Sans JP 2.004 adds 9,590,732 bytes. The latter database
contains 893,228 UniDic pitch patterns and 8,113 explicitly unofficial JLPT
estimates. The font is pinned to upstream source commit
`523d033d6cb47f4a80c58a35753646f5c3608a78` and ships with the SIL Open Font
License 1.1 and an attribution notice. The Microsoft-signed offline WebView2
payload used by the current build is 258,510,544 bytes (246.53 MiB), SHA-256
`1F4638309F3D82C31A3028C3CF7D75998F58E4D1407380F5CB8A8E9172CAF17D`.
It is an Evergreen Microsoft payload, so record its signature, exact size, and
hash for every release. The complete player-and-playlist, dual-subtitle,
Noto-bundled unsigned NSIS candidate is 388,989,979 bytes (370.97 MiB), SHA-256
`1FD9563771B66A9B6EB1F8B5A9FF742CD2AE797F99ED10B74AC15709931DCE46`;
compression saves substantially versus the installed files.

JMdict update procedure:

1. Select a new immutable English `jmdict-simplified` release and review its
   licence/source commit.
2. Update archive, licence, and extracted-JSON sizes/hashes in
   `runtime-dependencies.lock.json`.
3. Build twice with `dictionary-builder`; require byte-identical schema-3 output,
   more than 200,000 entries, and successful lookups for the subtitle smoke set.
4. Update the database size/hash/entry count and `JMDICT-NOTICE.txt`, then run
   `stage-runtime-dependencies.ps1` and the supply-chain guard.
5. Ship updates on a regular release cadence so the bundled dictionary tracks
   JMdict corrections. The release owner records the selected source date and
   why an available update was accepted or deferred.

Lexical-metadata update procedure:

1. Select an official New-BSD UniDic spoken lexicon export and a tagged
   `yomitan-jlpt-vocab` release. Confirm that the JLPT UI still says **estimate**.
2. Pin source, licence, size, and SHA-256 values in the runtime lock.
3. Build the metadata database twice and require byte-identical output, at least
   500,000 pitch rows, at least 5,000 JLPT rows, and exact spelling-plus-reading
   sample matches.
4. Update the database hash/counts and notice, stage offline, and run the exact
   resource-allowlist supply-chain guard.

The derived JMdict SQLite data remains CC BY-SA 4.0 and is installed with the
complete licence and modification/attribution notice. Public distribution is
still blocked until the release owner completes legal review, preserves access
to the exact corresponding FFmpeg source/build scripts, and signs the installer.
