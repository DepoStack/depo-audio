# SGMCA (Direct Audio) in DepoAudio

Stenograph Case CATalyst records multichannel "Direct Audio" into `.sgmca` files. DepoAudio's
handling lives in [`src-tauri/src/sgmca.rs`](../src-tauri/src/sgmca.rs).

**Source of the layout.** The container layout below is written in our own words from
owner-supplied interoperability notes; it is not yet confirmed against a file CATalyst wrote.
Every test uses synthetic files made by DepoAudio's own code or by FFmpeg (sine tones, no
speech). The audio inside each recording session is standard **Ogg**
([RFC 3533](https://www.rfc-editor.org/rfc/rfc3533)) carrying **Opus**
([RFC 7845](https://www.rfc-editor.org/rfc/rfc7845)), with no encryption and no proprietary
codec.

## File layout

All integers are little-endian.

```
 offset 0
 ┌───────────────── file header, 40 bytes ─────────────────┐
 │ u32 header size = 40                                     │
 │ u32 format version = 1                                   │
 │ 16 bytes recording identifier (a GUID, Windows layout)   │
 │ 16 bytes reserved, zero                                  │
 └──────────────────────────────────────────────────────────┘
 ┌──────────────── segment header, 56 bytes ───────────────┐  one per recording session
 │ u32 header size = 56                                     │
 │ u32 version = 1                                          │
 │ u32 record kind = 2 (audio)                              │
 │ u32 reserved = 0                                         │
 │ 32 bytes start time, ASCII, NUL-padded:                  │
 │   YYYY-MM-DDTHH:MM:SS±hhmm  (local wall-clock time)      │
 │ u64 payload length in bytes                              │
 ├──────────────── payload ────────────────────────────────┤
 │ a complete Ogg Opus stream: OpusHead page, OpusTags      │
 │ page, audio pages, end-of-stream page                    │
 └──────────────────────────────────────────────────────────┘
 ┌──────────────── next segment header … ──────────────────┐
```

So the first `OggS` is at byte 96 (40 + 56). Each segment is its own Ogg stream with its own
serial number, `OpusHead` and pre-skip. Sessions are stored in the order they were recorded,
and the time between two sessions is not in the file at all. Channel count, sample rate and
tags live only inside each Ogg stream; the binary headers hold no case, witness or channel
names.

Inside each stream, per the notes: a live recording uses Opus channel mapping family 255 with
one uncoupled mono stream per channel and an identity mapping; CATalyst's import path is said
to use family 1 with the same uncoupled layout. Granule positions and pre-skip count 48 kHz
samples. The Ogg CRC-32 is polynomial `0x04C11DB7`, MSB first, initial value 0, nothing XORed
at the end, computed over the page with its CRC field zeroed.

The notes also say that while recording, the length in the last segment header is rewritten
after each Ogg page is appended, and that CATalyst lines audio up with a transcript by
comparing each stroke's wall-clock time with the segment start times (the stored `±hhmm` is
said to be read but not applied).

A worked example from the notes (two channels, identifier
`{12345678-9ABC-DEF0-1122-334455667788}`, start `2026-11-22T10:15:30-0500`, payload 123 456
bytes) is reproduced byte for byte by the test `worked_example_header_bytes_match_the_notes`,
and the example `OpusHead` page's CRC (`0x801384E6`) by
`worked_example_opus_head_page_has_the_crc_in_the_notes`.

## Reading

| Step | How |
| --- | --- |
| File header | Must be 40 bytes, version 1 (the notes say CATalyst refuses a higher version and finds no playback position unless it is exactly 1). Any other size or version is refused with the value found. A file that starts with `OggS` is read as the **older bare-Ogg layout** earlier DepoAudio builds wrote: one session, no start time, and a note saying so. Anything else is refused; the earlier "find the stream somewhere in the first 8 KiB" search is gone. |
| Segments | Walked by their payload lengths with strict bounds. Each header must be 56 bytes, version 1, kind 2; otherwise the segment number and byte are named. |
| Each stream | Every page's capture pattern, version, sequence and CRC; packets reassembled across pages; `OpusHead` checked against RFC 7845. Damage is reported as `Segment N: …` with the byte where it is. Bytes after a stream's end-of-stream page are refused. |
| Layouts read | Family 255 or 1 with one uncoupled stream per channel and an identity mapping, or family 0 (plain mono or stereo). Any other layout is refused. |
| Joining | All sessions must have the same channel count, and the same Opus stream layout, or the file is refused with a message naming both counts. Sessions are joined in file order with no gap; the queue says so: "This file holds 3 recording sessions; they are joined in order (gaps between them are not kept)." |
| Still recording | If the last segment is shorter than its header says, only its complete pages are read. If there are bytes after the last segment that no header counts (a page appended before the header was rewritten), they are not read. A last session with no complete audio yet is left out. Each case is flagged (`growing`), noted, and shown in the queue. |
| Start times | Each segment's text is kept as stored and parsed into a local time and offset when it has the form above; text that does not parse is kept and named in a note. |
| Duration | Per session: last granule position minus first sample minus pre-skip. The file's duration is the sum. |

## Decoding

FFmpeg decodes with the **libopus** decoder (FFmpeg 6.1's built-in Opus decoder turned one
channel of a 16 kHz family-255 test stream into full-scale noise). No copy of the audio is
made: DepoAudio writes a small temporary FFmpeg concat script (a few hundred bytes, removed
when the job ends) whose entries point at each session's byte range of the original file
through FFmpeg's `subfile` protocol, with each session's exact duration, and reads it with
`-f concat -safe 0 -protocol_whitelist file,subfile`. A bare-Ogg file is read in place.
Every ffprobe and FFmpeg call that reads the source (the probes, Scan, auto-level, the
waveform, merge and conversion) uses this feed and its input options; the queue's duration and
WAV size estimate come from the reader's joined duration, because ffprobe cannot read the
Direct Audio layout. A synthetic three-session file (300, 700 and 1500 Hz tones, families 255, 1
and 255) converts to exactly the sum of its sessions' samples with each tone in its place
(`every_recording_session_is_converted_in_file_order`, which also runs the app's own ffprobe
arguments). Scan, the waveform and merge were not run against a multi-session file here.

## Writing (Convert → SGMCA)

FFmpeg encodes as before: `libopus`, `-mapping_family 255`, 20 ms frames, `-ar 16000`, 32 kbps
per mono stream, Ogg muxing. DepoAudio then writes the Direct Audio file next to it and
replaces FFmpeg's bare Ogg file with it:

- the 40-byte file header with a fresh random identifier (UUID v4, Windows byte layout) and
  reserved zeros;
- one segment per recording session, each with its 56-byte header and exact payload length.
  With one session the Ogg bytes are copied unchanged. With several (an SGMCA source with
  several sessions), each session is cut out of the encoded stream at the sample where it
  starts, without re-encoding (80 ms decoder pre-roll, exact pre-skip, end trim, new serial
  number), so it keeps its own start time.

Segment start times come from, in this order:

1. the source's own SGMCA segment headers, copied as stored, one output segment per source
   session (`sgmca-segment-start`);
2. a BWF source's date plus its bext time reference, in whole seconds, with this computer's UTC
   offset for that date because BWF stores none (`bwf-bext`). FFmpeg 6.1 reports the bext
   origination date as the WAV's `date` tag, which a RIFF INFO creation date shares;
3. otherwise the time of conversion, in this computer's local time (`conversion-time`). This is
   not when the recording was made, and the export record says so.

Leading-silence trim moves the first sample by an amount FFmpeg does not report, so with trim
the output is one segment dated at conversion time. DigitalCAT `.dm` files have no timeline
DepoAudio reads, so they fall under 3. The local offset comes from the operating system
(`localtime_r` on macOS and Linux, `SystemTimeToTzSpecificLocalTime` on Windows); only the
Linux path has been run here.

Every output is read back through the reader and removed unless it has the file header (version
1, an identifier), complete segments with the planned start times in order, and family 255 with
one uncoupled mono stream per channel.

## Export record

An SGMCA source adds an optional `source.sgmca` object: `fileVersion`, `guidPresent`,
`segments` (each with `startText`, `startLocal`, `utcOffset`, `startSample` in 48 kHz samples of
the joined audio, `durationSecs`, `channels`, `mappingFamily`), `growing` and `legacyBareOgg`.
An SGMCA output adds `output.sgmca`: `fileVersion`, `guidPresent`, `startSource`, `startNote`
and its segments as read back.

The recording identifier itself is **not** recorded, only whether one is present. It identifies
one recording, the record already identifies the source by its SHA-256, and records travel
further than recordings; nothing in DepoAudio uses the identifier. The notes say CATalyst stores
it but does not compare it.

## Slicing (library only for now)

`sgmca::slice` copies Opus packets without re-encoding (80 ms pre-roll, exact pre-skip, end
trim, rebased granules, new CRCs) into a new Direct Audio file with one segment. A cut must stay
inside one recording session; a range that runs into the next session is refused, because the
gap between sessions is not in the file. The new segment's start is the session's start plus
the whole seconds skipped (the header holds no fractions). Not on screen yet: a slice is a new
output and needs its export record.

## Not yet confirmed against a real recording

- the whole container layout above, and that CATalyst opens what DepoAudio writes;
- whether pausing a recording starts a new segment;
- whether CATalyst ignores the stored UTC offset everywhere;
- the pre-skip, frame duration, bitrate and tags a real recording uses.

A short tone file imported in CATalyst ("Associate Audio File", Other Audio) would settle most
of these.
