import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { Loader2, X } from 'lucide-react'
import { cn } from '../../lib/utils'
import { FORMATS_OUT, CH_COLORS } from '../../constants'
import { fmtSize, fmtTime } from '../../utils'
import { Button } from '../ui/button'
import { Card, CardHeader, CardTitle, CardContent } from '../ui/card'
import { Input } from '../ui/input'
import { Label } from '../ui/label'
import { Badge } from '../ui/badge'

export default function MergeTab() {
  const [sources, setSources] = useState([])
  // Merge output goes next to the first source; sample rate is fixed at 48kHz
  const outDir = ''
  const rate = '48000'
  const [outName, setOutName] = useState('')
  const [format, setFormat] = useState('wav')
  const [strategy, setStrategy] = useState('best_quality')
  const [syncing, setSyncing] = useState(false)
  const [merging, setMerging] = useState(false)
  const [syncResults, setSyncResults] = useState([])
  const [result, setResult] = useState(null)
  const [error, setError] = useState('')

  const browseFiles = async () => {
    try {
      const selected = await open({
        multiple: true,
        filters: [
          { name: 'Audio', extensions: ['wav','mp3','flac','opus','ogg','m4a','aac','wma','aif','aiff','caf','amr','sgmca','trm','ftr','bwf'] },
          { name: 'Video (audio extracted)', extensions: ['mp4','mov','mkv','avi','webm'] },
        ],
      })
      if (selected) {
        const paths = Array.isArray(selected) ? selected : [selected]
        const newSources = paths.map(p => {
          const path = typeof p === 'string' ? p : p.path
          return { path, name: path.split('/').pop().split('\\').pop() }
        })
        setSources(prev => [...prev, ...newSources])
        setSyncResults([]) // sync is stale once the source set changes
        setResult(null)
        setError('')
      }
    } catch {
      // Dialog dismissed or unavailable — nothing to add
    }
  }

  const removeSource = (idx) => {
    setSources(prev => prev.filter((_, i) => i !== idx))
    setSyncResults([])
    setResult(null)
  }

  const handleSync = async () => {
    if (sources.length < 2) return
    setSyncing(true)
    setError('')
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
      setError(String(e))
    }
    setSyncing(false)
  }

  const handleMerge = async () => {
    if (sources.length < 2 || merging) return
    setMerging(true)
    setError('')
    setResult(null)
    try {
      const res = await invoke('merge_audio_cmd', {
        job: {
          sources: sources.map(s => s.path),
          outDir,
          outName: outName || 'merged',
          format,
          rate: format === 'opus' ? '48000' : rate,
          strategy,
        }
      })
      setResult(res)
    } catch (e) {
      setError(String(e))
    }
    setMerging(false)
  }

  return (
    <>
      <div className="flex-1 overflow-y-auto overflow-x-hidden">
        <div className="w-full max-w-[1100px] mx-auto px-5 md:px-8 py-5 flex flex-col gap-3.5">

          {/* ── Source Files ──────────────────────────────── */}
          <Card>
            <CardHeader>
              <CardTitle>RECORDINGS TO MERGE</CardTitle>
              <Button size="sm" onClick={browseFiles}>Add Files</Button>
            </CardHeader>

            {sources.length === 0 ? (
              <div
                role="button"
                tabIndex={0}
                aria-label="Add recordings to merge: press Enter to browse"
                className="border-2 border-dashed border-border rounded-lg m-3 py-10 px-8 text-center cursor-pointer transition-colors hover:border-primary focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
                onClick={browseFiles}
                onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); browseFiles() } }}
              >
                <p className="text-[13px] font-semibold text-foreground mb-1">Add two or more recordings of the same event</p>
                <p className="text-[11px] text-[hsl(var(--sub))] mb-5">
                  Reporter mic, backup recorder, phone — any combination works.
                </p>
                <div className="flex items-start justify-center gap-6 text-left">
                  {[
                    ['1', 'Add recordings', 'Two or more captures of the same proceeding'],
                    ['2', 'Auto-sync', 'DepoAudio aligns them by sound, no timestamps needed'],
                    ['3', 'One clean file', 'The clearest source wins at every moment'],
                  ].map(([n, title, desc]) => (
                    <div key={n} className="flex items-start gap-2 max-w-[180px]">
                      <span className="w-5 h-5 rounded-full bg-[hsl(var(--gold-dim))] text-foreground text-[10px] font-bold flex items-center justify-center shrink-0 mt-0.5">{n}</span>
                      <div className="flex flex-col">
                        <span className="text-[11px] font-semibold text-foreground">{title}</span>
                        <span className="text-[10px] text-[hsl(var(--sub))] leading-snug">{desc}</span>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            ) : (
              <div className="p-2">
                {sources.map((s, i) => (
                  <div key={i} className="group flex items-center gap-2.5 px-3 py-2 rounded-md transition-colors hover:bg-secondary/50">
                    <span className="w-2 h-2 rounded-full shrink-0" style={{ background: CH_COLORS[i % 4] }} />
                    <span className="font-mono text-[10px] text-[hsl(var(--sub))] min-w-[65px]">
                      {i === 0 ? 'Reference' : `Source ${i + 1}`}
                    </span>
                    <span className="flex-1 text-xs text-foreground truncate min-w-0" title={s.path}>
                      {s.name}
                    </span>
                    {syncResults[i - 1] && i > 0 && (
                      <Badge variant={syncResults[i - 1].isSameEvent ? 'done' : 'error'}>
                        {syncResults[i - 1].isSameEvent
                          ? `${syncResults[i - 1].offsetSeconds > 0 ? '+' : ''}${syncResults[i - 1].offsetSeconds.toFixed(1)}s offset`
                          : 'May not match'}
                      </Badge>
                    )}
                    <button
                      className="text-[hsl(var(--sub))] opacity-0 group-hover:opacity-100 transition-opacity hover:text-destructive shrink-0"
                      aria-label="Remove source"
                      onClick={() => removeSource(i)}
                    >
                      <X size={9} />
                    </button>
                  </div>
                ))}
              </div>
            )}
          </Card>

          {/* ── Merge Options ────────────────────────────── */}
          {sources.length >= 2 && (
            <Card>
              <CardHeader>
                <CardTitle>MERGE OPTIONS</CardTitle>
              </CardHeader>

              <CardContent className="p-4">
                <div className="flex flex-col gap-3">
                  <label className="flex items-start gap-3 px-3 py-2.5 rounded-md cursor-pointer transition-colors hover:bg-secondary/50">
                    <input
                      type="radio"
                      name="strategy"
                      value="best_quality"
                      checked={strategy === 'best_quality'}
                      onChange={() => setStrategy('best_quality')}
                      className="mt-0.5 accent-primary"
                    />
                    <div className="flex flex-col">
                      <span className="text-xs font-semibold text-foreground">Best quality</span>
                      <span className="text-[11px] text-[hsl(var(--sub))]">
                        Picks the clearest source for each moment — fills gaps automatically
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
                      <span className="text-xs font-semibold text-foreground">Mix all together</span>
                      <span className="text-[11px] text-[hsl(var(--sub))]">
                        Blends all sources equally — louder but keeps everything
                      </span>
                    </div>
                  </label>
                </div>

                <div className="flex items-end gap-3 pt-3 border-t border-border/40 mt-3">
                  <div className="flex-1">
                    <Label className="mb-1.5 block">OUTPUT NAME</Label>
                    <Input value={outName} placeholder="merged" onChange={e => setOutName(e.target.value)} />
                  </div>
                  <div>
                    <Label className="mb-1.5 block">FORMAT</Label>
                    <div className="flex gap-0.5 bg-secondary rounded-md p-0.5">
                      {FORMATS_OUT.map(f => (
                        <button
                          key={f.id}
                          title={f.desc}
                          aria-pressed={format === f.id}
                          className={cn(
                            'px-3 py-1.5 rounded-md text-xs font-semibold transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring',
                            format === f.id
                              ? 'bg-[hsl(var(--gold-dim))] text-foreground'
                              : 'text-[hsl(var(--sub))] hover:text-[hsl(var(--text2))]'
                          )}
                          onClick={() => setFormat(f.id)}
                        >
                          {f.label}
                        </button>
                      ))}
                    </div>
                  </div>
                </div>
              </CardContent>
            </Card>
          )}

          {/* ── Sync & Result ────────────────────────────── */}
          {result && (
            <Card className="border-success/30">
              <CardHeader>
                <CardTitle>MERGED OUTPUT</CardTitle>
              </CardHeader>
              <CardContent className="px-4 py-3">
                <span className="text-sm font-semibold text-foreground block">{result.outputName}</span>
                <span className="text-[11px] text-[hsl(var(--sub))] block mt-0.5">
                  {fmtTime(result.duration)} · {fmtSize(result.outputSize)} · {result.sourcesUsed} sources
                </span>
                {result.syncOffsets?.length > 1 && (
                  <span className="text-[11px] text-[hsl(var(--sub))] block mt-0.5">
                    Sync offsets: {result.syncOffsets.slice(1).map(o => `${o > 0 ? '+' : ''}${o.toFixed(1)}s`).join(', ')}
                  </span>
                )}
              </CardContent>
            </Card>
          )}

          {error && (
            <p className="px-3 py-2 bg-destructive/10 border border-destructive/20 rounded-md text-xs text-destructive">
              {error}
            </p>
          )}

        </div>
      </div>

      <footer className="shrink-0 flex items-center justify-between px-6 py-3 border-t border-border bg-[hsl(var(--surface))]">
        <div className="flex items-center gap-2">
          {syncing && (
            <span className="flex items-center gap-1.5 px-2.5 py-1 bg-[hsl(var(--blue)/0.1)] text-[hsl(var(--blue))] text-[11px] font-mono rounded-full">
              <span className="w-1.5 h-1.5 rounded-full bg-[hsl(var(--blue))] animate-dot-pulse" />
              Detecting sync…
            </span>
          )}
          {merging && (
            <span className="flex items-center gap-1.5 px-2.5 py-1 bg-[hsl(var(--blue)/0.1)] text-[hsl(var(--blue))] text-[11px] font-mono rounded-full">
              <span className="w-1.5 h-1.5 rounded-full bg-[hsl(var(--blue))] animate-dot-pulse" />
              Merging…
            </span>
          )}
          {result && !merging && (
            <span className="flex items-center gap-1.5 px-2.5 py-1 bg-success/10 text-success text-[11px] font-mono rounded-full">
              Merge complete
            </span>
          )}
        </div>
        <div className="flex gap-2">
          {sources.length >= 2 && !merging && (
            <Button size="sm" onClick={handleSync} disabled={syncing}>
              {syncing ? <><Loader2 className="h-3.5 w-3.5 animate-spin" />Syncing…</> : 'Check Sync'}
            </Button>
          )}
          <Button
            variant="primary"
            onClick={handleMerge}
            disabled={merging || sources.length < 2}
          >
            {merging ? <><Loader2 className="h-3.5 w-3.5 animate-spin" />Merging…</> : 'Merge'}
          </Button>
        </div>
      </footer>
    </>
  )
}
