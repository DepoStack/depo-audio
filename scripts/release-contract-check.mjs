import { existsSync, readFileSync } from 'node:fs'

const read = path => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8')
const parseJson = path => JSON.parse(read(path))
const failures = []
const expect = (condition, message) => {
  if (!condition) failures.push(message)
}

const packageJson = parseJson('package.json')
const packageLock = parseJson('package-lock.json')
const tauriConfig = parseJson('src-tauri/tauri.conf.json')
const cargoToml = read('src-tauri/Cargo.toml')
const cargoLock = read('src-tauri/Cargo.lock')
const changelog = read('CHANGELOG.md')
const thirdPartyInventory = read('THIRD_PARTY_LICENSES.md')
const releaseCandidate = read('docs/V1.0.3-RELEASE-CANDIDATE.md')
const releaseGate = parseJson('docs/V1.0.3-RELEASE-GATE.json')
const denyPolicy = read('deny.toml')
const ciWorkflow = read('.github/workflows/ci.yml')
const releaseWorkflow = read('.github/workflows/release.yml')
const promoteDraftWorkflow = read('.github/workflows/promote-draft.yml')
const publishReleaseWorkflow = read('.github/workflows/publish-release.yml')
const restoreModelsWorkflow = read('.github/workflows/restore-models.yml')
const artifactInventory = read('scripts/release-artifact-inventory.mjs')
const modelDistributionVerifier = read('scripts/verify-model-distribution.mjs')
const javascriptNoticeGenerator = read('scripts/generate-javascript-notices.mjs')
const javascriptNoticeComponents = parseJson('src-tauri/resources/third-party/javascript/COMPONENTS.json')
const javascriptNoticeHtml = read('src-tauri/resources/third-party/javascript/THIRD-PARTY-NOTICES.html')
const privateFtrFixture = read('scripts/private-ftr-fixture.mjs')
const webviewEvidence = read('scripts/windows-webview2-evidence.ps1')
const macFfmpegBuild = read('scripts/build-ffmpeg-macos.sh')
const macPackagedSmoke = read('scripts/macos-packaged-smoke.sh')
const windowsPackagedSmoke = read('scripts/windows-packaged-smoke.ps1')
const setupDev = read('scripts/setup-dev.sh')
const gitIgnore = read('.gitignore')
const exportDccrnPresent = existsSync(new URL('../scripts/export_dccrn.py', import.meta.url))
const draftAssetUpload = read('scripts/upload-draft-release-assets.mjs')
const publicationHelper = read('scripts/prepare-release-publication.mjs')
const aboutPolicy = read('about.toml')
const aboutTemplate = read('scripts/third-party-licenses.hbs')
const rustLicenseReport = read('src-tauri/resources/third-party/rust/THIRD-PARTY-LICENSES.html')
const sbomNormalizer = read('scripts/normalize-release-sbom.mjs')
const cargoAboutNormalizer = read('scripts/normalize-cargo-about-report.mjs')
const appConstants = read('src/constants.js')
const settingsPanel = read('src/components/SettingsPanel.jsx')
const installerLicense = read('src-tauri/installer/license.rtf')
const tauriLib = read('src-tauri/src/lib.rs')
const modelsSource = read('src-tauri/src/models.rs')
const canonicalReleaseUrl = 'https://github.com/DepoStack/depo-audio/releases'
const configuredReleaseUrl = appConstants.match(/^export const DEPOAUDIO_RELEASES_URL = ['"]([^'"]+)['"]$/m)?.[1]
const installerReleaseUrl = installerLicense.match(/^(https:\/\/[^\s]+)\. DepoAudio's MIT license does\\par$/m)?.[1]
expect(!process.argv.includes('--published'), 'use --publication-ready; --published no longer describes this check')
const requiresPublicationReadyHeading = process.argv.includes('--publication-ready')
const candidateIsGo = releaseCandidate.includes('Status: **GO for publication**')
const candidateIsNoGo = releaseCandidate.includes('Status: **NO-GO for publication; private RC evidence only**')

const advisoryExceptionIds = [
  'RUSTSEC-2025-0081',
  'RUSTSEC-2025-0075',
  'RUSTSEC-2025-0080',
  'RUSTSEC-2025-0100',
  'RUSTSEC-2025-0098',
]
for (const advisory of advisoryExceptionIds) {
  expect(denyPolicy.includes(`"${advisory}"`), `deny.toml must explicitly scope advisory exception ${advisory}`)
}
expect(
  denyPolicy.includes('Review/remove by: 2026-09-30') &&
    denyPolicy.includes('Owner: Andrew Mayes') &&
    Date.now() <= Date.parse('2026-09-30T23:59:59Z'),
  'The rust-unic maintenance exception must have a current named owner and unexpired review date',
)
expect(
  ciWorkflow.includes('name: Enforce Rust advisory policy') && !ciWorkflow.includes('continue-on-error: true'),
  'Rust advisory policy must block CI outside the exact deny.toml exceptions',
)

const version = packageJson.version
const escapedVersion = version.replaceAll('.', '\\.')
const cargoPackageVersion = cargoToml.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1]
const cargoLockVersion = cargoLock.match(/^\[\[package\]\]\s*\r?\nname = "depo-audio"\s*\r?\nversion = "([^"]+)"/m)?.[1]
const isIsoDate = value => {
  if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return false
  const parsed = new Date(`${value}T00:00:00Z`)
  return !Number.isNaN(parsed.valueOf()) && parsed.toISOString().slice(0, 10) === value
}
const isTauriUpdaterPublicKey = value => {
  if (typeof value !== 'string' || !/^[A-Za-z0-9+/]+={0,2}$/.test(value)) return false
  try {
    const lines = Buffer.from(value, 'base64').toString('utf8').trim().split(/\r?\n/)
    if (lines.length !== 2 || !lines[0].startsWith('untrusted comment: ')) return false
    const key = Buffer.from(lines[1], 'base64')
    return key.length === 42 && ['Ed', 'ED'].includes(key.subarray(0, 2).toString('ascii'))
  } catch {
    return false
  }
}
const candidateReviewedDate = releaseCandidate.match(/^Reviewed: (\d{4}-\d{2}-\d{2})$/m)?.[1]
const candidateStatus = candidateIsGo ? 'GO' : candidateIsNoGo ? 'NO-GO' : undefined
const gateBlockers = Array.isArray(releaseGate.blockers) ? releaseGate.blockers : []
const gateBlockerIds = gateBlockers.map(blocker => blocker?.id)
const gateBlockerById = new Map(gateBlockers.map(blocker => [blocker?.id, blocker]))
const openGateBlockers = gateBlockers.filter(blocker => blocker?.status === 'open')
const gateCandidate = releaseGate.candidate ?? {}

