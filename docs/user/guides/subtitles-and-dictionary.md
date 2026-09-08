# Subtitles and local dictionary

The current desktop flow discovers adjacent UTF-8 SRT, ASS/SSA, and WebVTT files
after importing a video. It chooses a unique ranked match automatically and asks
you to choose when candidates tie; you can always use **Add JP subtitles** to
select another file. This Japanese study track is the only track that is
tokenized, looked up in JMdict, or used by the word-card composer. Subtitle text
is parsed locally, unsafe formatting is removed, and a bounded rolling window is
analyzed against media time. At a cue's exact end time the cue is no longer
active.

Use **Add English** to choose an independent, optional translation track. Its raw
text appears directly below Japanese in both normal and fullscreen playback and
can be changed or removed without changing the Japanese binding. Both tracks use
the same media clock and exact half-open cue intervals; the app does not guess at
text similarity or fuzz timings. That means a long Japanese cue can naturally
span several changing English cues, overlapping English cues appear in stable
track order, duplicate visible lines are collapsed, and authored gaps remain
empty. Visible translation output is capped for pathological overlaps and long
cues so it cannot cover the player controls. English cues never run Japanese NLP
or dictionary lookup.

Hover an underlined token, or focus it from the keyboard, to see its surface,
hiragana reading, part of speech, common-word marker, and every ranked dictionary
sense in the anchored lookup card. When an exact spelling-and-reading match is
available, the card also shows a mora-by-mora Tokyo pitch contour from UniDic.
The faint following **が** makes flat and final-drop patterns distinguishable.
This is dictionary-form lexical pitch, not a measurement of the actor's audio;
conjugation, compounds, focus, dialect, and performance can change the pitch you
hear. Multiple recorded patterns are preserved instead of silently choosing one.

The optional **JLPT N… estimate** badge comes from a community vocabulary
mapping. It is deliberately labeled as an estimate because the JLPT has not
published an official current N1-N5 vocabulary list since the 2010 revision.
Click the token only when you want to keep it in the word-card composer.
Punctuation remains visible but is not a lookup button. The app may still show
tokens when the dictionary is unavailable.

The Windows installer includes a full local English JMdict database, so lookup
works offline without an import or restart. It is derived from the pinned
JMdict-Simplified English release recorded in the installed third-party notice;
the source, licence, generated schema-3 database, and checksums are verified at
release staging. Pitch and JLPT metadata is stored in a separate, locally bundled
database derived from pinned UniDic and yomitan-jlpt-vocab sources. If that
optional metadata database is missing, definitions and tokenization keep working;
repair or reinstall to restore the learning metadata.

Embedded text streams appear as selectable tracks and are extracted into the
application cache before parsing. PGS and VobSub are images and cannot be used as
interactive text. If you choose **Remember file**, the active subtitle binding is
saved with that consent and restored only while its version still matches;
**Forget** removes the recent-file grant and both saved subtitle roles. The
Japanese and English bindings are stored separately, so changing or removing the
translation never overwrites the study track. Embedded text tracks can be chosen
for either role; embedded extraction, tie selection, and binding restore still
require packaged release evidence.
