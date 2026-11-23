# Synthetic DCR samples from the owner's interoperability notes

These two files are **synthetic test tones**. The project owner made them with
the rebuild tool that accompanies the owner's DCR interoperability notes. They
are not recordings made by Liberty Court Recorder, BIS Digital or DCR Player,
they contain no vendor program code, and they are not court audio.

| File | What it is |
| --- | --- |
| `test_tone_2ch_8k_pcm16.dcr` | A 12-second, 2-channel tone (8 kHz, 16-bit PCM), channels named Judge and Witness, format version 27.00 |
| `test_tone_redacted_4-6s.dcr` | The same tone with one redaction from 4.000 s to 6.000 s marked "black out" in its bookmarks |

Neither file has been opened in the vendor's own player, so they show how the
owner's rebuild tool writes the layout, not how the vendor's software does.

DepoAudio's tests (`src-tauri/src/dcr.rs` and `src-tauri/src/conversion.rs`)
read them to check the reader: the plain tone lasts 12.000 s with both channel
names, its checksums match, and in the redacted tone 4.000 s to 6.000 s is
silent on both channels while the rest matches the plain tone.

The files are marked binary in `.gitattributes`. Do not edit them: the tests rely
on their exact contents.
