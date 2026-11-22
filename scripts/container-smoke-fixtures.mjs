// Release smoke coverage for the legacy legal-video containers that DepoAudio
// lists as "audio track extracted" (MPEG program stream, VOB, MPEG-TS, M2TS,
// WMV/ASF). The bundled FFmpeg sidecars make one tiny synthetic fixture per
// container from a generated tone and generated video frames, then each
// sidecar must (1) see the expected audio codec, (2) decode the audio track
// with -xerror, and (3) extract audio-only WAV (plain RIFF and `-rf64 auto`)
// and M4A outputs with the same -map 0:a:0 and output options the app uses. A container is listed in the app only
// when its row here passes on macOS and Windows.
//
// DigitalCAT .dm files are read by DepoAudio itself (src-tauri/src/dm.rs),
// which writes the audio into a temporary WAV for FFmpeg. FFmpeg never sees a
// .dm, so the smoke here makes synthetic .dm files in the documented layout
// (docs/formats/dm.md), reads them into the same WAV layout dm.rs writes, and
// requires each sidecar to decode that WAV and convert it like the app. The
// Rust tests check dm.rs itself against FFmpeg on PATH; this checks that the
// bundled sidecars have the decoders those WAVs need. GSM 6.10, the
// SoundSync default, has no LGPL encoder in the sidecars, so for GSM only the
// decoder's presence is checked.
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

export function buildToneWav(sampleRate = SAMPLE_RATE) {
  const sampleCount = Math.round(sampleRate * DURATION_SECONDS)
  const output = Buffer.alloc(44 + sampleCount * 2)
  output.write('RIFF', 0, 'ascii')
  output.writeUInt32LE(output.length - 8, 4)
  output.write('WAVE', 8, 'ascii')
  output.write('fmt ', 12, 'ascii')
  output.writeUInt32LE(16, 16)
  output.writeUInt16LE(1, 20)
  output.writeUInt16LE(1, 22)
  output.writeUInt32LE(sampleRate, 24)
  output.writeUInt32LE(sampleRate * 2, 28)
  output.writeUInt16LE(2, 32)
  output.writeUInt16LE(16, 34)
  output.write('data', 36, 'ascii')
  output.writeUInt32LE(sampleCount * 2, 40)
  for (let sample = 0; sample < sampleCount; sample += 1) {
    const value = Math.round(Math.sin((2 * Math.PI * 440 * sample) / sampleRate) * 0.2 * 32_767)
    output.writeInt16LE(value, 44 + sample * 2)
  }
  return output
}

// ── DigitalCAT .dm ──────────────────────────────────────────────────────────

// Decoders the WAVs read from .dm files need, for the formats SoundSync
// offers besides plain PCM. build-ffmpeg-macos.sh and the Windows release
// step gate on the same list.
export const DM_DECODERS = ['gsm_ms', 'adpcm_ima_wav', 'adpcm_ms', 'pcm_alaw', 'pcm_mulaw']
const DM_SAMPLE_RATE = 11_025
// Block 1 of every fixture is marked recorded over (stamp 0) and must be
// left out.
const DM_ERASED_BLOCK = 1
const DM_FIRST_STAMP = 1_763_800_000
export const DM_FIXTURES = [
  { label: 'DigitalCAT .dm (PCM)', fixture: 'dm-smoke-pcm.dm', encoder: null, codec: 'pcm_s16le' },
  { label: 'DigitalCAT .dm (A-law)', fixture: 'dm-smoke-alaw.dm', encoder: 'pcm_alaw', codec: 'pcm_alaw' },
  { label: 'DigitalCAT .dm (mu-law)', fixture: 'dm-smoke-mulaw.dm', encoder: 'pcm_mulaw', codec: 'pcm_mulaw' },
  { label: 'DigitalCAT .dm (MS ADPCM)', fixture: 'dm-smoke-adpcm-ms.dm', encoder: 'adpcm_ms', codec: 'adpcm_ms' },
  {
    label: 'DigitalCAT .dm (IMA ADPCM)',
    fixture: 'dm-smoke-adpcm-ima.dm',
    encoder: 'adpcm_ima_wav',
    codec: 'adpcm_ima_wav',
  },
]