expect(releaseGate.schemaVersion === 2, 'the v1.0.3 release gate must use schemaVersion 2')
expect(releaseGate.version === version, 'the release-gate version must match package.json')
expect(isIsoDate(releaseGate.reviewedDate), 'the release gate must carry a valid reviewedDate')
expect(
  candidateReviewedDate === releaseGate.reviewedDate,
  'the release gate and release-candidate document must carry the same reviewed date',
)
expect(['GO', 'NO-GO'].includes(releaseGate.status), 'the release gate must carry an explicit GO or NO-GO status')
expect(candidateStatus === releaseGate.status, 'the machine and human release-candidate statuses must agree')
expect(gateCandidate.tag === `v${version}`, 'the release gate candidate tag must match the app version')
expect(
  gateCandidate.releaseId === null || Number.isSafeInteger(gateCandidate.releaseId),
  'the release gate candidate releaseId must be null or an exact numeric GitHub release ID',
)
expect(
  gateCandidate.sourceCommit === null || /^[0-9a-f]{40}$/.test(gateCandidate.sourceCommit),
  'the release gate candidate sourceCommit must be null or a full commit SHA',
)
expect(
  gateCandidate.assetManifestSha256 === null || /^[0-9a-f]{64}$/.test(gateCandidate.assetManifestSha256),
  'the release gate candidate assetManifestSha256 must be null or a SHA-256 digest',
)
expect(
  ['undecided', 'signed', 'unavailable'].includes(gateCandidate.updater),
  'the release gate candidate updater decision must be undecided, signed, or unavailable',
)
expect(
  gateCandidate.updater === 'signed'
    ? isTauriUpdaterPublicKey(gateCandidate.updaterPublicKey)
    : gateCandidate.updaterPublicKey === null,
  'the release gate must carry an approved Tauri updater public key only for a signed updater candidate',
)
expect(
  gateCandidate.publicationDate === null || isIsoDate(gateCandidate.publicationDate),
  'the release gate candidate publicationDate must be null or an ISO date',
)
expect(
  gateCandidate.approvedBy === null ||
    (typeof gateCandidate.approvedBy === 'string' && gateCandidate.approvedBy.trim().length > 0),
  'the release gate candidate approvedBy must be null or a named accountable reviewer',
)
expect(
  gateBlockerIds.length === new Set(gateBlockerIds).size,
  'the release gate must not contain duplicate blocker IDs',
)
expect(Array.isArray(releaseGate.blockers), 'the release gate must contain a blockers array')
for (const blocker of gateBlockers) {
  expect(
    typeof blocker?.id === 'string' && /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(blocker.id),
    'every release blocker must have a stable kebab-case ID',
  )
  expect(
    ['open', 'closed'].includes(blocker?.status),
    `release blocker ${blocker?.id ?? '<unknown>'} has invalid status`,
  )
  expect(blocker?.version === version, `release blocker ${blocker?.id ?? '<unknown>'} must match version ${version}`)
  expect(
    blocker?.reviewedDate === releaseGate.reviewedDate && isIsoDate(blocker?.reviewedDate),
    `release blocker ${blocker?.id ?? '<unknown>'} must carry the gate reviewed date`,
  )
  expect(
    Array.isArray(blocker?.evidenceNeeded) &&
      blocker.evidenceNeeded.length > 0 &&
      blocker.evidenceNeeded.every(item => typeof item === 'string' && item.trim().length > 0),
    `release blocker ${blocker?.id ?? '<unknown>'} must list concrete evidenceNeeded`,
  )
  if (blocker?.status === 'closed') {
    expect(
      isIsoDate(blocker?.closedDate) && blocker.closedDate <= releaseGate.reviewedDate,
      `closed release blocker ${blocker?.id ?? '<unknown>'} must carry a valid closedDate no later than the review date`,
    )
    expect(
      typeof blocker?.closedBy === 'string' && blocker.closedBy.trim().length > 0,
      `closed release blocker ${blocker?.id ?? '<unknown>'} must identify the accountable reviewer`,
    )
    expect(
      Array.isArray(blocker?.evidenceRefs) &&
        blocker.evidenceRefs.length > 0 &&
        blocker.evidenceRefs.every(item => typeof item === 'string' && item.trim().length > 0),
      `closed release blocker ${blocker?.id ?? '<unknown>'} must retain nonempty evidenceRefs`,
    )
  }
}
expect(
  (releaseGate.status === 'GO' && openGateBlockers.length === 0) ||
    (releaseGate.status === 'NO-GO' && openGateBlockers.length > 0),
  'GO requires zero open blockers; NO-GO requires at least one open blocker',
)
if (requiresPublicationReadyHeading) {
  expect(releaseGate.status === 'GO', 'the machine-readable release gate must be GO before publication')
  expect(openGateBlockers.length === 0, 'every machine-readable release blocker must be closed before publication')
  expect(
    Number.isSafeInteger(gateCandidate.releaseId) && gateCandidate.releaseId > 0,
    'publication requires the exact reviewed private draft release ID',
  )
  expect(/^[0-9a-f]{40}$/.test(gateCandidate.sourceCommit), 'publication requires the exact candidate source commit')
  expect(
    /^[0-9a-f]{64}$/.test(gateCandidate.assetManifestSha256),
    'publication requires the reviewed candidate asset-manifest SHA-256',
  )
  expect(['signed', 'unavailable'].includes(gateCandidate.updater), 'publication requires an updater decision')
  expect(
    gateCandidate.updater === 'signed'
      ? isTauriUpdaterPublicKey(gateCandidate.updaterPublicKey)
      : gateCandidate.updaterPublicKey === null,
    'publication requires updater-key state consistent with the approved updater decision',
  )
  expect(isIsoDate(gateCandidate.publicationDate), 'publication requires the actual release date')
  expect(
    changelog.includes(`## [${version}] - ${gateCandidate.publicationDate}`),
    'publication requires the changelog heading to match the approved publication date exactly',
  )
  expect(
    typeof gateCandidate.approvedBy === 'string' && gateCandidate.approvedBy.trim().length > 0,
    'publication requires a named accountable reviewer',
  )
}

