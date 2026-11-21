// Release smoke coverage for the legacy legal-video containers that DepoAudio
// lists as "audio track extracted" (MPEG program stream, VOB, MPEG-TS, M2TS,
// WMV/ASF). The bundled FFmpeg sidecars make one tiny synthetic fixture per
// container from a generated tone and generated video frames, then each
// sidecar must (1) see the expected audio codec, (2) decode the audio track
// with -xerror, and (3) extract audio-only WAV (plain RIFF and `-rf64 auto`)
// and M4A outputs with the same -map 0:a:0 and output options the app uses. A container is listed in the app only
// when its row here passes on macOS and Windows.
//
// Nothing is downloaded and no recording is used: every byte of input is made
// here. Fixtures live under RUNNER_TEMP and are removed by --clean.
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdir, mkdtemp, readFile, readdir, rm, rmdir, stat, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const runnerTemp = process.env.RUNNER_TEMP?.trim()
if (runnerTemp && !path.isAbsolute(runnerTemp)) {
  throw new Error('RUNNER_TEMP must be an absolute path when configured')
}

const FIXTURE_DIRECTORY = runnerTemp ? path.join(runnerTemp, 'depoaudio-container-smoke') : undefined
const FRAME_WIDTH = 64
const FRAME_HEIGHT = 48
const FRAME_RATE = 25
const SAMPLE_RATE = 48_000
const DURATION_SECONDS = 1.2

// One row per listed container. `extensions` are what the app accepts for it;
// `fixture` is the file this script makes; `audioCodec` is the typical audio
// codec for that container and the one the decode check must see.
export const CONTAINERS = [
  {
    label: 'MPEG-1 program stream',
    extensions: ['mpg', 'mpeg'],
    fixture: 'container-smoke.mpg',
    muxer: ['-f', 'mpeg'],
    videoCodec: 'mpeg1video',
    audioCodec: 'mp2',
  },
  {
    label: 'DVD VOB (MPEG-2 program stream)',
    extensions: ['vob'],
    fixture: 'container-smoke.vob',
    muxer: ['-f', 'vob'],
    videoCodec: 'mpeg2video',
    audioCodec: 'ac3',
  },
  {
    label: 'MPEG transport stream',
    extensions: ['ts'],
    fixture: 'container-smoke.ts',
    muxer: ['-f', 'mpegts'],
    videoCodec: 'mpeg2video',
    audioCodec: 'mp2',
  },
  {
    label: 'AVCHD / Blu-ray transport stream (192-byte packets)',
    extensions: ['mts', 'm2ts'],
    fixture: 'container-smoke.m2ts',
    muxer: ['-f', 'mpegts', '-mpegts_m2ts_mode', '1'],
    videoCodec: 'mpeg2video',
    audioCodec: 'ac3',
  },
  {
    label: 'Windows Media Video (ASF)',
    extensions: ['wmv', 'asf'],
    fixture: 'container-smoke.wmv',
    muxer: ['-f', 'asf'],
    videoCodec: 'wmv2',
    audioCodec: 'wmav2',
  },
]

// The output options the app adds for every conversion (helpers.rs
// output_args at 48 kHz): drop video and subtitle streams, then encode audio.
// A WAV that may pass 4 GiB also gets `-rf64 auto` (helpers.rs
// wav_rf64_args), so both WAV forms are run. The self-test checks this list
// against helpers.rs, so the smoke cannot drift from the app's arguments.
export const APP_OUTPUTS = [
  { extension: 'wav', args: ['-vn', '-sn', '-acodec', 'pcm_s16le', '-ar', '48000'] },
  { extension: 'wav', variant: 'rf64', args: ['-vn', '-sn', '-acodec', 'pcm_s16le', '-ar', '48000', '-rf64', 'auto'] },
  { extension: 'm4a', args: ['-vn', '-sn', '-c:a', 'aac', '-b:a', '128k', '-ar', '48000'] },
]

