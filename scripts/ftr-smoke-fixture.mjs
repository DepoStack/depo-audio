import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdir, mkdtemp, readFile, readdir, rename, rm, rmdir, stat, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const runnerTemp = process.env.RUNNER_TEMP?.trim()
if (runnerTemp && !path.isAbsolute(runnerTemp)) {
  throw new Error('RUNNER_TEMP must be an absolute path when configured')
}

const FIXTURE_PATH = runnerTemp
  ? path.join(runnerTemp, 'depoaudio-ftr-smoke', 'ftr-smoke.trm')
  : path.resolve('ftr-smoke.trm')
const FIXTURE_DIRECTORY = path.dirname(FIXTURE_PATH)
const FIXTURE_BASENAME = path.basename(FIXTURE_PATH)
const WORK_PREFIX = `${path.parse(FIXTURE_BASENAME).name}.${process.pid}`
const WAV_PATH = path.join(FIXTURE_DIRECTORY, `${WORK_PREFIX}.wav`)
const AAC_PATH = path.join(FIXTURE_DIRECTORY, `${WORK_PREFIX}.aac`)
const AVI_PATH = path.join(FIXTURE_DIRECTORY, `${WORK_PREFIX}.avi`)
const TEMPORARY_PATH = `${FIXTURE_PATH}.${process.pid}.tmp`
const TEMPORARY_PATTERN = new RegExp(
  `^(?:${FIXTURE_BASENAME.replace(/[.*+?^${}()|[\]\\]/gu, '\\$&')}\\.[0-9]+\\.tmp|ftr-smoke\\.[0-9]+\\.(?:wav|aac|avi))$`,
  'u',
)
const SMOKE_EXTENSIONS = ['wav', 'mp3', 'flac', 'opus', 'm4a']
const RIFF = Buffer.from('RIFF', 'ascii')
const AVI = Buffer.from('AVI ', 'ascii')
const AUDIO_STREAM = Buffer.from('auds', 'ascii')
const STREAM_FORMAT = Buffer.from('strf', 'ascii')
const AAC_ADTS_AVI_TAG = 0x1610
const FTR_AVI_TAG = 0x4180

function requireRunnerTemp() {
  if (!runnerTemp) throw new Error('RUNNER_TEMP is required for synthetic FTR fixture generation and cleanup')
}

function buildSyntheticToneWav() {
  const sampleRate = 24_000
  const durationSeconds = 1.25
  const sampleCount = Math.round(sampleRate * durationSeconds)
  const bytesPerSample = 2
  const dataLength = sampleCount * bytesPerSample
  const output = Buffer.alloc(44 + dataLength)

  output.write('RIFF', 0, 'ascii')
  output.writeUInt32LE(output.length - 8, 4)
  output.write('WAVE', 8, 'ascii')
  output.write('fmt ', 12, 'ascii')
  output.writeUInt32LE(16, 16)
  output.writeUInt16LE(1, 20)
  output.writeUInt16LE(1, 22)
  output.writeUInt32LE(sampleRate, 24)
  output.writeUInt32LE(sampleRate * bytesPerSample, 28)
  output.writeUInt16LE(bytesPerSample, 32)
  output.writeUInt16LE(16, 34)
  output.write('data', 36, 'ascii')
  output.writeUInt32LE(dataLength, 40)

  for (let sample = 0; sample < sampleCount; sample += 1) {
    const value = Math.round(Math.sin((2 * Math.PI * 440 * sample) / sampleRate) * 0.18 * 32_767)
    output.writeInt16LE(value, 44 + sample * bytesPerSample)
  }
  return output
}

function convertAacAviToFtr(aviBytes) {
  if (aviBytes.length < 32 || !aviBytes.subarray(0, 4).equals(RIFF) || !aviBytes.subarray(8, 12).equals(AVI)) {
    throw new Error('Synthetic fixture generation did not produce a RIFF AVI container')
  }
  if (aviBytes.indexOf(AUDIO_STREAM, 12) < 0) {
    throw new Error('Synthetic fixture generation did not produce an AVI audio stream')
  }

  const candidates = []
  let offset = aviBytes.indexOf(STREAM_FORMAT, 12)
  while (offset >= 0) {
    if (offset + 10 <= aviBytes.length) {
      const chunkSize = aviBytes.readUInt32LE(offset + 4)
      if (chunkSize >= 16 && offset + 8 + chunkSize <= aviBytes.length) {
        const formatTag = aviBytes.readUInt16LE(offset + 8)
        if (formatTag === AAC_ADTS_AVI_TAG) candidates.push(offset + 8)
      }
    }
    offset = aviBytes.indexOf(STREAM_FORMAT, offset + STREAM_FORMAT.length)
  }
  if (candidates.length !== 1) {
    throw new Error(`Synthetic fixture generation found ${candidates.length} AAC AVI format tags; expected exactly one`)
  }

  const ftrBytes = Buffer.from(aviBytes)
  ftrBytes.writeUInt16LE(FTR_AVI_TAG, candidates[0])
  return ftrBytes
}

function runFfmpeg(ffmpegPath, args, label) {
  const result = spawnSync(ffmpegPath, args, {
    encoding: 'utf8',
    timeout: 120_000,
    windowsHide: true,
  })
  if (result.error) throw new Error(`${label} could not start: ${result.error.message}`)
  if (result.status !== 0) {
    const detail = (result.stderr || result.stdout || '').trim().split(/\r?\n/u).at(-1)
    throw new Error(`${label} failed${detail ? `: ${detail}` : ''}`)
  }
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
  requireRunnerTemp()
  await rm(FIXTURE_PATH, { force: true })
  await cleanupTemporaryFiles()
  await cleanupDerivedSmokeOutputs()
  await rmdir(FIXTURE_DIRECTORY).catch(error => {
    if (!['ENOENT', 'ENOTEMPTY', 'EEXIST'].includes(error?.code)) throw error
  })
}

