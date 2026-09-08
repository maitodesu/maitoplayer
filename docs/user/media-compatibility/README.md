# Media compatibility

Compatibility depends on the container, selected video/audio codecs, Windows
codec support, GPU driver, and WebView2. The app probes the source and verifies
the webview rather than trusting the filename extension.

The fallback order preserves quality and time:

1. Play the selected file directly.
2. Copy streams into a compatible container.
3. Copy video and convert only audio.
4. Convert video only after explicit approval.

Stream copy is fast and lossless. Audio conversion changes only the audio track.
Video conversion is slower, consumes more power and disk space, and can change
quality; it is never automatic by default. Completed proxies are cached by source
fingerprint, streams, profile, and tool version.

SRT, ASS/SSA, WebVTT, and embedded text subtitles can become interactive. PGS and
VobSub are image tracks and require OCR, which is outside the MVP.