// The `fmt ` and `data` chunk bodies of a RIFF WAVE buffer.
export function wavChunks(buffer) {
  if (buffer.subarray(0, 4).toString('ascii') !== 'RIFF' || buffer.subarray(8, 12).toString('ascii') !== 'WAVE') {
    throw new Error('not a RIFF WAVE file')
  }
  const chunks = {}
  let at = 12
  while (at + 8 <= buffer.length) {
    const id = buffer.subarray(at, at + 4).toString('ascii')
    const size = buffer.readUInt32LE(at + 4)
    chunks[id] = buffer.subarray(at + 8, Math.min(at + 8 + size, buffer.length))
    at += 8 + size + (size % 2)
  }
  if (!chunks['fmt '] || !chunks.data) throw new Error('WAVE file has no fmt or data chunk')
  return { fmt: chunks['fmt '], data: chunks.data }
}

// The block size the recorder picks: a quarter second of whole codec blocks.
export function dmBlockSize(fmt) {
  const align = fmt.readUInt16LE(12)
  return Math.max(1, Math.floor(fmt.readUInt32LE(8) / align) >> 2) * align
}

// A .dm: 16-byte header (version 1, header size 8, format size, block size),
// the WAVEFORMATEX, then blocks of [u32 stamp][u32 length][bytes]. Stamps
// rise one second every four blocks; DM_ERASED_BLOCK gets stamp 0.
export function buildDm(fmt, data) {
  const blockSize = dmBlockSize(fmt)
  const header = Buffer.alloc(16)
  header.writeUInt32LE(1, 0)
  header.writeUInt32LE(8, 4)
  header.writeUInt32LE(fmt.length, 8)
  header.writeUInt32LE(blockSize, 12)
  const parts = [header, fmt]
  for (let index = 0; index * blockSize < data.length; index += 1) {
    const payload = data.subarray(index * blockSize, (index + 1) * blockSize)
    const blockHeader = Buffer.alloc(8)
    blockHeader.writeUInt32LE(index === DM_ERASED_BLOCK ? 0 : DM_FIRST_STAMP + Math.floor(index / 4), 0)
    blockHeader.writeUInt32LE(payload.length, 4)
    parts.push(blockHeader, payload)
  }
  return Buffer.concat(parts)
}

// Read a .dm into the WAV layout dm.rs writes: its WAVEFORMATEX as `fmt `
// (padded to even), a `fact` chunk for formats outside the PCM family when
// the samples are countable, and the blocks that are not recorded over, in
// file order, as `data`. A short last block keeps only whole codec blocks.
export function demuxDm(buffer) {
  if (buffer.length < 16 || buffer.readUInt32LE(4) !== 8) throw new Error('not a .dm audio header')
  const formatSize = buffer.readUInt32LE(8)
  const blockSize = buffer.readUInt32LE(12)
  if (formatSize < 16 || 16 + formatSize > buffer.length || blockSize === 0) throw new Error('bad .dm header')
  const fmt = buffer.subarray(16, 16 + formatSize)
  const tag = fmt.readUInt16LE(0)
  const align = fmt.readUInt16LE(12)
  const kept = []
  let at = 16 + formatSize
  while (at < buffer.length) {
    if (at + 8 > buffer.length) throw new Error('.dm ends inside a block header')
    const stamp = buffer.readUInt32LE(at)
    const length = buffer.readUInt32LE(at + 4)
    if (length === 0 || length > blockSize || at + 8 + length > buffer.length) throw new Error('bad .dm block')
    const keep = length < blockSize ? length - (length % align) : length
    if (stamp !== 0) kept.push(buffer.subarray(at + 8, at + 8 + keep))
    at += 8 + length
  }
  const data = Buffer.concat(kept)
  const cbSize = formatSize >= 18 ? fmt.readUInt16LE(16) : 0
  const samplesPerBlock = { 6: 1, 7: 1, 0x31: 320 }[tag] ?? (cbSize >= 2 ? fmt.readUInt16LE(18) : 0)
  const pcmFamily = [1, 3, 0xfffe].includes(tag)
  const fact = !pcmFamily && samplesPerBlock > 0 ? Buffer.alloc(12) : Buffer.alloc(0)
  if (fact.length) {
    fact.write('fact', 0, 'ascii')
    fact.writeUInt32LE(4, 4)
    fact.writeUInt32LE((data.length / align) * samplesPerBlock, 8)
  }
  const fmtChunk = Buffer.alloc(8 + formatSize + (formatSize % 2))
  fmtChunk.write('fmt ', 0, 'ascii')
  fmtChunk.writeUInt32LE(formatSize, 4)
  fmt.copy(fmtChunk, 8)
  const dataHeader = Buffer.alloc(8)
  dataHeader.write('data', 0, 'ascii')
  dataHeader.writeUInt32LE(data.length, 4)
  const body = [fmtChunk, fact, dataHeader, data, Buffer.alloc(data.length % 2)]
  const riff = Buffer.alloc(12)
  riff.write('RIFF', 0, 'ascii')
  riff.writeUInt32LE(4 + body.reduce((total, part) => total + part.length, 0), 4)
  riff.write('WAVE', 8, 'ascii')
  return Buffer.concat([riff, ...body])
}

