import { describe, it, expect } from 'vitest'
import {
  AUDIO_EXTS,
  SPEED_STEPS,
  loadSpeed,
  cycleSpeedStep,
  loadBookmarks,
  freshAudioPaths,
  partitionPlaybackPaths,
  bookmarksToText,
  BACKSTEP_STEPS,
  DEFAULT_BACKSTEP,
  loadBackstep,
  backstepLabel,
  resumePosition,
  compareTarget,
  COMPARE_UNAVAILABLE_REASON,
  COMPARE_TRIMMED_NOTE,
} from '../lib/player'

// Characterization tests for the Player tab's pure logic: accepted files,
// speed cycling, and persisted bookmark handling.

describe('loadSpeed', () => {
  it('accepts persisted speeds on the menu', () => {
    expect(loadSpeed('1.5')).toBe(1.5)
    expect(loadSpeed('0.5')).toBe(0.5)
  })
  it('falls back to 1× for anything off the menu', () => {
    expect(loadSpeed('1.3')).toBe(1)
    expect(loadSpeed(null)).toBe(1)
    expect(loadSpeed('garbage')).toBe(1)
  })
})

describe('cycleSpeedStep', () => {
  it('steps along the menu', () => {
    expect(cycleSpeedStep(1, +1)).toBe(1.25)
    expect(cycleSpeedStep(1, -1)).toBe(0.75)
  })
  it('clamps at both ends', () => {
    expect(cycleSpeedStep(2, +1)).toBe(2)
    expect(cycleSpeedStep(0.5, -1)).toBe(0.5)
  })
  it('treats an unknown speed as 1× before stepping', () => {
    expect(cycleSpeedStep(1.33, +1)).toBe(SPEED_STEPS[3]) // 1 → 1.25
  })
})

describe('loadBookmarks', () => {
  it('keeps only well-shaped entries', () => {
    const raw = JSON.stringify([
      { time: 5, trackPath: '/a.wav', label: 'x' },
      { time: 'bad', trackPath: '/a.wav' },
      { time: 6 },
      null,
      'junk',
    ])
    expect(loadBookmarks(raw)).toEqual([{ time: 5, trackPath: '/a.wav', label: 'x' }])
  })
  it('tolerates corrupt or missing storage', () => {
    expect(loadBookmarks(null)).toEqual([])
    expect(loadBookmarks('not json')).toEqual([])
    expect(loadBookmarks('{"a":1}')).toEqual([])
  })
})

describe('freshAudioPaths', () => {
  it('keeps directly playable audio and routes native containers through conversion', () => {
    const out = freshAudioPaths(['/a.wav', '/b.pdf', '/c.trm', '/d.WAV', '/e.sgmca'], [])
    expect(out).toEqual(['/a.wav', '/d.WAV'])
    expect(partitionPlaybackPaths(['/a.wav', '/c.trm', '/d.ftr', '/e.sgmca', '/f.DM'], []).conversionRequired).toEqual([
      '/c.trm',
      '/d.ftr',
      '/e.sgmca',
      '/f.DM',
    ])
  })
  it('skips paths already queued and dedupes the drop', () => {
    const tracks = [{ path: '/a.wav' }]
    expect(freshAudioPaths(['/a.wav', '/b.mp3', '/b.mp3'], tracks)).toEqual(['/b.mp3'])
  })
  it('unwraps { path } objects from the dialog plugin', () => {
    expect(freshAudioPaths([{ path: '/a.flac' }], [])).toEqual(['/a.flac'])
  })
})

describe('bookmarksToText', () => {
  it('exports the active track sorted by time as MM:SS<TAB>label', () => {
    const bms = [
      { time: 65, trackPath: '/a.wav', label: 'two' },
      { time: 5, trackPath: '/a.wav', label: 'one' },
      { time: 1, trackPath: '/other.wav', label: 'skip' },
    ]
    expect(bookmarksToText(bms, '/a.wav')).toBe('0:05\tone\n1:05\ttwo')
  })
  it('trims the tab when a bookmark has no label', () => {
    expect(bookmarksToText([{ time: 5, trackPath: '/a.wav' }], '/a.wav')).toBe('0:05')
  })
})

