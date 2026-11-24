# SGMCA sample files from the owner's interoperability notes

These two files came with the owner-supplied interoperability notes on Stenograph Case
CATalyst Direct Audio (`.sgmca`). They hold **generated sine tones only** (no speech, no court
audio). The Opus audio was encoded with FFmpeg's libopus encoder (48 kHz, mapping family 255,
two uncoupled mono streams, 20 ms frames) and the Direct Audio container was written by the
owner's own inspection tool, following the layout in the notes.

**They were not written by CATalyst.** They show that DepoAudio reads the layout the way the
notes describe it, from bytes DepoAudio did not write; they do not show that CATalyst writes
or opens files this way.

| File | Bytes | SHA-256 | Contents |
| --- | --- | --- | --- |
| `tone_1segment.sgmca` | 110 497 | `8665a5588707f02886b488c0a8ff60b8ee6100b0edd4140ff4d9ba765ae256b6` | one session starting `2026-11-22T10:15:30-0500`: 10.000 s, left 1000 Hz, right 1500 Hz |
| `tone_2sessions.sgmca` | 155 505 | `a3bc17e49740ab2a015dff5d0f52905e95cfe8946b97c6594b8a6d62e2dde836` | the same session, then a second starting `2026-11-22T10:20:00-0500`: 5.000 s, left 440 Hz, right 660 Hz |

Both use pre-skip 312. The notes give these timeline expectations for `tone_2sessions.sgmca`
(sessions laid end to end, gaps skipped): a stroke at 10:15:35 plays from 5.0 s, one at
10:20:02 from 12.0 s, and one at 10:18:00 (between the sessions) from 10.0 s.

The Rust test `vendor_notes_samples_read_as_the_notes_describe` (in `src-tauri/src/sgmca.rs`)
checks every value above. With FFmpeg on `PATH`,
`vendor_notes_two_session_sample_decodes_each_tone_in_its_place` (in
`src-tauri/src/conversion.rs`) converts the two-session file and checks the tones and length.

Keep the files byte for byte: `.gitattributes` marks `*.sgmca` as binary.
