# FTR `.trm` sample from the owner's rebuild tool

`sample_hex_report.trm` was made by the project owner's own rebuild tool,
**not by FTR (For The Record) software**. It is laid out the way the owner's
interoperability notes describe FTR's recorder writing a file. It is not a
real recording and contains no court audio: its AAC-LC audio is genuine but
silent.

`sample_hex_report.txt` is the field-by-field hex report the same tool wrote
for it (offsets, bytes and what each field means, then the first and last
512 bytes).

What the file holds, as DepoAudio's tests check it
(`src-tauri/src/trm.rs`, the `vendor_notes_sample_*` tests):

- 106 120 bytes, SHA-256
  `46a55edcfdd2950d608ed4673b842c01fc7ee4ddcc6086ddfd3a6a5397ae1d4d`
- format tag `0x4180` (FTR AAC, one mono stream per microphone), 4 channels,
  16 000 Hz, 240 audio chunks of one round each: 15.360 s
- FFmpeg 6.1.1's native `ftr` decoder reads it to 245 760 samples per
  channel, all zero
- `TUTC` and `TLOC` both 2026-01-01 09:00:00; empty `USER` and `UDAT`;
  `FILE` = `Synthetic sample (not FTR)_20260101-0900_01dc7afd0268a800.trm`
- both boundary flags set (`avih` flags `0x60000110`), `FTRA` {1, 0, 0, 240,
  0xFFFFFFFF}, no `SIGN`/`CERT`

Because the file was not made by FTR, passing these tests shows that
DepoAudio reads the layout the notes describe; it does not show that real FTR
recordings are laid out that way.
