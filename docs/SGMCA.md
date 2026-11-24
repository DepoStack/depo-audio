# SGMCA (Direct Audio) in DepoAudio

Stenograph Case CATalyst records multichannel "Direct Audio" into `.sgmca` files. DepoAudio's
handling lives in [`src-tauri/src/sgmca.rs`](../src-tauri/src/sgmca.rs).

**Source of the layout.** The container layout below is written in our own words from
owner-supplied interoperability notes; it is not yet confirmed against a file CATalyst wrote.
Every test uses synthetic files (sine tones, no speech): most are made by DepoAudio's own code
or by FFmpeg, and two came with the notes, made by the owner's own tool rather than by
DepoAudio or CATalyst
([`scripts/fixtures/sgmca/vendor-notes-samples/`](../scripts/fixtures/sgmca/vendor-notes-samples/README.md)).
Those two show that the reader agrees with the notes on bytes DepoAudio did not write: one
session of 480 000 samples, and two sessions of 480 000 and 240 000 samples (15.000 s joined),
pre-skip 312, family 255, payload lengths 110 401 and 44 952, starts 10:15:30 and 10:20:00, and
the notes' timeline expectations (10:15:35 → 5 s, 10:20:02 → 12 s, 10:18:00 in the gap → 10 s)
(`vendor_notes_samples_read_as_the_notes_describe`). With FFmpeg, the two-session file
converts to exactly 720 000 samples with each tone in its place
(`vendor_notes_two_session_sample_decodes_each_tone_in_its_place`). The audio inside each recording session is standard **Ogg**
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
| Segments | Walked by their payload lengths with strict bounds. The notes say CATalyst reads a segment header's size and finds the audio right after it, and does not check the version, kind or reserved fields, so DepoAudio does the same: a header size below 56 is refused (the start time and length would not fit); a larger one, or a version other than 1 or kind other than 2, is read and named in a note. A length of 0 followed straight away by another segment header is a session with no audio, noted, and the walk goes on. |
| Lengths that stop short | If a header's length (0, or short of its pages) leaves Ogg pages that no header counts, DepoAudio walks those pages; if another session's header follows them, the file is **refused**, naming the segment and bytes, because reading on from the headers would leave later sessions out without saying so. Uncounted pages at the very end of the file are the still-recording case below. |
| Timestamps | A page's granule position must not be past the end of its packets (RFC 7845 §4): a jump means audio is missing or the timestamps are damaged, and the file is refused rather than given a length the audio does not have. Only the end-of-stream page may end before its packets do, to trim the end (RFC 7845 §4.5); a short granule on any other page, such as a trimmed last page whose end-of-stream flag is missing, is refused, because FFmpeg trims nothing there and would play those samples, moving every later session and losing the end while the counts still matched. Granule positions beyond the signed 64-bit range are refused, and the sums use checked arithmetic. |
| Each stream | Every page's capture pattern, version, sequence and CRC; packets reassembled across pages; `OpusHead` checked against RFC 7845. Damage is reported as `Segment N: …` with the byte where it is. Bytes after a stream's end-of-stream page are refused. |
| One stream per session | A page of a second logical stream (another serial number) inside a segment, before the first stream's end-of-stream page, is refused with the byte and both serial numbers: a session holds one Ogg stream, and skipping the other would leave its audio out without a word (`a_second_logical_stream_inside_a_session_is_refused`). |
| Layouts read | Family 255 or 1 with one uncoupled stream per channel and an identity mapping, or family 0 (plain mono or stereo). Any other layout is refused. |
| Joining | All sessions must have the same channel count (the notes say CATalyst refuses to append a different one), and the same Opus stream layout, or the file is refused with a message naming both counts. A different output gain is not refused but named in a note: the sessions are decoded as one feed, and which gain FFmpeg applies to a later session has not been checked. Sessions are joined in file order with no gap; the queue says so: "This file holds 3 recording sessions; they are joined in order (gaps between them are not kept)." |
| Still recording | If the last segment is shorter than its header says, only its complete pages are read. If there are bytes after the last segment that no header counts (a page appended before the header was rewritten), they are not read. A last session with no complete audio yet is left out, whether or not its header's length covers what is there: one that ends before its `OpusHead` or `OpusTags` page, or holds those and no audio page or end-of-stream page. Each case is flagged (`growing`), noted, and recorded: the reader counts the **unread bytes** (every byte after the last complete page read, not counting the headers of sessions left out, which were read) and lists each **session left out** with its number and why (`zero-length-header`, `no-complete-audio`, or `empty` for a 0-length segment before the last). The queue shows the same facts before converting, for example "The last session (3) could not be read: its header says 0 bytes. 12.3 MB after it were not read.", and the export record keeps them (`unreadBytes`, `omittedSegments`). A test cuts a three-session file at every byte and checks that every session finished before the cut is read, that every byte of the file is read, a left-out session's header, or counted as unread, and that nothing else is an error. |
| No samples | A file whose sessions together hold no audio samples (only headers and pre-skip) is refused by the reader with its own message, before FFmpeg is asked to read it. |
| Start times | Each segment's text is kept as stored (bytes that are not printable ASCII are shown as `?`) and parsed into a local time and offset when it has the form above; text that does not parse is kept and named in a note. |
| Duration | Per session: last granule position minus first sample minus pre-skip. The file's duration is the sum. |

