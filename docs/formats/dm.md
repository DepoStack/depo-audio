# DigitalCAT / SoundSync `.dm` audio: file layout

Source: owner-supplied interoperability notes; not yet confirmed against a
recorded file. DepoAudio has read only synthetic `.dm` files built by its own
tests from this description. If a real recording does not match it, DepoAudio
reports what it could not read and converts nothing.

All numbers are little-endian.

## Header

| Offset | Size        | Field        | Notes                                                                                                                                |
| ------ | ----------- | ------------ | ------------------------------------------------------------------------------------------------------------------------------------ |
| 0      | 4           | version      | Written as 1. Not checked.                                                                                                           |
| 4      | 4           | header size  | Size of the two fields that follow. Always 8; any other value is refused.                                                            |
| 8      | 4           | format size  | Bytes of the audio format that follow the header. 16 to 1 MiB.                                                                       |
| 12     | 4           | block size   | Bytes of audio in a full block. 1 byte to 16 MiB, whole codec blocks.                                                                |
| 16     | format size | audio format | A Windows WAVEFORMATEX (format tag, channels, sample rate, byte rate, block alignment, bits per sample, extra-size and extra bytes). |

Blocks start at byte 16 + format size.

## Blocks

Blocks repeat to the end of the file:

| Size   | Field  | Notes                                                   |
| ------ | ------ | ------------------------------------------------------- |
| 4      | stamp  | Unix time in whole seconds, UTC. 0 means recorded over. |
| 4      | length | Bytes of audio that follow.                             |
| length | audio  | Codec data, continuing the stream of the block before.  |

- A recorded block is exactly `block size` bytes. A file made by importing a
  WAV may end with one shorter block.
- A stamp of 0 marks a block whose audio was recorded over. Its bytes are
  stale and are skipped.
- Stamps normally rise by 0 or 1 second per block (a block is about a quarter
  second) and jump during a pause. After part of a recording is recorded over,
  the newer blocks are added at the end of the file, so stamps can go
  backwards. Playback in SoundSync follows file order.
- For a recording, a stamp marks roughly the end of its block; for an imported
  WAV, the start. Positions come from counting blocks; stamps only tie blocks to
  clock time, to within about a second.

## Block size

The recorder picks a quarter second of whole codec blocks:
`frames = (byte rate / block alignment) / 4`, rounded down, at least 1, and
`block size = frames × block alignment`. GSM 6.10 at 11025 Hz gives 520 bytes
(eight 65-byte GSM blocks). DepoAudio warns, and still converts, when the header
says something else.

## Audio formats

The default is Microsoft GSM 6.10 (format tag 0x0031): mono, 11025 Hz, byte
rate 2239, block alignment 65, two extra bytes holding 320 samples per block.
The recording software can also use other Windows audio formats, such as PCM
(0x0001), Microsoft ADPCM (0x0002), A-law (0x0006), mu-law (0x0007) and IMA
ADPCM (0x0011).

## Side files

Marker files (`.bmf`) and the `$$$_buffer_.dm` working file are not needed to
read the audio, and DepoAudio does not read them.

## What DepoAudio does with it

`src-tauri/src/dm.rs` checks every size against the file before reading, then
writes a temporary RIFF WAVE file: the `.dm`'s audio format unchanged as the
`fmt ` chunk, a `fact` chunk for compressed formats, and the blocks that were not
recorded over, in file order, as `data`. A short last block keeps only its whole
codec blocks. FFmpeg decodes that copy; the `.dm` itself is only read. Audio
over 4 GiB is refused. The export record's `source.dmTimeline` lists the blocks,
the skipped blocks and where stamps jump (see `docs/export-manifest.schema.json`).
