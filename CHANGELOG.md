# Changelog

## [Unreleased]

## [1.1.0] - Unpublished

DepoAudio 1.1.0 is the first release since 1.0.2. It includes everything that was prepared for 1.0.3. That version was never published, so there is no 1.0.3 download.

### Added

- **Every converted file gets an export record.** DepoAudio writes `<output>.depoaudio.json` next to each output. It holds the source and output file names (never folders), their SHA-256, the app version, the exact FFmpeg filter chain and encoder settings, the channel and its label, and the recording start time when the source has one. A record never replaces an existing file: if the output or record name is taken, both move to the next numbered name. If the record cannot be written, the conversion fails and its outputs are removed rather than delivered without one. The format is published as `docs/export-manifest.schema.json`. Hashing reads each file once more, so large conversions take a little longer.
- **The recording start time is carried through.** When a source WAV has a BWF `bext` start time, WAV outputs get their own `bext` chunk with the same start, rescaled to the output sample rate. FLAC, MP3, Opus and M4A have no such chunk, so their start is kept only in the export record. With leading-silence trim on, the first sample moves by an amount FFmpeg does not report, so the output carries no start time and the record says the output start is not known.
- **Settings → Your data shows where DepoAudio keeps its data.** It lists the app data folder (with a button to show it) and each DepoAudio file in it with its size, and counts the saved transcripts and bookmarks. It says that recordings, converted files and export records stay where they were saved, that nothing is uploaded and no account is needed, and that DepoAudio does not encrypt these files itself, so full-disk encryption should be on. [`docs/LOCAL-DATA.md`](docs/LOCAL-DATA.md) documents every location.
- **Back up and restore the library and transcripts.** Export writes a new `.zip` (never replacing a file) with the case library, transcripts, bookmarks and a README; it holds no audio or settings and is not encrypted, and the app says so. Import reads only an unmodified DepoAudio backup and only adds: cases match by name and sessions by ID, a transcript is restored only where this computer has none for that track, and bookmarks are added unless already present. The result lists what was added, what was kept, and which Library files are not on this computer.
- **Clear all DepoAudio data.** After a confirmation, it removes the library, settings, interrupted-save files and model files left by an older version from the app data folder, and DepoAudio's saved transcripts, bookmarks and preferences, then reloads the app as on first launch. Recordings, converted files, export records and backups stay. It is not available while files are converting. It also recovers from a damaged library, which otherwise leaves the Library read-only.
- **Delete the transcript for a track.** A bin button on the Player's transcript card removes the saved transcript after a confirmation. Exported SRT and TXT files are not affected.
- **The app version is shown in Settings and on the recovery screen, with "Copy diagnostics".** It copies only the app version, operating system and processor type, FFmpeg version and, on the recovery screen, the error type. Error messages, file names and folder paths are never included. The exact text is shown before it is copied, and nothing is sent: you choose where to paste it.
- **Interface failures have a recovery screen.** If the interface hits an unexpected error, a recovery screen explains what happened and offers a reload, without uploading diagnostics, file names, recordings or case information.

### Changed

