// ── Transcript parsing / serialization (pure) ───────────────────────────────
//
// The formats the transcript editor reads and writes. Extracted from the
// Transcript component so characterization tests can pin the behavior:
// these functions define what happens to users' transcript files.

export const uid = () => Math.random().toString(36).slice(2, 10)

// Parse "HH:MM:SS,mmm" / "HH:MM:SS.mmm" / "MM:SS" → seconds (null if invalid)
export function parseTime(str) {
  const parts = str.trim().replace(',', '.').split(':').map(Number)
  if (!parts.length || parts.some(Number.isNaN)) return null
  if (parts.length === 3) return parts[0] * 3600 + parts[1] * 60 + parts[2]
  if (parts.length === 2) return parts[0] * 60 + parts[1]
  return parts[0]
}

// Split "SPEAKER: text" into { speaker, text }
export function splitSpeaker(line) {
  const m = line.match(/^\s*([A-Z][A-Za-z0-9 .'_-]{0,30}):\s+(.*)$/)
  return m ? { speaker: m[1], text: m[2] } : { speaker: '', text: line.trim() }
}

export function parseCues(text) {
  const lines = text.replace(/\r/g, '').split('\n')
  const segs = []
  let i = 0
  while (i < lines.length) {
    if (lines[i].includes('-->')) {
      const start = parseTime(lines[i].split('-->')[0])
      i++
      const body = []
      while (i < lines.length && lines[i].trim() !== '' && !lines[i].includes('-->')) {
        body.push(lines[i]); i++
      }
      const { speaker, text } = splitSpeaker(body.join(' ').trim())
      segs.push({ id: uid(), start, speaker, text })
    } else { i++ }
  }
  return segs
}

export function parsePlain(text) {
  return text.replace(/\r/g, '').split('\n').map(l => l.trim()).filter(Boolean).map(line => {
    const { speaker, text: body } = splitSpeaker(line)
    return { id: uid(), start: null, speaker, text: body }
  })
}

export function parseTranscript(text, ext) {
  if (ext === 'srt' || ext === 'vtt' || text.includes('-->')) return parseCues(text)
  return parsePlain(text)
}

export const srtStamp = (t) => {
  const ms = Math.floor((t % 1) * 1000)
  const s = Math.floor(t) % 60
  const m = Math.floor(t / 60) % 60
  const h = Math.floor(t / 3600)
  const pad = (n, w = 2) => String(n).padStart(w, '0')
  return `${pad(h)}:${pad(m)}:${pad(s)},${pad(ms, 3)}`
}

export function toPlainText(segs) {
  return segs.map(s => (s.speaker ? `${s.speaker}: ` : '') + s.text).join('\n')
}

export function toSRT(segs) {
  // Stamped lines can be added out of order, so sort by start time — otherwise
  // a cue's end (taken from the next line) could precede its start, producing
  // negative/overlapping durations.
  const timed = segs.filter(s => s.start != null).sort((a, b) => a.start - b.start)
  return timed.map((s, idx) => {
    const end = timed[idx + 1] ? timed[idx + 1].start : s.start + 3
    const body = (s.speaker ? `${s.speaker}: ` : '') + s.text
    return `${idx + 1}\n${srtStamp(s.start)} --> ${srtStamp(end)}\n${body}\n`
  }).join('\n')
}

// Transcripts persist per track in localStorage, keyed by file path.
export const storageKey = (path) => `transcript:${path}`

// Load a persisted transcript, tolerating corrupt storage (never crash the tab)
export function loadSegments(raw) {
  try {
    const v = JSON.parse(raw || '[]')
    return Array.isArray(v) ? v : []
  } catch { return [] }
}
