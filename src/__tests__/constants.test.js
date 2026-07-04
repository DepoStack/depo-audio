import { describe, it, expect } from 'vitest'
import { MODES, FORMATS_OUT, CH_COLORS, FORMAT_ROWS, STANDARD_EXTS } from '../constants'

describe('MODES', () => {
  it('has three modes with required fields', () => {
    expect(MODES).toHaveLength(3)
    MODES.forEach(m => {
      expect(m).toHaveProperty('id')
      expect(m).toHaveProperty('label')
      expect(m).toHaveProperty('desc')
    })
  })

  it('includes stereo, keep, split', () => {
    const ids = MODES.map(m => m.id)
    expect(ids).toEqual(['stereo', 'keep', 'split'])
  })
})

describe('FORMATS_OUT', () => {
  it('has five output formats', () => {
    expect(FORMATS_OUT).toHaveLength(5)
  })

  it('includes wav, mp3, flac, opus, m4a', () => {
    const ids = FORMATS_OUT.map(f => f.id)
    expect(ids).toEqual(['wav', 'mp3', 'flac', 'opus', 'm4a'])
  })
})

describe('CH_COLORS', () => {
  it('has 4 channel colors', () => {
    expect(CH_COLORS).toHaveLength(4)
    CH_COLORS.forEach(c => expect(c).toMatch(/^#[0-9a-f]{6}$/i))
  })
})

describe('FORMAT_ROWS', () => {
  it('has standard and court format groups', () => {
    const groups = [...new Set(FORMAT_ROWS.map(r => r.group))]
    expect(groups).toContain('standard')
    expect(groups).toContain('court')
  })

  it('all rows have required fields', () => {
    FORMAT_ROWS.forEach(r => {
      expect(r).toHaveProperty('ext')
      expect(r).toHaveProperty('vendor')
      expect(r).toHaveProperty('status')
      expect(r).toHaveProperty('group')
    })
  })
})

describe('STANDARD_EXTS', () => {
  it('includes common audio extensions', () => {
    expect(STANDARD_EXTS.has('wav')).toBe(true)
    expect(STANDARD_EXTS.has('mp3')).toBe(true)
    expect(STANDARD_EXTS.has('flac')).toBe(true)
  })

  it('does not include court formats', () => {
    expect(STANDARD_EXTS.has('sgmca')).toBe(false)
    expect(STANDARD_EXTS.has('trm')).toBe(false)
  })
})
