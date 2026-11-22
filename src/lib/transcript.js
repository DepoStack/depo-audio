// ── Transcript proofing (pure) ──────────────────────────────────────────────
//
// DepoAudio does not write or edit transcripts: that belongs in CAT software.
// The Player's transcript panel lets a reporter, scopist or proofreader proof
// an existing transcript against the audio. This file holds everything that
// panel needs and that tests pin down:
//   • reading SRT, WebVTT and timestamped plain text (CAT ASCII/TXT exports),
//     and telling elapsed-time stamps from time-of-day stamps;
//   • mapping a line's time to a position in the audio (sync shift, and the
//     recording's start time for time-of-day stamps). The mapping can join
//     several sessions, but the panel has one start time only, so it maps
//     every line against one session;
//   • finding the line at the playhead;
//   • proofing flags (stored on the line, never touching its text) and their
//     export as text or CSV, with the page and line number from the file;
//   • the per-track storage (localStorage) that earlier versions also used.

export const uid = () => Math.random().toString(36).slice(2, 10)

const DAY = 86400
const HALF_DAY = DAY / 2
// A time-of-day stamp up to an hour before the recording started is taken as
// a clock that runs behind the recorder, not as a time that cannot be placed.
const EARLY_LIMIT = 3600
export const MAX_FLAG_NOTE_CHARS = 200

const validTime = value => typeof value === 'number' && Number.isFinite(value) && value >= 0

// ── Time text ────────────────────────────────────────────────────────────────

// "HH:MM:SS,mmm" / "HH:MM:SS.mmm" / "MM:SS" / "SS" → seconds (null if invalid)
export function parseTime(str) {
  const text = String(str ?? '').trim()
  if (!/^\d+(?::\d{1,2}){0,2}(?:[.,]\d+)?$/.test(text)) return null
  const parts = text.replace(',', '.').split(':').map(Number)
  if (parts.some(Number.isNaN)) return null
  // Minutes and seconds after the first field must be under 60 ("1:75" is not a time).
  if (parts.slice(1).some(n => n >= 60)) return null
  if (parts.length === 3) return parts[0] * 3600 + parts[1] * 60 + parts[2]
  if (parts.length === 2) return parts[0] * 60 + parts[1]
  return parts[0]
}

// A time of day typed by the person: "9:30", "09:30:15", "9:30 am", "2:05:10 PM".
// Returns seconds after midnight, or null when it is not a time of day.
export function parseClockInput(str) {
  const match = String(str ?? '')
    .trim()
    .match(/^(\d{1,2}):(\d{2})(?::(\d{2})(?:[.,](\d{1,3}))?)?\s*([ap]\.?m\.?)?$/i)
  if (!match) return null
  let hours = Number(match[1])
  const minutes = Number(match[2])
  const seconds = Number(match[3] ?? 0) + Number(`0.${match[4] ?? 0}`)
  const meridiem = match[5]?.[0]?.toLowerCase()
  if (minutes > 59 || seconds >= 60) return null
  if (meridiem) {
    if (hours < 1 || hours > 12) return null
    if (meridiem === 'p' && hours !== 12) hours += 12
    if (meridiem === 'a' && hours === 12) hours = 0
  } else if (hours > 23) {
    return null
  }
  return hours * 3600 + minutes * 60 + seconds
}

const pad = (n, w = 2) => String(n).padStart(w, '0')

// Seconds → "H:MM:SS" (elapsed) — hours always shown so long records line up.
export function fmtElapsed(seconds) {
  if (!validTime(seconds)) return '—'
  const whole = Math.floor(seconds)
  return `${Math.floor(whole / 3600)}:${pad(Math.floor(whole / 60) % 60)}:${pad(whole % 60)}`
}

// Seconds after midnight → "HH:MM:SS" (wraps past midnight).
export function fmtClock(seconds) {
  if (!Number.isFinite(seconds)) return '—'
  const whole = ((Math.floor(seconds) % DAY) + DAY) % DAY
  return `${pad(Math.floor(whole / 3600))}:${pad(Math.floor(whole / 60) % 60)}:${pad(whole % 60)}`
}