- **DepoAudio uses the DepoStack design.** Colors, type and spacing follow the canonical DepoStack palette and type roles in both themes, the sidebar and Settings → About show the DepoStack mark, and the app is named "DepoAudio, powered by DepoStack". Text on gold buttons, badges and the Play button is plum-black and stays above 7:1 contrast, including on hover. Gold is kept for fills and the main action; lines and icons are ink. The macOS app category is now Productivity. The bundle identifier, saved data and update address are unchanged.
- **The Archivo and Fragment Mono typefaces are bundled with the app.** They replace the system font stacks, their SIL Open Font License texts are included in the app's third-party notices, and they load from the app itself, so DepoAudio never fetches a font from the network. Fragment Mono is used only for real code such as paths, hashes and diagnostics.
- **Scan suggests fixes instead of switching them on.** After a scan, Balance Microphone Levels and Fix Clipped Audio are marked Recommended with what was found, and a line names the suggested fixes with an "Apply suggestions" button. Nothing changes until you press it or turn a toggle on yourself.
- **macOS FFmpeg is built from source under the LGPL.** The Apple Silicon and Intel FFmpeg and ffprobe now come from one hash-pinned FFmpeg 7.1.5, LAME 3.100 and Opus 1.6.1 source set instead of community binaries that were marked nonfree and mixed major versions. The build rejects GPL and nonfree configurations, and the release carries the FFmpeg, LAME and Opus source archive this build is made from.
- **Windows uses the LGPL FFmpeg build.** The hash-pinned BtbN LGPL build replaces the GPL variant. Native FTR decoding and every output encoder were checked first, and the build's license, configuration and source reference ship with the app.
- **Channel and microphone wording no longer implies speaker identity.** Split output, automatic level balancing, default track labels and their descriptions describe channels and microphones, not people.
- **The interface is clearer for keyboard and screen-reader use.** The main window has a landmark and heading; search, scan, conversion, update, copy and error states announce short summaries; focus stays visible on workspace panels; reduced-motion preferences apply app-wide; and the compact minimum window reflows cleanly.
- **Only the canonical DepoStack and DepoAudio release links open**, and a link that cannot be opened now says so instead of doing nothing.
- **An empty transcript no longer leaves a saved entry.** Opening a track in the Player used to save an empty transcript entry that kept the track path, and often the case name, in the app's storage.
- **DepoAudio now runs on Tauri 2.12** with updater plugin 2.14.1. In-app updates stay off unless a release carries a verified updater configuration. The Windows app keeps using the Visual C++ runtime installed on the computer.

### Fixed

- **Every Settings section can be read to the end.** Cards in Settings were shrunk to fit and cut off their lower rows (most visibly in Model files and Your data). Each card now keeps its full height and only the content pane scrolls; the section list and the Settings header stay in place, and each section opens at its top.
- **A converted file can be revealed and opened in the Player from its row.** "Show in Explorer / Finder" appeared only when a conversion made several files. Every completed row now has "Reveal in folder", and "Open in Player" when the output is a format the Player opens directly (WAV, MP3, FLAC, Opus, M4A). Open in Player adds the outputs to the playlist and selects the first; split outputs are all added. Each button's accessible name starts with its visible label (for example "Reveal in folder: hearing.mp3"), so voice control can press it by what it says.
- **Re-export from the Library asks before replacing the Convert queue.** Re-export (and queueing a court-software job) used to replace whatever was waiting in Convert without a word. When the queue already has files, DepoAudio now asks "Replace the N files in the queue?" with Replace, Add to queue and Cancel. Add to queue has focus when the dialog opens, so pressing Enter never replaces the queue, and the count stays current if files are added while the dialog is open. Cancel leaves the queue as it was. With an empty queue it fills the queue straight away, as before. Nothing is deleted from disk either way.
- **File-picker failures in the Library are no longer treated as a cancel.** A failed picker now shows a clear error, with technical detail behind a disclosure.
- **Transcript following and saving hold up on long or messy transcripts.** Active-line lookup handles unsorted and duplicate timestamps, edits are saved shortly after typing and when you change track, and if local storage fails the edits stay in the session with one warning to export before closing.
- **The guided conversion steps stay readable at high display scaling.**
- **One file with an out-of-range modified date no longer fails a court-software scan.** A file dated past the year 9999 is left out of the job's date instead of stopping the whole scan.
- **Deleting model files left by an older version only deletes.** Unknown names, folders, links and paths outside the models folder are refused, and no model install or download action remains.

### Removed

- **Learned-model processing is not included in 1.1.0.** Noise Removal, Enhance Clarity, Reduce Room Echo, turn detection, speaker estimation and neural quality scoring are removed, along with the bundled model files, ONNX Runtime and model downloads. Model files left by an older version are never loaded; Settings → Model files lists them so you can delete them. The public `models-v1` assets that 1.0.2 uses are unchanged.
- **Merge stays hidden.** Its commands are no longer reachable from the interface layer either, until Merge returns in a later release.

### Security

