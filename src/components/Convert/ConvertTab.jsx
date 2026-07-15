import { useState, useEffect, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { MODES, FORMATS_OUT, MP3_BITRATES, CH_COLORS } from '../../constants'
import { PRESETS } from '../../presets'
import { usePreferencesContext } from '../../hooks/PreferencesContext'
import { Loader2 } from 'lucide-react'
import { Button } from '../ui/button'
import { Card, CardHeader, CardTitle, CardContent } from '../ui/card'
import { Badge } from '../ui/badge'
import { Input } from '../ui/input'
import { Label } from '../ui/label'
import { ModeIcon, WaveformIcon } from '../common/Icons'
import ProcessingToggle from './ProcessingToggle'
import FormatTable from './FormatTable'
import FileRow from './FileRow'

// The backend emits scan:progress events throughout each file's analysis
// (including ~10s heartbeats while a wedged ffmpeg drains its timeout), so
// staleness — not elapsed time — is the hang signal: a healthy scan of a big
// multichannel file can legitimately run for many minutes, but it never goes
// quiet for long. The stall threshold sits above the backend's longest
// legitimately-silent window (a single blocking model inference / EP compile,
// which cannot heartbeat mid-run). FILE_CAP_MS is the absolute backstop.
const SCAN_STALL_MS = 150000      // no progress event for 150s = stuck
const SCAN_FILE_CAP_MS = 900000   // 15 minutes absolute cap per file

// Human labels for the backend's scan phases
const SCAN_PHASES = {
  probe: 'Reading file info',
  loudness: 'Measuring loudness',
  noise: 'Estimating noise floor',
  speech: 'Detecting speech',
  turns: 'Detecting speaker turns',
  quality: 'Scoring quality',
  speakers: 'Counting speakers',
  done: 'Finishing up',
}

export default function ConvertTab({
  capabilities,
  // Files
  files, dragOver, caseName, setCaseName,
  onDragOver, onDragLeave, onDrop, browseFiles, browseOutDir,
  removeFile, clearAll,
  // Conversion
  jobs, converting, startConversion, doneCount, failCount,
}) {
  const {
    mode, setMode, formatOut, setFormatOut, labels, setLabels,
    chanVols, setChanVols, outDir, setOutDir, rate, setRate,
    mp3Bitrate, setMp3Bitrate,
    normalize, setNormalize, trim, setTrim, fade, setFade,
    fadeDur, setFadeDur, hpf, setHpf,
    denoise, setDenoise, denoiseQuality, setDenoiseQuality,
    autoLevel, setAutoLevel, declip, setDeclip, enhance, setEnhance,
    dereverb, setDereverb,
  } = usePreferencesContext()
  const anyProc = normalize || trim || fade || hpf
  const anyAi = denoise || autoLevel || declip || enhance || dereverb
  const [analysis, setAnalysis] = useState(null)
  const [scanning, setScanning] = useState(false)
  const [scanProgress, setScanProgress] = useState({ current: 0, total: 0, fileName: '', phase: '', filePct: 0 })
  const [scanError, setScanError] = useState(null)
  const [showAllProcessing, setShowAllProcessing] = useState(false)

  // Guards for long-running scans: scanIdRef invalidates a superseded scan
  // (cancel, re-click, unmount), filesRef detects the queue changing mid-scan
  const scanIdRef = useRef(0)
  const filesRef = useRef(files)
  useEffect(() => { filesRef.current = files }, [files])

  // A scan outliving this tab (unmount on tab switch) must stop computing —
  // otherwise it keeps churning invisibly and can flip toggles minutes later
  useEffect(() => () => {
    scanIdRef.current += 1
    invoke('cancel_scan_cmd').catch(() => {})
  }, [])

  // Clear analysis when files change (reset during render, not in an effect)
  const [prevFiles, setPrevFiles] = useState(files)
  if (files !== prevFiles) {
    setPrevFiles(files)
    setAnalysis(null)
    setScanError(null)
    setShowAllProcessing(false)
  }

  const cancelScan = async () => {
    scanIdRef.current += 1
    // Await the round-trip: cancel and analyze are independent IPC requests,
    // and an unawaited epoch bump could land AFTER the next scan snapshots
    // its epoch — instantly killing the wrong scan
    await invoke('cancel_scan_cmd').catch(() => {})
    setScanning(false)
  }

  const handleScan = async () => {
    if (!files.length || scanning) return
    setScanning(true)
    setScanError(null)
    scanIdRef.current += 1
    const scanId = scanIdRef.current
    const scanFiles = files
    setScanProgress({ current: 0, total: scanFiles.length, fileName: '', phase: '', filePct: 0 })

    // Within-file progress from the backend; also feeds the stall watchdog.
    // Filter by path: a just-cancelled file's inference task can emit a few
    // trailing events that must not move the next file's bar.
    let currentPath = null
    let lastEvent = Date.now()
    const unlisten = await listen('scan:progress', ({ payload }) => {
      if (scanIdRef.current !== scanId || payload.path !== currentPath) return
      lastEvent = Date.now()
      setScanProgress(p => ({ ...p, phase: payload.phase, filePct: payload.pct ?? p.filePct }))
    })

    const failed = []
    try {
      // Scan files one at a time to avoid overwhelming ONNX Runtime
      const results = []
      for (let i = 0; i < scanFiles.length; i++) {
        if (scanIdRef.current !== scanId) return // cancelled or superseded
        if (filesRef.current !== scanFiles) {
          // Queue edited mid-scan: stop burning CPU on a stale file list
          invoke('cancel_scan_cmd').catch(() => {})
          setScanError('File list changed during the scan — click Scan to re-analyze.')
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
                invoke('cancel_scan_cmd').catch(() => {}).finally(() =>
                  reject(new Error(stalled ? 'Scan stalled' : 'Scan timed out')))
              }
            }, 5000)
            invoke('analyze_audio_cmd', { path: file.path }).then(
              r => { clearInterval(watchdog); resolve(r) },
              e => { clearInterval(watchdog); reject(e) },
            )
          })
          if (result) results.push(result)
        } catch {
          failed.push(file.name)
        }
        // Guard the post-await write: a cancelled scan settling late must not
        // clobber a successor scan's progress header
        if (scanIdRef.current === scanId) {
          setScanProgress({ current: i + 1, total: scanFiles.length, fileName: file.name, phase: 'done', filePct: 1 })
        }
      }

      // Ignore results that no longer describe the current queue
      if (scanIdRef.current !== scanId) return
      if (filesRef.current !== scanFiles) {
        setScanError('File list changed during the scan — click Scan to re-analyze.')
        return
      }

      // Failures were silent before — a scan where everything timed out
      // looked exactly like a scan that never ran
      if (failed.length > 0) {
        setScanError(
          results.length === 0
            ? `Couldn't analyze ${failed.length === 1 ? failed[0] : `any of the ${failed.length} files`} — the file may need converting before it can be scanned.`
            : `${failed.length} of ${scanFiles.length} files couldn't be analyzed and were skipped.`
        )
      }

      if (results.length > 0) {
        // Aggregate: use worst-case across all files
        // speechRatio is absent when the VAD model isn't installed — average
        // only real values so a missing model doesn't read as "0% speech"
        const speechRatios = results.filter(r => r.speechRatio != null).map(r => r.speechRatio)
        const aggregated = {
          ...results[0],
          needsDenoise: results.some(r => r.needsDenoise),
          needsLeveling: results.some(r => r.needsLeveling),
          hasClipping: results.some(r => r.hasClipping),
          isNarrowband: results.some(r => r.isNarrowband),
          recommendations: [...new Set(results.flatMap(r => r.recommendations || []))],
          qualityScore: results[0].qualityScore,
          speakerCount: Math.max(...results.map(r => r.speakerCount || 0)) || null,
          speechRatio: speechRatios.length
            ? speechRatios.reduce((sum, r) => sum + r, 0) / speechRatios.length
            : null,
        }
        setAnalysis(aggregated)

        // Only enable processing that the scan actually recommends
        if (aggregated.needsDenoise) setDenoise(true)
        if (aggregated.needsLeveling) setAutoLevel(true)
        if (aggregated.hasClipping) setDeclip(true)
        if (aggregated.isNarrowband) setEnhance(true)
        // Trim silence only if there's significant dead air
        if (aggregated.speechRatio != null && aggregated.speechRatio < 0.5) setTrim(true)
      }
    } catch (e) {
      console.error('Scan failed:', e)
    } finally {
      unlisten()
      // A cancelled scan already reset the button via cancelScan
      if (scanIdRef.current === scanId) setScanning(false)
    }
  }

  // Guided stepper: reflects real state without gating anything — batch users
  // can still work the whole page at once.
  const step = converting ? 3 : files.length > 0 ? 2 : 1
  const stepDone = (n) => n < step || (n === 3 && !converting && doneCount > 0 && files.length > 0)
  const procCount = [denoise, autoLevel, declip, enhance, dereverb, hpf, normalize, trim, fade].filter(Boolean).length
  const formatLabel = FORMATS_OUT.find(f => f.id === formatOut)?.label || formatOut
  const modeLabel = MODES.find(m => m.id === mode)?.label || mode

  return (
    <>
      <div className="flex-1 overflow-y-auto overflow-x-hidden scrollbar-thin scrollbar-thumb-border scrollbar-track-transparent">
        <div className="w-full max-w-[1100px] mx-auto px-5 md:px-8 py-5 flex flex-col gap-3.5">

          {/* ── STEPPER (state-driven, purely informational) ─────────────── */}
          <ol className="flex items-center gap-2.5 mb-1" aria-label="Conversion progress">
            {['Add recording', 'Choose settings', 'Convert'].map((label, i) => {
              const n = i + 1
              const done = stepDone(n)
              const now = !done && n === step
              return (
                <li key={label} className="flex items-center gap-2.5" aria-current={now ? 'step' : undefined}>
                  <span aria-hidden="true" className={`w-6 h-6 rounded-full grid place-items-center text-[11px] font-semibold border transition-colors ${
                    done ? 'bg-[hsl(var(--success))] border-[hsl(var(--success))] text-white'
                    : now ? 'bg-primary border-primary text-primary-foreground'
                    : 'bg-card border-border text-[hsl(var(--sub))]'}`}>
                    {done ? '✓' : n}
                  </span>
                  <span className={`text-[12px] ${now ? 'font-semibold text-foreground' : done ? 'text-[hsl(var(--text2))]' : 'text-[hsl(var(--sub))]'}`}>{label}</span>
                  {n < 3 && <span aria-hidden="true" className={`w-9 h-px ${done ? 'bg-[hsl(var(--success))]' : 'bg-border'}`} />}
                </li>
              )
            })}
          </ol>

          {/* ── FILES (drop zone + queue) — the workflow starts here ─────── */}
          <div
            role="button"
            tabIndex={0}
            aria-label="Add audio files: drop them here or press Enter to browse"
            className={`flex flex-col items-center justify-center gap-2 py-8 px-6 border-2 border-dashed rounded-xl cursor-pointer transition-colors focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring ${dragOver ? 'border-primary bg-[hsl(var(--gold-dim))]' : 'border-border/60 hover:border-border hover:bg-secondary/30'}`}
            onDragOver={onDragOver} onDragLeave={onDragLeave} onDrop={onDrop}
            onClick={browseFiles}
            onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); browseFiles() } }}>
            <WaveformIcon />
            <p className="text-[13px] font-semibold text-foreground">Drop audio or video files here</p>
            <p className="text-[11px] text-[hsl(var(--sub))] text-center">or <span className="text-foreground cursor-pointer hover:underline">click to browse</span> — MP3 · WAV · FLAC · M4A · OGG · Opus · WMA · court formats (SGMCA · TRM · BWF) · video (MP4 · MOV · MKV)</p>
          </div>

          {files.length > 0 && (
            <div className="flex flex-col gap-2">
              <div className="flex items-center justify-between">
                <span className="font-mono text-[11px] text-[hsl(var(--sub))]">{files.length} file{files.length!==1?'s':''} queued</span>
                {!converting && <button className="text-[11px] text-[hsl(var(--sub))] hover:text-foreground transition-colors cursor-pointer" onClick={() => clearAll(converting)}>Clear all</button>}
              </div>
              <div className="flex flex-col gap-1.5">
                {files.map(f => <FileRow key={f.path} file={f} job={jobs[f.path]} onRemove={() => removeFile(f.path, converting)} converting={converting} />)}
              </div>
            </div>
          )}

          {/* ── PRESETS ──────────────────────────────────────────────────── */}
          {/* Highlight the preset the current settings match, so users can
              see which preset is active and when they've diverged from it */}
          <div className="flex items-center gap-1.5 flex-wrap mb-2">
            <Label>PRESET</Label>
            {PRESETS.map(p => {
              const s = p.settings
              // Opus always converts at 48 kHz, so compare the effective rate
              const effRate = formatOut === 'opus' ? '48000' : rate
              const sEffRate = s.format === 'opus' ? '48000' : s.rate
              const active = mode === s.mode && formatOut === s.format && effRate === sEffRate
                && normalize === s.normalize && trim === s.trim && fade === s.fade
                && fadeDur === s.fadeDur && hpf === s.hpf
                && denoise === s.denoise && denoiseQuality === s.denoiseQuality
                && autoLevel === s.autoLevel && declip === s.declip
                && enhance === s.enhance && dereverb === s.dereverb
                // Bitrate only matters for MP3; presets default to 192 kbps
                && (s.format !== 'mp3' || mp3Bitrate === (s.mp3Bitrate ?? 192))
              return (
                <Button key={p.id} variant="outline" size="sm" title={p.desc}
                  aria-pressed={active}
                  className={`rounded-full ${active ? 'border-primary bg-[hsl(var(--gold-dim))] text-foreground' : ''}`}
                  onClick={() => {
                    setMode(s.mode); setFormatOut(s.format); setRate(s.rate)
                    setNormalize(s.normalize); setTrim(s.trim); setFade(s.fade)
                    setFadeDur(s.fadeDur); setHpf(s.hpf)
                    setDenoise(s.denoise); setDenoiseQuality(s.denoiseQuality)
                    setAutoLevel(s.autoLevel); setDeclip(s.declip)
                    setEnhance(s.enhance); setDereverb(s.dereverb)
                    if (s.format === 'mp3') setMp3Bitrate(s.mp3Bitrate ?? 192)
                  }}>
                  {p.name}
                </Button>
              )
            })}
          </div>

          {/* ── OUTPUT MODE (segmented) ──────────────────────────────────── */}
          <Card>
            <CardHeader><CardTitle>OUTPUT MODE</CardTitle></CardHeader>
            <CardContent>
              <div className="grid grid-cols-3 gap-2 p-2 m-2 rounded-lg bg-secondary/60">
                {MODES.map(m => (
                  <button key={m.id}
                    aria-pressed={mode===m.id}
                    className={`flex items-center gap-3 p-2.5 rounded-md transition-all cursor-pointer focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring ${mode===m.id ? 'bg-card shadow-sm ring-1 ring-border' : 'hover:bg-card/50'}`}
                    onClick={() => setMode(m.id)}>
                    <ModeIcon id={m.id} active={mode===m.id} />
                    <div className="flex flex-col items-start">
                      <span className="text-[12px] font-semibold text-foreground">{m.label}</span>
                      <span className="text-[10px] text-[hsl(var(--text2))] leading-tight">{m.desc}</span>
                    </div>
                  </button>
                ))}
              </div>
            </CardContent>
          </Card>

          {/* ── FORMAT (tiles with plain-English trade-offs) ─────────────── */}
          <Card>
            <CardHeader><CardTitle>FORMAT</CardTitle></CardHeader>
            <CardContent>
              <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-2 p-3">
                {FORMATS_OUT.map(f => (
                  <button key={f.id}
                    aria-pressed={formatOut===f.id}
                    className={`relative flex flex-col items-start gap-0.5 px-3 py-2.5 rounded-lg border text-left transition-all cursor-pointer focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring ${
                      formatOut===f.id
                        ? 'border-primary bg-[hsl(var(--gold-dim))] shadow-sm'
                        : 'border-border bg-card hover:border-[hsl(var(--sub))]'}`}
                    onClick={() => setFormatOut(f.id)}>
                    <span className="text-[13px] font-bold text-foreground">{f.label}</span>
                    <span className="text-[10px] text-[hsl(var(--text2))] leading-tight">{f.desc}</span>
                    {formatOut===f.id && <span aria-hidden="true" className="absolute top-1.5 right-2 text-foreground text-[11px]">✓</span>}
                  </button>
                ))}
              </div>
              <div className="flex items-end gap-3 flex-wrap px-3 pb-3">
                <div className="min-w-[130px]">
                  <Label className="mb-1 block" htmlFor="sample-rate">SAMPLE RATE</Label>
                  <select id="sample-rate" className="flex h-8 w-full rounded-lg border border-border bg-card px-2.5 py-1.5 text-xs text-foreground transition-colors focus:outline-hidden focus:border-primary cursor-pointer"
                    value={formatOut === 'opus' ? '48000' : rate}
                    disabled={formatOut === 'opus'}
                    title={formatOut === 'opus' ? 'Opus is always 48 kHz' : ''}
                    onChange={e => setRate(e.target.value)}>
                    <option value="22050">22,050 Hz</option>
                    <option value="44100">44,100 Hz</option>
                    <option value="48000">48,000 Hz</option>
                  </select>
                </div>
                {formatOut === 'mp3' && (
                  <div>
                    <Label className="mb-1 block">MP3 BITRATE</Label>
                    <div className="flex gap-px bg-secondary rounded-md p-0.5">
                      {MP3_BITRATES.map(b => (
                        <button key={b.value} title={b.desc}
                          aria-pressed={mp3Bitrate===b.value}
                          className={`px-2.5 py-1.5 text-[11px] font-semibold rounded-md transition-colors cursor-pointer focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring ${mp3Bitrate===b.value ? 'bg-card text-foreground shadow-xs' : 'text-[hsl(var(--sub))] hover:text-[hsl(var(--text2))]'}`}
                          onClick={() => setMp3Bitrate(b.value)}>{b.label}</button>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            </CardContent>
          </Card>

          {/* ── CHANNELS (labels + mix) ────────────────────────────────── */}
          {/* Only meaningful once files are queued, and not in keep mode —
              keep mode preserves channels untouched and ignores labels */}
          {files.length > 0 && mode !== 'keep' && (
          <Card>
            <CardHeader>
              <CardTitle>CHANNELS</CardTitle>
              <span className="text-[10px] text-[hsl(var(--sub))]">
                {mode === 'stereo' && autoLevel && 'Name channels — volume managed by auto-level'}
                {mode === 'stereo' && !autoLevel && 'Name channels and adjust mix volumes'}
                {mode === 'split' && 'Channel names used as output filenames'}
              </span>
            </CardHeader>
            <CardContent>
              <div className="flex flex-col gap-1 p-3">
                {labels.map((l,i) => (
                  <div key={i} className="flex items-center gap-2">
                    <span className="w-2 h-2 rounded-full shrink-0" style={{background:CH_COLORS[i % CH_COLORS.length]}} />
                    <span className="font-mono text-[10px] text-[hsl(var(--sub))] shrink-0 w-7">CH {i+1}</span>
                    <Input className="h-7 text-[11px] w-32 shrink-0" value={l} maxLength={24} placeholder={`Channel ${i+1}`}
                      onChange={e => setLabels(p => p.map((v,j) => j===i ? e.target.value:v))} />
                    {mode === 'stereo' && (
                      <>
                        <input type="range" min="0" max="2" step="0.05" value={autoLevel ? 1.0 : chanVols[i]}
                          aria-label={`Mix volume for channel ${i+1}`}
                          className={`flex-1 h-1 accent-primary cursor-pointer ${autoLevel ? 'opacity-40' : ''}`}
                          disabled={autoLevel}
                          onChange={e => setChanVols(p => p.map((x,j) => j===i ? parseFloat(e.target.value):x))} />
                        <span className={`font-mono text-[10px] w-8 text-right shrink-0 ${autoLevel ? 'text-foreground' : 'text-[hsl(var(--sub))]'}`}>
                          {autoLevel ? 'auto' : chanVols[i]===0 ? 'mute' : chanVols[i].toFixed(2)}
                        </span>
                      </>
                    )}
                  </div>
                ))}
              </div>
            </CardContent>
          </Card>
          )}

          {/* ── AUDIO PROCESSING (unified panel) ─────────────────────────── */}
          <Card>
            <CardHeader>
              <CardTitle>AUDIO PROCESSING</CardTitle>
              <Button variant="scan" size="sm" onClick={handleScan}
                disabled={!files.length || scanning || converting}>
                {scanning ? `Scanning${files.length > 1 ? ` (${files.length} files)` : ''}…` : `Scan${files.length > 1 ? ` All (${files.length})` : ''}`}
              </Button>
            </CardHeader>
            <CardContent>
              {!analysis && !scanning && !scanError && (
                <p className="px-4 py-2.5 text-[11px] text-[hsl(var(--sub))]">Drop files and click <strong className="text-foreground">Scan</strong> to detect issues and auto-enable the right fixes.</p>
              )}

              {scanError && !scanning && (
                <p role="alert" className="px-4 py-2.5 text-[11px] text-[hsl(var(--warning))]">{scanError}</p>
              )}

              {scanning && (
                <div className="px-4 py-2.5">
                  <div className="flex items-center justify-between gap-2 mb-1.5">
                    <p className="text-[11px] text-[hsl(var(--sub))] flex items-center gap-1.5 min-w-0">
                      <Loader2 className="animate-spin h-3.5 w-3.5 shrink-0" />
                      <span className="shrink-0">Scanning {Math.min(scanProgress.current + 1, scanProgress.total)} of {scanProgress.total}</span>
                      {scanProgress.fileName && <span className="text-[hsl(var(--text2))] truncate max-w-[180px]">— {scanProgress.fileName}</span>}
                      {scanProgress.phase && <span className="text-[hsl(var(--sub))] truncate hidden sm:inline">· {SCAN_PHASES[scanProgress.phase] || scanProgress.phase}</span>}
                    </p>
                    <button className="text-[11px] text-[hsl(var(--sub))] hover:text-foreground transition-colors cursor-pointer shrink-0" onClick={cancelScan}>Cancel</button>
                  </div>
                  <div className="w-full h-1 bg-border rounded-full overflow-hidden">
                    <div className="h-full bg-primary rounded-full transition-all duration-300" style={{width: `${scanProgress.total > 0 ? ((scanProgress.current + (scanProgress.filePct || 0)) / scanProgress.total) * 100 : 0}%`}} />
                  </div>
                </div>
              )}

              {analysis && (
                <div className="flex flex-wrap gap-1.5 px-4 py-2.5">
                  {analysis.qualityScore && (
                    <Badge variant="outline" title={`Signal: ${analysis.qualityScore.sig?.toFixed(1)} · Background: ${analysis.qualityScore.bak?.toFixed(1)}`}>
                      Quality: {analysis.qualityScore.ovr?.toFixed(1)}/5
                    </Badge>
                  )}
                  {analysis.speakerCount != null && (
                    <Badge variant="outline">
                      {analysis.speakerCount} speaker{analysis.speakerCount !== 1 ? 's' : ''} detected
                    </Badge>
                  )}
                  {analysis.turns?.length > 0 && (
                    <Badge variant="outline">
                      {analysis.turns.length} turn{analysis.turns.length !== 1 ? 's' : ''} found
                    </Badge>
                  )}
                  {analysis.speechRatio != null && (
                    <Badge variant="outline">
                      {Math.round(analysis.speechRatio * 100)}% speech
                    </Badge>
                  )}
                </div>
              )}

              {/* ── Recommended (shown after scan, or all if no scan) ── */}
              {(() => {
                const hasAnalysis = !!analysis
                // After scan: only show toggles that are recommended or already enabled
                const showDenoise   = !hasAnalysis || analysis.needsDenoise || denoise || showAllProcessing
                const showAutoLevel = !hasAnalysis || analysis.needsLeveling || autoLevel || showAllProcessing
                const showDeclip    = !hasAnalysis || analysis.hasClipping || declip || showAllProcessing
                const showEnhance   = !hasAnalysis || analysis.isNarrowband || enhance || showAllProcessing
                const showDereverb  = !hasAnalysis || dereverb || showAllProcessing
                const showHpf       = !hasAnalysis || hpf || showAllProcessing
                const showNormalize = !hasAnalysis || normalize || showAllProcessing
                const showTrim      = !hasAnalysis || (analysis.speechRatio != null && analysis.speechRatio < 0.5) || trim || showAllProcessing
                const showFade      = !hasAnalysis || fade || showAllProcessing

                const hiddenCount = hasAnalysis && !showAllProcessing
                  ? [showDenoise, showAutoLevel, showDeclip, showEnhance, showDereverb, showHpf, showNormalize, showTrim, showFade].filter(v => !v).length
                  : 0

                return <>
                  {(showDenoise || showAutoLevel || showDeclip || showEnhance || showDereverb) && (
                    <div className="px-4 py-1.5 bg-secondary/50">
                      <span className="font-mono text-[9px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))]">
                        {hasAnalysis && !showAllProcessing ? 'RECOMMENDED' : 'SMART'}
                      </span>
                    </div>
                  )}

                  {showDenoise && <ProcessingToggle smart name="Remove Background Noise"
                    desc="Cleans up HVAC hum, paper rustling, and room noise"
                    checked={denoise} onChange={setDenoise}
                    detected={analysis?.needsDenoise ? 'Noise detected' : null}
                    extra={denoise && (
                      <span className="inline-flex items-center gap-1 mt-0.5" onClick={e => e.preventDefault()}>
                        <span className="text-[hsl(var(--sub))]">—</span>
                        <select className="font-mono text-[10px] bg-secondary border border-border rounded px-1 py-px text-[hsl(var(--text2))] cursor-pointer"
                          aria-label="Denoise quality"
                          value={denoiseQuality} onChange={e => { e.stopPropagation(); setDenoiseQuality(e.target.value) }}>
                          <option value="fast">Fast</option>
                          <option value="best">Best quality</option>
                        </select>
                      </span>
                    )}
                  />}

                  {showAutoLevel && <ProcessingToggle smart name="Balance Speaker Volume"
                    desc="Evens out volume so quiet speakers are easier to hear"
                    checked={autoLevel} onChange={v => { setAutoLevel(v); }}
                    detected={analysis?.needsLeveling ? `${analysis.recommendations?.find(r => r.includes('spread'))?.match(/[\d.]+/)?.[0] || ''}dB imbalance found` : null}
                  />}

                  {showDeclip && <ProcessingToggle smart name="Fix Clipped Audio"
                    desc="Repairs distorted peaks from recordings that were too loud"
                    checked={declip} onChange={setDeclip}
                    detected={analysis?.hasClipping ? 'Clipping found' : null}
                  />}

                  {showEnhance && <ProcessingToggle smart name="Enhance Clarity"
                    desc="Improves phone recordings and narrow-band audio"
                    checked={enhance} onChange={setEnhance}
                    detected={analysis?.isNarrowband ? `${analysis.sampleRate?.toLocaleString() || ''}Hz detected` : null}
                  />}

                  {/* The DCCRN+ model is optional and self-exported; hide the
                      toggle when it isn't installed so the switch isn't a lie */}
                  {showDereverb && capabilities?.dereverbAvailable && <ProcessingToggle smart name="Reduce Room Echo"
                    desc="Removes reverb from large rooms or hallways"
                    checked={dereverb} onChange={setDereverb}
                  />}

                  {/* ── Fine-tune ── */}
                  {(showHpf || showNormalize || showTrim || showFade) && (
                    <div className="px-4 py-1.5 bg-secondary/50">
                      <span className="font-mono text-[9px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))]">FINE-TUNE</span>
                    </div>
                  )}

                  {showHpf && <ProcessingToggle name="High-Pass Filter"
                    desc="80 Hz cutoff — removes low rumble and handling noise"
                    checked={hpf} onChange={setHpf}
                  />}

                  {showNormalize && <ProcessingToggle name="Normalize Volume"
                    desc="Targets –16 LUFS / –1.5 TP for consistent output level"
                    checked={normalize} onChange={setNormalize}
                  />}

                  {showTrim && <ProcessingToggle name="Trim Silence"
                    desc="Remove dead air at start and end (below –50 dB)"
                    checked={trim} onChange={setTrim}
                  />}

                  {showFade && <ProcessingToggle name="Fade In / Out"
                    desc="Smooth start and end"
                    checked={fade} onChange={setFade}
                    extra={fade && (
                      <span className="inline-flex items-center gap-1 mt-0.5" onClick={e => e.preventDefault()}>
                        <span className="text-[hsl(var(--sub))]">—</span>
                        <input type="number" className="w-[42px] bg-secondary border border-border rounded px-1 py-px font-mono text-[11px] text-foreground text-center focus:border-primary outline-hidden"
                          min="0.1" max="5" step="0.1" value={fadeDur}
                          onChange={e => setFadeDur(Math.max(0.1, parseFloat(e.target.value)||0.5))} />
                        <span className="text-[11px] text-[hsl(var(--sub))]">s</span>
                      </span>
                    )}
                  />}

                  {/* Show more / less toggle */}
                  {hasAnalysis && hiddenCount > 0 && !showAllProcessing && (
                    <button className="px-4 py-2 text-[11px] text-foreground hover:text-foreground transition-colors cursor-pointer text-left"
                      onClick={() => setShowAllProcessing(true)}>
                      Show {hiddenCount} more option{hiddenCount !== 1 ? 's' : ''}...
                    </button>
                  )}
                  {hasAnalysis && showAllProcessing && (
                    <button className="px-4 py-2 text-[11px] text-[hsl(var(--sub))] hover:text-foreground transition-colors cursor-pointer text-left"
                      onClick={() => setShowAllProcessing(false)}>
                      Show less
                    </button>
                  )}
                </>
              })()}

              {/* Processing chain preview */}
              <div className="flex items-center gap-2 px-4 py-2.5 border-t border-border/60 bg-secondary/30">
                <span className="font-mono text-[9px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))] shrink-0">CHAIN</span>
                {(anyProc || anyAi) ? (
                  <div className="flex items-center gap-1 flex-wrap">
                    {denoise   && <><Badge variant="info">Denoise</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {dereverb  && <><Badge variant="info">De-reverb</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {enhance   && <><Badge variant="info">Enhance</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {declip    && <><Badge variant="info">De-clip</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {hpf       && <><Badge variant="default">HPF</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {autoLevel && <><Badge variant="info">Auto-Level</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {normalize && <><Badge variant="default">Normalize</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {trim      && <><Badge variant="default">Trim</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {fade      && <Badge variant="default">Fade {fadeDur}s</Badge>}
                  </div>
                ) : (
                  <span className="text-[10px] text-[hsl(var(--sub))] italic">No processing — direct transcode only</span>
                )}
              </div>

              <p className="px-4 py-2 text-[10px] text-[hsl(var(--sub))] border-t border-border/60">
                All processing runs on your machine — nothing is uploaded or sent anywhere.
                {capabilities && (
                  <span className="opacity-60" title={`${capabilities.cpuCores} cores · ${Math.round(capabilities.ramMb/1024)}GB RAM${capabilities.appleSilicon ? ' · Apple Silicon' : ''}`}>
                    {' '}· {capabilities.tier === 'high' ? 'High performance' : capabilities.tier === 'mid' ? 'Standard performance' : 'Lightweight mode'}
                  </span>
                )}
              </p>
            </CardContent>
          </Card>

          {/* ── DESTINATION ──────────────────────────────────────────────── */}
          <Card>
            <CardHeader><CardTitle>DESTINATION</CardTitle></CardHeader>
            <CardContent>
              <div className="flex items-end gap-3 flex-wrap p-3">
                <div className="flex-1 min-w-[200px]">
                  <Label className="mb-1 block" htmlFor="case-name">CASE NAME</Label>
                  <Input id="case-name" value={caseName} placeholder="Auto-detected from filename — override here"
                    onChange={e => setCaseName(e.target.value)} />
                </div>
                <div className="flex-1 min-w-[200px]">
                  <Label className="mb-1 block" htmlFor="out-dir">OUTPUT FOLDER</Label>
                  <div className="flex gap-1.5">
                    <Input id="out-dir" className="flex-1" value={outDir} placeholder="Default: same folder as source"
                      onChange={e => setOutDir(e.target.value)} />
                    <Button variant="default" size="sm" onClick={() => browseOutDir(setOutDir)}>Browse</Button>
                  </div>
                </div>
              </div>
            </CardContent>
          </Card>

          <FormatTable />

        </div>
      </div>

      <footer className="flex items-center justify-between gap-4 px-7 py-3 border-t border-border/60 bg-card shrink-0">
        <div className="flex items-center gap-2 min-w-0">
          {converting && (
            <Badge variant="active">
              <span className="w-1.5 h-1.5 rounded-full bg-primary animate-pulse mr-1.5" />
              {doneCount > 0 ? `${doneCount} / ${files.length} done` : 'Converting…'}
            </Badge>
          )}
          {!converting && doneCount > 0 && (
            <Badge variant="done">
              ✓ {doneCount} file{doneCount!==1?'s':''} converted{failCount>0?`, ${failCount} failed`:''}
            </Badge>
          )}
          {!converting && doneCount === 0 && files.length > 0 && (
            <p className="text-[12px] text-[hsl(var(--sub))] truncate">
              Ready: <span className="font-semibold text-foreground">{formatLabel} · {modeLabel.toLowerCase()}</span>
              {procCount > 0 && <> with {procCount} enhancement{procCount !== 1 ? 's' : ''}</>}
              {' '}→ {outDir || 'same folder as source'}
            </p>
          )}
        </div>
        <Button variant="primary" size="lg" className="shrink-0"
          onClick={startConversion} disabled={converting||!files.length}>
          {converting ? <><Loader2 className="animate-spin h-3.5 w-3.5" />Converting…</> : <>▶ Convert{files.length > 1 ? ` ${files.length} Files` : ''}</>}
        </Button>
      </footer>
    </>
  )
}
