import { useState, useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'

export default function usePreferences() {
  const [mode, setMode]           = useState('stereo')
  const [formatOut, setFormatOut] = useState('wav')
  const [labels, setLabels]       = useState(['Speaker 1','Speaker 2','Speaker 3','Speaker 4'])
  const [chanVols, setChanVols]   = useState([1,1,1,1])
  const [outDir, setOutDir]       = useState('')
  const [rate, setRate]           = useState('48000')
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
  const [prefsReady, setPrefsReady] = useState(false)

  // Load prefs on mount
  useEffect(() => {
    invoke('prefs_get').then(p => {
      if (p.mode)      setMode(p.mode)
      if (p.format)    setFormatOut(p.format)
      if (p.rate)      setRate(p.rate)
      if (p.outDir !== undefined) setOutDir(p.outDir)
      if (p.labels?.length) setLabels(p.labels)
      if (p.chanVols?.length) setChanVols(p.chanVols)
      setNormalize(!!p.normalize); setTrim(!!p.trim)
      setFade(!!p.fade); setFadeDur(p.fadeDur || 0.5); setHpf(!!p.hpf)
      setDenoise(!!p.denoise); setDenoiseQuality(p.denoiseQuality || 'fast'); setAutoLevel(!!p.autoLevel)
      setDeclip(!!p.declip); setEnhance(!!p.enhance)
      setPrefsReady(true)
    }).catch(() => setPrefsReady(true))
  }, [])

  // Persist prefs on change
  useEffect(() => {
    if (!prefsReady) return
    invoke('prefs_set', { patch: { mode, format: formatOut, rate, outDir, labels, chanVols, normalize, trim, fade, fadeDur, hpf, denoise, denoiseQuality, autoLevel, declip, enhance } })
  }, [mode, formatOut, rate, outDir, labels, chanVols, normalize, trim, fade, fadeDur, hpf, denoise, denoiseQuality, autoLevel, declip, enhance, prefsReady])

  return {
    mode, setMode,
    formatOut, setFormatOut,
    labels, setLabels,
    chanVols, setChanVols,
    outDir, setOutDir,
    rate, setRate,
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
    prefsReady,
  }
}
