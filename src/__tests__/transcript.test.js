import { describe, it, expect } from 'vitest'
import {
  audioToTimeOfDay,
  cleanNote,
  describeDetection,
  detectTimeKind,
  findActiveIndex,
  flagRows,
  flagsToCsv,
  flagsToText,
  fmtClock,
  fmtElapsed,
  lineName,
  linePosition,
  linePositions,
  loadSegments,
  normalizeSync,
  parseClockInput,
  parseCues,
  parseStampedText,
  parseTime,
  parseTranscript,
  readStoredSegments,
  readStoredSync,
  removeStoredSegments,
  saveStoredSegments,
  saveStoredSync,
  setFlagNote,
  shiftForLineAt,
  splitSpeaker,
  storageKey,
  syncKey,
  timeOfDayToAudio,
  toggleFlag,
  transcriptText,
} from '../lib/transcript'

const strip = segs => segs.map(({ start, speaker, text }) => ({ start, speaker, text }))

describe('parseTime', () => {
  it('reads SRT, VTT, MM:SS and bare seconds', () => {
    expect(parseTime('01:02:03,500')).toBeCloseTo(3723.5)
    expect(parseTime('00:00:01.250')).toBeCloseTo(1.25)
    expect(parseTime('02:15')).toBe(135)
    expect(parseTime('90')).toBe(90)
  })
  it('returns null for anything else', () => {
    expect(parseTime('abc')).toBeNull()
    expect(parseTime('1:xx')).toBeNull()
    expect(parseTime('')).toBeNull()
    expect(parseTime('00:01:02 align:start')).toBeNull()
  })
  it('refuses minutes or seconds of 60 or more after the first field', () => {
    expect(parseTime('1:75')).toBeNull()
    expect(parseTime('00:61:00')).toBeNull()
    expect(parseTime('00:00:60,000')).toBeNull()
    expect(parseTime('75:30')).toBe(75 * 60 + 30) // the first field is not limited
  })
})

describe('parseClockInput', () => {
  it('reads 24-hour and am/pm times', () => {
    expect(parseClockInput('09:30')).toBe(9 * 3600 + 30 * 60)
    expect(parseClockInput('9:30:15')).toBe(9 * 3600 + 30 * 60 + 15)
    expect(parseClockInput('2:05:10 PM')).toBe(14 * 3600 + 5 * 60 + 10)
    expect(parseClockInput('12:00 am')).toBe(0)
    expect(parseClockInput('12:00 p.m.')).toBe(12 * 3600)
  })
  it('refuses times that do not exist', () => {
    expect(parseClockInput('25:00')).toBeNull()
    expect(parseClockInput('9:61')).toBeNull()
    expect(parseClockInput('13:00 pm')).toBeNull()
    expect(parseClockInput('nine thirty')).toBeNull()
  })
})

describe('time formatting', () => {
  it('formats elapsed time with hours and a clock time that wraps', () => {
    expect(fmtElapsed(3723.9)).toBe('1:02:03')
    expect(fmtElapsed(null)).toBe('—')
    expect(fmtClock(9 * 3600 + 5)).toBe('09:00:05')
    expect(fmtClock(86400 + 60)).toBe('00:01:00')
  })
})

