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
  // Audio Processing
  hpfCutoff: 80,
  normalizeLufs: -16,
  normalizeTp: -1.5,
  silenceThresh: -50,
  defaultFadeDur: 0.5,
  // Performance
  ffmpegTimeout: 300,
  maxScanDepth: 5,
  // Security
  maxFileSizeGb: 2,
  // Appearance
  defaultOutputFormat: 'wav',
  defaultOutputMode: 'stereo',
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function NumberField({ label, unit, value, setValue, min, max, step = 1, defaultVal }) {
  return (
    <div className="settings-field">
      <Label className="settings-label">
        {label}
        {unit && <span className="settings-unit">({unit})</span>}
      </Label>
      <Input
        type="number"
        className="settings-input"
        value={value}
        min={min}
        max={max}
        step={step}
        placeholder={String(defaultVal)}
        onChange={e => {
          const v = parseFloat(e.target.value)
          if (!isNaN(v) && v >= min && v <= max) setValue(v)
        }}
      />
    </div>
  )
}

function SelectField({ label, value, setValue, options }) {
  return (
    <div className="settings-field">
      <Label className="settings-label">{label}</Label>
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

  const loadModels = useCallback(async () => {
    try {
      const [catalog, capabilities] = await Promise.all([
        invoke('model_catalog_cmd'),
        invoke('system_capabilities_cmd'),
      ])
      setModels(catalog)
      setCaps(capabilities)
    } catch {
      // Not in Tauri context (dev mode)
      setModels([])
    }
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

  // Group by feature
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
    defaultFadeDur, setDefaultFadeDur,
    ffmpegTimeout, setFfmpegTimeout,
    maxScanDepth, setMaxScanDepth,
    maxFileSizeGb, setMaxFileSizeGb,
    defaultOutputFormat, setDefaultOutputFormat,
    defaultOutputMode, setDefaultOutputMode,
  } = prefs

  // Also pull theme from useTheme if passed, but theme is managed separately
  // via the cycleTheme in the header. We show it read-only or let it be set here.

  const resetAudio = () => {
    setHpfCutoff(DEFAULTS.hpfCutoff)
    setNormalizeLufs(DEFAULTS.normalizeLufs)
    setNormalizeTp(DEFAULTS.normalizeTp)
    setSilenceThresh(DEFAULTS.silenceThresh)
    setDefaultFadeDur(DEFAULTS.defaultFadeDur)
  }

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

          {/* ── Audio Processing ──────────────────────────────── */}
          <section className="settings-section">
            <SectionHeader title="Audio Processing" onReset={resetAudio} />
            <div className="settings-grid">
              <NumberField
                label="HPF Cutoff Frequency" unit="Hz"
                value={hpfCutoff} setValue={setHpfCutoff}
                min={20} max={500} step={1} defaultVal={DEFAULTS.hpfCutoff}
              />
              <NumberField
                label="Normalize Target" unit="LUFS"
                value={normalizeLufs} setValue={setNormalizeLufs}
                min={-24} max={-6} step={0.5} defaultVal={DEFAULTS.normalizeLufs}
              />
              <NumberField
                label="Normalize True Peak" unit="dB"
                value={normalizeTp} setValue={setNormalizeTp}
                min={-6} max={0} step={0.1} defaultVal={DEFAULTS.normalizeTp}
              />
              <NumberField
                label="Silence Threshold" unit="dB"
                value={silenceThresh} setValue={setSilenceThresh}
                min={-70} max={-20} step={1} defaultVal={DEFAULTS.silenceThresh}
              />
              <NumberField
                label="Default Fade Duration" unit="s"
                value={defaultFadeDur} setValue={setDefaultFadeDur}
                min={0.1} max={5.0} step={0.1} defaultVal={DEFAULTS.defaultFadeDur}
              />
            </div>
          </section>

          {/* ── Performance ───────────────────────────────────── */}
          <section className="settings-section">
            <SectionHeader title="Performance" onReset={resetPerformance} />
            <div className="settings-grid">
              <NumberField
                label="FFmpeg Timeout" unit="seconds"
                value={ffmpegTimeout} setValue={setFfmpegTimeout}
                min={60} max={3600} step={10} defaultVal={DEFAULTS.ffmpegTimeout}
              />
              <NumberField
                label="Max Scan Depth" unit="directories"
                value={maxScanDepth} setValue={setMaxScanDepth}
                min={1} max={20} step={1} defaultVal={DEFAULTS.maxScanDepth}
              />
            </div>
          </section>

          {/* ── Security ──────────────────────────────────────── */}
          <section className="settings-section">
            <SectionHeader title="Security" onReset={resetSecurity} />
            <div className="settings-grid">
              <NumberField
                label="Max File Size" unit="GB"
                value={maxFileSizeGb} setValue={setMaxFileSizeGb}
                min={0.5} max={10} step={0.5} defaultVal={DEFAULTS.maxFileSizeGb}
              />
            </div>
          </section>

          {/* ── Appearance ────────────────────────────────────── */}
          <section className="settings-section">
            <SectionHeader title="Appearance" onReset={resetAppearance} />
            <div className="settings-grid">
              <SelectField
                label="Theme"
                value={prefs.themePref || 'system'}
                setValue={v => prefs.cycleThemeTo?.(v)}
                options={[
                  { value: 'system', label: 'System' },
                  { value: 'dark', label: 'Dark' },
                  { value: 'light', label: 'Light' },
                ]}
              />
              <SelectField
                label="Default Output Format"
                value={defaultOutputFormat}
                setValue={setDefaultOutputFormat}
                options={[
                  { value: 'wav', label: 'WAV' },
                  { value: 'mp3', label: 'MP3' },
                  { value: 'flac', label: 'FLAC' },
                  { value: 'opus', label: 'Opus' },
                  { value: 'm4a', label: 'M4A' },
                ]}
              />
              <SelectField
                label="Default Output Mode"
                value={defaultOutputMode}
                setValue={setDefaultOutputMode}
                options={[
                  { value: 'stereo', label: 'Stereo' },
                  { value: 'keep', label: 'Keep Original' },
                  { value: 'split', label: 'Split Channels' },
                ]}
              />
            </div>
          </section>
        </div>
      </DialogContent>
    </Dialog>
  )
}