- **Local data is not encrypted by DepoAudio.** The library, settings, saved transcripts and backup files rely on the operating system's full-disk encryption, which Settings → Your data asks you to turn on (`SECURITY.md`, `docs/LOCAL-DATA.md`).
- **No Rust advisory exception is configured.** Tauri 2.12 removes the five unmaintained `unic-*` crates, and updated `plist`, `quick-xml`, `rustls` 0.23.46, `rustls-webpki` 0.103.17, `tokio` 1.54.1 and `zip` 9.4.1 (through the updater plugin) resolve the advisories published since 1.0.2. CI checks advisories on every change and weekly.
- **DepoAudio no longer uses chrono**, which RustSec lists as unmaintained (`RUSTSEC-2026-0543`). Timestamps now come from the `time` crate, and export records, backups, Library cases and session dates are written exactly as before. No advisory exception was needed.
- **Releases go through a fail-closed, one-person publication path.** A build only ever makes a private draft. Publishing needs a fully closed release gate, a 24-hour cancellation window that administrators cannot skip, protected release tags, and a signed release tag from the release owner at the exact source commit. Installers are inventoried after extraction, and each release carries per-platform SHA-256 checksums, third-party notices, SBOMs and the corresponding source for the macOS FFmpeg build.
- **CI scans the full history for committed secrets** on every pull request, push to `main` and weekly run.
- **Security reporting goes to the DepoStack repository.** `SECURITY.md` points to the DepoStack repository, public issues no longer ask for vulnerability details, optional update traffic is distinguished from local recording processing, and two incident runbooks cover a bad release and lost or compromised signing keys.

### What this release includes

- 1.1.0 adds local-data features (export records, backup and restore, clear all data, delete transcript, Copy diagnostics), fixes in Convert, Library and Settings, and the DepoStack design with bundled fonts. It adds no new input or output format.
- Learned-model processing is deliberately unavailable, and Merge stays hidden.

## [1.0.2] - 2026-08-20

### Fixed

- **Packaged desktop builds open when signed in-app updates are unavailable.** DepoAudio now registers the updater only when the release contains a complete updater configuration; unsigned Windows and macOS builds no longer abort during Tauri setup.
- **Release validation launches the exact packaged applications.** Windows CI administratively extracts the generated MSI and starts its executable, while macOS CI starts the generated app bundle, so launch-only configuration failures turn the release run red before manual publication.

## [1.0.1] - 2026-08-20

### Added

- **The synced transcript editor is available in the Player again.** Import SRT, VTT, or TXT, follow and edit cues against the playhead, stamp new lines, and export the finished transcript locally.

### Fixed

- **Bundled ONNX Runtime 1.22 now uses its stable C API 21 path.** DepoAudio avoids the unused AutoEP API that fails against Microsoft's macOS universal package, retains its explicit CPU/CoreML selection, compiles macOS-specific code in CI, and runs real Silero VAD inference against the staged native library before packaging.
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
- **Scan no longer stalls on "Detecting speech"** because the two dynamic-shape analysis models now run on the CPU and loaded models are cached for the app's lifetime.
- **Conversion progress shows a real percentage** during encoding, capped at 99% until the output is complete; AI cleanup phases retain an activity state.
- **The guided steps behave consistently** by completing with the current queue and resetting when a new file is added.

### Security

- Model deletion and Merge now validate catalog entries, local source paths, output names, and destination containment to block traversal, arbitrary deletion, and FFmpeg URL/device input.
- The WebView now has a restrictive CSP, no frontend shell execution permission, a narrowly scoped external URL opener, and persisted access only to paths selected by the user instead of a static whole-disk asset scope.
- Manually fetched native binaries and source archives use immutable asset IDs or versioned upstream URLs where available, with reviewed size and SHA-256 contracts. This does not pin hosted runner images, registry packages, or Tauri's moving WebView2 offline-installer resolution; those require separate final-artifact evidence. Updater artifacts are generated only when a verified updater key pair is configured, and platform signing remains opt-in. The Windows LGPL FFmpeg pin targets a retained month-end build instead of BtbN's short-lived rolling asset. Published assets are never replaced or uploaded with `--clobber`.
- Frontend dependency advisories were resolved without force-upgrading major versions.

### Changed

