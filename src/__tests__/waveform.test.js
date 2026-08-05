import { describe, expect, it } from 'vitest'
import {
  MAX_WAVEFORM_SECONDS,
  clampSeekTime,
  seekTimeFromKey,
  seekTimeFromPointer,
  waveformDecodeGuard,
} from '../lib/waveform'

describe('waveformDecodeGuard', () => {
  it('fails closed for long media while leaving playback as the stated fallback', () => {
    expect(waveformDecodeGuard({ duration: MAX_WAVEFORM_SECONDS + 1 })).toContain('Playback is still available')
    expect(waveformDecodeGuard({ duration: MAX_WAVEFORM_SECONDS })).toBe('')
  })
})

describe('waveform seek math', () => {
  it('uses the seek surface bounds and clamps pointer positions', () => {
    expect(seekTimeFromPointer({ clientX: 150, left: 100, width: 200, duration: 100 })).toBe(25)
    expect(seekTimeFromPointer({ clientX: 50, left: 100, width: 200, duration: 100 })).toBe(0)
    expect(seekTimeFromPointer({ clientX: 350, left: 100, width: 200, duration: 100 })).toBe(100)
    expect(seekTimeFromPointer({ clientX: 150, left: 100, width: 0, duration: 100 })).toBeNull()
  })

  it('implements bounded slider keyboard semantics', () => {
    expect(seekTimeFromKey({ key: 'ArrowRight', currentTime: 98, duration: 100 })).toBe(100)
    expect(seekTimeFromKey({ key: 'ArrowLeft', currentTime: 2, duration: 100 })).toBe(0)
    expect(seekTimeFromKey({ key: 'Home', currentTime: 40, duration: 100 })).toBe(0)
    expect(seekTimeFromKey({ key: 'End', currentTime: 40, duration: 100 })).toBe(100)
    expect(seekTimeFromKey({ key: 'Enter', currentTime: 40, duration: 100 })).toBeNull()
    expect(clampSeekTime(Number.NaN, 100)).toBeNull()
  })
})
