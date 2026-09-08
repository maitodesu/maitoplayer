# Release performance procedure

Use the exact packaged candidate and checked-in synthetic fixture generator. Run
direct, remux, audio conversion, and video conversion as separate sessions; do
not average unlike paths.

For the two-hour direct-play soak, start playback, find the desktop process ID,
and run:

```powershell
pwsh -File tests/performance/windows-process-sampler.ps1 `
  -ProcessId <pid> `
  -DurationSeconds 7200 `
  -IntervalSeconds 5 `
  -OutputCsv docs/spikes/media-compatibility/results/local/direct-soak.csv
```

The sampler records both the root desktop process and the aggregate descendant
tree, including WebView2 and any active tool children. Run it from a shell allowed
to query `Win32_Process`; inability to enumerate descendants invalidates the
memory result.

Record first-frame and seek latency from media events using a monotonic browser
clock. Report count, p50, p95, maximum, fixture hash, and whether the fixture was
already warm in the operating-system cache. Record dropped frames from
`getVideoPlaybackQuality`, and CPU/GPU with Windows Performance Recorder or Task
Manager. Record the largest observed range response and verify it is at most
8 MiB.

The gate fails if any required target is unmeasured. In particular, compilation
does not substitute for first-frame, seek, ten-minute dropped-frame, two-hour
memory-growth, subtitle-window, NLP/dictionary, or five-second card-creation
measurements.
