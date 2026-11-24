import { useState, useEffect, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import {
  MODES,
  FORMATS_OUT,
  MP3_BITRATES,
  CH_COLORS,
  FIXED_OUTPUT_RATES,
  KEEP_STEREO_ONLY_FORMATS,
  KEEP_STEREO_ONLY_NOTE,
  effectiveOutputRate,
} from '../../constants'
import { openUrl } from '@tauri-apps/plugin-opener'
import {
  PRESETS,
  NOT_TESTED_NOTE,
  OPUS_CATALYST_NOTE,
  STENOGRAPH_SOURCES,
  PROCESSING_OPTIONS,
  describeSettings,
  processingOptionsInUse,
  processingSummary,
} from '../../presets'
import { usePreferencesContext } from '../../hooks/PreferencesContext'
import { AlertTriangle, Check, Loader2, Play } from 'lucide-react'
import { Button } from '../ui/button'
import { Badge } from '../ui/badge'
import { Input } from '../ui/input'
import { Label } from '../ui/label'
import { ModeIcon, WaveformIcon } from '../common/Icons'
import WorkspaceHeader from '../common/WorkspaceHeader'
import ProcessingToggle from './ProcessingToggle'
import FormatTable from './FormatTable'
import FileRow from './FileRow'
import { wavSizeEstimateText } from '../../lib/outputSize'
import { trmSessionNotices } from '../../lib/queue'
import { SCAN_SAMPLE_NOTE, aggregateAnalysisResults } from '../../lib/analysis'

// The backend emits scan:progress events throughout each file's analysis
// (including ~10s heartbeats while a wedged ffmpeg drains its timeout), so
// staleness — not elapsed time — is the hang signal: a healthy scan of a big
// multichannel file can legitimately run for many minutes, but it never goes
// quiet for long. The stall threshold sits above the backend's longest
// legitimately-silent decode window. FILE_CAP_MS is the absolute backstop.
const SCAN_STALL_MS = 150000 // no progress event for 150s = stuck
const SCAN_FILE_CAP_MS = 900000 // 15 minutes absolute cap per file

// Human labels for the backend's scan phases
const SCAN_PHASES = {
  prepare: 'Reading the recording',
  probe: 'Reading file info',
  loudness: 'Measuring loudness',
  done: 'Finishing up',
}

// Each processing option's name and one-line reason, by id.
const PROCESSING = Object.fromEntries(PROCESSING_OPTIONS.map(option => [option.id, option]))

export default function ConvertTab({
  // Files
  files,
  audioStreams = {},
  dragOver,
  caseName,
  setCaseName,
  onDragOver,
  onDragLeave,
  onDrop,
  browseFiles,
  browseOutDir,
  removeFile,
  clearAll,
  // Conversion
  jobs,
  converting,
  cancelling = false,
  conversionError = '',
  queueLocked = converting,
  queueNotice = '',
  startConversion,
  cancelConversion,
  doneCount,
  failCount,
  cancelledCount = 0,
  onOpenInPlayer,
}) {
  const {
    mode,
    setMode,
    formatOut,
    setFormatOut,
    labels,
    setLabels,
    chanVols,
    setChanVols,
    outDir,
    setOutDir,
    rate,
    setRate,
    mp3Bitrate,
    setMp3Bitrate,
    normalize,
    setNormalize,
    trim,
    setTrim,
    fade,
    setFade,
    fadeDur,
    setFadeDur,
    hpf,
    setHpf,
    autoLevel,
    setAutoLevel,
    declip,
    setDeclip,
  } = usePreferencesContext()
  const trimEffective = trim && mode !== 'split'
  const autoLevelEffective = autoLevel && mode !== 'keep'
  const anyProcessing = normalize || trimEffective || fade || hpf || autoLevelEffective || declip
  const presetState = describeSettings({
    mode,
    format: formatOut,
    rate,
    mp3Bitrate,
    normalize,
    trim,
    fade,
    fadeDur,
    hpf,
    autoLevel,
    declip,
  })
  // The preset the person chose in this session. A preset is "in use" only
  // when they chose it and the settings still have its format; settings that
  // merely happen to match one (the defaults match "For CATalyst audio sync"
  // and "For other or older CAT software") are described as a match, not as
  // a choice. Processing is never part of a preset: the status and every
  // card say what processing the person has turned on.
  const [chosenPresetId, setChosenPresetId] = useState(null)
  const presetInUse = presetState.matches.find(p => p.id === chosenPresetId) || null
  const processingLine = processingSummary({
    mode,
    normalize,
    trim,
    fade,
    hpf,
    autoLevel,
    declip,
  })
  const quoted = presets => presets.map(p => `\u201c${p.name}\u201d`).join(' and ')
  const legacyNote = presetState.legacy
    ? ` These are the settings of the earlier \u201c${presetState.legacy.name}\u201d preset; its processing stays on until you turn it off.`
    : ''
  const presetStatus = presetInUse
    ? `Using: ${presetInUse.name}. ${processingLine}.`
    : presetState.kind === 'preset'
      ? `No preset chosen. The current settings are the same as ${quoted(presetState.matches)}.`
      : presetState.matches.length > 0
        ? `Custom: the format of ${quoted(presetState.matches)}. ${processingLine}.${legacyNote}`
        : `Custom settings. ${processingLine}.${legacyNote}`
  const [sourceLinkError, setSourceLinkError] = useState('')
  const openSource = async source => {
    setSourceLinkError('')
    try {
      await openUrl(source.url)
    } catch (error) {
      setSourceLinkError(`Couldn't open Stenograph's \u201c${source.title}\u201d article: ${String(error)}`)
    }
  }
  // The processing that would run with the current settings in `nextMode`.
  const processingFor = nextMode =>
    processingOptionsInUse({ mode: nextMode, normalize, trim, fade, hpf, autoLevel, declip })
  // Turn off the given processing options, only when the person asks.
  const processingSetters = {
    autoLevel: setAutoLevel,
    declip: setDeclip,
    hpf: setHpf,
    normalize: setNormalize,
    trim: setTrim,
    fade: setFade,
  }
  const turnProcessingOff = options => {
    for (const option of options) processingSetters[option.id](false)
  }
  // A preset sets only the channel mode, the format and the MP3 bitrate. It
  // never touches processing or the sample rate on its own. A preset whose
  // name promises the recording unchanged (Archive) asks first when
  // processing is on: the person keeps it, turns it off, or cancels.
  const [processingQuestion, setProcessingQuestion] = useState(null)
  const setPresetNow = preset => {
    setProcessingQuestion(null)
    setChosenPresetId(preset.id)
    const s = preset.settings
    handleModeChange(s.mode)
    setFormatOut(s.format)
    if (s.format === 'mp3') setMp3Bitrate(s.mp3Bitrate ?? 192)
  }
  const applyPreset = preset => {
    const inUse = processingFor(preset.settings.mode)
    if (preset.asksAboutProcessing && inUse.length > 0) {
      setProcessingQuestion({ preset, options: inUse })
      return
    }
    setPresetNow(preset)
  }
  const currentProcessing = processingFor(mode)
  const [analysis, setAnalysis] = useState(null)
  const [scanning, setScanning] = useState(false)
  const [scanProgress, setScanProgress] = useState({ current: 0, total: 0, fileName: '', phase: '', filePct: 0 })
  const [scanError, setScanError] = useState(null)
  const [scanCancelling, setScanCancelling] = useState(false)

  // Guards for long-running scans: scanIdRef invalidates a superseded scan
  // (cancel, re-click, unmount), filesRef detects the queue changing mid-scan
  const scanIdRef = useRef(0)
  const scanCancelInFlightRef = useRef(false)
  const scanStopRequestedRef = useRef(false)
  const scanStopBlockedRef = useRef(false)
  const filesRef = useRef(files)
  useEffect(() => {
    filesRef.current = files
  }, [files])

  // Suggestions from the last scan that are not switched on yet
  const pendingSuggestions = analysis
    ? [
        analysis.needsLeveling &&
          mode !== 'keep' &&
          !autoLevel && { id: 'autoLevel', label: 'Balance Microphone Levels' },
        analysis.hasClipping && !declip && { id: 'declip', label: 'Fix Clipped Audio' },
      ].filter(Boolean)
    : []
  const applySuggestions = () => {
    for (const suggestion of pendingSuggestions) {
      if (suggestion.id === 'autoLevel') setAutoLevel(true)
      if (suggestion.id === 'declip') setDeclip(true)
    }
  }

  const handleModeChange = nextMode => {
    setMode(nextMode)
    if (nextMode === 'keep') setAutoLevel(false)
  }

  // A scan outliving this tab (unmount on tab switch) must stop computing —
  // otherwise it keeps churning invisibly and can flip toggles minutes later
  useEffect(
    () => () => {
      scanIdRef.current += 1
      invoke('cancel_scan_cmd').catch(() => {})
    },
    [],
  )

  // Clear analysis when files change (reset during render, not in an effect)
  const [prevFiles, setPrevFiles] = useState(files)
  if (files !== prevFiles) {
    setPrevFiles(files)
    setAnalysis(null)
    setScanError(null)
  }

  const cancelScan = async () => {
    if (scanCancelInFlightRef.current) return
    scanStopRequestedRef.current = true
    scanCancelInFlightRef.current = true
    setScanCancelling(true)
    setScanError(null)

    try {
      // Do not invalidate the local scan or enable a successor until the
      // backend confirms its generation was advanced.
      await invoke('cancel_scan_cmd')
      scanStopBlockedRef.current = false
      scanIdRef.current += 1
      setScanning(false)
    } catch (error) {
      scanStopBlockedRef.current = true
      setScanError(
        `Could not stop the scan: ${String(error)}. The backend may still be working; retry Cancel before starting another scan.`,
      )
    } finally {
      scanCancelInFlightRef.current = false
      setScanCancelling(false)
    }
  }

  const handleScan = async () => {
    if (!files.length || scanning) return
    setScanning(true)
    setScanError(null)
    setScanCancelling(false)
    scanStopRequestedRef.current = false
    scanStopBlockedRef.current = false
    scanIdRef.current += 1
    const scanId = scanIdRef.current
    const scanFiles = files
    setScanProgress({ current: 0, total: scanFiles.length, fileName: '', phase: '', filePct: 0 })

    // Within-file progress from the backend; also feeds the stall watchdog.
    // Filter by path AND backend generation: a just-cancelled scan's dying
    // passes emit trailing events — for the SAME path if the user re-scans
    // immediately — that must not move the bar or feed the watchdog. Each
    // cancel bumps the backend epoch, so the successor's events always carry
    // a strictly higher gen; drop anything below the highest gen seen.
    let currentPath = null
    let maxGen = 0
    let lastEvent = Date.now()
    let unlisten
    try {
      unlisten = await listen('scan:progress', ({ payload }) => {
        if (scanIdRef.current !== scanId || payload.path !== currentPath) return
        const gen = payload.gen ?? 0
        if (gen < maxGen) return
        maxGen = gen
        lastEvent = Date.now()
        setScanProgress(p => ({ ...p, phase: payload.phase, filePct: payload.pct ?? p.filePct }))
      })
    } catch (error) {
      setScanError(`Could not monitor scan progress: ${String(error)}`)
      setScanning(false)
      return
    }

    const failed = []
    const stopForQueueChange = async () => {
      scanStopRequestedRef.current = true
      try {
        await invoke('cancel_scan_cmd')
        setScanError('File list changed during the scan — click Scan to re-analyze.')
      } catch (error) {
        scanStopBlockedRef.current = true
        setScanError(
          `The file list changed, but the active scan could not be stopped: ${String(error)}. Retry Cancel before starting another scan.`,
        )
      }
    }

    try {
      // Scan files one at a time to keep sidecar work bounded and cancellable.
      const results = []
      for (let i = 0; i < scanFiles.length; i++) {
        if (scanIdRef.current !== scanId || scanStopRequestedRef.current) return // cancelled or superseded
        if (filesRef.current !== scanFiles) {
          // Queue edited mid-scan: stop burning CPU on a stale file list
          // Keep Scan disabled until the epoch bump completes; otherwise a
          // successor can snapshot the old epoch and be killed by this late
          // cancellation request.
          await stopForQueueChange()
          return
        }
        const file = scanFiles[i]
        currentPath = file.path
        setScanProgress({ current: i, total: scanFiles.length, fileName: file.name, phase: 'probe', filePct: 0 })
        lastEvent = Date.now()
        try {
          // Watchdog: a healthy scan streams progress events; silence means a
          // wedged backend, so cancel it server-side and skip the file. The
          // absolute cap catches a backend that keeps emitting but never ends.
          const startedAt = Date.now()
          const result = await new Promise((resolve, reject) => {
            const watchdog = setInterval(() => {
              const stalled = Date.now() - lastEvent > SCAN_STALL_MS
              const overCap = Date.now() - startedAt > SCAN_FILE_CAP_MS
              if (stalled || overCap) {
                clearInterval(watchdog)
                // Reject only after the cancel round-trip: otherwise the next
                // file's analyze could snapshot its epoch before this bump
                // lands and be spuriously cancelled at its first check
                invoke('cancel_scan_cmd').then(
                  () => reject(new Error(stalled ? 'Scan stalled' : 'Scan timed out')),
                  error => {
                    scanStopRequestedRef.current = true
                    scanStopBlockedRef.current = true
                    reject(
                      Object.assign(
                        new Error(
                          `${stalled ? 'The scan stalled' : 'The scan timed out'}, but it could not be stopped: ${String(error)}. Retry Cancel before starting another scan.`,
                        ),
                        { scanCancellationFailed: true },
                      ),
                    )
                  },
                )
              }
            }, 5000)
            invoke('analyze_audio_cmd', { path: file.path }).then(
              r => {
                clearInterval(watchdog)
                resolve(r)
              },
              e => {
                clearInterval(watchdog)
                reject(e)
              },
            )
          })
          if (scanStopRequestedRef.current) return
          if (result) results.push(result)
        } catch (error) {
          if (error?.scanCancellationFailed) {
            setScanError(error.message)
            return
          }
          failed.push(file.name)
        }
        if (scanStopRequestedRef.current) return
        // Guard the post-await write: a cancelled scan settling late must not
        // clobber a successor scan's progress header
        if (scanIdRef.current === scanId) {
          setScanProgress({ current: i + 1, total: scanFiles.length, fileName: file.name, phase: 'done', filePct: 1 })
        }
      }

      // Ignore results that no longer describe the current queue
      if (scanIdRef.current !== scanId) return
      if (filesRef.current !== scanFiles) {
        await stopForQueueChange()
        return
      }

      // Failures were silent before — a scan where everything timed out
      // looked exactly like a scan that never ran
      if (failed.length > 0) {
        setScanError(
          results.length === 0
            ? `Couldn't analyze ${failed.length === 1 ? failed[0] : `any of the ${failed.length} files`} — the file may need converting before it can be scanned.`
            : `${failed.length} of ${scanFiles.length} files couldn't be analyzed and were skipped.`,
        )
      }

      if (results.length > 0) {
        // Aggregate non-learned signal measurements across all files.
        const aggregated = aggregateAnalysisResults(results)
        // The scan only suggests: nothing is switched on until the person
        // chooses "Apply suggestions" or turns a toggle on themselves.
        setAnalysis(aggregated)
      }
    } catch (e) {
      console.error('Scan failed:', e)
    } finally {
      unlisten()
      // A cancelled scan already reset the button via cancelScan
      if (scanIdRef.current === scanId && !scanStopBlockedRef.current) setScanning(false)
    }
  }

  // Guided stepper: reflects real state without gating anything — batch users
  // can still work the whole page at once. Completion is judged against the
  // CURRENT queue (jobs are keyed by path), so adding a new file resets the
  // stepper instead of inheriting the previous conversion's checkmark, and a
  // finished queue shows all three steps done rather than bouncing the
  // highlight back to "Choose settings".
  const queueStatuses = files.map(f => jobs[f.path]?.status)
  const queueSettled =
    files.length > 0 &&
    !converting &&
    queueStatuses.every(s => s === 'done' || s === 'error' || s === 'cancelled') &&
    queueStatuses.some(s => s === 'done')
  const step = converting || queueSettled ? 3 : files.length > 0 ? 2 : 1
  const stepDone = n => n < step || (n === 3 && queueSettled)
  const procCount = [autoLevelEffective, declip, hpf, normalize, trimEffective, fade].filter(Boolean).length
  const formatLabel = FORMATS_OUT.find(f => f.id === formatOut)?.label || formatOut
  const wavEstimate = formatOut === 'wav' ? wavSizeEstimateText(files, audioStreams, { mode, rate }) : ''
  const sessionNotices = trmSessionNotices(files, audioStreams)
  const modeLabel = MODES.find(m => m.id === mode)?.label || mode
  const unsupportedFiles = files.filter(file => file.fmt?.status === 'unsupported')
  const resultParts = [
    doneCount > 0 ? `${doneCount} converted` : '',
    failCount > 0 ? `${failCount} failed` : '',
    cancelledCount > 0 ? `${cancelledCount} cancelled` : '',
  ].filter(Boolean)
  const resultCount = doneCount + failCount + cancelledCount
  const resultVariant = failCount > 0 ? 'error' : cancelledCount > 0 ? 'warning' : 'done'
  const scanProgressPercent =
    scanProgress.total > 0
      ? Math.min(
          100,
          Math.max(0, Math.round(((scanProgress.current + (scanProgress.filePct || 0)) / scanProgress.total) * 100)),
        )
      : 0
  const convertDisabledReason = queueLocked
    ? 'Wait for the current conversion to finish.'
    : files.length === 0
      ? 'Add at least one recording to continue.'
      : unsupportedFiles.length > 0
        ? unsupportedFiles.every(file => file.fmt?.key === 'dcr-image')
          ? 'Remove the files that are not recordings first.'
          : 'Remove unsupported files first. The note for each one says how to export it.'
        : ''

  const warnGlyph = <AlertTriangle aria-hidden="true" size={14} className="mt-0.5 shrink-0 text-warning" />
  const title = 'type-label text-ink-2'

  return (
    <>
      <div className="flex-1 overflow-y-auto overflow-x-hidden scrollbar-thin scrollbar-thumb-border scrollbar-track-transparent">
        <div className="conversion-workspace w-full max-w-[1100px] mx-auto px-4 sm:px-6 lg:px-8 py-5 flex flex-col gap-4">
          <WorkspaceHeader
            eyebrow="Court-audio conversion"
            title="Convert court recordings"
            description="Open difficult court audio, choose a practical output, and prepare files for review or transcript production."
            status="Recordings and processing stay on this computer"
          />

          {/* ── STEPPER (state-driven, purely informational) ─────────────── */}
          <ol className="conversion-stepper flex items-center gap-2" aria-label="Conversion progress">
            {['Add recording', 'Choose settings', 'Convert'].map((label, i) => {
              const n = i + 1
              const done = stepDone(n)
              const now = !done && n === step
              return (
                <li
                  key={label}
                  className="conversion-step flex items-center gap-2"
                  aria-current={now ? 'step' : undefined}
                >
                  <span
                    aria-hidden="true"
                    className={`w-6 h-6 rounded-full grid place-items-center type-label tabular-nums border transition-colors ${
                      done
                        ? 'bg-success border-success text-background'
                        : now
                          ? 'bg-foreground border-foreground text-background'
                          : 'bg-card border-border text-ink-2'
                    }`}
                  >
                    {done ? <Check aria-hidden="true" className="h-3.5 w-3.5" strokeWidth={2.2} /> : n}
                  </span>
                  <span className={`type-small ${now ? 'font-semibold' : 'text-ink-2'}`}>{label}</span>
                  {n < 3 && (
                    <span
                      aria-hidden="true"
                      className={`conversion-step-line w-8 h-px ${done ? 'bg-success' : 'bg-border'}`}
                    />
                  )}
                </li>
              )
            })}
          </ol>

          {/* ── FILES (drop zone + queue) — the workflow starts here ─────── */}
          <div
            aria-busy={queueLocked}
            className={`recording-intake grid items-center gap-4 border border-dashed p-4 transition-colors sm:grid-cols-[auto_minmax(0,1fr)_auto] ${
              dragOver && !queueLocked ? 'border-foreground bg-secondary' : 'border-input bg-card'
            } ${queueLocked ? 'text-ink-2' : ''}`}
            onDragOver={queueLocked ? undefined : onDragOver}
            onDragLeave={onDragLeave}
            onDrop={queueLocked ? undefined : onDrop}
          >
            <span aria-hidden="true" className="grid place-items-center">
              <WaveformIcon />
            </span>
            <div className="min-w-0">
              <p className="font-semibold">
                {queueLocked ? 'File queue locked during conversion' : 'Add the recording you need to open or convert'}
              </p>
              <p className="type-small text-ink-2">
                {queueLocked
                  ? 'Wait for the current batch to finish before adding or replacing files.'
                  : 'Drop files here, or browse for WAV, MP3, FLAC, SGMCA, TRM, BWF, and supported video containers.'}
              </p>
            </div>
            <Button
              type="button"
              variant="outline"
              className="w-full sm:w-auto"
              aria-label="Add audio files: browse for recordings"
              disabled={queueLocked}
              onClick={browseFiles}
            >
              Browse recordings
            </Button>
          </div>

          {files.length > 0 && (
            <div className="flex flex-col gap-2">
              <div className="flex items-center justify-between">
                <span className="type-small text-ink-2">
                  {files.length} file{files.length !== 1 ? 's' : ''} queued
                </span>
                {!converting && (
                  <button
                    className="type-small text-ink-2 hover:text-foreground transition-colors cursor-pointer"
                    onClick={() => clearAll(converting)}
                  >
                    Clear all
                  </button>
                )}
              </div>
              <div className="flex flex-col border-y border-border divide-y divide-border">
                {files.map(f => (
                  <FileRow
                    key={f.path}
                    file={f}
                    streams={audioStreams[f.path]}
                    sessionNotice={sessionNotices[f.path] || ''}
                    job={jobs[f.path]}
                    onRemove={() => removeFile(f.path, converting)}
                    converting={converting}
                    onOpenInPlayer={onOpenInPlayer}
                  />
                ))}
              </div>
            </div>
          )}

          {unsupportedFiles.length > 0 && (
            <p role="alert" className="notice notice-error">
              Remove{' '}
              {unsupportedFiles.length === 1
                ? unsupportedFiles[0].name
                : `${unsupportedFiles.length} unsupported files`}{' '}
              before converting.
              {unsupportedFiles.map(file => (
                <span key={file.path} className="mt-1 block text-foreground">
                  <span className="font-semibold">{file.name}:</span>{' '}
                  {file.fmt?.note || 'Export it to WAV from the software that recorded it first.'}
                </span>
              ))}
            </p>
          )}

          {queueNotice && (
            <p role="status" className="notice flex items-start gap-2">
              {warnGlyph}
              {queueNotice}
            </p>
          )}

          {conversionError && (
            <p role="alert" className="notice notice-error">
              {conversionError}
            </p>
          )}

          {/* ── PRESETS ──────────────────────────────────────────────────── */}
          {/* Each preset is named for where the file is going and states its
              basis. The one the current settings match is highlighted; when
              none matches, the picker says "Custom" and changes nothing. */}
          <div className="workflow-section zone" data-testid="preset-picker">
            <div className="zone-head">
              <h3 className={title}>Quick setup</h3>
              <span className="type-small text-ink-2" data-testid="preset-status" role="status">
                {presetStatus}
              </span>
              {currentProcessing.length > 0 && !processingQuestion && (
                <Button
                  variant="outline"
                  size="sm"
                  aria-label={`Turn processing off: ${currentProcessing.map(option => option.name).join(', ')}`}
                  onClick={() => turnProcessingOff(currentProcessing)}
                >
                  Turn processing off
                </Button>
              )}
            </div>
            <>
              {processingQuestion && (
                <div
                  role="group"
                  aria-labelledby="preset-processing-question"
                  data-testid="preset-processing-question"
                  className="notice flex flex-col gap-2"
                >
                  <p id="preset-processing-question">
                    {`\u201c${processingQuestion.preset.name}\u201d is for keeping the recording as it is, but processing is on: ${processingQuestion.options.map(option => option.name).join(', ')}. With it on, the archive copy is changed. Nothing has changed yet.`}
                  </p>
                  <div className="flex flex-wrap gap-2">
                    <Button
                      size="sm"
                      onClick={() => {
                        turnProcessingOff(processingQuestion.options)
                        setPresetNow(processingQuestion.preset)
                      }}
                    >
                      Turn processing off and use this preset
                    </Button>
                    <Button size="sm" variant="outline" onClick={() => setPresetNow(processingQuestion.preset)}>
                      Keep processing on and use this preset
                    </Button>
                    <Button size="sm" variant="ghost" onClick={() => setProcessingQuestion(null)}>
                      Cancel
                    </Button>
                  </div>
                </div>
              )}
              <div className="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-3">
                {PRESETS.map(p => {
                  const active = presetInUse?.id === p.id
                  return (
                    <button
                      key={p.id}
                      type="button"
                      title={p.desc}
                      aria-pressed={active}
                      aria-labelledby={`preset-name-${p.id}`}
                      aria-describedby={`preset-basis-${p.id} preset-processing-${p.id} preset-tested-${p.id}`}
                      className="choice"
                      onClick={() => applyPreset(p)}
                    >
                      <span id={`preset-name-${p.id}`} className="type-small font-semibold pr-5">
                        {p.name}
                      </span>
                      <span id={`preset-basis-${p.id}`} className="type-small text-ink-2">
                        {p.basis}
                      </span>
                      <span
                        id={`preset-processing-${p.id}`}
                        data-testid="preset-processing"
                        className="type-small text-ink-2"
                      >
                        {processingLine}
                      </span>
                      <span id={`preset-tested-${p.id}`} className="type-small text-ink-2">
                        {NOT_TESTED_NOTE}
                      </span>
                      {active && (
                        <span aria-hidden="true" className="absolute top-2 right-2 text-foreground">
                          <Check aria-hidden="true" className="h-3.5 w-3.5" strokeWidth={2.2} />
                        </span>
                      )}
                    </button>
                  )
                })}
              </div>
              <p className="type-small text-ink-2">
                Format notes come from Stenograph&rsquo;s Solution Center articles{' '}
                {STENOGRAPH_SOURCES.map((source, index) => (
                  <span key={source.url}>
                    {index > 0 && ' and '}
                    <button
                      type="button"
                      className="underline underline-offset-2 hover:text-foreground cursor-pointer"
                      aria-label={`${source.title} (Stenograph Solution Center, opens in your browser)`}
                      onClick={() => openSource(source)}
                    >
                      &ldquo;{source.title}&rdquo;
                    </button>
                  </span>
                ))}
                . A preset sets only the mode, format and MP3 bitrate. It does not turn processing on or off or change
                the sample rate.
              </p>
              {sourceLinkError && (
                <p role="alert" className="notice notice-error">
                  {sourceLinkError}
                </p>
              )}
            </>
          </div>

          {/* ── OUTPUT MODE (segmented) ──────────────────────────────────── */}
          <div className="workflow-section zone">
            <div className="zone-head">
              <h3 className={title}>Output mode</h3>
            </div>
            <div className="output-mode-grid grid grid-cols-1 gap-2 sm:grid-cols-3">
              {MODES.map(m => (
                <button
                  key={m.id}
                  aria-pressed={mode === m.id}
                  className="choice flex-row items-center gap-3"
                  onClick={() => handleModeChange(m.id)}
                >
                  <ModeIcon id={m.id} active={mode === m.id} />
                  <div className="flex flex-col items-start">
                    <span className="type-small font-semibold">{m.label}</span>
                    <span className="type-small text-ink-2">{m.desc}</span>
                  </div>
                </button>
              ))}
            </div>
          </div>

          {/* ── FORMAT (tiles with plain-English trade-offs) ─────────────── */}
          <div className="workflow-section zone">
            <div className="zone-head">
              <h3 className={title}>Format</h3>
            </div>
            <>
              <div className="format-output-grid grid grid-cols-1 gap-2 min-[420px]:grid-cols-2 sm:grid-cols-3 lg:grid-cols-5">
                {FORMATS_OUT.map(f => (
                  <button
                    key={f.id}
                    aria-pressed={formatOut === f.id}
                    className="choice"
                    onClick={() => setFormatOut(f.id)}
                  >
                    <span className="type-small font-semibold">{f.label}</span>
                    <span className="type-small text-ink-2">{f.desc}</span>
                    {f.preview && <span className="type-small font-semibold text-warning">{f.preview}</span>}
                    {formatOut === f.id && (
                      <span aria-hidden="true" className="absolute top-2 right-2">
                        <Check aria-hidden="true" className="h-3.5 w-3.5" strokeWidth={2.2} />
                      </span>
                    )}
                  </button>
                ))}
              </div>
              {formatOut === 'opus' && (
                <p data-testid="opus-catalyst-note" className="notice flex items-start gap-2">
                  {warnGlyph}
                  {OPUS_CATALYST_NOTE}
                </p>
              )}
              {mode === 'keep' && KEEP_STEREO_ONLY_FORMATS.includes(formatOut) && (
                <p data-testid="keep-stereo-only-note" className="notice flex items-start gap-2">
                  {warnGlyph}
                  {KEEP_STEREO_ONLY_NOTE}
                </p>
              )}
              <div className="flex items-end gap-3 flex-wrap">
                <div className="flex min-w-[130px] flex-col gap-1">
                  <Label htmlFor="sample-rate">Sample rate</Label>
                  <select
                    id="sample-rate"
                    className="h-8 w-full border border-input bg-card px-2 type-small cursor-pointer disabled:border-dashed disabled:text-ink-2"
                    value={effectiveOutputRate(formatOut, rate)}
                    disabled={formatOut in FIXED_OUTPUT_RATES}
                    title={
                      formatOut === 'opus'
                        ? 'Opus is always 48 kHz'
                        : formatOut === 'sgmca'
                          ? 'SGMCA is encoded at 48 kHz'
                          : ''
                    }
                    onChange={e => setRate(e.target.value)}
                  >
                    <option value="22050">22,050 Hz</option>
                    <option value="44100">44,100 Hz</option>
                    <option value="48000">48,000 Hz</option>
                  </select>
                </div>
                {formatOut === 'mp3' && (
                  <div className="flex flex-col gap-1">
                    <Label>MP3 bitrate</Label>
                    <div className="flex gap-0.5 bg-secondary p-0.5">
                      {MP3_BITRATES.map(b => (
                        <button
                          key={b.value}
                          title={b.desc}
                          aria-pressed={mp3Bitrate === b.value}
                          className={`h-7 px-3 type-label cursor-pointer ${mp3Bitrate === b.value ? 'bg-card ring-1 ring-inset ring-foreground' : 'text-ink-2 hover:text-foreground'}`}
                          onClick={() => setMp3Bitrate(b.value)}
                        >
                          {b.label}
                        </button>
                      ))}
                    </div>
                  </div>
                )}
              </div>
              {wavEstimate && <p className="type-small text-ink-2">{wavEstimate}</p>}
            </>
          </div>

          {/* ── CHANNELS (labels + mix) ────────────────────────────────── */}
          {/* Only meaningful once files are queued, and not in keep mode —
              keep mode preserves channels untouched and ignores labels */}
          {files.length > 0 && mode !== 'keep' && (
            <div className="workflow-section zone">
              <div className="zone-head">
                <h3 className={title}>Channels</h3>
                <span className="type-small text-ink-2">
                  {mode === 'stereo' && autoLevel && 'Name channels — volume managed by auto-level'}
                  {mode === 'stereo' && !autoLevel && 'Name channels and adjust mix volumes'}
                  {mode === 'split' && 'Channel names used as output filenames'}
                </span>
              </div>
              <>
                <div className="flex flex-col gap-1">
                  {labels.map((l, i) => (
                    <div key={i} className="channel-row flex items-center gap-2">
                      <span
                        className="w-2 h-2 rounded-full shrink-0"
                        style={{ background: CH_COLORS[i % CH_COLORS.length] }}
                      />
                      <span className="type-min tabular-nums text-ink-2 shrink-0 w-8">CH {i + 1}</span>
                      <Input
                        className="h-7 type-small w-32 shrink-0"
                        value={l}
                        maxLength={24}
                        placeholder={`Channel ${i + 1}`}
                        onChange={e => setLabels(p => p.map((v, j) => (j === i ? e.target.value : v)))}
                      />
                      {mode === 'stereo' && (
                        <div className="channel-mix flex min-w-0 flex-1 items-center gap-2">
                          <input
                            type="range"
                            min="0"
                            max="2"
                            step="0.05"
                            value={autoLevel ? 1.0 : chanVols[i]}
                            aria-label={`Mix volume for channel ${i + 1}`}
                            className={`mix-range h-7 min-w-0 flex-1 ${autoLevel ? '' : 'cursor-pointer'}`}
                            style={{ '--mix-fill': `${((autoLevel ? 1 : chanVols[i]) / 2) * 100}%` }}
                            disabled={autoLevel}
                            onChange={e =>
                              setChanVols(p => p.map((x, j) => (j === i ? parseFloat(e.target.value) : x)))
                            }
                          />
                          <span
                            className={`type-min tabular-nums w-8 text-right shrink-0 ${autoLevel ? '' : 'text-ink-2'}`}
                          >
                            {autoLevel ? 'auto' : chanVols[i] === 0 ? 'mute' : chanVols[i].toFixed(2)}
                          </span>
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              </>
            </div>
          )}

          {/* ── AUDIO PROCESSING (unified panel) ─────────────────────────── */}
          <div className="workflow-section zone">
            <div className="zone-head">
              <h3 className={title}>Audio processing</h3>
              <Button variant="scan" size="sm" onClick={handleScan} disabled={!files.length || scanning || converting}>
                {scanning
                  ? `Scanning${files.length > 1 ? ` (${files.length} files)` : ''}…`
                  : `Scan${files.length > 1 ? ` All (${files.length})` : ''}`}
              </Button>
            </div>
            <div>
              {!analysis && !scanning && !scanError && (
                <p className="pb-2 type-small text-ink-2">
                  Drop files and click <strong className="text-foreground">Scan</strong> to check for level differences
                  and clipping. {SCAN_SAMPLE_NOTE} Scan suggests fixes; nothing changes until you apply them.
                </p>
              )}

              {analysis && !scanning && (
                <p className="pb-2 type-small text-ink-2">{SCAN_SAMPLE_NOTE} Anything after that is not measured.</p>
              )}

              {analysis && !scanning && pendingSuggestions.length > 0 && (
                <div className="flex flex-wrap items-center justify-between gap-2 pb-2">
                  <p className="type-small text-ink-2">
                    Scan suggests {pendingSuggestions.map(s => s.label).join(' and ')}. Nothing is switched on until you
                    apply {pendingSuggestions.length === 1 ? 'it' : 'them'}.
                  </p>
                  <Button variant="outline" size="sm" onClick={applySuggestions} disabled={converting}>
                    Apply suggestions
                  </Button>
                </div>
              )}

              {scanError && !scanning && (
                <p role="alert" className="notice mb-2 flex items-start gap-2">
                  {warnGlyph}
                  {scanError}
                </p>
              )}

              {scanning && (
                <div className="pb-2">
                  <div className="flex items-center justify-between gap-2 mb-1">
                    <p
                      role="status"
                      aria-label="Recording scan status"
                      aria-live="polite"
                      aria-atomic="true"
                      className="type-small text-ink-2 flex items-center gap-1.5 min-w-0"
                    >
                      <Loader2
                        aria-hidden="true"
                        className="animate-spin motion-reduce:animate-none h-3.5 w-3.5 shrink-0"
                      />
                      <span className="shrink-0">
                        Scanning {Math.min(scanProgress.current + 1, scanProgress.total)} of {scanProgress.total}
                      </span>
                      {scanProgress.fileName && (
                        <span className="text-ink-2 truncate max-w-[180px]">— {scanProgress.fileName}</span>
                      )}
                      {scanProgress.phase && (
                        <span className="text-ink-2 truncate hidden sm:inline">
                          · {SCAN_PHASES[scanProgress.phase] || scanProgress.phase}
                        </span>
                      )}
                    </p>
                    <button
                      className="type-small text-ink-2 hover:text-foreground transition-colors cursor-pointer shrink-0"
                      onClick={cancelScan}
                      disabled={scanCancelling}
                    >
                      {scanCancelling ? 'Cancelling…' : scanError ? 'Retry cancel' : 'Cancel'}
                    </button>
                  </div>
                  {scanError && (
                    <p role="alert" className="notice mb-1 flex items-start gap-2">
                      {warnGlyph}
                      {scanError}
                    </p>
                  )}
                  <div
                    role="progressbar"
                    aria-label="Recording scan progress"
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={scanProgressPercent}
                    className="w-full h-1 bg-border overflow-hidden"
                  >
                    <div
                      className="h-full bg-foreground transition-all motion-reduce:transition-none duration-300"
                      style={{
                        width: `${scanProgressPercent}%`,
                      }}
                    />
                  </div>
                </div>
              )}

              {/* ── Every processing option, always shown and off until the
                  person turns it on. A scan only marks what it recommends. ── */}
              <p className={`border-t border-border px-3 pt-4 pb-1 ${title}`}>Signal repair</p>

              <ProcessingToggle
                smart
                name={PROCESSING.autoLevel.name}
                desc={
                  mode === 'keep'
                    ? 'Unavailable while preserving the original channel layout'
                    : PROCESSING.autoLevel.reason
                }
                checked={autoLevelEffective}
                disabled={mode === 'keep'}
                onChange={setAutoLevel}
                detected={
                  analysis?.needsLeveling
                    ? `${analysis.recommendations?.find(r => r.includes('spread'))?.match(/[\d.]+/)?.[0] || ''}dB imbalance found`
                    : null
                }
              />

              <ProcessingToggle
                smart
                name={PROCESSING.declip.name}
                desc={PROCESSING.declip.reason}
                checked={declip}
                onChange={setDeclip}
                detected={analysis?.hasClipping ? 'Clipping found' : null}
              />

              <p className={`px-3 pt-4 pb-1 ${title}`}>Fine-tune</p>

              <ProcessingToggle
                name={PROCESSING.hpf.name}
                desc={PROCESSING.hpf.reason}
                checked={hpf}
                onChange={setHpf}
              />

              <ProcessingToggle
                name={PROCESSING.normalize.name}
                desc={PROCESSING.normalize.reason}
                checked={normalize}
                onChange={setNormalize}
              />

              <ProcessingToggle
                name={PROCESSING.trim.name}
                desc={
                  mode === 'split'
                    ? 'Skipped in Split Channels mode to preserve cross-channel alignment'
                    : PROCESSING.trim.reason
                }
                checked={trim}
                onChange={setTrim}
              />

              <ProcessingToggle
                name={PROCESSING.fade.name}
                desc={PROCESSING.fade.reason}
                checked={fade}
                onChange={setFade}
                extra={
                  fade && (
                    <span className="inline-flex items-center gap-1" onClick={e => e.preventDefault()}>
                      <input
                        type="number"
                        className="h-7 w-12 bg-transparent border border-input px-1 type-small tabular-nums text-center"
                        min="0.1"
                        max="5"
                        step="0.1"
                        value={fadeDur}
                        onChange={e => setFadeDur(Math.max(0.1, parseFloat(e.target.value) || 0.5))}
                      />
                      <span className="type-small text-ink-2">s</span>
                    </span>
                  )
                }
              />

              {/* Processing chain preview */}
              <div className="flex items-center gap-2 px-3 py-2 border-b border-border">
                <span className={`shrink-0 ${title}`}>Chain</span>
                {anyProcessing ? (
                  <div className="flex items-center gap-1 flex-wrap">
                    {/* Leading trim runs first, at the recording's own rate. */}
                    {trimEffective && (
                      <>
                        <Badge variant="default">Leading trim</Badge>
                        <span className="type-small text-ink-2">→</span>
                      </>
                    )}
                    {declip && (
                      <>
                        <Badge variant="info">De-clip</Badge>
                        <span className="type-small text-ink-2">→</span>
                      </>
                    )}
                    {hpf && (
                      <>
                        <Badge variant="default">HPF</Badge>
                        <span className="type-small text-ink-2">→</span>
                      </>
                    )}
                    {autoLevelEffective && (
                      <>
                        <Badge variant="info">Auto-Level</Badge>
                        <span className="type-small text-ink-2">→</span>
                      </>
                    )}
                    {normalize && (
                      <>
                        <Badge variant="default">Normalize</Badge>
                        <span className="type-small text-ink-2">→</span>
                      </>
                    )}
                    {fade && <Badge variant="default">Fade {fadeDur}s</Badge>}
                  </div>
                ) : (
                  <span className="type-small text-ink-2 italic">No processing — direct transcode only</span>
                )}
              </div>

              <p className="px-3 pt-2 type-small text-ink-2">
                All processing runs on your machine — nothing is uploaded or sent anywhere.
              </p>
            </div>
          </div>

          {/* ── DESTINATION ──────────────────────────────────────────────── */}
          <div className="workflow-section zone">
            <div className="zone-head">
              <h3 className={title}>Destination</h3>
            </div>
            <>
              <div className="flex items-end gap-3 flex-wrap">
                <div className="flex flex-1 min-w-[200px] flex-col gap-1">
                  <Label htmlFor="case-name">Case name</Label>
                  <Input
                    id="case-name"
                    value={caseName}
                    placeholder="Auto-detected from filename — override here"
                    onChange={e => setCaseName(e.target.value)}
                  />
                </div>
                <div className="flex flex-1 min-w-[200px] flex-col gap-1">
                  <Label htmlFor="out-dir">Output folder</Label>
                  <div className="flex gap-2">
                    <Input
                      id="out-dir"
                      className="flex-1"
                      value={outDir}
                      placeholder="Default: same folder as source"
                      onChange={e => setOutDir(e.target.value)}
                    />
                    <Button variant="default" onClick={() => browseOutDir(setOutDir)}>
                      Browse
                    </Button>
                  </div>
                </div>
              </div>
            </>
          </div>

          <FormatTable />
        </div>
      </div>

      <footer className="conversion-footer flex shrink-0 items-center justify-between gap-4 border-t border-border bg-card px-4 py-3 sm:px-6 lg:px-[max(2rem,calc((100%-1100px)/2+2rem))]">
        <div
          role="status"
          aria-label="Conversion status"
          aria-live="polite"
          aria-atomic="true"
          className="conversion-footer-status flex min-w-0 items-center gap-2"
        >
          {converting && (
            <Badge variant="active">
              <span
                aria-hidden="true"
                className="w-1.5 h-1.5 rounded-full bg-foreground animate-pulse motion-reduce:animate-none mr-1.5"
              />
              {cancelling ? 'Cancelling…' : doneCount > 0 ? `${doneCount} / ${files.length} done` : 'Converting…'}
            </Badge>
          )}
          {!converting && resultCount > 0 && <Badge variant={resultVariant}>{resultParts.join(', ')}</Badge>}
          {!converting && resultCount === 0 && files.length === 0 && (
            <p className="type-small text-ink-2">Add a recording to choose an output and convert.</p>
          )}
          {!converting && resultCount === 0 && unsupportedFiles.length > 0 && (
            <p className="type-small text-warning">{convertDisabledReason}</p>
          )}
          {!converting && resultCount === 0 && files.length > 0 && unsupportedFiles.length === 0 && (
            <p className="type-small text-ink-2 truncate">
              Ready:{' '}
              <span className="font-semibold text-foreground">
                {formatLabel} · {modeLabel.toLowerCase()}
              </span>
              {procCount > 0 && (
                <>
                  {' '}
                  with {procCount} processing option{procCount !== 1 ? 's' : ''}
                </>
              )}{' '}
              → {outDir || 'same folder as source'}
            </p>
          )}
        </div>
        {converting ? (
          <Button
            variant="destructive"
            size="lg"
            className="conversion-footer-action shrink-0"
            onClick={cancelConversion}
            disabled={cancelling}
          >
            <>
              {cancelling && (
                <Loader2 aria-hidden="true" className="animate-spin motion-reduce:animate-none h-3.5 w-3.5" />
              )}
              {cancelling ? 'Cancelling…' : 'Cancel conversion'}
            </>
          </Button>
        ) : (
          <Button
            variant="primary"
            size="lg"
            className="conversion-footer-action shrink-0"
            onClick={startConversion}
            disabled={queueLocked || !files.length || unsupportedFiles.length > 0}
            title={convertDisabledReason || undefined}
          >
            <Play aria-hidden="true" className="h-3.5 w-3.5 fill-current" />
            Convert{files.length > 1 ? ` ${files.length} Files` : ''}
          </Button>
        )}
      </footer>
    </>
  )
}
