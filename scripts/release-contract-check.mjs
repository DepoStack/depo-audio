import { readFileSync } from 'node:fs'

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
const releaseWorkflow = read('.github/workflows/release.yml')
const tauriLib = read('src-tauri/src/lib.rs')
const requiresPublishedHeading = process.argv.includes('--published')

const version = packageJson.version
const escapedVersion = version.replaceAll('.', '\\.')
const cargoPackageVersion = cargoToml.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1]
const cargoLockVersion = cargoLock.match(
  /^\[\[package\]\]\s*\r?\nname = "depo-audio"\s*\r?\nversion = "([^"]+)"/m,
)?.[1]

expect(packageLock.version === version, 'package-lock.json top-level version must match package.json')
expect(packageLock.packages?.['']?.version === version, 'package-lock.json root package version must match package.json')
expect(tauriConfig.version === version, 'tauri.conf.json version must match package.json')
expect(cargoPackageVersion === version, 'Cargo.toml package version must match package.json')
expect(cargoLockVersion === version, 'Cargo.lock depo-audio version must match package.json')

const unreleasedPosition = changelog.indexOf('## [Unreleased]')
const datedReleaseHeading = new RegExp(`^## \\[${escapedVersion}\\] - \\d{4}-\\d{2}-\\d{2}$`, 'm')
const releaseHeading = requiresPublishedHeading
  ? datedReleaseHeading
  : new RegExp(`^## \\[${escapedVersion}\\] - (?:\\d{4}-\\d{2}-\\d{2}|Unpublished)$`, 'm')
const releaseMatch = changelog.match(releaseHeading)
expect(unreleasedPosition >= 0, 'CHANGELOG.md must retain an Unreleased heading')
expect(
  releaseMatch,
  requiresPublishedHeading
    ? `CHANGELOG.md must contain a dated ${version} release heading before a final build`
    : `CHANGELOG.md must contain a dated or explicitly Unpublished ${version} heading`,
)
if (releaseMatch) {
  expect(
    unreleasedPosition < releaseMatch.index,
    'The Unreleased changelog heading must precede the current release heading',
  )
}

expect(!releaseWorkflow.includes('includeUpdaterJson:'), 'release.yml uses an invalid tauri-action input: includeUpdaterJson')
expect(
  releaseWorkflow.includes('uploadUpdaterJson: ${{ steps.signing.outputs.updater }}'),
  'release.yml must gate updater JSON upload on verified updater signing configuration',
)
expect(
  releaseWorkflow.includes('uploadUpdaterSignatures: ${{ steps.signing.outputs.updater }}'),
  'release.yml must gate updater signature upload on verified updater signing configuration',
)
expect(releaseWorkflow.includes('releaseDraft: true'), 'release.yml must create a draft release')
expect(
  releaseWorkflow.includes('releaseCommitish: ${{ github.sha }}'),
  'release.yml must bind the draft to the workflow commit',
)
expect(
  releaseWorkflow.includes('Delete stale draft release for this tag'),
  'release.yml must remove a stale same-tag draft before rebuilding',
)
expect(
  releaseWorkflow.includes('Smoke-test packaged app startup (Windows)'),
  'release.yml must launch the packaged Windows app before the release workflow finishes',
)
expect(
  releaseWorkflow.includes("'src-tauri\\target\\release\\bundle\\msi'"),
  'release.yml must inspect the Windows bundle directory produced without an explicit target argument',
)
expect(
  releaseWorkflow.includes('Smoke-test packaged app startup (macOS)'),
  'release.yml must launch the packaged macOS app before the release workflow finishes',
)
expect(
  tauriLib.includes('if updater_config_is_valid(updater_config)'),
  'desktop startup must not register the updater without a valid plugin configuration',
)

if (failures.length > 0) {
  console.error('Release contract check failed:')
  for (const failure of failures) console.error(`- ${failure}`)
  process.exit(1)
}

console.log(`Release contract OK for v${version}`)
