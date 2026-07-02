import { describe, it, expect } from 'vitest'
import {
  AUDIO_EXTS, SPEED_STEPS, loadSpeed, cycleSpeedStep,
  loadBookmarks, freshAudioPaths, bookmarksToText,
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
  it('keeps only audio extensions (court formats included)', () => {
    const out = freshAudioPaths(['/a.wav', '/b.pdf', '/c.trm', '/d.WAV'], [])
    expect(out).toEqual(['/a.wav', '/c.trm', '/d.WAV'])
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
    expect(AUDIO_EXTS).toEqual(['wav','mp3','flac','opus','ogg','m4a','aac','wma','aif','aiff','sgmca','trm','ftr','bwf'])
  })
  it('locks the speed menu', () => {
    expect(SPEED_STEPS).toEqual([0.5, 0.75, 1, 1.25, 1.5, 2])
  })
})
