# First run and card workflow

1. Import one local video, or choose **Import folder** for playlist mode. Before
   choosing a folder, select **Preview new** to approve videos individually or
   **Auto-add** to add every supported video in that folder. FFmpeg, FFprobe,
   Japanese language data, the English JMdict dictionary, Noto Sans JP subtitle
   font, and WebView2 are included by the Windows installer.
2. The app authorizes only that selected file for the
   current session. Choose **Remember file** only if you want it listed for
   explicit reopen and available to recover an interrupted pre-extraction job.
3. Choose a supported Japanese subtitle file or select an embedded text track.
   Optionally add a separate English translation track; it is display-only and
   does not replace the Japanese study source.
4. Play or seek. Both subtitle windows are selected locally from the same media
   clock, with English displayed immediately below Japanese.
5. Select an underlined Japanese token to view its lemma, reading, and ranked
   local definitions.
6. Pause on the desired frame and review a draft. Configure Anki/AnkiConnect only
   if you want to publish the note.
7. If publishing becomes uncertain, do not blindly retry. Let the app reconcile
   the stable mining marker with Anki first.

Keyboard controls include Space/K (play/pause), Left/Right (seek five seconds),
M (mute), F (fullscreen), J (Japanese), R (furigana), and E (English). All
subtitle tokens and card controls are reachable by keyboard. See
`keyboard-shortcuts.md`, `folder-playlists.md`, `subtitles-and-dictionary.md`,
and `anki-and-cards.md` for current pre-release limitations and safe test setup.
