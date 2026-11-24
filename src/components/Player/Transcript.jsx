import { useState, useRef, useEffect, useLayoutEffect, useMemo, useId } from 'react'
import { createPortal } from 'react-dom'
import { open, save } from '@tauri-apps/plugin-dialog'
import { readTextFile, writeTextFile } from '@tauri-apps/plugin-fs'
import { Upload, Copy, Check, Flag, Locate, Trash2, Download, X, ChevronDown, AlertTriangle } from 'lucide-react'
import { cn } from '../../lib/utils'
import { basename } from '../../utils'
import {
  DEFAULT_SYNC,
  describeDetection,
  findActiveIndex,
  flagRows,
  flagsToCsv,
  flagsToText,
  fmtClock,
  fmtElapsed,
  lineName,
  linePositions,
  lineRef,
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
  transcriptText,
} from '../../lib/transcript'
import { Card } from '../ui/card'
import { Button } from '../ui/button'
import { ConfirmDialog } from '../ui/confirm-dialog'

// ── Transcript proofing ─────────────────────────────────────────────────────
//
// Proof an existing transcript against the audio. DepoAudio never writes or
// edits transcript text: that is done in CAT software.
//   • Open SRT, WebVTT or timestamped text (CAT ASCII/TXT exports).
//   • The line at the playhead is highlighted and followed ("Follow audio").
//     While following, a keyboard focus on a line moves with the playing
//     line, so F flags the line being heard. Moving through the lines pauses
//     following (and says so) until playback starts again, a line is played,
//     or "." / "Back to playing line" is pressed.
//   • Click a line, or press Enter on it, to play from its time. Arrow keys,
//     Home, End, Page Up and Page Down move between lines.
//   • Sync shift ("Transcript is ahead by 1.5 s") and, for time-of-day
//     stamps, the recording's start time. Both change only when the person
//     changes them, and are saved per track.
//   • F flags a line for review with an optional note. Flags never change
//     the text, and can be saved as CSV or text with the page and line
//     number from the file.
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
  // No sync key means the transcript was saved by an earlier version (always
  // elapsed times) or restored from a backup, which carries no sync settings.
  // Either way the times are read as time into the recording, not guessed;
  // the person can change it.
  const storedSync = readStoredSync(storage, path)
  return {
    path,
    segments: stored.segments,
    sync: storedSync ?? { ...DEFAULT_SYNC },
    detection: null,
    storageError: stored.storageAvailable ? '' : STORAGE_WARNING,
  }
}

const PAGE_LINES = 8
// Flags shown before the flag list scrolls (it says how many there are), when
// the flags sit under the lines. In the Player's right pane the pane scrolls.
const FLAG_ROWS_SHOWN = 5

// The file types the picker offers, said the same way in the empty state.
const TRANSCRIPT_EXTENSIONS = ['srt', 'vtt', 'txt', 'asc', 'ascii']
const SUPPORTED_FORMATS = [
  ['SRT', '.srt'],
  ['WebVTT', '.vtt'],
  ['Time-stamped text, such as a CAT ASCII export', '.txt, .asc, .ascii'],
]

// A flag note in the Player's right pane: a textarea that grows with the
// note, so the whole note can be read in a narrow pane. Enter does not add a
// line break (a note is one line in the export).
function FlagNote({ className, value, ...props }) {
  const ref = useRef(null)
  const fit = () => {
    const el = ref.current
    // A hidden pane (a closed drawer) has no layout yet: measure when shown.
    if (!el || !el.clientWidth) return
    el.style.height = 'auto'
    el.style.height = `${el.scrollHeight}px`
  }
  useLayoutEffect(fit, [value])
  // Again when the pane is resized or shown, since the wrapping changes.
  useEffect(() => {
    const el = ref.current
    if (!el || typeof ResizeObserver !== 'function') return undefined
    let width = el.clientWidth
    const observer = new ResizeObserver(() => {
      if (el.clientWidth === width) return
      width = el.clientWidth
      fit()
    })
    observer.observe(el)
    return () => observer.disconnect()
  }, [])
  return (
    <textarea
      ref={ref}
      rows={1}
      value={value}
      className={cn('resize-none overflow-hidden', className)}
      onKeyDown={e => {
        if (e.key === 'Enter') e.preventDefault()
      }}
      {...props}
    />
  )
}

