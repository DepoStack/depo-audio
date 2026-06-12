import { useState, useEffect, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { RotateCcw, Download, Trash2, CheckCircle, Loader2, AlertCircle, Cpu } from 'lucide-react'
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogClose,
} from './ui/dialog'
import { Button } from './ui/button'
import { Input } from './ui/input'
import { Label } from './ui/label'
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from './ui/select'

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

// ── Settings presets ──────────────────────────────────────────────────────────

const SETTINGS_PRESETS = [
  {
    id: 'recommended',
    name: 'Recommended',
    desc: 'Best for most court recordings',
    values: { ...DEFAULTS },
  },
  {
    id: 'high-quality',
    name: 'High Quality',
    desc: 'Louder output, tighter silence trim',
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

// ── Helpers ───────────────────────────────────────────────────────────────────

function NumberField({ label, hint, unit, value, setValue, min, max, step = 1, defaultVal }) {
  // Hold the raw text locally so intermediate keystrokes ("-", "1" on the way
  // to "150") aren't reverted by the controlled input; clamp on blur
  const [text, setText] = useState(String(value))
  const [prevValue, setPrevValue] = useState(value)
  if (value !== prevValue) {
    setPrevValue(value)
    setText(String(value))
  }

  const commit = () => {
    const v = parseFloat(text)
    if (isNaN(v)) { setText(String(value)); return }
    const clamped = Math.min(max, Math.max(min, v))
    setValue(clamped)
    setText(String(clamped))
  }

  return (
    <div className="settings-field">
      <Label className="settings-label">
        {label}
        {unit && <span className="settings-unit">({unit})</span>}
      </Label>
      {hint && <p className="settings-hint">{hint}</p>}
      <Input
        type="number"
        className="settings-input"
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
    <div className="settings-field">
      <Label className="settings-label">{label}</Label>
      {hint && <p className="settings-hint">{hint}</p>}
      <Select value={value} onValueChange={setValue}>
        <SelectTrigger className="settings-select">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {options.map(o => (
            <SelectItem key={o.value} value={o.value}>{o.label}</SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  )
}

function SectionHeader({ title, onReset }) {
  return (
    <div className="settings-section-header">
      <h3 className="settings-section-title">{title}</h3>
      <Button
        variant="ghost"
        size="sm"
        className="settings-reset-btn"
        onClick={onReset}
        title="Reset section to defaults"
      >
        <RotateCcw size={12} />
        <span>Reset</span>
      </Button>
    </div>
  )
}

// ── Model Manager ────────────────────────────────────────────────────────────

function ModelManager() {
  const [models, setModels] = useState([])
  const [caps, setCaps] = useState(null)
  const [downloading, setDownloading] = useState({})
  const [error, setError] = useState(null)

  const loadModels = useCallback(() => {
    return Promise.all([
      invoke('model_catalog_cmd'),
      invoke('system_capabilities_cmd'),
    ]).then(([catalog, capabilities]) => {
      setModels(catalog)
      setCaps(capabilities)
    }).catch(() => {
      setModels([])
    })
  }, [])

  useEffect(() => { loadModels() }, [loadModels])

  const handleDownload = async (filename) => {
    setDownloading(d => ({ ...d, [filename]: true }))
    setError(null)
    try {
      await invoke('download_model_cmd', { filename })
      await loadModels()
    } catch (e) {
      setError(`Failed to download: ${e}`)
    } finally {
      setDownloading(d => ({ ...d, [filename]: false }))
    }
  }

  const handleDelete = async (filename) => {
    setError(null)
    try {
      await invoke('delete_model_cmd', { filename })
      await loadModels()
    } catch (e) {
      setError(`Failed to delete: ${e}`)
    }
  }

  const groups = {}
  models.forEach(m => {
    if (!groups[m.feature]) groups[m.feature] = []
    groups[m.feature].push(m)
  })

  const installedCount = models.filter(m => m.installed).length
  const totalSize = models.filter(m => m.installed).reduce((s, m) => s + m.sizeMb, 0)

  return (
    <div className="model-manager">
      {caps && (
        <div className="model-caps">
          <Cpu size={14} />
          <span>{caps.acceleratorDesc}</span>
          <span className="model-cap-badge">{caps.tier} tier</span>
          <span className="model-cap-badge">{caps.cpuCores} cores</span>
          <span className="model-cap-badge">{Math.round(caps.ramMb / 1024)} GB RAM</span>
        </div>
      )}

      <div className="model-summary">
        {installedCount}/{models.length} models installed ({totalSize.toFixed(1)} MB)
      </div>

      {error && (
        <div className="model-error">
          <AlertCircle size={14} /> {error}
        </div>
      )}

      {Object.entries(groups).map(([feature, items]) => (
        <div key={feature} className="model-group">
          <div className="model-group-title">{feature}</div>
          {items.map(m => (
            <div key={m.filename} className="model-row">
              <div className="model-info">
                <div className="model-name">
                  {m.installed ? <CheckCircle size={14} className="model-installed" /> : <Download size={14} className="model-missing" />}
                  {m.displayName}
                  {m.required && <span className="model-badge-req">Required</span>}
                  {m.recommended && !m.required && <span className="model-badge-rec">Recommended</span>}
                </div>
                <div className="model-desc">{m.description} — {m.sizeMb} MB</div>
              </div>
              <div className="model-actions">
                {!m.installed && (
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={downloading[m.filename]}
                    onClick={() => handleDownload(m.filename)}
                    aria-label={`Download ${m.displayName}`}
                  >
                    {downloading[m.filename]
                      ? <><Loader2 size={12} className="animate-spin" /> Downloading...</>
                      : <><Download size={12} /> Install</>
                    }
                  </Button>
                )}
                {m.installed && !m.required && (
                  <Button
                    variant="ghost"
                    size="sm"
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
      ))}

      <div className="model-footer">
        Models run 100% locally. No data leaves your machine.
      </div>
    </div>
  )
}

// ── Main component ────────────────────────────────────────────────────────────

export default function SettingsPanel({ open, onOpenChange, prefs }) {
  const {
    hpfCutoff, setHpfCutoff,
    normalizeLufs, setNormalizeLufs,
    normalizeTp, setNormalizeTp,
    silenceThresh, setSilenceThresh,
    fadeDur, setFadeDur,
    ffmpegTimeout, setFfmpegTimeout,
    maxScanDepth, setMaxScanDepth,
    maxFileSizeGb, setMaxFileSizeGb,
    defaultOutputFormat, setDefaultOutputFormat,
    defaultOutputMode, setDefaultOutputMode,
  } = prefs

  const applyPreset = (preset) => {
    const v = preset.values
    setHpfCutoff(v.hpfCutoff)
    setNormalizeLufs(v.normalizeLufs)
    setNormalizeTp(v.normalizeTp)
    setSilenceThresh(v.silenceThresh)
    setFadeDur(v.fadeDur)
  }

  const resetAudio = () => applyPreset(SETTINGS_PRESETS[0])

  const resetPerformance = () => {
    setFfmpegTimeout(DEFAULTS.ffmpegTimeout)
    setMaxScanDepth(DEFAULTS.maxScanDepth)
  }

  const resetSecurity = () => {
    setMaxFileSizeGb(DEFAULTS.maxFileSizeGb)
  }

  const resetAppearance = () => {
    setDefaultOutputFormat(DEFAULTS.defaultOutputFormat)
    setDefaultOutputMode(DEFAULTS.defaultOutputMode)
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[520px]">
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
          <DialogClose />
        </DialogHeader>
        <DialogDescription className="sr-only">Application settings</DialogDescription>

        <div className="settings-body">
          {/* ── AI Models ────────────────────────────────────── */}
          <section className="settings-section">
            <h3 className="settings-section-title">AI Models</h3>
            <ModelManager />
          </section>

          {/* ── Audio Presets ──────────────────────────────────── */}
          <section className="settings-section">
            <h3 className="settings-section-title">Quick Setup</h3>
            <p className="settings-hint mb-2">Choose a preset to configure audio settings, or customize below.</p>
            <div className="flex gap-1.5 flex-wrap">
              {SETTINGS_PRESETS.map(p => (
                <Button key={p.id} variant="outline" size="sm" className="rounded-full"
                  title={p.desc} onClick={() => applyPreset(p)}>
                  {p.name}
                </Button>
              ))}
            </div>
          </section>

          {/* ── Audio Processing ──────────────────────────────── */}
          <section className="settings-section">
            <SectionHeader title="Audio Processing" onReset={resetAudio} />
            <div className="settings-grid">
              <NumberField
                label="Low-Frequency Cutoff" unit="Hz"
                hint="Removes rumble and handling noise below this frequency"
                value={hpfCutoff} setValue={setHpfCutoff}
                min={20} max={500} step={1} defaultVal={DEFAULTS.hpfCutoff}
              />
              <NumberField
                label="Target Volume Level" unit="LUFS"
                hint="How loud the output should be. Lower = quieter. Standard is -16"
                value={normalizeLufs} setValue={setNormalizeLufs}
                min={-24} max={-6} step={0.5} defaultVal={DEFAULTS.normalizeLufs}
              />
              <NumberField
                label="Peak Limit" unit="dB"
                hint="Prevents distortion on the loudest moments"
                value={normalizeTp} setValue={setNormalizeTp}
                min={-6} max={0} step={0.1} defaultVal={DEFAULTS.normalizeTp}
              />
              <NumberField
                label="Silence Detection" unit="dB"
                hint="Audio quieter than this is treated as silence for trimming"
                value={silenceThresh} setValue={setSilenceThresh}
                min={-70} max={-20} step={1} defaultVal={DEFAULTS.silenceThresh}
              />
              <NumberField
                label="Fade Duration" unit="seconds"
                hint="How long the fade in/out lasts at the start and end"
                value={fadeDur} setValue={setFadeDur}
                min={0.1} max={5.0} step={0.1} defaultVal={DEFAULTS.fadeDur}
              />
            </div>
          </section>

          {/* ── Performance ───────────────────────────────────── */}
          <section className="settings-section">
            <SectionHeader title="Performance" onReset={resetPerformance} />
            <div className="settings-grid">
              <NumberField
                label="Processing Timeout" unit="seconds"
                hint="Max time allowed per file before canceling"
                value={ffmpegTimeout} setValue={setFfmpegTimeout}
                min={60} max={3600} step={10} defaultVal={DEFAULTS.ffmpegTimeout}
              />
              <NumberField
                label="Folder Scan Depth" unit="levels"
                hint="How many folder levels deep to search for recordings"
                value={maxScanDepth} setValue={setMaxScanDepth}
                min={1} max={20} step={1} defaultVal={DEFAULTS.maxScanDepth}
              />
            </div>
          </section>

          {/* ── Security ──────────────────────────────────────── */}
          <section className="settings-section">
            <SectionHeader title="Limits" onReset={resetSecurity} />
            <div className="settings-grid">
              <NumberField
                label="Max File Size" unit="GB"
                hint="Files larger than this will be rejected"
                value={maxFileSizeGb} setValue={setMaxFileSizeGb}
                min={0.5} max={10} step={0.5} defaultVal={DEFAULTS.maxFileSizeGb}
              />
            </div>
          </section>

          {/* ── Defaults ──────────────────────────────────────── */}
          <section className="settings-section">
            <SectionHeader title="Defaults" onReset={resetAppearance} />
            <div className="settings-grid">
              <SelectField
                label="Theme"
                value={prefs.themePref || 'system'}
                setValue={v => prefs.cycleThemeTo?.(v)}
                options={[
                  { value: 'system', label: 'Match System' },
                  { value: 'dark', label: 'Dark' },
                  { value: 'light', label: 'Light' },
                ]}
              />
              <SelectField
                label="Default Output Format"
                hint="Format the app opens with — or remember whatever you used last"
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
                label="Default Output Mode"
                hint="Channel layout the app opens with — or remember whatever you used last"
                value={defaultOutputMode || 'last'}
                setValue={v => setDefaultOutputMode(v === 'last' ? '' : v)}
                options={[
                  { value: 'last', label: 'Remember last used' },
                  { value: 'stereo', label: 'Mix to Stereo' },
                  { value: 'keep', label: 'Keep Original Channels' },
                  { value: 'split', label: 'Split by Speaker' },
                ]}
              />
            </div>
          </section>
        </div>
      </DialogContent>
    </Dialog>
  )
}
