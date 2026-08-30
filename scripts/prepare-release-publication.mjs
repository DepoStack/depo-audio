#!/usr/bin/env node

import { createHash, createPublicKey, verify as verifyCryptographicSignature } from 'node:crypto'
import { createReadStream, createWriteStream } from 'node:fs'
import { mkdir, readFile, stat, writeFile } from 'node:fs/promises'
import { basename, join, resolve } from 'node:path'
import { Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'

const API_VERSION = '2022-11-28'
const GATE_PATH = new URL('../docs/V1.0.3-RELEASE-GATE.json', import.meta.url)
const CHANGELOG_PATH = new URL('../CHANGELOG.md', import.meta.url)
const RECEIPT_NAME = 'RELEASE-APPROVAL.json'
const UPDATER_NAME = 'latest.json'
const INSPECTION_NAME = 'RELEASE-CANDIDATE-INSPECTION.json'
const UPLOAD_PLAN_NAME = 'PUBLICATION-UPLOAD-PLAN.json'
let githubRequestMode = 'read-only'

function fail(message) {
  throw new Error(message)
}

function configureGitHubRequestMode(phase) {
  githubRequestMode = phase === 'publish' ? 'publish' : 'read-only'
}

function authorizeGitHubRequest(method = 'GET') {
  const normalized = method.toUpperCase()
  if (githubRequestMode === 'read-only' && normalized !== 'GET') {
    fail(`Read-only release inspection rejected GitHub ${normalized}`)
  }
  return normalized
}

function parseArguments(argv) {
  const value = flag => {
    const index = argv.indexOf(flag)
    if (index < 0 || !argv[index + 1]) fail(`Missing ${flag}`)
    return argv[index + 1]
  }
  const phase = value('--phase')
  if (!['inspect', 'prepare', 'publish'].includes(phase)) fail('Phase must be inspect, prepare, or publish')
  return {
    phase,
    repo: value('--repo'),
    outputDir: resolve(value('--output-dir')),
  }
}

function isSafeAssetName(name) {
  return (
    typeof name === 'string' &&
    name.length > 0 &&
    name === basename(name) &&
    name !== '.' &&
    name !== '..' &&
    !/[\x00-\x1f\x7f/\\]/.test(name)
  )
}

function validateRepo(repo) {
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo)) fail('Invalid repository contract')
  if (repo.split('/').some(part => part === '.' || part === '..')) fail('Unsafe repository contract')
}

function validateApprovalContext(context) {
  if (!/^[0-9a-f]{40}$/.test(context.commit ?? '')) fail('Approval commit is invalid')
  if (typeof context.actor !== 'string' || !/^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$/.test(context.actor)) {
    fail('Approval actor is invalid')
  }
  for (const field of ['actorId', 'runId', 'runAttempt']) {
    if (!/^[1-9][0-9]*$/.test(context[field] ?? '')) fail(`Approval ${field} is invalid`)
  }
  if (
    typeof context.workflowRef !== 'string' ||
    !context.workflowRef.includes('/.github/workflows/publish-release.yml@refs/heads/main')
  ) {
    fail('Approval workflow reference is invalid')
  }
  return context
}

function readApprovalContext(environment = process.env) {
  return validateApprovalContext({
    commit: environment.APPROVAL_COMMIT,
    actor: environment.APPROVAL_ACTOR,
    actorId: environment.APPROVAL_ACTOR_ID,
    runId: environment.APPROVAL_RUN_ID,
    runAttempt: environment.APPROVAL_RUN_ATTEMPT,
    workflowRef: environment.APPROVAL_WORKFLOW_REF,
  })
}

function validateGate(gate, { phase, now = new Date() }) {
  const candidate = gate?.candidate
  if (gate?.schemaVersion !== 2 || !['GO', 'NO-GO'].includes(gate?.status)) {
    fail('Release gate must be schema 2 with an explicit status')
  }
  if (!/^\d+\.\d+\.\d+$/.test(gate?.version ?? '')) fail('Release gate version is invalid')
  if (!Array.isArray(gate?.blockers)) fail('Release gate blockers are invalid')
  if (candidate?.tag !== `v${gate.version}`) fail('Candidate tag does not match the release version')
  if (!Number.isSafeInteger(candidate?.releaseId) || candidate.releaseId <= 0) {
    fail('Candidate release ID is invalid')
  }
  if (!/^[0-9a-f]{40}$/.test(candidate?.sourceCommit ?? '')) fail('Candidate source commit is invalid')
  if (phase === 'inspect') return candidate

  if (gate.status !== 'GO' || gate.blockers.some(blocker => blocker?.status !== 'closed')) {
    fail('Publication requires a GO gate with every release blocker closed')
  }
  if (!/^[0-9a-f]{64}$/.test(candidate?.assetManifestSha256 ?? '')) {
    fail('Candidate asset manifest digest is invalid')
  }
  if (!['signed', 'unavailable'].includes(candidate?.updater)) fail('Candidate updater decision is invalid')
  if (!/^\d{4}-\d{2}-\d{2}$/.test(candidate?.publicationDate ?? '')) {
    fail('Candidate publication date is invalid')
  }
  const date = new Date(`${candidate.publicationDate}T00:00:00.000Z`)
  if (date.toISOString().slice(0, 10) !== candidate.publicationDate) {
    fail('Candidate publication date does not exist')
  }
  if (candidate.publicationDate !== now.toISOString().slice(0, 10)) {
    fail('Candidate publication date must equal the current UTC date')
  }
  if (typeof candidate?.approvedBy !== 'string' || candidate.approvedBy.trim().length === 0) {
    fail('Candidate approval owner is missing')
  }
  if (candidate.updater === 'signed') parseMinisignPublicKey(candidate.updaterPublicKey)
  else if (candidate.updaterPublicKey !== null) fail('Updater-unavailable candidate must not approve a public key')
  return candidate
}

