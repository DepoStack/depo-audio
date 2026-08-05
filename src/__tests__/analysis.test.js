import { describe, expect, it } from 'vitest'
import { aggregateAnalysisResults } from '../lib/analysis'

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