// The PCM a fixture's data must decode to: every block but the erased one.
export function dmExpectedPcm(fmt, data) {
  const blockSize = dmBlockSize(fmt)
  return Buffer.concat([
    data.subarray(0, DM_ERASED_BLOCK * blockSize),
    data.subarray((DM_ERASED_BLOCK + 1) * blockSize),
  ])
}

async function generateDm(ffmpeg) {
  const tone = path.join(FIXTURE_DIRECTORY, 'dm-source-tone.wav')
  try {
    await writeFile(tone, buildToneWav(DM_SAMPLE_RATE), { flag: 'wx', mode: 0o600 })
    for (const entry of DM_FIXTURES) {
      const fixture = path.join(FIXTURE_DIRECTORY, entry.fixture)
      if (await stat(fixture).catch(() => undefined))
        throw new Error(`Refusing to replace ${entry.fixture}; run --clean first`)
      let wav = buildToneWav(DM_SAMPLE_RATE)
      if (entry.encoder) {
        const encoded = path.join(FIXTURE_DIRECTORY, `${path.parse(entry.fixture).name}-source.wav`)
        try {
          run(
            ffmpeg,
            ['-hide_banner', '-v', 'error', '-xerror', '-i', tone, '-c:a', entry.encoder, '-f', 'wav', '-y', encoded],
            `${entry.label} source encode`,
          )
          wav = await readFile(encoded)
        } finally {
          await rm(encoded, { force: true })
        }
      }
      const { fmt, data } = wavChunks(wav)
      await writeFile(fixture, buildDm(fmt, data), { flag: 'wx', mode: 0o600 })
      console.log(`Generated ${entry.label} fixture ${entry.fixture}`)
    }
  } finally {
    await rm(tone, { force: true })
  }
}

