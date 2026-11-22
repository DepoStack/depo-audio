import { useState, useRef, useEffect, useMemo, useId } from 'react'
import { open, save } from '@tauri-apps/plugin-dialog'
import { readTextFile, writeTextFile } from '@tauri-apps/plugin-fs'
import { Upload, Copy, Check, Flag, Locate, Trash2, Download, X } from 'lucide-react'
import { cn } from '../../lib/utils'
import { basename } from '../../utils'
import {
  DEFAULT_SYNC,
  describeDetection,
  detectTimeKind,
  findActiveIndex,
  flagRows,
  flagsToCsv,
  flagsToText,
  fmtClock,
  fmtElapsed,
  linePositions,
  MAX_FLAG_NOTE_CHARS,
  normalizeSync,
  parseClockInput,
  parseTranscript,
  readStoredSegments,
  readStoredSync,
  removeStoredSegments,
  saveStoredSegments,
  saveStoredSync,
  setFlagNote,
  shiftForLineAt,
  toggleFlag,
} from '../../lib/transcript'
import { Card, CardHeader, CardTitle } from '../ui/card'
import { Button } from '../ui/button'
import { ConfirmDialog } from '../ui/confirm-dialog'

// ── Transcript proofing ─────────────────────────────────────────────────────
//
// Proof an existing transcript against the audio. DepoAudio never writes or
// edits transcript text: that is done in CAT software.
//   • Open SRT, WebVTT or timestamped text (CAT ASCII/TXT exports).
//   • The line at the playhead is highlighted and followed ("Follow audio").
//   • Click a line, or press Enter on it, to play from its time. Arrow keys,
//     Home, End, Page Up and Page Down move between lines.
//   • Sync shift ("Transcript is ahead by 1.5 s") and, for time-of-day
//     stamps, the recording's start time. Both change only when the person
//     changes them, and are saved per track.
//   • F flags a line for review with an optional note. Flags never change
//     the text, and can be saved as CSV or text.
//
// Lines and flags persist per track in localStorage ("transcript:<path>", the
// key earlier versions used, so their saved transcripts open here read-only);
// sync settings use "transcript-sync:<path>". "Remove this transcript from
// DepoAudio" deletes only those stored copies (DA-07).

const STORAGE_WARNING =
  'Saving is unavailable on this computer. Flags and sync changes are kept only while DepoAudio is open; export your flags before closing it.'
const volatileState = new Map()

function browserStorage() {
  try {
    return window.localStorage
  } catch {
    return null
  }
}

function loadTrackState(path) {
  const kept = volatileState.get(path)
  if (kept) return { ...kept, path }
  const storage = browserStorage()
  const stored = readStoredSegments(storage, path)
  // An empty or unreadable entry keeps no key (DA-07: the key names the track).
  if (stored.storageAvailable && stored.segments.length === 0) removeStoredSegments(storage, path)
  const storedSync = readStoredSync(storage, path)
  return {
    path,
    segments: stored.segments,
    sync: storedSync ?? { ...DEFAULT_SYNC, timeKind: detectTimeKind(stored.segments) ?? 'elapsed' },
    detection: null,
    storageError: stored.storageAvailable ? '' : STORAGE_WARNING,
  }
}

const PAGE_LINES = 8