// The card fills the height it is given: the line list takes all the room
// left under the detection row (and the timing settings, when open) and
// scrolls inside itself. With `flagsContainer` (a DOM node, the Player's
// right pane), the flags list is shown there instead of under the lines.
// `flush` drops the card frame, for a pane that is its own frame.
export default function Transcript({
  trackPath,
  currentTime,
  playing,
  duration,
  onSeek,
  onPlayFrom,
  onStorageError,
  onClockChange,
  flagsContainer = null,
  flush = false,
}) {
  const [state, setState] = useState(() => loadTrackState(trackPath))
  if (state.path !== trackPath) setState(loadTrackState(trackPath))
  const { segments, sync, detection, storageError } = state.path === trackPath ? state : loadTrackState(trackPath)

  const [follow, setFollow] = useState(true)
  // Following is held while the person moves through the lines, and resumes
  // when playback starts again.
  const [held, setHeld] = useState(false)
  const [wasPlaying, setWasPlaying] = useState(playing)
  if (playing !== wasPlaying) {
    setWasPlaying(playing)
    if (playing) setHeld(false)
  }
  const following = follow && !held
  const [copied, setCopied] = useState(false)
  const [message, setMessage] = useState('')
  const [operationError, setOperationError] = useState('')
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [pendingImport, setPendingImport] = useState(null)
  const [focusIndex, setFocusIndex] = useState(0)
  const [startDraft, setStartDraft] = useState('')
  const [startError, setStartError] = useState('')
  // The timing settings (what the times mean, the recording start, the sync
  // shift) are mostly set once, so they sit behind one disclosure.
  const [timingOpen, setTimingOpen] = useState(false)
  const timingId = useId()
  const listRef = useRef(null)
  const startId = useId()
  const rowRefs = useRef([])

  useEffect(() => {
    onStorageError?.(storageError)
  }, [onStorageError, storageError])

  // The recording's start time of day, when the person entered one, so the
  // Player can show the time of day at the playhead.
  const clockStart =
    sync.timeKind === 'time-of-day' && segments.some(s => s.start != null) ? (sync.recordingStart ?? null) : null
  useEffect(() => {
    onClockChange?.(trackPath, clockStart)
  }, [onClockChange, trackPath, clockStart])

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
  const safeFocus = Math.min(focusIndex, Math.max(0, segments.length - 1))
  // The line at the playhead. When the focused line plays from the same
  // position (equal times, or an untimed line under it), it is that line.
  const activeIndex = useMemo(() => {
    const index = findActiveIndex(positions, currentTime)
    const focused = positions[safeFocus]?.playFrom
    return index >= 0 && focused != null && focused === positions[index].own ? safeFocus : index
  }, [positions, currentTime, safeFocus])
  // The line that F, Enter, Tab and "Set this line to now" use: the playing
  // line while following, else the line the person moved to.
  const currentIndex = following && activeIndex >= 0 ? activeIndex : safeFocus
  const flags = useMemo(() => flagRows(segments, sync), [segments, sync])
  const timedCount = positions.filter(p => p.own != null).length
  const needsStart = sync.timeKind === 'time-of-day' && sync.recordingStart == null
  // Open while the start time is needed or was entered wrong, and it stays
  // open after that until the person closes it.
  const timingForced = needsStart || Boolean(startError)
  if (timingForced && !timingOpen) setTimingOpen(true)
  const timingShown = timingOpen || timingForced
  const firstTimed = segments.find(s => s.start != null)
  const hasTimes = firstTimed != null
  const stampedCount = segments.filter(s => s.start != null).length
  const hasLines = segments.length > 0
  const currentName = lineName(segments[currentIndex], currentIndex)

  // Follow the line at the playhead inside the list. A keyboard focus on a
  // line moves with it; focus anywhere else is never moved.
  useEffect(() => {
    if (!following || activeIndex < 0) return
    const list = listRef.current
    const row = rowRefs.current[activeIndex]
    if (!list || !row) return
    const focused = document.activeElement
    if (focused !== row && focused?.parentElement === list) row.focus({ preventScroll: true })
    const top = row.offsetTop - list.offsetTop
    if (top < list.scrollTop || top + row.offsetHeight > list.scrollTop + list.clientHeight) {
      list.scrollTop = Math.max(0, top - list.clientHeight / 3)
    }
    // In a very short window the list can sit partly below its pane's fold.
    row.scrollIntoView?.({ block: 'nearest' })
  }, [activeIndex, following, playing])

  const focusRow = index => {
    const next = Math.max(0, Math.min(segments.length - 1, index))
    if (follow) setHeld(true)
    setFocusIndex(next)
    rowRefs.current[next]?.focus()
  }

  const goToPlaying = () => {
    if (activeIndex < 0) {
      setMessage('No line is playing yet.')
      return
    }
    setFollow(true)
    setHeld(false)
    setFocusIndex(activeIndex)
    rowRefs.current[activeIndex]?.focus()
  }

  const playLine = index => {
    const at = positions[index]?.playFrom
    setFocusIndex(index)
    if (at == null) {
      setMessage(
        needsStart
          ? 'Enter the time the recording started to play from a line.'
          : positions[index]?.unplaced
            ? "This line's time is outside the recording. Check the start time."
            : 'This line has no time, so it cannot be played from.',
      )
      return
    }
    setHeld(false)
    setMessage('')
    ;(onPlayFrom ?? onSeek)?.(at)
  }

  const flagLine = index => {
    const line = segments[index]
    if (!line) return
    commit({ segments: toggleFlag(segments, line.id) })
    const name = lineName(line, index)
    setMessage(line.flag ? `Flag removed from ${name}.` : `Flagged ${name} for review.`)
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
    } else if (event.key === '.') {
      event.preventDefault()
      event.stopPropagation()
      goToPlaying()
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
    setHeld(false)
    setStartDraft('')
    setMessage('')
  }

  const importFile = async () => {
    setOperationError('')
    try {
      const sel = await open({
        multiple: false,
        filters: [{ name: 'Transcript', extensions: TRANSCRIPT_EXTENSIONS }],
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
    const line = segments[currentIndex]
    const shift = shiftForLineAt(line?.start, currentTime, sync)
    if (shift == null) {
      setMessage('This line has no time.')
      return
    }
    setSync({ shift })
    setMessage(`Set ${currentName} to play from ${fmtElapsed(currentTime)}.`)
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
      await navigator.clipboard.writeText(transcriptText(segments, sync.timeKind))
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
      ? undefined
      : `Lines play ${Math.abs(sync.shift).toFixed(1)} s ${sync.shift > 0 ? 'later' : 'earlier'} than their stamps.`

  const pill = on =>
    cn(
      'inline-flex h-7 items-center gap-1 px-2 type-small transition-colors',
      on ? 'bg-secondary text-foreground ring-1 ring-inset ring-foreground' : 'text-ink-2 hover:text-foreground',
    )
  const gutter = 'w-10 shrink-0 text-right type-min tabular-nums text-ink-2'
  // Unavailable is a hidden (dashed) line in ink 2, never faded; it also drops
  // to regular weight, so it differs from the semibold live buttons in dark too.
  const smallButton =
    'inline-flex h-7 items-center gap-1 border border-input px-2 font-semibold tabular-nums hover:bg-secondary disabled:border-dashed disabled:font-normal disabled:text-ink-2 disabled:hover:bg-transparent'
  const title = 'type-label text-ink-2'

  const status = copied ? 'Lines and times copied to the clipboard.' : message

  // The flags list: under the lines, or (in the Player) in the right pane,
  // where each flag gets two rows and the pane scrolls.
  const inPane = flagsContainer != null
  const flagsPanel = (
    <section
      aria-label="Flags for review"
      className={cn('flex flex-col', !inPane && 'shrink-0 border-t border-border px-4 pb-2')}
    >
      {/* The title holds its own 40px head line, as the Listening mix does; in
          a narrow pane the action wraps under it, its text on the same edge. */}
      <div className={cn('flex flex-wrap items-center justify-between gap-x-2', !inPane && 'min-h-10')}>
        <h3 className={cn('shrink-0', inPane && 'flex min-h-10 items-center', title)}>
          Flags for review{flags.length ? ` (${flags.length})` : ''}
          {!inPane && flags.length > FLAG_ROWS_SHOWN && (
            <span className="font-normal"> · scroll for {flags.length - FLAG_ROWS_SHOWN} more</span>
          )}
        </h3>
        {flags.length > 0 && (
          <Button
            size="sm"
            variant="ghost"
            className={cn('shrink-0', inPane && '-mx-2.5 mb-1')}
            onClick={exportFlags}
            title="Save flags as CSV or text"
          >
            <Download size={12} /> Export flags
          </Button>
        )}
      </div>
      {!hasLines ? (
        <p className="type-small text-ink-2">
          Open a transcript to flag lines for review. Flags never change the transcript.
        </p>
      ) : flags.length === 0 ? (
        <p className="type-small text-ink-2">
          Press <kbd className="tabular-nums">F</kbd> on a line, or click its flag, to mark it for review. Flags never
          change the transcript.
        </p>
      ) : (
        <ul
          className={cn('flex flex-col', inPane ? 'gap-3' : 'gap-1 overflow-y-auto')}
          style={inPane ? undefined : { maxHeight: `${FLAG_ROWS_SHOWN * 1.625}rem` }}
        >
          {flags.map(f => {
            const jump = (
              <button
                className="w-14 shrink-0 text-left tabular-nums text-ink-2 hover:text-foreground"
                onClick={() => playLine(f.index)}
                aria-label={`Play from ${f.name}, ${fmtElapsed(f.audioTime)}`}
              >
                {f.audioTime == null ? '—' : fmtElapsed(f.audioTime)}
              </button>
            )
            const noteProps = {
              'aria-label': `Note for flag on ${f.name}`,
              placeholder: 'Add a note…',
              maxLength: MAX_FLAG_NOTE_CHARS,
              value: f.note,
              onChange: e => commit({ segments: setFlagNote(segments, f.id, e.target.value) }),
            }
            const note = inPane ? (
              <FlagNote
                className="min-h-6 min-w-0 flex-1 border-b border-input bg-transparent py-0.5 placeholder:italic placeholder:text-ink-2 focus:border-foreground"
                {...noteProps}
              />
            ) : (
              <input
                className="h-6 min-w-0 flex-1 border-b border-input bg-transparent px-0.5 type-small placeholder:italic placeholder:text-ink-2 focus:border-foreground"
                {...noteProps}
              />
            )
            const remove = (
              <button
                className="inline-flex h-6 w-6 shrink-0 items-center justify-center text-ink-2 transition-colors hover:bg-secondary hover:text-destructive"
                aria-label={`Remove flag from ${f.name}`}
                title="Remove flag"
                onClick={() => flagLine(f.index)}
              >
                <X size={13} />
              </button>
            )
            const ref = (
              <span aria-hidden="true" className={gutter}>
                {lineRef(segments[f.index], f.index)}
              </span>
            )
            return inPane ? (
              // Ref, excerpt and note share one left edge; the excerpt and
              // note are transcript text, so they keep the 14px body size.
              <li key={f.id} className="group flex flex-col gap-1">
                <div className="flex items-center gap-2 min-w-0 type-min">
                  <span aria-hidden="true" className="shrink-0 tabular-nums text-ink-2">
                    {lineRef(segments[f.index], f.index)}
                  </span>
                  {jump}
                  <span className="flex-1" />
                  {remove}
                </div>
                <span className="line-clamp-2 break-words" title={f.excerpt}>
                  {f.excerpt}
                </span>
                <div className="flex">{note}</div>
              </li>
            ) : (
              <li key={f.id} className="group flex items-center gap-2 type-min">
                {ref}
                {jump}
                <span className="w-2/5 min-w-0 shrink truncate text-foreground" title={f.excerpt}>
                  {f.excerpt}
                </span>
                {note}
                {remove}
              </li>
            )
          })}
        </ul>
      )}
    </section>
  )

  return (
    <Card className={cn('@container flex h-full min-h-0 flex-col', flush && 'border-0 bg-transparent')}>
      <div className="pane-head shrink-0">
        {/* The title gives way (truncates) before the actions wrap. */}
        <h2 className={cn('min-w-[5rem] shrink-[4] truncate', title)} title="Transcript proofing">
          Transcript proofing
        </h2>
        <div className="flex min-w-0 flex-wrap items-center justify-end gap-1">
          {segments.length > 0 && hasTimes && (
            <button
              onClick={held ? goToPlaying : () => setFollow(f => !f)}
              aria-pressed={following}
              title={held ? 'Go back to the line playing now (.)' : 'Keep the line playing now in view'}
              className={pill(following)}
            >
              <Locate size={11} />{' '}
              {held ? (
                <span>
                  <span className="@max-[40rem]:sr-only">Follow paused: </span>back to playing line
                </span>
              ) : (
                'Follow audio'
              )}
            </button>
          )}
          {hasLines && (
            <>
              {/* In a narrow card the labels become icon-only (the name stays for screen readers). */}
              <Button size="sm" variant="ghost" onClick={importFile} title="Open another transcript">
                <Upload size={12} /> <span className="@max-[34rem]:sr-only">Open another</span>
              </Button>
              <Button size="sm" variant="ghost" onClick={copyText} title="Copy the lines with their times">
                {copied ? <Check size={12} /> : <Copy size={12} />}{' '}
                <span className="@max-[34rem]:sr-only">{copied ? 'Copied' : 'Copy with times'}</span>
              </Button>
              <Button
                size="sm"
                variant="ghost"
                className="hover:text-destructive"
                onClick={() => setConfirmDelete(true)}
                title="Remove this transcript from DepoAudio"
                aria-label="Remove this transcript from DepoAudio"
              >
                <Trash2 size={12} />
              </Button>
            </>
          )}
        </div>
      </div>

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
        <p role="alert" className="notice mx-4 mb-2 flex items-start gap-2">
          <AlertTriangle aria-hidden="true" size={14} className="mt-0.5 shrink-0 text-warning" />
          {storageError}
        </p>
      )}

      {operationError && (
        <p role="alert" className="notice notice-error mx-4 mb-2">
          {operationError}
        </p>
      )}

      {!hasLines ? (
        <div className="flex min-h-0 flex-1 overflow-y-auto">
          <div className="m-auto flex w-full max-w-[34rem] flex-col items-start gap-4 px-6 py-8">
            <p className="type-h3 text-foreground">Proof a transcript against this recording</p>
            <p className="type-small text-ink-2">
              Open the transcript of this recording. The line at the playhead is highlighted as the audio plays, you can
              play from any line, and you can flag lines to check. DepoAudio does not change the transcript; make
              corrections in your CAT software.
            </p>
            <Button variant="primary" onClick={importFile}>
              <Upload size={14} /> Open transcript
            </Button>
            <div className="w-full">
              <p className={title}>Formats you can open</p>
              <ul className="mt-1.5 flex flex-col gap-1 type-small text-foreground">
                {SUPPORTED_FORMATS.map(([label, ext]) => (
                  <li key={ext} className="flex items-baseline gap-2">
                    <span className="w-28 shrink-0 tabular-nums text-ink-2">{ext}</span>
                    <span>{label}</span>
                  </li>
                ))}
              </ul>
            </div>
          </div>
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col">
          {/* Detection, the sync shift, and the timing settings behind one
              disclosure. The disclosure stays open while the recording start
              time is still needed, so the lines are never out of step
              without the person seeing why. */}
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border px-4 py-1 type-small text-ink-2">
            <span className="min-w-0">
              {detection
                ? `${detection.name}: ${describeDetection(detection)}`
                : `Saved transcript: ${describeDetection({
                    format: 'text',
                    timedCount: stampedCount,
                    untimedCount: segments.length - stampedCount,
                    timeKind: sync.timeKind,
                  })}`}
            </span>
            {timedCount > 0 && sync.shift !== 0 && !timingShown && (
              <span className="tabular-nums font-medium text-foreground" title={shiftTitle}>
                {shiftLabel}
              </span>
            )}
            {hasTimes && !timingForced && (
              <button
                type="button"
                className={pill(timingShown)}
                aria-expanded={timingShown}
                aria-controls={timingId}
                onClick={() => setTimingOpen(o => !o)}
              >
                Timing and sync
                <ChevronDown size={11} className={cn('transition-transform', timingShown && 'rotate-180')} />
              </button>
            )}
            {/* What the last action did (which line was flagged, and so on), in view. */}
            <p role="status" aria-live="polite" className="ml-auto font-medium text-foreground">
              {status}
            </p>
          </div>

          {hasTimes && timingShown && (
            <div
              id={timingId}
              role="group"
              aria-label="Timing and sync"
              className="flex flex-col gap-2 border-b border-border px-4 py-2 type-small text-ink-2"
            >
              <span className="inline-flex flex-wrap items-center gap-1" role="group" aria-label="What the times mean">
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

              {sync.timeKind === 'time-of-day' && (
                <form onSubmit={saveStart} className="flex flex-wrap items-center gap-x-2 gap-y-1">
                  <label htmlFor={startId}>Recording started at</label>
                  {sync.recordingStart != null && !startDraft && (
                    <span className="tabular-nums font-medium text-foreground">{fmtClock(sync.recordingStart)}</span>
                  )}
                  <input
                    id={startId}
                    className="h-7 w-28 border border-input bg-transparent px-1.5 tabular-nums placeholder:text-ink-2"
                    placeholder={sync.recordingStart != null ? 'change' : 'HH:MM:SS'}
                    value={startDraft}
                    onChange={e => setStartDraft(e.target.value)}
                    aria-describedby={`${startId}-help`}
                    aria-invalid={startError ? true : undefined}
                  />
                  <button type="submit" className={smallButton} disabled={!startDraft.trim()}>
                    Save
                  </button>
                  {needsStart && firstTimed && (
                    <button
                      type="button"
                      className={smallButton}
                      onClick={() => setSync({ recordingStart: firstTimed.start })}
                    >
                      Use first line&apos;s time ({fmtClock(firstTimed.start)})
                    </button>
                  )}
                  <span id={`${startId}-help`} className={startError ? 'text-destructive' : ''}>
                    {startError ||
                      (needsStart ? 'DepoAudio does not know when this recording started.' : 'Entered by you.')}
                  </span>
                </form>
              )}

              {timedCount > 0 && (
                <div className="flex flex-wrap items-center gap-x-2 gap-y-1" role="group" aria-label="Sync">
                  <span className="tabular-nums font-medium text-foreground" title={shiftTitle}>
                    {shiftLabel}
                  </span>
                  <button
                    className={smallButton}
                    onClick={() => nudge(-0.5)}
                    aria-label="Move lines 0.5 seconds earlier"
                  >
                    −0.5 s
                  </button>
                  <button className={smallButton} onClick={() => nudge(0.5)} aria-label="Move lines 0.5 seconds later">
                    +0.5 s
                  </button>
                  <button
                    className={smallButton}
                    onClick={setLineToNow}
                    title={`Make ${currentName} play from the current audio position`}
                  >
                    Set {currentName} to now
                  </button>
                  {sync.shift !== 0 && (
                    <button className={smallButton} onClick={() => setSync({ shift: 0 })}>
                      Clear shift
                    </button>
                  )}
                </div>
              )}
            </div>
          )}
          {/* Lines (read-only) */}
          <div
            ref={listRef}
            role="listbox"
            aria-label="Transcript lines. Enter plays a line, F flags it, period goes to the playing line."
            className="relative min-h-[124px] flex-1 overflow-y-auto p-2"
          >
            {segments.map((s, index) => {
              const active = index === activeIndex
              const at = positions[index]?.own
              const name = lineName(s, index)
              return (
                <div
                  key={s.id}
                  ref={el => {
                    rowRefs.current[index] = el
                  }}
                  role="option"
                  aria-current={active ? 'true' : undefined}
                  tabIndex={index === currentIndex ? 0 : -1}
                  onClick={() => playLine(index)}
                  onFocus={() => setFocusIndex(index)}
                  onKeyDown={e => onRowKey(e, index)}
                  title={at == null ? undefined : `Plays from ${fmtElapsed(at)} in the audio`}
                  className={cn(
                    // Line text is body copy (14px); the gutter and time are record numbers (type-min).
                    'inset-focus group flex cursor-pointer items-baseline gap-2 border-l-2 py-0.5 pl-1.5 pr-2',
                    active ? 'bg-secondary border-l-foreground' : 'border-l-transparent hover:bg-secondary/50',
                  )}
                >
                  <span className="sr-only">{name}: </span>
                  <span aria-hidden="true" className={gutter}>
                    {lineRef(s, index)}
                  </span>
                  <span className="w-16 shrink-0 type-min tabular-nums text-ink-2">
                    {s.start == null ? '' : sync.timeKind === 'time-of-day' ? fmtClock(s.start) : fmtElapsed(s.start)}
                  </span>
                  {s.speaker && <span className="shrink-0 font-medium text-ink-2">{s.speaker}:</span>}
                  {/* A readable measure at any width; the flag sits right after the text. */}
                  <span className="min-w-0 max-w-[80ch] whitespace-pre-wrap break-words">{s.text}</span>
                  {s.flag && <span className="sr-only">, flagged</span>}
                  {/* Mouse only: the keyboard uses F, and the state is read above. */}
                  <span
                    aria-hidden="true"
                    onClick={e => {
                      e.stopPropagation()
                      flagLine(index)
                    }}
                    title={s.flag ? 'Remove flag (F)' : 'Flag for review (F)'}
                    className={cn(
                      '-my-0.5 inline-flex h-6 w-6 shrink-0 items-center justify-center self-start transition-opacity',
                      s.flag
                        ? 'text-foreground'
                        : 'text-ink-2 opacity-0 group-hover:opacity-100 group-focus:opacity-100 hover:text-foreground',
                    )}
                  >
                    <Flag size={13} fill={s.flag ? 'currentColor' : 'none'} />
                  </span>
                </div>
              )
            })}
          </div>

          {!flagsContainer && flagsPanel}
        </div>
      )}
      {flagsContainer && createPortal(flagsPanel, flagsContainer)}
    </Card>
  )
}
