# DepoAudio Parity Contract

Baseline: `main @ 0047549` (v0.8.0 lineage, 2026-07-02).

This document is the inventory of behavior that installed users depend on.
Any refactor, redesign, or rebuild must preserve everything listed here unless
a change is deliberate and called out in the changelog. The characterization
test suite (see [Test index](#test-index)) mechanically enforces the starred
(★) items; the rest are verified by the release smoke test.

## Capability map (user-facing)

| Area               | Capabilities                                                                                                                                                                                                                                                                                                                                 |
| ------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Convert            | Single/batch conversion; output modes `stereo` (downmix), `keep` (original channels), `split` (one file per channel); formats wav/mp3 (128/192/320k)/flac/opus/m4a; sample-rate choice; presets; per-channel labels & volumes                                                                                                                |
| Processing         | Normalize (loudnorm), leading-silence trim, fade in/out, high-pass filter, de-clip; AI clarity chain: RNNoise denoise, optional dereverb, bandwidth extension (FlashSR), and bounded per-channel auto-level analysis. DeepFilterNet assets are reserved for a future complete pipeline and are not selectable.                               |
| Scan (AI analysis) | Loudness/peak per channel, clipping, narrowband detection, turn detection (Smart-Turn), VAD speech ratio (Silero), active speaker-slot estimate (pyannote segmentation; not voice identification), and DNSMOS quality score — all bounded to sample caps + subprocess timeouts                                                               |
| Formats            | Court-reporting: SGMCA (header strip), FTR/TRM (native `ftr` decoder), BWF, DigitalCAT (experimental); rejected with guidance: Eclipse AES, Liberty DCR; video containers (audio extracted); phone formats (AMR/3GA), CAF                                                                                                                    |
| Merge              | Two-source sync detection + merge                                                                                                                                                                                                                                                                                                            |
| Library            | Cases → sessions → participants → files; rename/archive/delete; import with case-name sanitization; auto-filing after conversion with case-name inference from filenames                                                                                                                                                                     |
| Player             | Playlist (drop/browse, dedupe; proprietary SGMCA/TRM/FTR inputs are routed to Convert first); transport with keyboard map (Space/K, ←/→ ±5s, J/L ±10s, ↑/↓ speed, [/] track, B bookmark); speed steps 0.5–2×; A-B loop; persistent bookmarks + clipboard export; synced transcript editor (import SRT/VTT/TXT, stamp, proof, export SRT/TXT) |
| CAT detection      | Detect installed CAT software; scan job folders                                                                                                                                                                                                                                                                                              |
| Settings           | Theme (system/light/dark), startup defaults ("remember last used" sentinel = empty string), processing parameters, model manager (download/delete, hash-verified), software update section                                                                                                                                                   |
| Updates            | Tauri updater against the signed GitHub Releases `latest.json`; updater configuration is injected only into release builds, which fail closed unless updater and platform-signing credentials are present                                                                                                                                    |

## IPC surface

The de-facto API between the React frontend and the Rust core. Names,
argument shapes (camelCase), and return shapes are all contract.

| Command                                                                                  | Contract notes                                                                                                                                                                                                                                     |
| ---------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `health_check`                                                                           | Executes both sidecars with `-version`, verifies the exact native FTR decoder, and returns `{ffmpeg, ffprobe, ftrDecoder, models, accelerator, tier}`                                                                                              |
| `get_formats_list` / `detect_format` ★                                                   | Format registry incl. status/handler/notes; detection by extension (see helpers tests)                                                                                                                                                             |
| `infer_case_name_cmd` ★                                                                  | Strips date patterns, underscores → spaces                                                                                                                                                                                                         |
| `begin_conversion_batch_cmd`, `cancel_conversion_cmd`, `convert` ★                       | Generation-based cancellation is serialized with the final output/library commit. `convert` emits progress, done (including warnings), cancelled, or error events; event-delivery failures reject instead of leaving the frontend waiting forever. |
| `analyze_audio_cmd`, `cancel_scan_cmd`                                                   | `AnalysisResult` (camelCase); analysis is cancellable and bounded to a 180 s sample (`ANALYSIS_SAMPLE_SECS`)                                                                                                                                       |
| `score_quality_cmd`                                                                      | DNSMOS SIG/BAK/OVR 1–5, first ~9 s                                                                                                                                                                                                                 |
| `detect_speakers_cmd`                                                                    | Pyannote powerset argmax; first 60 s; reports active model slots (count ≥ 1), not clustered or identified voices                                                                                                                                   |
| `detect_speech_cmd`                                                                      | Silero VAD segments, 300 ms merge gap, 0.5 threshold                                                                                                                                                                                               |
| `waveform_peaks_cmd`                                                                     | Produces a bounded fixed-size peak envelope through the FFmpeg sidecar; browser code never decodes the full recording                                                                                                                              |
| `system_capabilities_cmd`, `model_catalog_cmd`, `download_model_cmd`, `delete_model_cmd` | Model manager; downloads are size/hash verified, textual payloads rejected, and only app-data models are removable; app-data `models/` overrides bundled resources                                                                                 |
| `detect_cat_software_cmd`, `scan_cat_jobs_cmd`                                           | Recursive filesystem work runs on the blocking pool; depth clamped 1–20 (default 5)                                                                                                                                                                |
| `detect_sync_cmd`, `merge_audio_cmd`                                                     | Merge tab; completed output is returned with a warning if temporary playback authorization fails                                                                                                                                                   |
| `library_get` ★                                                                          | Returns the startup-loaded in-memory library or a durable-load error; corrupt storage disables mutations instead of appearing empty                                                                                                                |
| `library_rename_case`, `library_archive_case`                                            | Persist a candidate atomically before publishing it in memory; return whether a matching case changed                                                                                                                                              |
| `library_delete_case`, `library_delete_session`                                          | Return `{changed, warning}` so a committed deletion is not misreported as failed if ephemeral playback revocation fails                                                                                                                            |
| `library_import_file`, `library_import_files` ★                                          | Canonical, playable local audio paths only; multi-file validation and persistence are atomic; case name sanitized (≤200 chars), label ≤100 chars, and an archived target case is re-activated. Returns `{changed, warning}`.                       |
| `prefs_get` / `prefs_set` ★                                                              | Patch-merge on known top-level camelCase keys; unknown keys, non-object patches, and type-invalid updates fail explicitly without changing disk or memory                                                                                          |
| `show_in_folder`                                                                         | Reveals path via opener plugin                                                                                                                                                                                                                     |

## On-device state (must survive upgrades)

| State            | Location                          | Shape                                                                                                                        |
| ---------------- | --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| Preferences ★    | app-data `prefs.json`             | `Prefs` camelCase; all post-v0.6 fields have serde defaults so older files still load (locked in types tests); atomic writes |
| Case library ★   | app-data `library.json`           | `{version, cases[]}` camelCase (locked in types tests); atomic writes                                                        |
| Transcripts ★    | localStorage `transcript:${path}` | Segment array; corrupt storage tolerated (locked in transcript tests)                                                        |
| Bookmarks ★      | localStorage `player-bookmarks`   | `{time, trackPath, label?, color?}[]`; shape-validated on load                                                               |
| Playback speed ★ | localStorage `player-speed`       | Must be on the speed menu, else 1×                                                                                           |
| AI models        | app-data `models/`                | User-downloaded ONNX files; checked before bundled resources                                                                 |

## Processing contracts (audio output)

These define what converted files sound like. Changing them changes output
for identical inputs — never do it silently.

- ★ Filter order: de-clip → high-pass → auto-level gain → loudnorm → source-relative fade-out → leading trim → fade-in (`proc_filters` tests).
- ★ Stereo mode: unity-gain SUM of all channels on both L/R via `pan=stereo` + `volume=N` compensation; `alimiter=limit=0.97` appended when normalize is off (peak guard for correlated content).
- ★ Split mode: `asplit` + `pan=mono|c0=cN` per channel (NOT `channelsplit`, which assumes a stereo layout); auto-level injects each channel's own gain right after isolation; near-unity gains (±0.01) skipped.
- ★ Keep Original mode rejects auto-leveling at the backend boundary; one shared gain cannot perform the documented per-channel balancing while preserving the channel layout.
- ★ MP3 bitrates limited to 128/192/320 kbps, off-menu values fall back to 192.
- Opus always encodes 64k VBR at 48 kHz; m4a AAC 128k.
- SGMCA: bytes before the first `OggS` magic in the supported 8 KiB header region are stripped before FFmpeg sees the file; whole-file preparation runs on the blocking pool and checks cancellation every 64 KiB.
- FTR/TRM: original court recordings explicitly use FFmpeg's native `-c:a ftr` decoder; after AI processing, the feed is already DepoAudio-generated PCM WAV and needs no FTR override.
- FlashSR: 16 kHz input is processed in at most one-second frames with 500-sample input overlap and a normalized linear crossfade, bounding the interval between conversion cancellation checks.
- Analysis sampling caps: scan/auto-level 180 s, speakers 60 s, DNSMOS 12 s decode; every sidecar call has a timeout and a kill path (the fix for the scan-hang bug, PR #52).

## Release / update channel

- GitHub Actions `release.yml` builds universal macOS and Windows installers sequentially, stages pinned ONNX Runtime and FFmpeg assets, smoke-tests real FTR decoding, and creates a draft release for manual publication.
- Release builds fail closed unless the configured updater private/public keys cryptographically match and the selected platform's code-signing credentials are present. Updater key/endpoint configuration is generated only inside the release job; local builds contain neither placeholders nor a live update endpoint.
- Existing published asset names and bytes are immutable. Promotion resolves the draft, published release target, and tag to the same commit and may add only non-colliding assets; it never merges, replaces, or clobbers `latest.json`. Publishing while a later platform is still building normally creates a `latest.json` collision, so finalization intentionally fails closed and retains the stray draft for manual reconciliation.

## Test index

| Suite                          | Command                                           | Locks                                                                                                                                                                                   |
| ------------------------------ | ------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Rust unit + characterization   | `cargo test --manifest-path src-tauri/Cargo.toml` | Filter chains, filtergraphs, prefs merge, library filing, sanitization, wire shapes, format registry, case-name inference, mel spectrogram (numpy-pinned), merge helpers, safety guards |
| JS characterization            | `npm test`                                        | Conversion/cancellation state, queue ordering, preferences/theme, transcript persistence and errors, player/waveform logic, utilities, and constants                                    |
| ORT smoke (ignored by default) | `cargo test -- --ignored ort_loads_and_runs`      | Silero VAD actually runs via the bundled dylib (needs desktop env)                                                                                                                      |