- React/ReactDOM and Radix UI primitives are refreshed as coordinated sets, with Dependabot grouping that prevents incompatible version skew; current Rust patch releases are also locked and validated at the Rust 1.88 support boundary.
- Frontend dialog bindings now match the Rust plugin, test helpers are updated without raising the documented Node baseline, and CI's Rust cache action is pinned to the verified v2.9.2 commit.
- Vite, its React plugin, Tauri's JavaScript API/CLI, Lucide icons, and the remaining build/lint/CSS tools are refreshed as compatible sets; the existing Node 22.12 minimum is now explicit in package metadata and developer setup.
- DeepFilterNet "Best" processing is disabled until its complete spectral pipeline is implemented. Requested denoise, dereverb, and enhancement stages now fail clearly instead of silently falling back or no-oping.
- Windows releases now accurately report the bundled CPU ONNX Runtime; Apple Silicon builds may use CoreML. Hardware tiers no longer claim unavailable Windows DirectML/NPU execution providers.
- Speaker segmentation is now labeled as an active speaker-slot estimate instead of distinct-speaker identification; embedding and cross-window voice clustering are not implemented in this release.
- Auto-update configuration is injected only into release builds with updater-signing keys configured; local/development builds no longer carry a placeholder updater key or stale repository endpoint.

## [1.0.0] - 2026-07-16

The first public release. A desktop audio converter and enhancer for court reporters that processes recordings locally.

### Features

- **Conversion** — court formats (SGMCA, FTR/TRM, BWF, DigitalCAT) plus standard audio and video, out to WAV/MP3/FLAC/Opus/M4A; three output modes (mix to stereo, keep original, split by channel) with per-channel labels and volume; batch processing and 5 presets.
- **Smart Cleanup interface** — one-click Scan presents denoise, auto-level, de-clip, clarity, turn, speaker-slot, and quality analysis. Several ONNX-backed paths were not verified in the published v1.0.0 installers and should not be treated as released behavior.
- **Player** — color-coded speakers, 0.5×–2× speed, A-B loop, and editable bookmarks.
- **Library & detection** — case library auto-filed by case and participant (search, archive, re-export); court-software detection (Case CATalyst, FTR Gold, Eclipse, DigitalCAT, CourtSmart) with direct job import.
- **Platform** — universal macOS (Apple Silicon + Intel) and Windows. The published macOS app was ad-hoc signed and not notarized, the Windows installers were unsigned, and the release contained no updater manifest. Recordings remained local; no account or subscription was required.

### Added

