import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { createHash, createHmac } from 'node:crypto'
import { mkdir, mkdtemp, open, readdir, rename, rm, rmdir, stat, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const configuredFixturePath = process.env.FTR_FIXTURE_PATH?.trim()
if (configuredFixturePath && !path.isAbsolute(configuredFixturePath)) {
  throw new Error('FTR_FIXTURE_PATH must be an absolute path when configured')
}
const runnerTemp = process.env.RUNNER_TEMP?.trim()
if (runnerTemp && !path.isAbsolute(runnerTemp)) {
  throw new Error('RUNNER_TEMP must be an absolute path when configured')
}

const FIXTURE_PATH = configuredFixturePath
  ? path.normalize(configuredFixturePath)
  : runnerTemp
    ? path.join(runnerTemp, 'depoaudio-private-ftr', 'ftr-smoke.trm')
    : path.resolve('ftr-smoke.trm')
const FIXTURE_DIRECTORY = path.dirname(FIXTURE_PATH)
const FIXTURE_BASENAME = path.basename(FIXTURE_PATH)
const TEMPORARY_PATH = `${FIXTURE_PATH}.${process.pid}.tmp`
const TEMPORARY_PATTERN = new RegExp(
  `^${FIXTURE_BASENAME.replace(/[.*+?^${}()|[\]\\]/gu, '\\$&')}\\.[0-9]+\\.tmp$`,
  'u',
)
const SMOKE_EXTENSIONS = ['wav', 'mp3', 'flac', 'opus', 'm4a']

function requiredSecret(name) {
  const value = process.env[name]?.trim()
  if (!value) throw new Error(`Required release secret ${name} is not configured`)
  return value
}

function readFixtureContract() {
  const source = requiredSecret('FTR_FIXTURE_URL')
  let sourceUrl
  try {
    sourceUrl = new URL(source)
  } catch {
    throw new Error('FTR_FIXTURE_URL must be a valid URL')
  }
  if (sourceUrl.protocol !== 'https:') throw new Error('FTR_FIXTURE_URL must use HTTPS')
  if (sourceUrl.username || sourceUrl.password) {
    throw new Error('FTR_FIXTURE_URL must not contain embedded credentials')
  }

  const authorization = requiredSecret('FTR_FIXTURE_AUTHORIZATION')
  if (/[\r\n]/u.test(authorization)) {
    throw new Error('FTR_FIXTURE_AUTHORIZATION must be a single HTTP Authorization value')
  }

  const expectedSizeText = requiredSecret('FTR_FIXTURE_SIZE')
  if (!/^[1-9][0-9]*$/u.test(expectedSizeText)) {
    throw new Error('FTR_FIXTURE_SIZE must be a positive integer byte count')
  }
  const expectedSize = Number(expectedSizeText)
  if (!Number.isSafeInteger(expectedSize)) {
    throw new Error('FTR_FIXTURE_SIZE exceeds the supported safe integer range')
  }

  const expectedSha256 = requiredSecret('FTR_FIXTURE_SHA256').toLowerCase()
  if (!/^[0-9a-f]{64}$/u.test(expectedSha256)) {
    throw new Error('FTR_FIXTURE_SHA256 must be exactly 64 hexadecimal characters')
  }

  const evidenceId = requiredSecret('FTR_FIXTURE_EVIDENCE_ID')
  if (evidenceId.length > 256 || /[\u0000-\u001f\u007f]/u.test(evidenceId)) {
    throw new Error('FTR_FIXTURE_EVIDENCE_ID must be a short, single-line evidence-record identifier')
  }

  const evidenceFingerprint = createHmac('sha256', evidenceId)
    .update(sourceUrl.href)
    .update('\0')
    .update(expectedSizeText)
    .update('\0')
    .update(expectedSha256)
    .digest('hex')

  return { sourceUrl, authorization, expectedSize, expectedSha256, evidenceFingerprint }
}

async function fixtureExists() {
  try {
    await stat(FIXTURE_PATH)
    return true
  } catch (error) {
    if (error?.code === 'ENOENT') return false
    throw error
  }
}

async function cleanupTemporaryFiles() {
  let entries
  try {
    entries = await readdir(FIXTURE_DIRECTORY, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT') return
    throw error
  }

  await Promise.all(
    entries
      .filter(entry => entry.isFile() && TEMPORARY_PATTERN.test(entry.name))
      .map(entry => rm(path.join(FIXTURE_DIRECTORY, entry.name), { force: true })),
  )
}

async function cleanupDerivedSmokeOutputs() {
  if (!runnerTemp) return

  const prefixes = ['ffmpeg-aarch64-apple-darwin-smoke', 'ffmpeg-x86_64-apple-darwin-smoke', 'depoaudio-encoder-smoke']
  await Promise.all(
    prefixes.flatMap(prefix =>
      SMOKE_EXTENSIONS.map(extension => rm(path.join(runnerTemp, `${prefix}.${extension}`), { force: true })),
    ),
  )
}

async function cleanupFixture() {
  await rm(FIXTURE_PATH, { force: true })
  await cleanupTemporaryFiles()
  await cleanupDerivedSmokeOutputs()
  if (configuredFixturePath || runnerTemp)
    await rmdir(FIXTURE_DIRECTORY).catch(error => {
      if (!['ENOENT', 'ENOTEMPTY', 'EEXIST'].includes(error?.code)) throw error
    })
}

async function downloadFixture() {
  const contract = readFixtureContract()
  if (await fixtureExists()) {
    throw new Error('Refusing to replace an existing ftr-smoke.trm; run the cleanup gate first')
  }

  await mkdir(FIXTURE_DIRECTORY, { recursive: true, mode: 0o700 })
  await cleanupTemporaryFiles()
  let handle
  try {
    let response
    try {
      response = await fetch(contract.sourceUrl, {
        headers: {
          Accept: 'application/octet-stream',
          'Accept-Encoding': 'identity',
          Authorization: contract.authorization,
          'Cache-Control': 'no-store',
        },
        redirect: 'error',
        signal: AbortSignal.timeout(120_000),
      })
    } catch {
      throw new Error('Private FTR fixture request failed before a valid HTTPS response was received')
    }
    if (!response.ok) throw new Error(`Private FTR fixture request failed with HTTP ${response.status}`)
    if (new URL(response.url).protocol !== 'https:') {
      throw new Error('Private FTR fixture redirects must remain on HTTPS')
    }
    if (!response.body) throw new Error('Private FTR fixture response has no body')
    const contentEncoding = response.headers.get('content-encoding')
    if (contentEncoding && contentEncoding.toLowerCase() !== 'identity') {
      throw new Error('Private FTR fixture response must not transform the approved bytes')
    }

    const contentLength = response.headers.get('content-length')
    if (contentLength !== null && /^\d+$/u.test(contentLength)) {
      if (Number(contentLength) !== contract.expectedSize) {
        throw new Error('Private FTR fixture Content-Length does not match FTR_FIXTURE_SIZE')
      }
    }

    handle = await open(TEMPORARY_PATH, 'wx', 0o600)
    const digest = createHash('sha256')
    let received = 0
    try {
      for await (const chunk of response.body) {
        received += chunk.byteLength
        if (received > contract.expectedSize) {
          throw new Error('Private FTR fixture exceeded FTR_FIXTURE_SIZE')
        }
        digest.update(chunk)
        await handle.write(chunk)
      }
    } catch (error) {
      if (error?.message === 'Private FTR fixture exceeded FTR_FIXTURE_SIZE') throw error
      throw new Error('Private FTR fixture response could not be read safely')
    }
    await handle.sync()
    await handle.close()
    handle = undefined

    if (received !== contract.expectedSize) {
      throw new Error('Private FTR fixture byte count does not match FTR_FIXTURE_SIZE')
    }
    if (digest.digest('hex') !== contract.expectedSha256) {
      throw new Error('Private FTR fixture SHA-256 does not match FTR_FIXTURE_SHA256')
    }

    await rename(TEMPORARY_PATH, FIXTURE_PATH)
    console.log(`Permission-cleared private FTR fixture verified (evidence contract ${contract.evidenceFingerprint})`)
  } finally {
    if (handle) await handle.close().catch(() => {})
    await rm(TEMPORARY_PATH, { force: true })
  }
}

async function runSelfTest() {
  const root = await mkdtemp(path.join(os.tmpdir(), 'depoaudio-ftr-fixture-test-'))
  const privateDirectory = path.join(root, 'depoaudio-private-ftr')
  const fixture = path.join(privateDirectory, 'ftr-smoke.trm')
  const orphan = `${fixture}.987654.tmp`
  const derived = path.join(root, 'depoaudio-encoder-smoke.wav')
  const unrelated = path.join(root, 'keep.txt')

  try {
    await mkdir(privateDirectory, { recursive: true })
    await Promise.all([
      writeFile(fixture, 'fixture'),
      writeFile(orphan, 'partial'),
      writeFile(derived, 'derived'),
      writeFile(unrelated, 'keep'),
    ])

    const environment = { ...process.env, RUNNER_TEMP: root }
    delete environment.FTR_FIXTURE_PATH
    const cleanup = spawnSync(process.execPath, [fileURLToPath(import.meta.url), '--clean'], {
      encoding: 'utf8',
      env: environment,
    })
    assert.equal(cleanup.status, 0, cleanup.stderr || 'fixture cleanup child failed')

    for (const removed of [fixture, orphan, derived]) {
      await assert.rejects(stat(removed), error => error?.code === 'ENOENT')
    }
    assert.equal((await stat(unrelated)).isFile(), true)

    const invalidPath = spawnSync(process.execPath, [fileURLToPath(import.meta.url), '--clean'], {
      encoding: 'utf8',
      env: { ...environment, RUNNER_TEMP: 'relative-runner-temp' },
    })
    assert.notEqual(invalidPath.status, 0, 'relative RUNNER_TEMP unexpectedly passed')
  } finally {
    await rm(root, { force: true, recursive: true })
  }

  console.log('Private FTR fixture self-test passed')
}

const command = process.argv[2]
if (command === '--self-test') {
  await runSelfTest()
} else if (command === '--check') {
  readFixtureContract()
  console.log('Private FTR fixture release-secret contract is configured')
} else if (command === '--download') {
  await downloadFixture()
} else if (command === '--clean') {
  await cleanupFixture()
  console.log('Private FTR fixture removed from the release workspace')
} else {
  throw new Error('Usage: node scripts/private-ftr-fixture.mjs --self-test|--check|--download|--clean')
}
