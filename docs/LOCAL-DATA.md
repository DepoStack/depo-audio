# Where DepoAudio keeps data, and how to back it up or delete it

DepoAudio runs with no account and keeps everything on the computer it runs
on. It does not upload recordings, transcripts or the case library. This page
lists what DepoAudio itself stores, where it stores it, and how to export,
import or delete it. **Settings → Your data** shows the same information in
the app, with the exact folder path on that computer.

## What DepoAudio stores

| Data | Where | Holds |
| --- | --- | --- |
| Case library | app data folder, `library.json` | Case names, sessions, participant labels, the paths and sizes of converted files, and the source file name and path |
| Settings | app data folder, `prefs.json` | Audio processing settings, startup defaults and theme |
| Unfinished saves | app data folder, `library.tmp.*` / `prefs.tmp.*` | A temporary copy left only if a save was interrupted |
| Legacy model files | app data folder, `models/` | Files left by an older build. v1.0.3 never loads them |
| Transcripts | app web view storage, key `transcript:<track path>` | The transcript lines you edited for a track (speaker, text, time) |
| Bookmarks | app web view storage, `player-bookmarks` | Bookmark times, labels and the track path |
| Playback speed and theme | app web view storage, `player-speed`, `depoaudio-theme` | One setting each |

The app data folder is `<system app data>/com.andrewmayes.depoaudio`:

- macOS: `~/Library/Application Support/com.andrewmayes.depoaudio`
- Windows: `%APPDATA%\com.andrewmayes.depoaudio`

The web view storage sits in the system web view's data folder for DepoAudio.
On Windows that is expected under `%LOCALAPPDATA%\com.andrewmayes.depoaudio\EBWebView`;
on macOS it is expected under `~/Library/WebKit/com.andrewmayes.depoaudio`.
Neither location has been checked on an installed build yet. The installed-app
pass in `V1.0.3-RELEASE-CANDIDATE.md` must confirm both before the website
states them.

**Not stored by DepoAudio:** your recordings, converted files, the
`<output>.depoaudio.json` export records and backup zips. They stay wherever you
saved them. DepoAudio never moves or deletes them, including when you delete a
Library entry or clear all data.

An empty transcript keeps no storage key. Before these data controls,
opening a track in the Player left an empty `transcript:<path>` key; such keys
are removed the next time that track is opened, or by "Clear all DepoAudio
data".

## Export and import

**Export library and transcripts** (Settings → Your data) writes a new `.zip`.
It never replaces an existing file. The zip holds:

- `depoaudio-backup.json`: format marker (`depoaudio-backup`, version 1), app
  version, export time and counts
- `library.json`: the case library
- `transcripts.json`: non-empty transcripts keyed by track path
- `bookmarks.json`: bookmarks
- `README.txt`: what the file is

It holds no audio and no settings. **It is not encrypted**: keep it where you
keep the recordings.

**Import a backup** reads only an unmodified DepoAudio backup. It refuses a zip
that has been unpacked and re-zipped, has compressed or unknown entries, or
fails its checksums. Import only adds:

- Cases are matched by name; sessions are matched by ID. A session already in
  the Library is left as it is.
- A transcript is restored only when this computer has none for that track. A
  different local transcript is kept, and the result says so.
- Bookmarks are added unless the same one is already there.
- Every case name and Library file entry is checked before anything is added.
  A Library file entry must be a supported audio file.

Transcripts and Library entries follow the file paths they were saved with. On
a new computer, they line up with the audio when the files are at the same
paths. Library entries whose files are missing are kept and counted.

## Deleting data

- **Delete transcript for this track** (the bin button in the Player's
  transcript card): removes the saved transcript for that track only, after a
  confirmation. Exported SRT or TXT files are not affected.
- **Library → delete a case or session**: removes the Library entry only. The
  audio files are not deleted.
- **Settings → Model files**: removes individual legacy model files.
- **Settings → Your data → Clear all DepoAudio data**: after a confirmation,
  removes everything in the table above. DepoAudio deletes only those exact
  names from the app data folder and only its own keys from web view storage.
  It never follows a path recorded in the Library. The app then reloads as on
  first launch. If a file cannot be removed (for example, another program has
  it open), the app names it and does not reload, so you can try again.

Uninstalling DepoAudio may leave the app data folder in place. Use "Clear all
DepoAudio data" first, or delete the folders listed above.

## Encryption at rest (recorded exception)

DepoAudio does not encrypt `library.json`, `prefs.json`, web view storage or
backup zips itself. This is a recorded exception for a local, single-user tool.
The compensating control is the operating system's full-disk encryption:
FileVault on macOS, BitLocker or Device Encryption on Windows. Turn it on for
any computer that holds case recordings. Settings → Your data says so next to
the storage locations. See `SECURITY.md`.
