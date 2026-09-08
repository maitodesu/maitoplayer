# Windows WebView2 support matrix

Status: implementation baseline; environment-specific observations must be added
by the release runbook before certification.

| Source | Expected first attempt | Deterministic fallback | Approval |
|---|---|---|---|
| MP4, H.264 8-bit, AAC | Direct | Audio conversion, then video conversion | Video only |
| WebM, VP9, Opus | Direct when capability confirms | Video conversion | Video only |
| Matroska, H.264 8-bit, AAC | Direct only if capability confirms | Stream-copy MP4 remux | No |
| Matroska, H.264 8-bit, FLAC | Direct only if capability confirms | Copy video, AAC audio | No |
| HEVC 8/10-bit | Direct only if capability and first-frame confirm | H.264/AAC conversion | Yes |
| H.264 High 10 | Direct only if first-frame confirms | H.264 8-bit/AAC conversion | Yes |
| AV1 | Direct only if capability and first-frame confirm | H.264/AAC conversion | Yes |
| Text subtitles | External parse or embedded extraction | Manual source selection on tie | No |
| PGS/VobSub | Video may play; not an interactive text source | Select external/embedded text | N/A |

No extension-based assumption is sufficient. A failed direct decode is promoted
once to the next plan; retries cannot cycle back to an earlier plan.

