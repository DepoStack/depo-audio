import { useState, useEffect, useCallback, useId } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'
import {
  RotateCcw,
  Download,
  Trash2,
  Loader2,
  AlertCircle,
  RefreshCw,
  ExternalLink,
  Boxes,
  SlidersHorizontal,
  AppWindow,
  DownloadCloud,
} from 'lucide-react'
import { DEPOAUDIO_RELEASES_URL, DEPOSTACK_URL } from '../constants'
import { Dialog, DialogContent, DialogTitle, DialogClose, DialogDescription } from './ui/dialog'
import { Button } from './ui/button'
import { Input } from './ui/input'
import { Label } from './ui/label'
import { Card, CardHeader, CardTitle, CardContent } from './ui/card'
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from './ui/select'

// ── Default values ────────────────────────────────────────────────────────────

const DEFAULTS = {
  theme: 'system',
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
  {
    id: 'high-quality',
    name: 'High Quality',
    desc: 'Louder output, more sensitive leading-silence trim',
    values: { ...DEFAULTS, normalizeLufs: -14, normalizeTp: -1.0, silenceThresh: -40 },
  },
  {
    id: 'gentle',
    name: 'Gentle',
    desc: 'Minimal processing, preserve original character',
    values: { ...DEFAULTS, hpfCutoff: 40, normalizeLufs: -18, normalizeTp: -2.0, silenceThresh: -60, fadeDur: 0.3 },
  },
  {
    id: 'broadcast',
    name: 'Broadcast',
    desc: 'Matches broadcast loudness standards',
    values: { ...DEFAULTS, normalizeLufs: -23, normalizeTp: -1.0, hpfCutoff: 80 },
  },
]

// The modal's left rail — each entry is a group of Cards on the right.
const NAV = [
  { id: 'models', label: 'Model files', Icon: Boxes },
  { id: 'audio', label: 'Audio', Icon: SlidersHorizontal },
  { id: 'app', label: 'App', Icon: AppWindow },
  { id: 'updates', label: 'Updates', Icon: DownloadCloud },
]

// ── Field helpers (token-styled) ───────────────────────────────────────────────

