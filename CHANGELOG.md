# Changelog

## [Unreleased]

## [1.0.3] - Unpublished

### Added

- **Unhandled interface rendering failures have an explicit recovery path.** A root-level React recovery screen explains the failure and offers a user-initiated reload without uploading diagnostics, filenames, recording data, or case information.
- **Frontend size budgets and release-target dependency policy now run in CI.** Deterministic JavaScript and CSS limits catch bundle regressions, while Cargo license, source, wildcard, and duplicate-dependency rules cover the macOS and Windows release graph.
- **Distribution evidence is tracked separately from dependency policy.** A third-party ledger records native binaries, installer payloads, CI fixtures, and already-published legacy model assets that still require provenance, notices, hashes, or redistribution review.
- **The release workflow revalidates the complete version contract before building.** A draft cannot be prepared when npm, Cargo, Tauri, or changelog versions have drifted.
- **Publication now has a machine-readable, fail-closed approval contract.** The gate records every open blocker and, before publication, must bind a reviewed private release ID, source commit, asset-manifest digest, updater decision, publication date, and accountable approver.
- **JavaScript and generated-CSS notices are deterministic release evidence.** The exact production closure generates byte-stable HTML and JSON inventories, with a strict candidate gate for unresolved license evidence.
- **A zero-model distribution verifier now fails closed.** It rejects learned-model resources, known model hashes, runtime download paths, compiled learned dependencies, and registered learned commands before a v1.0.3 candidate can be built.

### Changed

- **Maintenance dependencies and the paired CodeQL actions were refreshed without changing product behavior.** The release graph now uses `uuid` 1.24.1, `globals` 17.11.0, and CodeQL Action 4.37.7 for both initialization and analysis.
- **The legacy DNSMOS verification source now uses an immutable upstream revision.** It remains historical v1.0.2 evidence only and is not downloaded or executed by v1.0.3.
- **Release builds now pin the exact Node and Rust toolchains exercised by the private candidate.** A later hosted-run image update can no longer silently change the compiler or JavaScript runtime used for final artifacts.
- **macOS FFmpeg/ffprobe now build from one hash-pinned LGPL source set for Apple Silicon and Intel.** The release no longer consumes the prior community binaries, which enabled FFmpeg's nonfree/unredistributable mode and mixed major versions. The build rejects GPL/nonfree configuration, verifies the released FTR/codec/filter surface, preserves exact source/configuration/toolchain/license evidence in the app, and publishes the complete corresponding source beside the installers.
- **Windows now uses the retained, hash-pinned BtbN LGPL FFmpeg build instead of its GPL variant.** Native FTR decoding and every released output encoder were verified before the workflow pin changed; the exact archive license, binary configuration, source commit, and asset digest are bundled for final installer review.
- **The legacy `models-v1` verification workflow is now read-only.** It checks every published model against the app pins and reviewed source bytes but can no longer replace public model assets with `--clobber`; future model sets require a new versioned release.
- **Rust advisory policy is blocking with one narrow, expiring maintenance exception.** Only the five INFO/unmaintained `unic-*` findings inherited through released Tauri 2.11.5 are excepted, with a named owner and 2026-09-30 deadline; any new advisory fails CI, and the exception ends sooner if Tauri publishes its merged `urlpattern` fix.
- **Release candidates now use one deliberate dispatch path.** Candidate builds must be started manually from `main`; pushing a version tag no longer starts or mutates a private draft, and the protected publication workflow alone binds the approved tag.
- **Publication now fails closed when GitHub environment protection is missing.** Before any release write, the protected workflow verifies that `release-publication` has a required reviewer, prevents self-review, and carries a deployment branch policy. Its strict local contract is named `--publication-ready` to describe what it actually verifies.
- **Library file-picker failures are no longer treated as cancellation.** A failed native picker now produces a focused, actionable error while retaining technical diagnostics behind disclosure.
- **Keyboard focus remains visible on workspace tab panels.** Active panels keep their layout wrapper and display the shared two-pixel focus treatment when they receive focus.
- **Drafts now carry the versioned changelog section and per-platform SHA-256 manifests.** Reviewers can verify the exact MSI, EXE, DMG, and app archive instead of publishing a generic release body with unrecorded hashes.
- **Installer builds and release uploads are separated.** Tauri runs without a release ID, tag, or GitHub mutation token; every installer, signature, and evidence asset is uploaded only through a fail-closed helper that rechecks the exact private draft, source commit, tag, state, and asset-name uniqueness before and after each upload.
- **Native and packaged smoke tests now cover both downloadable containers on each platform.** Before packaging, the workflow generates a transient synthetic 440 Hz FTR-tagged fixture, normally decodes it with the exact staged candidate sidecars, and removes the fixture and derived outputs. It then extracts the MSI, NSIS installer, DMG, and app archive, exercises the packaged sidecars and released encoders, rejects learned-model or ONNX Runtime payloads, and checks macOS signatures and universal binaries.
- **Signing credentials are scoped to packaging.** The imported Windows certificate and private key are removed before candidate execution, while updater signatures are retained as private-draft evidence and `latest.json` remains reserved for a separate publication-only approval path.
- **Dependency evidence is reproducible and generated before packaging.** Hash-pinned cargo-about and cargo-cyclonedx binaries regenerate the bundled Rust notice and target-specific SBOMs; npm and Rust SBOM normalization removes checkout identity, binds timestamps to the release commit, and verifies two independent generations byte-for-byte. SBOMs remain inventories rather than substitutes for unresolved JavaScript notices.
- **Draft evidence now inventories the contents extracted from the DMG, app archive, MSI, and NSIS installer.** The gate requires FFmpeg and JavaScript-notice evidence, rejects the generated FTR fixture and its derived outputs, every ONNX file, and known model bytes even when renamed, and proves the two macOS containers carry the same app payload.
- **Artifact inventories enforce exact packaged paths as well as filenames and hashes.** A native runtime or application executable in an unexpected location now fails the binary contract instead of passing through basename-only matching.
- **v1.0.3 distributes and exposes zero learned models.** Bundled ONNX files, embedded RNNoise weights, ONNX Runtime, model downloads, learned cleanup and neural analysis commands are removed from the release build. Exact obsolete app-data files can appear only as user-removable legacy storage and are never loaded. The public `models-v1` assets remain untouched for already-published v1.0.2 compatibility.
- **Existing workflows now use clearer, more accessible interface semantics without adding capabilities.** The normal shell has a main landmark and accessible heading; search, scan, conversion, update, model, copy, and error states announce useful summaries; focus treatments and compact-window reflow are consistent; and reduced-motion preferences apply app-wide.
- **Channel and microphone language replaces inferred-person wording.** Split output, automatic level balancing, default track labels, and supporting descriptions no longer imply that DepoAudio identifies a speaker's voice.
- **Startup no longer requests blocked Google Fonts, and approved trust links are exact.** The app uses its local/system font stack, permits only the canonical DepoStack and DepoAudio Releases URLs, and reports opener failures instead of silently doing nothing.