// The WAV and M4A arguments helpers.rs builds, read from its source: the
// `-vn -sn` prefix, the arm's string literals (`rate` as 48000), and the
// `-rf64 auto` pair from wav_rf64_args.
export function appOutputsFromHelpers(helpersSource) {
  const body = helpersSource.match(/pub\(crate\) fn output_args\([^)]*\)[^{]*\{([\s\S]*?)\n\}\n/u)?.[1]
  if (!body) throw new Error('helpers.rs has no output_args function')
  const literals = text =>
    [...text.matchAll(/"([^"]*)"\.into\(\)|\b(rate)\.into\(\)/gu)].map(match => (match[2] ? '48000' : match[1]))
  const arm = pattern => {
    const found = body.match(pattern)?.[1]
    if (!found) throw new Error(`helpers.rs output_args has no arm matching ${pattern}`)
    return literals(found)
  }
  const prefix = literals(body.match(/let mut args: Vec<String> = vec!\[([^\]]*)\]/u)?.[1] ?? '')
  const wav = arm(/\n\s*_ => vec!\[([^\]]*)\]/u)
  const m4a = arm(/"m4a" => vec!\[([^\]]*)\]/u)
  const rf64Body = helpersSource.match(/pub\(crate\) fn wav_rf64_args\([^)]*\)[^{]*\{([\s\S]*?)\n\}\n/u)?.[1]
  if (!rf64Body) throw new Error('helpers.rs has no wav_rf64_args function')
  const rf64 = literals(rf64Body.match(/_ => vec!\[([^\]]*)\]/u)?.[1] ?? '')
  return [
    { extension: 'wav', args: [...prefix, ...wav] },
    { extension: 'wav', variant: 'rf64', args: [...prefix, ...wav, ...rf64] },
    { extension: 'm4a', args: [...prefix, ...m4a] },
  ]
}

function requireRunnerTemp() {
  if (!runnerTemp) throw new Error('RUNNER_TEMP is required for container smoke fixtures')
}

export function buildToneWav() {
  const sampleCount = Math.round(SAMPLE_RATE * DURATION_SECONDS)
  const output = Buffer.alloc(44 + sampleCount * 2)
  output.write('RIFF', 0, 'ascii')
  output.writeUInt32LE(output.length - 8, 4)
  output.write('WAVE', 8, 'ascii')
  output.write('fmt ', 12, 'ascii')
  output.writeUInt32LE(16, 16)
  output.writeUInt16LE(1, 20)
  output.writeUInt16LE(1, 22)
  output.writeUInt32LE(SAMPLE_RATE, 24)
  output.writeUInt32LE(SAMPLE_RATE * 2, 28)
  output.writeUInt16LE(2, 32)
  output.writeUInt16LE(16, 34)
  output.write('data', 36, 'ascii')
  output.writeUInt32LE(sampleCount * 2, 40)
  for (let sample = 0; sample < sampleCount; sample += 1) {
    const value = Math.round(Math.sin((2 * Math.PI * 440 * sample) / SAMPLE_RATE) * 0.2 * 32_767)
    output.writeInt16LE(value, 44 + sample * 2)
  }
  return output
}

// Raw yuv420p frames: a grey picture whose brightness steps each frame.
export function buildRawFrames() {
  const frameCount = Math.round(FRAME_RATE * DURATION_SECONDS)
  const luma = FRAME_WIDTH * FRAME_HEIGHT
  const frameSize = luma + 2 * (luma / 4)
  const output = Buffer.alloc(frameCount * frameSize)
  for (let frame = 0; frame < frameCount; frame += 1) {
    const offset = frame * frameSize
    output.fill(40 + ((frame * 6) % 180), offset, offset + luma)
    output.fill(128, offset + luma, offset + frameSize)
  }
  return output
}

function run(binary, args, label) {
  const result = spawnSync(binary, args, { encoding: 'utf8', timeout: 120_000, windowsHide: true })
  if (result.error) throw new Error(`${label} could not start: ${result.error.message}`)
  if (result.status !== 0) {
    const detail = (result.stderr || result.stdout || '').trim().split(/\r?\n/u).at(-1)
    throw new Error(`${label} failed${detail ? `: ${detail}` : ''}`)
  }
  return result.stdout
}

async function requireFile(argument, flag) {
  if (!argument) throw new Error(`${flag} <path> is required`)
  const resolved = path.resolve(argument)
  const info = await stat(resolved).catch(() => undefined)
  if (!info?.isFile()) throw new Error(`Container smoke could not find ${flag} ${argument}`)
  return resolved
}

function streams(ffprobe, file, label) {
  const json = run(
    ffprobe,
    ['-v', 'error', '-show_entries', 'stream=codec_type,codec_name', '-of', 'json', file],
    `${label} probe`,
  )
  return JSON.parse(json).streams ?? []
}

