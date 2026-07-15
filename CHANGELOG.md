# Changelog

## [Unreleased]

## [1.1.0] - 2026-07-15

### Fixed
- **Scan progress works and scans finish** — the Convert-tab Scan previously showed a frozen bar and could grind for tens of minutes with nothing to show. The backend now streams progress for every analysis phase (with heartbeats while a slow FFmpeg pass drains its timeout), the bar advances *within* each file with a phase label, and a Cancel button actually stops the backend compute. A stalled file is skipped after 150 seconds of silence instead of poisoning the whole scan, and failures are reported ("2 of 3 files couldn't be analyzed") instead of silently reverting to the idle hint.
- **FTR (.trm) files can now be scanned** — analysis passes never applied the forced FTR decoder the converter has always used, so every Scan of a `.trm` file failed or timed out. Scan decodes now handle FTR's proprietary codec, and files that genuinely can't be decoded short-circuit the remaining passes with an honest "convert it first" recommendation instead of burning minutes of timeouts.
- **Scans no longer freeze the app** — AI inference used to run on the async runtime's worker threads; a few queued scans could starve the whole backend (timers, other commands). Inference now runs on the blocking pool with cancellation checks and wall-clock budgets, timed-out FFmpeg processes are killed instead of orphaned at full CPU, and a channel-probe failure no longer invents four phantom channels of extra work.
- **Auto-level gain safety** — conversion only applies per-channel auto-level gains when the analysis measured the same channel count the converter sees, preventing a desynced analysis from boosting some channels and silencing others.
- **Model downloads restored** — the `models-v1` release hosting the AI models was rebuilt after the repo cleanup deleted it (all nine assets verified against the app's SHA-256 pins), the real DNSMOS model is pinned (the previously committed file was an HTML error page), and its catalog size corrected.

### Added
- **FTR session chunks auto-order chronologically** — dropping or browsing a set of `.trm`/`.ftr` chunks now queues them in recording order (parsed from FTR's filename timestamp, verified against real court-produced files) regardless of how the OS delivered them. Applies to the Convert queue, Merge sources (the earliest chunk becomes the sync reference), and the Player playlist (newly added batches only — manual reordering is preserved). Mixed selections keep the order you chose.

## [1.0.0] - 2026-07-04

### Changed — DepoStack brand
- **Full visual rebrand to the DepoStack identity** — plum + gold on warm cream (light) and a deep plum-night (dark). Plum is the primary ink, gold the accent and call-to-action (gold buttons with plum ink), with generously rounded cards and soft shadows. Every color lives in `design/tokens.json`; light-mode status colors are the brand hues tuned to stay legible. Both themes remain **WCAG 2.2 AA** (axe-verified, 0 violations across every screen).

### Added — Universal macOS build
- **macOS now ships as a single universal binary** — one `.app`/`.dmg` runs natively on both Apple Silicon and Intel. FFmpeg sidecars and the ONNX Runtime library are combined for both architectures; no more separate Intel/Apple-Silicon downloads.

### Changed — "Docket" redesign (part 1)
- **Sidebar navigation** replaces the top tab bar: icons + labels, number-key shortcuts (1–4), the case-library count, and a live system-health card (FFmpeg status, installed AI models, update state) with Settings and theme at the bottom. Collapses to an icon rail on narrow windows.
- **Guided steps on Convert** — a state-driven "Add recording → Choose settings → Convert" stepper shows where you are without hiding anything; the whole page still works at once for batch users.
- **Format tiles** with plain-English trade-offs replace the small format buttons; sample rate and MP3 bitrate live alongside them.
- **Output mode** is now a segmented control; scan findings appear as green "Recommended" pills on the matching enhancement toggles; the action bar summarizes what's about to happen ("Ready: MP3 · mix to stereo with 2 enhancements → same folder as source").
- **Light theme retuned** to cool neutrals with white cards and soft shadows (dark theme keeps its ink palette with the new structure); corner radius increased app-wide. All via design tokens; both themes remain WCAG 2.2 AA (axe-verified, 0 violations).

### Fixed
- **AI feature scan no longer hangs on long recordings** — analysis (loudness, noise floor, VAD, Smart-Turn, speaker count, quality) now reads a bounded sample of each file instead of the whole thing, so the Scan finishes in seconds-to-minutes regardless of length (a multi-hour multichannel deposition previously ran tens of thousands of ONNX inferences and effectively never completed). Every analysis FFmpeg pass also has a timeout backstop.
- **App now closes immediately** — closing the window quits the process directly, so an in-flight scan or conversion (synchronous ONNX inference + ffmpeg subprocesses) can no longer leave the app stuck on exit.

### Added
- **Auto-update from GitHub Releases** — on launch the app checks for a newer **signed** release; when one exists a banner offers "Update & restart", which downloads, verifies, installs, and relaunches into the new version. A manual "Check for updates" lives in Settings → Software Update. Updates are cryptographically signed (minisign) and verified against the bundled public key before install.
- **Synced transcript editor** — proof an existing transcript against the audio or build one from scratch, right in the Player:
  - Import **SRT, VTT, or TXT** (or paste text); every line is editable inline and autosaves per track.
  - Timed lines **highlight and auto-scroll** as the audio plays ("Follow"); click a line's time to jump there.
  - **Stamp** the current audio position onto a line to anchor plain text, and press **Enter** to start a new line stamped at the playhead (fast capture).
  - Optional per-line **speaker** labels (auto-detected from "SPEAKER: text").
  - **Export** to SRT or TXT, or copy the whole transcript to the clipboard.
- **Player keyboard transport** — Space/K play-pause, ←/→ seek ±5s, J/L seek ±10s, ↑/↓ change speed, [ / ] previous/next track, B add bookmark (ignored while typing in a field).
- **Playback speed** — 0.5×–2× control in the player, persists across sessions (essential for transcription).
- **A-B loop** — set in/out points and repeat a passage for re-listening.
- **Bookmark notes & export** — bookmark labels are now editable (e.g. "Objection", "Exhibit 4") and the whole list copies to the clipboard as timestamped lines for a transcript.

### Improved
- **Responsive layout** — the UI now scales to the window instead of sitting in a fixed 920px column: content fluidly uses available width (up to a comfortable 1100px for readability) and reflows cleanly down to a 720px minimum, eliminating horizontal scrolling. Default window enlarged to 1160×820 for more breathing room.
- **Library tab** no longer permits horizontal scrolling (added the same overflow guard the other tabs already had).
- **Accessibility (WCAG 2.2 AA)** — audited every screen in both themes with axe-core and fixed all violations: hint/muted text and gold link/tab colors now meet the 4.5:1 contrast threshold on every surface, primary buttons in light mode use a compliant label color, and all selects (sample rate, denoise quality, import case, settings) have accessible names for screen readers.
- **Theme is now token-driven** — all colors live in `design/tokens.json` (W3C design-token format) as primitive→semantic layers; `npm run tokens` regenerates the CSS, and CI fails if the two drift apart.

### Internal
- **Characterization test suite** — 70 new golden-master tests pin the behavior users depend on: FFmpeg filter chains and filtergraphs per output mode, preference patch-merge semantics, library filing rules, import sanitization, IPC wire shapes and defaults, transcript SRT/plain parsing round-trips, and player logic. `PARITY.md` documents the full capability/contract inventory.
- **CI workflow** — every PR now runs eslint, the JS and Rust test suites, a Vite build, `cargo clippy -D warnings` (codebase is warning-clean), and the token-drift check.


## [0.8.0] - 2026-06-12

### Added
- **Video file support** — drop MP4, MOV, MKV, AVI, or WebM recordings (Zoom depositions, courtroom video, phone clips) and the audio track is extracted and converted like any other input. Also accepted in Merge.
- **More phone formats** — CAF (Apple Core Audio) and AMR/3GA phone recordings.

### Fixed — AI features (verified against the actual bundled models)
- **Improve Audio Clarity (FlashSR) never ran** — the model was fed the wrong input tensor name, so every inference failed silently. Verified working: clean 3× bandwidth extension.
- **Speaker count detection never ran** — wrong input name, and the model's output (powerset speaker classes) was being misread; it now decodes per-frame argmax over the class set.
- **Smart Turn detection never ran** — the model expects Whisper-style log-mel spectrograms, not raw audio. Implemented the mel frontend (validated numerically against a reference implementation and the real model).
- **Quality scoring (DNSMOS) could never have worked** — the bundled model file was a saved HTML error page, not a model. Removed; the model manager will offer it for download once a real asset is published, and downloads now reject non-model payloads.
- **"Reduce Room Echo" no longer shows for a model that isn't installed** — the DCCRN+ de-reverb model is optional and self-exported; the toggle now appears only when it's present.

### Improved
- **Convert tab leads with the file drop zone** — it was previously at the bottom of the page, below every setting.
- **Readability** — muted hint text now meets WCAG AA contrast in both dark and light themes.
- **Settings dialog close button** — it existed but rendered invisible; it now shows a proper ×.
- **Contextual channels card** — the channel labels/mix card only appears once files are queued, and not in "Keep Original" mode where it has no effect.
- **Player & Merge empty states** — the Player opens with a single focused drop zone; the Merge tab explains its three-step flow (add recordings → auto-sync → one clean file).
- **Active preset highlight** — the Convert tab highlights which preset matches the current settings, and clears it when you diverge.
- **Bookmarks persist** — player bookmarks survive app restarts.
- **Keyboard & screen-reader support** — drop zones are keyboard-operable; icon buttons, toggles, and sliders have accessible names; toggle groups expose pressed state.

### Fixed
- **In-app audio playback restored** — the Tauri asset protocol had been disabled since 0.6.0, so the Player tab, library inline playback, result mini-players, waveforms, and before/after comparison could not load audio in installed builds.
- **Model downloads work on installed apps** — models now download to the user data directory instead of the read-only install directory (Program Files / signed macOS bundle), and the model manager reports their real sizes.
- **Stereo mix no longer hard-clips** — mixing multiple channels that picked up the same voice could exceed full scale by up to 12 dB; a true-peak limiter now guards the mix when loudness normalization is off.
- **"Best quality" denoise falls back honestly** — when the DeepFilterNet3 models can't run, denoising now falls back to RNNoise instead of silently passing audio through unprocessed.
- **Merge crossfades are complete** — source switches in "Best quality" merges ramped only half the window, leaving an audible step; the full crossfade is now applied. Sync confidence threshold recalibrated for the corrected normalization.
- **FTR files with AI processing** — the forced AAC decoder hint now follows the original file into the AI pipeline instead of being wrongly applied to the intermediate WAV.
- **Split/stereo conversion fails loudly on probe errors** — a failed channel probe used to assume 4 channels, which would have produced silent "speaker" files; it now reports an error instead.
- **Library write failures are reported** — imports and library edits now surface disk errors instead of claiming success while the change exists only in memory.
- **Split mode works for mono and 4-channel files** — the FFmpeg `channelsplit` filter defaulted to a stereo layout, so splitting any non-stereo input (including 4-channel SGMCA court recordings) failed. Now uses a layout-agnostic `asplit` + `pan` graph.
- **Merge sync alignment** — the detected sync offset had an inverted sign, misaligning merged tracks by twice the true offset. Sync confidence is now properly normalized so loud unrelated recordings no longer classify as "same event".
- **Per-speaker separation preserved** — de-reverb, bandwidth extension (FlashSR), and DeepFilterNet3 denoising now process each channel independently instead of downmixing everything to mono and replicating it across channels.
- **FTR probing** — ffprobe was being passed an ffmpeg-only `-acodec` option, so duration and channel count probing always failed for FTR files (fade-out was silently skipped; channel count fell back to 4).
- **FFmpeg timeout** — a silently wedged FFmpeg process is now actually killed when the timeout elapses, and the "FFmpeg Timeout" setting is honored (it previously had no effect).
- **Noise detection** — "Background noise detected" was based on overall RMS (including speech), recommending denoising for nearly every recording. Now uses the measured noise floor from astats.
- **Health check** — startup health check now actually runs the FFmpeg/FFprobe binaries instead of only constructing the command.
- **Library saves are atomic** — library.json is written to a temp file and renamed, so a crash mid-write can no longer corrupt or empty the library. Imports now go through the same path.
- **16/24-bit WAV decoding** — integer WAVs fed into the AI pipeline were scaled by the wrong constant (~96 dB attenuation, effectively silence).
- **Player tab drag & drop** — dropping audio onto the Player tab now adds it to the playlist instead of silently queueing it in the Convert tab.
- **Playlist auto-advance** — when a track ends, the next one now plays automatically (stops at end of playlist).
- **Scan with missing VAD model** — scanning without the VAD model installed no longer reports "0% speech" and force-enables Trim Silence.
- **Settings number fields** — values can now be typed normally; previously most keystrokes were rejected (e.g. typing a negative dB value was impossible).
- **Default Output Format/Mode settings** — these now actually work, with an explicit "Remember last used" option (the default, preserving the old behavior). The Settings "Fade Duration" field now edits the live conversion setting directly. "Folder Scan Depth" is now honored when detecting court software.
- **Event listener leaks** — conversion completion listeners and waveform AudioContexts are no longer leaked; repeated use no longer breaks waveform rendering.
- **Temp file cleanup** — VAD/scoring/speaker-detection temp WAVs are cleaned up on error paths via drop guards.
- **Peak limit of 0 dB** — a configured 0 dB peak limit is no longer silently replaced with the default −1.5.
- **Library badge** — case count now shows on startup instead of after first opening the Library tab.

### Changed
- CI installs dependencies with plain `npm ci` — the eslint peer-dependency conflict was fixed by upgrading `eslint-plugin-react-hooks`, so `--legacy-peer-deps` is gone.
- Dependency refresh (in-range updates for Radix UI, Tauri plugins, React, Vite tooling).
- **Vite 8** (from 6) — the build toolchain moved to Vite 8 (Rolldown-based); `esbuild` is now an explicit dev dependency since Vite no longer bundles it.
- **sha2 0.11** (from 0.10) — model-integrity hashing updated for the new digest API (explicit hex encoding of the digest).

## [0.7.0] - 2026-03-24

### Added
- **Multi-source merge** — combine multiple recordings of the same event into one clean file with automatic sync detection via cross-correlation. Two strategies: "Best quality" (picks clearest source per segment) and "Mix all" (blends everything).
- **Merge tab** — new fourth tab with source management, sync badges, strategy selector, and merge controls.
- **Silero VAD** — voice activity detection at 10ms granularity. Detects speech vs. silence segments. Integrated into analysis with speech ratio reporting.
- **De-reverberation** — optional DCCRN+ model reduces room echo (model exported via scripts/export_dccrn.py).
- **Hardware accelerator detection** — detects Apple CoreML (ANE), AMD XDNA NPU (Ryzen AI), AMD ROCm, Intel AI Boost NPU, Intel OpenVINO, and Windows DirectML. Shown in UI with performance tier recommendation.
- **Health check command** — verifies FFmpeg/FFprobe sidecar availability and reports models + accelerator at startup.
- **Radix UI upgrade** — Switch (accessible toggle), Dialog (focus-trapped modal), Tabs (keyboard-navigable), Tooltip (reusable with provider).

### Security
- **TempFile drop guard** — automatic cleanup of temporary audio files, even on panic/crash.
- **ONNX model SHA256 verification** — framework for verifying bundled model integrity.
- **File-level locking** — library.json uses exclusive file locks (fs2) to prevent concurrent write corruption.
- **Input sanitization** — case names stripped of path separators and control characters, length limits enforced.
- **Error sanitization** — filesystem paths stripped from all user-facing error messages.
- **File size limits** — 2GB maximum for audio files loaded into memory.
- **Parameter validation** — sample rate (8-192kHz) and fade duration (0-30s) validated before processing.
- **SGMCA safety** — handles empty/short files properly instead of silent failure.
- **Event-driven completion** — replaced polling with event listeners in useConversion (eliminates stale closure risk).
- **App init race fix** — library loaded from disk once, then uses in-memory state.
- **Windows path fix** — correct parent directory extraction using native separators.

### Changed
- Four-tab layout: Convert | Player | Merge | Library
- Analysis includes speech ratio from VAD and de-reverb recommendation

## [0.6.0] - 2026-03-24

### Added
- **Smart Audio Cleanup** — new AI Enhance panel that scans audio and auto-detects issues (noise, level imbalance, clipping, narrow bandwidth)
- **Remove Background Noise** — neural noise suppression via nnnoiseless (RNNoise, fast) or DeepFilterNet3 ONNX (best quality)
- **Balance Speaker Volume** — turn-aware auto-leveling using Pipecat Smart Turn v3 ONNX model + ebur128 loudness analysis
- **Fix Clipped Audio** — FFmpeg adeclip filter, auto-enabled when clipping detected
- **Improve Audio Clarity** — FlashSR ONNX neural bandwidth extension for phone recordings (16kHz to 48kHz)
- **Quality Scoring** — DNSMOS model rates audio quality on a 1-5 scale before and after processing
- **Speaker Count Detection** — pyannote speaker segmentation detects how many speakers are present
- **Hardware-Aware Recommendations** — detects CPU cores, RAM, and Apple Silicon to recommend optimal processing settings
- **Global Audio Player** (new tab) — play any audio file without conversion, with color-coded speaker tracks and editable labels
- **Court Software Detection** — scans for installed Case CATalyst, FTR Gold, Eclipse, DigitalCAT, CourtSmart and lists available jobs for import
- **Denoise Quality Selector** — choose between "Fast" (RNNoise) and "Best quality" (DeepFilterNet3) per conversion
- **Phase-Aware Progress** — shows "Listening to audio...", "Cleaning up audio...", "Converting..." during AI processing stages

### Changed
- App tagline updated to "Audio Converter & Enhancer" to reflect broader use
- Default channel labels changed to "Speaker 1-4" (editable, no longer court-specific)
- Drop zone now lists standard formats first (WAV, MP3, FLAC, Opus) before court formats
- Three-tab layout: Convert | Player | Library
- Processing chain preview includes AI steps (Denoise, Enhance, De-clip, Auto-Level)
- Channel Mix sliders auto-disabled and show "auto" when Balance Speaker Volume is on

### Architecture
- New two-step conversion pipeline: Rust AI processing (denoise, enhance) writes temp WAV, then FFmpeg handles DSP + format conversion
- ONNX Runtime integration via `ort` crate for running AI models natively in Rust
- `nnnoiseless` crate for pure-Rust RNNoise noise suppression
- Bundled ONNX models (~57MB): Smart Turn v3, FlashSR, DeepFilterNet3, DNSMOS, speaker segmentation + embedding
- Dynamic channel handling throughout (mono, stereo, multi-channel)

## [0.5.0] - 2026-03-21

### Added
- **AAC/M4A output format** — 128 kbps AAC in M4A container for Apple device compatibility
- **Configurable MP3 bitrate** — choose 128, 192, 256, or 320 kbps from the format selector
- **.dcr format detection** — Liberty Court Recorder files are identified with guidance to export via Liberty software
- **Dynamic channel count** — auto-detects 4–16 channel recordings and resizes the UI accordingly
- **Retry button** — failed conversions show a retry action instead of requiring re-import
- **Extended channel palette** — 16 distinct colors for multi-channel recordings (up from 4)

### Changed
- Drop zone moved above configuration panels for a more natural top-down workflow
- Progress bar now shows real percentage derived from FFmpeg duration probe
- Stereo mix uses actual detected channel count instead of hardcoded 4

### Fixed
- Vite 8 build — switched minifier from deprecated esbuild to oxc

## [0.4.0] - 2026-03-12

### Added
- **Track preview** — audition individual channels before conversion for quick identification (Reporter, Witness, Attorney 1 & 2)
- **Mix audition** — preview the final stereo mix with per-channel volume and processing applied
- **Processing preview** — hear the effect of high-pass filter, normalization, and silence trimming before committing
- **Cancel conversion** — stop an in-progress conversion at any time; partially written files are cleaned up automatically
- **Atomic file writes** — output files are written to a temporary path first, then moved into place, preventing corrupted files on crash or cancel
- **Global audio player** — persistent playback bar with queue support across Convert and Library views
- **React error boundary** — catches unhandled UI crashes and offers a recovery button instead of a blank screen
- Dependabot configuration for automated dependency updates
- MIT LICENSE file and CODEOWNERS

### Changed
- Upgraded React 18 → 19 and react-dom to 19.2.4
- Upgraded Vite 7 → 8 (Rolldown-based bundler)
- Upgraded @vitejs/plugin-react 4 → 6 (Babel removed, Oxc-based transforms)
- Updated Cargo dependencies including tauri-plugin-updater to 2.10.0
- Updated CI actions (actions/checkout v6, actions/setup-node v6)
- Improved conversion progress UI with smoother status updates

### Security
- Bumped `tar` crate 0.4.44 → 0.4.45 (fixes symlink-directory collision chmod attack)
- Bumped `rustls-webpki` 0.103.9 → 0.103.10

## [0.3.0] - 2026-03-12

### Added
- In-app auto-updater powered by GitHub Releases — update notifications with change notes and skip option
- Shared audio player hook (`useAudioPlayer`) reducing code duplication
- ARIA labels and semantic roles throughout the UI for accessibility
- Update banner component with download progress and release notes viewer

### Fixed
- **Critical:** `ConvertJob` serde deserialization — added `camelCase` rename so frontend `srcPath`/`outDir`/`chanVols`/`fadeDur` fields map correctly to Rust struct
- macOS Intel CI builds — replaced deprecated `macos-13` runner with cross-compilation on `macos-latest`
- Toggle component accessibility — changed from `<div>` to `<button role="switch">` with keyboard support

### Changed
- ConvertTab now accepts grouped prop objects (`prefs`, `fileDrop`, `conversion`) instead of 30+ individual props
- Consolidated duplicate `.proc-chain` CSS definitions
- Updated README with full feature docs, architecture overview, and format support table
- Replaced placeholder SECURITY.md with actual project information

## [0.2.0] - 2026-03-11

### Changed
- Refactored Rust backend from single 806-line `lib.rs` into 7 focused modules:
  - `types.rs` — all struct/enum definitions
  - `helpers.rs` — format registry, path helpers, SGMCA stripping
  - `ffmpeg.rs` — FFmpeg probe, filter chain, sidecar runner
  - `conversion.rs` — core conversion logic (stereo/keep/split)
  - `persistence.rs` — library and preferences storage
  - `commands.rs` — Tauri command handlers
  - `lib.rs` — module declarations and app builder
- Added `protocol-asset` feature to Tauri dependency
- Skipped macOS code signing for unsigned builds

## [0.1.0] - Initial Release

### Added
- SGMCA, BWF, FTR/TRM, DigitalCAT .dm, and standard audio format support
- Three output modes: Mix to Stereo, Keep Original, Split Channels
- Per-channel volume controls (Reporter, Witness, Attorney 1 & 2)
- Processing chain: High-pass filter, loudness normalization, silence trim, fade in/out
- Output formats: WAV, MP3 (192 kbps), FLAC, Opus (64 kbps VBR)
- Case/witness auto-filing library with inline playback and re-export
- Batch processing support
- Windows (x64) and macOS (Apple Silicon + Intel) builds
