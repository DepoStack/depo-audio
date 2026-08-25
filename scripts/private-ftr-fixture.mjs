import { createHash, createHmac } from 'node:crypto'
import { open, rename, rm, stat } from 'node:fs/promises'
import path from 'node:path'

const FIXTURE_PATH = path.resolve('ftr-smoke.trm')
const TEMPORARY_PATH = `${FIXTURE_PATH}.${process.pid}.tmp`

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

async function downloadFixture() {
  const contract = readFixtureContract()
  if (await fixtureExists()) {
    throw new Error('Refusing to replace an existing ftr-smoke.trm; run the cleanup gate first')
  }

  await rm(TEMPORARY_PATH, { force: true })
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

    handle = await open(TEMPORARY_PATH, 'wx')
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

const command = process.argv[2]
if (command === '--check') {
  readFixtureContract()
  console.log('Private FTR fixture release-secret contract is configured')
} else if (command === '--download') {
  await downloadFixture()
} else if (command === '--clean') {
  await rm(FIXTURE_PATH, { force: true })
  await rm(TEMPORARY_PATH, { force: true })
  console.log('Private FTR fixture removed from the release workspace')
} else {
  throw new Error('Usage: node scripts/private-ftr-fixture.mjs --check|--download|--clean')
}
