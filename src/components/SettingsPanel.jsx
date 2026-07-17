import { useState, useEffect, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'
import { RotateCcw, Download, Trash2, CheckCircle, Loader2, AlertCircle, Cpu, RefreshCw, ExternalLink, Boxes, SlidersHorizontal, AppWindow, DownloadCloud } from 'lucide-react'
import { DEPOSTACK_URL } from '../constants'
import { Dialog, DialogContent, DialogTitle, DialogClose, DialogDescription } from './ui/dialog'
import { Button } from './ui/button'
import { Input } from './ui/input'
import { Label } from './ui/label'
import { Badge } from './ui/badge'
import { Card, CardHeader, CardTitle, CardContent } from './ui/card'
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from './ui/select'

// ── Default values ────────────────────────────────────────────────────────────

const DEFAULTS = {
  hpfCutoff: 80,
  normalizeLufs: -16,
  normalizeTp: -1.5,
  silenceThresh: -50,
  fadeDur: 0.5,
  ffmpegTimeout: 300,
  maxScanDepth: 5,
  maxFileSizeGb: 2,
  defaultOutputFormat: '',
  defaultOutputMode: '',
}

const SETTINGS_PRESETS = [
  { id: 'recommended', name: 'Recommended', desc: 'Best for most court recordings', values: { ...DEFAULTS } },
  { id: 'high-quality', name: 'High Quality', desc: 'Louder output, tighter silence trim', values: { ...DEFAULTS, normalizeLufs: -14, normalizeTp: -1.0, silenceThresh: -40 } },
  { id: 'gentle', name: 'Gentle', desc: 'Minimal processing, preserve original character', values: { ...DEFAULTS, hpfCutoff: 40, normalizeLufs: -18, normalizeTp: -2.0, silenceThresh: -60, fadeDur: 0.3 } },
  { id: 'broadcast', name: 'Broadcast', desc: 'Matches broadcast loudness standards', values: { ...DEFAULTS, normalizeLufs: -23, normalizeTp: -1.0, hpfCutoff: 80 } },
]

// The modal's left rail — each entry is a group of Cards on the right.
const NAV = [
  { id: 'models', label: 'AI Models', Icon: Boxes },
  { id: 'audio', label: 'Audio', Icon: SlidersHorizontal },
  { id: 'app', label: 'App', Icon: AppWindow },
  { id: 'updates', label: 'Updates', Icon: DownloadCloud },
]

// ── Field helpers (token-styled) ───────────────────────────────────────────────

function NumberField({ label, hint, unit, value, setValue, min, max, step = 1, defaultVal }) {
  // Hold the raw text locally so intermediate keystrokes aren't reverted by the
  // controlled input; clamp on blur.
  const [text, setText] = useState(String(value))
  const [prevValue, setPrevValue] = useState(value)
  if (value !== prevValue) { setPrevValue(value); setText(String(value)) }

  const commit = () => {
    const v = parseFloat(text)
    if (isNaN(v)) { setText(String(value)); return }
    const clamped = Math.min(max, Math.max(min, v))
    setValue(clamped)
    setText(String(clamped))
  }

  return (
    <div className="flex flex-col gap-1">
      <Label className="text-[12px] font-medium text-foreground">
        {label}{unit && <span className="ml-1 text-[hsl(var(--sub))] font-normal">({unit})</span>}
      </Label>
      {hint && <p className="text-[11px] leading-snug text-[hsl(var(--sub))]">{hint}</p>}
      <Input
        type="number" className="h-8 text-[12px] max-w-[140px]"
        value={text} min={min} max={max} step={step} placeholder={String(defaultVal)}
        onChange={e => {
          setText(e.target.value)
          const v = parseFloat(e.target.value)
          if (!isNaN(v) && v >= min && v <= max) setValue(v)
        }}
        onBlur={commit}
      />
    </div>
  )
}

function SelectField({ label, hint, value, setValue, options }) {
  return (
    <div className="flex flex-col gap-1">
      <Label className="text-[12px] font-medium text-foreground">{label}</Label>
      {hint && <p className="text-[11px] leading-snug text-[hsl(var(--sub))]">{hint}</p>}
      <Select value={value} onValueChange={setValue}>
        <SelectTrigger className="h-8 text-[12px] max-w-[260px]" aria-label={label}><SelectValue /></SelectTrigger>
        <SelectContent>
          {options.map(o => <SelectItem key={o.value} value={o.value}>{o.label}</SelectItem>)}
        </SelectContent>
      </Select>
    </div>
  )
}

// A Card whose header carries a "Reset" action for the section.
function ResetCard({ title, onReset, children }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        {onReset && (
          <Button variant="ghost" size="sm" className="h-6 text-[10px]" onClick={onReset} title="Reset to defaults">
            <RotateCcw size={11} /> Reset
          </Button>
        )}
      </CardHeader>
      <CardContent className="p-4">{children}</CardContent>
    </Card>
  )
}

