import { useState, useRef, useEffect, useMemo } from 'react'
import { open, save } from '@tauri-apps/plugin-dialog'
import { readTextFile, writeTextFile } from '@tauri-apps/plugin-fs'
import { Upload, Download, Copy, Check, Crosshair, Plus, X, Locate } from 'lucide-react'
import { cn } from '../../lib/utils'
import { fmtTime } from '../../utils'
import {
  createSegmentAtTime,
  parseTranscript,
  toPlainText,
  toSRT,
  canExportSrt,
  readStoredSegments,
  saveStoredSegments,
} from '../../lib/transcript'
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
// Parsing/serialization lives in lib/transcript.js (characterization-tested).
// Transcripts persist per track in localStorage, keyed by file path.

const STORAGE_WARNING =
  'Transcript autosave is unavailable. Changes are kept only in this open session; export them before leaving this track.'

function browserStorage() {
  try {
    return window.localStorage
  } catch {
    return null
  }
}

function loadTrackState(path) {
  const stored = readStoredSegments(browserStorage(), path)
  return {
    path,
    segments: stored.segments,
    storageError: stored.storageAvailable ? '' : STORAGE_WARNING,
  }
}

export default function Transcript({ trackPath, currentTime, playing, onSeek }) {
  const [trackState, setTrackState] = useState(() => loadTrackState(trackPath))
  if (trackState.path !== trackPath) {
    setTrackState(loadTrackState(trackPath))
  }
  const segments = useMemo(() => (trackState.path === trackPath ? trackState.segments : []), [trackState, trackPath])
  const storageError = trackState.path === trackPath ? trackState.storageError : ''
  const setSegments = update =>
    setTrackState(previous => {
      const currentState = previous.path === trackPath ? previous : loadTrackState(trackPath)
      return {
        ...currentState,
        path: trackPath,
        segments: typeof update === 'function' ? update(currentState.segments) : update,
      }
    })
  const [follow, setFollow] = useState(true)
  const [copied, setCopied] = useState(false)
  const [paste, setPaste] = useState('')
  const [showExport, setShowExport] = useState(false)
  const [operationError, setOperationError] = useState('')
  const rowRefs = useRef({})
  const focusId = useRef(null)

  // Persist per track
  useEffect(() => {
    if (trackState.path === trackPath) {
      const saved = saveStoredSegments(browserStorage(), trackState.path, trackState.segments)
      const nextError = saved ? '' : STORAGE_WARNING
      const statusTimer = window.setTimeout(() => {
        setTrackState(previous =>
          previous.path === trackState.path && previous.storageError !== nextError
            ? { ...previous, storageError: nextError }
            : previous,
        )
      }, 0)
      return () => window.clearTimeout(statusTimer)
    }
    return undefined
  }, [trackState.path, trackState.segments, trackPath])

  // The active (currently-playing) timed segment
  const activeId = useMemo(() => {
    let id = null
    for (const s of segments) {
      if (s.start != null && s.start <= currentTime + 0.05) id = s.id
    }
    // segments aren't guaranteed sorted; take the latest start <= currentTime
    let best = null,
      bestStart = -1
    for (const s of segments) {
      if (s.start != null && s.start <= currentTime + 0.05 && s.start > bestStart) {
        bestStart = s.start
        best = s.id
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

  const update = (id, patch) => setSegments(prev => prev.map(s => (s.id === id ? { ...s, ...patch } : s)))
  const remove = id => setSegments(prev => prev.filter(s => s.id !== id))

  const addLine = (afterId = null) => {
    const seg = createSegmentAtTime(currentTime)
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
    setOperationError('')
    try {
      const sel = await open({ multiple: false, filters: [{ name: 'Transcript', extensions: ['srt', 'vtt', 'txt'] }] })
      if (!sel) return
      const path = typeof sel === 'string' ? sel : sel.path
      const text = await readTextFile(path)
      setSegments(parseTranscript(text, path.split('.').pop()?.toLowerCase()))
    } catch (error) {
      setOperationError(`Transcript import failed: ${String(error)}`)
    }
  }

  const usePaste = () => {
    if (!paste.trim()) return
    setSegments(parseTranscript(paste, 'txt'))
    setPaste('')
  }

  const exportFile = async fmt => {
    if (fmt === 'srt' && !canExportSrt(segments)) return
    setShowExport(false)
    setOperationError('')
    try {
      const content = fmt === 'srt' ? toSRT(segments) : toPlainText(segments)
      const path = await save({
        defaultPath: `transcript.${fmt}`,
        filters: [{ name: fmt.toUpperCase(), extensions: [fmt] }],
      })
      if (path) await writeTextFile(path, content)
    } catch (error) {
      setOperationError(`Transcript export failed: ${String(error)}`)
    }
  }

  const copyAll = async () => {
    setOperationError('')
    try {
      await navigator.clipboard.writeText(toPlainText(segments))
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch (error) {
      setOperationError(`Could not copy the transcript: ${String(error)}`)
    }
  }

  const srtReady = canExportSrt(segments)

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
                follow ? 'bg-[hsl(var(--gold-dim))] text-foreground' : 'text-[hsl(var(--sub))] hover:text-foreground',
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
              <button
                onClick={copyAll}
                title="Copy transcript to clipboard"
                className="flex items-center gap-1 px-1.5 py-1 rounded text-[11px] text-[hsl(var(--sub))] hover:text-foreground transition-colors"
              >
                {copied ? (
                  <>
                    <Check size={12} /> Copied
                  </>
                ) : (
                  <>
                    <Copy size={12} /> Copy
                  </>
                )}
              </button>
              <div className="relative">
                <Button size="sm" variant="ghost" onClick={() => setShowExport(s => !s)} title="Export transcript">
                  <Download size={12} /> Export
                </Button>
                {showExport && (
                  <div className="absolute right-0 top-full mt-1 z-10 bg-card border border-border rounded-md shadow-lg py-1 min-w-[110px]">
                    <button
                      onClick={() => exportFile('txt')}
                      className="block w-full text-left px-3 py-1.5 text-xs text-foreground hover:bg-secondary"
                    >
                      Plain text (.txt)
                    </button>
                    <button
                      onClick={() => exportFile('srt')}
                      disabled={!srtReady}
                      className="block w-full text-left px-3 py-1.5 text-xs text-foreground hover:bg-secondary disabled:opacity-40 disabled:cursor-not-allowed"
                      title={srtReady ? '' : 'Stamp every transcript line with a time before exporting SRT'}
                    >
                      Subtitles (.srt)
                    </button>
                    {!srtReady && (
                      <p className="max-w-40 px-3 py-1 text-[9px] leading-snug text-[hsl(var(--sub))]">
                        Stamp every line before SRT export.
                      </p>
                    )}
                  </div>
                )}
              </div>
            </>
          )}
        </div>
      </CardHeader>

      {storageError && (
        <p
          role="alert"
          className="mx-4 mt-3 rounded-md border border-warning/40 bg-warning/10 px-3 py-2 text-[11px] text-foreground"
        >
          {storageError}
        </p>
      )}

      {operationError && (
        <p
          role="alert"
          className="mx-4 mt-3 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-[11px] text-destructive"
        >
          {operationError}
        </p>
      )}

      {segments.length === 0 ? (
        <div className="px-4 py-4 flex flex-col gap-3">
          <p className="text-[12px] text-[hsl(var(--sub))]">
            Import a transcript (SRT, VTT, or TXT) to proof against the audio, paste text below, or start typing a new
            one.
          </p>
          <textarea
            className="w-full h-24 bg-secondary/40 border border-border rounded-md p-2.5 text-[12px] text-foreground resize-y focus:outline-hidden focus:border-primary"
            placeholder="Paste transcript text here…"
            value={paste}
            onChange={e => setPaste(e.target.value)}
          />
          <div className="flex gap-2">
            <Button size="sm" variant="primary" onClick={usePaste} disabled={!paste.trim()}>
              Use pasted text
            </Button>
            <Button size="sm" variant="outline" onClick={() => addLine()}>
              <Plus size={12} /> Start typing
            </Button>
          </div>
        </div>
      ) : (
        <div className="flex flex-col">
          <div className="max-h-[420px] overflow-y-auto px-2 py-1.5">
            {segments.map(s => {
              const active = s.id === activeId
              return (
                <div
                  key={s.id}
                  ref={el => {
                    rowRefs.current[s.id] = el
                  }}
                  className={cn(
                    'group flex items-start gap-2 px-2 py-1.5 rounded-md transition-colors',
                    active
                      ? 'bg-[hsl(var(--gold-dim))] border-l-2 border-l-primary'
                      : 'border-l-2 border-l-transparent hover:bg-secondary/40',
                  )}
                >
                  {/* Time / stamp */}
                  <div className="shrink-0 w-14 pt-0.5 flex items-center gap-0.5">
                    {s.start != null ? (
                      <button
                        onClick={() => onSeek(s.start)}
                        title="Jump to this point"
                        className="font-mono text-[10px] text-foreground hover:underline focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring rounded"
                      >
                        {fmtTime(s.start)}
                      </button>
                    ) : (
                      <button
                        onClick={() => update(s.id, { start: currentTime })}
                        title="Stamp the current audio position onto this line"
                        className="text-[hsl(var(--sub))] hover:text-foreground transition-colors"
                      >
                        <Crosshair size={12} />
                      </button>
                    )}
                  </div>

                  {/* Speaker (optional) */}
                  <input
                    className="shrink-0 w-16 bg-transparent border-none p-0 pt-0.5 text-[10px] font-mono text-[hsl(var(--text2))] focus:text-foreground focus:outline-hidden placeholder:text-[hsl(var(--sub))]"
                    value={s.speaker}
                    placeholder="speaker"
                    onChange={e => update(s.id, { speaker: e.target.value })}
                  />

                  {/* Text */}
                  <textarea
                    rows={1}
                    className="flex-1 min-w-0 bg-transparent border-none p-0 text-[12px] leading-snug text-foreground resize-none focus:outline-hidden overflow-hidden"
                    value={s.text}
                    placeholder="…"
                    ref={el => {
                      if (el) {
                        el.style.height = 'auto'
                        el.style.height = el.scrollHeight + 'px'
                      }
                    }}
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
                  <div className="shrink-0 flex items-center gap-1 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity pt-0.5">
                    <button
                      onClick={() => update(s.id, { start: currentTime })}
                      aria-label="Set this line's time to the current position"
                      title="Set this line's time to the current position"
                      className="rounded text-[hsl(var(--sub))] hover:text-foreground transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
                    >
                      <Crosshair size={11} />
                    </button>
                    <button
                      onClick={() => remove(s.id)}
                      aria-label="Delete line"
                      className="rounded text-[hsl(var(--sub))] hover:text-destructive transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
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
              <Plus size={12} /> Add line{' '}
              {Number.isFinite(currentTime) && currentTime >= 0 && (
                <span className="font-mono text-[10px] text-[hsl(var(--sub))] ml-1">@ {fmtTime(currentTime)}</span>
              )}
            </Button>
          </div>
        </div>
      )}
    </Card>
  )
}
