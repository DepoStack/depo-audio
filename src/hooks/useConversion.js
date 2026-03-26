import { useState, useCallback, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

let jobCounter = 0

export default function useConversion() {
  const [jobs, setJobs]             = useState({})
  const [converting, setConverting] = useState(false)
  const convertingRef = useRef(false)

  const startConversion = useCallback(async ({ files, outDir, mode, formatOut, rate, labels, chanVols, normalize, trim, fade, fadeDur, hpf, denoise, denoiseQuality, autoLevel, declip, enhance, dereverb, hpfCutoff, normalizeLufs, normalizeTp, silenceThresh, ffmpegTimeout, maxFileSizeGb, caseName, setCases }) => {
    if (convertingRef.current || !files.length) return
    convertingRef.current = true
    setConverting(true)
    setJobs(Object.fromEntries(files.map(f => [f.path, { status:'queued', outputs:[], error:null }])))

    const unlistenProg = await listen('convert:progress', ({ payload }) => {
      setJobs(prev => {
        const match = Object.entries(prev).find(([,j]) => j.id === payload.id)
        if (!match) return prev
        return { ...prev, [match[0]]: { ...match[1], seconds: payload.seconds, phase: payload.phase || null } }
      })
    })
    const unlistenDone = await listen('convert:done', ({ payload }) => {
      setJobs(prev => {
        const match = Object.entries(prev).find(([,j]) => j.id === payload.id)
        if (!match) return prev
        return { ...prev, [match[0]]: { ...match[1], status:'done', outputs: payload.files } }
      })
    })
    const unlistenErr = await listen('convert:error', ({ payload }) => {
      setJobs(prev => {
        const match = Object.entries(prev).find(([,j]) => j.id === payload.id)
        if (!match) return prev
        return { ...prev, [match[0]]: { ...match[1], status:'error', error: payload.message } }
      })
    })

    for (const file of files) {
      const id = `job_${++jobCounter}`
      setJobs(prev => ({ ...prev, [file.path]: { ...prev[file.path], status:'converting', id } }))
      // Extract parent directory using the last path separator (works on both Windows and Unix)
      const lastSep = Math.max(file.path.lastIndexOf('/'), file.path.lastIndexOf('\\'))
      const resolved = outDir || (lastSep > 0 ? file.path.substring(0, lastSep) : '')

      // Use event-driven completion instead of polling
      await new Promise(resolve => {
        let settled = false
        const settle = () => { if (!settled) { settled = true; resolve() } }

        const unDone = listen('convert:done', ({ payload }) => {
          if (payload.id === id) { unDone.then(fn => fn()); settle() }
        })
        const unErr = listen('convert:error', ({ payload }) => {
          if (payload.id === id) { unErr.then(fn => fn()); settle() }
        })

        invoke('convert', { job: {
          id, srcPath: file.path, outDir: resolved, mode,
          format: formatOut, rate: formatOut === 'opus' ? '48000' : rate,
          labels, chanVols, normalize, trim, fade, fadeDur, hpf,
          denoise, denoiseQuality, autoLevel, declip, enhance, dereverb,
          hpfCutoff: hpfCutoff || 80, normalizeLufs: normalizeLufs || -16,
          normalizeTp: normalizeTp || -1.5, silenceThresh: silenceThresh || -50,
          ffmpegTimeout: ffmpegTimeout || 300, maxFileSizeGb: maxFileSizeGb || 2,
          caseName: caseName || null
        }}).catch(e => {
          setJobs(prev => ({ ...prev, [file.path]: { ...prev[file.path], status:'error', error: String(e) } }))
          settle()
        })
      })
    }

    await unlistenProg(); await unlistenDone(); await unlistenErr()
    convertingRef.current = false
    setConverting(false)
    invoke('library_get').then(setCases).catch(() => {})
  }, [])

  const doneCount = Object.values(jobs).filter(j => j.status === 'done').length
  const failCount = Object.values(jobs).filter(j => j.status === 'error').length

  return { jobs, setJobs, converting, startConversion, doneCount, failCount }
}
