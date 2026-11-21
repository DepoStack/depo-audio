import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { SCAN_SAMPLE_NOTE, SCAN_SAMPLE_SECS, aggregateAnalysisResults, scanSampleSpan } from '../lib/analysis'

describe('aggregateAnalysisResults', () => {
  it('uses worst quality and aggregates batch-wide findings', () => {
    const result = aggregateAnalysisResults([
      {
        qualityScore: { ovr: 4.2, sig: 4.1, bak: 4.3 },
        speakerCount: 2,
        speechRatio: 0.8,
        turns: [{ start: 1 }],
        recommendations: ['normalize'],
        sampleRate: 48000,
      },
      {
        qualityScore: { ovr: 2.1, sig: 2.2, bak: 2.0 },
        speakerCount: 4,
        speechRatio: 0.4,
        turns: [{ start: 2 }, { start: 3 }],
        recommendations: ['normalize', 'denoise'],
        needsDenoise: true,
        isNarrowband: true,
        sampleRate: 16000,
      },
    ])

    expect(result.qualityScore.ovr).toBe(2.1)
    expect(result.speakerCount).toBe(4)
    expect(result.speechRatio).toBeCloseTo(0.6)
    expect(result.turns).toHaveLength(3)
    expect(result.recommendations).toEqual(['normalize', 'denoise'])
    expect(result.needsDenoise).toBe(true)
    expect(result.sampleRate).toBe(16000)
  })

  it('does not invent values when optional models returned no metrics', () => {
    expect(aggregateAnalysisResults([{}])).toMatchObject({
      qualityScore: null,
      speakerCount: null,
      speechRatio: null,
      turns: [],
    })
  })
})

describe('Scan sample copy', () => {
  it('matches the backend sample length in src-tauri/src/ffmpeg.rs', () => {
    const rust = readFileSync('src-tauri/src/ffmpeg.rs', 'utf8')
    const match = rust.match(/const ANALYSIS_SAMPLE_SECS: u32 = (\d+);/)
    expect(match).not.toBeNull()
    expect(SCAN_SAMPLE_SECS).toBe(Number(match[1]))
  })

  it('says how much of each file Scan checks', () => {
    expect(SCAN_SAMPLE_NOTE).toBe('Scan checks the first 3 minutes of each file.')
    expect(scanSampleSpan(60)).toBe('the first minute')
    expect(scanSampleSpan(90)).toBe('the first 90 seconds')
  })
})
