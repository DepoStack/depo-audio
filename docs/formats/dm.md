# DigitalCAT / SoundSync `.dm` audio: file layout

Source: owner-supplied interoperability notes, written from a study of how
SoundSync reads and writes these files. They have not been confirmed against
a file SoundSync itself recorded: the copy studied could not record or save.
DepoAudio has read only synthetic `.dm` files: ones built by its own tests
from this description, and a GSM test tone the owner made with their own tool
following the same notes (`scripts/fixtures/dm/vendor-notes-samples`). A file
whose structure does not match this layout (the header sizes, the format, or
a block length longer than the block size) is refused and nothing is
converted. A file whose structure matches but whose meaning differs from
these notes would not be detected: for example erased blocks marked some
other way than stamp 0, or stamps counted from another epoch. Such
differences can only be found by checking a real recording.

All numbers are little-endian. There is no magic number and no checksum.

## Header

| Offset | Size        | Field        | Notes                                                                                                                                |
| ------ | ----------- | ------------ | ------------------------------------------------------------------------------------------------------------------------------------ |
| 0      | 4           | version      | Written as 1. SoundSync does not check it, and neither does DepoAudio.                                                               |
| 4      | 4           | header size  | Size of the two fields that follow. Always 8; SoundSync does not check it. DepoAudio refuses any other value.                        |
| 8      | 4           | format size  | Bytes of the audio format that follow the header: 18 for PCM, 18 + extra size otherwise (20 for GSM). DepoAudio takes 16 to 1 MiB.   |
| 12     | 4           | block size   | Bytes of audio in a full block. DepoAudio takes 1 byte to 16 MiB, whole codec blocks.                                                |
| 16     | format size | audio format | A Windows WAVEFORMATEX (format tag, channels, sample rate, byte rate, block alignment, bits per sample, extra-size and extra bytes). |

Blocks start at byte 8 + header size + format size (16 + format size).

## Blocks

Blocks repeat to the end of the file:

| Size   | Field  | Notes                                                   |
| ------ | ------ | ------------------------------------------------------- |
| 4      | stamp  | Unix time in whole seconds, UTC. 0 means recorded over. |
| 4      | length | Bytes of audio that follow.                             |
| length | audio  | Codec data, continuing the stream of the block before.  |

- Every block is `block size` bytes, so block _n_ starts at
  `16 + format size + n × (block size + 8)`. SoundSync relies on that fixed
  spacing to find blocks by stamp, including when it marks blocks as
  recorded over.
