# Changelog

## [Unreleased]

### Added

- **The synced transcript editor is available in the Player again.** Import SRT, VTT, or TXT, follow and edit cues against the playhead, stamp new lines, and export the finished transcript locally.

### Fixed

- **Bundled ONNX Runtime and the Rust binding now use the same 1.22 C API.** The app no longer requests an API newer than its signed desktop runtime, macOS-specific CoreML code compiles in CI, and release builds run real Silero VAD inference against the staged native library before packaging.
- **TRM/FTR decoding now uses FFmpeg's native `ftr` decoder** instead of forcing ordinary AAC. The same verified decoder contract is used by scan, conversion, channel splitting, scoring, speaker detection, VAD, and AI preprocessing; release builds smoke-test a real four-channel court recording before packaging.
- **FTR sessions keep every recording chunk** when imported from court-software detection. Detection and enumeration now share one recursive depth, canonical chunks are grouped into chronological session runs, every result is reachable through pagination, and the combined conversion queue remains deduplicated and ordered across separate drops.
- **Silence trimming preserves the evidentiary timeline.** It no longer removes pauses from the middle of testimony, and split-channel exports retain their common start time.
- **Library and preference persistence fail closed.** Corrupt or unreadable JSON is surfaced instead of becoming an empty library, and mutations are saved before in-memory/UI state is reported as successful.
- **Concurrent conversions reserve output names atomically**, preventing two jobs from overwriting or deleting each other's files.
- **Long-running media work is bounded and cleaned up:** FFmpeg timeouts kill and drain child processes, SGMCA preparation runs off the async executor with copy-time cancellation, merge inputs and memory are capped, and normal Tauri shutdown runs temporary-file cleanup.
- **Conversion can be cancelled end to end.** Queued jobs stop, active FFmpeg and AI work observes cancellation, FlashSR runs in bounded overlapping frames, partial outputs are removed, and completed audio remains available even if library filing reports a warning.
- **AI processing has explicit resource limits.** Requests are validated before work starts, channel counts are capped, peak PCM memory is bounded, auto-level uses a lightweight streaming analysis, and the backend rejects auto-level in channel-preserving mode instead of applying a misleading shared gain.
- **Player failures are visible.** Proprietary court formats require conversion before WebView playback, waveform work is cached, deduplicated, independently cancellable, and never spins forever on errors, and long recordings skip unsafe full-browser decoding.
- **Model management distinguishes bundled resources from removable downloads**, so the settings UI no longer offers an impossible Delete action for read-only packaged models.
- **Multi-file library imports are transactional**, preventing partial imports and duplicate sessions when one selected file fails validation or persistence.
- **Settings, queue, scan aggregation, playback state, Canvas colors, and keyboard/screen-reader controls** now reflect the operation actually being performed.

### Security

- Model deletion and Merge now validate catalog entries, local source paths, output names, and destination containment to block traversal, arbitrary deletion, and FFmpeg URL/device input.
- The WebView now has a restrictive CSP, no frontend shell execution permission, a narrowly scoped external URL opener, and persisted access only to paths selected by the user instead of a static whole-disk asset scope.
- Release executables are downloaded through immutable GitHub asset IDs, checked for expected size/hash where available, and rejected unless updater plus platform code-signing credentials are configured and the updater key pair cryptographically matches. The Windows FFmpeg pin now targets a retained month-end build instead of BtbN's short-lived rolling asset. Published assets are never replaced or uploaded with `--clobber`.
- Frontend dependency advisories were resolved without force-upgrading major versions.

### Changed

- React/ReactDOM and Radix UI primitives are refreshed as coordinated sets, with Dependabot grouping that prevents incompatible version skew; current Rust patch releases are also locked and validated at the Rust 1.88 support boundary.
- Frontend dialog bindings now match the Rust plugin, test helpers are updated without raising the documented Node baseline, and CI's Rust cache action is pinned to the verified v2.9.2 commit.
- Vite, its React plugin, Tauri's JavaScript API/CLI, Lucide icons, and the remaining build/lint/CSS tools are refreshed as compatible sets; the existing Node 22.12 minimum is now explicit in package metadata and developer setup.
- DeepFilterNet "Best" processing is disabled until its complete spectral pipeline is implemented. Requested denoise, dereverb, and enhancement stages now fail clearly instead of silently falling back or no-oping.
- Windows releases now accurately report the bundled CPU ONNX Runtime; Apple Silicon builds may use CoreML. Hardware tiers no longer claim unavailable Windows DirectML/NPU execution providers.
- Speaker segmentation is now labeled as an active speaker-slot estimate instead of distinct-speaker identification; embedding and cross-window voice clustering are not implemented in this release.
- Auto-update configuration is injected only into signed release builds; local/development builds no longer carry a placeholder updater key or stale repository endpoint.

