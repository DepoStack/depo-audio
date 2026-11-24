import { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { open } from '@tauri-apps/plugin-dialog'
import { Loader2, X } from 'lucide-react'
import { cn } from '../../lib/utils'
import { FORMATS_OUT, CH_COLORS } from '../../constants'
import MERGE_FORMATS from '../../mergeFormats.json'
import { basename, fmtSize, fmtTime, sortRecordingChunks } from '../../utils'
import { Button } from '../ui/button'
import { Card, CardHeader, CardTitle, CardContent } from '../ui/card'
import { Input } from '../ui/input'
import { Label } from '../ui/label'
import { Badge } from '../ui/badge'

/** The job takes 2 to 4 recordings, each up to 30 minutes (merge_engine.rs). */
export const MAX_MERGE_SOURCES = 4
export const MAX_MERGE_MINUTES = 30
const MERGE_CANCELLED = 'Merge cancelled'
const STAGE_LABELS = {
  preparing: 'Reading',
  matching: 'Matching',
  decoding: 'Decoding',
  merging: 'Merging',
  encoding: 'Saving',
  done: 'Saving',
}

/** Message for a recording over the length limit, naming it. */
export function overLengthMessage(name) {
  return `"${name}" is longer than ${MAX_MERGE_MINUTES} minutes. Each recording can be up to ${MAX_MERGE_MINUTES} minutes.`
}

/**
 * Add picked paths to the list: skips any already in it and stops at the
 * four-source limit. Returns the new list, the sources added, and a notice
 * for what was left out.
 */
export function addMergeSources(current, paths) {
  const known = new Set(current.map(s => s.path))
  const added = []
  const duplicates = []
  let overLimit = 0
  for (const path of paths) {
    if (known.has(path)) {
      duplicates.push(basename(path))
    } else if (current.length + added.length >= MAX_MERGE_SOURCES) {
      overLimit += 1
    } else {
      known.add(path)
      added.push({ path, name: basename(path) })
    }
  }
  const notices = []
  if (duplicates.length) {
    notices.push(`${duplicates.join(', ')} ${duplicates.length === 1 ? 'is' : 'are'} already in the list.`)
  }
  if (overLimit) {
    notices.push(
      `Merge takes up to ${MAX_MERGE_SOURCES} recordings, so ${overLimit} more ${overLimit === 1 ? 'was' : 'were'} not added.`,
    )
  }
  return { sources: [...current, ...added], added, notice: notices.join(' ') }
}

export default function MergeTab() {
  const [sources, setSources] = useState([])
  // Next to the reference recording unless a folder is chosen; the merge
  // itself runs at 48 kHz.
  const [outDir, setOutDir] = useState('')
  const rate = '48000'
  const [outName, setOutName] = useState('')
  const [format, setFormat] = useState('wav')
  const [strategy, setStrategy] = useState('loudest')
  const [syncing, setSyncing] = useState(false)
  const [merging, setMerging] = useState(false)
  const [cancelling, setCancelling] = useState(false)
  const [progress, setProgress] = useState(null)
  const [syncResults, setSyncResults] = useState([])
  const [result, setResult] = useState(null)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  const tooLong = sources.filter(s => s.durationSecs > MAX_MERGE_MINUTES * 60)
  const atLimit = sources.length >= MAX_MERGE_SOURCES
  const busy = syncing || merging

  // Each recording's length, read before Start so one over the limit is
  // named here rather than refused after decoding has begun. A length that
  // can't be read is left for the merge itself to check.
  const checkLengths = async added => {
    for (const source of added) {
      let durationSecs = null
      try {
        const info = await invoke('probe_audio_streams_cmd', { path: source.path })
        durationSecs = info?.durationSecs ?? null
      } catch {
        durationSecs = null
      }
      if (!mounted.current) return
      setSources(prev => prev.map(s => (s.path === source.path ? { ...s, durationSecs } : s)))
    }
  }

  const browseFiles = async () => {
    try {
      const selected = await open({
        multiple: true,
        filters: [
          {
            name: 'Audio',
            extensions: [
              'wav',
              'mp3',
              'flac',
              'opus',
              'ogg',
              'm4a',
              'aac',
              'wma',
              'aif',
              'aiff',
              'caf',
              'amr',
              'sgmca',
              'trm',
              'ftr',
              'tll',
              'bwf',
            ],
          },
          { name: 'Video (audio extracted)', extensions: ['mp4', 'mov', 'mkv', 'avi', 'webm'] },
        ],
      })
      if (selected) {
        const paths = (Array.isArray(selected) ? selected : [selected]).map(p => (typeof p === 'string' ? p : p.path))
        const next = addMergeSources(sources, paths)
        // Order FTR session chunks chronologically ACROSS batches: source
        // order is load-bearing here — sources[0] becomes the sync Reference,
        // so a second "Add Files" must not leave a later chunk in front.
        // (Mixed/non-chunk source sets keep their order.)
        setSources(sortRecordingChunks(next.sources, s => s.path))
        setNotice(next.notice)
        setSyncResults([]) // sync is stale once the source set changes
        setResult(null)
        setError('')
        checkLengths(next.added)
      }
    } catch (error) {
      setError(`Could not open the audio picker: ${String(error)}`)
    }
  }

  const removeSource = idx => {
    setSources(prev => prev.filter((_, i) => i !== idx))
    setSyncResults([])
    setResult(null)
    setNotice('')
  }

  const chooseFolder = async () => {
    try {
      const folder = await open({ directory: true, multiple: false })
      if (typeof folder === 'string') setOutDir(folder)
    } catch (error) {
      setError(`Could not open the folder picker: ${String(error)}`)
    }
  }

  const cancel = async () => {
    setCancelling(true)
    try {
      await invoke('merge_cancel_cmd')
    } catch (e) {
      setError(String(e))
      setCancelling(false)
    }
  }

  // "Merge cancelled" is the answer to Cancel, not an error.
  const showFailure = e => {
    const message = String(e)
    if (message.includes(MERGE_CANCELLED)) {
      setNotice('Cancelled. Nothing was saved.')
    } else {
      setError(message)
    }
  }

  const handleSync = async () => {
    if (sources.length < 2 || busy) return
    setSyncing(true)
    setCancelling(false)
    setError('')
    setNotice('')
    try {
      const results = []
      for (let i = 1; i < sources.length; i++) {
        const sync = await invoke('detect_sync_cmd', {
          sourceA: sources[0].path,
          sourceB: sources[i].path,
        })
        results.push(sync)
      }
      setSyncResults(results)
    } catch (e) {
      showFailure(e)
    }
    setSyncing(false)
    setCancelling(false)
  }

  const handleMerge = async () => {
    if (sources.length < 2 || busy || tooLong.length) return
    setMerging(true)
    setCancelling(false)
    setError('')
    setNotice('')
    setResult(null)
    setProgress({ stage: 'preparing', percent: 0 })
    let unlisten = null
    try {
      unlisten = await listen('merge:progress', ({ payload }) => {
        if (mounted.current) setProgress(payload)
      })
      const res = await invoke('merge_audio_cmd', {
        job: {
          sources: sources.map(s => s.path),
          outDir,
          outName: outName || 'merged',
          format,
          rate,
          strategy,
        },
      })
      setResult(res)
    } catch (e) {
      showFailure(e)
    } finally {
      if (unlisten) unlisten()
    }
    setMerging(false)
    setCancelling(false)
    setProgress(null)
  }

  return (
    <>
      <div className="flex-1 overflow-y-auto overflow-x-hidden">
        <div className="w-full max-w-[1100px] mx-auto px-5 md:px-8 py-5 flex flex-col gap-3.5">
          {/* ── Source Files ──────────────────────────────── */}
          <Card>
            <CardHeader>
              <CardTitle>Recordings to merge</CardTitle>
              <Button size="sm" onClick={browseFiles} disabled={atLimit || busy}>
                Add Files
              </Button>
            </CardHeader>

            {sources.length === 0 ? (
              <div
                role="button"
                tabIndex={0}
                aria-label="Add recordings to merge: press Enter to browse"
                className="border-2 border-dashed border-border rounded-lg m-3 py-10 px-8 text-center cursor-pointer transition-colors hover:border-foreground"
                onClick={browseFiles}
                onKeyDown={e => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault()
                    browseFiles()
                  }
                }}
              >
                <p className="text-[13px] font-semibold text-foreground mb-1">
                  Add two or more recordings of the same event
                </p>
                <p className="type-small text-[hsl(var(--sub))] mb-5">
                  Two to four recordings, each up to 30 minutes: reporter mic, backup recorder, phone.
                </p>
                <div className="flex items-start justify-center gap-6 text-left">
                  {[
                    ['1', 'Add recordings', 'Two to four captures of the same proceeding'],
                    ['2', 'Auto-sync', 'DepoAudio aligns them by sound, no timestamps needed'],
                    ['3', 'One file', 'The loudest source at each moment, or all of them mixed'],
                  ].map(([n, title, desc]) => (
                    <div key={n} className="flex items-start gap-2 max-w-[180px]">
                      <span className="w-5 h-5 rounded-full border border-foreground text-foreground type-label flex items-center justify-center shrink-0 mt-0.5">
                        {n}
                      </span>
                      <div className="flex flex-col">
                        <span className="type-label text-foreground">{title}</span>
                        <span className="type-small text-[hsl(var(--sub))] leading-snug">{desc}</span>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            ) : (
              <div className="p-2">
                {sources.map((s, i) => (
                  <div
                    key={s.path}
                    className="group flex items-center gap-2.5 px-3 py-2 rounded-md transition-colors hover:bg-secondary/50"
                  >
                    <span className="w-2 h-2 rounded-full shrink-0" style={{ background: CH_COLORS[i % 4] }} />
                    <span className="tabular-nums text-[12px] text-[hsl(var(--sub))] min-w-[65px]">
                      {i === 0 ? 'Reference' : `Source ${i + 1}`}
                    </span>
                    <span className="flex-1 type-small text-foreground truncate min-w-0" title={s.path}>
                      {s.name}
                    </span>
                    {s.durationSecs > 0 && (
                      <span
                        className={cn(
                          'tabular-nums text-[12px]',
                          s.durationSecs > MAX_MERGE_MINUTES * 60 ? 'text-destructive' : 'text-[hsl(var(--sub))]',
                        )}
                      >
                        {fmtTime(s.durationSecs)}
                      </span>
                    )}
                    {syncResults[i - 1] && i > 0 && (
                      <Badge variant={syncResults[i - 1].isSameEvent ? 'done' : 'error'}>
                        {syncResults[i - 1].isSameEvent
                          ? `${syncResults[i - 1].offsetSeconds > 0 ? '+' : ''}${syncResults[i - 1].offsetSeconds.toFixed(1)}s offset`
                          : 'May not match'}
                      </Badge>
                    )}
                    <button
                      className="text-[hsl(var(--sub))] opacity-0 group-hover:opacity-100 focus-visible:opacity-100 transition-opacity hover:text-destructive shrink-0"
                      aria-label={`Remove ${s.name}`}
                      disabled={busy}
                      onClick={() => removeSource(i)}
                    >
                      <X size={9} />
                    </button>
                  </div>
                ))}
                {atLimit && (
                  <p className="px-3 pt-1 type-small text-[hsl(var(--sub))]">
                    Four recordings is the most one merge takes.
                  </p>
                )}
              </div>
            )}
          </Card>

          {tooLong.map(s => (
            <p
              key={s.path}
              role="alert"
              className="px-3 py-2 border-l-[3px] border-l-destructive border border-destructive/20 rounded-md type-small text-destructive"
            >
              {overLengthMessage(s.name)}
            </p>
          ))}

          {notice && (
            <p role="status" className="px-3 py-2 border border-border rounded-md type-small text-foreground">
              {notice}
            </p>
          )}

          {/* ── Merge Options ────────────────────────────── */}
          {sources.length >= 2 && (
            <Card>
              <CardHeader>
                <CardTitle>Merge options</CardTitle>
              </CardHeader>

              <CardContent className="p-4">
                <div className="flex flex-col gap-3">
                  <label className="flex items-start gap-3 px-3 py-2.5 rounded-md cursor-pointer transition-colors hover:bg-secondary/50">
                    <input
                      type="radio"
                      name="strategy"
                      value="loudest"
                      checked={strategy === 'loudest'}
                      onChange={() => setStrategy('loudest')}
                      className="mt-0.5 accent-primary"
                    />
                    <div className="flex flex-col">
                      <span className="type-label text-foreground">Loudest source</span>
                      <span className="type-small text-[hsl(var(--sub))]">
                        Uses whichever recording is loudest for each half second, with a short crossfade when it
                        switches
                      </span>
                    </div>
                  </label>
                  <label className="flex items-start gap-3 px-3 py-2.5 rounded-md cursor-pointer transition-colors hover:bg-secondary/50">
                    <input
                      type="radio"
                      name="strategy"
                      value="mix_all"
                      checked={strategy === 'mix_all'}
                      onChange={() => setStrategy('mix_all')}
                      className="mt-0.5 accent-primary"
                    />
                    <div className="flex flex-col">
                      <span className="type-label text-foreground">Mix all</span>
                      <span className="type-small text-[hsl(var(--sub))]">
                        Averages every recording that is playing at each moment
                      </span>
                    </div>
                  </label>
                </div>

                <div className="flex flex-wrap items-end gap-3 pt-3 border-t border-border/40 mt-3">
                  <div className="flex-1 min-w-[160px]">
                    <Label className="mb-1.5 block">Output name</Label>
                    <Input value={outName} placeholder="merged" onChange={e => setOutName(e.target.value)} />
                  </div>
                  <div>
                    <Label className="mb-1.5 block">Format</Label>
                    <div className="flex gap-0.5 bg-secondary rounded-md p-0.5">
                      {FORMATS_OUT.filter(f => MERGE_FORMATS.includes(f.id)).map(f => (
                        <button
                          key={f.id}
                          title={f.desc}
                          aria-pressed={format === f.id}
                          className={cn(
                            'px-3 py-1.5 rounded-md type-label transition-colors',
                            format === f.id
                              ? 'bg-card text-foreground ring-1 ring-inset ring-foreground'
                              : 'text-[hsl(var(--sub))] hover:text-[hsl(var(--text2))]',
                          )}
                          onClick={() => setFormat(f.id)}
                        >
                          {f.label}
                        </button>
                      ))}
                    </div>
                  </div>
                </div>

                <div className="flex items-center gap-3 pt-3 mt-3 border-t border-border/40">
                  <span className="flex-1 type-small text-[hsl(var(--sub))] truncate min-w-0" title={outDir || ''}>
                    Saves to {outDir ? basename(outDir) : 'the folder of the reference recording'}
                  </span>
                  {outDir && (
                    <Button size="sm" variant="ghost" onClick={() => setOutDir('')} disabled={busy}>
                      Use reference folder
                    </Button>
                  )}
                  <Button size="sm" onClick={chooseFolder} disabled={busy}>
                    Choose folder
                  </Button>
                </div>
              </CardContent>
            </Card>
          )}

          {/* ── Sync & Result ────────────────────────────── */}
          {result && (
            <Card className="border-success/30">
              <CardHeader>
                <CardTitle>Merged output</CardTitle>
              </CardHeader>
              <CardContent className="px-4 py-3">
                <span className="text-sm font-semibold text-foreground block">{result.outputName}</span>
                <span className="type-small text-[hsl(var(--sub))] block mt-0.5">
                  {fmtTime(result.duration)} · {fmtSize(result.outputSize)} · {result.sourcesUsed} sources
                </span>
                {result.syncOffsets?.length > 1 && (
                  <span className="type-small text-[hsl(var(--sub))] block mt-0.5">
                    Sync offsets:{' '}
                    {result.syncOffsets
                      .slice(1)
                      .map(o => `${o > 0 ? '+' : ''}${o.toFixed(1)}s`)
                      .join(', ')}
                  </span>
                )}
                {result.warning && (
                  <p className="mt-2 rounded-md border border-warning/30 bg-warning/10 px-2.5 py-2 type-small text-foreground">
                    {result.warning}
                  </p>
                )}
              </CardContent>
            </Card>
          )}

          {error && (
            <p className="px-3 py-2 border-l-[3px] border-l-destructive border border-destructive/20 rounded-md type-small text-destructive">
              {error}
            </p>
          )}
        </div>
      </div>

      <footer className="shrink-0 flex items-center justify-between px-6 py-3 border-t border-border bg-[hsl(var(--surface))]">
        <div className="flex items-center gap-2">
          {syncing && (
            <span className="flex items-center gap-1.5 px-2.5 py-1 bg-[hsl(var(--blue)/0.1)] text-[hsl(var(--blue))] text-[12px] tabular-nums">
              <span className="w-1.5 h-1.5 rounded-full bg-[hsl(var(--blue))] animate-dot-pulse" />
              Detecting sync…
            </span>
          )}
          {merging && progress && (
            <span
              role="status"
              className="flex items-center gap-2 px-2.5 py-1 bg-[hsl(var(--blue)/0.1)] text-[hsl(var(--blue))] text-[12px] tabular-nums"
            >
              <span>
                {STAGE_LABELS[progress.stage] ?? 'Merging'} {progress.percent}%
              </span>
              <span
                className="w-28 h-1 rounded-full bg-[hsl(var(--blue)/0.2)] overflow-hidden"
                role="progressbar"
                aria-label="Merge progress"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={progress.percent}
              >
                <span className="block h-full bg-[hsl(var(--blue))]" style={{ width: `${progress.percent}%` }} />
              </span>
            </span>
          )}
          {result && !merging && (
            <span className="flex items-center gap-1.5 px-2.5 py-1 bg-success/10 text-success text-[12px] tabular-nums">
              Merge complete
            </span>
          )}
        </div>
        <div className="flex gap-2">
          {busy && (
            <Button size="sm" variant="ghost" onClick={cancel} disabled={cancelling}>
              {cancelling ? 'Cancelling…' : 'Cancel'}
            </Button>
          )}
          {sources.length >= 2 && !merging && (
            <Button size="sm" onClick={handleSync} disabled={syncing}>
              {syncing ? (
                <>
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  Syncing…
                </>
              ) : (
                'Check Sync'
              )}
            </Button>
          )}
          <Button variant="primary" onClick={handleMerge} disabled={busy || sources.length < 2 || tooLong.length > 0}>
            {merging ? (
              <>
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
                Merging…
              </>
            ) : (
              'Merge'
            )}
          </Button>
        </div>
      </footer>
    </>
  )
}