function NumberField({ label, hint, unit, value, setValue, min, max, step = 1, defaultVal }) {
  const fieldId = useId()
  const hintId = hint ? `${fieldId}-hint` : undefined
  // Hold the raw text locally so intermediate keystrokes aren't reverted by the
  // controlled input; clamp on blur.
  const [text, setText] = useState(String(value))
  const [prevValue, setPrevValue] = useState(value)
  if (value !== prevValue) {
    setPrevValue(value)
    setText(String(value))
  }

  const commit = () => {
    const v = parseFloat(text)
    if (isNaN(v)) {
      setText(String(value))
      return
    }
    const clamped = Math.min(max, Math.max(min, v))
    setValue(clamped)
    setText(String(clamped))
  }

  return (
    <div className="flex flex-col gap-1">
      <Label htmlFor={fieldId} className="text-[12px] font-medium text-foreground">
        {label}
        {unit && <span className="ml-1 text-[hsl(var(--sub))] font-normal">({unit})</span>}
      </Label>
      {hint && (
        <p id={hintId} className="text-[11px] leading-snug text-[hsl(var(--sub))]">
          {hint}
        </p>
      )}
      <Input
        id={fieldId}
        type="number"
        aria-label={`${label}${unit ? ` (${unit})` : ''}`}
        aria-describedby={hintId}
        className="h-8 text-[12px] max-w-[140px]"
        value={text}
        min={min}
        max={max}
        step={step}
        placeholder={String(defaultVal)}
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
        <SelectTrigger className="h-8 text-[12px] max-w-[260px]" aria-label={label}>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {options.map(o => (
            <SelectItem key={o.value} value={o.value}>
              {o.label}
            </SelectItem>
          ))}
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

function LegacyModelStorage() {
  const [models, setModels] = useState([])
  const [error, setError] = useState(null)
  const [modelsLoading, setModelsLoading] = useState(true)
  const [modelLoadError, setModelLoadError] = useState('')

  const loadModels = useCallback(
    () =>
      invoke('legacy_model_cleanup_catalog_cmd')
        .then(catalog => {
          setModels(catalog)
          setModelLoadError('')
        })
        .catch(loadError => {
          setModelLoadError(`Couldn't check for legacy model files: ${String(loadError)}`)
        })
        .finally(() => setModelsLoading(false)),
    [],
  )

  useEffect(() => {
    loadModels()
  }, [loadModels])

  const retryLoadModels = () => {
    setModelsLoading(true)
    setModelLoadError('')
    loadModels()
  }

  const handleDelete = async filename => {
    setError(null)
    try {
      await invoke('delete_legacy_model_cmd', { filename })
      await loadModels()
    } catch (e) {
      setError(`Couldn't delete that legacy model file: ${e}`)
    }
  }

  const legacyModels = models.filter(model => model.installed)
  const legacySize = legacyModels.reduce((sum, model) => sum + model.sizeMb, 0)

  return (
    <Card>
      <CardHeader>
        <CardTitle>Legacy model files</CardTitle>
        {modelsLoading ? (
          <span role="status" aria-live="polite" className="font-mono text-[10px] text-[hsl(var(--sub))]">
            Checking local storage…
          </span>
        ) : (
          !modelLoadError && (
            <span className="font-mono text-[10px] text-[hsl(var(--sub))]">
              {legacyModels.length} removable {legacyModels.length === 1 ? 'file' : 'files'} · {legacySize.toFixed(1)}{' '}
              MB
            </span>
          )
        )}
      </CardHeader>
      <CardContent className="p-4 flex flex-col gap-3">
        {modelLoadError && (
          <div
            role="alert"
            className="flex items-center gap-2 px-3 py-2 rounded-md bg-destructive/10 text-destructive text-[11px]"
          >
            <AlertCircle size={13} className="shrink-0" />
            <span className="flex-1">{modelLoadError}</span>
            <Button variant="outline" size="sm" onClick={retryLoadModels} disabled={modelsLoading}>
              <RefreshCw size={12} /> Retry
            </Button>
          </div>
        )}

        {error && (
          <div
            role="alert"
            className="flex items-center gap-2 px-3 py-2 rounded-md bg-destructive/10 text-destructive text-[11px]"
          >
            <AlertCircle size={13} className="shrink-0" /> {error}
          </div>
        )}

        {!modelLoadError && legacyModels.length > 0 && (
          <div
            role="status"
            className="flex items-start gap-2 rounded-md border border-warning/40 bg-warning/10 px-3 py-2 text-[11px] text-foreground"
          >
            <AlertCircle size={13} className="mt-0.5 shrink-0 text-warning" />
            <span>
              These {legacyModels.length} unused {legacyModels.length === 1 ? 'file is' : 'files are'} left from an
              earlier build and use {legacySize.toFixed(1)} MB. This release does not use them.
            </span>
          </div>
        )}

        {!modelsLoading && !modelLoadError && legacyModels.length === 0 && (
          <div
            role="status"
            className="rounded-md border border-border bg-secondary/30 px-3 py-3 text-[11px] text-foreground"
          >
            No legacy model files were found. This release does not install or download learned-model files.
          </div>
        )}

        {!modelLoadError && (
          <div className="flex flex-col gap-3">
            {legacyModels.map(m => (
              <div
                key={m.filename}
                className="flex items-center gap-3 rounded-md border border-warning/30 bg-warning/10 px-2 py-2"
              >
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-1.5 text-[12px] font-medium text-foreground">
                    <AlertCircle size={13} className="text-warning shrink-0" />
                    <span className="truncate">{m.displayName}</span>
                  </div>
                  <div className="text-[11px] text-[hsl(var(--sub))] mt-0.5 ml-[19px]">
                    {m.description} — {m.sizeMb} MB
                  </div>
                </div>
                <div className="shrink-0">
                  {m.removable && (
                    <Button
                      variant="ghost"
                      size="icon"
                      className="h-7 w-7 hover:text-destructive"
                      onClick={() => handleDelete(m.filename)}
                      aria-label={`Delete ${m.displayName}`}
                    >
                      <Trash2 size={12} />
                    </Button>
                  )}
                </div>
              </div>
            ))}
          </div>
        )}

        <p className="text-[11px] text-[hsl(var(--sub))] border-t border-border/60 pt-2.5">
          Learned-model processing is not included in v1.0.3. This page only removes unused files left by an older
          build; it never downloads or installs model files.
        </p>
      </CardContent>
    </Card>
  )
}

// ── Panel ──────────────────────────────────────────────────────────────────────

export default function SettingsPanel({ open, onOpenChange, prefs, updater = {} }) {
  const {
    status: updateStatus,
    update,
    progress: updateProgress,
    error: updateError,
    checkForUpdate,
    installUpdate,
  } = updater
  const {
    hpfCutoff,
    setHpfCutoff,
    normalizeLufs,
    setNormalizeLufs,
    normalizeTp,
    setNormalizeTp,
    silenceThresh,
    setSilenceThresh,
    fadeDur,
    setFadeDur,
    ffmpegTimeout,
    setFfmpegTimeout,
    maxScanDepth,
    setMaxScanDepth,
    maxFileSizeGb,
    setMaxFileSizeGb,
    defaultOutputFormat,
    setDefaultOutputFormat,
    defaultOutputMode,
    setDefaultOutputMode,
  } = prefs
  const [section, setSection] = useState('models')
  const [externalLinkError, setExternalLinkError] = useState('')
  const updateProgressPercent = Math.min(100, Math.max(0, Math.round((updateProgress || 0) * 100)))

  const openExternalUrl = async (url, label) => {
    setExternalLinkError('')
    try {
      await openUrl(url)
    } catch (error) {
      setExternalLinkError(`Couldn't open ${label}: ${String(error)}`)
    }
  }

  const applyPreset = preset => {
    const v = preset.values
    setHpfCutoff(v.hpfCutoff)
    setNormalizeLufs(v.normalizeLufs)
    setNormalizeTp(v.normalizeTp)
    setSilenceThresh(v.silenceThresh)
    setFadeDur(v.fadeDur)
  }
  const resetAudio = () => applyPreset(SETTINGS_PRESETS[0])
  const resetDefaults = () => {
    prefs.cycleThemeTo?.(DEFAULTS.theme)
    setDefaultOutputFormat(DEFAULTS.defaultOutputFormat)
    setDefaultOutputMode(DEFAULTS.defaultOutputMode)
  }
  const resetPerformance = () => {
    setFfmpegTimeout(DEFAULTS.ffmpegTimeout)
    setMaxScanDepth(DEFAULTS.maxScanDepth)
    setMaxFileSizeGb(DEFAULTS.maxFileSizeGb)
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
          <nav
            aria-label="Settings sections"
            className="w-16 md:w-44 shrink-0 border-r border-border bg-[hsl(var(--surface))] p-2 flex flex-col gap-0.5"
          >
            {NAV.map(({ id, label, Icon }) => (
              <button
                key={id}
                type="button"
                onClick={() => setSection(id)}
                aria-label={label}
                aria-pressed={section === id}
                aria-current={section === id ? 'true' : undefined}
                title={label}
                className={`flex items-center gap-2.5 px-2.5 md:px-3 py-2 rounded-lg text-[12.5px] font-medium transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring ${
                  section === id
                    ? 'bg-[hsl(var(--gold-dim))] text-foreground'
                    : 'text-[hsl(var(--text2))] hover:bg-secondary/60'
                }`}
              >
                <Icon size={15} className="shrink-0" aria-hidden="true" />
                <span className="hidden md:inline">{label}</span>
              </button>
            ))}
          </nav>

          {/* Content pane */}
          <div className="flex-1 overflow-y-auto p-4 md:p-5 flex flex-col gap-3.5">
            {section === 'models' && <LegacyModelStorage />}

            {section === 'audio' && (
              <>
                <Card>
                  <CardHeader>
                    <CardTitle>Quick setup</CardTitle>
                  </CardHeader>
                  <CardContent className="p-4 flex flex-col gap-2">
                    <p className="text-[11px] text-[hsl(var(--sub))]">Start from a preset, then fine-tune below.</p>
                    <div className="flex gap-1.5 flex-wrap">
                      {SETTINGS_PRESETS.map(p => (
                        <Button
                          key={p.id}
                          variant="outline"
                          size="sm"
                          className="rounded-full"
                          title={p.desc}
                          onClick={() => applyPreset(p)}
                        >
                          {p.name}
                        </Button>
                      ))}
                    </div>
                  </CardContent>
                </Card>

                <ResetCard title="Audio processing" onReset={resetAudio}>
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-x-6 gap-y-4">
                    <NumberField
                      label="Low-frequency cutoff"
                      unit="Hz"
                      hint="Removes rumble and handling noise below this frequency"
                      value={hpfCutoff}
                      setValue={setHpfCutoff}
                      min={20}
                      max={500}
                      step={1}
                      defaultVal={DEFAULTS.hpfCutoff}
                    />
                    <NumberField
                      label="Target volume level"
                      unit="LUFS"
                      hint="How loud the output should be. Lower = quieter. Standard is -16"
                      value={normalizeLufs}
                      setValue={setNormalizeLufs}
                      min={-24}
                      max={-6}
                      step={0.5}
                      defaultVal={DEFAULTS.normalizeLufs}
                    />
                    <NumberField
                      label="Peak limit"
                      unit="dB"
                      hint="Prevents distortion on the loudest moments"
                      value={normalizeTp}
                      setValue={setNormalizeTp}
                      min={-6}
                      max={0}
                      step={0.1}
                      defaultVal={DEFAULTS.normalizeTp}
                    />
                    <NumberField
                      label="Leading silence detection"
                      unit="dB"
                      hint="Audio quieter than this is treated as removable dead air at the start"
                      value={silenceThresh}
                      setValue={setSilenceThresh}
                      min={-70}
                      max={-20}
                      step={1}
                      defaultVal={DEFAULTS.silenceThresh}
                    />
                    <NumberField
                      label="Fade duration"
                      unit="seconds"
                      hint="How long the fade in/out lasts at the start and end"
                      value={fadeDur}
                      setValue={setFadeDur}
                      min={0.1}
                      max={5.0}
                      step={0.1}
                      defaultVal={DEFAULTS.fadeDur}
                    />
                  </div>
                </ResetCard>
              </>
            )}

            {section === 'app' && (
              <>
                <ResetCard title="Defaults" onReset={resetDefaults}>
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-x-6 gap-y-4">
                    <SelectField
                      label="Theme"
                      value={prefs.themePref || 'system'}
                      setValue={v => prefs.cycleThemeTo?.(v)}
                      options={[
                        { value: 'system', label: 'Match system' },
                        { value: 'dark', label: 'Dark' },
                        { value: 'light', label: 'Light' },
                      ]}
                    />
                    <SelectField
                      label="Default output format"
                      hint="Format the app opens with"
                      value={defaultOutputFormat || 'last'}
                      setValue={v => setDefaultOutputFormat(v === 'last' ? '' : v)}
                      options={[
                        { value: 'last', label: 'Remember last used' },
                        { value: 'wav', label: 'WAV (lossless)' },
                        { value: 'mp3', label: 'MP3 (smaller, universal)' },
                        { value: 'flac', label: 'FLAC (lossless, compressed)' },
                        { value: 'opus', label: 'Opus (smallest, voice-optimized)' },
                        { value: 'm4a', label: 'M4A (Apple devices)' },
                      ]}
                    />
                    <SelectField
                      label="Default output mode"
                      hint="Channel layout the app opens with"
                      value={defaultOutputMode || 'last'}
                      setValue={v => setDefaultOutputMode(v === 'last' ? '' : v)}
                      options={[
                        { value: 'last', label: 'Remember last used' },
                        { value: 'stereo', label: 'Mix to Stereo' },
                        { value: 'keep', label: 'Keep Original Channels' },
                        { value: 'split', label: 'Split Channels' },
                      ]}
                    />
                  </div>
                </ResetCard>

                <ResetCard title="Performance & limits" onReset={resetPerformance}>
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-x-6 gap-y-4">
                    <NumberField
                      label="Processing timeout"
                      unit="seconds"
                      hint="Max time allowed per file before canceling"
                      value={ffmpegTimeout}
                      setValue={setFfmpegTimeout}
                      min={60}
                      max={3600}
                      step={10}
                      defaultVal={DEFAULTS.ffmpegTimeout}
                    />
                    <NumberField
                      label="Folder scan depth"
                      unit="levels"
                      hint="How many folder levels deep to search for recordings"
                      value={maxScanDepth}
                      setValue={setMaxScanDepth}
                      min={1}
                      max={20}
                      step={1}
                      defaultVal={DEFAULTS.maxScanDepth}
                    />
                    <NumberField
                      label="Max file size"
                      unit="GB"
                      hint="Files larger than this will be rejected"
                      value={maxFileSizeGb}
                      setValue={setMaxFileSizeGb}
                      min={0.5}
                      max={10}
                      step={0.5}
                      defaultVal={DEFAULTS.maxFileSizeGb}
                    />
                  </div>
                </ResetCard>
              </>
            )}

            {section === 'updates' && (
              <>
                <Card>
                  <CardHeader>
                    <CardTitle>Software update</CardTitle>
                  </CardHeader>
                  <CardContent className="p-4 flex items-center justify-between gap-3">
                    <div className="flex flex-col min-w-0">
                      <span className="text-[12px] font-medium text-foreground">
                        Check for updates from GitHub Releases
                      </span>
                      <span className="text-[11px] text-[hsl(var(--sub))] mt-0.5">
                        <span role="status" aria-live="polite" aria-atomic="true">
                          {updateStatus === 'checking' && 'Checking for updates…'}
                          {updateStatus === 'available' && `Version ${update?.version} is available.`}
                          {updateStatus === 'uptodate' && "You're on the latest version."}
                          {updateStatus === 'downloading' && 'Downloading…'}
                          {updateStatus === 'ready' && 'Update installed — restarting…'}
                          {updateStatus === 'error' && "Couldn't check for updates (offline, or no release published)."}
                          {(!updateStatus || updateStatus === 'idle') &&
                            (updateError
                              ? 'Automatic update check was unavailable. You can download future versions from GitHub.'
                              : 'Checked automatically each time you open the app.')}
                        </span>
                        {updateStatus === 'downloading' && <span aria-hidden="true"> {updateProgressPercent}%</span>}
                      </span>
                      {updateStatus === 'downloading' && (
                        <progress
                          className="sr-only"
                          aria-label="Update download progress"
                          max="100"
                          value={updateProgressPercent}
                        />
                      )}
                    </div>
                    {updateStatus === 'available' ? (
                      <Button size="sm" variant="primary" className="shrink-0" onClick={() => installUpdate?.()}>
                        <Download size={12} /> Update &amp; restart
                      </Button>
                    ) : (
                      <Button
                        size="sm"
                        variant="outline"
                        className="shrink-0"
                        onClick={() => checkForUpdate?.(true)}
                        disabled={
                          updateStatus === 'checking' || updateStatus === 'downloading' || updateStatus === 'ready'
                        }
                      >
                        {updateStatus === 'checking' ? (
                          <>
                            <Loader2 size={12} className="animate-spin" /> Checking…
                          </>
                        ) : (
                          <>
                            <RefreshCw size={12} /> Check for updates
                          </>
                        )}
                      </Button>
                    )}
                  </CardContent>
                </Card>

                <Card>
                  <CardHeader>
                    <CardTitle>About</CardTitle>
                  </CardHeader>
                  <CardContent className="p-4 flex flex-col gap-3">
                    <div className="flex items-center justify-between gap-3">
                      <div className="flex flex-col min-w-0">
                        <span className="text-[12px] font-medium text-foreground">DepoAudio is part of DepoStack</span>
                        <span className="text-[11px] text-[hsl(var(--sub))] mt-0.5">
                          A growing suite of tools built for court reporters. DepoAudio itself stays free and open
                          source.
                        </span>
                      </div>
                      <Button
                        size="sm"
                        variant="outline"
                        className="shrink-0"
                        onClick={() => openExternalUrl(DEPOSTACK_URL, 'depostack.com')}
                      >
                        <ExternalLink size={12} /> depostack.com
                      </Button>
                    </div>

                    <div className="flex items-center justify-between gap-3 border-t border-border pt-3">
                      <div className="flex flex-col min-w-0">
                        <span className="text-[12px] font-medium text-foreground">FFmpeg and audio codecs</span>
                        <span className="text-[11px] text-[hsl(var(--sub))] mt-0.5">
                          DepoAudio uses FFmpeg under LGPL v2.1 or later. License notices and reviewed build/source
                          references are included; corresponding-source delivery is verified before publication.
                        </span>
                      </div>
                      <Button
                        size="sm"
                        variant="outline"
                        className="shrink-0"
                        aria-label="Open DepoAudio releases with FFmpeg source and license evidence"
                        onClick={() => openExternalUrl(DEPOAUDIO_RELEASES_URL, 'DepoAudio release evidence')}
                      >
                        <ExternalLink size={12} /> Evidence
                      </Button>
                    </div>
                    {externalLinkError && (
                      <p
                        role="alert"
                        className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-[11px] text-destructive"
                      >
                        {externalLinkError}
                      </p>
                    )}
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