## [1.0.1] - 2026-07-17

### Fixed

- **Scan no longer stalls for minutes on "Detecting speech"** — the AI models were being compiled for the hardware accelerator (Apple Neural Engine / DirectML) from scratch for every pass of every file, and the voice-activity model's dynamic shapes made that compilation hang outright. The two dynamic-shape models now run on the CPU (they're tiny — milliseconds per inference), and loaded models are cached for the app's lifetime, which also makes multi-file scans significantly faster.
- **Conversion progress shows a real percentage** — the bar was an indeterminate full-width pulse that read as "stuck at 100%" while work continued. It now shows actual encode progress ("Encoding… 43%"), capped at 99% until the file is truly finished; AI cleanup phases keep the activity animation.
- **The guided steps behave** — "Add recording → Choose settings → Convert" now checks all three steps when the current queue finishes, and resets when you add a new file instead of inheriting the previous conversion's checkmark.

## [1.0.0] - 2026-07-16

The first public release. A desktop audio converter and enhancer for court reporters — 100% local.

### Features

- **Conversion** — court formats (SGMCA, FTR/TRM, BWF, DigitalCAT) plus standard audio and video, out to WAV/MP3/FLAC/Opus/M4A; three output modes (mix to stereo, keep original, split by channel) with per-channel labels and volume; batch processing and 5 presets.
- **Smart Cleanup (on-device AI)** — one-click Scan recommends denoise, auto-level, de-clip, and clarity fixes; turn detection, speaker count, and 1–5 quality scoring (DNSMOS); hardware-accelerated (Apple Neural Engine, Ryzen AI, Intel AI Boost, or GPU) with live, cancellable progress.
- **Player** — color-coded speakers, 0.5×–2× speed, A-B loop, and editable bookmarks.
- **Library & detection** — case library auto-filed by case and participant (search, archive, re-export); court-software detection (Case CATalyst, FTR Gold, Eclipse, DigitalCAT, CourtSmart) with direct job import.
- **Platform** — universal macOS (Apple Silicon + Intel) and Windows; signed auto-updates from GitHub Releases; dark & light themes (WCAG 2.2 AA); no cloud, no accounts, no subscription.

### Added