function validateRelease(release, candidate, { allowPublicationAssets = false } = {}) {
  if (
    release?.id !== candidate.releaseId ||
    release?.draft !== true ||
    release?.tag_name !== candidate.tag ||
    release?.target_commitish !== candidate.sourceCommit ||
    release?.prerelease !== false
  ) {
    fail('The exact approved private draft contract no longer matches')
  }
  if (!Array.isArray(release.assets) || release.assets.length === 0) fail('The private draft has no assets')
  const names = new Set()
  for (const asset of release.assets) {
    if (
      !Number.isSafeInteger(asset?.id) ||
      asset.id <= 0 ||
      !isSafeAssetName(asset?.name) ||
      !Number.isSafeInteger(asset?.size) ||
      asset.size <= 0 ||
      asset?.state !== 'uploaded' ||
      typeof asset?.url !== 'string'
    ) {
      fail('The private draft contains an invalid or incomplete asset')
    }
    if (names.has(asset.name)) fail(`The private draft contains duplicate asset ${asset.name}`)
    names.add(asset.name)
  }
  if (!allowPublicationAssets && (names.has(RECEIPT_NAME) || names.has(UPDATER_NAME))) {
    fail('Publication assets already exist on the private draft')
  }
}

function publicationAssetNames(updater) {
  return updater === 'signed' ? [RECEIPT_NAME, UPDATER_NAME] : [RECEIPT_NAME]
}

function sha256Buffer(buffer) {
  return createHash('sha256').update(buffer).digest('hex')
}

function parseMinisignPublicKey(value) {
  if (typeof value !== 'string' || value.length > 2_000) fail('Approved updater public key is invalid')
  const outer = value.trim()
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(outer)) fail('Approved updater public key is invalid')
  const publicKeyText = Buffer.from(outer, 'base64').toString('utf8')
  const lines = publicKeyText.trim().split(/\r?\n/)
  if (lines.length !== 2 || !lines[0].startsWith('untrusted comment: ')) {
    fail('Approved updater public key is not a complete Minisign public-key file')
  }
  const encoded = lines[1]
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(encoded)) fail('Approved updater public key is invalid')
  const decoded = Buffer.from(encoded, 'base64')
  if (decoded.length !== 42 || !['Ed', 'ED'].includes(decoded.subarray(0, 2).toString('ascii'))) {
    fail('Approved updater public key is not a Minisign Ed25519 key')
  }
  return {
    encoded,
    keyId: decoded.subarray(2, 10),
    rawKey: decoded.subarray(10),
    sha256: sha256Buffer(Buffer.from(outer, 'utf8')),
  }
}

function parseMinisignSignature(value) {
  if (typeof value !== 'string' || value.length > 20_000 || value.includes('\0')) {
    fail('Updater signature file is invalid')
  }
  const outer = value.trim()
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(outer)) fail('Updater signature file is not valid base64')
  const lines = Buffer.from(outer, 'base64').toString('utf8').trim().split(/\r?\n/)
  if (lines.length !== 4 || !lines[2].startsWith('trusted comment: ')) {
    fail('Updater signature file is not a complete Minisign signature')
  }
  const signatureRecord = Buffer.from(lines[1], 'base64')
  const globalSignature = Buffer.from(lines[3], 'base64')
  if (
    signatureRecord.length !== 74 ||
    globalSignature.length !== 64 ||
    signatureRecord.subarray(0, 2).toString('ascii') !== 'ED'
  ) {
    fail('Updater signature must use Minisign prehashed mode')
  }
  return {
    keyId: signatureRecord.subarray(2, 10),
    signature: signatureRecord.subarray(10),
    trustedComment: lines[2].slice('trusted comment: '.length),
    globalSignature,
  }
}

function verifyMinisignDigest(digest, signatureText, publicKeyText) {
  const publicKey = parseMinisignPublicKey(publicKeyText)
  const signature = parseMinisignSignature(signatureText)
  if (!publicKey.keyId.equals(signature.keyId)) fail('Updater signature key ID does not match the approved public key')
  const spkiPrefix = Buffer.from('302a300506032b6570032100', 'hex')
  const key = createPublicKey({ key: Buffer.concat([spkiPrefix, publicKey.rawKey]), format: 'der', type: 'spki' })
  if (!verifyCryptographicSignature(null, digest, key, signature.signature)) {
    fail('Updater artifact signature verification failed')
  }
  const globalMessage = Buffer.concat([signature.signature, Buffer.from(signature.trustedComment, 'utf8')])
  if (!verifyCryptographicSignature(null, globalMessage, key, signature.globalSignature)) {
    fail('Updater trusted-comment signature verification failed')
  }
  return publicKey
}

async function verifyMinisignFile(artifactPath, signatureText, publicKeyText) {
  const hash = createHash('blake2b512')
  for await (const chunk of createReadStream(artifactPath)) hash.update(chunk)
  return verifyMinisignDigest(hash.digest(), signatureText, publicKeyText)
}

async function sha256File(path) {
  const hash = createHash('sha256')
  for await (const chunk of createReadStream(path)) hash.update(chunk)
  return hash.digest('hex')
}

function compareAssetNames(left, right) {
  return Buffer.from(left.name).compare(Buffer.from(right.name))
}

function createManifest(entries) {
  return `${[...entries]
    .sort(compareAssetNames)
    .map(entry => `${entry.sha256}  ${entry.size}  ${entry.name}`)
    .join('\n')}\n`
}

function extractReleaseNotes(changelog, version, publicationDate) {
  const marker = `## [${version}] - ${publicationDate}`
  const start = changelog.indexOf(marker)
  if (start < 0) fail(`Changelog is missing exact release heading ${marker}`)
  const nextHeading = changelog.indexOf('\n## [', start + marker.length)
  const notes = changelog.slice(start, nextHeading < 0 ? undefined : nextHeading).trim()
  if (!notes) fail('Approved release notes are empty')
  return notes
}

