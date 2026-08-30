# DepoAudio Parity Contract

Baseline: `main @ 0047549` (v0.8.0 lineage, 2026-07-02).

This document is the inventory of behavior that installed users depend on.
Any refactor, redesign, or rebuild must preserve everything listed here unless
a change is deliberate and called out in the changelog. The characterization
test suite (see [Test index](#test-index)) mechanically enforces the starred
(★) items. Release automation separately verifies native FTR decoding, zero
learned-model distribution, and bounded packaged application startup. Every
other capability requires the manual installed-app checks in
[`docs/V1.0.3-RELEASE-CANDIDATE.md`](docs/V1.0.3-RELEASE-CANDIDATE.md); a green
automated smoke test is not evidence that the complete capability map was
exercised.

## Capability map (user-facing)

| Area               | Capabilities                                                                                                                                                                                                                                                                                                                                          |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Convert            | Single/batch conversion; output modes `stereo` (downmix), `keep` (original channels), `split` (one file per channel); formats wav/mp3 (128/192/320k)/flac/opus/m4a; sample-rate choice; presets; per-channel labels & volumes                                                                                                                         |
| Processing         | Normalize (loudnorm), leading-silence trim, fade in/out, high-pass filter, de-clip, and bounded per-channel auto-level analysis. Learned denoise, enhancement, and de-reverb compatibility fields are forced off and rejected at the backend boundary.                                                                                          |
| Scan               | Non-learned loudness and peak observations per channel, clipping, balance, and sample-rate/narrowband guidance, bounded to sample caps and subprocess timeouts. Neural turn, speech, speaker-slot, and quality metrics are returned empty.                                                                                                     |
| Formats            | Court-reporting: SGMCA (header strip), FTR/TRM (native `ftr` decoder), BWF, DigitalCAT (experimental); rejected with guidance: Eclipse AES, Liberty DCR; video containers (audio extracted); phone formats (AMR/3GA), CAF                                                                                                                             |
| Merge              | Source is retained, but the feature is hidden from the current UI and is not a supported release workflow; readiness remains tracked in issue #90                                                                                                                                                                                                     |
| Library            | Cases → sessions → participants → files; rename/archive/delete; import with case-name sanitization; auto-filing after conversion with case-name inference from filenames                                                                                                                                                                              |
| Player             | Playlist (drop/browse, dedupe; proprietary SGMCA/TRM/FTR inputs are routed to Convert first); transport with keyboard map (Space/K, ←/→ ±5s, J/L ±10s, ↑/↓ speed, [/] track, B bookmark); speed steps 0.5–2×; A-B loop; persistent bookmarks + clipboard export; synced transcript editor (import SRT/VTT/TXT, stamp, proof, export SRT/TXT)          |
| CAT detection      | Detect installed CAT software; scan job folders                                                                                                                                                                                                                                                                                                       |
| Settings           | Theme (system/light/dark), startup defaults ("remember last used" sentinel = empty string), non-learned processing parameters, deletion-only legacy model-file cleanup, software update section                                                                                                                                                        |
| Updates            | Tauri updater against GitHub Releases `latest.json` when updater-signing keys are configured; otherwise the updater is dormant. Platform signing is independent and must be verified from the published artifacts                                                                                                                                     |

## IPC surface

The de-facto API between the React frontend and the Rust core. Names,
argument shapes (camelCase), and return shapes are all contract.

| Command                                                                                  | Contract notes                                                                                                                                                                                                                                     |
| ---------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `health_check`                                                                           | Executes both sidecars with `-version`, verifies the exact native FTR decoder, and returns `{ffmpeg, ffprobe, ftrDecoder}`                                                                                                                        |
| `get_formats_list` / `detect_format` ★                                                   | Format registry incl. status/handler/notes; detection by extension (see helpers tests)                                                                                                                                                             |
| `infer_case_name_cmd` ★                                                                  | Strips date patterns, underscores → spaces                                                                                                                                                                                                         |
| `begin_conversion_batch_cmd`, `cancel_conversion_cmd`, `convert` ★                       | Generation-based cancellation is serialized with the final output/library commit. `convert` emits progress, done (including warnings), cancelled, or error events; event-delivery failures reject instead of leaving the frontend waiting forever. |
| `analyze_audio_cmd`, `cancel_scan_cmd`                                                   | `AnalysisResult` (camelCase); analysis is cancellable and bounded to a 180 s sample (`ANALYSIS_SAMPLE_SECS`)                                                                                                                                       |
| `waveform_peaks_cmd`                                                                     | Produces a bounded fixed-size peak envelope through the FFmpeg sidecar; browser code never decodes the full recording                                                                                                                              |
| `system_capabilities_cmd`                                                                | Reports conservative local CPU/system information and learned-processing availability as false                                                                                                                                                    |
| `legacy_model_cleanup_catalog_cmd`, `delete_legacy_model_cmd`                            | Lists and deletes only exact allowlisted regular legacy files in app-data; there is no download, install, resolution, or execution path                                                                                                            |
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
| Legacy model files | app-data `models/`              | Never loaded or advertised; exact allowlisted regular files can be shown only for user-initiated deletion                     |

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
- FTR/TRM: original court recordings explicitly use FFmpeg's native `-c:a ftr` decoder.
- Learned processing flags are compatibility-only wire fields. Any request with `denoise`, `enhance`, or `dereverb` set fails before file preparation or decode.
- Analysis and auto-level use at most a 180-second representative sample; every sidecar call has a timeout and a kill path (the fix for the scan-hang bug, PR #52).

## Release / update channel

- GitHub Actions `release.yml` creates one commit-bound private draft, builds universal macOS and Windows installers sequentially, stages reviewed FFmpeg inputs, proves zero learned-model/ONNX Runtime payloads, generates and normally decodes a transient synthetic 440 Hz FTR-tagged fixture with the exact candidate sidecars, and uploads only through an exact-draft verifier. `scripts/ftr-smoke-fixture.mjs` keeps the fixture and derived outputs under the runner's temporary directory and removes them before packaging; artifact inventories still reject every `.trm` file. Tauri's action is build-only and has no release mutation credentials.
- Updater artifacts are generated only when the configured private/public keys cryptographically match. Platform signing is enabled independently when that platform's credentials are present; otherwise the workflow produces an ad-hoc-signed macOS app or unsigned Windows installers. Updater key/endpoint configuration is generated only inside the release job; local builds contain neither placeholders nor a live update endpoint.
- Existing published asset names and bytes are immutable. The candidate workflow rejects a same-tag draft instead of deleting it, never modifies a published release, and finalizes only by auditing the complete private draft. It deliberately rejects `latest.json`; updater metadata and publication require a separate approval gate bound to the exact release ID, source commit, assets, signing decision, and release date.
- `publish-release.yml` separates read-only candidate-manifest inspection from protected publication. Its publication path checks out the immutable approval commit, re-downloads and verifies the complete candidate and evidence inventory, cryptographically validates signed updater artifacts, records both source and approval commits, binds the release tag, and publishes only the approved release ID. The `release-publication` environment's reviewer and prevent-self-review settings must be verified in GitHub because repository source cannot prove dashboard policy.

## Test index

| Suite                          | Command                                           | Locks                                                                                                                                                                                   |
| ------------------------------ | ------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Rust unit + characterization   | `cargo test --manifest-path src-tauri/Cargo.toml` | Filter chains, filtergraphs, learned-flag rejection, prefs migration, library filing, sanitization, wire shapes, format registry, case-name inference, merge helpers, safety guards |
| JS characterization            | `npm test`                                        | Conversion/cancellation state, queue ordering, preferences/theme, transcript persistence and errors, player/waveform logic, utilities, and constants                                    |
| Distribution contract          | `npm run models:check`                            | No learned weights, known model hashes, ONNX Runtime graph entry, model download path, or compiled learned module can enter v1.0.3                                                      |