describe('splitSpeaker', () => {
  it('splits a speaker prefix and leaves other lines alone', () => {
    expect(splitSpeaker('MR. SMITH: Objection.')).toEqual({ speaker: 'MR. SMITH', text: 'Objection.' })
    expect(splitSpeaker('the witness said: hello')).toEqual({ speaker: '', text: 'the witness said: hello' })
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

const VTT = `\uFEFFWEBVTT

NOTE this block is skipped
00:00:00.000 --> 00:00:00.500

intro
00:00:01.000 --> 00:00:02.000 align:start position:10%
<v Dr. Lee>Good <b>morning</b>.

00:01:02.500 --> 00:01:04.000
Q. Next question.
`

describe('SRT and WebVTT', () => {
  it('parses SRT cues with start times and speakers', () => {
    expect(strip(parseCues(SRT))).toEqual([
      { start: 1, speaker: 'MR. SMITH', text: 'Good morning.' },
      { start: 3, speaker: '', text: 'Second line continues here' },
    ])
  })
  it('parses VTT with a BOM, NOTE blocks, cue ids, settings and voice tags', () => {
    const parsed = parseTranscript(VTT, 'vtt')
    expect(parsed.format).toBe('vtt')
    expect(strip(parsed.segments)).toEqual([
      { start: 1, speaker: 'Dr. Lee', text: 'Good morning.' },
      { start: 62.5, speaker: '', text: 'Q. Next question.' },
    ])
    expect(parsed.timeKind).toBe('elapsed')
  })
  it('sniffs cues in a .txt file', () => {
    expect(parseTranscript(SRT, 'txt').format).toBe('srt')
  })
  it('keeps a cue with an unreadable time as an untimed line', () => {
    const segs = parseCues('1\nxx:yy --> 00:00:02,000\nKept text\n')
    expect(strip(segs)).toEqual([{ start: null, speaker: '', text: 'Kept text' }])
  })
})

const CAT_TOD = `   1  09:31:02   Q.  Please state your name.
   2  09:31:05   A.  Jane Example.
   3             continuing the answer
[09:31:20] MR. SMITH: Objection.
09:31:24  5  Q.  Go ahead.`

describe('timestamped text (CAT ASCII/TXT exports)', () => {
  it('reads line stamps with optional line numbers and brackets', () => {
    expect(strip(parseStampedText(CAT_TOD))).toEqual([
      { start: 9 * 3600 + 31 * 60 + 2, speaker: '', text: 'Q.  Please state your name.' },
      { start: 9 * 3600 + 31 * 60 + 5, speaker: '', text: 'A.  Jane Example.' },
      { start: null, speaker: '', text: 'continuing the answer' },
      { start: 9 * 3600 + 31 * 60 + 20, speaker: 'MR. SMITH', text: 'Objection.' },
      { start: 9 * 3600 + 31 * 60 + 24, speaker: '', text: 'Q.  Go ahead.' },
    ])
  })
  it('keeps the page and line number from the file, and does not list page or number-only rows', () => {
    const cat = [
      '                                                  12',
      '   1  09:31:02   Q.  Please state your name.',
      '   2',
      '   3             continuing the answer',
      '                                                  13',
      '   1  09:32:00   A.  Next page.',
      '09:32:04  2  Q.  Stamp first.',
    ].join('\n')
    const segs = parseStampedText(cat)
    expect(segs.map(({ page, lineNo, text }) => ({ page, lineNo, text }))).toEqual([
      { page: 12, lineNo: 1, text: 'Q.  Please state your name.' },
      { page: 12, lineNo: 3, text: 'continuing the answer' },
      { page: 13, lineNo: 1, text: 'A.  Next page.' },
      { page: 13, lineNo: 2, text: 'Q.  Stamp first.' },
    ])
    expect(segs.map((s, i) => lineName(s, i))).toEqual([
      'page 12, line 1',
      'page 12, line 3',
      'page 13, line 1',
      'page 13, line 2',
    ])
    expect(lineName({ text: 'cue' }, 4)).toBe('row 5')
  })
  it('keeps a line whose stamp is not a real time as an untimed line, text unchanged', () => {
    const [line] = parseStampedText('09:75:00 odd')
    expect(line.start).toBeNull()
    expect(line.text).toBe('09:75:00 odd')
  })
  it('reads elapsed stamps with fractions', () => {
    const segs = parseStampedText('00:00:01.500 First\n00:01:00 Second')
    expect(segs.map(s => s.start)).toEqual([1.5, 60])
  })
  it('keeps text without stamps as untimed lines, in order', () => {
    const parsed = parseTranscript('Line one\n\nLine two\n', 'txt')
    expect(parsed.format).toBe('text')
    expect(parsed.timedCount).toBe(0)
    expect(parsed.timeKind).toBeNull()
    expect(parsed.segments.map(s => s.text)).toEqual(['Line one', 'Line two'])
  })
  it('does not treat a time inside a sentence as a stamp', () => {
    expect(parseStampedText('We met at 10:30:00 that day')[0].start).toBeNull()
  })
  it('handles malformed and empty input without throwing', () => {
    expect(parseTranscript('', 'txt').segments).toEqual([])
    expect(parseTranscript(null, undefined).segments).toEqual([])
    expect(parseTranscript('-->\n-->', 'srt').segments).toEqual([])
    expect(parseStampedText('99:99:99 odd')[0].start).toBeNull()
  })
})

describe('elapsed vs time-of-day detection', () => {
  const at = (...times) => times.map(start => ({ start }))
  it('says elapsed for times near the start of the recording', () => {
    expect(detectTimeKind(at(5, 60, 3000))).toBe('elapsed')
  })
  it('says time of day for morning clock stamps', () => {
    expect(detectTimeKind(at(9 * 3600, 9 * 3600 + 30))).toBe('time-of-day')
  })
  it('says time of day when stamps start after the audio ends', () => {
    expect(detectTimeKind(at(2 * 3600), { duration: 1800 })).toBe('time-of-day')
    expect(detectTimeKind(at(2 * 3600), { duration: 4 * 3600 })).toBe('elapsed')
  })
  it('keeps SRT and WebVTT as elapsed', () => {
    expect(detectTimeKind(at(9 * 3600), { format: 'srt' })).toBe('elapsed')
  })
  it('describes what it found', () => {
    expect(describeDetection(parseTranscript(CAT_TOD, 'txt'))).toBe(
      '4 timed lines, 1 without a time; times look like time of day',
    )
    expect(describeDetection(parseTranscript(SRT, 'srt'))).toBe(
      '2 timed lines; times look like time into the recording',
    )
    expect(describeDetection(parseTranscript('a\nb', 'txt'))).toMatch(/2 lines of text with no times/)
  })
})

describe('time of day → audio position', () => {
  const nine = 9 * 3600
  it('maps against a single open-ended session', () => {
    expect(timeOfDayToAudio(nine + 90, [{ start: nine, duration: Infinity }])).toBe(90)
  })
  it('joins sessions end to end, and a time in a gap goes to the next session', () => {
    const sessions = [
      { start: nine, duration: 3600 }, // 09:00–10:00 → 0–3600
      { start: nine + 2 * 3600, duration: 1800 }, // 11:00–11:30 → 3600–5400
    ]
    expect(timeOfDayToAudio(nine + 600, sessions)).toBe(600)
    expect(timeOfDayToAudio(nine + 2 * 3600 + 60, sessions)).toBe(3660)
    expect(timeOfDayToAudio(nine + 3600 + 1200, sessions)).toBe(3600) // 10:20, in the gap
    expect(timeOfDayToAudio(nine - 300, sessions)).toBe(-300) // shortly before: left for the sync shift
    expect(timeOfDayToAudio(nine + 2 * 3600 + 1800, sessions)).toBe(5400) // the very end
    expect(timeOfDayToAudio(nine + 4 * 3600, sessions)).toBeNull() // after the last session
  })
  it('keeps stamps a little before the start unclamped, and refuses ones it cannot place', () => {
    const open = [{ start: nine, duration: Infinity }]
    expect(timeOfDayToAudio(nine - 15, open)).toBe(-15)
    expect(timeOfDayToAudio(nine - 2 * 3600, open)).toBeNull() // 07:00, two hours early
    expect(timeOfDayToAudio(21 * 3600 + 1800, open)).toBeNull() // 21:30, over 12 h after
    expect(timeOfDayToAudio(3600 + 1815, open)).toBeNull() // "01:30:15" on a 12-hour clock
    expect(timeOfDayToAudio(nine + 11 * 3600, open)).toBe(11 * 3600)
  })
  it('follows a recording past midnight', () => {
    expect(timeOfDayToAudio(10 * 60, [{ start: 23 * 3600 + 50 * 60, duration: Infinity }])).toBe(20 * 60)
  })
  it('refuses missing input', () => {
    expect(timeOfDayToAudio(nine, [])).toBeNull()
    expect(timeOfDayToAudio(NaN, [{ start: nine, duration: 10 }])).toBeNull()
  })
})

describe('sync and the active line', () => {
  const segs = [
    { id: 'a', start: 0, text: 'zero' },
    { id: 'b', start: 10, text: 'ten' },
    { id: 'c', start: null, text: 'continued' },
    { id: 'd', start: 20, text: 'twenty' },
  ]
  it('applies the sync shift to every timed line', () => {
    const sync = { shift: 2 }
    expect(linePositions(segs, sync)).toEqual([
      { own: 2, playFrom: 2 },
      { own: 12, playFrom: 12 },
      { own: null, playFrom: 12 },
      { own: 22, playFrom: 22 },
    ])
    expect(linePosition(0, { shift: -5 })).toBe(0) // never before the start
  })
  it('finds the line at the playhead with a shift', () => {
    const positions = linePositions(segs, { shift: 2 })
    expect(findActiveIndex(positions, 1)).toBe(-1)
    expect(findActiveIndex(positions, 2)).toBe(0)
    expect(findActiveIndex(positions, 15)).toBe(1)
    expect(findActiveIndex(positions, 22)).toBe(3)
    expect(findActiveIndex(positions, NaN)).toBe(-1)
  })
  it('resolves equal times to the first line in the file, and handles out-of-order times', () => {
    const positions = linePositions([{ start: 30 }, { start: 5 }, { start: 5 }], {})
    expect(findActiveIndex(positions, 6)).toBe(1)
    expect(findActiveIndex(positions, 40)).toBe(0)
  })
  it('places time-of-day lines only once the recording start is known', () => {
    const tod = [{ start: 9 * 3600 + 30 }]
    expect(linePositions(tod, { timeKind: 'time-of-day' })[0].own).toBeNull()
    expect(linePositions(tod, { timeKind: 'time-of-day', recordingStart: 9 * 3600, shift: 1 })[0].own).toBe(31)
    expect(audioToTimeOfDay(31, { recordingStart: 9 * 3600 })).toBe(9 * 3600 + 31)
  })
  it('lets a shift correct a clock that runs behind the recorder', () => {
    // Recording starts 09:30:00; the CAT clock is about 15 s behind.
    const start = 9 * 3600 + 30 * 60
    const tod = [{ start: start - 15 }, { start: start - 10 }, { start: start + 10 }]
    const sync = { timeKind: 'time-of-day', recordingStart: start }
    const shift = shiftForLineAt(tod[1].start, 5, sync)
    expect(shift).toBe(15)
    const positions = linePositions(tod, { ...sync, shift })
    expect(positions.map(p => p.own)).toEqual([0, 5, 25])
    expect(findActiveIndex(positions, 6)).toBe(1)
  })
  it('marks a time-of-day line that cannot be placed, with nowhere to play from', () => {
    const sync = { timeKind: 'time-of-day', recordingStart: 9 * 3600 }
    const positions = linePositions([{ start: 9 * 3600 + 5 }, { start: 21 * 3600 + 1800 }], sync)
    expect(positions[1]).toEqual({ own: null, playFrom: null, unplaced: true })
  })
  it('works out "Set this line to now"', () => {
    expect(shiftForLineAt(10, 12.34, {})).toBe(2.3)
    expect(shiftForLineAt(null, 12, {})).toBeNull()
  })
  it('normalizes stored sync settings', () => {
    expect(normalizeSync({ shift: 'x', timeKind: 'other', recordingStart: -1 })).toEqual({
      shift: 0,
      timeKind: 'elapsed',
      recordingStart: null,
    })
    expect(normalizeSync(null)).toEqual({ shift: 0, timeKind: 'elapsed', recordingStart: null })
  })
})

describe('flags', () => {
  const segs = [
    { id: 'a', start: 5, speaker: 'Q', text: 'Where were you?' },
    { id: 'b', start: 9, speaker: '', text: '=SUM(A1) "quoted", text' },
  ]
  it('flags and unflags without changing text, speaker or time', () => {
    const flagged = toggleFlag(segs, 'b')
    expect(flagged[1]).toEqual({ ...segs[1], flag: { note: '' } })
    expect(flagged[0]).toBe(segs[0])
    const noted = setFlagNote(flagged, 'b', 'check\nthis')
    expect(noted[1].flag.note).toBe('check this')
    expect(noted[1].text).toBe(segs[1].text)
    expect(toggleFlag(noted, 'b')[1]).toEqual(segs[1])
    expect(setFlagNote(segs, 'a', 'ignored')[0]).toEqual(segs[0])
    expect(cleanNote('x'.repeat(500))).toHaveLength(200)
  })
  it('lists flags with audio time and time of day', () => {
    const rows = flagRows(setFlagNote(toggleFlag(segs, 'a'), 'a', 'name?'), { recordingStart: 9 * 3600, shift: 1 })
    expect(rows).toEqual([
      {
        id: 'a',
        index: 0,
        page: undefined,
        lineNo: undefined,
        name: 'row 1',
        excerpt: 'Q: Where were you?',
        audioTime: 6,
        timeOfDay: 9 * 3600 + 6,
        note: 'name?',
      },
    ])
  })
  it('exports CSV safely and text plainly', () => {
    const rows = flagRows(toggleFlag(toggleFlag(segs, 'a'), 'b'), {})
    const csv = flagsToCsv(rows)
    expect(csv.split('\r\n')[0]).toBe('Page,Line,Row,Audio time,Time of day,Excerpt,Note')
    expect(csv).toContain(',,1,0:00:05,,Q: Where were you?,')
    expect(csv).toContain(`,,2,0:00:09,,"'=SUM(A1) ""quoted"", text",`)
    const text = flagsToText(rows, 'day1.wav')
    expect(text).toMatch(/^Proofing flags for day1.wav \(2\)/)
    expect(text).toContain('Row 1 · audio 0:00:05')
  })
  it('exports the page and line number from the file, not the row in the list', () => {
    const cat = '            12\n  1  00:00:05  Q.  First.\n  2  00:00:09  A.  Second.'
    const segs = parseStampedText(cat)
    const rows = flagRows(toggleFlag(segs, segs[1].id), {})
    expect(flagsToCsv(rows).split('\r\n')[1]).toBe('12,2,2,0:00:09,,A. Second.,')
    expect(flagsToText(rows)).toContain('Page 12, line 2 · audio 0:00:09')
  })
  it('uses the same session mapping as the panel', () => {
    const tod = [{ id: 'a', start: 9 * 3600 + 3 * 3600, flag: { note: '' }, speaker: '', text: 'after lunch' }]
    const sessions = [
      { start: 9 * 3600, duration: 3600 },
      { start: 11 * 3600, duration: 3600 },
    ]
    const sync = { timeKind: 'time-of-day', recordingStart: 9 * 3600 }
    expect(flagRows(tod, sync)[0].audioTime).toBe(3 * 3600)
    expect(flagRows(tod, sync, sessions)[0].audioTime).toBe(3600 + 3600)
  })
})

describe('copy text', () => {
  it("keeps each line's time, so stored transcripts can still be taken out with their times", () => {
    const segs = [
      { start: 62.5, speaker: 'Q', text: 'Typed earlier.' },
      { start: null, speaker: '', text: 'No time.' },
    ]
    expect(transcriptText(segs)).toBe('0:01:02.500\tQ: Typed earlier.\nNo time.')
    expect(transcriptText([{ start: 9 * 3600 + 1.25, speaker: '', text: 'x' }], 'time-of-day')).toBe('09:00:01.250\tx')
  })
})

function memoryStorage(initial = {}) {
  const data = new Map(Object.entries(initial))
  return {
    data,
    getItem: key => (data.has(key) ? data.get(key) : null),
    setItem: (key, value) => data.set(key, String(value)),
    removeItem: key => data.delete(key),
  }
}

describe('storage', () => {
  it('loads transcripts saved by earlier versions unchanged', () => {
    const old = [
      { id: 'x1', start: 1.5, speaker: 'Q', text: 'Typed in an earlier version' },
      { id: 'x2', start: null, speaker: '', text: 'Not stamped' },
    ]
    const storage = memoryStorage({ [storageKey('/a.wav')]: JSON.stringify(old) })
    expect(readStoredSegments(storage, '/a.wav')).toEqual({ segments: old, storageAvailable: true })
  })
  it('tolerates corrupt storage and bad fields', () => {
    expect(loadSegments('not json')).toEqual([])
    expect(loadSegments('{"a":1}')).toEqual([])
    const loaded = loadSegments(JSON.stringify([{ id: 'a', start: -1, text: 5 }, null, { id: 'a', flag: { note: 7 } }]))
    expect(loaded).toEqual([
      { id: 'a', start: null, speaker: '', text: '' },
      { id: 'a-2', start: null, speaker: '', text: '', flag: { note: '7' } },
    ])
  })
  it('keeps the page and line number through a save and load, and drops bad ones', () => {
    const loaded = loadSegments(
      JSON.stringify([
        { id: 'a', start: 1, speaker: '', text: 't', page: 12, lineNo: 4 },
        { id: 'b', start: 1, speaker: '', text: 't', page: 'x', lineNo: -1 },
      ]),
    )
    expect(loaded[0]).toMatchObject({ page: 12, lineNo: 4 })
    expect(loaded[1]).not.toHaveProperty('page')
    expect(loaded[1]).not.toHaveProperty('lineNo')
  })
  it('keeps flags through a save and load', () => {
    const storage = memoryStorage()
    const segs = setFlagNote(toggleFlag([{ id: 'a', start: 1, speaker: '', text: 't' }], 'a'), 'a', 'n')
    expect(saveStoredSegments(storage, '/a.wav', segs)).toBe(true)
    expect(readStoredSegments(storage, '/a.wav').segments).toEqual(segs)
  })
  it('saves sync settings per track, and removing the transcript removes them too', () => {
    const storage = memoryStorage({ [storageKey('/a.wav')]: '[]' })
    expect(saveStoredSync(storage, '/a.wav', { shift: 1.5, timeKind: 'time-of-day', recordingStart: 100 })).toBe(true)
    expect(readStoredSync(storage, '/a.wav')).toEqual({ shift: 1.5, timeKind: 'time-of-day', recordingStart: 100 })
    expect(readStoredSync(storage, '/b.wav')).toBeNull()
    expect(removeStoredSegments(storage, '/a.wav')).toBe(true)
    expect(storage.data.has(syncKey('/a.wav'))).toBe(false)
    expect(storage.data.has(storageKey('/a.wav'))).toBe(false)
  })
  it('reports unavailable storage', () => {
    expect(readStoredSegments(null, '/a.wav').storageAvailable).toBe(false)
    expect(saveStoredSegments(null, '/a.wav', [])).toBe(false)
    expect(readStoredSync({ getItem: () => '{bad' }, '/a.wav')).toBeNull()
  })
})
