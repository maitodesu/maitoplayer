# Anki and cards

Anki is optional and comes after the core viewing and lookup experience. If you
choose to publish cards, use a disposable Anki profile until the release
candidate passes exact-once and recovery tests. Start Anki Desktop, install and
enable AnkiConnect, and keep its default loopback endpoint available at
`127.0.0.1:8765`.

The default pre-release profile targets deck `Default` and a note type named
`Kiku`, with separate Expression, Reading, Sentence, Definition, Audio, Image,
Source, Timestamp, and MiningId fields. Create that note type manually or enter
an existing deck, note type, and one-to-one field mapping in Settings. The app
validates and persists the mapping but does not yet provide deck/model discovery;
it never alters an existing note type.

To prepare a draft, pause within the selected subtitle cue and select a lookup
token. The composer receives the same hiragana reading, complete numbered sense
list, labels, and sentence context shown in the hover card; those editable values
are also the values sent to Anki for consistency. The backend freezes the media
session, subtitle version, cue, token, dictionary entry, and canonical source
time. A stable `kiku_id_…` tag makes remote reconciliation possible.

The source candidate composes audio/image extraction, content-addressed media
upload, field mapping, and durable retry. After extraction, saved asset hashes let
restart recovery proceed without the media session. An interruption before
extraction needs an explicitly remembered, unchanged source file. These paths
still need process-crash and real-Anki release evidence. If the app reports
`PUBLISH_OUTCOME_UNCERTAIN`, do not create a separate draft: retry the same draft
so the app can reconcile the existing mining marker first.
