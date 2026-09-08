# Folder playlists

Choose the folder import mode in the top bar before selecting **Import folder**:

- **Preview new** scans the selected folder and waits for you to check the videos
  you want before adding them.
- **Auto-add** adds every supported video immediately and continues adding new
  discoveries on later scans.

The scan is local, non-recursive, and limited to the folder you selected. While
the Watch screen is visible, the app performs a single-flight rescan every 15
seconds. Scanning pauses when the app is hidden. **Refresh** is always available
for an immediate scan.

Use the arrow controls to change playback order. **Remove** takes an item out of
the playlist but never deletes the video from disk. If you remove the item that
is currently playing, playback continues until you choose something else.

## Subtitle matching

Each video has independent JP and EN rows. The matcher removes common release,
resolution, codec, bracket, and separator noise, then requires compatible title
and episode identity. Language tags raise confidence. A different episode is
never accepted as a match.

- **Auto** shows the selected file, confidence, and reason.
- **Ambiguous** means equally strong candidates were found; choose a file.
- **Missing** means no candidate crossed the confidence threshold.
- **Override** is a file you selected manually for that video.

**Set JP/EN** or **Change JP/EN** can point either role at another SRT, ASS/SSA,
or WebVTT file. Changing the currently playing item's subtitle applies in the
same media session without restarting the video. **Clear** removes only the
manual override and returns to the current automatic or unresolved state.
