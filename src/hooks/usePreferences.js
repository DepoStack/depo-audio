import { useState, useEffect, useCallback, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'

export default function usePreferences() {
  const [mode, setModeState] = useState('stereo')
  const [formatOut, setFormatOut] = useState('wav')
  const [labels, setLabels] = useState(['Microphone 1', 'Microphone 2', 'Microphone 3', 'Microphone 4'])
  const [chanVols, setChanVols] = useState([1, 1, 1, 1])
  const [outDir, setOutDir] = useState('')
  const [rate, setRate] = useState('48000')
  const [mp3Bitrate, setMp3Bitrate] = useState(192)
  const [normalize, setNormalize] = useState(false)
  const [trim, setTrim] = useState(false)
  const [fade, setFade] = useState(false)
  const [fadeDur, setFadeDur] = useState(0.5)
  const [hpf, setHpf] = useState(false)
  const [autoLevel, setAutoLevelState] = useState(false)
  const [declip, setDeclip] = useState(false)
  // Advanced settings
  const [hpfCutoff, setHpfCutoff] = useState(80)
  const [normalizeLufs, setNormalizeLufs] = useState(-16)
  const [normalizeTp, setNormalizeTp] = useState(-1.5)
  const [silenceThresh, setSilenceThresh] = useState(-50)
  const [ffmpegTimeout, setFfmpegTimeout] = useState(300)
  const [maxScanDepth, setMaxScanDepth] = useState(5)
  const [maxFileSizeGb, setMaxFileSizeGb] = useState(2)
  // Empty string is a real persisted value: it means "remember last used".
  const [defaultOutputFormat, setDefaultOutputFormat] = useState('')
  const [defaultOutputMode, setDefaultOutputMode] = useState('')
  const [themePref, setThemePref] = useState('system')
  const [prefsReady, setPrefsReady] = useState(false)
  const [prefsError, setPrefsError] = useState('')
  const modeRef = useRef(mode)

  // Keeping the original channel layout and cross-channel auto-leveling are
  // mutually exclusive. Centralize the transition so every UI caller clears
  // the incompatible option immediately.
  const setMode = useCallback(nextMode => {
    const resolvedMode = typeof nextMode === 'function' ? nextMode(modeRef.current) : nextMode
    modeRef.current = resolvedMode
    setModeState(resolvedMode)
    if (resolvedMode === 'keep') setAutoLevelState(false)
  }, [])

  const setAutoLevel = useCallback(nextValue => {
    setAutoLevelState(currentValue => {
      const resolvedValue = typeof nextValue === 'function' ? nextValue(currentValue) : nextValue
      return modeRef.current === 'keep' ? false : Boolean(resolvedValue)
    })
  }, [])

  // Load prefs on mount
  useEffect(() => {
    invoke('prefs_get')
      .then(p => {
        // A configured "Default Output ..." setting wins on startup; the empty
        // string means "remember last used" (the out-of-box behavior)
        const startMode = p.defaultOutputMode || p.mode
        const startFormat = p.defaultOutputFormat || p.format
        const storedTheme = ['system', 'dark', 'light'].includes(p.theme) ? p.theme : 'system'
        if (startMode) {
          modeRef.current = startMode
          setModeState(startMode)
        }
        if (startFormat) setFormatOut(startFormat)
        setThemePref(storedTheme)
        if (p.rate) setRate(p.rate)
        if (p.mp3Bitrate != null) setMp3Bitrate(p.mp3Bitrate)
        if (p.outDir !== undefined) setOutDir(p.outDir)
        if (p.labels?.length) setLabels(p.labels)
        if (p.chanVols?.length) setChanVols(p.chanVols)
        setNormalize(!!p.normalize)
        setTrim(!!p.trim)
        setFade(!!p.fade)
        setFadeDur(p.fadeDur ?? 0.5)
        setHpf(!!p.hpf)
        setAutoLevelState(startMode !== 'keep' && !!p.autoLevel)
        setDeclip(!!p.declip)
        // Advanced settings
        if (p.hpfCutoff != null) setHpfCutoff(p.hpfCutoff)
        if (p.normalizeLufs != null) setNormalizeLufs(p.normalizeLufs)
        if (p.normalizeTp != null) setNormalizeTp(p.normalizeTp)
        if (p.silenceThresh != null) setSilenceThresh(p.silenceThresh)
        if (p.ffmpegTimeout != null) setFfmpegTimeout(p.ffmpegTimeout)
        if (p.maxScanDepth != null) setMaxScanDepth(p.maxScanDepth)
        if (p.maxFileSizeGb != null) setMaxFileSizeGb(p.maxFileSizeGb)
        if (p.defaultOutputFormat !== undefined) setDefaultOutputFormat(p.defaultOutputFormat ?? '')
        if (p.defaultOutputMode !== undefined) setDefaultOutputMode(p.defaultOutputMode ?? '')
        setPrefsReady(true)
      })
      .catch(e => {
        setPrefsError(
          `Preferences could not be loaded: ${String(e)}. Saving is disabled to protect the existing preferences file.`,
        )
        setPrefsReady(true)
      })
  }, [])

  // Persist prefs on change (debounced)
  useEffect(() => {
    if (!prefsReady || prefsError) return
    const timer = setTimeout(() => {
      invoke('prefs_set', {
        patch: {
          theme: themePref,
          mode,
          format: formatOut,
          rate,
          mp3Bitrate,
          outDir,
          labels,
          chanVols,
          normalize,
          trim,
          fade,
          fadeDur,
          hpf,
          // v1.0.3 does not distribute learned models. Persist false so stale
          // preferences from earlier builds cannot re-enable hidden paths.
          denoise: false,
          denoiseQuality: 'fast',
          autoLevel: mode !== 'keep' && autoLevel,
          declip,
          enhance: false,
          dereverb: false,
          hpfCutoff,
          normalizeLufs,
          normalizeTp,
          silenceThresh,
          ffmpegTimeout,
          maxScanDepth,
          maxFileSizeGb,
          defaultOutputFormat,
          defaultOutputMode,
        },
      }).catch(e => {
        console.error('Preference save failed:', e)
        setPrefsError(
          `Preferences could not be saved: ${String(e)}. Further preference writes are disabled for this session.`,
        )
      })
    }, 500)
    return () => clearTimeout(timer)
  }, [
    themePref,
    mode,
    formatOut,
    rate,
    mp3Bitrate,
    outDir,
    labels,
    chanVols,
    normalize,
    trim,
    fade,
    fadeDur,
    hpf,
    autoLevel,
    declip,
    hpfCutoff,
    normalizeLufs,
    normalizeTp,
    silenceThresh,
    ffmpegTimeout,
    maxScanDepth,
    maxFileSizeGb,
    defaultOutputFormat,
    defaultOutputMode,
    prefsReady,
    prefsError,
  ])

  return {
    themePref,
    setThemePref,
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
    hpfCutoff,
    setHpfCutoff,
    normalizeLufs,
    setNormalizeLufs,
    normalizeTp,
    setNormalizeTp,
    silenceThresh,
    setSilenceThresh,
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
    prefsReady,
    prefsError,
  }
}
