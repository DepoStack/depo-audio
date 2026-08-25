# Third-party distribution inventory

This document defines the evidence required before a DepoAudio installer or
model release is approved for distribution. It is an engineering release gate,
not a legal opinion and not a claim that every item below has already been
cleared for commercial use.

The repository currently contains an MIT application license in `LICENSE` and
`src-tauri/installer/license.rtf`. That license covers DepoAudio's own code; it
does not replace licenses or notices required by dependencies, native binaries,
models, training data, fonts, or other third-party material.

## What the automated Rust gate covers

`deny.toml` evaluates the locked Cargo graph for the release targets:

- Apple silicon macOS (`aarch64-apple-darwin`)
- Intel macOS (`x86_64-apple-darwin`)
- x64 Windows (`x86_64-pc-windows-msvc`)

CI runs cargo-deny 0.20.2 through the commit-pinned official action. It blocks
licenses outside the reviewed allowlist, wildcard requirements, and unapproved
registry or git sources. Multiple resolved crate versions remain warnings
because resolving Tauri ecosystem duplication is a separate maintenance task.

Run the same policy locally with:

```text
cargo deny --manifest-path src-tauri/Cargo.toml --locked --all-features check bans licenses sources
cargo deny --manifest-path src-tauri/Cargo.toml --locked --all-features check advisories
```

A passing result does **not** generate attribution notices, inspect binary
payloads, decide patent rights, or establish that a model and its training data
may be redistributed. Those checks remain part of the release inventory below.

### Current advisory baseline

The initial 2026-08-24 check found two vulnerabilities in `quick-xml 0.39.4`:
`RUSTSEC-2026-0194` (quadratic duplicate-attribute checks) and
`RUSTSEC-2026-0195` (unbounded namespace-declaration allocation). The advisory
fix is `quick-xml >=0.41.0`. Because `plist 1.9.0` constrained quick-xml to
`^0.39.2`, the lockfile was updated to `plist 1.10.0`, which resolves
`quick-xml 0.41.0`. A fresh cargo-deny scan no longer reports either
vulnerability.

Five upstream maintenance advisories remain, each at version `0.9.0` and each
reported by RustSec as having no safe direct upgrade:

| Advisory            | Package              | Dependency path                                              |
| ------------------- | -------------------- | ------------------------------------------------------------ |
| `RUSTSEC-2025-0081` | `unic-char-property` | `unic-char-property -> unic-ucd-ident -> urlpattern`          |
| `RUSTSEC-2025-0075` | `unic-char-range`    | `unic-char-range -> unic-ucd-ident -> urlpattern`             |
| `RUSTSEC-2025-0080` | `unic-common`        | `unic-common -> unic-ucd-version -> unic-ucd-ident`           |
| `RUSTSEC-2025-0100` | `unic-ucd-ident`     | `unic-ucd-ident -> urlpattern`                                |
| `RUSTSEC-2025-0098` | `unic-ucd-version`   | `unic-ucd-version -> unic-ucd-ident -> urlpattern`            |

For every row, the path continues through `urlpattern 0.3.0 -> tauri-utils
2.9.3 -> Tauri 2.11.5 -> DepoAudio`. Target-filtered cargo-deny checks reproduce
the findings for Apple silicon macOS, Intel macOS, and x64 Windows. `cargo tree
-e normal` includes the chain on macOS and Windows while `cargo tree -e build`
does not, so this is normal/runtime dependency debt rather than build-only debt.
That graph evidence does not prove that a particular unmaintained code path is
reachable from untrusted application input.

No advisory is silently ignored in `deny.toml`. CI reports the five current
maintenance findings as non-blocking so the new commercial-use policy does not
make the existing branch unmergeable. Remediate them through a compatible
Tauri/urlpattern dependency change, verify the affected code paths, and then
remove `continue-on-error` from the advisory step. Recheck the live advisory
database on every review because this baseline can change after the date above.

## Evidence record required for every distributed item

Keep one reviewed record per exact artifact. A record is complete only when it
contains all applicable fields:

1. component and exact version, revision, or release tag;
2. upstream owner and canonical source URL;
3. exact downloaded filename, byte size, and SHA-256 digest;
4. where the bytes enter the build and where they appear in the installed app;
5. SPDX license expression, backed by the license text from that exact source;
6. copyright, attribution, and third-party-notice files that must accompany it;
7. corresponding source and build configuration when a license requires them;
8. modification, patch, conversion, quantization, or export history;
9. for codecs, a separate patent/pool review appropriate to distribution
   territories (an open-source copyright license does not grant every patent);