## Decoding

FFmpeg decodes with the **libopus** decoder (FFmpeg 6.1's built-in Opus decoder turned one
channel of a 16 kHz family-255 test stream into full-scale noise). No copy of the audio is
made: DepoAudio writes a small temporary FFmpeg concat script (a few hundred bytes, in
DepoAudio's private work folder like every other temporary file, removed when the job ends)
whose entries point at each session's byte range of the original file
through FFmpeg's `subfile` protocol, with each session's exact duration, and reads it with
`-f concat -safe 0 -protocol_whitelist file,subfile`. A bare-Ogg file is read in place.
The concat demuxer starts the audio at minus the first session's pre-skip, where a bare Ogg file
starts at zero, and FFmpeg then moves the whole input later by that much. WAV, FLAC, MP3 and Ogg
outputs drop the gap, but M4A keeps it as a lead-in: every sample came out 312 samples (6.5 ms)
late. FFmpeg gets `-itsoffset` of minus that pre-skip on the concat input (the script's file name
carries the value), so the first sample is at zero for every output; measured with FFmpeg 6.1 by
the position of clicks in every output format
(`every_output_from_a_two_session_sgmca_keeps_each_click_in_place`).
Every ffprobe and FFmpeg call that reads the source (the probes, Scan, auto-level, the
waveform, merge and conversion) uses this feed and its input options; the queue's duration and
WAV size estimate come from the reader's joined duration, because ffprobe cannot read the
Direct Audio layout. A synthetic three-session file (300, 700 and 1500 Hz tones, families 255, 1
and 255) converts to exactly the sum of its sessions' samples with each tone in its place
(`every_recording_session_is_converted_in_file_order`, which also runs the app's own ffprobe
arguments). With Fade on, the fade-out is placed from the decoded length of this same feed (the
decoded-length pass reads the concat script with its input options), so it lands at the end of
the joined timeline, never at the end of the first or the last session alone, and the export
record's `processing.fadeOut` names that length. A two-session click file is converted in every
mode to WAV, MP3, M4A and SGMCA, plain, faded and with Normalize, Fade, high-pass and de-clip on;
every output holds the joined 240 000 samples with each click on its sample in its session, and a
faded one starts its fade at 4.5 s of 5 s
(`a_two_session_sgmca_source_lines_up_sample_for_sample`, beside the alignment matrix).
Scan, the waveform and merge were not run against a multi-session file here.

## Writing (Convert → SGMCA)

FFmpeg encodes with `libopus`, `-mapping_family 255`, 20 ms frames, `-ar 48000` (the notes say
CATalyst's own encoder runs at 48 kHz; earlier DepoAudio builds encoded at 16 kHz), 32 kbps
per mono stream, Ogg muxing. DepoAudio then writes the Direct Audio file next to it and
replaces FFmpeg's bare Ogg file with it:

- the 40-byte file header with a fresh random identifier (UUID v4, Windows byte layout) and
  reserved zeros;
- one segment per recording session, each with its 56-byte header and exact payload length.
  With one session the Ogg bytes are copied unchanged. With several (an SGMCA source with
  several sessions), each segment is cut out of the encoded stream at the sample where it starts, without re-encoding (80 ms
  decoder pre-roll, exact pre-skip, end trim, new serial number), so it keeps its own start
  time. The first segment always starts at the first sample: the writer refuses any plan that
  would leave encoded audio out.

Segment start times come from, in this order:

1. the source's own SGMCA segment headers, copied as stored, one output segment per source
   session (`sgmca-segment-start`). A start text that is not in the form above is never
   copied, since CATalyst could not place the output against a transcript: the output then
   falls under 3, as one segment, and the note names the source segment;
2. a BWF source's date plus its bext time reference, with this computer's UTC offset for that
   date because BWF stores none (`bwf-bext`). The header holds whole seconds only, so when the
   time reference falls between two seconds the start text is the whole second at or before the
   first sample (rounded down). **No source audio is ever left out to make the time round:**
   every sample is kept, and the export record gives the fraction as `startOffsetMs` and in the
   note, for example "CATalyst stores whole seconds; this recording's first sample is 0.42 s
   after the time written." FFmpeg 6.1 reports the
   bext origination date as the WAV's `date` tag, which a RIFF INFO creation date shares;
3. otherwise the time of conversion, in this computer's local time (`conversion-time`). This is
   not when the recording was made, and the export record says so.

Leading-silence trim moves the first sample later by a measured number of samples, which the
export record holds as `processing.leadingTrim`. When it removes any samples from a source with
one recording session, the SGMCA output is one segment dated at conversion time, and the note says
so. On an SGMCA source with **more than one** session, trim is **refused** before anything is
converted, for every output format: the sessions' boundaries and start times could no longer be
placed in the output, and a note that they were dropped would still leave a record that cannot
locate them. Converting without trim keeps them. DigitalCAT `.dm` files have no timeline
DepoAudio reads, so they fall under 3. The local offset comes from the operating system
(`localtime_r` on macOS and Linux, `SystemTimeToTzSpecificLocalTime` on Windows); only the
Linux path has been run here.

Every output is read back through the reader and removed unless it has the file header (version
1, an identifier), complete segments with the planned start times in order, every start time in
the form above, and family 255 with one uncoupled mono stream per channel.

**No sample dropped, padded or moved.** When the source is an SGMCA file and no filter moved its
start (no trim), its samples are known exactly from its pages, so the written output must hold
exactly that many 48 kHz samples, and each output segment exactly as many as the matching source
session. Anything else removes the output and fails with the numbers, for example "It holds
720000 samples (48 kHz) where the source holds 720001: 1 fewer, so audio would have been dropped
from the recording." The owner's two sample files convert to exactly 480 000 and 480 000 + 240 000
samples per session in Keep Original, Split Channels and Mix to Stereo with Normalize Volume
(`vendor_notes_samples_convert_to_sgmca_with_every_sample_in_its_session`), and changing what the
check expects by one sample, or moving 960 samples between sessions, removes the output
(`an_sgmca_output_that_does_not_hold_every_source_sample_is_removed`).

Every other source is checked the same way. A PCM or float WAV (RIFF or RF64, including the
temporary WAVs a `.dm` or `.dcr` is read into) states its exact length in its data chunk; any other
source (FLAC, MP3, a WAV still being recorded, ...) is decoded once before converting and its
samples counted at its own rate, and if that count fails nothing is converted. The output must
hold that many samples at 48 kHz; when the source is at another rate and its length there is not
a whole number of samples, the length rounded down or one more, never fewer. A last Opus frame
that is only partly audio is kept: libopus pads it and the last page's granule position ends at
the source's last sample, which FFmpeg 6.1 and 8.0 both write
(`a_wav_source_keeps_its_partial_last_opus_frame_in_sgmca_and_a_lost_one_fails`, 95 001 samples:
98 whole 20 ms frames and 921 samples). A stream that ended early fails, for example "It holds
94080 samples (48 kHz) where the source holds 95001: 921 fewer, so audio would have been dropped
from the recording." Only after a leading-silence trim that removed samples is the
count not checked.

A filter that **delays** the audio keeps the count and so needs its own check. FFmpeg's
`alimiter` without `latency=1` outputs its 5 ms look-ahead as a delay, so every sample is 240
samples late and the last 5 ms are lost (measured with FFmpeg 6.1). Mix to Stereo without
Normalize Volume now adds it with `latency=1`, which measured 0 samples late with the length
kept, for every output format. Every conversion chain, in any format, may hold only filters
measured not to move the audio (`pan`, `volume`, `asplit`, `aformat`, `adeclip`, `highpass`,
`loudnorm`, `afade`, `alimiter` with `latency=1`, and `silenceremove`, whose start shift is
recorded separately); any other filter, or the limiter without `latency=1`, is refused before
FFmpeg runs, and an SGMCA output is checked again after writing. The counts alone cannot see a
delay; the click test above checks positions in every format, including Mix to Stereo.

## Export record

An SGMCA source adds an optional `source.sgmca` object: `fileVersion`, `guidPresent`,
`segments` (each with `startText`, `startLocal`, `utcOffset`, `startSample` in 48 kHz samples of
the joined audio, `durationSecs`, `channels`, `mappingFamily`), `growing`, `unreadBytes` (bytes after
the last complete page converted, not counting the headers of sessions left out), `omittedSegments`
(each session in the file that was not converted: `index`, its 1-based place in the file, and
`reason`: `zero-length-header`, `no-complete-audio` or `empty`) and `legacyBareOgg`.
An SGMCA output adds `output.sgmca`: `fileVersion`, `guidPresent`, `startSource`, `startNote`,
`startOffsetMs` (how far the first sample falls after the whole second the first header names,
in milliseconds; null when the start was copied from the source's headers or is the conversion
time) and its segments as read back.

The recording identifier itself is **not** recorded, only whether one is present. It identifies
one recording, the record already identifies the source by its SHA-256, and records travel
further than recordings; nothing in DepoAudio uses the identifier. The notes say CATalyst stores
it but does not compare it.

## Slicing (library only for now)

`sgmca::slice` copies Opus packets without re-encoding (80 ms pre-roll, exact pre-skip, end
trim, rebased granules, new CRCs) into a new Direct Audio file with one segment. A cut must stay
inside one recording session; a range that runs into the next session is refused, because the
gap between sessions is not in the file. The cut is made exactly at the requested start and
end; no audio is added or removed to make the start round. The header holds no fractions of a
second, so the new segment's start is the session's start plus the whole seconds skipped (the
second at or before the first sample), and the report gives the fraction left over as
`startOffsetMs` with the same kind of note as the export record. Not on screen yet: a slice is a new output and needs its export
record.

## Not yet confirmed against a real recording

- the whole container layout above, and that CATalyst opens what DepoAudio writes;
- whether pausing a recording starts a new segment;
- whether CATalyst ignores the stored UTC offset everywhere;
- the pre-skip, frame duration, bitrate and tags a real recording uses.

A short tone file imported in CATalyst ("Associate Audio File", Other Audio) would settle most
of these.