### Fixed

- **Rust dependency advisories in `plist` and `quick-xml` were resolved** by updating the locked packages, removing `RUSTSEC-2026-0194` and `RUSTSEC-2026-0195` from the release graph.
- **Waveform PCM decoding passes the Rust 1.98 Clippy gate** by using the Rust 1.88 fixed-size slice API after the existing byte-alignment validation; the decoded samples and output peaks are unchanged.
- **Release documentation no longer treats a narrow automated smoke test as complete product verification.** Installed conversion, player, library, transcript, model, installer, signing, and hardware behavior remain explicit manual candidate gates.
- **Security and product documentation now use the canonical DepoStack repository and precise privacy language.** Public issue reports no longer request vulnerability details, optional update traffic is distinguished from local recording processing, and unsupported response-time and absolute reliability promises were removed.
- **Legacy model cleanup is deletion-only and fail-closed.** Unknown names, traversal, directories, and symlinks are rejected; no model install or download action remains.
- **Transcript following and persistence are bounded and failure-aware.** Active-line lookup handles unsorted and duplicate timestamps through a cached index, edits are debounced and flushed on track changes or unmount, storage failures preserve a recoverable in-session draft, and one warning tells the user to export before closing.
- **The guided conversion steps remain readable at high display scaling.** At compact effective widths, all three steps use an unclipped three-column layout.

### Release scope

- This maintenance release does not add a conversion, format, learned-cleanup, player, case-library, Merge, or transcript capability. Its interface, accessibility, failure handling, and persistence safeguards are improved while learned-model processing is deliberately unavailable.
- Five upstream unmaintained `rust-unic` advisories remain visible through Tauri's `urlpattern` dependency. They are not suppressed and require a documented risk decision before publication.

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
