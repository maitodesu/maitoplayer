# Windows installation

## Requirements

- 64-bit Windows 11
- Anki Desktop with AnkiConnect only if you want to publish cards

The self-contained installer includes FFmpeg, FFprobe, the full English JMdict
database, Noto Sans JP for subtitle rendering, and Microsoft's offline WebView2
runtime installer. Installation and first playback do not require a network
connection, font install, or dependency setup.

Normal release builds use only packaged dependencies. Diagnostic/development
builds can enable explicit overrides with
`MAITOPLAYER_ENABLE_DEPENDENCY_OVERRIDES=1`; only then do the FFmpeg, FFprobe, and
dictionary path overrides or `PATH` discovery apply. Those controls are not an
end-user installation step.

On first launch, import a video and confirm subtitles and definitions appear.
If a bundled dependency health row is unavailable, repair or reinstall the app;
do not download an arbitrary replacement executable. Configure Anki only after
the viewing experience works. Use a disposable Anki profile until the real-Anki
recovery gate passes. Media remains local and no telemetry is sent.

## Build an installer

```powershell
pnpm install --frozen-lockfile
./packaging/windows/build-self-contained.ps1
```

The unsigned NSIS artifact is produced under the Tauri target bundle directory.
A release is not certified until the exact installer passes the clean-VM runbook
and is signed by the release owner.