// ── Model Manager ──────────────────────────────────────────────────────────────

function ModelManager() {
  const [models, setModels] = useState([])
  const [caps, setCaps] = useState(null)
  const [downloading, setDownloading] = useState({})
  const [error, setError] = useState(null)

  const loadModels = useCallback(() => (
    Promise.all([invoke('model_catalog_cmd'), invoke('system_capabilities_cmd')])
      .then(([catalog, capabilities]) => { setModels(catalog); setCaps(capabilities) })
      .catch(() => setModels([]))
  ), [])

  useEffect(() => { loadModels() }, [loadModels])

  const handleDownload = async (filename) => {
    setDownloading(d => ({ ...d, [filename]: true })); setError(null)
    try { await invoke('download_model_cmd', { filename }); await loadModels() }
    catch (e) { setError(`Couldn't download that model: ${e}`) }
    finally { setDownloading(d => ({ ...d, [filename]: false })) }
  }
  const handleDelete = async (filename) => {
    setError(null)
    try { await invoke('delete_model_cmd', { filename }); await loadModels() }
    catch (e) { setError(`Couldn't delete that model: ${e}`) }
  }

  const groups = {}
  models.forEach(m => { (groups[m.feature] ||= []).push(m) })
  const installedCount = models.filter(m => m.installed).length
  const totalSize = models.filter(m => m.installed).reduce((s, m) => s + m.sizeMb, 0)

  return (
    <Card>
      <CardHeader>
        <CardTitle>AI Models</CardTitle>
        <span className="font-mono text-[10px] text-[hsl(var(--sub))]">
          {installedCount}/{models.length} installed · {totalSize.toFixed(1)} MB
        </span>
      </CardHeader>
      <CardContent className="p-4 flex flex-col gap-3">
        {caps && (
          <div className="flex flex-wrap items-center gap-1.5 text-[11px] text-[hsl(var(--text2))]">
            <Cpu size={13} className="text-[hsl(var(--sub))]" />
            <span className="font-medium">{caps.acceleratorDesc}</span>
            <Badge variant="default">{caps.tier} tier</Badge>
            <Badge variant="default">{caps.cpuCores} cores</Badge>
            <Badge variant="default">{Math.round(caps.ramMb / 1024)} GB RAM</Badge>
          </div>
        )}

        {error && (
          <div className="flex items-center gap-2 px-3 py-2 rounded-md bg-destructive/10 text-destructive text-[11px]">
            <AlertCircle size={13} className="shrink-0" /> {error}
          </div>
        )}

        <div className="flex flex-col gap-3">
          {Object.entries(groups).map(([feature, items]) => (
            <div key={feature} className="flex flex-col gap-1">
              <span className="font-mono text-[9px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))] px-1">{feature}</span>
              {items.map(m => (
                <div key={m.filename} className="flex items-center gap-3 px-2 py-2 rounded-md hover:bg-secondary/50 transition-colors">
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-1.5 text-[12px] font-medium text-foreground">
                      {m.installed
                        ? <CheckCircle size={13} className="text-[hsl(var(--success))] shrink-0" />
                        : <Download size={13} className="text-[hsl(var(--sub))] shrink-0" />}
                      <span className="truncate">{m.displayName}</span>
                      {m.required && <Badge variant="active">Required</Badge>}
                      {m.recommended && !m.required && <Badge variant="done">Recommended</Badge>}
                    </div>
                    <div className="text-[11px] text-[hsl(var(--sub))] mt-0.5 ml-[19px]">{m.description} — {m.sizeMb} MB</div>
                  </div>
                  <div className="shrink-0">
                    {!m.installed && (
                      <Button variant="outline" size="sm" disabled={downloading[m.filename]}
                        onClick={() => handleDownload(m.filename)} aria-label={`Download ${m.displayName}`}>
                        {downloading[m.filename]
                          ? <><Loader2 size={12} className="animate-spin" /> Downloading…</>
                          : <><Download size={12} /> Install</>}
                      </Button>
                    )}
                    {m.installed && !m.required && (
                      <Button variant="ghost" size="icon" className="h-7 w-7 hover:text-destructive"
                        onClick={() => handleDelete(m.filename)} aria-label={`Delete ${m.displayName}`}>
                        <Trash2 size={12} />
                      </Button>
                    )}
                  </div>
                </div>
              ))}
            </div>
          ))}
        </div>

        <p className="text-[11px] text-[hsl(var(--sub))] border-t border-border/60 pt-2.5">
          Models run 100% locally. No audio ever leaves your machine.
        </p>
      </CardContent>
    </Card>
  )
}