10. for models, the weights license, code license, dataset terms, acceptable-use
    restrictions, and explicit redistribution/commercial-use permission;
11. delivery state: bundled, installer-added, downloaded on demand, CI-only, or
    not released;
12. release version, app commit, reviewer, review date, and evidence archive
    location.

Do not infer a license from a project name, model architecture, GitHub topic,
package-manager metadata alone, or an asset filename. If any required field is
unknown, record `Evidence required` and do not treat the item as cleared.

## Native and installer payloads observed in the release path

This table records repository evidence as of 2026-08-24. `Evidence required`
means the repository does not yet contain enough material for a distribution
conclusion.

| Component                                         | Exact repository evidence                                                                                                                                                                                      | Delivery                                                                         | License/notice status and required action                                                                                                                                                                                                                                          |
| ------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| FFmpeg and ffprobe, macOS arm64/x64               | `descriptinc/ffmpeg-ffprobe-static` release `b6.1.2-rc.1`; asset IDs `202650084`, `202650083`, `202650098`, `202650090`; expected sizes are pinned in `.github/workflows/release.yml`                          | Thin binaries are downloaded, checked, then combined/staged in the universal app | **Evidence required.** Archive exact SHA-256 values, FFmpeg version/configuration, enabled libraries/codecs, license texts, notices, and corresponding-source obligations for these four assets. Size and immutable GitHub asset ID are integrity controls, not license evidence.  |
| FFmpeg and ffprobe, Windows x64                   | `BtbN/FFmpeg-Builds` asset `ffmpeg-N-125875-g5d4d3bdc61-win64-gpl.zip`, release `autobuild-2026-07-31-14-10`, asset ID `496767003`, SHA-256 `68a5e966533002785c3e4b9a98327e21d5277802668bf889d94086cb6426cbb4` | `ffmpeg.exe` and `ffprobe.exe` are extracted into the installer                  | **Evidence required before distribution approval.** The asset name says `gpl`, which is a review trigger rather than a complete legal determination. Preserve the exact license/notices, build configuration, corresponding source or valid source offer, and codec/patent review. |
| ONNX Runtime, macOS universal2                    | Microsoft ONNX Runtime `v1.22.0`, asset `onnxruntime-osx-universal2-1.22.0.tgz`, asset ID `253300416`, expected size `54820264`                                                                                | `libonnxruntime.dylib` is copied into the signed app framework location          | **Evidence required.** Commit a reviewed digest and preserve the exact upstream license plus third-party notices from the archive; copying only the dylib must not discard required notices.                                                                                       |
| ONNX Runtime, Windows x64 CPU                     | Microsoft ONNX Runtime `v1.22.0`, asset `onnxruntime-win-x64-1.22.0.zip`, asset ID `253300435`, expected size `72368545`                                                                                       | `onnxruntime.dll` is copied into app resources                                   | **Evidence required.** Commit a reviewed digest and preserve the exact upstream license plus third-party notices from the archive; copying only the DLL must not discard required notices.                                                                                         |
| Microsoft Edge WebView2 Runtime offline installer | Tauri config selects `webviewInstallMode.type = "offlineInstaller"`; the exact redistributable version and bytes are not pinned in this repository                                                             | Added during Windows installer construction                                      | **Evidence required.** Capture the exact redistributable filename/version/hash and Microsoft redistribution terms from each release build. Verify whether notices or installer terms must be surfaced.                                                                             |
| Tauri-generated NSIS/WiX installer payloads       | Generated by the Tauri bundler; repository config selects both installer targets                                                                                                                               | Installer/bootstrapper material                                                  | **Evidence required.** Inventory an extracted final installer, including bundler-added libraries and redistributables, and retain the applicable notices for the exact toolchain versions.                                                                                         |
| FTR smoke fixture                                 | `https://samples.ffmpeg.org/ffmpeg-bugs/trac/ticket7279/t1.trm`, size `3775794`, SHA-256 `60993c1f5379d56d43f231ea5391504f9c808daf8978fba80e5ad60e81c4c4d7`                                                    | CI/release test input only; not configured as an app resource                    | **Evidence required for CI retention.** Record provenance and permission to retain/use this recording. Confirm from built artifacts that it is never packaged.                                                                                                                     |

The final installer audit is authoritative. Workflow inputs alone cannot prove
what a bundler, linker, framework packager, or bootstrapper placed in an
artifact.

## Model inventory observed in the app and `models-v1` workflow

The eight files named in `src-tauri/tauri.conf.json` are bundled resources.
`.github/workflows/restore-models.yml` can also publish nine hash-pinned files
to the `models-v1` release for the app's model manager. Hash verification proves
identity, not permission to redistribute.