describe('constants', () => {
  it('locks the accepted extension list', () => {
    expect(AUDIO_EXTS).toEqual([
      'wav',
      'mp3',
      'flac',
      'opus',
      'ogg',
      'm4a',
      'aac',
      'wma',
      'aif',
      'aiff',
      'sgmca',
      'trm',
      'ftr',
      'tll',
      'bwf',
      'dm',
    ])
  })
  it('locks the speed menu', () => {
    expect(SPEED_STEPS).toEqual([0.5, 0.75, 1, 1.25, 1.5, 2])
  })
})

describe('auto-backstep', () => {
  it('offers Off, 1, 2, 3 and 5 seconds and defaults to 2', () => {
    expect(BACKSTEP_STEPS).toEqual([0, 1, 2, 3, 5])
    expect(DEFAULT_BACKSTEP).toBe(0)
    expect(BACKSTEP_STEPS.map(backstepLabel)).toEqual(['Off', '1s', '2s', '3s', '5s'])
  })

  it('loads persisted values on the menu and falls back to 2 for anything else', () => {
    expect(loadBackstep('0')).toBe(0)
    expect(loadBackstep('5')).toBe(5)
    expect(loadBackstep(null)).toBe(0)
    expect(loadBackstep('')).toBe(0)
    expect(loadBackstep('4')).toBe(0)
    expect(loadBackstep('-1')).toBe(0)
    expect(loadBackstep('nonsense')).toBe(0)
  })

  it('rewinds when resuming at the spot where playback was paused', () => {
    expect(resumePosition({ currentTime: 42, pausedAt: 42, backstep: 2 })).toBe(40)
    expect(resumePosition({ currentTime: 42, pausedAt: 42, backstep: 5 })).toBe(37)
  })

  it('clamps the rewind at the start of the track', () => {
    expect(resumePosition({ currentTime: 1.2, pausedAt: 1.2, backstep: 3 })).toBe(0)
  })

  it('does not rewind when Off', () => {
    expect(resumePosition({ currentTime: 42, pausedAt: 42, backstep: 0 })).toBe(42)
  })

  it('does not rewind after a seek, track change or loop wrap (no pause point)', () => {
    expect(resumePosition({ currentTime: 42, pausedAt: null, backstep: 2 })).toBe(42)
  })

  it('does not rewind when the position moved since the pause', () => {
    expect(resumePosition({ currentTime: 90, pausedAt: 42, backstep: 2 })).toBe(90)
  })

  it('stays inside an active A-B loop', () => {
    expect(resumePosition({ currentTime: 31, pausedAt: 31, backstep: 3, loopStart: 30 })).toBe(30)
    expect(resumePosition({ currentTime: 20, pausedAt: 20, backstep: 3, loopStart: 30 })).toBe(17)
  })
})

describe('compareTarget', () => {
  const mp3 = { path: '/out/a.mp3', name: 'a.mp3' }
  it('enables compare for a playable source and output', () => {
    expect(compareTarget('/in/a.wav', [mp3])).toEqual({
      enabled: true,
      output: mp3,
      reason: '',
      note: '',
      aligned: true,
    })
  })
  it('disables compare for court containers with a plain reason', () => {
    for (const ext of ['sgmca', 'trm', 'ftr', 'tll', 'dm']) {
      const target = compareTarget(`/in/a.${ext}`, [mp3])
      expect(target.enabled).toBe(false)
      expect(target.reason).toBe(COMPARE_UNAVAILABLE_REASON)
    }
  })
  it('names the first output for split conversions', () => {
    const ch2 = { path: '/out/a_ch2.wav', name: 'a_ch2.wav' }
    const target = compareTarget('/in/a.wav', [mp3, ch2])
    expect(target.output).toBe(mp3)
    expect(target.note).toMatch(/^Comparing a\.mp3 with the original/)
  })
  it('says positions differ after leading-silence trim', () => {
    const target = compareTarget('/in/a.wav', [mp3], { trimmed: true })
    expect(target).toMatchObject({ enabled: true, aligned: false, note: COMPARE_TRIMMED_NOTE })
    expect(compareTarget('/in/a.wav', [mp3], { trimmed: false }).aligned).toBe(true)
  })
})
