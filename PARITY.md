# DepoAudio Parity Contract

Baseline: `main @ 0047549` (v0.8.0 lineage, 2026-07-02).

This document is the inventory of behavior that installed users depend on.
Any refactor, redesign, or rebuild must preserve everything listed here unless
a change is deliberate and called out in the changelog. The characterization
test suite (see [Test index](#test-index)) mechanically enforces the starred
(★) items; the rest are verified by the release smoke test.

## Capability map (user-facing)

| Area | Capabilities |
|---|---|
| Convert | Single/batch conversion; output modes `stereo` (downmix), `keep` (original channels), `split` (one file per channel); formats wav/mp3 (128/192/320k)/flac/opus/m4a; sample-rate choice; presets; per-channel labels & volumes |
| Processing | Normalize (loudnorm), trim silence, fade in/out, high-pass filter, de-clip; AI clarity chain: denoise (RNNoise fast / DFN3 best with RNNoise fallback), dereverb, bandwidth extension (FlashSR), auto-level from analysis |
| Scan (AI analysis) | Loudness/peak per channel, clipping, narrowband detection, turn detection (Smart-Turn), VAD speech ratio (Silero), speaker count (pyannote segmentation), DNSMOS quality score — all bounded to sample caps + subprocess timeouts |
| Formats | Court-reporting: SGMCA (header strip), FTR/TRM (AAC codec force), BWF, DigitalCAT (experimental); rejected with guidance: Eclipse AES, Liberty DCR; video containers (audio extracted); phone formats (AMR/3GA), CAF |
| Merge | Two-source sync detection + merge |
| Library | Cases → sessions → participants → files; rename/archive/delete; import with case-name sanitization; auto-filing after conversion with case-name inference from filenames |
| Player | Playlist (drop/browse, dedupe, court formats accepted); transport with keyboard map (Space/K, ←/→ ±5s, J/L ±10s, ↑/↓ speed, [/] track, B bookmark); speed steps 0.5–2×; A-B loop; persistent bookmarks + clipboard export; synced transcript editor (import SRT/VTT/TXT, stamp, proof, export SRT/TXT) |
| CAT detection | Detect installed CAT software; scan job folders |
| Settings | Theme (system/light/dark), startup defaults ("remember last used" sentinel = empty string), processing parameters, model manager (download/delete, hash-verified), software update section |
| Updates | Tauri updater against GitHub Releases `latest.json`; signed artifacts; dormant until a real pubkey + signing secrets exist |

## IPC surface (26 commands)

The de-facto API between the React frontend and the Rust core. Names,
argument shapes (camelCase), and return shapes are all contract.

| Command | Contract notes |
|---|---|
| `health_check` | Executes both sidecars with `-version`; returns `{ffmpeg, ffprobe, models, accelerator, tier}` |
| `get_formats_list` / `detect_format` ★ | Format registry incl. status/handler/notes; detection by extension (see helpers tests) |
| `infer_case_name_cmd` ★ | Strips date patterns, underscores → spaces |
| `convert` ★ | Takes `ConvertJob` (defaults locked in types tests); emits `convert:progress` (`{id, seconds, phase?}`), then `convert:done` (`{id, files}`) or `convert:error` (`{id, message}`); never rejects the invoke itself |
| `analyze_audio_cmd` | `AnalysisResult` (camelCase); analysis bounded to 180 s sample (`ANALYSIS_SAMPLE_SECS`) |
| `score_quality_cmd` | DNSMOS SIG/BAK/OVR 1–5, first ~9 s |
| `detect_speakers_cmd` | Pyannote powerset argmax; first 60 s; count ≥ 1 |
| `detect_speech_cmd` | Silero VAD segments, 300 ms merge gap, 0.5 threshold |
| `system_capabilities_cmd`, `model_catalog_cmd`, `download_model_cmd`, `delete_model_cmd` | Model manager; downloads hash-verified, textual payloads rejected; app-data `models/` overrides bundled resources |
| `detect_cat_software_cmd`, `scan_cat_jobs_cmd` | Depth clamped 1–20 (default 5) |
| `detect_sync_cmd`, `merge_audio_cmd` | Merge tab |
| `library_get` ★ | Loads from disk only when in-memory state is empty |
| `library_rename_case`, `library_archive_case`, `library_delete_case`, `library_delete_session` | Return `bool` (found && saved) |
| `library_import_file` ★ | Case name sanitized (≤200 chars, separators/control chars stripped); label ≤100 chars; importing into an archived case re-activates it |
| `prefs_get` / `prefs_set` ★ | Patch-merge on top-level camelCase keys; a patch that breaks deserialization is dropped wholesale |
| `show_in_folder` | Reveals path via opener plugin |

## On-device state (must survive upgrades)

| State | Location | Shape |
|---|---|---|
| Preferences ★ | app-data `prefs.json` | `Prefs` camelCase; all post-v0.6 fields have serde defaults so older files still load (locked in types tests); atomic writes |
| Case library ★ | app-data `library.json` | `{version, cases[]}` camelCase (locked in types tests); atomic writes |
| Transcripts ★ | localStorage `transcript:${path}` | Segment array; corrupt storage tolerated (locked in transcript tests) |
| Bookmarks ★ | localStorage `player-bookmarks` | `{time, trackPath, label?, color?}[]`; shape-validated on load |
| Playback speed ★ | localStorage `player-speed` | Must be on the speed menu, else 1× |
| AI models | app-data `models/` | User-downloaded ONNX files; checked before bundled resources |

## Processing contracts (audio output)

These define what converted files sound like. Changing them changes output
for identical inputs — never do it silently.

- ★ Filter order: de-clip → high-pass → auto-level gain → loudnorm → trim → fades (`proc_filters` tests).
- ★ Stereo mode: unity-gain SUM of all channels on both L/R via `pan=stereo` + `volume=N` compensation; `alimiter=limit=0.97` appended when normalize is off (peak guard for correlated content).
- ★ Split mode: `asplit` + `pan=mono|c0=cN` per channel (NOT `channelsplit`, which assumes a stereo layout); auto-level injects each channel's own gain right after isolation; near-unity gains (±0.01) skipped.
- ★ MP3 bitrates limited to 128/192/320 kbps, off-menu values fall back to 192.
- Opus always encodes 64k VBR at 48 kHz; m4a AAC 128k.
- SGMCA: bytes before the first `OggS` magic are stripped before FFmpeg sees the file.
- FTR/TRM: input forced `-acodec aac` — except after AI processing, when the feed is already our own PCM WAV.
- Analysis sampling caps: scan/auto-level 180 s, speakers 60 s, DNSMOS 12 s decode; every sidecar call has a timeout and a kill path (the fix for the scan-hang bug, PR #52).

## Release / update channel

- GitHub Actions `release.yml`: builds Win/macOS/Linux via tauri-action, ONNX Runtime 1.22.0 fetched per-platform, draft releases (publish is manual).
- Updater artifacts + `latest.json` only produced when `TAURI_SIGNING_PRIVATE_KEY` secret exists (detect step); config merged from `tauri.updater.conf.json`.
- `plugins.updater.pubkey` is a placeholder until the maintainer generates a keypair (README "Enabling auto-updates").

## Test index

| Suite | Command | Locks |
|---|---|---|
| Rust unit + characterization | `cargo test --manifest-path src-tauri/Cargo.toml` | Filter chains, filtergraphs, prefs merge, library filing, sanitization, wire shapes, format registry, case-name inference, mel spectrogram (numpy-pinned), merge helpers, safety guards |
| JS characterization | `npm test` | Transcript parse/serialize round-trips, player logic, utils, constants |
| ORT smoke (ignored by default) | `cargo test -- --ignored ort_loads_and_runs` | Silero VAD actually runs via the bundled dylib (needs desktop env) |
