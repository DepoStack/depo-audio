import { describe, it, expect } from 'vitest'
import { fmtSize, fmtTime, basename, sortRecordingChunks } from '../utils'

describe('fmtSize', () => {
  it('returns dash for zero/null', () => {
    expect(fmtSize(0)).toBe('—')
    expect(fmtSize(null)).toBe('—')
    expect(fmtSize(undefined)).toBe('—')
  })

  it('formats KB', () => {
    expect(fmtSize(512 * 1024)).toBe('512.0 KB')
  })

  it('formats MB', () => {
    expect(fmtSize(150 * 1024 * 1024)).toBe('150.0 MB')
  })

  it('formats GB', () => {
    expect(fmtSize(2 * 1024 * 1024 * 1024)).toBe('2.00 GB')
  })
})

describe('fmtTime', () => {
  it('returns 0:00 for falsy values', () => {
    expect(fmtTime(0)).toBe('0:00')
    expect(fmtTime(null)).toBe('0:00')
    expect(fmtTime(NaN)).toBe('0:00')
  })

  it('formats seconds', () => {
    expect(fmtTime(5)).toBe('0:05')
    expect(fmtTime(65)).toBe('1:05')
    expect(fmtTime(3661)).toBe('61:01')
  })
})

describe('basename', () => {
  it('extracts filename from Unix path', () => {
    expect(basename('/Users/foo/bar.wav')).toBe('bar.wav')
  })

  it('extracts filename from Windows path', () => {
    expect(basename('C:\\Users\\foo\\bar.wav')).toBe('bar.wav')
  })

  it('handles empty/null', () => {
    expect(basename('')).toBe('')
    expect(basename(null)).toBe('')
  })
})

describe('sortRecordingChunks', () => {
  // Real FTR Gold chunk names from a court-produced session
  // ({Location}_{YYYYMMDD}-{HHMM}_{16-hex FILETIME}.trm)
  const chrono = [
    '/case/CR24_20180621-1449_01d4096f0757ee50.trm',
    '/case/CR24_20180621-1454_01d4096fbaa19b00.trm',
    '/case/CR24_20180621-1459_01d409706d824cb0.trm',
  ]

  it('orders FTR chunks chronologically regardless of arrival order', () => {
    const shuffled = [chrono[2], chrono[0], chrono[1]]
    expect(sortRecordingChunks(shuffled)).toEqual(chrono)
  })

  it('does not mutate the input array', () => {
    const shuffled = [chrono[1], chrono[0]]
    const before = [...shuffled]
    sortRecordingChunks(shuffled)
    expect(shuffled).toEqual(before)
  })

  it('leaves mixed batches in user order', () => {
    const mixed = [chrono[2], '/case/deposition.wav', chrono[0]]
    expect(sortRecordingChunks(mixed)).toEqual(mixed)
  })

  it('groups chunks by recording location instead of interleaving courtrooms', () => {
    const twoRooms = [
      '/x/CR9_20180621-1454_01d4096fbaa19b99.trm',
      '/x/CR24_20180621-1459_01d409706d824cb0.trm',
      '/x/CR9_20180621-1449_01d4096f0757ee99.trm',
      '/x/CR24_20180621-1449_01d4096f0757ee50.trm',
    ]
    const sorted = sortRecordingChunks(twoRooms).map(basename)
    // Each room's chunks stay contiguous and internally chronological
    const rooms = sorted.map(n => n.split('_')[0])
    expect(rooms.indexOf('CR24')).toBe(rooms.lastIndexOf('CR24') - 1)
    expect(rooms.indexOf('CR9')).toBe(rooms.lastIndexOf('CR9') - 1)
    const cr9 = sorted.filter(n => n.startsWith('CR9'))
    expect(cr9).toEqual(['CR9_20180621-1449_01d4096f0757ee99.trm', 'CR9_20180621-1454_01d4096fbaa19b99.trm'])
    const cr24 = sorted.filter(n => n.startsWith('CR24'))
    expect(cr24).toEqual(['CR24_20180621-1449_01d4096f0757ee50.trm', 'CR24_20180621-1459_01d409706d824cb0.trm'])
  })

  it('orders by FILETIME hex when name order misleads, tolerating underscores in the location', () => {
    // 14:50:00 (HHMMSS variant) starts BEFORE 14:51, but numeric name
    // collation ranks 1451 < 145000 — only the hex FILETIME orders these
    // correctly. The location itself contains an underscore.
    const misleading = [
      '/case/Dept 3_Annex_20180621-1451_01d4096f44bf6200.trm',
      '/case/Dept 3_Annex_20180621-145000_01d4096f20fc1c00.trm',
    ]
    expect(sortRecordingChunks(misleading).map(basename)).toEqual([
      'Dept 3_Annex_20180621-145000_01d4096f20fc1c00.trm',
      'Dept 3_Annex_20180621-1451_01d4096f44bf6200.trm',
    ])
  })

  it('pads mixed-width time fields in the natural-sort fallback (no hex)', () => {
    const noHex = [
      '/case/Room 2_20180621-1451.trm',
      '/case/Room 2_20180621-145000.trm',
    ]
    expect(sortRecordingChunks(noHex).map(basename)).toEqual([
      'Room 2_20180621-145000.trm',
      'Room 2_20180621-1451.trm',
    ])
  })

  it('falls back to natural sort for non-FTR-named .trm files', () => {
    const numbered = ['/y/session part 10.trm', '/y/session part 2.trm', '/y/session part 1.trm']
    expect(sortRecordingChunks(numbered).map(basename)).toEqual([
      'session part 1.trm', 'session part 2.trm', 'session part 10.trm',
    ])
  })

  it('supports an accessor for object arrays (Merge sources)', () => {
    const sources = [
      { path: chrono[1], name: 'b' },
      { path: chrono[0], name: 'a' },
    ]
    expect(sortRecordingChunks(sources, s => s.path).map(s => s.path)).toEqual([chrono[0], chrono[1]])
  })

  it('passes through empty and single-item batches', () => {
    expect(sortRecordingChunks([])).toEqual([])
    expect(sortRecordingChunks([chrono[0]])).toEqual([chrono[0]])
  })
})