expect(packageLock.version === version, 'package-lock.json top-level version must match package.json')
expect(
  packageLock.packages?.['']?.version === version,
  'package-lock.json root package version must match package.json',
)
expect(tauriConfig.version === version, 'tauri.conf.json version must match package.json')
expect(cargoPackageVersion === version, 'Cargo.toml package version must match package.json')
expect(cargoLockVersion === version, 'Cargo.lock depo-audio version must match package.json')

const forbiddenModels = [
  'dccrn_plus.onnx',
  'dfn3_config.ini',
  'dfn3_df_dec.onnx',
  'dfn3_enc.onnx',
  'dfn3_erb_dec.onnx',
  'dnsmos_sig_bak_ovr.onnx',
  'flashsr.onnx',
  'silero_vad.onnx',
  'smart-turn-v3-int8.onnx',
  'speaker_embed.onnx',
  'speaker_seg_int8.onnx',
  'weights.rnn',
]
const forbiddenModelHashes = [
  '1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b',
  '23114ce3b0f6464b763ee62f7bb8aab6b2a129a21eabd5bcfe59413db05f278a',
  '269fbebdb513aa23cddfbb593542ecc540284a91849ac50516870e1ac78f6edd',
  '3d072c8fb04446955a365b533686e7e06015ad09929bb824b910c72ff89f5be1',
  '7c5399d3da8a50ebef1c1a0ae421b33376aa5e45d0e92df16da7e83c9c131916',
  'a4a068cd6cf1ea8355b84327595838ca748ec29a25bc91fc82e6c299ccdc5808',
  'ab669a1d10afe20911728b33053a452071042317a90581092b325da7b2f9d895',
  'd582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d',
  'e255c76b227f16f7f392cc43677c38bd2c5aa129f042a2ba3eb03fb29e470c7a',
  'e6de5fbfadf7ec91d1b24d6a6ccfd0290cb4d8bf555c5eab3ce41506f67a58b1',
]
const bundleResources = tauriConfig.bundle?.resources ?? []
expect(
  !bundleResources.some(resource => /(?:^|\/)(?:model|models|onnxruntime)(?:\/|$)|\.onnx$/i.test(resource)),
  'tauri.conf.json must not bundle learned-model material or ONNX Runtime',
)
expect(
  modelsSource.includes('legacy_model_cleanup_catalog') &&
    modelsSource.includes('delete_legacy_model') &&
    !modelsSource.includes('download_model') &&
    !modelsSource.includes('reqwest::') &&
    !modelsSource.includes('http://') &&
    !modelsSource.includes('https://'),
  'models.rs must expose only deletion-only legacy cleanup without a network installation path',
)
expect(
  modelsSource.includes('recommended_denoise: "unavailable".into()') &&
    modelsSource.includes('recommend_speaker_detection: false') &&
    modelsSource.includes('recommend_enhance: false') &&
    modelsSource.includes('dereverb_available: false'),
  'v1.0.3 system capabilities must fail closed for every learned-model feature',
)
for (const filename of forbiddenModels) {
  expect(artifactInventory.includes(`'${filename}'`), `artifact inventory must reject model material ${filename}`)
  expect(
    modelDistributionVerifier.includes(`'${filename}'`),
    `source distribution verifier must reject model material ${filename}`,
  )
}
for (const hash of forbiddenModelHashes) {
  expect(artifactInventory.includes(`'${hash}'`), `artifact inventory must reject known model bytes ${hash}`)
  expect(modelDistributionVerifier.includes(`'${hash}'`), `source verifier must reject known model bytes ${hash}`)
}
expect(
  packageJson.scripts['models:check'] === 'node scripts/verify-model-distribution.mjs' &&
    packageJson.scripts['models:self-test'] === 'node scripts/verify-model-distribution.mjs --self-test' &&
    packageJson.scripts['release:check'].includes('npm run models:check') &&
    packageJson.scripts['release:check'].includes('npm run models:self-test') &&
    modelDistributionVerifier.includes('Model distribution contract verified') &&
    modelDistributionVerifier.includes('Model distribution contract self-test OK') &&
    ciWorkflow.includes('npm run models:check') &&
    releaseWorkflow.includes('npm run models:check') &&
    !releaseWorkflow.toLowerCase().includes('onnxruntime'),
  'source, CI, release, and negative-test contracts must enforce the zero learned-model boundary',
)
expect(
  !setupDev.toLowerCase().includes('onnxruntime') &&
    !setupDev.includes('ORT_DYLIB_PATH') &&
    !macPackagedSmoke.includes('ORT_DYLIB_PATH') &&
    !windowsPackagedSmoke.includes('ORT_DYLIB_PATH') &&
    !macPackagedSmoke.includes('ort_loads_and_runs_silero_vad') &&
    !windowsPackagedSmoke.includes('ort_loads_and_runs_silero_vad') &&
    !exportDccrnPresent &&
    !gitIgnore.includes('resources/onnxruntime'),
  'active setup, packaged smoke, export, and ignore paths must not restore learned-model runtime material',
)

const unreleasedPosition = changelog.indexOf('## [Unreleased]')
const datedReleaseHeading = new RegExp(`^## \\[${escapedVersion}\\] - \\d{4}-\\d{2}-\\d{2}$`, 'm')
const releaseHeading = requiresPublicationReadyHeading
  ? datedReleaseHeading
  : new RegExp(`^## \\[${escapedVersion}\\] - (?:\\d{4}-\\d{2}-\\d{2}|Unpublished)$`, 'm')
