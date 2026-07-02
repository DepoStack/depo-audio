import { describe, it, expect } from 'vitest'
import {
  parseTime, splitSpeaker, parseCues, parsePlain, parseTranscript,
  srtStamp, toPlainText, toSRT, storageKey, loadSegments,
} from '../lib/transcript'

// Characterization tests: these pin the transcript formats the editor reads
// and writes. Users' saved transcripts depend on this behavior staying put.

describe('parseTime', () => {
  it('parses SRT timestamps (comma millis)', () => {
    expect(parseTime('01:02:03,500')).toBeCloseTo(3723.5)
  })
  it('parses VTT timestamps (dot millis)', () => {
    expect(parseTime('00:00:01.250')).toBeCloseTo(1.25)
  })
  it('parses MM:SS', () => {
    expect(parseTime('02:15')).toBe(135)
  })
  it('parses bare seconds', () => {
    expect(parseTime('90')).toBe(90)
  })
  it('returns null for non-numeric input', () => {
    expect(parseTime('abc')).toBeNull()
    expect(parseTime('1:xx')).toBeNull()
  })
})

describe('splitSpeaker', () => {
  it('splits an uppercase-led speaker prefix', () => {
    expect(splitSpeaker('MR. SMITH: Objection.')).toEqual({ speaker: 'MR. SMITH', text: 'Objection.' })
  })
  it('splits a mixed-case name after a capital', () => {
    expect(splitSpeaker('The Court: Overruled.')).toEqual({ speaker: 'The Court', text: 'Overruled.' })
  })
  it('does not treat lowercase-led text as a speaker', () => {
    expect(splitSpeaker('the witness said: hello')).toEqual({ speaker: '', text: 'the witness said: hello' })
  })
  it('leaves plain lines untouched (trimmed)', () => {
    expect(splitSpeaker('  just text  ')).toEqual({ speaker: '', text: 'just text' })
  })
})

const SRT = `1
00:00:01,000 --> 00:00:03,000
MR. SMITH: Good morning.

2
00:00:03,000 --> 00:00:05,500
Second line
continues here
`

describe('parseCues / parseTranscript', () => {
  it('parses SRT cues with start times and speakers', () => {
    const segs = parseCues(SRT)
    expect(segs).toHaveLength(2)
    expect(segs[0]).toMatchObject({ start: 1, speaker: 'MR. SMITH', text: 'Good morning.' })
    expect(segs[1].start).toBe(3)
    // Multi-line cue bodies are joined with a space
    expect(segs[1].text).toBe('Second line continues here')
  })
  it('routes by extension and by --> sniffing', () => {
    expect(parseTranscript(SRT, 'srt')[0].start).toBe(1)
    expect(parseTranscript(SRT, 'txt')[0].start).toBe(1) // sniffed via -->
    expect(parseTranscript('just some text', 'txt')[0].start).toBeNull()
  })
  it('parses plain text one segment per non-empty line', () => {
    const segs = parsePlain('A: one\n\n  \nB: two\n')
    expect(segs).toHaveLength(2)
    expect(segs[0]).toMatchObject({ start: null, speaker: 'A', text: 'one' })
  })
  it('handles CRLF input', () => {
    expect(parsePlain('one\r\ntwo\r\n')).toHaveLength(2)
  })
})

describe('srtStamp / toSRT', () => {
  it('formats HH:MM:SS,mmm', () => {
    expect(srtStamp(3723.5)).toBe('01:02:03,500')
    expect(srtStamp(0)).toBe('00:00:00,000')
  })
  it('sorts out-of-order stamps so cues never overlap backwards', () => {
    const segs = [
      { id: 'b', start: 10, speaker: '', text: 'later' },
      { id: 'a', start: 2, speaker: 'A', text: 'earlier' },
    ]
    const srt = toSRT(segs)
    const lines = srt.split('\n')
    expect(lines[0]).toBe('1')
    expect(lines[1]).toBe('00:00:02,000 --> 00:00:10,000')
    expect(lines[2]).toBe('A: earlier')
  })
  it('ends the last cue 3 seconds after its start', () => {
    const srt = toSRT([{ id: 'a', start: 5, speaker: '', text: 'only' }])
    expect(srt).toContain('00:00:05,000 --> 00:00:08,000')
  })
  it('omits unstamped lines from SRT output', () => {
    const srt = toSRT([
      { id: 'a', start: null, speaker: '', text: 'unstamped' },
      { id: 'b', start: 1, speaker: '', text: 'stamped' },
    ])
    expect(srt).not.toContain('unstamped')
    expect(srt).toContain('stamped')
  })
})

describe('round trips', () => {
  it('SRT → segments → SRT preserves times, speakers, and text', () => {
    const once = toSRT(parseCues(SRT))
    const twice = toSRT(parseCues(once))
    expect(twice).toBe(once)
  })
  it('plain text → segments → plain text is stable', () => {
    const text = 'MR. SMITH: Good morning.\nunattributed line'
    expect(toPlainText(parsePlain(text))).toBe(text)
  })
})

describe('persistence', () => {
  it('keys transcripts by track path', () => {
    expect(storageKey('/a/b.wav')).toBe('transcript:/a/b.wav')
  })
  it('tolerates corrupt or missing storage', () => {
    expect(loadSegments(null)).toEqual([])
    expect(loadSegments('not json')).toEqual([])
    expect(loadSegments('{"not":"array"}')).toEqual([])
    expect(loadSegments('[{"id":"a"}]')).toEqual([{ id: 'a' }])
  })
})
