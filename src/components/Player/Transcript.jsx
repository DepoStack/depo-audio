import { useState, useRef, useEffect, useMemo } from 'react'
import { open, save } from '@tauri-apps/plugin-dialog'
import { readTextFile, writeTextFile } from '@tauri-apps/plugin-fs'
import { Upload, Download, Copy, Check, Crosshair, Plus, X, Locate } from 'lucide-react'
import { cn } from '../../lib/utils'
import { fmtTime } from '../../utils'
import { Card, CardHeader, CardTitle } from '../ui/card'
import { Button } from '../ui/button'

// ── Synced transcript editor ────────────────────────────────────────────────
//
// Proof an existing transcript against the audio, or build one from scratch:
//   • Import SRT / VTT / TXT (or paste); edit every line inline (autosaved).
//   • Timed lines highlight and follow as audio plays; click a time to jump.
//   • "Stamp" a line with the current audio position to anchor plain text.
//   • Enter starts a new line stamped at the current position (fast capture).
//   • Export to SRT or TXT, or copy to the clipboard.
//
// Transcripts persist per track in localStorage, keyed by file path.

const uid = () => Math.random().toString(36).slice(2, 10)

// Parse "HH:MM:SS,mmm" / "HH:MM:SS.mmm" / "MM:SS" → seconds (null if invalid)
function parseTime(str) {
  const parts = str.trim().replace(',', '.').split(':').map(Number)
  if (!parts.length || parts.some(Number.isNaN)) return null
  if (parts.length === 3) return parts[0] * 3600 + parts[1] * 60 + parts[2]
  if (parts.length === 2) return parts[0] * 60 + parts[1]
  return parts[0]
}

