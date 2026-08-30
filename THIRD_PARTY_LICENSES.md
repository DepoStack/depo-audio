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

A passing cargo-deny result does **not** itself generate attribution notices,
inspect binary payloads, decide patent rights, or establish that a model and its
training data may be redistributed. DepoAudio separately commits and bundles a
cargo-about report for the locked Rust dependency union across supported
release targets, including build dependencies. The release workflow regenerates
that report, rejects drift, and publishes target-specific Rust plus production
npm CycloneDX SBOMs with commit-bound timestamps and checksums. Those artifacts
remain evidence aids, not legal clearance for the other inventory below.

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

| Advisory            | Package              | Dependency path                                      |
| ------------------- | -------------------- | ---------------------------------------------------- |
| `RUSTSEC-2025-0081` | `unic-char-property` | `unic-char-property -> unic-ucd-ident -> urlpattern` |
| `RUSTSEC-2025-0075` | `unic-char-range`    | `unic-char-range -> unic-ucd-ident -> urlpattern`    |
| `RUSTSEC-2025-0080` | `unic-common`        | `unic-common -> unic-ucd-version -> unic-ucd-ident`  |
| `RUSTSEC-2025-0100` | `unic-ucd-ident`     | `unic-ucd-ident -> urlpattern`                       |
| `RUSTSEC-2025-0098` | `unic-ucd-version`   | `unic-ucd-version -> unic-ucd-ident -> urlpattern`   |

For every row, the path continues through `urlpattern 0.3.0 -> tauri-utils
2.9.3 -> Tauri 2.11.5 -> DepoAudio`. Target-filtered cargo-deny checks reproduce
the findings for Apple silicon macOS, Intel macOS, and x64 Windows. `cargo tree
-e normal` includes the chain on macOS and Windows while `cargo tree -e build`
does not, so this is normal/runtime dependency debt rather than build-only debt.
That graph evidence does not prove that a particular unmaintained code path is
reachable from untrusted application input.

