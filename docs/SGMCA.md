# SGMCA (Direct Audio) in DepoAudio

An SGMCA recording is a standard **Ogg** stream ([RFC 3533](https://www.rfc-editor.org/rfc/rfc3533))
carrying one **Opus** stream ([RFC 7845](https://www.rfc-editor.org/rfc/rfc7845)) in channel
mapping family 255: each court channel is its own uncoupled mono Opus stream. There is no
encryption and no proprietary codec. DepoAudio's handling lives in
[`src-tauri/src/sgmca.rs`](../src-tauri/src/sgmca.rs) and is written from those public
specifications only.

## File layout

```
 byte 0
 ┌──────────────────────────────── Ogg page 0 (BOS) ────────────────────────────────┐
 │ "OggS" │ ver 0 │ flags 0x02 │ granule 0 │ serial │ seq 0 │ CRC-32 │ lacing │ body │
 │        body = OpusHead                                                            │
 │          "OpusHead" │ version 1 │ channels C │ pre-skip │ input rate 16000 │ gain  │
 │          mapping family 255 │ streams N = C │ coupled M = 0 │ mapping 0,1,…,C-1    │
 └───────────────────────────────────────────────────────────────────────────────────┘
 ┌──────────────────────────────── Ogg page 1 ──────────────────────────────────────┐
 │ body = OpusTags: "OpusTags" │ vendor │ comment count │ KEY=value …                │
 └───────────────────────────────────────────────────────────────────────────────────┘
 ┌──────────────────────────────── Ogg pages 2 … ───────────────────────────────────┐
 │ granule = 48 kHz sample count at the end of the last packet on the page           │
 │ body = audio packets, one per 20 ms; each packet holds C mono Opus packets:       │
 │   [ch 0, self-delimited] [ch 1, self-delimited] … [ch C-1, normal framing]        │
 └───────────────────────────────────────────────────────────────────────────────────┘
 ┌──────────────────────────────── last page (EOS, flags 0x04) ─────────────────────┐
 │ granule = final sample position; may stop inside the last packet (end trim)       │
 └───────────────────────────────────────────────────────────────────────────────────┘

 duration = (final granule − first sample − pre-skip) / 48 000 seconds
```

Ogg CRC-32: polynomial `0x04C11DB7`, MSB first, initial value 0, no final XOR, computed over
the whole page with its CRC field set to zero.

## What DepoAudio does with it

| Step | How |
| --- | --- |
| Find the stream | The first CRC-correct beginning-of-stream page that carries `OpusHead`, at byte 0 or inside the first 8 KiB (a prefix some exports carry is skipped; stray `OggS` bytes are not mistaken for it). A file holding another codec, or a damaged header, gets its own message. |
| Check it | Every page's capture pattern, version, sequence and CRC; packets reassembled across pages; `OpusHead` fields checked against RFC 7845 (channel count, stream and coupled counts, mapping table). Damage is reported with the page and byte where it is. |
| Duration | Exact, from the last granule position minus pre-skip, not from an estimate. |
| Decode, mix, split | FFmpeg with the **libopus** decoder. FFmpeg 6.1's built-in Opus decoder turned one channel of a 16 kHz family-255 test stream into full-scale noise; libopus decodes it cleanly. Split Channels gives one file per court channel. |
| Write (Convert → SGMCA) | FFmpeg `libopus` with `-mapping_family 255 -frame_duration 20 -ar 16000`, 32 kbps per mono stream, Ogg muxing. Every output is read back and must have the Direct Audio layout or it is removed. |
| Slice (library only for now) | Copies Opus packets without re-encoding: starts 80 ms before the cut for decoder pre-roll, sets the new pre-skip so playback starts on the exact requested sample, trims the end with the last page's granule, rebases granules to zero and re-frames every page with new sequence numbers and CRCs. Not yet on screen: a slice is a new output and needs its export record. |

## Not yet confirmed against a real recording

The tests use synthetic files made with FFmpeg (sine tones, no speech). A real SGMCA file
would confirm:

- that the stream starts at byte 0 (the prefix handling stays in case some do not);
- the pre-skip value and any comments in `OpusTags` (channel labels, start time);
- frame duration and bitrate, which matter only for byte-identical re-encoding, not reading.