export default function Transcript({ trackPath, currentTime, playing, duration, onSeek, onPlayFrom, onStorageError }) {
  const [state, setState] = useState(() => loadTrackState(trackPath))
  if (state.path !== trackPath) setState(loadTrackState(trackPath))
  const { segments, sync, detection, storageError } = state.path === trackPath ? state : loadTrackState(trackPath)

  const [follow, setFollow] = useState(true)
  const [copied, setCopied] = useState(false)
  const [message, setMessage] = useState('')
  const [operationError, setOperationError] = useState('')
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [pendingImport, setPendingImport] = useState(null)
  const [focusIndex, setFocusIndex] = useState(0)
  const [startDraft, setStartDraft] = useState('')
  const [startError, setStartError] = useState('')
  const listRef = useRef(null)
  const startId = useId()
  const rowRefs = useRef([])

  useEffect(() => {
    onStorageError?.(storageError)
  }, [onStorageError, storageError])

  // Every change is written at once. When storage fails, the state is kept in
  // memory for this session (also across track switches) and the person is told.
  const commit = patch => {
    const next = { segments, sync, detection, ...patch, path: trackPath }
    const storage = browserStorage()
    const saved =
      saveStoredSegments(storage, trackPath, next.segments) &&
      (next.segments.length === 0 || saveStoredSync(storage, trackPath, next.sync))
    next.storageError = saved ? '' : STORAGE_WARNING
    if (saved) volatileState.delete(trackPath)
    else volatileState.set(trackPath, next)
    setState(next)
  }

  const setSync = patch => commit({ sync: normalizeSync({ ...sync, ...patch }) })

  const positions = useMemo(() => linePositions(segments, sync), [segments, sync])
  const activeIndex = useMemo(() => findActiveIndex(positions, currentTime), [positions, currentTime])
  const flags = useMemo(() => flagRows(segments, sync), [segments, sync])
  const timedCount = positions.filter(p => p.own != null).length
  const needsStart = sync.timeKind === 'time-of-day' && sync.recordingStart == null
  const hasTimes = segments.some(s => s.start != null)
  const safeFocus = Math.min(focusIndex, Math.max(0, segments.length - 1))

  // Follow the line at the playhead inside the list only (the page never scrolls).
  useEffect(() => {
    if (!follow || activeIndex < 0) return
    const list = listRef.current
    const row = rowRefs.current[activeIndex]
    if (!list || !row) return
    const top = row.offsetTop - list.offsetTop
    if (top < list.scrollTop || top + row.offsetHeight > list.scrollTop + list.clientHeight) {
      list.scrollTop = Math.max(0, top - list.clientHeight / 3)
    }
  }, [activeIndex, follow, playing])

  const focusRow = index => {
    const next = Math.max(0, Math.min(segments.length - 1, index))
    setFocusIndex(next)
    rowRefs.current[next]?.focus({ preventScroll: false })
  }

  const playLine = index => {
    const at = positions[index]?.playFrom
    setFocusIndex(index)
    if (at == null) {
      setMessage(
        needsStart
          ? 'Enter the time the recording started to play from a line.'
          : 'This line has no time, so it cannot be played from.',
      )
      return
    }
    setMessage('')
    ;(onPlayFrom ?? onSeek)?.(at)
  }

  const flagLine = index => {
    const line = segments[index]
    if (!line) return
    commit({ segments: toggleFlag(segments, line.id) })
    setMessage(line.flag ? `Flag removed from line ${index + 1}.` : `Line ${index + 1} flagged for review.`)
  }

  const onRowKey = (event, index) => {
    if (event.metaKey || event.ctrlKey || event.altKey) return
    const moves = {
      ArrowDown: index + 1,
      ArrowUp: index - 1,
      Home: 0,
      End: segments.length - 1,
      PageDown: index + PAGE_LINES,
      PageUp: index - PAGE_LINES,
    }
    if (event.key in moves) {
      event.preventDefault()
      event.stopPropagation()
      focusRow(moves[event.key])
    } else if (event.key === 'Enter') {
      event.preventDefault()
      event.stopPropagation()
      playLine(index)
    } else if (event.key === 'f' || event.key === 'F') {
      event.preventDefault()
      event.stopPropagation()
      flagLine(index)
    }
  }

  // ── Import ────────────────────────────────────────────────────────────────

  const applyImport = ({ parsed, name }) => {
    setPendingImport(null)
    commit({
      segments: parsed.segments,
      sync: { ...DEFAULT_SYNC, timeKind: parsed.timeKind ?? 'elapsed' },
      detection: { ...parsed, name },
    })
    setFocusIndex(0)
    setStartDraft('')
    setMessage('')
  }

  const importFile = async () => {
    setOperationError('')
    try {
      const sel = await open({
        multiple: false,
        filters: [{ name: 'Transcript', extensions: ['srt', 'vtt', 'txt', 'asc', 'ascii'] }],
      })
      if (!sel) return
      const path = typeof sel === 'string' ? sel : sel.path
      const text = await readTextFile(path)
      const parsed = parseTranscript(text, path.split('.').pop(), { duration })
      if (parsed.segments.length === 0) {
        setOperationError(`No transcript lines were found in ${basename(path)}. Nothing was changed.`)
        return
      }
      const next = { parsed, name: basename(path) }
      if (segments.length > 0) setPendingImport(next)
      else applyImport(next)
    } catch (error) {
      setOperationError(`The transcript could not be opened: ${String(error)}`)
    }
  }

  // ── Sync ──────────────────────────────────────────────────────────────────

  const nudge = delta => setSync({ shift: Math.round((sync.shift + delta) * 10) / 10 })

  const setLineToNow = () => {
    const line = segments[safeFocus]
    const shift = shiftForLineAt(line?.start, currentTime, sync)
    if (shift == null) {
      setMessage('Choose a line that has a time, then press "Set this line to now".')
      return
    }
    setSync({ shift })
    setMessage(`Line ${safeFocus + 1} now plays from ${fmtElapsed(currentTime)}.`)
  }

  const saveStart = event => {
    event.preventDefault()
    const value = parseClockInput(startDraft)
    if (value == null) {
      setStartError('Type the start time as hours, minutes and seconds, for example 09:30:00 or 2:05:10 PM.')
      return
    }
    setStartError('')
    setStartDraft('')
    setSync({ recordingStart: value })
  }

  // ── Flags export, copy, remove ────────────────────────────────────────────

  const exportFlags = async () => {
    setOperationError('')
    const stem = basename(trackPath).replace(/\.[^.]+$/, '') || 'recording'
    try {
      const path = await save({
        defaultPath: `${stem} proofing flags.csv`,
        filters: [
          { name: 'CSV', extensions: ['csv'] },
          { name: 'Text', extensions: ['txt'] },
        ],
      })
      if (!path) return
      const content = /\.txt$/i.test(path) ? flagsToText(flags, basename(trackPath)) : flagsToCsv(flags)
      await writeTextFile(path, content)
      setMessage(`${flags.length} ${flags.length === 1 ? 'flag' : 'flags'} saved to ${basename(path)}.`)
    } catch (error) {
      setOperationError(`The flags could not be saved: ${String(error)}`)
    }
  }

  const copyText = async () => {
    setOperationError('')
    try {
      await navigator.clipboard.writeText(segments.map(s => (s.speaker ? `${s.speaker}: ` : '') + s.text).join('\n'))
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch (error) {
      setOperationError(`Could not copy the transcript: ${String(error)}`)
    }
  }

  const deleteTranscript = () => {
    setOperationError('')
    volatileState.delete(trackPath)
    const removed = removeStoredSegments(browserStorage(), trackPath)
    setState({
      path: trackPath,
      segments: [],
      sync: { ...DEFAULT_SYNC },
      detection: null,
      storageError: removed ? '' : STORAGE_WARNING,
    })
    if (!removed) {
      setOperationError(
        "DepoAudio's stored copy could not be removed because this app's storage is unavailable. It is cleared on screen; try again after restarting DepoAudio.",
      )
    }
  }

  const shiftLabel =
    sync.shift === 0
      ? 'No sync shift'
      : `Transcript is ${sync.shift > 0 ? 'ahead' : 'behind'} by ${Math.abs(sync.shift).toFixed(1)} s`
  const shiftTitle =
    sync.shift === 0
      ? 'Lines play from the times in the transcript.'
      : `Each line plays ${Math.abs(sync.shift).toFixed(1)} s ${sync.shift > 0 ? 'later' : 'earlier'} than its time in the transcript.`

  const pill = on =>
    cn(
      'inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[12px] transition-colors',
      on
        ? 'bg-secondary text-foreground ring-1 ring-inset ring-foreground'
        : 'text-[hsl(var(--sub))] hover:text-foreground',
    )
  const smallButton =
    'inline-flex items-center gap-1 rounded border border-border px-1.5 py-0.5 text-[12px] tabular-nums text-foreground hover:bg-secondary disabled:opacity-45'

  return (
    <Card>
      <CardHeader className="py-2">
        <CardTitle>Transcript proofing</CardTitle>
        <div className="flex items-center gap-1.5">
          {segments.length > 0 && hasTimes && (
            <button
              onClick={() => setFollow(f => !f)}
              aria-pressed={follow}
              title="Keep the line playing now in view"
              className={pill(follow)}
            >
              <Locate size={11} /> Follow audio
            </button>
          )}
          <Button size="sm" variant="ghost" onClick={importFile} title="Open an SRT, WebVTT or timestamped text file">
            <Upload size={12} /> {segments.length > 0 ? 'Open another' : 'Open transcript'}
          </Button>
          {segments.length > 0 && (
            <>
              <Button size="sm" variant="ghost" onClick={copyText} title="Copy the transcript text to the clipboard">
                {copied ? <Check size={12} /> : <Copy size={12} />} {copied ? 'Copied' : 'Copy text'}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                className="hover:text-destructive"
                onClick={() => setConfirmDelete(true)}
                aria-label="Remove this transcript from DepoAudio"
                title="Remove this transcript from DepoAudio"
              >
                <Trash2 size={12} />
              </Button>
            </>
          )}
        </div>
      </CardHeader>

      <span role="status" aria-live="polite" className="sr-only">
        {copied ? 'Transcript text copied to clipboard.' : message}
      </span>

      <ConfirmDialog
        open={confirmDelete}
        onOpenChange={setConfirmDelete}
        title="Remove this transcript from DepoAudio"
        description="This removes the copy of the transcript DepoAudio keeps for this track, with its flags and sync settings, on this computer. It cannot be undone. The transcript file you opened and the recording are not affected. Export your flags first if you want them."
        confirmLabel="Remove transcript"
        initialFocus="cancel"
        onConfirm={deleteTranscript}
      />

      <ConfirmDialog
        open={pendingImport != null}
        onOpenChange={openState => !openState && setPendingImport(null)}
        title="Replace the transcript for this track"
        description={`DepoAudio keeps ${segments.length} ${segments.length === 1 ? 'line' : 'lines'}${flags.length ? ` and ${flags.length} ${flags.length === 1 ? 'flag' : 'flags'}` : ''} for this track. Opening ${pendingImport?.name ?? 'the new file'} replaces them, and resets the sync settings. Copy the text or export the flags first if you need them.`}
        confirmLabel="Replace"
        initialFocus="cancel"
        onConfirm={() => pendingImport && applyImport(pendingImport)}
      />

      {storageError && !onStorageError && (
        <p
          role="alert"
          className="mx-3 mt-2 rounded-md border border-warning/40 bg-warning/10 px-3 py-1.5 type-small text-foreground"
        >
          {storageError}
        </p>
      )}

      {operationError && (
        <p
          role="alert"
          className="mx-3 mt-2 rounded-md border border-destructive/30 border-l-[3px] border-l-destructive px-3 py-1.5 type-small text-destructive"
        >
          {operationError}
        </p>
      )}

      {segments.length === 0 ? (
        <div className="px-4 py-3">
          <p className="text-[13px] font-medium text-foreground">Proof a transcript against this recording</p>
          <p className="mt-1 type-small leading-relaxed text-[hsl(var(--sub))]">
            Open an SRT, WebVTT or text file with time stamps (for example an ASCII export from your CAT software). The
            line being spoken is highlighted as the audio plays, and you can flag lines to check. DepoAudio does not
            change the transcript; make corrections in your CAT software.
          </p>
        </div>
      ) : (
        <div className="flex flex-col">
          {/* Detection + sync */}
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 border-b border-border/60 px-3 py-1 text-[12px] text-[hsl(var(--text2))]">
            <span className="min-w-0">
              {detection
                ? `${detection.name}: ${describeDetection(detection)}`
                : `Saved transcript: ${describeDetection({
                    format: 'text',
                    timedCount: segments.filter(s => s.start != null).length,
                    untimedCount: segments.filter(s => s.start == null).length,
                    timeKind: sync.timeKind,
                  })}`}
            </span>
            {hasTimes && (
              <span className="inline-flex items-center gap-1" role="group" aria-label="What the times mean">
                <span>Times are</span>
                <button
                  className={pill(sync.timeKind === 'elapsed')}
                  aria-pressed={sync.timeKind === 'elapsed'}
                  onClick={() => setSync({ timeKind: 'elapsed' })}
                >
                  time into recording
                </button>
                <button
                  className={pill(sync.timeKind === 'time-of-day')}
                  aria-pressed={sync.timeKind === 'time-of-day'}
                  onClick={() => setSync({ timeKind: 'time-of-day' })}
                >
                  time of day
                </button>
              </span>
            )}
          </div>

          {sync.timeKind === 'time-of-day' && hasTimes && (
            <form
              onSubmit={saveStart}
              className="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-border/60 px-3 py-1 text-[12px] text-[hsl(var(--text2))]"
            >
              <label htmlFor={startId}>Recording started at</label>
              {sync.recordingStart != null && !startDraft && (
                <span className="tabular-nums font-medium text-foreground">{fmtClock(sync.recordingStart)}</span>
              )}
              <input
                id={startId}
                className="h-6 w-28 rounded border border-border bg-transparent px-1.5 tabular-nums text-foreground placeholder:text-[hsl(var(--sub))]"
                placeholder={sync.recordingStart != null ? 'change' : 'HH:MM:SS'}
                value={startDraft}
                onChange={e => setStartDraft(e.target.value)}
                title="The time on the recording's clock when it started. It is used to match time-of-day stamps to the audio."
                aria-describedby={`${startId}-help`}
                aria-invalid={startError ? true : undefined}
              />
              <button type="submit" className={smallButton} disabled={!startDraft.trim()}>
                Save
              </button>
              {needsStart && segments.find(s => s.start != null) && (
                <button
                  type="button"
                  className={smallButton}
                  onClick={() => setSync({ recordingStart: segments.find(s => s.start != null).start })}
                >
                  Use first line&apos;s time ({fmtClock(segments.find(s => s.start != null).start)})
                </button>
              )}
              <span id={`${startId}-help`} className={startError ? 'text-destructive' : ''}>
                {startError ||
                  (needsStart ? 'DepoAudio does not know when this recording started.' : 'Entered by you.')}
              </span>
            </form>
          )}

          {timedCount > 0 && (
            <div
              className="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-border/60 px-3 py-1 text-[12px] text-[hsl(var(--text2))]"
              role="group"
              aria-label="Sync"
            >
              <span className="tabular-nums font-medium text-foreground" title={shiftTitle}>
                {shiftLabel}
              </span>
              <button className={smallButton} onClick={() => nudge(-0.5)} aria-label="Move lines 0.5 seconds earlier">
                −0.5 s
              </button>
              <button className={smallButton} onClick={() => nudge(0.5)} aria-label="Move lines 0.5 seconds later">
                +0.5 s
              </button>
              <button
                className={smallButton}
                onClick={setLineToNow}
                title={`Make line ${safeFocus + 1} play from the current audio position`}
              >
                Set this line to now
              </button>
              {sync.shift !== 0 && (
                <button className={smallButton} onClick={() => setSync({ shift: 0 })}>
                  Clear shift
                </button>
              )}
            </div>
          )}

          {/* Lines (read-only) */}
          <div
            ref={listRef}
            role="listbox"
            aria-label="Transcript lines. Enter plays from a line, F flags it."
            className="max-h-[124px] overflow-y-auto px-1.5 py-1"
          >
            {segments.map((s, index) => {
              const active = index === activeIndex
              const at = positions[index]?.own
              return (
                <div
                  key={s.id}
                  ref={el => {
                    rowRefs.current[index] = el
                  }}
                  role="option"
                  aria-selected={index === safeFocus}
                  aria-current={active ? 'true' : undefined}
                  tabIndex={index === safeFocus ? 0 : -1}
                  onClick={() => playLine(index)}
                  onFocus={() => setFocusIndex(index)}
                  onKeyDown={e => onRowKey(e, index)}
                  title={at == null ? undefined : `Plays from ${fmtElapsed(at)} in the audio`}
                  className={cn(
                    'group flex cursor-pointer items-start gap-2 rounded px-1.5 py-0.5 text-[12px] leading-snug focus-visible:outline focus-visible:outline-2 focus-visible:outline-foreground aria-selected:ring-1 aria-selected:ring-inset aria-selected:ring-[hsl(var(--sub))]',
                    active
                      ? 'bg-secondary border-l-2 border-l-foreground'
                      : 'border-l-2 border-l-transparent hover:bg-secondary/40',
                  )}
                >
                  <span className="w-16 shrink-0 tabular-nums text-[hsl(var(--text2))]">
                    {s.start == null ? '' : sync.timeKind === 'time-of-day' ? fmtClock(s.start) : fmtElapsed(s.start)}
                  </span>
                  {s.speaker && <span className="shrink-0 font-medium text-[hsl(var(--text2))]">{s.speaker}:</span>}
                  <span className="min-w-0 flex-1 whitespace-pre-wrap break-words text-foreground">{s.text}</span>
                  <button
                    tabIndex={-1}
                    onClick={e => {
                      e.stopPropagation()
                      flagLine(index)
                    }}
                    aria-label={s.flag ? `Remove flag from line ${index + 1}` : `Flag line ${index + 1} for review`}
                    title={s.flag ? 'Remove flag (F)' : 'Flag for review (F)'}
                    className={cn(
                      'shrink-0 rounded transition-opacity',
                      s.flag
                        ? 'text-foreground'
                        : 'text-[hsl(var(--sub))] opacity-0 group-hover:opacity-100 group-focus:opacity-100 hover:text-foreground',
                    )}
                  >
                    <Flag size={11} fill={s.flag ? 'currentColor' : 'none'} />
                  </button>
                </div>
              )
            })}
          </div>

          {/* Flags */}
          <div className="border-t border-border/60 px-3 py-1">
            <div className="flex items-center justify-between gap-2">
              <h4 className="text-[12px] font-medium text-[hsl(var(--text2))]">
                Flags for review{flags.length ? ` (${flags.length})` : ''}
              </h4>
              {flags.length > 0 && (
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-6"
                  onClick={exportFlags}
                  title="Save flags as CSV or text"
                >
                  <Download size={12} /> Export flags
                </Button>
              )}
            </div>
            {flags.length === 0 ? (
              <p className="type-small text-[hsl(var(--sub))]">
                Press <kbd className="tabular-nums">F</kbd> on a line, or its flag button, to mark it for review. Flags
                never change the transcript.
              </p>
            ) : (
              <ul className="mt-0.5 flex max-h-[44px] flex-col gap-0.5 overflow-y-auto">
                {flags.map(f => (
                  <li key={f.id} className="group flex items-center gap-2 text-[12px]">
                    <button
                      className="w-14 shrink-0 text-left tabular-nums text-[hsl(var(--text2))] hover:text-foreground"
                      onClick={() => playLine(f.line - 1)}
                      title={`Play from line ${f.line}`}
                    >
                      {f.audioTime == null ? `line ${f.line}` : fmtElapsed(f.audioTime)}
                    </button>
                    <span className="w-2/5 min-w-0 shrink truncate text-foreground" title={f.excerpt}>
                      {f.excerpt}
                    </span>
                    <input
                      className="min-w-0 flex-1 border-none bg-transparent p-0 text-[12px] text-foreground placeholder:text-[hsl(var(--sub))]"
                      aria-label={`Note for flag on line ${f.line}`}
                      placeholder="Add a note…"
                      maxLength={MAX_FLAG_NOTE_CHARS}
                      value={f.note}
                      onChange={e => commit({ segments: setFlagNote(segments, f.id, e.target.value) })}
                    />
                    <button
                      className="shrink-0 rounded text-[hsl(var(--sub))] opacity-0 transition-opacity hover:text-destructive group-hover:opacity-100 group-focus-within:opacity-100"
                      aria-label={`Remove flag from line ${f.line}`}
                      onClick={() => flagLine(f.line - 1)}
                    >
                      <X size={10} />
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      )}
    </Card>
  )
}
