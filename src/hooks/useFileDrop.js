import { useState, useCallback, useEffect, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { basename } from '../utils'

export default function useFileDrop(dropOverrideRef) {
  const [files, setFiles]       = useState([])
  const [dragOver, setDragOver] = useState(false)
  const [caseName, setCaseName] = useState('')

  const filesRef    = useRef(files)
  const caseNameRef = useRef(caseName)
  useEffect(() => { filesRef.current = files }, [files])
  useEffect(() => { caseNameRef.current = caseName }, [caseName])

  const addFiles = useCallback(async (paths) => {
    const filtered = paths.filter(p => !p.toLowerCase().endsWith('.trs'))
    const next = []
    for (const p of filtered) {
      if (filesRef.current.some(f => f.path === p)) continue
      const fmt = await invoke('detect_format', { path: p }).catch(() => null)
      next.push({ path: p, name: basename(p), fmt })
      if (!caseNameRef.current && next.length === 1) {
        const detected = await invoke('infer_case_name_cmd', { filename: basename(p) }).catch(() => '')
        setCaseName(detected)
      }
    }
    if (next.length) setFiles(prev => [...prev, ...next])
  }, [])

  const removeFile = useCallback((path, converting) => {
    if (converting) return
    setFiles(p => { const next = p.filter(f => f.path !== path); if (next.length === 0) setCaseName(''); return next })
  }, [])

  const clearAll = useCallback((converting) => {
    if (!converting) { setFiles([]); setCaseName('') }
  }, [])

  // Tauri native drag-drop
  useEffect(() => {
    const unlisten = listen('tauri://drag-drop', (event) => {
      setDragOver(false)
      if (!event.payload?.paths?.length) return
      // A mounted tab (e.g. Player) may claim drops for itself
      if (dropOverrideRef?.current) dropOverrideRef.current(event.payload.paths)
      else addFiles(event.payload.paths)
    })
    // Resolve the promise in cleanup so a listener registered after a fast
    // unmount (e.g. StrictMode double-mount) is still removed
    return () => { unlisten.then(fn => fn()).catch(() => {}) }
  }, [addFiles, dropOverrideRef])

  const onDragOver  = (e) => { e.preventDefault(); setDragOver(true) }
  const onDragLeave = () => setDragOver(false)
  const onDrop      = (e) => { e.preventDefault(); setDragOver(false) }

  const browseFiles = async () => {
    const selected = await openDialog({ multiple: true, filters: [
      { name: 'Audio', extensions: ['mp3','wav','flac','m4a','aac','ogg','opus','wma','aif','aiff','sgmca','trm','ftr','bwf','dm'] },
      { name: 'All Files', extensions: ['*'] }
    ]}).catch(() => null)
    if (selected) await addFiles(Array.isArray(selected) ? selected : [selected])
  }

  const browseOutDir = async (setOutDir) => {
    const dir = await openDialog({ directory: true }).catch(() => null)
    if (dir) setOutDir(dir)
  }

  return {
    files, setFiles,
    dragOver,
    caseName, setCaseName,
    addFiles, removeFile, clearAll,
    onDragOver, onDragLeave, onDrop,
    browseFiles, browseOutDir,
  }
}