- A recording writes every block at the full `block size`. A WAV import
  (SoundSync copies the WAV's own format and does not re-encode) may end with
  one shorter block.
- The notes say SoundSync's player stops at the first block shorter than
  `block size`, wherever it is, and does not play that block; its cut command
  drops a short block and everything after it.
- Recording over existing sound sets the stamp of each overwritten block to
  0 (the audio bytes stay in the file) and adds the new blocks at the end of
  the file, so stamps can go backwards. A recorder clock set back during a
  session would also make stamps go backwards; the file does not say which
  happened.
- Stamps normally rise by 0 or 1 second per block (a block is about a quarter
  second) and jump during a pause; there is no other pause marker.
- For a recording, a stamp is the clock time when the block's buffer came
  back from the sound card: about the **end** of the block. For a WAV import,
  block _k_ is stamped with the import's start time plus the whole seconds of
  audio before it: about the **start** of the block. Nothing in the file says
  which kind it is. Positions come from counting blocks; stamps tie blocks to
  clock time to within about a second.
- GSM is decoded with one decoder state across blocks, in file order.

## Block size

The recorder picks a quarter second of whole codec blocks:
`frames = (byte rate / block alignment) / 4`, rounded down, at least 1, and
`block size = frames × block alignment`. GSM 6.10 at 11025 Hz gives 520 bytes
(eight 65-byte GSM blocks, 2560 samples); 16-bit mono PCM gives 5512 bytes at
11025 Hz and 4000 at 8000 Hz. DepoAudio warns, and still converts, when the
header says something else.

## Audio formats

The default is Microsoft GSM 6.10 (format tag 0x0031): mono, 11025 Hz, byte
rate 2239, block alignment 65, two extra bytes holding 320 samples per block.
The person recording can pick any format Windows offers, such as PCM
(0x0001), Microsoft ADPCM (0x0002), A-law (0x0006), mu-law (0x0007) and IMA
ADPCM (0x0011); a WAV import keeps the WAV's format, usually PCM.

## Marker files (`.bmf`)

SoundSync keeps bookmarks in a file named after the `.dm` with `.bmf` for the
extension (`Smith.dm` gives `smith.bmf`, in lower case, on Windows where case
does not matter). It is a Windows compound file (structured storage) with one
stream, `FileInfo`:

| Size   | Field    | Notes                                                                 |
| ------ | -------- | --------------------------------------------------------------------- |
| 4      | count    | Signed. Number of entries that follow.                                |
| 4      | position | Per entry, signed: milliseconds on SoundSync's timeline.              |
| 4      | length   | Per entry: bytes of text that follow.                                 |
| length | text     | Per entry: the bookmark's text in the Windows code page, no ending 0. |

SoundSync's timeline counts clock seconds from the **first block's stamp**:
a bookmark at _p_ ms seeks to the first block stamped
`first stamp + floor(p / 1000)`. It keeps only bookmarks from 0 to its
timeline length, which it sets from the amount of sound (every block's stated
length added up, divided by the byte rate, in whole seconds). A missing `.bmf`
means no bookmarks. The audio does not depend on it.

## What DepoAudio does with it

`src-tauri/src/dm.rs` checks every size against the file before reading, then
writes a temporary RIFF WAVE file: the `.dm`'s audio format unchanged as the
`fmt ` chunk, a `fact` chunk for compressed formats, and the blocks read that
were not recorded over, in file order, as `data`. FFmpeg decodes that copy;
the `.dm` itself is only read. Audio over 4 GiB is refused. The export
record's `source.dmTimeline` lists the blocks, the skipped blocks, what was
left out and where stamps jump (see `docs/export-manifest.schema.json`).

Blocks are read as SoundSync's player reads them, with one exception that
keeps audio SoundSync has not deleted:

- **Recorded-over blocks** (stamp 0) are never in the audio.
- **A short block** ends the reading, as it ends SoundSync's playback. Its
  whole codec blocks are kept (a short block is the tail of an imported WAV,
  not deleted audio), with a warning that SoundSync does not play them; any
  bytes after the last whole codec block are left out and counted. If the
  file goes on after a short block, the rest is not read and is not in the
  audio: it is counted as `bytesAfterShortBlock`, with a warning naming the
  block. SoundSync finds blocks, including the ones it marks as recorded
  over, by their fixed spacing, and a short block breaks that spacing, so
  DepoAudio cannot tell which later blocks SoundSync meant to erase.
- **An incomplete block** (the file ends partway through it, as a recording
  or copy that did not finish can) is left out, and every complete block
  before it is kept; its size is recorded as `truncatedTailBytes`. A file
  with no complete block is refused.
- **Time stamps** are recorded as stored (`firstStampUtc` for each segment),
  not moved to the start of their block, because the file does not say
  whether it was recorded or imported.

`src-tauri/src/bmf.rs` reads a `.bmf` beside the `.dm` (matched without
regard to case) and records in `markerFile` only counts: the entries, how
many carry text, how many SoundSync would keep, and how many point at the
stamp of a block DepoAudio read. Marker text can hold notes about the case,
so it is never kept, logged or written to the export record; there is no
option to include it yet. A `.bmf` that cannot be read is reported and never
stops a conversion.

`scripts/fixtures/dm` holds five small synthetic `.dm` files and the WAV
`dm.rs` writes for each; the Rust tests and the release smoke both check
them. `scripts/fixtures/dm/vendor-notes-samples` holds the owner's GSM test
tone, its `.bmf` and the owner's decoded WAV; a test converts the tone with
FFmpeg's `gsm_ms` decoder and checks the result equals that WAV sample for
sample (23040 samples).