function requiredReleaseEvidence(version) {
  return [
    'SHA256SUMS-macos.txt',
    'SHA256SUMS-windows.txt',
    'SHA256SUMS-third-party-source.txt',
    'SHA256SUMS-dependency-evidence.txt',
    'RELEASE-INVENTORY-macos-dmg.json',
    'RELEASE-INVENTORY-macos-app-archive.json',
    'RELEASE-INVENTORY-windows-msi.json',
    'RELEASE-INVENTORY-windows-nsis.json',
    'WEBVIEW2-EVIDENCE-windows.json',
    'WEBVIEW2-SIGNATURE-windows.txt',
    `DepoAudio_${version}_third-party-source.tar.gz`,
    `DepoAudio_${version}_rust-third-party-licenses.html`,
    `DepoAudio_${version}_javascript-third-party-notices.html`,
    `DepoAudio_${version}_javascript-components.json`,
    `DepoAudio_${version}_third-party-review.md`,
    'DEPENDENCY-EVIDENCE-TOOLCHAIN.txt',
    `DepoAudio_${version}_npm.cdx.json`,
    `DepoAudio_${version}_rust_aarch64-apple-darwin.cdx.json`,
    `DepoAudio_${version}_rust_x86_64-apple-darwin.cdx.json`,
    `DepoAudio_${version}_rust_x86_64-pc-windows-msvc.cdx.json`,
  ]
}

function validateCandidateAssetSet(entries, version, updater) {
  const names = new Set(entries.map(entry => entry.name))
  const baseNames = {
    macArchive: `DepoAudio_${version}_universal.app.tar.gz`,
    macDmg: `DepoAudio_${version}_universal.dmg`,
    windowsMsi: `DepoAudio_${version}_x64_en-US.msi`,
    windowsNsis: `DepoAudio_${version}_x64-setup.exe`,
  }
  for (const name of Object.values(baseNames)) {
    if (!names.has(name)) fail(`Candidate is missing required downloadable asset ${name}`)
  }
  const requiredEvidence = requiredReleaseEvidence(version)
  for (const name of requiredEvidence) {
    if (!names.has(name)) fail(`Candidate is missing required release evidence ${name}`)
  }
  const signatureNames = entries.filter(entry => entry.name.endsWith('.sig')).map(entry => entry.name)
  const expectedSignatures = [
    `${baseNames.macArchive}.sig`,
    `${baseNames.windowsMsi}.sig`,
    `${baseNames.windowsNsis}.sig`,
  ]
  if (updater === 'signed') {
    for (const name of expectedSignatures) {
      if (!names.has(name)) fail(`Signed updater candidate is missing ${name}`)
    }
    if (signatureNames.some(name => !expectedSignatures.includes(name))) {
      fail('Signed updater candidate contains an unexpected signature asset')
    }
  } else if (updater === 'unavailable' && signatureNames.length > 0) {
    fail('Updater-unavailable candidate must not contain signature assets')
  } else if (!['signed', 'unavailable', 'undecided'].includes(updater)) {
    fail('Candidate updater state is invalid')
  }
  return baseNames
}

async function verifyUpdaterSignatures(entries, names, publicKeyText) {
  const byName = new Map(entries.map(entry => [entry.name, entry]))
  let verifiedPublicKey
  for (const artifactName of [names.macArchive, names.windowsMsi, names.windowsNsis]) {
    const artifact = byName.get(artifactName)
    const signatureAsset = byName.get(`${artifactName}.sig`)
    const signatureText = await readFile(signatureAsset.path, 'utf8')
    const publicKey = await verifyMinisignFile(artifact.path, signatureText, publicKeyText)
    verifiedPublicKey ??= publicKey
  }
  return verifiedPublicKey
}

function updaterPlatforms({ repo, tag, names, signatures }) {
  const assetUrl = name =>
    `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(name)}`
  const mac = {
    signature: signatures[names.macArchive],
    url: assetUrl(names.macArchive),
  }
  const msi = {
    signature: signatures[names.windowsMsi],
    url: assetUrl(names.windowsMsi),
  }
  const nsis = {
    signature: signatures[names.windowsNsis],
    url: assetUrl(names.windowsNsis),
  }
  return {
    'darwin-aarch64': mac,
    'darwin-x86_64': mac,
    'darwin-aarch64-app': mac,
    'darwin-x86_64-app': mac,
    'darwin-universal': mac,
    'darwin-universal-app': mac,
    'windows-x86_64': msi,
    'windows-x86_64-msi': msi,
    'windows-x86_64-nsis': nsis,
  }
}

async function createUpdaterManifest({ repo, gate, candidate, entries, names, notes }) {
  const byName = new Map(entries.map(entry => [entry.name, entry]))
  const signatures = {}
  for (const artifactName of [names.macArchive, names.windowsMsi, names.windowsNsis]) {
    signatures[artifactName] = (await readFile(byName.get(`${artifactName}.sig`).path, 'utf8')).trim()
  }
  return `${JSON.stringify(
    {
      version: gate.version,
      notes,
      pub_date: `${candidate.publicationDate}T00:00:00.000Z`,
      platforms: updaterPlatforms({ repo, tag: candidate.tag, names, signatures }),
    },
    null,
    2,
  )}\n`
}

function assertSameManifest(actual, approved) {
  if (actual !== approved) fail('Candidate asset manifest no longer matches the approved digest')
}

