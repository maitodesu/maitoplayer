# Player and folder-playlist browser result — 2026-09-05

The production Vite build was exercised in headed Chrome through Playwright CLI
with the range-capable generated H.264/AAC fixture and a deterministic mock of
the narrow Tauri command boundary.

Verified in one journey:

- Initial **Preview new** folder import mounted no video and exposed three staged
  discoveries for explicit selection.
- Selected discoveries were added, naturally displayed, reordered, and removed.
- Removing the currently playing item preserved the same video element and source.
- Automatic JP/EN matches displayed confidence and reason; an ambiguous JP match
  remained unresolved until an explicit override was selected.
- Selecting and clearing the active JP override updated the track without creating
  a second media element.
- The cinematic controls and cursor hid after 2.7 seconds of uninterrupted
  playback, then returned on pointer movement.
- With the Fullscreen button still focused, Space paused and resumed playback
  without exiting fullscreen.
- Manual folder refresh completed through the playlist rescan command.
- Browser page errors: 0. Browser console errors: 0.

Final result:

```json
{"playlistItemsRemaining":1,"videoCount":1,"autoHideVerified":true,"fullscreenShortcutPriorityVerified":true,"ambiguityAndOverrideVerified":true,"pageErrors":[],"consoleErrors":[],"invokes":{"choose_and_import_playlist_folder":1,"import_playlist_discoveries":1,"reorder_playlist_item":1,"remove_playlist_item":2,"select_playlist_item":2,"subtitle_window":2,"translation_window":2,"report_playback_capabilities":2,"report_decode_outcome":2,"send_playback_checkpoint":7,"close_media_session":1,"choose_playlist_subtitle_override":1,"clear_playlist_subtitle_override":1,"rescan_playlist":1}}
```

Artifacts:

- `output/playwright/player-playlist-e2e/player-playlist-desktop.png`
- `output/playwright/player-playlist-e2e/player-playlist-final.png`
- `output/playwright/player-playlist-e2e/run-journey.js`
