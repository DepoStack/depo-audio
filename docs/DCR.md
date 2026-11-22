# Liberty / BIS Digital DCR in DepoAudio (experimental)

DepoAudio reads `.dcr` court recordings that start with the `HGCRLCRS` signature with its own
reader, [`src-tauri/src/dcr.rs`](../src-tauri/src/dcr.rs). The layout comes from interoperability
notes the project owner supplied. It has **not** been checked against a recording made by Liberty
Court Recorder or BIS Digital: the tests use files DepoAudio builds itself, and two synthetic tones
the owner made with a rebuild tool
([`scripts/fixtures/dcr/vendor-notes-samples/`](../scripts/fixtures/dcr/vendor-notes-samples/README.md)).
Neither of those has been opened in the vendor's player.

## File layout (in our own words)

```
 offset 0        header: "HGCRLCRS", u32 major, u32 minor   (version = major × 100 + minor)
 offset 16       meta chunk (XML)            version 21.00 and later
                 data chunk (the audio)
                 footer chunks, each optional: info, mark (bookmarks), rinf (session),
                 prot, sign, …
                 directory: 24-byte entries (tag, id, offset, size), closed by
                 "head" + its own offset and size
 end − 12        three u32 checksums          version 21.00 and later
```

Every chunk is framed as `<tag>#bgn`, a u32 id, the payload, `<tag>#end`. All numbers are
little-endian.

- **meta**: XML, UTF-8 unless it declares `windows-1252`, inside a `<Root>` element. Text is in
  CDATA, binary values are hex (lower case; DepoAudio accepts either), integers are attributes
  written as signed 32-bit numbers (DepoAudio reads them back as the same 32 bits), and 64-bit
  values and times are child elements with two halves (`N64Low`/`N64High`, or `FTLow`/`FTHigh`
  for a Windows FILETIME). DepoAudio reads the channel mask, the one-channel WAVEFORMATEX, the
  channel names, the password flags and the time-zone offset. Before 21.00, the format is a raw
  WAVEFORMATEX in `wfmt` and the mask is at the start of `chnl`.
- **data**: at each step ("atom") one block per active channel, lowest channel bit first, with
  8-byte records between atoms: start of plain audio, start of password-protected audio, a video
  frame, an attachment, or a remote participant joining or leaving.
- **Checksums**: each is a byte sum with 41 repeating weights, in wrapping 32-bit arithmetic,
  starting at 0. The first covers everything before the `data` chunk, the second an empty `data`
  chunk (the same value for every file), the third everything after the `data` chunk up to the
  last checksum. They do not cover the audio.

## What DepoAudio does

| Situation | What happens |
| --- | --- |
| Checksums | Checked when the file is read. The queue and the export record say `checksums match` or `checksums do not match`; a mismatch is a warning and the recording is still read (the notes say the recorder's player does not check them when it opens a file). |
| Remote participant joins / leaves | A join adds the bits of the record's `ac` mask to the recorded channels, a leave removes them, `ac="0"` changes nothing. Absent channels are silent so all channels stay on one timeline. A record whose `ac` cannot be read stops the conversion. |
| Redactions (bookmark type 13 begins, 14 ends) | Applied by default, with no option to ignore them. Each begin pairs with the next unused end at or after it. A begin without an end runs to the end of the recording; an end without a begin is ignored with a notice; a begin without a position stops the conversion. Overlaps are joined and the strictest action wins. **Skip** (`<play_skip>`, `<play_omit>`, or no tag) leaves the audio of every channel out, so the output is shorter; **black** (`<play_black>`, `<play_empty>`) and **distort** (`<play_dist>`, a video effect) write silence; only **play** (`<play_play>`) keeps the audio. A bookmark section that cannot be read stops the conversion. |
| Password-protected audio (after `audi#crp`) | Never decoded. It becomes silence of the same length (counted at the plain atom size) and the queue warns before export. Password-protected and deleted attachments, like all attachments, are never extracted. |
| `prot` chunk | Its layout is not known; the queue warns. |
| Session start (`rinf` `STARTREC`) and time zone | Written to the export record as stored. The notes do not settle whether the start is UTC or local time, so DepoAudio does not convert it and says so. The time-zone offset is local time minus UTC in minutes, measured on the recording machine. |
| Missing footer chunks | Fine: only `meta` (or `wfmt` and `chnl`) and `data` are needed. |
| Speex, unknown codecs, more than one `data` chunk | Refused with a message before anything is written. |
| GSM 6.10 or MP3 with redactions, protected sections or participant changes | Refused: DepoAudio has no silent block for those codecs. |

## Privacy

Case and docket fields, registration data, password data, bookmark notes (public and private),
speakers, attachment names and the text of remote-participant records are never kept in results,
logs, queue notices or export records. A redaction's note is searched only for its action tag. The
tests seed each of these and check that none of them leaves the reader.

## Export record

The optional `source.dcr` object (schema: [`export-manifest.schema.json`](export-manifest.schema.json))
holds the format version, codec, channel names, counts of skipped video frames and attachments,
participant changes, `checksum`, bookmark counts by type, `redactions`
(`[{startSecs, endSecs, action, applied}]`, in recording time, with no note text), `timing` when
skipped audio made the output shorter, `sealedSections` / `sealedSecs` when protected audio was
silenced, and `startTime` / `timeZone` when the recording has them.