async function githubRequest(url, token, init = {}, acceptedStatuses = []) {
  const method = authorizeGitHubRequest(init.method)
  const response = await fetch(url, {
    ...init,
    method,
    headers: {
      Accept: 'application/vnd.github+json',
      Authorization: `Bearer ${token}`,
      'X-GitHub-Api-Version': API_VERSION,
      ...(init.headers ?? {}),
    },
    redirect: 'follow',
  })
  if (!response.ok && !acceptedStatuses.includes(response.status)) {
    const body = (await response.text()).slice(0, 500)
    fail(`GitHub API request failed with HTTP ${response.status}: ${body}`)
  }
  return response
}

async function getRelease(repo, releaseId, token) {
  const response = await githubRequest(`https://api.github.com/repos/${repo}/releases/${releaseId}`, token)
  return await response.json()
}

async function peelTagObject(object, loadAnnotatedTag) {
  const visited = new Set()
  let current = object
  for (let depth = 0; depth < 8; depth += 1) {
    if (!/^[0-9a-f]{40}$/.test(current?.sha ?? '')) fail('Git tag object has an invalid SHA')
    if (current.type === 'commit') return current.sha
    if (current.type !== 'tag' || visited.has(current.sha)) fail('Git tag object cannot be peeled safely')
    visited.add(current.sha)
    const annotated = await loadAnnotatedTag(current.sha)
    current = annotated?.object
  }
  fail('Git tag annotation chain is too deep')
}

async function resolveTagCommit(repo, tag, token) {
  const response = await githubRequest(
    `https://api.github.com/repos/${repo}/git/ref/tags/${encodeURIComponent(tag)}`,
    token,
    {},
    [404],
  )
  if (response.status === 404) return null
  const ref = await response.json()
  if (ref?.ref !== `refs/tags/${tag}`) fail('GitHub returned the wrong release tag reference')
  return peelTagObject(ref.object, async sha => {
    const annotatedResponse = await githubRequest(`https://api.github.com/repos/${repo}/git/tags/${sha}`, token)
    return annotatedResponse.json()
  })
}

async function ensureTagAtCommit(repo, tag, commit, token) {
  const existing = await resolveTagCommit(repo, tag, token)
  if (existing !== null) {
    if (existing !== commit) fail(`Existing release tag ${tag} does not resolve to the approved source commit`)
    return
  }
  const response = await githubRequest(
    `https://api.github.com/repos/${repo}/git/refs`,
    token,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ref: `refs/tags/${tag}`, sha: commit }),
    },
    [422],
  )
  if (response.status !== 201 && response.status !== 422) fail('GitHub did not create the approved release tag')
  const resolved = await resolveTagCommit(repo, tag, token)
  if (resolved !== commit) fail(`Release tag ${tag} does not resolve to the approved source commit`)
}

function releaseAssetContract(release) {
  return JSON.stringify(
    release.assets
      .map(asset => ({
        id: asset.id,
        name: asset.name,
        size: asset.size,
        state: asset.state,
        digest: asset.digest ?? null,
        updatedAt: asset.updated_at ?? null,
      }))
      .sort(compareAssetNames),
  )
}

async function downloadAssets(assets, directory, token) {
  await mkdir(directory, { recursive: false })
  const entries = []
  for (const asset of [...assets].sort((left, right) => compareAssetNames(left, right))) {
    const destination = join(directory, asset.name)
    const response = await githubRequest(asset.url, token, {
      headers: { Accept: 'application/octet-stream' },
    })
    if (!response.body) fail(`GitHub returned no body for asset ${asset.name}`)
    await pipeline(Readable.fromWeb(response.body), createWriteStream(destination, { flags: 'wx' }))
    const metadata = await stat(destination)
    if (metadata.size !== asset.size) fail(`Downloaded size mismatch for ${asset.name}`)
    const sha256 = await sha256File(destination)
    if (asset.digest && asset.digest !== `sha256:${sha256}`) {
      fail(`GitHub asset digest mismatch for ${asset.name}`)
    }
    entries.push({ id: asset.id, name: asset.name, size: asset.size, sha256, path: destination })
  }
  return entries
}

async function readJson(path) {
  return JSON.parse(await readFile(path, 'utf8'))
}

function releaseNotesContent(notes, version) {
  const [heading, ...body] = notes.trim().split(/\r?\n/)
  if (
    !new RegExp(`^## \\[${version.replaceAll('.', '\\.')}\\] - (?:Unpublished|\\d{4}-\\d{2}-\\d{2})$`).test(heading)
  ) {
    fail('Release notes have an invalid version heading')
  }
  return body.join('\n').trim()
}

async function inspect({ repo, outputDir }, gate, candidate, token) {
  await mkdir(outputDir, { recursive: false })
  const release = await getRelease(repo, candidate.releaseId, token)
  validateRelease(release, candidate)
  const entries = await downloadAssets(release.assets, join(outputDir, 'candidate-assets'), token)
  validateCandidateAssetSet(entries, gate.version, 'undecided')
  const manifest = createManifest(entries)
  const manifestSha256 = sha256Buffer(Buffer.from(manifest, 'utf8'))
  if (candidate.assetManifestSha256 !== null && candidate.assetManifestSha256 !== manifestSha256) {
    fail('Inspected candidate assets have drifted from the recorded manifest digest')
  }
  await writeFile(join(outputDir, 'PREPUBLICATION-ASSET-MANIFEST.txt'), manifest, { flag: 'wx' })
  const inspection = {
    schemaVersion: 1,
    release: {
      version: gate.version,
      tag: candidate.tag,
      releaseId: candidate.releaseId,
      sourceCommit: candidate.sourceCommit,
      name: release.name,
      bodySha256: sha256Buffer(Buffer.from(release.body ?? '', 'utf8')),
      updatedAt: release.updated_at,
    },
    assetManifest: {
      format: 'sha256  size  name; UTF-8; LF; bytewise filename order',
      sha256: manifestSha256,
      entries: entries.map(({ name, size, sha256 }) => ({ name, size, sha256 })).sort(compareAssetNames),
    },
    updaterSignatureAssets: entries
      .filter(entry => entry.name.endsWith('.sig'))
      .map(entry => entry.name)
      .sort(),
  }
  await writeFile(join(outputDir, INSPECTION_NAME), `${JSON.stringify(inspection, null, 2)}\n`, { flag: 'wx' })
  process.stdout.write(`Candidate asset manifest SHA-256: ${manifestSha256}\n`)
}

