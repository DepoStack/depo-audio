# Synthetic `.dm` fixtures

Small `.dm` files laid out as `docs/formats/dm.md` describes, and the WAV that
`src-tauri/src/dm.rs` writes for each. None of them is a real recording: the
codec bytes are generated tones that FFmpeg encoded (libgsm_ms, adpcm_ima_wav,
adpcm_ms, pcm_mulaw, pcm_u8), placed into blocks by hand.

| File                | What it covers                                                                                                                                                                           |
| ------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `gsm-default`       | The SoundSync default GSM 6.10 header, an erased block, a pause, stamps going backwards, a 160-byte last block                                                                           |
| `ima-adpcm-damaged` | A 1034-byte block before the end (one codec block and 10 stray bytes), where reading stops, and after it two more blocks and a file cut off inside its last block, none of which is read |
| `adpcm-ms`          | The 50-byte MS ADPCM format, an erased block and a short last block of whole codec blocks                                                                                                |
| `mulaw-odd-format`  | A 19-byte format (padded in the WAV), a pause and a short last block                                                                                                                     |
| `pcm-u8-odd`        | A 16-byte format with no `cbSize`, an erased block and an odd amount of audio (padded in the WAV)                                                                                        |

`golden_dm_fixtures_demux_to_the_committed_wavs` in `dm.rs` fails if `dm.rs`
stops writing these exact WAVs; `DEPOAUDIO_WRITE_DM_GOLDEN=1` rewrites them
after a deliberate change. `scripts/container-smoke-fixtures.mjs --self-test`
checks that its JavaScript reader gives the same bytes, and the release smoke
decodes each WAV with the bundled FFmpeg sidecars.

`vendor-notes-samples/` holds the owner's synthetic GSM test tone, its `.bmf`
marker file and the owner's decoded WAV; its own README says what they are.
They are not in the list above: no `dm.rs`-written WAV is committed for them,
and a test compares their decode with the owner's WAV instead.