// Split "SPEAKER: text" into { speaker, text }
function splitSpeaker(line) {
  const m = line.match(/^\s*([A-Z][A-Za-z0-9 .'_-]{0,30}):\s+(.*)$/)
  return m ? { speaker: m[1], text: m[2] } : { speaker: '', text: line.trim() }
}

function parseCues(text) {
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

function parsePlain(text) {
  return text.replace(/\r/g, '').split('\n').map(l => l.trim()).filter(Boolean).map(line => {
    const { speaker, text: body } = splitSpeaker(line)
    return { id: uid(), start: null, speaker, text: body }
  })
}

function parseTranscript(text, ext) {
  if (ext === 'srt' || ext === 'vtt' || text.includes('-->')) return parseCues(text)
  return parsePlain(text)
}

const srtStamp = (t) => {
  const ms = Math.floor((t % 1) * 1000)
  const s = Math.floor(t) % 60
  const m = Math.floor(t / 60) % 60
  const h = Math.floor(t / 3600)
  const pad = (n, w = 2) => String(n).padStart(w, '0')
  return `${pad(h)}:${pad(m)}:${pad(s)},${pad(ms, 3)}`
}

function toPlainText(segs) {
  return segs.map(s => (s.speaker ? `${s.speaker}: ` : '') + s.text).join('\n')
}

function toSRT(segs) {
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

const storageKey = (path) => `transcript:${path}`

export default function Transcript({ trackPath, currentTime, playing, onSeek }) {
  const [segments, setSegments] = useState(() => {
    try {
      const v = JSON.parse(localStorage.getItem(storageKey(trackPath)) || '[]')
      return Array.isArray(v) ? v : []
    } catch { return [] }
  })
  const [follow, setFollow] = useState(true)
  const [copied, setCopied] = useState(false)
  const [paste, setPaste] = useState('')
  const [showExport, setShowExport] = useState(false)
  const rowRefs = useRef({})
  const focusId = useRef(null)

  // Persist per track
  useEffect(() => {
    try { localStorage.setItem(storageKey(trackPath), JSON.stringify(segments)) } catch { /* ignore */ }
  }, [segments, trackPath])

  // The active (currently-playing) timed segment
  const activeId = useMemo(() => {
    let id = null
    for (const s of segments) {
      if (s.start != null && s.start <= currentTime + 0.05) id = s.id
    }
    // segments aren't guaranteed sorted; take the latest start <= currentTime
    let best = null, bestStart = -1
    for (const s of segments) {
      if (s.start != null && s.start <= currentTime + 0.05 && s.start > bestStart) {
        bestStart = s.start; best = s.id
      }
    }
    return best ?? id
  }, [segments, currentTime])

  // Auto-scroll the active line into view while following playback
  useEffect(() => {
    if (!follow || !playing || !activeId) return
    rowRefs.current[activeId]?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  }, [activeId, follow, playing])

  // Focus a newly created row's text field
  useEffect(() => {
    if (focusId.current) {
      rowRefs.current[focusId.current]?.querySelector('textarea')?.focus()
      focusId.current = null
    }
  })

  const update = (id, patch) => setSegments(prev => prev.map(s => s.id === id ? { ...s, ...patch } : s))
  const remove = (id) => setSegments(prev => prev.filter(s => s.id !== id))

  const addLine = (afterId = null) => {
    const seg = { id: uid(), start: currentTime || null, speaker: '', text: '' }
    focusId.current = seg.id
    setSegments(prev => {
      if (afterId == null) return [...prev, seg]
      const i = prev.findIndex(s => s.id === afterId)
      const next = [...prev]
      next.splice(i + 1, 0, seg)
      return next
    })
  }

  const importFile = async () => {
    try {
      const sel = await open({ multiple: false, filters: [{ name: 'Transcript', extensions: ['srt', 'vtt', 'txt'] }] })
      if (!sel) return
      const path = typeof sel === 'string' ? sel : sel.path
      const text = await readTextFile(path)
      setSegments(parseTranscript(text, path.split('.').pop()?.toLowerCase()))
    } catch { /* cancelled or unreadable */ }
  }

  const usePaste = () => {
    if (!paste.trim()) return
    setSegments(parseTranscript(paste, 'txt'))
    setPaste('')
  }

  const exportFile = async (fmt) => {
    setShowExport(false)
    try {
      const content = fmt === 'srt' ? toSRT(segments) : toPlainText(segments)
      const path = await save({ defaultPath: `transcript.${fmt}`, filters: [{ name: fmt.toUpperCase(), extensions: [fmt] }] })
      if (path) await writeTextFile(path, content)
    } catch { /* cancelled */ }
  }

  const copyAll = async () => {
    try {
      await navigator.clipboard.writeText(toPlainText(segments))
      setCopied(true); setTimeout(() => setCopied(false), 1500)
    } catch { /* clipboard unavailable */ }
  }

  const hasTimes = segments.some(s => s.start != null)

  return (
    <Card>
      <CardHeader>
        <CardTitle>TRANSCRIPT</CardTitle>
        <div className="flex items-center gap-1.5">
          {segments.length > 0 && (
            <button
              onClick={() => setFollow(f => !f)}
              aria-pressed={follow}
              title="Auto-scroll to the line playing now"
              className={cn(
                'flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] font-mono transition-colors',
                follow ? 'bg-[hsl(var(--gold-dim))] text-primary' : 'text-[hsl(var(--sub))] hover:text-foreground'
              )}
            >
              <Locate size={11} /> Follow
            </button>
          )}
          <Button size="sm" variant="ghost" onClick={importFile} title="Import SRT, VTT, or TXT">
            <Upload size={12} /> Import
          </Button>
          {segments.length > 0 && (
            <>
              <button onClick={copyAll} title="Copy transcript to clipboard"
                className="flex items-center gap-1 px-1.5 py-1 rounded text-[11px] text-[hsl(var(--sub))] hover:text-foreground transition-colors">
                {copied ? <><Check size={12} /> Copied</> : <><Copy size={12} /> Copy</>}
              </button>
              <div className="relative">
                <Button size="sm" variant="ghost" onClick={() => setShowExport(s => !s)} title="Export transcript">
                  <Download size={12} /> Export
                </Button>
                {showExport && (
                  <div className="absolute right-0 top-full mt-1 z-10 bg-card border border-border rounded-md shadow-lg py-1 min-w-[110px]">
                    <button onClick={() => exportFile('txt')} className="block w-full text-left px-3 py-1.5 text-xs text-foreground hover:bg-secondary">Plain text (.txt)</button>
                    <button onClick={() => exportFile('srt')} disabled={!hasTimes}
                      className="block w-full text-left px-3 py-1.5 text-xs text-foreground hover:bg-secondary disabled:opacity-40 disabled:cursor-not-allowed"
                      title={hasTimes ? '' : 'Stamp at least one line with a time first'}>
                      Subtitles (.srt)
                    </button>
                  </div>
                )}
              </div>
            </>
          )}
        </div>
      </CardHeader>

      {segments.length === 0 ? (
        <div className="px-4 py-4 flex flex-col gap-3">
          <p className="text-[12px] text-[hsl(var(--sub))]">
            Import a transcript (SRT, VTT, or TXT) to proof against the audio, paste text below, or start typing a new one.
          </p>
          <textarea
            className="w-full h-24 bg-secondary/40 border border-border rounded-md p-2.5 text-[12px] text-foreground resize-y focus:outline-none focus:border-primary"
            placeholder="Paste transcript text here…"
            value={paste}
            onChange={e => setPaste(e.target.value)}
          />
          <div className="flex gap-2">
            <Button size="sm" variant="primary" onClick={usePaste} disabled={!paste.trim()}>Use pasted text</Button>
            <Button size="sm" variant="outline" onClick={() => addLine()}>
              <Plus size={12} /> Start typing
            </Button>
          </div>
        </div>
      ) : (
        <div className="flex flex-col">
          <div className="max-h-[420px] overflow-y-auto px-2 py-1.5">
            {segments.map((s) => {
              const active = s.id === activeId
              return (
                <div
                  key={s.id}
                  ref={el => { rowRefs.current[s.id] = el }}
                  className={cn(
                    'group flex items-start gap-2 px-2 py-1.5 rounded-md transition-colors',
                    active ? 'bg-[hsl(var(--gold-dim))] border-l-2 border-l-primary' : 'border-l-2 border-l-transparent hover:bg-secondary/40'
                  )}
                >
                  {/* Time / stamp */}
                  <div className="shrink-0 w-14 pt-0.5 flex items-center gap-0.5">
                    {s.start != null ? (
                      <button
                        onClick={() => onSeek(s.start)}
                        title="Jump to this point"
                        className="font-mono text-[10px] text-primary hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded"
                      >
                        {fmtTime(s.start)}
                      </button>
                    ) : (
                      <button
                        onClick={() => update(s.id, { start: currentTime })}
                        title="Stamp the current audio position onto this line"
                        className="text-[hsl(var(--sub))] hover:text-primary transition-colors"
                      >
                        <Crosshair size={12} />
                      </button>
                    )}
                  </div>

                  {/* Speaker (optional) */}
                  <input
                    className="shrink-0 w-16 bg-transparent border-none p-0 pt-0.5 text-[10px] font-mono text-[hsl(var(--text2))] focus:text-primary focus:outline-none placeholder:text-[hsl(var(--sub))]"
                    value={s.speaker}
                    placeholder="speaker"
                    onChange={e => update(s.id, { speaker: e.target.value })}
                  />

                  {/* Text */}
                  <textarea
                    rows={1}
                    className="flex-1 min-w-0 bg-transparent border-none p-0 text-[12px] leading-snug text-foreground resize-none focus:outline-none overflow-hidden"
                    value={s.text}
                    placeholder="…"
                    ref={el => { if (el) { el.style.height = 'auto'; el.style.height = el.scrollHeight + 'px' } }}
                    onChange={e => {
                      e.target.style.height = 'auto'
                      e.target.style.height = e.target.scrollHeight + 'px'
                      update(s.id, { text: e.target.value })
                    }}
                    onKeyDown={e => {
                      // Enter (no shift) starts a new line stamped at the current
                      // position — the from-scratch capture loop. Shift+Enter
                      // inserts a newline within the line.
                      if (e.key === 'Enter' && !e.shiftKey) {
                        e.preventDefault()
                        addLine(s.id)
                      }
                    }}
                  />

                  {/* Re-stamp + delete (on hover) */}
                  <div className="shrink-0 flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity pt-0.5">
                    <button
                      onClick={() => update(s.id, { start: currentTime })}
                      title="Set this line's time to the current position"
                      className="text-[hsl(var(--sub))] hover:text-primary transition-colors"
                    >
                      <Crosshair size={11} />
                    </button>
                    <button
                      onClick={() => remove(s.id)}
                      aria-label="Delete line"
                      className="text-[hsl(var(--sub))] hover:text-destructive transition-colors"
                    >
                      <X size={11} />
                    </button>
                  </div>
                </div>
              )
            })}
          </div>
          <div className="px-3 py-2 border-t border-border/60">
            <Button size="sm" variant="ghost" onClick={() => addLine()}>
              <Plus size={12} /> Add line {currentTime > 0 && <span className="font-mono text-[10px] text-[hsl(var(--sub))] ml-1">@ {fmtTime(currentTime)}</span>}
            </Button>
          </div>
        </div>
      )}
    </Card>
  )
}