function validateApprovalReceipt(receipt, { gate, candidate, approval, notes, manifestSha256, entries, gateSha256 }) {
  const storedApproval = validateApprovalContext(receipt?.approval ?? {})
  const expectedEntries = entries.map(({ name, size, sha256 }) => ({ name, size, sha256 })).sort(compareAssetNames)
  if (
    receipt?.schemaVersion !== 1 ||
    receipt?.release?.version !== gate.version ||
    receipt?.release?.tag !== candidate.tag ||
    receipt?.release?.releaseId !== candidate.releaseId ||
    receipt?.release?.sourceCommit !== candidate.sourceCommit ||
    receipt?.release?.publicationDate !== candidate.publicationDate ||
    receipt?.release?.approvedBy !== candidate.approvedBy.trim() ||
    receipt?.release?.updater !== candidate.updater ||
    receipt?.release?.updaterPublicKeySha256 !==
      (candidate.updater === 'signed' ? parseMinisignPublicKey(candidate.updaterPublicKey).sha256 : null) ||
    receipt?.release?.approvedName !== `DepoAudio ${candidate.tag}` ||
    receipt?.release?.approvedBodySha256 !== sha256Buffer(Buffer.from(notes, 'utf8')) ||
    receipt?.gate?.sha256 !== gateSha256 ||
    storedApproval.commit !== approval.commit ||
    receipt?.assetManifest?.sha256 !== manifestSha256 ||
    JSON.stringify(receipt?.assetManifest?.entries) !== JSON.stringify(expectedEntries)
  ) {
    fail('Publication receipt does not match the exact approved release contract')
  }
  return receipt
}

async function prepare({ repo, outputDir }, gate, candidate, token, approval) {
  await mkdir(outputDir, { recursive: false })
  const release = await getRelease(repo, candidate.releaseId, token)
  validateRelease(release, candidate, { allowPublicationAssets: true })
  const notes = extractReleaseNotes(await readFile(CHANGELOG_PATH, 'utf8'), gate.version, candidate.publicationDate)
  if (release.name !== `DepoAudio ${candidate.tag}`) fail('Private draft name no longer matches the release contract')
  if (releaseNotesContent(release.body ?? '', gate.version) !== releaseNotesContent(notes, gate.version)) {
    fail('Private draft notes content does not match the approved changelog section')
  }

  const specialNames = new Set([RECEIPT_NAME, UPDATER_NAME])
  const expectedPublicationNames = new Set(publicationAssetNames(candidate.updater))
  const candidateAssets = release.assets.filter(asset => !specialNames.has(asset.name))
  const existingPublicationAssets = release.assets.filter(asset => specialNames.has(asset.name))
  if (existingPublicationAssets.some(asset => !expectedPublicationNames.has(asset.name))) {
    fail('The private draft contains a publication asset forbidden by the updater decision')
  }
  const entries = await downloadAssets(candidateAssets, join(outputDir, 'candidate-assets'), token)
  const names = validateCandidateAssetSet(entries, gate.version, candidate.updater)
  const manifest = createManifest(entries)
  const manifestSha256 = sha256Buffer(Buffer.from(manifest, 'utf8'))
  assertSameManifest(manifestSha256, candidate.assetManifestSha256)
  await writeFile(join(outputDir, 'PREPUBLICATION-ASSET-MANIFEST.txt'), manifest, { flag: 'wx' })

  let latestContent = null
  if (candidate.updater === 'signed') {
    await verifyUpdaterSignatures(entries, names, candidate.updaterPublicKey)
    latestContent = await createUpdaterManifest({ repo, gate, candidate, entries, names, notes })
  }

  const gateSha256 = sha256Buffer(await readFile(GATE_PATH))
  const receipt = {
    schemaVersion: 1,
    release: {
      version: gate.version,
      tag: candidate.tag,
      releaseId: candidate.releaseId,
      sourceCommit: candidate.sourceCommit,
      publicationDate: candidate.publicationDate,
      approvedBy: candidate.approvedBy.trim(),
      updater: candidate.updater,
      updaterPublicKeySha256:
        candidate.updater === 'signed' ? parseMinisignPublicKey(candidate.updaterPublicKey).sha256 : null,
      approvedName: `DepoAudio ${candidate.tag}`,
      approvedBodySha256: sha256Buffer(Buffer.from(notes, 'utf8')),
    },
    gate: { sha256: gateSha256 },
    approval,
    assetManifest: {
      format: 'sha256  size  name; UTF-8; LF; bytewise filename order',
      sha256: manifestSha256,
      entries: entries.map(({ name, size, sha256 }) => ({ name, size, sha256 })).sort(compareAssetNames),
    },
  }
  let receiptContent = `${JSON.stringify(receipt, null, 2)}\n`
  const existingByName = new Map()
  if (existingPublicationAssets.length > 0) {
    const existingEntries = await downloadAssets(
      existingPublicationAssets,
      join(outputDir, 'existing-publication-assets'),
      token,
    )
    for (const entry of existingEntries) existingByName.set(entry.name, entry)
    const existingReceipt = existingByName.get(RECEIPT_NAME)
    if (existingReceipt) {
      receiptContent = await readFile(existingReceipt.path, 'utf8')
      validateApprovalReceipt(JSON.parse(receiptContent), {
        gate,
        candidate,
        approval,
        notes,
        manifestSha256,
        entries,
        gateSha256,
      })
    }
    const existingLatest = existingByName.get(UPDATER_NAME)
    if (existingLatest && (await readFile(existingLatest.path, 'utf8')) !== latestContent) {
      fail('Existing latest.json does not byte-match the approved updater manifest')
    }
  }
  await writeFile(join(outputDir, RECEIPT_NAME), receiptContent, { flag: 'wx' })
  if (latestContent !== null) await writeFile(join(outputDir, UPDATER_NAME), latestContent, { flag: 'wx' })
  const uploadPlan = publicationAssetNames(candidate.updater).filter(name => !existingByName.has(name))
  await writeFile(join(outputDir, UPLOAD_PLAN_NAME), `${JSON.stringify(uploadPlan, null, 2)}\n`, { flag: 'wx' })
  process.stdout.write(`Prepared publication evidence for private draft ${candidate.releaseId}\n`)
}