// ── Panel ──────────────────────────────────────────────────────────────────────

export default function SettingsPanel({ open, onOpenChange, prefs, updater = {} }) {
  const { status: updateStatus, update, progress: updateProgress, checkForUpdate, installUpdate } = updater
  const {
    hpfCutoff, setHpfCutoff, normalizeLufs, setNormalizeLufs, normalizeTp, setNormalizeTp,
    silenceThresh, setSilenceThresh, fadeDur, setFadeDur,
    ffmpegTimeout, setFfmpegTimeout, maxScanDepth, setMaxScanDepth,
    maxFileSizeGb, setMaxFileSizeGb, defaultOutputFormat, setDefaultOutputFormat,
    defaultOutputMode, setDefaultOutputMode,
  } = prefs
  const [section, setSection] = useState('models')

  const applyPreset = (preset) => {
    const v = preset.values
    setHpfCutoff(v.hpfCutoff); setNormalizeLufs(v.normalizeLufs); setNormalizeTp(v.normalizeTp)
    setSilenceThresh(v.silenceThresh); setFadeDur(v.fadeDur)
  }
  const resetAudio = () => applyPreset(SETTINGS_PRESETS[0])
  const resetApp = () => {
    setFfmpegTimeout(DEFAULTS.ffmpegTimeout); setMaxScanDepth(DEFAULTS.maxScanDepth)
    setMaxFileSizeGb(DEFAULTS.maxFileSizeGb)
    setDefaultOutputFormat(DEFAULTS.defaultOutputFormat); setDefaultOutputMode(DEFAULTS.defaultOutputMode)
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[760px] p-0 gap-0 overflow-hidden">
        <div className="flex items-center justify-between px-5 py-3.5 border-b border-border">
          <DialogTitle className="text-[15px] font-semibold text-foreground">Settings</DialogTitle>
          <DialogClose />
        </div>
        <DialogDescription className="sr-only">Application settings</DialogDescription>

        <div className="flex min-h-0" style={{ height: '68vh' }}>
          {/* Left rail */}
          <nav aria-label="Settings sections" className="w-16 md:w-44 shrink-0 border-r border-border bg-[hsl(var(--surface))] p-2 flex flex-col gap-0.5">
            {NAV.map(({ id, label, Icon }) => (
              <button
                key={id}
                onClick={() => setSection(id)}
                aria-current={section === id ? 'true' : undefined}
                className={`flex items-center gap-2.5 px-2.5 md:px-3 py-2 rounded-lg text-[12.5px] font-medium transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring ${
                  section === id
                    ? 'bg-[hsl(var(--gold-dim))] text-foreground'
                    : 'text-[hsl(var(--text2))] hover:bg-secondary/60'}`}
              >
                <Icon size={15} className="shrink-0" aria-hidden="true" />
                <span className="hidden md:inline">{label}</span>
              </button>
            ))}
          </nav>

          {/* Content pane */}
          <div className="flex-1 overflow-y-auto p-4 md:p-5 flex flex-col gap-3.5">
            {section === 'models' && <ModelManager />}

            {section === 'audio' && (
              <>
                <Card>
                  <CardHeader><CardTitle>Quick setup</CardTitle></CardHeader>
                  <CardContent className="p-4 flex flex-col gap-2">
                    <p className="text-[11px] text-[hsl(var(--sub))]">Start from a preset, then fine-tune below.</p>
                    <div className="flex gap-1.5 flex-wrap">
                      {SETTINGS_PRESETS.map(p => (
                        <Button key={p.id} variant="outline" size="sm" className="rounded-full" title={p.desc} onClick={() => applyPreset(p)}>
                          {p.name}
                        </Button>
                      ))}
                    </div>
                  </CardContent>
                </Card>

                <ResetCard title="Audio processing" onReset={resetAudio}>
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-x-6 gap-y-4">
                    <NumberField label="Low-frequency cutoff" unit="Hz" hint="Removes rumble and handling noise below this frequency"
                      value={hpfCutoff} setValue={setHpfCutoff} min={20} max={500} step={1} defaultVal={DEFAULTS.hpfCutoff} />
                    <NumberField label="Target volume level" unit="LUFS" hint="How loud the output should be. Lower = quieter. Standard is -16"
                      value={normalizeLufs} setValue={setNormalizeLufs} min={-24} max={-6} step={0.5} defaultVal={DEFAULTS.normalizeLufs} />
                    <NumberField label="Peak limit" unit="dB" hint="Prevents distortion on the loudest moments"
                      value={normalizeTp} setValue={setNormalizeTp} min={-6} max={0} step={0.1} defaultVal={DEFAULTS.normalizeTp} />
                    <NumberField label="Silence detection" unit="dB" hint="Audio quieter than this is treated as silence for trimming"
                      value={silenceThresh} setValue={setSilenceThresh} min={-70} max={-20} step={1} defaultVal={DEFAULTS.silenceThresh} />
                    <NumberField label="Fade duration" unit="seconds" hint="How long the fade in/out lasts at the start and end"
                      value={fadeDur} setValue={setFadeDur} min={0.1} max={5.0} step={0.1} defaultVal={DEFAULTS.fadeDur} />
                  </div>
                </ResetCard>
              </>
            )}

            {section === 'app' && (
              <>
                <ResetCard title="Defaults" onReset={resetApp}>
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-x-6 gap-y-4">
                    <SelectField label="Theme" value={prefs.themePref || 'system'} setValue={v => prefs.cycleThemeTo?.(v)}
                      options={[{ value: 'system', label: 'Match system' }, { value: 'dark', label: 'Dark' }, { value: 'light', label: 'Light' }]} />
                    <SelectField label="Default output format" hint="Format the app opens with"
                      value={defaultOutputFormat || 'last'} setValue={v => setDefaultOutputFormat(v === 'last' ? '' : v)}
                      options={[
                        { value: 'last', label: 'Remember last used' }, { value: 'wav', label: 'WAV (lossless)' },
                        { value: 'mp3', label: 'MP3 (smaller, universal)' }, { value: 'flac', label: 'FLAC (lossless, compressed)' },
                        { value: 'opus', label: 'Opus (smallest, voice-optimized)' }, { value: 'm4a', label: 'M4A (Apple devices)' },
                      ]} />
                    <SelectField label="Default output mode" hint="Channel layout the app opens with"
                      value={defaultOutputMode || 'last'} setValue={v => setDefaultOutputMode(v === 'last' ? '' : v)}
                      options={[
                        { value: 'last', label: 'Remember last used' }, { value: 'stereo', label: 'Mix to Stereo' },
                        { value: 'keep', label: 'Keep Original Channels' }, { value: 'split', label: 'Split by Speaker' },
                      ]} />
                  </div>
                </ResetCard>

                <ResetCard title="Performance & limits">
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-x-6 gap-y-4">
                    <NumberField label="Processing timeout" unit="seconds" hint="Max time allowed per file before canceling"
                      value={ffmpegTimeout} setValue={setFfmpegTimeout} min={60} max={3600} step={10} defaultVal={DEFAULTS.ffmpegTimeout} />
                    <NumberField label="Folder scan depth" unit="levels" hint="How many folder levels deep to search for recordings"
                      value={maxScanDepth} setValue={setMaxScanDepth} min={1} max={20} step={1} defaultVal={DEFAULTS.maxScanDepth} />
                    <NumberField label="Max file size" unit="GB" hint="Files larger than this will be rejected"
                      value={maxFileSizeGb} setValue={setMaxFileSizeGb} min={0.5} max={10} step={0.5} defaultVal={DEFAULTS.maxFileSizeGb} />
                  </div>
                </ResetCard>
              </>
            )}

            {section === 'updates' && (
              <>
                <Card>
                  <CardHeader><CardTitle>Software update</CardTitle></CardHeader>
                  <CardContent className="p-4 flex items-center justify-between gap-3">
                    <div className="flex flex-col min-w-0">
                      <span className="text-[12px] font-medium text-foreground">Updates install automatically from GitHub Releases</span>
                      <span className="text-[11px] text-[hsl(var(--sub))] mt-0.5">
                        {updateStatus === 'checking' && 'Checking for updates…'}
                        {updateStatus === 'available' && `Version ${update?.version} is available.`}
                        {updateStatus === 'uptodate' && "You're on the latest version."}
                        {updateStatus === 'downloading' && `Downloading… ${Math.round((updateProgress || 0) * 100)}%`}
                        {updateStatus === 'ready' && 'Update installed — restarting…'}
                        {updateStatus === 'error' && "Couldn't check for updates (offline, or no release published)."}
                        {(!updateStatus || updateStatus === 'idle') && 'Checked automatically each time you open the app.'}
                      </span>
                    </div>
                    {updateStatus === 'available' ? (
                      <Button size="sm" variant="primary" className="shrink-0" onClick={() => installUpdate?.()}>
                        <Download size={12} /> Update &amp; restart
                      </Button>
                    ) : (
                      <Button size="sm" variant="outline" className="shrink-0" onClick={() => checkForUpdate?.(true)}
                        disabled={updateStatus === 'checking' || updateStatus === 'downloading' || updateStatus === 'ready'}>
                        {updateStatus === 'checking'
                          ? <><Loader2 size={12} className="animate-spin" /> Checking…</>
                          : <><RefreshCw size={12} /> Check for updates</>}
                      </Button>
                    )}
                  </CardContent>
                </Card>

                <Card>
                  <CardHeader><CardTitle>About</CardTitle></CardHeader>
                  <CardContent className="p-4 flex items-center justify-between gap-3">
                    <div className="flex flex-col min-w-0">
                      <span className="text-[12px] font-medium text-foreground">DepoAudio is part of DepoStack</span>
                      <span className="text-[11px] text-[hsl(var(--sub))] mt-0.5">
                        A growing suite of tools built for court reporters. DepoAudio itself stays free and open source.
                      </span>
                    </div>
                    <Button size="sm" variant="outline" className="shrink-0" onClick={() => openUrl(DEPOSTACK_URL)}>
                      <ExternalLink size={12} /> depostack.com
                    </Button>
                  </CardContent>
                </Card>
              </>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