const releaseMatch = changelog.match(releaseHeading)
expect(unreleasedPosition >= 0, 'CHANGELOG.md must retain an Unreleased heading')
expect(
  releaseMatch,
  requiresPublicationReadyHeading
    ? `CHANGELOG.md must contain a dated ${version} release heading before publication`
    : `CHANGELOG.md must contain a dated or explicitly Unpublished ${version} heading`,
)
if (releaseMatch) {
  expect(
    unreleasedPosition < releaseMatch.index,
    'The Unreleased changelog heading must precede the current release heading',
  )
}
expect(
  requiresPublicationReadyHeading
    ? candidateIsGo
    : releaseCandidate.includes('Status: **NO-GO for publication; private RC evidence only**') ||
        releaseCandidate.includes('Status: **GO for publication**'),
  requiresPublicationReadyHeading
    ? 'the reviewed release-candidate status must be GO before publication'
    : 'the release-candidate document must carry an explicit GO or NO-GO status',
)

expect(
  !releaseWorkflow.includes('includeUpdaterJson:'),
  'release.yml uses an invalid tauri-action input: includeUpdaterJson',
)
expect(
  !releaseWorkflow.includes('uploadUpdaterJson:') && !releaseWorkflow.includes('uploadUpdaterSignatures:'),
  'build-only tauri-action steps must never upload updater metadata or signatures directly',
)
expect(
  releaseWorkflow.includes('createUpdaterArtifacts: true') &&
    releaseWorkflow.includes('find src-tauri/target/universal-apple-darwin/release/bundle \\') &&
    releaseWorkflow.includes("-type f -name '*.sig'") &&
    releaseWorkflow.includes("-Filter '*.sig'"),
  'updater artifacts must only be created with verified keys and routed through the safe draft uploader',
)
expect(releaseWorkflow.includes('draft: true'), 'release.yml must create a private draft release')
expect(
  releaseWorkflow.includes('release_id: ${{ steps.create-draft.outputs.release_id }}') &&
    releaseWorkflow.includes('name: Create one commit-bound draft release') &&
    releaseWorkflow.includes('target_commitish: $commit') &&
    releaseWorkflow.includes('echo "release_id=$release_id" >> "$GITHUB_OUTPUT"') &&
    !releaseWorkflow.includes('releaseId:') &&
    !releaseWorkflow.includes('tagName:') &&
    !releaseWorkflow.includes('GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}\n        with:'),
  'release.yml must create one exact commit-bound draft while keeping tauri-action build-only',
)
const draftAssetUploadCalls = releaseWorkflow.match(/scripts\/upload-draft-release-assets\.mjs/g) ?? []
expect(
  draftAssetUploadCalls.length >= 3 &&
    !releaseWorkflow.includes('gh release upload') &&
    draftAssetUpload.includes('release?.draft !== true') &&
    draftAssetUpload.includes('release?.target_commitish !== contract.commit') &&
    draftAssetUpload.includes('Draft release already contains asset') &&
    draftAssetUpload.includes("process.argv.includes('--self-test')") &&
    packageJson.scripts['release:check'].includes('upload-draft-release-assets.mjs --self-test'),
  'release evidence uploads must target the verified draft ID through the self-tested fail-closed upload helper',
)
expect(
  releaseWorkflow.includes('name: Verify the complete release remains a private draft') &&
    releaseWorkflow.includes('.draft == true') &&
    releaseWorkflow.includes('a published release appeared before final draft verification') &&
    !releaseWorkflow.includes('gh release edit') &&
    !releaseWorkflow.includes('make_latest'),
  'release finalization must only audit the complete private draft and must never publish it',
)
expect(
  promoteDraftWorkflow.includes('name: Audit release draft') &&
    promoteDraftWorkflow.includes('contents: read') &&
    promoteDraftWorkflow.includes('.draft == true') &&
    !promoteDraftWorkflow.includes('contents: write') &&
    !promoteDraftWorkflow.includes('gh release upload') &&
    !promoteDraftWorkflow.includes('gh release edit') &&
    !promoteDraftWorkflow.includes('-X DELETE') &&
    !promoteDraftWorkflow.includes('--method DELETE') &&
    !promoteDraftWorkflow.includes('--method PATCH') &&
    !promoteDraftWorkflow.includes('--method POST'),
  'promote-draft.yml must remain a read-only audit with no upload, delete, promotion, or publication path',
)
const protectedPublicationJob = publishReleaseWorkflow.slice(publishReleaseWorkflow.indexOf('\n  publish:\n'))
const publicationReadyCalls = publishReleaseWorkflow.match(/release-contract-check\.mjs --publication-ready/g) ?? []
expect(
  protectedPublicationJob.includes('environment: release-publication') &&
    protectedPublicationJob.includes('ref: ${{ github.sha }}') &&
    protectedPublicationJob.includes('release-contract-check.mjs --publication-ready') &&
    publicationReadyCalls.length === 1 &&
    protectedPublicationJob.includes('Verify publication environment protections') &&
    protectedPublicationJob.includes('.prevent_self_review == true') &&
    protectedPublicationJob.includes("--header 'X-GitHub-Api-Version: 2026-03-10'") &&
    protectedPublicationJob.includes('.deployment_branch_policy.custom_branch_policies == true') &&
    protectedPublicationJob.includes('.total_count == 1') &&
    protectedPublicationJob.includes('.branch_policies[0].name == "main"') &&
    publishReleaseWorkflow.includes('--phase inspect') &&
    protectedPublicationJob.includes('--phase prepare') &&
    protectedPublicationJob.includes('--phase publish') &&
    protectedPublicationJob.includes('APPROVAL_COMMIT: ${{ github.sha }}') &&
    protectedPublicationJob.includes('PUBLICATION-UPLOAD-PLAN.json') &&
    protectedPublicationJob.includes('scripts/upload-draft-release-assets.mjs') &&
    !publishReleaseWorkflow.includes('gh release upload') &&
    !publishReleaseWorkflow.includes('--method DELETE') &&
    !publishReleaseWorkflow.includes('-X DELETE') &&
    publicationHelper.includes('verifyMinisignFile') &&
    publicationHelper.includes('ensureTagAtCommit') &&
    publicationHelper.includes('releaseAssetContract(final) !== finalAssetContract') &&
    packageJson.scripts['release:check'].includes('prepare-release-publication.mjs --self-test'),
  'publication must use the protected, immutable, approval-bound, self-tested release workflow without deletion or replacement paths',
)
expect(
  releaseWorkflow.includes("NODE_VERSION: '22.23.1'") && releaseWorkflow.includes("RUST_VERSION: '1.98.0'"),
  'release.yml must pin the verified Node and Rust candidate toolchains',
)
expect(
  releaseWorkflow.includes('target_commitish: $commit') && releaseWorkflow.includes('--arg commit "$GITHUB_SHA"'),
  'release.yml must bind the draft to the workflow commit',
)
expect(
  releaseWorkflow.includes('on:\n  workflow_dispatch:') &&
    !releaseWorkflow.includes('\n  push:') &&
    !releaseWorkflow.includes('github.ref_name') &&
    releaseWorkflow.includes('Verify release source is main') &&
    releaseWorkflow.includes('git merge-base --is-ancestor "$GITHUB_SHA" refs/remotes/origin/main'),
  'release.yml must be a dispatch-only private-candidate workflow for commits contained in main',
)
expect(
  releaseWorkflow.includes('[ "$GITHUB_REF" != "refs/heads/main" ]'),
  'release.yml must reject a manual release dispatch from a non-main branch',
)
expect(
  releaseWorkflow.includes('Reject a stale draft release for this tag') &&
    releaseWorkflow.includes('the release workflow never deletes drafts') &&
    !releaseWorkflow.includes('-X DELETE "repos/$GITHUB_REPOSITORY/releases/$id"'),
  'release.yml must fail closed on a stale same-tag draft without deleting it',
)
expect(
  releaseWorkflow.includes('already exists at $tagged_commit before candidate construction') &&
    !releaseWorkflow.includes('Existing tag $tag resolves to this run') &&
    releaseWorkflow.includes('may be bound during protected publication'),
  'candidate construction must reject every pre-existing release tag so only protected publication can bind it',
)
expect(
  releaseWorkflow.includes('Smoke-test packaged app startup (Windows)') &&
    releaseWorkflow.includes('Smoke-test packaged native payloads and NSIS startup (Windows)') &&
    releaseWorkflow.includes('run: ./scripts/windows-packaged-smoke.ps1') &&
    windowsPackagedSmoke.includes("-Filter 'ffprobe.exe'") &&
    windowsPackagedSmoke.includes("$_.Name -like '*.onnx'") &&
    windowsPackagedSmoke.includes("$_.Name -ieq 'onnxruntime.dll'") &&
    windowsPackagedSmoke.includes('-c:a ftr') &&
    windowsPackagedSmoke.includes("Invoke-PackagedNativeSmoke -Root $msiRoot -Label 'MSI'") &&
    windowsPackagedSmoke.includes("Invoke-PackagedNativeSmoke -Root $nsisRoot -Label 'NSIS'") &&
    windowsPackagedSmoke.includes('Invoke-PackagedStartup -App $nsisApp'),
  'release.yml must FTR-inspect and native-smoke both Windows installer payloads and startup-smoke both MSI and NSIS payloads',
)
expect(
  releaseWorkflow.includes("'src-tauri\\target\\release\\bundle\\msi'"),
  'release.yml must inspect the Windows bundle directory produced without an explicit target argument',
)
expect(
  releaseWorkflow.includes('Smoke-test downloadable macOS containers') &&
    releaseWorkflow.includes('run: bash scripts/macos-packaged-smoke.sh') &&
    macPackagedSmoke.includes('hdiutil attach "$dmg"') &&
    macPackagedSmoke.includes('tar xzf "$app_archive"') &&
    macPackagedSmoke.includes('local ffprobe="$app/Contents/MacOS/ffprobe"') &&
    macPackagedSmoke.includes("-iname '*.onnx'") &&
    macPackagedSmoke.includes("-iname 'libonnxruntime*.dylib'") &&
    macPackagedSmoke.includes('-c:a ftr') &&
    macPackagedSmoke.includes('codesign --verify --deep --strict') &&
    macPackagedSmoke.includes('if [ "${EXPECT_PLATFORM_SIGNING:-false}" = \'true\' ]') &&
    macPackagedSmoke.includes('xcrun stapler validate "$app"') &&
    macPackagedSmoke.includes('spctl --assess --type execute') &&
    macPackagedSmoke.includes('smoke_app "$dmg_app" \'DMG\'') &&
    macPackagedSmoke.includes('smoke_app "$archive_app" \'Archive\''),
  'release.yml must extract, FTR-inspect, signature-check, and startup-smoke both downloadable macOS containers',
)
expect(
  releaseWorkflow.includes('const marker = `## [${version}] - `') &&
    releaseWorkflow.includes('--arg body "$notes"') &&
    releaseWorkflow.includes('body: $body') &&
    !releaseWorkflow.includes('Release candidate build in progress'),
  'release.yml must populate the draft with the current versioned changelog section',
)
expect(
  releaseWorkflow.includes('Publish macOS inventory and checksum evidence to the draft') &&
    releaseWorkflow.includes('SHA256SUMS-macos.txt') &&
    releaseWorkflow.includes('RELEASE-INVENTORY-macos-dmg.json'),
  'release.yml must publish an extracted inventory and checksum manifest for the macOS artifacts',
)
expect(
  releaseWorkflow.includes('Publish Windows inventories and checksum evidence to the draft') &&
    releaseWorkflow.includes('SHA256SUMS-windows.txt') &&
    releaseWorkflow.includes('RELEASE-INVENTORY-windows-msi.json') &&
    releaseWorkflow.includes('RELEASE-INVENTORY-windows-nsis.json'),
  'release.yml must publish extracted inventories and a checksum manifest for Windows installers',
)
const privateFtrSecretNames = [
  'FTR_FIXTURE_URL',
  'FTR_FIXTURE_AUTHORIZATION',
  'FTR_FIXTURE_SIZE',
  'FTR_FIXTURE_SHA256',
  'FTR_FIXTURE_EVIDENCE_ID',
]
expect(
  privateFtrSecretNames.every(name => releaseWorkflow.includes(`${name}: ` + '${{ secrets.' + name + ' }}')) &&
    releaseWorkflow.includes('node scripts/private-ftr-fixture.mjs --check') &&
    releaseWorkflow.includes('node scripts/private-ftr-fixture.mjs --download') &&
    releaseWorkflow.includes('node scripts/private-ftr-fixture.mjs --clean') &&
    !releaseWorkflow.includes('FTR_SMOKE_'),
  'release.yml must fail closed on a permission-cleared private FTR fixture contract without a public recording URL',
)
expect(
  privateFtrFixture.includes("sourceUrl.protocol !== 'https:'") &&
    privateFtrFixture.includes('Authorization: contract.authorization') &&
    privateFtrFixture.includes('received > contract.expectedSize') &&
    privateFtrFixture.includes("createHash('sha256')") &&
    privateFtrFixture.includes("createHmac('sha256', evidenceId)") &&
    privateFtrFixture.includes('evidence contract ${contract.evidenceFingerprint}') &&
    privateFtrFixture.includes('request failed before a valid HTTPS response was received') &&
    artifactInventory.includes("endsWith('.trm')"),
  'private FTR fixture handling must enforce HTTPS, authorization, bounded reads, exact hashing, redacted errors, evidence binding, cleanup, and artifact exclusion',
)
expect(
  releaseWorkflow.includes('scripts/windows-webview2-evidence.ps1') &&
    releaseWorkflow.includes('WEBVIEW2-EVIDENCE-windows.json') &&
    releaseWorkflow.includes('WEBVIEW2-SIGNATURE-windows.txt') &&
    webviewEvidence.includes("$tauriCliVersion -ne '2.11.4'") &&
    webviewEvidence.includes('Get-AuthenticodeSignature') &&
    webviewEvidence.includes('$wixHash -ne $nsisHash') &&
    webviewEvidence.includes('finalInstallerEmbeddingVerified = $false'),
  'Windows releases must verify and retain exact Microsoft-signed, byte-identical WebView2 build-input evidence without overstating final extraction',
)
expect(
  bundleResources.includes('resources/third-party/rust') &&
    cargoToml.includes('publish = false') &&
    aboutPolicy.includes('ignore-dev-dependencies = true') &&
    !aboutPolicy.includes('"OpenSSL"') &&
    aboutPolicy.includes('aarch64-apple-darwin') &&
    aboutPolicy.includes('x86_64-pc-windows-msvc') &&
    aboutTemplate.includes('DepoAudio Rust third-party licenses') &&
    rustLicenseReport.includes('DepoAudio Rust third-party licenses') &&
    artifactInventory.includes('Missing generated Rust third-party license report'),
  'the locked supported-target Cargo union must produce a bundled third-party report without including DepoAudio itself',
)
expect(
  releaseWorkflow.includes('dependency-evidence:') &&
    releaseWorkflow.includes('publish-dependency-evidence:') &&
    releaseWorkflow.includes("CARGO_ABOUT_SIZE: '6732751'") &&
    releaseWorkflow.includes(
      "CARGO_ABOUT_SHA256: '9099a59e820c38a68b9d65f300662a567d56562f9a10f6aa4c7e86c17c2566af'",
    ) &&
    releaseWorkflow.includes("CARGO_CYCLONEDX_SIZE: '1766496'") &&
    releaseWorkflow.includes(
      "CARGO_CYCLONEDX_SHA256: '9bd3e599314f50810c9d98b8b68a617ff9d3cc20873968d90b29d121f6b226ff'",
    ) &&
    releaseWorkflow.includes('CARGO_NET_OFFLINE=true cargo about generate') &&
    releaseWorkflow.includes('normalize-cargo-about-report.mjs') &&
    releaseWorkflow.includes('--no-build-deps') &&
    releaseWorkflow.includes('npm sbom') &&
    releaseWorkflow.includes('SHA256SUMS-dependency-evidence.txt') &&
    releaseWorkflow.includes('needs: [prepare, dependency-evidence]') &&
    releaseWorkflow.includes('needs: [prepare, release, dependency-evidence]') &&
    releaseWorkflow.includes('needs: [prepare, release, publish-dependency-evidence]') &&
    releaseWorkflow.includes('actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a') &&
    releaseWorkflow.includes('actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c') &&
    sbomNormalizer.includes('delete document.serialNumber') &&
    sbomNormalizer.includes('depoaudio:source-commit') &&
    sbomNormalizer.includes('path+file://') &&
    cargoAboutNormalizer.includes("replaceAll('\\r\\n', '\\n')") &&
    releaseWorkflow.includes('generate-javascript-notices.mjs --strict --check') &&
    releaseWorkflow.includes('DepoAudio_${version}_javascript-third-party-notices.html') &&
    releaseWorkflow.includes('DepoAudio_${version}_javascript-components.json') &&
    publicationHelper.includes('DepoAudio_${version}_javascript-third-party-notices.html') &&
    publicationHelper.includes('DepoAudio_${version}_javascript-components.json') &&
    publicationHelper.includes('missing JavaScript notice evidence') &&
    packageJson.scripts['release:check'].includes('normalize-release-sbom.mjs --self-test') &&
    packageJson.scripts['release:check'].includes('normalize-cargo-about-report.mjs --self-test'),
  'release.yml must generate reproducible commit-bound Rust/npm evidence before packaging and publish it before finalization',
)
expect(
  javascriptNoticeComponents.schemaVersion === 1 &&
    javascriptNoticeComponents.componentCount === 60 &&
    javascriptNoticeComponents.javascriptComponentCount === 58 &&
    javascriptNoticeComponents.generatedCssComponentCount === 2 &&
    javascriptNoticeHtml.includes('DepoAudio JavaScript and generated-CSS third-party notices') &&
    javascriptNoticeGenerator.includes('javascript-notice-overrides.json') &&
    packageJson.scripts['notices:generate'] === 'node scripts/generate-javascript-notices.mjs' &&
    packageJson.scripts['notices:check'] === 'node scripts/generate-javascript-notices.mjs --check' &&
    packageJson.scripts['notices:self-test'] === 'node scripts/generate-javascript-notices.mjs --self-test' &&
    packageJson.scripts['release:check'].includes('npm run notices:self-test') &&
    packageJson.scripts['release:check'].includes('npm run notices:check') &&
    bundleResources.includes('resources/third-party/javascript') &&
    artifactInventory.includes("'javascript-notices-html'") &&
    artifactInventory.includes("'javascript-notices-components'") &&
    artifactInventory.includes("'byte-exact'") &&
    releaseWorkflow.indexOf('generate-javascript-notices.mjs --strict --check') <
      releaseWorkflow.indexOf('Write the pre-package binary and notice contract'),
  'deterministic JavaScript/CSS notices must be bundled, byte-bound, self-tested, and strict-gated before packaging',
)
const hasJavaScriptNoticeBlocker =
  thirdPartyInventory.includes('JavaScript and generated CSS notice status') &&
  thirdPartyInventory.includes('react-remove-scroll-bar@2.3.8') &&
  thirdPartyInventory.includes('an SBOM does not replace copyright notices or license') &&
  thirdPartyInventory.includes('v1.0.3 remains blocked') &&
  javascriptNoticeComponents.unresolvedComponentCount === 1 &&
  javascriptNoticeComponents.unresolvedComponents?.includes('react-remove-scroll-bar@2.3.8')