`deny.toml` contains a narrow, time-bounded exception for only these five
INFO/unmaintained findings. Owner: Andrew Mayes. Review/remove by 2026-09-30 or
the first published Tauri release containing
[`tauri-apps/tauri#15660`](https://github.com/tauri-apps/tauri/pull/15660),
whichever comes first. DepoAudio configures no `Capability.remote` URL surface,
the Tauri path that uses `urlpattern`; the dependency is still distributed, so
this is maintenance-debt acceptance rather than removal. The advisory job is
blocking: any finding outside the five exact IDs fails CI. Recheck the live
database on every review because this baseline can change after the date above.

## JavaScript and generated CSS notice status

The reviewed production closure contains 57 JavaScript packages plus Tailwind
CSS and tailwindcss-animate (59 components total). A deterministic generator
commits and bundles byte-stable HTML and JSON notice evidence from the lockfile
and reviewed overrides. The release workflow also generates a production npm
CycloneDX inventory, but an SBOM does not replace copyright notices or license
texts.

The exact audit found one published helper whose archive had no complete legal
file: `react-remove-scroll-bar@2.3.8`. DepoAudio does not infer an exact notice
from package metadata. Vite instead maps the helper's two emitted import paths
to a small, independently authored, DepoAudio-owned compatibility module. The
production graph therefore contains no bytes from that package while retaining
Radix and `react-remove-scroll` focus, keyboard, inertness, and scroll-isolation
behavior. Focused tests cover nested locks, fractional scrollbar gaps,
pre-existing body styles, and React Strict Mode cleanup.

The remaining reviewed notice cases are:

- `@tauri-apps/plugin-dialog@2.7.2`, `plugin-fs@2.5.1`,
  `plugin-opener@2.5.4`, `plugin-process@2.3.1`, `plugin-shell@2.3.5`, and
  `plugin-updater@2.10.1` ship SPDX declarations but not the complete MIT and
  Apache-2.0 texts. Preserve those declarations and add the exact upstream
  license texts.
- `tslib@2.8.1` includes a separate `CopyrightNotice.txt`; preserve it beside
  `LICENSE.txt`.
- The generated CSS retains Tailwind CSS 4.3.3's legal banner and includes
  output from `tailwindcss-animate@1.0.7`; bundle both complete licenses.

`scripts/generate-javascript-notices.mjs` is driven by the lockfile plus a
reviewed override manifest. It collects applicable `LICENSE*`, `NOTICE*`, and
`COPYRIGHT*` artifacts, sorts output deterministically, rejects checkout paths
and timestamps, and supports a strict review gate. Normal generation,
self-tests, and strict mode now pass with zero unresolved components. The
release blocker remains open until the exact generated report and component
registry are verified byte-for-byte in the DMG, app archive, MSI, and NSIS
inventories.

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

This table records repository evidence as of 2026-08-29. `Evidence required`
means the repository does not yet contain enough material for a distribution
conclusion.

| Component                                         | Exact repository evidence                                                                                                                                                                                                                                                                                                                                                                   | Delivery                                                                                                                                                                                                                                                                                   | License/notice status and required action                                                                                                                                                                                                                                                                                                             |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| FFmpeg and ffprobe, macOS arm64/x64               | Official FFmpeg `7.1.5`, LAME `3.100`, and Opus `1.6.1` source archives; exact URLs, sizes, and SHA-256 values are pinned in `.github/workflows/release.yml`                                                                                                                                                                                                                                | `scripts/build-ffmpeg-macos.sh` builds both slices with the same minimum macOS target, disables GPL/nonfree/autodetection/network, enables only LGPL-compatible external libraries, verifies FTR and every released encoder/filter, and preserves source/build/license evidence in the app | **RC2 evidence required.** The previous community binaries enabled FFmpeg's `nonfree`/unredistributable mode and mixed FFmpeg 6.1.1 with 7.1; they are prohibited. Verify the new hosted build, source hashes, exact configuration, LGPL/LAME/Opus notices, system-only linkage, corresponding-source delivery, and codec/patent review.              |
| FFmpeg and ffprobe, Windows x64                   | `BtbN/FFmpeg-Builds` LGPL asset `ffmpeg-N-125875-g5d4d3bdc61-win64-lgpl.zip`, release `autobuild-2026-07-31-14-10`, asset ID `496767001`, size `147672337`, SHA-256 `5d65df0c0ca5346d82df8ade9c2e12db45d1f978f18ff908b42f03f5223dfc90`                                                                                                                                                      | `ffmpeg.exe` and `ffprobe.exe` are extracted; the archive's exact LGPL text, binary-reported configuration, release URL, asset digest, and upstream FFmpeg commit are bundled                                                                                                              | The release workflow requires native FTR decoding and the released WAV, MP3, FLAC, Opus, and AAC/M4A encoders. **Final evidence required:** prove those checks on the exact candidate, verify the bundled notice/configuration/source files after MSI and NSIS extraction, establish corresponding-source delivery, and complete codec/patent review. |
| Microsoft Edge WebView2 Runtime offline installer | Tauri config selects `webviewInstallMode.type = "offlineInstaller"`. Tauri CLI 2.11.4 / bundler 2.9.4 resolves a moving Microsoft fwlink independently for WiX and NSIS, so the repository lockfile cannot pin the downloaded bytes. `scripts/windows-webview2-evidence.ps1` fails unless both resolved build inputs are byte-identical and carry a valid Microsoft Authenticode signature. | Added during Windows installer construction                                                                                                                                                                                                                                                | **Final evidence required.** Retain the generated filename/version/size/SHA-256/signature report, extract the final MSI Binary table and NSIS payload to match that digest, and review the exact Microsoft redistribution terms. The build-input report deliberately does not claim final-installer extraction.                                       |
| Tauri-generated NSIS/WiX installer payloads       | Generated by the Tauri bundler; repository config selects both installer targets                                                                                                                                                                                                                                                                                                            | Installer/bootstrapper material                                                                                                                                                                                                                                                            | **Evidence required.** Inventory an extracted final installer, including bundler-added libraries and redistributables, and retain the applicable notices for the exact toolchain versions.                                                                                                                                                            |
| Generated synthetic FTR smoke fixture             | `scripts/ftr-smoke-fixture.mjs` creates a deterministic mono 440 Hz PCM WAV, uses the exact candidate FFmpeg to encode AAC and mux AVI with audio tag `0x1610`, then changes that tag to FTR `0x4180` for the normal decoder path. No recording is downloaded or supplied by a user.                                                                                                        | Transient CI/release test input under `RUNNER_TEMP/depoaudio-ftr-smoke`; the fixture and every derived output are removed before packaging and are not configured as app resources                                                                                                         | No third-party recording content is present. **Candidate evidence required:** prove the exact macOS and Windows candidate sidecars decode the generated fixture, cleanup completes, and final artifact inventories contain neither `.trm` fixture bytes nor derived smoke outputs.                                                                    |

The final installer audit is authoritative. Workflow inputs alone cannot prove
what a bundler, linker, framework packager, or bootstrapper placed in an
artifact.

## Historical model inventory and v1.0.3 non-distribution boundary

v1.0.3 distributes and exposes zero learned models. Its resource manifest has
no model directory, the resolved Rust graph excludes `nnnoiseless` and ONNX
Runtime, the command/UI surface has no install or download path, and legacy
app-data files are never resolved or executed. An exact allowlisted regular
file can be shown only so the user can delete it. Final installer extraction
must independently prove this source contract.

The public `models-v1` release retains nine files because already-published
v1.0.2 can reference that compatibility contract. The read-only restoration
workflow reconstructs and verifies those historical bytes from immutable
sources; it cannot create, upload, replace, or delete a public asset. The
rights findings below therefore remain an owner-governance issue for existing
public history, not a v1.0.3 capability or distribution path. Hash verification
proves identity, not permission to redistribute.

### Exact historical findings (reviewed 2026-08-29)

- **FlashSR was not cleared for commercial distribution.** The historical
  ONNX is pinned to
  [YatharthS/FlashSR revision `3e19cc92`](https://huggingface.co/YatharthS/FlashSR/blob/3e19cc92e655c2e0661e6268efdade60f42fd0b8/onnx/model.onnx).
  Its SpeechSR-48k lineage is byte-linked to the
  [HierSpeech++ weight at `5a91f947`](https://github.com/sh-lee-prml/HierSpeechpp/blob/5a91f94744651aba73711ebe06d11cab31efa2fd/speechsr48k/G_100000.pth),
  whose [same-revision documentation](https://github.com/sh-lee-prml/HierSpeechpp/blob/5a91f94744651aba73711ebe06d11cab31efa2fd/README.md)
  names Expresso as training data. Meta's
  [immutable Expresso dataset record](https://github.com/facebookresearch/textlesslib/blob/a4329612e1f712af5961da01b612d7b5a5fcbb1a/examples/expresso/dataset/README.md#license)
  states CC BY-NC 4.0. Under this repository's evidence policy, that is a direct
  commercial-governance conflict until applicable rights are documented or the
  model is replaced/removed.
- **Speaker segmentation was not cleared for commercial distribution.** The
  [exact ONNX conversion at `9403a690`](https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/tree/9403a6902bb58e3d5ae8c7e77c3422de279db2e0)
  derives from pyannote segmentation 3.0. Its
  [immutable model card](https://huggingface.co/pyannote/segmentation-3.0/blob/e66f3d3b9eb0873085418a7b813d3b369bf160bb/README.md)
  names nine training corpora without versions, splits, or the applicable
  rights chain. The published
  [MSDWild license agreement](https://github.com/X-LANCE/MSDWILD/blob/b9f97946f7001e86f643d893811fbded918407bc/MSDWILD_license_agreement.pdf)
  prohibits commercial use and redistribution; DIHARD and
  [REPERE](https://catalog.elda.org/en-us/repository/browse/ELRA-E0044/)
  publish separate commercial terms. This does not prove which permissions the
  trainer held; it means the redistribution rights chain required by this gate
  is undocumented.
- **Smart Turn remains unverified.** The historical export is pinned to
  [revision `dbd03130`](https://huggingface.co/onnx-community/smart-turn-v3-ONNX/blob/dbd03130e87854011fa8c2f203077f2cbfc3adad/onnx/model_quantized.onnx),
  while the exact published
  [training](https://huggingface.co/datasets/pipecat-ai/smart-turn-data-v3.1-train/tree/d1691dd73ec827334a98b9245c47f8f2f0bac935)
  and [test](https://huggingface.co/datasets/pipecat-ai/smart-turn-data-v3.1-test/tree/2a9377baf2bbc73ba176c4505fe4adf988288fe9)
  dataset revisions expose no license field. The applicable Whisper Tiny notice
  and complete export/training record must also be preserved.
- **Silero VAD remains unverified.** The historical community conversion is pinned
  to [revision `e71cae96`](https://huggingface.co/onnx-community/silero-vad/blob/e71cae966052b992a7eca6b17738916ce0eca4ec/onnx/model.onnx),
  but it is not byte-identical to the identified official Silero v5 artifact.
  The exact export lineage, training-data terms, notices, and commercial
  redistribution permission remain incomplete.
- **DNSMOS has pinned source bytes but incomplete model-specific clearance.**
  The historical optional weight comes from
  [Microsoft revision `82f1b17e`](https://github.com/microsoft/DNS-Challenge/blob/82f1b17e7776a43eee395d0f45bae8abb700ad00/DNSMOS/DNSMOS/sig_bak_ovr.onnx).
  Repository content/code licenses are available, but the exact model's
  training, notice, and commercial-redistribution record is not yet complete.

In the historical delivery column below, `Bundled` and `Download-only` describe
v1.0.2 behavior. None of these files is bundled, downloaded, or executed by
v1.0.3.

| Model artifact            | Reviewed SHA-256                                                   | Repository delivery evidence                                                                                                                                                                                                                                                              | Provenance/license status                                                                                                                                                                                                                                                     |
| ------------------------- | ------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `silero_vad.onnx`         | `a4a068cd6cf1ea8355b84327595838ca748ec29a25bc91fc82e6c299ccdc5808` | Bundled; exact community-conversion revision is linked above                                                                                                                                                                                                                              | **Evidence required:** the identified upstream official artifact is not byte-identical. Establish the exact export lineage, weights/training-data terms, notices, and commercial redistribution permission.                                                                   |
| `smart-turn-v3-int8.onnx` | `3d072c8fb04446955a365b533686e7e06015ad09929bb824b910c72ff89f5be1` | Bundled; exact ONNX and dataset revisions are linked above                                                                                                                                                                                                                                | **Evidence required:** the exact train/test revisions publish no license field. Preserve applicable dataset/Whisper notices and establish the conversion, training-data, and commercial redistribution rights chain.                                                          |
| `flashsr.onnx`            | `e255c76b227f16f7f392cc43677c38bd2c5aa129f042a2ba3eb03fb29e470c7a` | Bundled; exact ONNX, HierSpeech++ lineage, and Expresso source are linked above                                                                                                                                                                                                           | **Blocked:** Expresso is documented as CC BY-NC 4.0 training data. Record applicable commercial rights or replace/remove the model before publication; an MIT code or wrapper license does not resolve that conflict.                                                         |
| `dfn3_enc.onnx`           | `7c5399d3da8a50ebef1c1a0ae421b33376aa5e45d0e92df16da7e83c9c131916` | Legacy `models-v1` asset and bundled through v1.0.2; removed from the v1.0.3 bundle and install catalog                                                                                                                                                                                   | **Evidence required for the retained legacy public asset:** exact DeepFilterNet revision, weights license, export history, training-data terms, notices, and commercial redistribution permission.                                                                            |
| `dfn3_erb_dec.onnx`       | `ab669a1d10afe20911728b33053a452071042317a90581092b325da7b2f9d895` | Legacy `models-v1` asset and bundled through v1.0.2; removed from the v1.0.3 bundle and install catalog                                                                                                                                                                                   | Same evidence required as `dfn3_enc.onnx`.                                                                                                                                                                                                                                    |
| `dfn3_df_dec.onnx`        | `23114ce3b0f6464b763ee62f7bb8aab6b2a129a21eabd5bcfe59413db05f278a` | Legacy `models-v1` asset and bundled through v1.0.2; removed from the v1.0.3 bundle and install catalog                                                                                                                                                                                   | Same evidence required as `dfn3_enc.onnx`.                                                                                                                                                                                                                                    |
| `speaker_seg_int8.onnx`   | `d582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d` | Bundled; exact converter and pyannote revisions are linked above                                                                                                                                                                                                                          | **Blocked:** MSDWild terms prohibit commercial use/redistribution and other named corpora have separate terms. Document the exact versions and applicable rights chain, or replace/remove the model. Do not infer that the trainer lacked permissions.                        |
| `speaker_embed.onnx`      | `1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b` | Legacy `models-v1` asset; removed from the v1.0.3 install catalog. An already-downloaded app-data copy is shown only as removable legacy storage.                                                                                                                                         | **Evidence required for the retained legacy public asset:** upstream provenance is not established by the historical app commit. Record the original model/revision, weights license, export history, training-data terms, notices, and commercial redistribution permission. |
| `dnsmos_sig_bak_ovr.onnx` | `269fbebdb513aa23cddfbb593542ecc540284a91849ac50516870e1ac78f6edd` | Download-only; restore workflow fetches path `DNSMOS/DNSMOS/sig_bak_ovr.onnx` from immutable Microsoft commit `82f1b17e7776a43eee395d0f45bae8abb700ad00`. The fetched file is 1,157,965 bytes and matches the app pin. The repository root at that commit contains a CC BY 4.0 `LICENSE`. | **Evidence required:** immutable byte identity is established, but repository-level licensing alone does not prove the terms applicable to this exact weight. Archive the applicable model/dataset terms, attribution, and commercial redistribution review before approval.  |

`dfn3_config.ini` was bundled through v1.0.2 and is removed from the v1.0.3
bundle with the three inactive weights. The historical/public DeepFilterNet
distribution record still requires the same reviewed source and license
evidence; removing it from a new installer does not rewrite that history.

## Release approval checklist

Before publishing or promoting a release:

- pass the blocking cargo-deny license, source, and bans checks against the
  committed lockfile, and review every advisory finding;
- produce a versioned Rust attribution/notice report from that exact lockfile;
- build installers only from pinned native inputs with recorded hashes;
- extract each final installer and inventory every executable, library, model,
  font, redistributable, and bundled notice;
- close every `Evidence required` entry for each artifact actually shipped;
- prove the resolved dependency graph, app resources, and extracted installers
  contain no learned-model weight, known model hash, ONNX Runtime library,
  model download URL, or learned execution command;
- include required license and notice texts in the installed application and/or
  distribution channel, as the applicable licenses require;
- preserve corresponding source/build scripts or a compliant source offer when
  required for each FFmpeg/LAME/Opus distribution;
- preserve the historical v1.0.2 model provenance record next to the immutable
  model hashes and record the owner's separate remediation decision;
- record codec patent review separately from copyright-license review;
- compare the extracted inventory on both macOS and Windows with this document;
- confirm the v1.0.3 inventories contain no learned-model bytes, generated FTR
  fixture bytes, or derived smoke-test outputs;
- record the release tag, app commit, artifact hashes, reviewer, date, and any
  approved time-bounded exception.

Cargo, Node, native binaries, model weights, datasets, and authored product
assets are separate evidence domains. A green result in one domain must never
be presented as clearance for the others.