async function checkDm(ffmpeg, ffprobe, tag) {
  const decoders = run(ffmpeg, ['-hide_banner', '-decoders'], 'decoder list')
  for (const decoder of DM_DECODERS) {
    if (!new RegExp(`\\s${decoder}\\s`, 'u').test(decoders)) {
      throw new Error(`${path.basename(ffmpeg)} lacks the ${decoder} decoder that .dm audio can need`)
    }
  }
  for (const entry of DM_FIXTURES) {
    const fixture = path.join(FIXTURE_DIRECTORY, entry.fixture)
    await requireFile(fixture, `fixture for ${entry.label}`)
    const feed = path.join(FIXTURE_DIRECTORY, `${tag}-${path.parse(entry.fixture).name}-feed.wav`)
    try {
      await writeFile(feed, demuxDm(await readFile(fixture)), { flag: 'wx', mode: 0o600 })
      const found = streams(ffprobe, feed, entry.label)
      if (found.length !== 1 || found[0].codec_name !== entry.codec) {
        throw new Error(`${entry.label}: expected one ${entry.codec} stream, found ${JSON.stringify(found)}`)
      }
      run(
        ffmpeg,
        ['-hide_banner', '-v', 'error', '-xerror', '-i', feed, '-map', '0:a:0', '-f', 'null', '-'],
        `${entry.label} audio decode`,
      )
      if (!entry.encoder) {
        const decoded = spawnSync(
          ffmpeg,
          ['-hide_banner', '-v', 'error', '-xerror', '-i', feed, '-map', '0:a:0', '-f', 's16le', '-'],
          { maxBuffer: 16 * 1024 * 1024, timeout: 120_000, windowsHide: true },
        )
        const { fmt, data } = wavChunks(buildToneWav(DM_SAMPLE_RATE))
        if (decoded.status !== 0 || !Buffer.from(decoded.stdout).equals(dmExpectedPcm(fmt, data))) {
          throw new Error(`${entry.label}: decoded samples are not the source minus the recorded-over block`)
        }
      }
      for (const output of APP_OUTPUTS) {
        const target = path.join(
          FIXTURE_DIRECTORY,
          `${tag}-${path.parse(entry.fixture).name}${output.variant ? `-${output.variant}` : ''}.${output.extension}`,
        )
        try {
          run(
            ffmpeg,
            ['-hide_banner', '-v', 'error', '-xerror', '-i', feed, '-map', '0:a:0', ...output.args, '-y', target],
            `${entry.label} to ${output.variant ?? output.extension}`,
          )
          const written = streams(ffprobe, target, `${entry.label} ${output.extension} output`)
          if (written.length !== 1 || written[0].codec_type !== 'audio') {
            throw new Error(`${entry.label}: ${output.extension} output must hold one audio stream`)
          }
        } finally {
          await rm(target, { force: true })
        }
      }
    } finally {
      await rm(feed, { force: true })
    }
    console.log(`${path.basename(ffmpeg)}: ${entry.label} decodes`)
  }
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
  await generateDm(ffmpeg)
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
  await checkDm(ffmpeg, ffprobe, tag)
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

  // .dm: build a PCM fixture, read it back, and check the layout and that the
  // recorded-over block is gone; a short last block keeps whole codec blocks.
  const pcm = wavChunks(buildToneWav(DM_SAMPLE_RATE))
  assert.equal(dmBlockSize(pcm.fmt), 5512)
  const dm = buildDm(pcm.fmt, pcm.data)
  assert.equal(dm.readUInt32LE(4), 8)
  assert.equal(dm.readUInt32LE(12), 5512)
  assert.equal(dm.readUInt32LE(16 + pcm.fmt.length + 8 + 5512), 0, 'block 1 is marked recorded over')
  const fed = wavChunks(demuxDm(dm))
  assert.ok(fed.fmt.equals(pcm.fmt))
  assert.ok(fed.data.equals(dmExpectedPcm(pcm.fmt, pcm.data)))
  assert.equal(fed.data.length, pcm.data.length - 5512)
  const gsmFmt = Buffer.from('31000100112b0000bf08000041000000020040 01'.replace(/ /gu, ''), 'hex')
  assert.equal(dmBlockSize(gsmFmt), 520)
  const gsm = wavChunks(demuxDm(buildDm(gsmFmt, Buffer.alloc(520 * 3 + 100, 1))))
  assert.equal(gsm.data.length, 520 * 2 + 65, 'erased block dropped, 35 stray bytes dropped')
  assert.throws(() => demuxDm(Buffer.from('RIFF0000WAVEfmt ')), /not a \.dm audio header/u)
  assert.equal(new Set(DM_FIXTURES.map(entry => entry.fixture)).size, DM_FIXTURES.length)
  const ffmpegBuild = await readFile(path.join(repository, 'scripts/build-ffmpeg-macos.sh'), 'utf8')
  for (const decoder of DM_DECODERS) {
    assert.match(ffmpegBuild, new RegExp(`for decoder in [^\\n]*\\b${decoder}\\b`, 'u'), `macOS gate lists ${decoder}`)
  }
  const dmSource = await readFile(path.join(repository, 'src-tauri/src/dm.rs'), 'utf8')
  assert.ok(dmSource.includes('const EXPECTED_HEADER_SIZE: u32 = 8;'), 'dm.rs still reads header size 8')

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
