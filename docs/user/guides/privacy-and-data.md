# Privacy, cache, and backups

All product processing is local. The app has no analytics, remote dictionary
lookup, or remote media upload. When you confirm a card, its content-addressed
audio and image assets are sent only to AnkiConnect on the local loopback
endpoint. The webview receives opaque session IDs, redacted source metadata, and
scoped media URLs—not reusable file paths.

Media authorization is session-scoped by default and ends when the media session
closes. **Remember file** explicitly stores a recent-file grant plus separate
Japanese-study and optional English-translation bindings; **Forget** removes the
grant and both saved bindings. These controls exist in the desktop
flow, but their packaged revocation behavior is not release-certified yet.
Diagnostics provide a redacted on-screen preview. Diagnostic-file export is not
implemented or release-certified in this build.

Back up the user SQLite database from the application data directory while the
app is closed. The replaceable dictionary database and playback cache need not be
backed up. The current installer data-retention behavior is not yet certified.
Before release, upgrade/uninstall testing must record whether the user database,
dictionary, extracted mining assets, and playback cache are retained or removed;
the installer must not silently delete user data.