async function verifyPublicationFiles(outputDir, gate, candidate, approval) {
  const receiptPath = join(outputDir, RECEIPT_NAME)
  const receipt = await readJson(receiptPath)
  const approvedBody = extractReleaseNotes(
    await readFile(CHANGELOG_PATH, 'utf8'),
    gate.version,
    candidate.publicationDate,
  )
  validateApprovalReceipt(receipt, {
    gate,
    candidate,
    approval,
    notes: approvedBody,
    manifestSha256: candidate.assetManifestSha256,
    entries: receipt?.assetManifest?.entries ?? [],
    gateSha256: sha256Buffer(await readFile(GATE_PATH)),
  })
  const files = [{ name: RECEIPT_NAME, path: receiptPath, sha256: await sha256File(receiptPath) }]
  const latestPath = join(outputDir, UPDATER_NAME)
  if (candidate.updater === 'signed') {
    const latest = await readJson(latestPath)
    if (latest?.version !== gate.version || latest?.pub_date !== `${candidate.publicationDate}T00:00:00.000Z`) {
      fail('Local updater manifest no longer matches the release gate')
    }
    files.push({ name: UPDATER_NAME, path: latestPath, sha256: await sha256File(latestPath) })
  } else {
    try {
      await stat(latestPath)
      fail('Updater-unavailable publication must not contain latest.json')
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
    }
  }
  return files
}

async function publish({ repo, outputDir }, gate, candidate, token, approval) {
  const publicationFiles = await verifyPublicationFiles(outputDir, gate, candidate, approval)
  const publicationNames = new Set(publicationFiles.map(file => file.name))
  const release = await getRelease(repo, candidate.releaseId, token)
  validateRelease(release, candidate, { allowPublicationAssets: true })
  const candidateAssets = release.assets.filter(asset => !publicationNames.has(asset.name))
  const attachedPublicationAssets = release.assets.filter(asset => publicationNames.has(asset.name))
  if (attachedPublicationAssets.length !== publicationFiles.length) {
    fail('The exact publication evidence files are not attached to the private draft')
  }
  for (const expected of publicationFiles) {
    const remote = attachedPublicationAssets.find(asset => asset.name === expected.name)
    const verifyDir = join(outputDir, `verify-${expected.name.replaceAll('.', '-')}`)
    const [downloaded] = await downloadAssets([remote], verifyDir, token)
    if (downloaded.sha256 !== expected.sha256) fail(`Remote ${expected.name} does not match local approval evidence`)
  }

  const verifyEntries = await downloadAssets(candidateAssets, join(outputDir, 'verify-candidate-assets'), token)
  const names = validateCandidateAssetSet(verifyEntries, gate.version, candidate.updater)
  if (candidate.updater === 'signed') {
    await verifyUpdaterSignatures(verifyEntries, names, candidate.updaterPublicKey)
  }
  const manifestSha256 = sha256Buffer(Buffer.from(createManifest(verifyEntries), 'utf8'))
  assertSameManifest(manifestSha256, candidate.assetManifestSha256)
  const approvedBody = extractReleaseNotes(
    await readFile(CHANGELOG_PATH, 'utf8'),
    gate.version,
    candidate.publicationDate,
  )
  validateApprovalReceipt(await readJson(join(outputDir, RECEIPT_NAME)), {
    gate,
    candidate,
    approval,
    notes: approvedBody,
    manifestSha256,
    entries: verifyEntries,
    gateSha256: sha256Buffer(await readFile(GATE_PATH)),
  })
  if (candidate.updater === 'signed') {
    const expectedLatest = await createUpdaterManifest({
      repo,
      gate,
      candidate,
      entries: verifyEntries,
      names,
      notes: approvedBody,
    })
    if ((await readFile(join(outputDir, UPDATER_NAME), 'utf8')) !== expectedLatest) {
      fail('Local latest.json no longer matches the verified updater artifacts')
    }
  }

  const tagBeforePublication = await resolveTagCommit(repo, candidate.tag, token)
  if (tagBeforePublication !== null && tagBeforePublication !== candidate.sourceCommit) {
    fail(`Existing release tag ${candidate.tag} does not resolve to the approved source commit`)
  }

  // Re-fetch immediately before the bounded publication mutations. A missing
  // tag is first created at the approved source commit, then the exact release
  // ID is published. GitHub documents conditional GETs, but not an If-Match
  // precondition for release updates, so these metadata comparisons avoid
  // depending on an unsupported header.
  const immediatelyBeforePublish = await getRelease(repo, candidate.releaseId, token)
  validateRelease(immediatelyBeforePublish, candidate, { allowPublicationAssets: true })
  if (
    immediatelyBeforePublish.body !== release.body ||
    immediatelyBeforePublish.name !== release.name ||
    immediatelyBeforePublish.updated_at !== release.updated_at ||
    releaseAssetContract(immediatelyBeforePublish) !== releaseAssetContract(release)
  ) {
    fail('The private draft changed during final publication verification')
  }

  const approvedName = `DepoAudio ${candidate.tag}`
  await ensureTagAtCommit(repo, candidate.tag, candidate.sourceCommit, token)
  const releaseAfterTagCreation = await getRelease(repo, candidate.releaseId, token)
  validateRelease(releaseAfterTagCreation, candidate, { allowPublicationAssets: true })
  if (
    releaseAfterTagCreation.body !== immediatelyBeforePublish.body ||
    releaseAfterTagCreation.name !== immediatelyBeforePublish.name ||
    releaseAfterTagCreation.updated_at !== immediatelyBeforePublish.updated_at ||
    releaseAssetContract(releaseAfterTagCreation) !== releaseAssetContract(immediatelyBeforePublish)
  ) {
    fail('The private draft changed while binding the approved release tag')
  }
  const finalAssetContract = releaseAssetContract(releaseAfterTagCreation)
  const response = await githubRequest(`https://api.github.com/repos/${repo}/releases/${candidate.releaseId}`, token, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      name: approvedName,
      body: approvedBody,
      draft: false,
      prerelease: false,
      make_latest: 'true',
    }),
  })
  const published = await response.json()
  if (
    published?.id !== candidate.releaseId ||
    published?.draft !== false ||
    published?.prerelease !== false ||
    published?.tag_name !== candidate.tag ||
    published?.target_commitish !== candidate.sourceCommit ||
    published?.name !== approvedName ||
    published?.body !== approvedBody
  ) {
    fail('GitHub did not confirm the exact approved release publication')
  }
  const final = await getRelease(repo, candidate.releaseId, token)
  if (
    final?.id !== candidate.releaseId ||
    final?.draft !== false ||
    final?.tag_name !== candidate.tag ||
    final?.target_commitish !== candidate.sourceCommit ||
    final?.name !== approvedName ||
    final?.body !== approvedBody ||
    releaseAssetContract(final) !== finalAssetContract ||
    (await resolveTagCommit(repo, candidate.tag, token)) !== candidate.sourceCommit
  ) {
    fail('The published release does not match the approved release contract')
  }
  process.stdout.write(`Published exact release ${candidate.releaseId} for ${candidate.tag}\n`)
}