- **Live scan progress** — the Convert-tab Scan streams progress for every analysis phase (loudness, noise floor, speech, speaker turns, quality, speakers), the bar advances _within_ each file with a phase label — including heartbeats while a slow FFmpeg pass drains its timeout, so "slow but alive" looks different from "stuck" — and a Cancel button actually stops the backend compute.
- **FTR session chunks auto-order chronologically** — dropping or browsing a set of `.trm`/`.ftr` chunks queues them in recording order (parsed from FTR's filename timestamp, verified against real court-produced files) regardless of how the OS delivered them. Applies to the Convert queue and the Player playlist (newly added batches only — manual reordering is preserved). Mixed selections keep the order you chose.
- **Universal macOS build** — one `.app`/`.dmg` runs natively on both Apple Silicon and Intel. FFmpeg sidecars and the ONNX Runtime library are combined for both architectures; no more separate Intel/Apple-Silicon downloads.
- **Auto-update from GitHub Releases** — on launch the app checks for a newer **signed** release; when one exists a banner offers "Update & restart", which downloads, verifies, installs, and relaunches into the new version. A manual "Check for updates" lives in Settings → Software Update. Updates are cryptographically signed (minisign) and verified against the bundled public key before install.
- **Player keyboard transport** — Space/K play-pause, ←/→ seek ±5s, J/L seek ±10s, ↑/↓ change speed, [ / ] previous/next track, B add bookmark (ignored while typing in a field).
- **Playback speed** — 0.5×–2× control in the player, persists across sessions (essential for transcription).
- **A-B loop** — set in/out points and repeat a passage for re-listening.
- **Bookmark notes & export** — bookmark labels are now editable (e.g. "Objection", "Exhibit 4") and the whole list copies to the clipboard as timestamped lines for a transcript.

### Changed

- **DepoStack brand** — full visual rebrand: plum + gold on warm cream (light) and a deep plum-night (dark). Plum is the primary ink, gold the accent and call-to-action (gold buttons with plum ink), with generously rounded cards and soft shadows. Every color lives in `design/tokens.json`; light-mode status colors are the brand hues tuned to stay legible. Both themes remain **WCAG 2.2 AA** (axe-verified, 0 violations across every screen).
- **"Docket" redesign (part 1)** — the app shell and Convert flow, rebuilt:
  - **Sidebar navigation** replaces the top tab bar: icons + labels, number-key shortcuts (1–4), the case-library count, and a live system-health card (FFmpeg status, installed AI models, update state) with Settings and theme at the bottom. Collapses to an icon rail on narrow windows.
  - **Guided steps on Convert** — a state-driven "Add recording → Choose settings → Convert" stepper shows where you are without hiding anything; the whole page still works at once for batch users.
  - **Format tiles** with plain-English trade-offs replace the small format buttons; sample rate and MP3 bitrate live alongside them.
  - **Output mode** is now a segmented control; scan findings appear as green "Recommended" pills on the matching enhancement toggles; the action bar summarizes what's about to happen ("Ready: MP3 · mix to stereo with 2 enhancements → same folder as source").
  - **Light theme retuned** to cool neutrals with white cards and soft shadows (dark theme keeps its ink palette with the new structure); corner radius increased app-wide. All via design tokens; both themes remain WCAG 2.2 AA (axe-verified, 0 violations).

### Fixed

- **Scans finish now — every prior release could hang or freeze** — one fix in three layers. Analysis reads a bounded sample of each file instead of the whole recording (a multi-hour multichannel deposition previously ran tens of thousands of ONNX inferences and effectively never completed), and every analysis FFmpeg pass has a timeout backstop whose expired processes are killed rather than orphaned at full CPU. AI inference moved off the async runtime onto the blocking pool with cancellation checks and wall-clock budgets, so queued scans can no longer freeze the entire app. And a stalled file is skipped after 150 seconds of silence instead of poisoning the whole scan, with failures reported ("2 of 3 files couldn't be analyzed") instead of silently reverting to the idle hint.
- **Scanning FTR (.trm) files always failed** — analysis passes never applied the forced FTR decoder that conversion has always used, so every Scan of a `.trm` file failed or timed out. Scan decodes now handle FTR's proprietary codec, and files that genuinely can't be decoded short-circuit the remaining passes with an honest "convert it first" recommendation. A channel-probe failure also no longer invents four phantom channels of extra work.
- **Model downloads restored** — the `models-v1` release hosting all nine downloadable AI models was deleted during a repository cleanup, so every in-app model download returned 404; it was rebuilt with each asset verified against the app's SHA-256 pins. The real DNSMOS quality-scoring model is now published and integrity-pinned like the rest (the previously committed file was an HTML error page), and its catalog size corrected (0.3 → 1.1 MB).
- **Auto-level gain safety** — conversion only applies per-channel auto-level gains when the analysis measured the same channel count the converter sees, preventing a desynced analysis from boosting some channels and silencing others.
- **App now closes immediately** — closing the window quits the process directly, so an in-flight scan or conversion can no longer leave the app stuck on exit.

### Improved

- **Responsive layout** — the UI now scales to the window instead of sitting in a fixed 920px column: content fluidly uses available width (up to a comfortable 1100px for readability) and reflows cleanly down to a 720px minimum, eliminating horizontal scrolling. Default window enlarged to 1160×820 for more breathing room.
- **Library tab** no longer permits horizontal scrolling (added the same overflow guard the other tabs already had).
- **Accessibility (WCAG 2.2 AA)** — audited every screen in both themes with axe-core and fixed all violations: hint/muted text and gold link/tab colors now meet the 4.5:1 contrast threshold on every surface, primary buttons in light mode use a compliant label color, and all selects (sample rate, denoise quality, import case, settings) have accessible names for screen readers.
- **Theme is now token-driven** — all colors live in `design/tokens.json` (W3C design-token format) as primitive→semantic layers; `npm run tokens` regenerates the CSS, and CI fails if the two drift apart.

### Internal

- **Characterization test suite** — 70 new golden-master tests pin the behavior users depend on: FFmpeg filter chains and filtergraphs per output mode, preference patch-merge semantics, library filing rules, import sanitization, IPC wire shapes and defaults, transcript SRT/plain parsing round-trips, and player logic. `PARITY.md` documents the full capability/contract inventory.
- **CI workflow** — every PR now runs eslint, the JS and Rust test suites, a Vite build, `cargo clippy -D warnings` (codebase is warning-clean), and the token-drift check.