| Model artifact            | SHA-256 pinned by the app                                          | Repository delivery evidence                                                                                             | Provenance/license status                                                                                                                                                                                                                |
| ------------------------- | ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `silero_vad.onnx`         | `a4a068cd6cf1ea8355b84327595838ca748ec29a25bc91fc82e6c299ccdc5808` | Bundled; copied from the current tree by the restore workflow                                                            | **Evidence required:** exact upstream model/revision, weights license, export history, training-data terms, notices, and commercial redistribution permission.                                                                           |
| `smart-turn-v3-int8.onnx` | `3d072c8fb04446955a365b533686e7e06015ad09929bb824b910c72ff89f5be1` | Bundled; copied from the current tree by the restore workflow                                                            | **Evidence required:** exact upstream model/revision, weights license, int8 conversion/export history, training-data terms, notices, and commercial redistribution permission.                                                           |
| `flashsr.onnx`            | `e255c76b227f16f7f392cc43677c38bd2c5aa129f042a2ba3eb03fb29e470c7a` | Bundled; copied from the current tree by the restore workflow                                                            | **Evidence required:** exact upstream model/revision, weights license, ONNX export history, training-data terms, notices, and commercial redistribution permission.                                                                      |
| `dfn3_enc.onnx`           | `7c5399d3da8a50ebef1c1a0ae421b33376aa5e45d0e92df16da7e83c9c131916` | Bundled but described in source as reserved/not used; copied by the restore workflow                                     | **Evidence required:** exact DeepFilterNet revision, weights license, export history, training-data terms, notices, and commercial redistribution permission. Unused status does not remove distribution obligations.                    |
| `dfn3_erb_dec.onnx`       | `ab669a1d10afe20911728b33053a452071042317a90581092b325da7b2f9d895` | Bundled but described in source as reserved/not used; copied by the restore workflow                                     | Same evidence required as `dfn3_enc.onnx`.                                                                                                                                                                                               |
| `dfn3_df_dec.onnx`        | `23114ce3b0f6464b763ee62f7bb8aab6b2a129a21eabd5bcfe59413db05f278a` | Bundled but described in source as reserved/not used; copied by the restore workflow                                     | Same evidence required as `dfn3_enc.onnx`.                                                                                                                                                                                               |
| `speaker_seg_int8.onnx`   | `d582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d` | Bundled; copied from the current tree by the restore workflow                                                            | **Evidence required:** exact upstream model/revision, weights license, quantization/export history, training-data terms, notices, and commercial redistribution permission.                                                              |
| `speaker_embed.onnx`      | `1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b` | Download-only; restored from app commit `b15b28c2d5c5872f073f30370b8facb203949b77`                                       | **Evidence required:** upstream provenance is not established by the historical app commit. Record the original model/revision, weights license, export history, training-data terms, notices, and commercial redistribution permission. |
| `dnsmos_sig_bak_ovr.onnx` | `269fbebdb513aa23cddfbb593542ecc540284a91849ac50516870e1ac78f6edd` | Download-only; restore workflow fetches `microsoft/DNS-Challenge` branch `master`, path `DNSMOS/DNSMOS/sig_bak_ovr.onnx` | **Evidence required:** replace the moving branch URL with an immutable source revision, then archive the exact weights license, repository notices, model/dataset terms, and commercial redistribution permission.                       |

`dfn3_config.ini` is also bundled and must be tied to the same reviewed
DeepFilterNet source revision and license record as the three DFN3 weight files.

## Release approval checklist

Before publishing or promoting a release:

- pass the blocking cargo-deny license, source, and bans checks against the
  committed lockfile, and review every advisory finding;
- produce a versioned Rust attribution/notice report from that exact lockfile;
- build installers only from pinned native inputs with recorded hashes;
- extract each final installer and inventory every executable, library, model,
  font, redistributable, and bundled notice;
- close every `Evidence required` entry for each artifact actually shipped or
  offered by the model manager;
- include required license and notice texts in the installed application and/or
  distribution channel, as the applicable licenses require;
- preserve corresponding source/build scripts or a compliant source offer when
  required, especially for the Windows FFmpeg build identified as `gpl`;
- archive the model provenance and modification chain next to the immutable
  model hashes;
- record codec patent review separately from copyright-license review;
- compare the extracted inventory on both macOS and Windows with this document;
- record the release tag, app commit, artifact hashes, reviewer, date, and any
  approved time-bounded exception.

Cargo, Node, native binaries, model weights, datasets, and authored product
assets are separate evidence domains. A green result in one domain must never
be presented as clearance for the others.