async function selfTest() {
  configureGitHubRequestMode('inspect')
  let rejectedInspectionMutation = false
  try {
    authorizeGitHubRequest('PATCH')
  } catch {
    rejectedInspectionMutation = true
  }
  if (!rejectedInspectionMutation) fail('Read-only inspection accepted a GitHub mutation')
  configureGitHubRequestMode('publish')
  if (authorizeGitHubRequest('PATCH') !== 'PATCH') fail('Publication request authorization is invalid')
  configureGitHubRequestMode('inspect')

  const publicKeyText =
    'untrusted comment: minisign public key E7620F1842B4E81F\n' +
    'RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3'
  const updaterPublicKey = Buffer.from(publicKeyText, 'utf8').toString('base64')
  const signatureText =
    'untrusted comment: signature from minisign secret key\n' +
    'RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\n' +
    'trusted comment: timestamp:1556193335\tfile:test\n' +
    'y/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg=='
  const updaterSignature = Buffer.from(signatureText, 'utf8').toString('base64')
  const gate = {
    schemaVersion: 2,
    version: '1.0.3',
    status: 'GO',
    blockers: [{ status: 'closed' }],
    candidate: {
      tag: 'v1.0.3',
      releaseId: 123,
      sourceCommit: 'a'.repeat(40),
      assetManifestSha256: 'b'.repeat(64),
      updater: 'signed',
      updaterPublicKey,
      publicationDate: '2026-08-25',
      approvedBy: 'Release Owner',
    },
  }
  const candidate = validateGate(gate, { phase: 'prepare', now: new Date('2026-08-25T12:00:00Z') })
  const assets = [
    { name: 'z.txt', size: 2, sha256: 'f'.repeat(64) },
    { name: 'a.txt', size: 1, sha256: 'e'.repeat(64) },
  ]
  const manifest = createManifest(assets)
  if (!manifest.startsWith(`${'e'.repeat(64)}  1  a.txt\n`)) fail('Manifest order is not deterministic')
  const names = {
    macArchive: 'DepoAudio_1.0.3_universal.app.tar.gz',
    windowsMsi: 'DepoAudio_1.0.3_x64_en-US.msi',
    windowsNsis: 'DepoAudio_1.0.3_x64-setup.exe',
  }
  const platforms = updaterPlatforms({
    repo: 'DepoStack/depo-audio',
    tag: candidate.tag,
    names,
    signatures: {
      [names.macArchive]: 'mac-signature',
      [names.windowsMsi]: 'msi-signature',
      [names.windowsNsis]: 'nsis-signature',
    },
  })
  if (
    platforms['darwin-universal'].signature !== 'mac-signature' ||
    platforms['windows-x86_64'].url !==
      'https://github.com/DepoStack/depo-audio/releases/download/v1.0.3/DepoAudio_1.0.3_x64_en-US.msi' ||
    platforms['windows-x86_64-nsis'].signature !== 'nsis-signature'
  ) {
    fail('Updater platform map is invalid')
  }
  verifyMinisignDigest(createHash('blake2b512').update('test').digest(), updaterSignature, candidate.updaterPublicKey)
  const requiredAssets = [
    'DepoAudio_1.0.3_universal.dmg',
    names.macArchive,
    names.windowsMsi,
    names.windowsNsis,
    ...requiredReleaseEvidence(gate.version),
  ].map((name, index) => ({ name, size: index + 1, sha256: 'c'.repeat(64) }))
  const signedAssets = [
    ...requiredAssets,
    `${names.macArchive}.sig`,
    `${names.windowsMsi}.sig`,
    `${names.windowsNsis}.sig`,
  ].map((entry, index) =>
    typeof entry === 'string' ? { name: entry, size: index + 10, sha256: 'd'.repeat(64) } : entry,
  )
  validateCandidateAssetSet(signedAssets, gate.version, 'signed')
  validateCandidateAssetSet(requiredAssets, gate.version, 'unavailable')
  const approvalCommit = 'd'.repeat(40)
  const firstApproval = validateApprovalContext({
    commit: approvalCommit,
    actor: 'release-owner',
    actorId: '1',
    runId: '10',
    runAttempt: '1',
    workflowRef: 'DepoStack/depo-audio/.github/workflows/publish-release.yml@refs/heads/main',
  })
  const retryApproval = { ...firstApproval, runId: '11', runAttempt: '2' }
  const retryNotes = '## [1.0.3] - 2026-08-25\n\nApproved notes.'
  const retryReceipt = {
    schemaVersion: 1,
    release: {
      version: gate.version,
      tag: candidate.tag,
      releaseId: candidate.releaseId,
      sourceCommit: candidate.sourceCommit,
      publicationDate: candidate.publicationDate,
      approvedBy: candidate.approvedBy,
      updater: candidate.updater,
      updaterPublicKeySha256: parseMinisignPublicKey(candidate.updaterPublicKey).sha256,
      approvedName: `DepoAudio ${candidate.tag}`,
      approvedBodySha256: sha256Buffer(Buffer.from(retryNotes, 'utf8')),
    },
    gate: { sha256: '9'.repeat(64) },
    approval: firstApproval,
    assetManifest: {
      sha256: candidate.assetManifestSha256,
      entries: signedAssets.map(({ name, size, sha256 }) => ({ name, size, sha256 })).sort(compareAssetNames),
    },
  }
  validateApprovalReceipt(retryReceipt, {
    gate,
    candidate,
    approval: retryApproval,
    notes: retryNotes,
    manifestSha256: candidate.assetManifestSha256,
    entries: signedAssets,
    gateSha256: '9'.repeat(64),
  })
  if (
    extractReleaseNotes(
      '# Changelog\n\n## [Unreleased]\n\n## [1.0.3] - 2026-08-25\n\nApproved notes.\n\n## [1.0.2] - 2026-08-20',
      gate.version,
      candidate.publicationDate,
    ) !== '## [1.0.3] - 2026-08-25\n\nApproved notes.'
  ) {
    fail('Release note extraction is invalid')
  }
  const validRelease = {
    id: 123,
    draft: true,
    prerelease: false,
    tag_name: candidate.tag,
    target_commitish: candidate.sourceCommit,
    assets: [{ id: 1, name: 'asset.bin', size: 1, state: 'uploaded', url: 'https://api.github.test/1' }],
  }
  validateRelease(validRelease, candidate)
  for (const [label, operation] of [
    [
      'NO-GO publication gate',
      () => validateGate({ ...gate, status: 'NO-GO' }, { phase: 'prepare', now: new Date('2026-08-25') }),
    ],
    [
      'open publication blocker',
      () =>
        validateGate({ ...gate, blockers: [{ status: 'open' }] }, { phase: 'prepare', now: new Date('2026-08-25') }),
    ],
    ['stale publication date', () => validateGate(gate, { phase: 'prepare', now: new Date('2026-08-26T00:00:00Z') })],
    ['wrong release ID', () => validateRelease({ ...validRelease, id: 124 }, candidate)],
    ['published release', () => validateRelease({ ...validRelease, draft: false }, candidate)],
    [
      'publication collision',
      () =>
        validateRelease({ ...validRelease, assets: [{ ...validRelease.assets[0], name: RECEIPT_NAME }] }, candidate),
    ],
    ['manifest drift', () => assertSameManifest('a'.repeat(64), 'b'.repeat(64))],
    [
      'tampered updater artifact',
      () =>
        verifyMinisignDigest(
          createHash('blake2b512').update('Test').digest(),
          updaterSignature,
          candidate.updaterPublicKey,
        ),
    ],
    [
      'missing signed updater signature',
      () => validateCandidateAssetSet(signedAssets.slice(0, -1), gate.version, 'signed'),
    ],
    [
      'missing JavaScript notice evidence',
      () =>
        validateCandidateAssetSet(
          signedAssets.filter(entry => !entry.name.endsWith('_javascript-third-party-notices.html')),
          gate.version,
          'signed',
        ),
    ],
    ['signature with unavailable updater', () => validateCandidateAssetSet(signedAssets, gate.version, 'unavailable')],
  ]) {
    let rejected = false
    try {
      operation()
    } catch {
      rejected = true
    }
    if (!rejected) fail(`Publication self-test accepted ${label}`)
  }
  const peeled = await peelTagObject({ type: 'tag', sha: 'c'.repeat(40) }, async () => ({
    object: { type: 'commit', sha: candidate.sourceCommit },
  }))
  if (peeled !== candidate.sourceCommit) fail('Annotated release tag did not peel to the source commit')
  process.stdout.write('Release publication self-test passed\n')
}

if (process.argv.includes('--self-test')) {
  await selfTest()
} else {
  const options = parseArguments(process.argv.slice(2))
  configureGitHubRequestMode(options.phase)
  validateRepo(options.repo)
  const gate = await readJson(GATE_PATH)
  const candidate = validateGate(gate, { phase: options.phase })
  const token = process.env.GH_TOKEN
  if (!token) fail('GH_TOKEN is required')
  const approval = options.phase === 'inspect' ? null : readApprovalContext()
  if (options.phase === 'inspect') await inspect(options, gate, candidate, token)
  else if (options.phase === 'prepare') await prepare(options, gate, candidate, token, approval)
  else await publish(options, gate, candidate, token, approval)
}
