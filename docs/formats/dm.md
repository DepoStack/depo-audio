# DigitalCAT / SoundSync `.dm` audio: file layout

Source: owner-supplied interoperability notes; not yet confirmed against a
recorded file. DepoAudio has read only synthetic `.dm` files built by its own
tests from this description. A file whose structure does not match it (the
header sizes, the format, or a block length that does not fit) is refused and
nothing is converted. A file whose structure matches but whose meaning differs
from these notes would not be detected: for example erased blocks marked some
other way than stamp 0, stamps counted from another epoch, or blocks meant to
be played in some order other than file order. Such differences can only be
found by checking a real recording.

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
  WAV may end with one shorter block. The notes say SoundSync's player stops
  at a short block.
- A stamp of 0 marks a block whose audio was recorded over. Its bytes are
  stale and are skipped.
- Stamps normally rise by 0 or 1 second per block (a block is about a quarter
  second) and jump during a pause. After part of a recording is recorded over,
  the newer blocks are added at the end of the file, so stamps can go
  backwards. Playback in SoundSync follows file order. A recorder clock set
  back during a session would also make stamps go backwards; the file does not
  say which happened.
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
recorded over, in file order, as `data`. A short block keeps only its whole
codec blocks. FFmpeg decodes that copy; the `.dm` itself is only read. Audio
over 4 GiB is refused. The export record's `source.dmTimeline` lists the blocks,
the skipped blocks and where stamps jump (see `docs/export-manifest.schema.json`).

Where a file departs from the notes in ways that still leave its blocks
readable, DepoAudio keeps the audio it can and says what it left out:

- A file that ends partway through a block (a recording or copy that did not
  finish) keeps every complete block before that point. The incomplete block
  is left out and its size is recorded as `truncatedTailBytes`. A file with no
  complete block is refused.
- A short block that is not the last block keeps its whole codec blocks, and
  the blocks after it are kept too, with a warning naming the block. This
  differs from SoundSync's player as the notes describe it. More than 16 such
  blocks are taken as a file that does not match the layout and are refused.

`scripts/fixtures/dm` holds five small synthetic `.dm` files and the WAV
`dm.rs` writes for each; the Rust tests and the release smoke both check them.