const generatedJavaScriptNoticeIsUnresolved = javascriptNoticeComponents.unresolvedComponentCount > 0
expect(
  generatedJavaScriptNoticeIsUnresolved ? hasJavaScriptNoticeBlocker : !hasJavaScriptNoticeBlocker,
  generatedJavaScriptNoticeIsUnresolved
    ? 'the release inventory must preserve every unresolved JavaScript notice gate'
    : 'resolved JavaScript notice evidence must remove the obsolete blocker copy',
)
expect(
  releaseWorkflow.includes("FFMPEG_WIN_ID: '496767001'") &&
    releaseWorkflow.includes("FFMPEG_WIN_SHA256: '5d65df0c0ca5346d82df8ade9c2e12db45d1f978f18ff908b42f03f5223dfc90'") &&
    releaseWorkflow.includes('ffmpeg-N-125875-g5d4d3bdc61-win64-lgpl.zip'),
  'release.yml must use the reviewed Windows LGPL FFmpeg archive',
)
expect(
  releaseWorkflow.includes(
    "FFMPEG_SOURCE_SHA256: 'de668509caf9e35e3cd162473441fdb29538c6d96ed080292b3cf9e6fc5d558f'",
  ) &&
    releaseWorkflow.includes(
      "LAME_SOURCE_SHA256: 'ddfe36cab873794038ae2c1210557ad34857a4b6bdc515785d1da9e175b1da1e'",
    ) &&
    releaseWorkflow.includes(
      "OPUS_SOURCE_SHA256: '6ffcb593207be92584df15b32466ed64bbec99109f007c82205f0194572411a1'",
    ) &&
    releaseWorkflow.includes('bash scripts/build-ffmpeg-macos.sh') &&
    !releaseWorkflow.includes('descriptinc/ffmpeg-ffprobe-static'),
  'release.yml must build both macOS FFmpeg slices from the reviewed LGPL source set',
)
expect(
  macFfmpegBuild.includes('--disable-autodetect') &&
    macFfmpegBuild.includes('--disable-gpl') &&
    macFfmpegBuild.includes('--disable-nonfree') &&
    macFfmpegBuild.includes('--enable-parser=ftr') &&
    macFfmpegBuild.includes("grep -q -- '--enable-gpl'") &&
    macFfmpegBuild.includes("grep -q -- '--enable-nonfree'") &&
    macFfmpegBuild.includes('for decoder in ftr aac flac pcm_s16le') &&
    macFfmpegBuild.includes('for encoder in libmp3lame libopus aac flac pcm_s16le') &&
    macFfmpegBuild.includes('FFmpeg-COPYING.LGPLv2.1') &&
    macFfmpegBuild.includes('Opus-COPYING') &&
    macFfmpegBuild.includes('depoaudio-third-party-source') &&
    macFfmpegBuild.includes(': > "$source_bundle/changes.diff"') &&
    releaseWorkflow.includes('changes.diff') &&
    releaseWorkflow.includes('test ! -s "$source_dir/changes.diff"') &&
    releaseWorkflow.includes('DepoAudio_${version}_third-party-source.tar.gz') &&
    releaseWorkflow.includes('SHA256SUMS-third-party-source.txt'),
  'macOS FFmpeg build must reject prohibited configurations and verify/preserve the released runtime contract',
)
expect(
  releaseWorkflow.includes('resources/third-party/ffmpeg') &&
    releaseWorkflow.includes("'BUILD-CONFIGURATION.txt'") &&
    releaseWorkflow.includes("'SOURCE.txt'") &&
    artifactInventory.includes('Missing FFmpeg distribution material'),
  'release.yml and extracted-artifact checks must preserve Windows FFmpeg license, configuration, and source evidence',
)
expect(
  configuredReleaseUrl === canonicalReleaseUrl &&
    settingsPanel.includes('uses FFmpeg under LGPL v2.1 or later') &&
    settingsPanel.includes('DEPOAUDIO_RELEASES_URL') &&
    installerLicense.includes('FFmpeg project under the GNU Lesser General') &&
    installerReleaseUrl === canonicalReleaseUrl,
  'the app About surface and installer license must attribute FFmpeg and route users to matching release source',
)
expect(
  tauriLib.includes('if updater_config_is_valid(updater_config)'),
  'desktop startup must not register the updater without a valid plugin configuration',
)
expect(
  restoreModelsWorkflow.includes(
    'microsoft/DNS-Challenge/82f1b17e7776a43eee395d0f45bae8abb700ad00/DNSMOS/DNSMOS/sig_bak_ovr.onnx',
  ),
  'restore-models.yml must fetch DNSMOS from the reviewed immutable upstream commit',
)
expect(
  !restoreModelsWorkflow.includes('microsoft/DNS-Challenge/master/'),
  'restore-models.yml must not fetch DNSMOS from a moving branch',
)
expect(
  restoreModelsWorkflow.includes(
    '269fbebdb513aa23cddfbb593542ecc540284a91849ac50516870e1ac78f6edd  dnsmos_sig_bak_ovr.onnx',
  ),
  'restore-models.yml must retain the reviewed DNSMOS SHA-256 pin',
)
expect(
  restoreModelsWorkflow.includes('contents: read') && !restoreModelsWorkflow.includes('contents: write'),
  'restore-models.yml must not have release mutation permission',
)
expect(
  !restoreModelsWorkflow.includes('--clobber') &&
    !restoreModelsWorkflow.includes('gh release upload') &&
    !restoreModelsWorkflow.includes('gh release create'),
  'restore-models.yml must verify the published models-v1 contract without replacing it',
)
expect(
  restoreModelsWorkflow.includes("V102_RELEASE_COMMIT: '6113fac8d8bac8f240dd2b10de6e77cd79cc772c'") &&
    restoreModelsWorkflow.includes('${V102_RELEASE_COMMIT}:src-tauri/resources/models/$f.onnx') &&
    !restoreModelsWorkflow.includes('cp "src-tauri/resources/models/$f.onnx" expected/'),
  'historical models-v1 verification must reconstruct v1.0.2 bytes from its pinned commit, not the current tree',
)

