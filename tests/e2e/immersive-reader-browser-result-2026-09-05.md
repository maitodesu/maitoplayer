# Immersive reader production-browser E2E result (2026-09-05)

## Result

PASS against the production Vite build in Chromium using a test-only Tauri
bridge and a byte-range-capable local server. The media fixture SHA-256 was
`771A0A64246D5BA5728A6874031D8FB0E92889B6D42F690149A05FC823ECFCFC`.

## Covered journey

- Imported one seekable media session and asserted exactly one video element.
- Switched among Shibui Cinema, Tokyo Editorial, and Neon Night; verified root
  theme state, persistence, and no horizontal overflow, including 390 x 844.
- Toggled Japanese (`J`), English (`E`), and furigana (`R`) independently and
  verified shortcuts are suppressed while a playback control is focused.
- Verified hiragana ruby (`べんきょう`) with no katakana in `<rt>`.
- Verified hover opens only the viewport-contained dictionary, while an explicit
  token click opens the populated word-card composer; closing restores focus to
  the source token.
- Navigated Watch -> Settings -> Watch and verified the same paused video node,
  source, session, and frame-accurate position were preserved without reimport.
- Exercised fullscreen with Japanese, two overlapping English cues, and an
  interactive dictionary overlay.
- Ran 25 mixed shortcut, hover, composer, seek, fullscreen, and settings cycles.

## Recorded diagnostics

```json
{
  "theme": "neon-night",
  "videoCount": 1,
  "shortcutCycles": 25,
  "pageErrors": [],
  "consoleErrors": [],
  "invokes": {
    "send_playback_checkpoint": 10,
    "settings": 12
  },
  "before": {
    "jsHeapUsedBytes": 3958996,
    "nodes": 1264,
    "documents": 2,
    "listeners": 86
  },
  "after": {
    "jsHeapUsedBytes": 4043096,
    "nodes": 1264,
    "documents": 2,
    "listeners": 86
  },
  "heapGrowthBytes": 84100
}
```

There were no page errors, console errors, duplicate media/dialog elements, or
viewport overflow. After settling and forced GC, node, document, and listener
counts were identical to baseline. The 84,100-byte retained-heap delta is a
short-run browser diagnostic only; it does not replace the packaged two-hour
process-tree requirement of less than 25 MiB aggregate growth after warm-up.

## Evidence

- `output/playwright/final-ux-e2e/theme-shibui.png`
- `output/playwright/final-ux-e2e/theme-editorial.png`
- `output/playwright/final-ux-e2e/theme-neon.png`
- `output/playwright/final-ux-e2e/theme-neon-narrow.png`
- `output/playwright/final-ux-e2e/fullscreen-neon.png`
- `output/playwright/final-ux-e2e/final-neon.png`