async function generate(ffmpegArgument) {
  requireRunnerTemp()
  const ffmpeg = await requireFile(ffmpegArgument, '--ffmpeg')
  await mkdir(FIXTURE_DIRECTORY, { recursive: true, mode: 0o700 })
  const tone = path.join(FIXTURE_DIRECTORY, 'source-tone.wav')
  const frames = path.join(FIXTURE_DIRECTORY, 'source-frames.yuv')
  try {
    await writeFile(tone, buildToneWav(), { flag: 'wx', mode: 0o600 })
    await writeFile(frames, buildRawFrames(), { flag: 'wx', mode: 0o600 })
    for (const container of CONTAINERS) {
      const fixture = path.join(FIXTURE_DIRECTORY, container.fixture)
      const exists = await stat(fixture).catch(() => undefined)
      if (exists) throw new Error(`Refusing to replace ${container.fixture}; run --clean first`)
      run(
        ffmpeg,
        [
          '-hide_banner',
          '-v',
          'error',
          '-xerror',
          '-f',
          'rawvideo',
          '-pix_fmt',
          'yuv420p',
          '-video_size',
          `${FRAME_WIDTH}x${FRAME_HEIGHT}`,
          '-framerate',
          String(FRAME_RATE),
          '-i',
          frames,
          '-i',
          tone,
          '-map',
          '0:v:0',
          '-map',
          '1:a:0',
          '-c:v',
          container.videoCodec,
          '-c:a',
          container.audioCodec,
          '-shortest',
          ...container.muxer,
          fixture,
        ],
        `${container.label} fixture generation`,
      )
      console.log(`Generated ${container.label} fixture ${container.fixture}`)
    }
  } finally {
    await Promise.all([rm(tone, { force: true }), rm(frames, { force: true })])
  }
}

async function check(ffmpegArgument, ffprobeArgument) {
  requireRunnerTemp()
  const ffmpeg = await requireFile(ffmpegArgument, '--ffmpeg')
  const ffprobe = await requireFile(ffprobeArgument, '--ffprobe')
  const tag = path.basename(ffmpeg).replace(/[^A-Za-z0-9_-]/gu, '_')
  for (const container of CONTAINERS) {
    const fixture = path.join(FIXTURE_DIRECTORY, container.fixture)
    await requireFile(fixture, `fixture for ${container.label}`)

    const found = streams(ffprobe, fixture, container.label)
    const audio = found.filter(stream => stream.codec_type === 'audio')
    if (audio.length !== 1 || audio[0].codec_name !== container.audioCodec) {
      throw new Error(
        `${container.label}: expected one ${container.audioCodec} audio stream, found ${JSON.stringify(found)}`,
      )
    }
    if (!found.some(stream => stream.codec_type === 'video')) {
      throw new Error(`${container.label}: the fixture has no video stream to discard`)
    }

    run(
      ffmpeg,
      ['-hide_banner', '-v', 'error', '-xerror', '-i', fixture, '-map', '0:a:0', '-f', 'null', '-'],
      `${container.label} audio decode`,
    )

    for (const output of APP_OUTPUTS) {
      const target = path.join(
        FIXTURE_DIRECTORY,
        `${tag}-${path.parse(container.fixture).ext.slice(1)}${output.variant ? `-${output.variant}` : ''}.${output.extension}`,
      )
      try {
        run(
          ffmpeg,
          // The app maps the converted stream explicitly (helpers.rs
          // source_audio_map_args) instead of FFmpeg's automatic selection.
          ['-hide_banner', '-v', 'error', '-xerror', '-i', fixture, '-map', '0:a:0', ...output.args, '-y', target],
          `${container.label} to ${output.variant ?? output.extension}`,
        )
        const written = streams(ffprobe, target, `${container.label} ${output.extension} output`)
        if (written.length !== 1 || written[0].codec_type !== 'audio') {
          throw new Error(
            `${container.label}: ${output.extension} output must hold only one audio stream, found ${JSON.stringify(written)}`,
          )
        }
      } finally {
        await rm(target, { force: true })
      }
    }
    console.log(`${path.basename(ffmpeg)}: ${container.label} (${container.extensions.join(', ')}) decodes`)
  }
}