- **Live scan progress** — the Convert-tab Scan streams progress for every analysis phase (loudness, noise floor, speech, speaker turns, quality, speakers), the bar advances _within_ each file with a phase label — including heartbeats while a slow FFmpeg pass drains its timeout, so "slow but alive" looks different from "stuck" — and a Cancel button actually stops the backend compute.
- **FTR session chunks auto-order chronologically** — dropping or browsing a set of `.trm`/`.ftr` chunks queues them in recording order (parsed from FTR's filename timestamp, verified against real court-produced files) regardless of how the OS delivered them. Applies to the Convert queue and the Player playlist (newly added batches only — manual reordering is preserved). Mixed selections keep the order you chose.
- **Universal macOS build** — one `.app`/`.dmg` runs natively on both Apple Silicon and Intel. FFmpeg sidecars and the ONNX Runtime library are combined for both architectures; no more separate Intel/Apple-Silicon downloads.
- **Auto-update code path** — the app contains an updater UI and signed-manifest verification path, but the published v1.0.0 assets did not include `latest.json` or updater signatures, so updating was dormant in that release.
- **Player keyboard transport** — Space/K play-pause, ←/→ seek ±5s, J/L seek ±10s, ↑/↓ change speed, [ / ] previous/next track, B add bookmark (ignored while typing in a field).
- **Playback speed** — 0.5×–2× control in the player, persists across sessions (essential for transcription).
- **A-B loop** — set in/out points and repeat a passage for re-listening.
- **Bookmark notes & export** — bookmark labels are now editable (e.g. "Objection", "Exhibit 4") and the whole list copies to the clipboard as timestamped lines for a transcript.

### Changed

- **DepoStack brand** — full visual rebrand: plum + gold on warm cream (light) and a deep plum-night (dark). Plum is the primary ink, gold the accent and call-to-action (gold buttons with plum ink), with generously rounded cards and soft shadows. Every color lives in `design/tokens.json`; light-mode status colors are tuned for legibility. Axe-core returned no violations in the tested screens and themes; that automated result is not a full conformance assessment.
- **"Docket" redesign (part 1)** — the app shell and Convert flow, rebuilt:
  - **Sidebar navigation** replaces the top tab bar: icons + labels, number-key shortcuts (1–4), the case-library count, and a live system-health card (FFmpeg status, installed AI models, update state) with Settings and theme at the bottom. Collapses to an icon rail on narrow windows.
  - **Guided steps on Convert** — a state-driven "Add recording → Choose settings → Convert" stepper shows where you are without hiding anything; the whole page still works at once for batch users.
  - **Format tiles** with plain-English trade-offs replace the small format buttons; sample rate and MP3 bitrate live alongside them.
  - **Output mode** is now a segmented control; scan findings appear as green "Recommended" pills on the matching enhancement toggles; the action bar summarizes what's about to happen ("Ready: MP3 · mix to stereo with 2 enhancements → same folder as source").
  - **Light theme retuned** to cool neutrals with white cards and soft shadows (dark theme keeps its ink palette with the new structure); corner radius increased app-wide. All changes use design tokens, and the tested screens returned no axe-core violations.

### Fixed

- **Scans finish now — every prior release could hang or freeze** — one fix in three layers. Analysis reads a bounded sample of each file instead of the whole recording (a multi-hour multichannel deposition previously ran tens of thousands of ONNX inferences and effectively never completed), and every analysis FFmpeg pass has a timeout backstop whose expired processes are killed rather than orphaned at full CPU. AI inference moved off the async runtime onto the blocking pool with cancellation checks and wall-clock budgets, so queued scans can no longer freeze the entire app. And a stalled file is skipped after 150 seconds of silence instead of poisoning the whole scan, with failures reported ("2 of 3 files couldn't be analyzed") instead of silently reverting to the idle hint.
- **Scanning FTR (.trm) files always failed** — analysis passes never applied the forced FTR decoder that conversion has always used, so every Scan of a `.trm` file failed or timed out. Scan decodes now handle FTR's proprietary codec, and files that genuinely can't be decoded short-circuit the remaining passes with an honest "convert it first" recommendation. A channel-probe failure also no longer invents four phantom channels of extra work.
- **Model downloads restored** — the `models-v1` release hosting all nine downloadable AI models was deleted during a repository cleanup, so every in-app model download returned 404; it was rebuilt with each asset verified against the app's SHA-256 pins. The real DNSMOS quality-scoring model is now published and integrity-pinned like the rest (the previously committed file was an HTML error page), and its catalog size corrected (0.3 → 1.1 MB).
- **Auto-level gain safety** — conversion only applies per-channel auto-level gains when the analysis measured the same channel count the converter sees, preventing a desynced analysis from boosting some channels and silencing others.
- **App now closes immediately** — closing the window quits the process directly, so an in-flight scan or conversion can no longer leave the app stuck on exit.

### Improved

- **Responsive layout** — the UI now scales to the window instead of sitting in a fixed 920px column: content fluidly uses available width (up to a comfortable 1100px for readability) and reflows cleanly down to a 720px minimum, eliminating horizontal scrolling. Default window enlarged to 1160×820 for more breathing room.
- **Library tab** no longer permits horizontal scrolling (added the same overflow guard the other tabs already had).
- **Accessibility checks** — axe-core findings in the tested screens and both themes were fixed: hint/muted text and gold link/tab colors were adjusted for contrast, primary-button labels were corrected in light mode, and selects received accessible names. This automated test result is not a full WCAG conformance assessment.
- **Theme is now token-driven** — all colors live in `design/tokens.json` (W3C design-token format) as primitive→semantic layers; `npm run tokens` regenerates the CSS, and CI fails if the two drift apart.

### Internal

- **Characterization test suite** — 70 new golden-master tests pin the behavior users depend on: FFmpeg filter chains and filtergraphs per output mode, preference patch-merge semantics, library filing rules, import sanitization, IPC wire shapes and defaults, transcript SRT/plain parsing round-trips, and player logic. `PARITY.md` documents the full capability/contract inventory.
- **CI workflow** — every PR now runs eslint, the JS and Rust test suites, a Vite build, `cargo clippy -D warnings` (codebase is warning-clean), and the token-drift check.
