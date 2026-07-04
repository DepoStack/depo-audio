import { useState, useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'

export default function usePreferences() {
  const [mode, setMode]           = useState('stereo')
  const [formatOut, setFormatOut] = useState('wav')
  const [labels, setLabels]       = useState(['Speaker 1','Speaker 2','Speaker 3','Speaker 4'])
  const [chanVols, setChanVols]   = useState([1,1,1,1])
  const [outDir, setOutDir]       = useState('')
  const [rate, setRate]           = useState('48000')
  const [mp3Bitrate, setMp3Bitrate] = useState(192)
  const [normalize, setNormalize] = useState(false)
  const [trim, setTrim]           = useState(false)
  const [fade, setFade]           = useState(false)
  const [fadeDur, setFadeDur]     = useState(0.5)
  const [hpf, setHpf]             = useState(false)
  const [denoise, setDenoise]     = useState(false)
  const [denoiseQuality, setDenoiseQuality] = useState('fast')
  const [autoLevel, setAutoLevel] = useState(false)
  const [declip, setDeclip]       = useState(false)
  const [enhance, setEnhance]     = useState(false)
  const [dereverb, setDereverb]   = useState(false)
  // Advanced settings
  const [hpfCutoff, setHpfCutoff]               = useState(80)
  const [normalizeLufs, setNormalizeLufs]       = useState(-16)
  const [normalizeTp, setNormalizeTp]           = useState(-1.5)
  const [silenceThresh, setSilenceThresh]       = useState(-50)
  const [ffmpegTimeout, setFfmpegTimeout]       = useState(300)
  const [maxScanDepth, setMaxScanDepth]         = useState(5)
  const [maxFileSizeGb, setMaxFileSizeGb]       = useState(2)
  const [defaultOutputFormat, setDefaultOutputFormat] = useState('wav')
  const [defaultOutputMode, setDefaultOutputMode]     = useState('stereo')
  const [prefsReady, setPrefsReady] = useState(false)

  // Load prefs on mount
  useEffect(() => {
    invoke('prefs_get').then(p => {
      // A configured "Default Output ..." setting wins on startup; the empty
      // string means "remember last used" (the out-of-box behavior)
      const startMode = p.defaultOutputMode || p.mode
      const startFormat = p.defaultOutputFormat || p.format
      if (startMode)   setMode(startMode)
      if (startFormat) setFormatOut(startFormat)
      if (p.rate)      setRate(p.rate)
      if (p.mp3Bitrate != null) setMp3Bitrate(p.mp3Bitrate)
      if (p.outDir !== undefined) setOutDir(p.outDir)
      if (p.labels?.length) setLabels(p.labels)
      if (p.chanVols?.length) setChanVols(p.chanVols)
      setNormalize(!!p.normalize); setTrim(!!p.trim)
      setFade(!!p.fade); setFadeDur(p.fadeDur ?? 0.5); setHpf(!!p.hpf)
      setDenoise(!!p.denoise); setDenoiseQuality(p.denoiseQuality || 'fast'); setAutoLevel(!!p.autoLevel)
      setDeclip(!!p.declip); setEnhance(!!p.enhance); setDereverb(!!p.dereverb)
      // Advanced settings
      if (p.hpfCutoff != null) setHpfCutoff(p.hpfCutoff)
      if (p.normalizeLufs != null) setNormalizeLufs(p.normalizeLufs)
      if (p.normalizeTp != null) setNormalizeTp(p.normalizeTp)
      if (p.silenceThresh != null) setSilenceThresh(p.silenceThresh)
      if (p.ffmpegTimeout != null) setFfmpegTimeout(p.ffmpegTimeout)
      if (p.maxScanDepth != null) setMaxScanDepth(p.maxScanDepth)
      if (p.maxFileSizeGb != null) setMaxFileSizeGb(p.maxFileSizeGb)
      if (p.defaultOutputFormat) setDefaultOutputFormat(p.defaultOutputFormat)
      if (p.defaultOutputMode) setDefaultOutputMode(p.defaultOutputMode)
      setPrefsReady(true)
    }).catch(() => setPrefsReady(true))
  }, [])

  // Persist prefs on change (debounced)
  useEffect(() => {
    if (!prefsReady) return
    const timer = setTimeout(() => {
      invoke('prefs_set', { patch: {
        mode, format: formatOut, rate, mp3Bitrate, outDir, labels, chanVols, normalize, trim, fade, fadeDur, hpf,
        denoise, denoiseQuality, autoLevel, declip, enhance, dereverb,
        hpfCutoff, normalizeLufs, normalizeTp, silenceThresh,
        ffmpegTimeout, maxScanDepth, maxFileSizeGb, defaultOutputFormat, defaultOutputMode,
      } })
    }, 500)
    return () => clearTimeout(timer)
  }, [mode, formatOut, rate, mp3Bitrate, outDir, labels, chanVols, normalize, trim, fade, fadeDur, hpf,
      denoise, denoiseQuality, autoLevel, declip, enhance, dereverb,
      hpfCutoff, normalizeLufs, normalizeTp, silenceThresh,
      ffmpegTimeout, maxScanDepth, maxFileSizeGb, defaultOutputFormat, defaultOutputMode,
      prefsReady])

  return {
    mode, setMode,
    formatOut, setFormatOut,
    labels, setLabels,
    chanVols, setChanVols,
    outDir, setOutDir,
    rate, setRate,
    mp3Bitrate, setMp3Bitrate,
    normalize, setNormalize,
    trim, setTrim,
    fade, setFade,
    fadeDur, setFadeDur,
    hpf, setHpf,
    denoise, setDenoise,
    denoiseQuality, setDenoiseQuality,
    autoLevel, setAutoLevel,
    declip, setDeclip,
    enhance, setEnhance,
    dereverb, setDereverb,
    hpfCutoff, setHpfCutoff,
    normalizeLufs, setNormalizeLufs,
    normalizeTp, setNormalizeTp,
    silenceThresh, setSilenceThresh,
    ffmpegTimeout, setFfmpegTimeout,
    maxScanDepth, setMaxScanDepth,
    maxFileSizeGb, setMaxFileSizeGb,
    defaultOutputFormat, setDefaultOutputFormat,
    defaultOutputMode, setDefaultOutputMode,
    prefsReady,
  }
}