const releaseGateEvidenceBindings = {
  'shipped-model-commercial-rights':
    packageJson.scripts['models:check'] === 'node scripts/verify-model-distribution.mjs' &&
    artifactInventory.includes('Forbidden learned-model material') &&
    releaseWorkflow.includes('npm run models:check'),
  'javascript-runtime-notices':
    thirdPartyInventory.includes('JavaScript and generated CSS notice status') &&
    thirdPartyInventory.includes('react-remove-scroll-bar@2.3.8'),
  'macos-ffmpeg-rc2-evidence':
    releaseCandidate.includes('Build RC2 with the new macOS source-built FFmpeg sidecars') &&
    releaseWorkflow.includes('Smoke-test downloadable macOS containers') &&
    macPackagedSmoke.includes('smoke_app "$dmg_app" \'DMG\'') &&
    macPackagedSmoke.includes('smoke_app "$archive_app" \'Archive\''),
  'windows-ffmpeg-source-and-notices':
    thirdPartyInventory.includes('establish corresponding-source delivery') &&
    releaseWorkflow.includes('resources/third-party/ffmpeg') &&
    windowsPackagedSmoke.includes("-Filter 'ffmpeg.exe'") &&
    windowsPackagedSmoke.includes("-Filter 'ffprobe.exe'"),
  'webview2-final-installer-evidence':
    webviewEvidence.includes('finalInstallerEmbeddingVerified = $false') &&
    releaseCandidate.includes('extracting the MSI Binary table and NSIS payload'),
  'private-ftr-fixture-permission':
    privateFtrFixture.includes('FTR_FIXTURE_EVIDENCE_ID') &&
    privateFtrFixture.includes("createHmac('sha256', evidenceId)") &&
    releaseCandidate.includes('owner-approved provenance and permission record'),
  'packaged-install-and-upgrade-validation':
    artifactInventory.includes('BINARY-CONTRACT.json') &&
    releaseWorkflow.includes('RELEASE-INVENTORY-macos-dmg.json') &&
    releaseWorkflow.includes('RELEASE-INVENTORY-windows-msi.json') &&
    releaseWorkflow.includes('RELEASE-INVENTORY-windows-nsis.json') &&
    releaseCandidate.toLowerCase().includes('upgrade an existing v1.0.2 installation'),
  'signing-notarization-and-updater-decision':
    releaseCandidate.includes('Verify Authenticode, Apple signing, notarization, Gatekeeper, SmartScreen') &&
    releaseWorkflow.includes('createUpdaterArtifacts: true') &&
    releaseWorkflow.includes('Build signed and notarized macOS packages') &&
    releaseWorkflow.includes('Remove Windows signing certificate before candidate execution') &&
    macPackagedSmoke.includes('EXPECT_PLATFORM_SIGNING'),
  'rust-advisory-exception-acceptance':
    advisoryExceptionIds.every(advisory => denyPolicy.includes(`"${advisory}"`)) &&
    releaseCandidate.includes('Confirm the release owner accepts the five exact INFO/unmaintained'),
  'codec-patent-review':
    thirdPartyInventory.includes('complete codec/patent review') &&
    thirdPartyInventory.includes('an open-source copyright license does not grant every patent'),
  'release-changelog-finalization':
    changelog.includes(`## [${version}]`) &&
    releaseCandidate.includes('replace the marker with the approved') &&
    releaseCandidate.includes('changelog heading whose date exactly matches the approved publication date'),
}
for (const [blockerId, isBoundToEvidence] of Object.entries(releaseGateEvidenceBindings)) {
  expect(gateBlockerById.has(blockerId), `the release gate must retain blocker ID ${blockerId}`)
  expect(isBoundToEvidence, `release blocker ${blockerId} is no longer tied to its repository evidence gate`)
}

const javascriptNoticeGateIsOpen = gateBlockerById.get('javascript-runtime-notices')?.status === 'open'
expect(
  javascriptNoticeGateIsOpen === hasJavaScriptNoticeBlocker,
  'the JavaScript notice blocker status must match the unresolved third-party inventory evidence',
)
const otherOpenGateBlockers = openGateBlockers.filter(blocker => blocker.id !== 'release-changelog-finalization')
const changelogIsDated = datedReleaseHeading.test(changelog)
const changelogGateIsOpen = gateBlockerById.get('release-changelog-finalization')?.status === 'open'
if (otherOpenGateBlockers.length > 0) {
  expect(!changelogIsDated, 'the v1.0.3 changelog must remain Unpublished while another publication blocker is open')
}
expect(
  changelogGateIsOpen === (!changelogIsDated || otherOpenGateBlockers.length > 0),
  'the changelog blocker may close only after the release heading is dated and every other blocker is closed',
)

if (failures.length > 0) {
  console.error('Release contract check failed:')
  for (const failure of failures) console.error(`- ${failure}`)
  process.exit(1)
}

console.log(
  `Release contract OK for v${version} (${releaseGate.status}; ${openGateBlockers.length} open publication blockers)`,
)
