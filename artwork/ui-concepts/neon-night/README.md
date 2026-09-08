# Neon Night

A static layout exploration for Maito Player. It uses a restrained Tokyo-at-night palette, cinematic player-first hierarchy, bilingual subtitles, a hover-style dictionary result, and a deliberately secondary word-card workflow.

Open `index.html` directly in a browser. It has no network, build, or runtime dependencies. Subtitle typography requests the locally installed `Noto Sans JP` first, with Japanese system-font fallbacks for machines where it is unavailable.

Key decisions:

- A compact left rail keeps navigation visible without competing with the video.
- English translation sits directly beneath the Japanese line as one synchronized subtitle stack.
- Furigana uses dense multi-layer shadows so it remains legible over bright footage.
- The dictionary is an anchored, opaque surface rather than a permanently docked panel.
- Card composition stays visible but optional in a narrower study column.
- Below-player dialogue gives sync context and a natural route for seeking nearby cues.
