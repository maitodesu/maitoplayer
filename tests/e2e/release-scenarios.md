# Desktop release scenarios

Run these against the exact packaged release candidate. Capture the installer
hash, application version, Windows/WebView2 versions, fixture hashes, and result
for every scenario.

## Local player

1. Import the direct-play MP4 from a directory containing spaces and Japanese
   characters. Confirm the UI never displays the full path.
2. Observe first frame and audio; seek forward/backward; pause/resume; select
   0.5x, 1x, and 2x; change volume; use Space, arrows, and F.
3. Switch every audio track and confirm the selected language is audible after
   any required remux/conversion.
4. Close the session. A request using its prior scoped URL must fail, and process
   handle/child counts must return to baseline.
5. Capture protocol requests while seeking. Confirm bounded normal and suffix
   ranges return `206` with correct `Content-Range`; malformed, multiple,
   reversed, unsatisfiable, and oversized whole-file requests return `416`
   without reading the complete source into application memory.

## Compatibility fallback

1. Import H.264/AAC Matroska. Confirm the plan is remux, FFmpeg stream-copies both
   selected streams, and the completed proxy plays.
2. Import H.264/FLAC Matroska. Confirm video is copied and only audio is converted.
3. Import the codec-incompatible fixture. Decline video conversion once, then
   approve it. Confirm visible progress and cancel halfway through; no partial is
   exposed or retained. Retry and confirm the completed cache artifact is reused.
4. Trigger a direct decode failure. Confirm the backend records it and advances
   exactly once without cycling through prior plans.
5. Start conversion and mining extraction concurrently. Confirm one is visibly
   queued, the active operation has numeric progress, queued cancellation is
   prompt, and no more than one heavy FFmpeg operation runs at once.

## Subtitles and lookup

1. Exercise external SRT, ASS/SSA, and WebVTT plus one embedded UTF-8 track.
2. At exact cue ends, confirm half-open timing. Seek rapidly in both directions,
   change speed, pause, and resume; an older window must never replace a newer one.
3. Present two equally ranked adjacent files. Confirm explicit selection is
   required and the approved binding survives restart.
4. Navigate tokens and dictionary results with the keyboard. Concatenated token
   surfaces must exactly reproduce each cue.

## Mining and recovery

1. Pause inside a known cue and publish one card. Verify sentence, expression,
   reading, definition, JPEG, MP3, source label, timestamp, and mining marker.
2. Repeat while playback uses a timestamp-normalized proxy. Audio and image must
   still come from canonical source time.
3. Inject a disconnect immediately after `addNote` commits. Retry only through
   reconciliation and confirm exactly one note and the same note ID.
4. Terminate after validation, extraction, media upload, note creation, and local
   confirmation in separate runs. Restart and verify crash-safe continuation.
5. Repeat a publish from eight concurrent clients. Confirm exactly one `addNote`,
   one note ID, identical media names, and deterministic results for all callers.
6. Terminate immediately after the durable request is stored but before a
   publish-attempt row exists. Restart with Anki offline, then open Health. The
   recoverable job and recovery error must remain visible; a `0 pending` result
   is a release-blocking defect.

## Failure and privacy states

Verify an actionable state for missing tools, corrupt/unsupported media, image
subtitles, subtitle tie, absent/replaced dictionary, offline Anki, field mismatch,
cache full, low disk, moved source, and damaged user database. Export diagnostics
only after preview; search them for the media path, subtitle path, endpoint secrets,
and user-authored fields. None may appear without explicit consent.
