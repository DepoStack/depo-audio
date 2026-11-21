import { describe, expect, it } from 'vitest'
import { RIFF_LIMIT_BYTES, estimateWavOutput, wavSizeEstimateText } from '../lib/outputSize'

const probe = (channels, durationSecs) => ({ convertedIndex: 0, streams: [{ channels }], durationSecs })

describe('estimateWavOutput', () => {
  it('is duration x channels x sample rate x 2 bytes for each mode', () => {
    // 60 s at 48 kHz, 16-bit: 5,760,000 bytes per channel.
    expect(estimateWavOutput(probe(4, 60), { mode: 'keep', rate: '48000' })).toEqual({
      perFileBytes: 23_040_000,
      totalBytes: 23_040_000,
      files: 1,
    })
    expect(estimateWavOutput(probe(4, 60), { mode: 'stereo', rate: '48000' })).toEqual({
      perFileBytes: 11_520_000,
      totalBytes: 11_520_000,
      files: 1,
    })
    expect(estimateWavOutput(probe(4, 60), { mode: 'split', rate: '48000' })).toEqual({
      perFileBytes: 5_760_000,
      totalBytes: 23_040_000,
      files: 4,
    })
  })

  it('uses the converted stream, not the widest one', () => {
    const twoTracks = { convertedIndex: 0, streams: [{ channels: 1 }, { channels: 16 }], durationSecs: 10 }
    expect(estimateWavOutput(twoTracks, { mode: 'keep', rate: '48000' }).perFileBytes).toBe(960_000)
  })

  it('gives no estimate without a duration or channel count', () => {
    expect(estimateWavOutput(undefined, { mode: 'keep', rate: '48000' })).toBeNull()
    expect(estimateWavOutput(probe(2, null), { mode: 'keep', rate: '48000' })).toBeNull()
    expect(estimateWavOutput(probe(null, 60), { mode: 'keep', rate: '48000' })).toBeNull()
  })
})

describe('wavSizeEstimateText', () => {
  const files = [{ path: '/a.wav' }, { path: '/b.mkv' }]

  it('labels the figure as an estimate and says how it is worked out', () => {
    const text = wavSizeEstimateText(
      files,
      { '/a.wav': probe(2, 60), '/b.mkv': probe(2, 60) },
      {
        mode: 'keep',
        rate: '48000',
      },
    )
    expect(text).toBe(
      'Estimated WAV size: about 22.0 MB for these 2 files (duration × channels × sample rate × 2 bytes).',
    )
  })

  it('says when some files could not be measured, and says nothing when none could', () => {
    expect(wavSizeEstimateText(files, { '/a.wav': probe(2, 60) }, { mode: 'keep', rate: '48000' })).toContain(
      'for 1 of 2 files; the others could not be measured',
    )
    expect(wavSizeEstimateText(files, {}, { mode: 'keep', rate: '48000' })).toBe('')
  })

  it('mentions RF64 only when an output passes 4 GiB', () => {
    // 16 channels at 48 kHz pass 4 GiB after about 46.6 minutes.
    const secondsToLimit = RIFF_LIMIT_BYTES / (16 * 48000 * 2)
    expect(secondsToLimit / 60).toBeCloseTo(46.6, 1)
    const long = { '/a.wav': probe(16, secondsToLimit + 60) }
    expect(wavSizeEstimateText([files[0]], long, { mode: 'keep', rate: '48000' })).toContain('RF64 header')
    // The same recording split into 16 mono files stays under the limit per file.
    expect(wavSizeEstimateText([files[0]], long, { mode: 'split', rate: '48000' })).not.toContain('RF64')
  })
})
