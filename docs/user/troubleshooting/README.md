# Troubleshooting

- **FFmpeg/FFprobe missing:** repair or reinstall the application. Both exact
  reviewed executables are included; do not download an arbitrary replacement.
- **Probe failed:** confirm the file still exists and FFprobe can read it. Corrupt
  or metadata-bomb inputs are rejected with bounded output.
- **Video will not decode:** review the proposed remux/conversion plan. Video
  conversion requires approval.
- **Subtitles unavailable:** choose UTF-8 SRT, ASS/SSA, or WebVTT. Image subtitle
  tracks are not text sources.
- **No definitions:** repair or reinstall the application, then recheck
  dictionary health. The full checksummed JMdict database is included.
- **Anki offline:** start Anki Desktop, install/enable AnkiConnect, and allow local
  access on `127.0.0.1:8765`.
- **Field mismatch:** select an existing deck/note type and remap the exact fields.
- **Outcome uncertain:** reconcile the `migaku_id_…` tag before retrying; this
  avoids duplicate notes after a post-commit disconnect.
- **Interrupted card needs its source:** reopen the unchanged source, choose
  **Remember file**, and retry recovery. Jobs that already reached assets-ready
  use their saved content hashes instead.
- **User database damaged:** preserve the original and restore a backup. The app
  does not silently replace a corrupt database.
- **Conversion stopped:** confirm free space and repair the app if FFmpeg health
  is unavailable.
  A cancelled conversion must not leave an exposed `.part` file. If it does,
  preserve diagnostics and report a defect rather than opening the artifact.
- **Installer fails on an offline PC:** preserve the installer checksum and error
  details and report a packaging defect. The package includes Microsoft's
  offline WebView2 runtime installer and must not need network access.