async function clean() {
  requireRunnerTemp()
  let entries
  try {
    entries = await readdir(FIXTURE_DIRECTORY, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT') return
    throw error
  }
  await Promise.all(
    entries.filter(entry => entry.isFile()).map(entry => rm(path.join(FIXTURE_DIRECTORY, entry.name), { force: true })),
  )
  await rmdir(FIXTURE_DIRECTORY).catch(error => {
    if (!['ENOENT', 'ENOTEMPTY', 'EEXIST'].includes(error?.code)) throw error
  })
}

// The list here, the Rust detection arm and the file picker must agree, so a
// container cannot be listed in the app without a smoke fixture.
export function listedLegacyContainers(helpersSource) {
  const arm = helpersSource.match(/\/\/ Legacy legal-video containers[^\n]*\n\s*((?:"[a-z0-9]+"(?:\s*\|\s*)?)+)\s*=>/u)
  if (!arm) throw new Error('helpers.rs has no legacy legal-video container arm')
  return [...arm[1].matchAll(/"([a-z0-9]+)"/gu)].map(match => match[1])
}

async function selfTest() {
  const tone = buildToneWav()
  assert.equal(tone.subarray(0, 4).toString('ascii'), 'RIFF')
  assert.equal(tone.readUInt32LE(24), SAMPLE_RATE)
  const frames = buildRawFrames()
  assert.equal(frames.length % ((FRAME_WIDTH * FRAME_HEIGHT * 3) / 2), 0)

  const repository = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
  const helpers = await readFile(path.join(repository, 'src-tauri/src/helpers.rs'), 'utf8')
  const fileDrop = await readFile(path.join(repository, 'src/hooks/useFileDrop.js'), 'utf8')
  const constants = await readFile(path.join(repository, 'src/constants.js'), 'utf8')
  assert.deepEqual(APP_OUTPUTS, appOutputsFromHelpers(helpers), 'smoke output arguments must match helpers.rs')
  assert.ok(helpers.includes('pub(crate) fn source_audio_map_args()'), 'the app must still map 0:a:0 explicitly')
  const smoked = CONTAINERS.flatMap(container => container.extensions).sort()
  assert.deepEqual(listedLegacyContainers(helpers).sort(), smoked, 'helpers.rs containers must match the smoke list')
  for (const extension of smoked) {
    assert.ok(fileDrop.includes(`'${extension}'`), `the file picker must offer .${extension}`)
    assert.ok(constants.includes(`.${extension}`), `the format table must list .${extension}`)
  }
  assert.equal(new Set(CONTAINERS.map(container => container.fixture)).size, CONTAINERS.length)

  const root = await mkdtemp(path.join(os.tmpdir(), 'depoaudio-container-smoke-test-'))
  try {
    const directory = path.join(root, 'depoaudio-container-smoke')
    const unrelated = path.join(root, 'keep.txt')
    await mkdir(directory)
    await Promise.all([
      writeFile(path.join(directory, 'container-smoke.mpg'), 'fixture'),
      writeFile(path.join(directory, 'ffmpeg-mpg.wav'), 'derived'),
      writeFile(unrelated, 'keep'),
    ])
    const cleanup = spawnSync(process.execPath, [fileURLToPath(import.meta.url), '--clean'], {
      encoding: 'utf8',
      env: { ...process.env, RUNNER_TEMP: root },
    })
    assert.equal(cleanup.status, 0, cleanup.stderr || 'container fixture cleanup failed')
    await assert.rejects(stat(directory), error => error?.code === 'ENOENT')
    assert.equal((await stat(unrelated)).isFile(), true)

    const relative = spawnSync(process.execPath, [fileURLToPath(import.meta.url), '--clean'], {
      encoding: 'utf8',
      env: { ...process.env, RUNNER_TEMP: 'relative-runner-temp' },
    })
    assert.notEqual(relative.status, 0, 'relative RUNNER_TEMP unexpectedly passed')
  } finally {
    await rm(root, { force: true, recursive: true })
  }
  console.log('Container smoke fixture self-test passed')
}

function optionValue(name) {
  const index = process.argv.indexOf(name)
  return index >= 0 ? process.argv[index + 1] : undefined
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const command = process.argv[2]
  if (command === '--self-test') await selfTest()
  else if (command === '--generate') await generate(optionValue('--ffmpeg'))
  else if (command === '--check') await check(optionValue('--ffmpeg'), optionValue('--ffprobe'))
  else if (command === '--clean') {
    await clean()
    console.log('Container smoke fixtures removed from the release workspace')
  } else {
    throw new Error(
      'Usage: node scripts/container-smoke-fixtures.mjs --self-test|--generate --ffmpeg <path>|--check --ffmpeg <path> --ffprobe <path>|--clean',
    )
  }
}
