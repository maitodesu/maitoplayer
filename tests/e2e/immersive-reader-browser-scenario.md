# Immersive reader browser E2E scenario

This scenario specifies the deterministic Playwright browser journey executed
against the production UI build. Its test-only Tauri bridge, local range server,
runner and screenshots are retained under
`output/playwright/final-ux-e2e`; the production bundle contains none of that
harness. The dated result report records the completed run.

## Harness contract

Launch the production UI build in Chromium with a test-only Tauri invoke bridge.
The bridge must return the checked-in synthetic media session, a Japanese cue
whose token `勉強` has reading `ベンキョウ`, an overlapping English translation,
and one JMdict summary. Serve a small local seekable video URL for the scoped
playback URL. Record every invoke name, arguments, start, and finish time. Do not
ship the bridge or fixture switch in the production bundle.

Launch Chromium with precise memory information and exposed GC when supported.
Collect CDP `Performance.getMetrics`, DOM counts, `HTMLVideoElement` counts, and
invoke counts. A separate packaged Tauri run remains required for native working
set and custom-protocol proof.

## Journey and assertions

1. Open the app at 1280 x 800, import the fixture, and wait for the first frame.
   Assert exactly one video element and that JP, EN, and furigana toggles are all
   pressed. Confirm rendered ruby base `勉強` and hiragana `べんきょう`; katakana
   must not appear in `rt`.
2. Select Shibui Cinema, Tokyo Editorial, and Neon Night through the `Visual
   theme` fieldset. After each choice, assert the root `data-theme`, native color
   scheme, visible focus, and no horizontal overflow. Reload once and assert the
   last theme persists.
3. Press `E`, `J`, and `R` independently outside editable/media controls. After
   each key, assert only the matching toggle and subtitle layer changes. Focus the
   seek slider and repeat the keys; playback-control focus must suppress these
   global shortcuts. Re-enable all three layers.
4. Hover the `勉強` token. Assert a single dictionary dialog appears, stays within
   the viewport, exposes the expected reading/definition, and remains open while
   the pointer moves from token to dialog. Click the token and assert the optional
   word-card composer opens with the same expression, hiragana reading, sentence,
   and definition. Close it and verify focus returns to a sensible study control.
5. Click `Toggle fullscreen`. In fullscreen, advance the mock media clock through
   a JP cue containing two overlapping EN cues. Assert JP stays visually primary,
   every currently active authored EN cue is visible, and expired cues disappear
   on their half-open end. Hover and keyboard-focus `勉強`; the dictionary dialog
   must remain visible, interactive, and inside the fullscreen viewport. Exit with
   Escape and assert there is still exactly one video element.
6. Start playback, capture `currentTime`, choose Settings, wait one second, then
   return through Watch. Assert the same video node/session/source URL remain
   mounted, playback is paused while hidden, position differs by no more than one
   frame, and no `close_media_session` or new import invoke occurred. Resume and
   assert time advances from the preserved position.
7. Warm the journey, force GC if available, and capture baseline. Then run a
   bounded diagnostic loop: seek across cue boundaries, toggle JP/EN/furigana,
   hover/focus a token, open/close dictionary and composer, visit Settings and
   return, and enter/exit fullscreen. Allow pending invokes to settle, force GC,
   and capture the final sample. The checked-in release scenario uses 25 cycles;
   the separate packaged two-hour soak remains the authoritative endurance gate.

## Pass criteria

- No page errors, unhandled rejections, failed invokes, duplicate media elements,
  duplicate dialogs, or horizontal overflow at 1280 x 800, 760 x 900, and 390 x
  844.
- At most one subtitle-window request per track is in flight; requests are bounded
  to rolling refreshes rather than frame rate. Progress polling must have at most
  one invoke in flight per operation.
- After settling/GC, no detached video, dialog, subtitle-token, or settings-panel
  nodes remain. Listener/timer and DOM counts return to their warmed baseline.
- Browser retained-heap growth is recorded with snapshots and investigated if it
  trends upward; it is diagnostic, not a replacement for the release gate.
- The packaged two-hour process-tree soak is the authoritative memory gate:
  aggregate growth after warm-up must remain under 25 MiB.
