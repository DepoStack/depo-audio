import { describe, it, expect } from 'vitest'
import { fmtSize, fmtTime, basename } from '../utils'

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
