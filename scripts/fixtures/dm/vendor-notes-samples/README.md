# Owner's synthetic `.dm` samples

These four files are synthetic test tones made by the project owner's own
rebuild tool, which writes `.dm` and `.bmf` files following the layout in
`docs/formats/dm.md`. They are not vendor programs or parts of one, they were
not made by SoundSync or DigitalCAT, and they are not court audio or any other
recording of people.

| File                      | What it is                                                                                                                                               |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tone_440hz_11025_2s.wav` | The source tone: a 440 Hz sine, 16-bit mono, 11025 Hz, 2 seconds (22050 samples).                                                                        |
| `tone_gsm.dm`             | That tone as GSM 6.10 (SoundSync's default header): 9 full 520-byte blocks, the first five stamped 1700000000 and the last four 1700000001 (2023-11-14). |
| `tone_gsm.bmf`            | A marker file for `tone_gsm.dm` with two entries, at 0 ms and 1000 ms, each with a short text.                                                           |
| `tone_gsm_decoded.wav`    | The owner's decoded reference for `tone_gsm.dm`: 16-bit mono PCM, 11025 Hz, 23040 samples (9 blocks of 2560).                                            |

The owner says the GSM bytes came from a Windows GSM 6.10 encoder, and the `.bmf`
container was written by the owner's tool, not by Windows, so its padding
differs from one SoundSync would write; the `FileInfo` stream inside is what
DepoAudio reads.

Tests that use them:

- `bmf.rs` reads the marker counts and checks that 0 ms and 1000 ms land on
  blocks 0 and 5, and that no marker text reaches the summary.
- `manifest.rs` checks the export record's `markerFile` against the schema.
- `conversion.rs` (`the_owner_gsm_sample_decodes_to_the_owner_decoded_wav`)
  converts `tone_gsm.dm` with FFmpeg's `gsm_ms` decoder and checks that the
  result equals `tone_gsm_decoded.wav` sample for sample.

| File                      | SHA-256                                                            |
| ------------------------- | ------------------------------------------------------------------ |
| `tone_440hz_11025_2s.wav` | `92bd0993abb9e2824cb83248ab799315fffe6f1f115571ae5c47dc6c91352f59` |
| `tone_gsm.dm`             | `5aee9ec1f33ab373d99e1bdcf54013878612a2f347bfe7b29d9065f533cc8830` |
| `tone_gsm.bmf`            | `829b55dc6234ee21d6cd4be40fa99c4b9cd540d775abdc198902203747fb6b8b` |
| `tone_gsm_decoded.wav`    | `5d47ab9c42cdb32237b4bad95b71c65b381d41e50f3321d35f2abfebdbd6ba84` |
