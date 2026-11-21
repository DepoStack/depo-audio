import { describe, it, expect } from 'vitest'
import { MODES, FORMATS_OUT, CH_COLORS, FORMAT_ROWS, STANDARD_EXTS, effectiveOutputRate } from '../constants'

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

  it('describes split output as channels without inferring speakers', () => {
    expect(MODES.find(mode => mode.id === 'split')).toMatchObject({
      label: 'Split Channels',
      desc: 'One file per channel, named by role',
    })
    expect(MODES.map(mode => `${mode.label} ${mode.desc}`).join(' ')).not.toMatch(/speaker/i)
  })
})

describe('FORMATS_OUT', () => {
  it('has six output formats', () => {
    expect(FORMATS_OUT).toHaveLength(6)
  })

  it('includes wav, mp3, flac, opus, m4a, sgmca', () => {
    const ids = FORMATS_OUT.map(f => f.id)
    expect(ids).toEqual(['wav', 'mp3', 'flac', 'opus', 'm4a', 'sgmca'])
  })

  it('fixes the rate only for formats that set their own', () => {
    expect(effectiveOutputRate('opus', '44100')).toBe('48000')
    expect(effectiveOutputRate('sgmca', '44100')).toBe('16000')
    expect(effectiveOutputRate('wav', '44100')).toBe('44100')
  })
})

describe('CH_COLORS', () => {
  it('has 4 channel colors, each a --speaker-N token', () => {
    expect(CH_COLORS).toEqual([1, 2, 3, 4].map(n => `hsl(var(--speaker-${n}))`))
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

describe('FORMAT_ROWS intake rows', () => {
  it('lists the legacy deposition video containers as audio-extracted standard rows', () => {
    const rows = ['.mpg  .mpeg  .vob', '.ts  .mts  .m2ts', '.wmv  .asf'].map(ext =>
      FORMAT_ROWS.find(r => r.ext === ext),
    )
    for (const row of rows) {
      expect(row).toMatchObject({ status: 'supported', group: 'standard' })
      expect(row.vendor).toContain('audio track extracted')
    }
  })

  it('keeps every closed format in the export-first group', () => {
    const closed = FORMAT_ROWS.filter(r => r.status === 'unsupported')
    expect(closed.map(r => r.ext)).toEqual(['.aes', '.dcr', '.ds2', '.arf  .wrf', '.msv  .dvf'])
    expect(closed.every(r => r.group === 'export')).toBe(true)
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