// Signed seconds → "+1.5 s" / "-0.5 s"
export const fmtShift = seconds => `${seconds > 0 ? '+' : seconds < 0 ? '-' : ''}${Math.abs(seconds).toFixed(1)} s`

// ── Parsing ──────────────────────────────────────────────────────────────────

// Split "SPEAKER: text" into { speaker, text }
export function splitSpeaker(line) {
  const m = line.match(/^\s*([A-Z][A-Za-z0-9 .'_-]{0,30}):\s+(.*)$/)
  return m ? { speaker: m[1], text: m[2] } : { speaker: '', text: line.trim() }
}

const segment = (start, line) => ({ id: uid(), start, ...splitSpeaker(line) })

// SRT or WebVTT cues. VTT headers, NOTE/STYLE/REGION blocks, cue settings and
// voice tags (<v Name>) are handled; a cue whose start time cannot be read is
// kept as an untimed line so no text is dropped.
export function parseCues(text) {
  const lines = String(text ?? '')
    .replace(/^\uFEFF/, '')
    .replace(/\r/g, '')
    .split('\n')
  const segs = []
  let i = 0
  while (i < lines.length) {
    const line = lines[i]
    if (/^(NOTE|STYLE|REGION)\b/.test(line.trim())) {
      while (i < lines.length && lines[i].trim() !== '') i++
      continue
    }
    if (line.includes('-->')) {
      const start = parseTime(line.split('-->')[0])
      i++
      const body = []
      while (i < lines.length && lines[i].trim() !== '' && !lines[i].includes('-->')) {
        body.push(lines[i])
        i++
      }
      let joined = body.join(' ').trim()
      const voice = joined.match(/^<v(?:\.[^\s>]*)?\s+([^>]+)>/)
      joined = joined.replace(/<[^>]+>/g, '').trim()
      const parsed = voice ? { id: uid(), start, speaker: voice[1].trim(), text: joined } : segment(start, joined)
      if (parsed.text || parsed.speaker) segs.push(parsed)
    } else {
      i++
    }
  }
  return segs
}

// A line stamp in a CAT ASCII/TXT export: "[09:31:02]", "09:31:02", or
// "00:01:02.500", optionally after a line number and before a line number
// ("  4  09:31:02   Q.  Did you..." or "09:31:02  4  Q. ...").
const STAMPED_LINE =
  /^\s*(?:(\d{1,5})\s+)?\[?(\d{1,2}:\d{2}:\d{2}(?:[.,]\d{1,3})?)\]?(?=\s|$)\s*(?:(\d{1,2})(?=\s{2,}|\t|$)\s*)?(.*)$/
// A line with no stamp that starts with its line number: "  3        continuing".
const NUMBERED_LINE = /^\s*(\d{1,2})(?:\s{2,}|\t)\s*(\S.*)$/
// A row holding only a number: a page number, or a numbered line with no text.
const NUMBER_ONLY = /^(\s*)(\d{1,5})\s*$/

// Each line keeps the page and line number it has in the file (`page`,
// `lineNo`), so a flag can be found again in the CAT file. A row holding only
// a number is not a transcript line: it is the next line number on the page
// when it follows the last one and is not indented far, and a page number
// otherwise. Either way it sets where the next lines are, and is not listed.
export function parseStampedText(text) {
  const segs = []
  let page = null
  let lastLine = null
  for (const raw of String(text ?? '')
    .replace(/^\uFEFF/, '')
    .replace(/\r/g, '')
    .split('\n')) {
    if (!raw.trim()) continue
    const only = raw.match(NUMBER_ONLY)
    if (only) {
      const n = Number(only[2])
      if (only[1].length < 8 && lastLine != null && n === lastLine + 1) {
        lastLine = n
      } else {
        page = n
        lastLine = null
      }
      continue
    }
    let start = null
    let lineNo = null
    let body = raw
    const stamped = raw.match(STAMPED_LINE)
    const stamp = stamped ? parseTime(stamped[2]) : null
    if (stamped && stamp != null) {
      start = stamp
      lineNo = stamped[3] ?? stamped[1] ?? null
      body = stamped[4]
    } else {
      const numbered = raw.match(NUMBERED_LINE)
      if (numbered) {
        lineNo = numbered[1]
        body = numbered[2]
      }
    }
    const line = segment(start, body.trim())
    if (page != null) line.page = page
    if (lineNo != null) {
      line.lineNo = Number(lineNo)
      lastLine = line.lineNo
    }
    segs.push(line)
  }
  return segs
}

// Elapsed time vs time of day. SRT and WebVTT are elapsed by definition. For
// text, stamps read as time of day when the first one is an hour or more in
// and either the audio is shorter than that or the stamps start at a clock
// hour a recording rarely reaches (7 am or later). The person can change it.
export function detectTimeKind(segments, { format = 'text', duration = null } = {}) {
  const times = segments.map(s => s.start).filter(validTime)
  if (times.length === 0) return null
  if (format === 'srt' || format === 'vtt') return 'elapsed'
  const first = Math.min(...times)
  if (first < 3600) return 'elapsed'
  if (validTime(duration) && duration > 0 && first > duration) return 'time-of-day'
  return first >= 7 * 3600 ? 'time-of-day' : 'elapsed'
}

// Read an imported file. Returns the lines plus what was found, so the panel
// can say so ("42 timed lines; times look like time of day").
export function parseTranscript(text, ext, { duration = null } = {}) {
  const lower = String(ext ?? '').toLowerCase()
  const body = String(text ?? '')
  const isVtt = lower === 'vtt' || /^\uFEFF?WEBVTT/.test(body)
  const isCues = isVtt || lower === 'srt' || body.includes('-->')
  const format = isVtt ? 'vtt' : isCues ? 'srt' : 'text'
  const segments = isCues ? parseCues(body) : parseStampedText(body)
  const timedCount = segments.filter(s => validTime(s.start)).length
  return {
    segments,
    format,
    timedCount,
    untimedCount: segments.length - timedCount,
    timeKind: detectTimeKind(segments, { format, duration }),
  }
}

const FORMAT_NAMES = { srt: 'SRT subtitles', vtt: 'WebVTT', text: 'text' }

export function describeDetection({ format, timedCount, untimedCount, timeKind }) {
  const lines = n => `${n} ${n === 1 ? 'line' : 'lines'}`
  if (!timedCount) {
    return `${lines(untimedCount)} of ${FORMAT_NAMES[format] ?? 'text'} with no times, so lines cannot follow the audio`
  }
  const parts = [`${timedCount} timed ${timedCount === 1 ? 'line' : 'lines'}`]
  if (untimedCount) parts.push(`${untimedCount} without a time`)
  return `${parts.join(', ')}; times look like ${timeKind === 'time-of-day' ? 'time of day' : 'time into the recording'}`
}

// ── Sync: transcript time → audio position ──────────────────────────────────

export const DEFAULT_SYNC = { shift: 0, timeKind: 'elapsed', recordingStart: null }

export function normalizeSync(value) {
  const sync = value && typeof value === 'object' ? value : {}
  return {
    shift: Number.isFinite(sync.shift) ? Math.round(sync.shift * 1000) / 1000 : 0,
    timeKind: sync.timeKind === 'time-of-day' ? 'time-of-day' : 'elapsed',
    recordingStart: Number.isFinite(sync.recordingStart) && sync.recordingStart >= 0 ? sync.recordingStart % DAY : null,
  }
}

// Map a time of day to a position in the audio. `sessions` is the recording's
// sessions in order, each { start: seconds after midnight, duration: seconds }
// (the last may be open-ended: duration Infinity, taken as at most 12 hours).
// The timeline joins the sessions end to end. A time between two sessions
// goes to the start of the next session (how CATalyst places a time stamp
// that falls in a gap). A time up to an hour before the first session gives a
// negative position, so a sync shift can still correct a clock that runs
// behind; linePosition clamps it to 0 after the shift. A time earlier than
// that, or after the last session ends, returns null: the line cannot be
// placed. A recording that runs past midnight still maps.
export function timeOfDayToAudio(timeOfDay, sessions) {
  if (!Number.isFinite(timeOfDay) || !Array.isArray(sessions) || sessions.length === 0) return null
  const origin = sessions[0].start
  if (!Number.isFinite(origin)) return null
  // Seconds from the first session's start, in [-1 h, +23 h).
  const fromOrigin = t => ((((t - origin + EARLY_LIMIT) % DAY) + DAY) % DAY) - EARLY_LIMIT
  const target = fromOrigin(timeOfDay)
  if (target < 0) return target
  let elapsed = 0
  let previousEnd = -Infinity
  for (const session of sessions) {
    if (!Number.isFinite(session?.start)) return null
    let start = fromOrigin(session.start)
    while (start < previousEnd) start += DAY
    const duration = session.duration > 0 ? Math.min(session.duration, HALF_DAY) : 0
    if (target < start) return elapsed
    if (target < start + duration) return elapsed + (target - start)
    elapsed += duration
    previousEnd = start + duration
  }
  return target === previousEnd ? elapsed : null
}

// The audio position a line plays from, before the sync shift (negative for a
// time-of-day stamp a little before the recording started). Null when the
// line has no time, or its time of day cannot be placed.
export function lineBasePosition(start, sync, sessions = null) {
  if (!validTime(start)) return null
  const { timeKind, recordingStart } = normalizeSync(sync)
  if (timeKind !== 'time-of-day') return start
  const list = sessions ?? (recordingStart == null ? null : [{ start: recordingStart, duration: Infinity }])
  return list ? timeOfDayToAudio(start, list) : null
}

export function linePosition(start, sync, sessions = null) {
  const base = lineBasePosition(start, sync, sessions)
  return base == null ? null : Math.max(0, base + normalizeSync(sync).shift)
}

// Audio position → time of day, when the recording's start is known (single session).
export function audioToTimeOfDay(position, sync) {
  const { recordingStart } = normalizeSync(sync)
  return recordingStart == null || !Number.isFinite(position) ? null : (recordingStart + position) % DAY
}

// Each line's audio position, or null. Lines without a time play from the
// nearest timed line above them (CAT exports often stamp only some lines). A
// line whose time cannot be placed in the audio is marked `unplaced` and has
// nowhere to play from.
export function linePositions(segments, sync, sessions = null) {
  let previous = null
  return (Array.isArray(segments) ? segments : []).map(s => {
    const own = linePosition(s?.start, sync, sessions)
    if (own != null) previous = own
    if (own == null && validTime(s?.start)) return { own, playFrom: null, unplaced: true }
    return { own, playFrom: own ?? previous }
  })
}

// The line playing at `currentTime`: the last line (in time order) whose own
// position is at or before it. Ties go to the first such line in the file.
export function findActiveIndex(positions, currentTime, tolerance = 0.05) {
  if (!Number.isFinite(currentTime) || !Array.isArray(positions)) return -1
  const index = []
  positions.forEach((p, sourceIndex) => {
    if (p?.own != null) index.push({ at: p.own, sourceIndex })
  })
  index.sort((a, b) => a.at - b.at || b.sourceIndex - a.sourceIndex)
  const cutoff = currentTime + (Number.isFinite(tolerance) ? Math.max(0, tolerance) : 0)
  let low = 0
  let high = index.length - 1
  let active = -1
  while (low <= high) {
    const middle = low + Math.floor((high - low) / 2)
    if (index[middle].at <= cutoff) {
      active = middle
      low = middle + 1
    } else {
      high = middle - 1
    }
  }
  return active < 0 ? -1 : index[active].sourceIndex
}

// "Set this line to now": the shift that makes `line` play from `now`.
export function shiftForLineAt(start, now, sync, sessions = null) {
  const base = lineBasePosition(start, sync, sessions)
  if (base == null || !Number.isFinite(now)) return null
  return Math.round((now - base) * 10) / 10
}

// ── Flags ────────────────────────────────────────────────────────────────────

export const cleanNote = note =>
  String(note ?? '')
    // eslint-disable-next-line no-control-regex
    .replace(/[\u0000-\u001f\u007f]+/g, ' ')
    .slice(0, MAX_FLAG_NOTE_CHARS)

// Flag or unflag a line. Only the line's `flag` changes; its text, speaker
// and time are copied as they are.
export function toggleFlag(segments, id) {
  return segments.map(s => {
    if (s.id !== id) return s
    if (s.flag) {
      const { flag: _flag, ...rest } = s
      return rest
    }
    return { ...s, flag: { note: '' } }
  })
}

export function setFlagNote(segments, id, note) {
  return segments.map(s => (s.id === id && s.flag ? { ...s, flag: { note: cleanNote(note) } } : s))
}

const lineText = s => (s.speaker ? `${s.speaker}: ` : '') + s.text

// How a line is named on screen and in exports: by its page and line number
// in the file when it has them, else by its row in DepoAudio's list.
export const lineName = (s, index) =>
  s?.lineNo == null ? `row ${index + 1}` : `${s.page == null ? '' : `page ${s.page}, `}line ${s.lineNo}`

// The short form shown beside each line: "12:4", "4" or the row number.
export const lineRef = (s, index) =>
  s?.lineNo == null ? index + 1 : `${s.page == null ? '' : `${s.page}:`}${s.lineNo}`

const capitalize = text => text.charAt(0).toUpperCase() + text.slice(1)

const withMillis = (formatted, seconds) =>
  `${formatted}.${pad(Math.min(999, Math.floor((seconds % 1) * 1000 + 1e-6)), 3)}`

// The transcript as plain text, each timed line after its time as shown in
// the panel, to the millisecond. Copy with times uses it, so the times of
// transcripts saved by earlier versions can still be taken out of DepoAudio.
export function transcriptText(segments, timeKind) {
  return (Array.isArray(segments) ? segments : [])
    .map(s => {
      if (!validTime(s.start)) return lineText(s)
      const time = timeKind === 'time-of-day' ? fmtClock(s.start) : fmtElapsed(s.start)
      return `${withMillis(time, s.start)}\t${lineText(s)}`
    })
    .join('\n')
}

export function excerpt(text, max = 80) {
  const flat = String(text ?? '')
    .replace(/\s+/g, ' ')
    .trim()
  return flat.length > max ? `${flat.slice(0, max - 1).trimEnd()}…` : flat
}

// `sessions` must be what the panel maps with, so exported audio times match it.
export function flagRows(segments, sync, sessions = null) {
  const positions = linePositions(segments, sync, sessions)
  const normalized = normalizeSync(sync)
  return segments
    .map((s, index) => ({ s, index, position: positions[index].playFrom }))
    .filter(({ s }) => s.flag)
    .map(({ s, index, position }) => ({
      id: s.id,
      index,
      page: s.page,
      lineNo: s.lineNo,
      name: lineName(s, index),
      excerpt: excerpt(lineText(s)),
      audioTime: position,
      timeOfDay:
        normalized.timeKind === 'time-of-day' && validTime(s.start)
          ? s.start
          : position == null
            ? null
            : audioToTimeOfDay(position, normalized),
      note: s.flag.note ?? '',
    }))
}

// Spreadsheet programs run a cell that starts with = + - @ as a formula;
// a leading apostrophe keeps it as text.
function csvCell(value) {
  let text = String(value ?? '')
  if (/^[=+\-@\t\r]/.test(text)) text = `'${text}`
  return /[",\n\r]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text
}

export function flagsToCsv(rows) {
  const header = ['Page', 'Line', 'Row', 'Audio time', 'Time of day', 'Excerpt', 'Note']
  const body = rows.map(r => [
    r.page ?? '',
    r.lineNo ?? '',
    r.index + 1,
    fmtElapsed(r.audioTime),
    r.timeOfDay == null ? '' : fmtClock(r.timeOfDay),
    r.excerpt,
    r.note,
  ])
  return [header, ...body].map(row => row.map(csvCell).join(',')).join('\r\n') + '\r\n'
}

export function flagsToText(rows, trackName = '') {
  const head = [`Proofing flags${trackName ? ` for ${trackName}` : ''} (${rows.length})`, '']
  const body = rows.map(r => {
    const times = [`audio ${fmtElapsed(r.audioTime)}`]
    if (r.timeOfDay != null) times.push(`time of day ${fmtClock(r.timeOfDay)}`)
    return [`${capitalize(r.name)} · ${times.join(' · ')}`, `  ${r.excerpt}`, r.note ? `  Note: ${r.note}` : null]
      .filter(Boolean)
      .join('\n')
  })
  return [...head, ...body].join('\n') + '\n'
}

// ── Storage (per track, localStorage) ───────────────────────────────────────
//
// "transcript:<track path>" holds the lines as earlier versions saved them
// ([{ id, start, speaker, text }]), now with an optional `flag: { note }` and
// the line's `page` and `lineNo` in the file when it had them.
// Backups carry this key unchanged. "transcript-sync:<track path>" holds the
// sync settings for that track.

export const TRANSCRIPT_KEY_PREFIX = 'transcript:'
export const SYNC_KEY_PREFIX = 'transcript-sync:'
export const storageKey = path => `${TRANSCRIPT_KEY_PREFIX}${path}`
export const syncKey = path => `${SYNC_KEY_PREFIX}${path}`

function uniqueStoredId(value, usedIds) {
  const base = typeof value === 'string' && value.trim() ? value.trim() : uid()
  let id = base
  let suffix = 2
  while (usedIds.has(id)) {
    id = `${base}-${suffix}`
    suffix += 1
  }
  usedIds.add(id)
  return id
}

// Load a persisted transcript, tolerating corrupt storage (never crash the tab)
export function loadSegments(raw) {
  try {
    const v = JSON.parse(raw || '[]')
    if (!Array.isArray(v)) return []

    const usedIds = new Set()
    return v
      .filter(segment => segment && typeof segment === 'object' && !Array.isArray(segment))
      .map(segment => {
        const line = {
          id: uniqueStoredId(segment.id, usedIds),
          start: validTime(segment.start) ? segment.start : null,
          speaker: typeof segment.speaker === 'string' ? segment.speaker : '',
          text: typeof segment.text === 'string' ? segment.text : '',
        }
        for (const key of ['page', 'lineNo']) {
          if (Number.isInteger(segment[key]) && segment[key] >= 0) line[key] = segment[key]
        }
        if (segment.flag && typeof segment.flag === 'object') line.flag = { note: cleanNote(segment.flag.note) }
        return line
      })
  } catch {
    return []
  }
}

function getItem(storage, key) {
  if (!storage || typeof storage.getItem !== 'function') return { value: null, ok: false }
  try {
    return { value: storage.getItem(key), ok: true }
  } catch {
    return { value: null, ok: false }
  }
}

export function readStoredSegments(storage, path) {
  const { value, ok } = getItem(storage, storageKey(path))
  return { segments: loadSegments(value), storageAvailable: ok }
}

export function readStoredSync(storage, path) {
  const { value } = getItem(storage, syncKey(path))
  try {
    return value ? normalizeSync(JSON.parse(value)) : null
  } catch {
    return null
  }
}

function setItem(storage, key, value) {
  if (!storage || typeof storage.setItem !== 'function') return false
  try {
    storage.setItem(key, value)
    return true
  } catch {
    return false
  }
}

export function saveStoredSegments(storage, path, segments) {
  if (!setItem(storage, storageKey(path), JSON.stringify(segments))) return false
  // An empty transcript keeps no key (DA-07): the key holds the track's path,
  // which can name the case. The write above still runs first, so blocked
  // storage is reported.
  if (Array.isArray(segments) && segments.length === 0) return removeStoredSegments(storage, path)
  return true
}

export function saveStoredSync(storage, path, sync) {
  return setItem(storage, syncKey(path), JSON.stringify(normalizeSync(sync)))
}

// "Remove this transcript from DepoAudio" (DA-07): removes the stored copy
// and its sync settings only. The transcript file itself is never touched.
export function removeStoredSegments(storage, path) {
  if (!storage || typeof storage.removeItem !== 'function') return false
  try {
    storage.removeItem(storageKey(path))
    storage.removeItem(syncKey(path))
    return true
  } catch {
    return false
  }
}