async function generateFixture(ffmpegArgument) {
  requireRunnerTemp()
  if (!ffmpegArgument) throw new Error('--generate requires --ffmpeg <path>')
  const ffmpegPath = path.resolve(ffmpegArgument)
  const ffmpegInfo = await stat(ffmpegPath).catch(() => undefined)
  if (!ffmpegInfo?.isFile()) throw new Error('The synthetic FTR fixture generator could not find the requested FFmpeg')
  if (await fixtureExists()) {
    throw new Error('Refusing to replace an existing ftr-smoke.trm; run the cleanup gate first')
  }

  await mkdir(FIXTURE_DIRECTORY, { recursive: true, mode: 0o700 })
  await cleanupTemporaryFiles()
  try {
    await writeFile(WAV_PATH, buildSyntheticToneWav(), { flag: 'wx', mode: 0o600 })
    runFfmpeg(
      ffmpegPath,
      [
        '-hide_banner',
        '-v',
        'error',
        '-xerror',
        '-i',
        WAV_PATH,
        '-ac',
        '1',
        '-c:a',
        'aac',
        '-b:a',
        '32k',
        '-f',
        'adts',
        '-y',
        AAC_PATH,
      ],
      'Synthetic AAC tone encoding',
    )
    runFfmpeg(
      ffmpegPath,
      [
        '-hide_banner',
        '-v',
        'error',
        '-xerror',
        '-f',
        'aac',
        '-i',
        AAC_PATH,
        '-c:a',
        'copy',
        '-tag:a',
        '0x1610',
        '-f',
        'avi',
        '-y',
        AVI_PATH,
      ],
      'Synthetic AAC-in-AVI muxing',
    )

    const ftrBytes = convertAacAviToFtr(await readFile(AVI_PATH))
    await writeFile(TEMPORARY_PATH, ftrBytes, { flag: 'wx', mode: 0o600 })
    await rename(TEMPORARY_PATH, FIXTURE_PATH)
    console.log(`Generated synthetic FTR decode fixture at ${FIXTURE_PATH}`)
  } finally {
    await Promise.all([
      rm(WAV_PATH, { force: true }),
      rm(AAC_PATH, { force: true }),
      rm(AVI_PATH, { force: true }),
      rm(TEMPORARY_PATH, { force: true }),
    ])
  }
}

async function runSelfTest() {
  const tone = buildSyntheticToneWav()
  assert.equal(tone.subarray(0, 4).toString('ascii'), 'RIFF')
  assert.equal(tone.subarray(8, 12).toString('ascii'), 'WAVE')
  assert.equal(tone.readUInt32LE(24), 24_000)
  assert.equal(tone.readUInt16LE(22), 1)

  const fakeAvi = Buffer.alloc(64)
  fakeAvi.write('RIFF', 0, 'ascii')
  fakeAvi.writeUInt32LE(fakeAvi.length - 8, 4)
  fakeAvi.write('AVI ', 8, 'ascii')
  fakeAvi.write('auds', 16, 'ascii')
  fakeAvi.write('strf', 24, 'ascii')
  fakeAvi.writeUInt32LE(16, 28)
  fakeAvi.writeUInt16LE(AAC_ADTS_AVI_TAG, 32)
  const fakeFtr = convertAacAviToFtr(fakeAvi)
  assert.equal(fakeAvi.readUInt16LE(32), AAC_ADTS_AVI_TAG)
  assert.equal(fakeFtr.readUInt16LE(32), FTR_AVI_TAG)
  assert.throws(() => convertAacAviToFtr(Buffer.alloc(32)), /RIFF AVI/u)

  const root = await mkdtemp(path.join(os.tmpdir(), 'depoaudio-ftr-fixture-test-'))
  const fixtureDirectory = path.join(root, 'depoaudio-ftr-smoke')
  const fixture = path.join(fixtureDirectory, 'ftr-smoke.trm')
  const orphan = `${fixture}.987654.tmp`
  const intermediate = path.join(fixtureDirectory, 'ftr-smoke.987654.aac')
  const derived = path.join(root, 'depoaudio-encoder-smoke.wav')
  const unrelated = path.join(root, 'keep.txt')

  try {
    await mkdir(fixtureDirectory, { recursive: true })
    await Promise.all([
      writeFile(fixture, 'fixture'),
      writeFile(orphan, 'partial'),
      writeFile(intermediate, 'intermediate'),
      writeFile(derived, 'derived'),
      writeFile(unrelated, 'keep'),
    ])

    const environment = { ...process.env, RUNNER_TEMP: root }
    const cleanup = spawnSync(process.execPath, [fileURLToPath(import.meta.url), '--clean'], {
      encoding: 'utf8',
      env: environment,
    })
    assert.equal(cleanup.status, 0, cleanup.stderr || 'fixture cleanup child failed')

    for (const removed of [fixture, orphan, intermediate, derived]) {
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

  console.log('Synthetic FTR fixture self-test passed')
}

function optionValue(name) {
  const index = process.argv.indexOf(name)
  return index >= 0 ? process.argv[index + 1] : undefined
}

const command = process.argv[2]
if (command === '--self-test') {
  await runSelfTest()
} else if (command === '--generate') {
  await generateFixture(optionValue('--ffmpeg'))
} else if (command === '--clean') {
  await cleanupFixture()
  console.log('Synthetic FTR fixture removed from the release workspace')
} else {
  throw new Error('Usage: node scripts/ftr-smoke-fixture.mjs --self-test|--generate --ffmpeg <path>|--clean')
}
