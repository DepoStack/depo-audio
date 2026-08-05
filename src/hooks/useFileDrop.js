import { useState, useCallback, useEffect, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { basename, sortRecordingChunks } from '../utils'
import { mergeQueuedFiles, partitionQueuePaths } from '../lib/queue'
import { notifyConversionQueueChanged } from './useConversion'

export default function useFileDrop(dropOverrideRef, queueLocked = false) {
  const [files, setFiles] = useState([])
  const [dragOver, setDragOver] = useState(false)
  const [caseName, setCaseName] = useState('')
  const [queueNotice, setQueueNotice] = useState('')

  const filesRef = useRef(files)
  const caseNameRef = useRef(caseName)
  const queueLockedRef = useRef(queueLocked)
  const addQueueRef = useRef(Promise.resolve())
  useEffect(() => {
    filesRef.current = files
  }, [files])
  useEffect(() => {
    caseNameRef.current = caseName
  }, [caseName])
  useEffect(() => {
    queueLockedRef.current = queueLocked
  }, [queueLocked])

  const addFiles = useCallback((paths, options = {}) => {
    const task = async () => {
      if (queueLockedRef.current) return { added: 0, locked: true }

      // Serialize async additions so two native drops/dialog completions cannot
      // both inspect the same stale queue and append duplicate paths.
      const { queuePaths, trsCompanions } = partitionQueuePaths(paths)
      const filtered = sortRecordingChunks(queuePaths)
      const detected = await Promise.all(
        filtered.map(async p => {
          try {
            return {
              file: { path: p, name: basename(p), fmt: await invoke('detect_format', { path: p }) },
              error: null,
            }
          } catch (error) {
            return { file: { path: p, name: basename(p), fmt: null }, error }
          }
        }),
      )
      const next = detected.map(result => result.file)
      const formatFailures = detected.filter(result => result.error)

      // A conversion may have started while format detection was in flight.
      if (queueLockedRef.current) return { added: 0, locked: true }

      const previous = filesRef.current
      const merged = mergeQueuedFiles(previous, next, { replace: !!options.replace })
      const added = options.replace ? merged.length : Math.max(0, merged.length - previous.length)
      const queueChanged =
        !!options.replace ||
        merged.length !== previous.length ||
        merged.some((file, index) => file.path !== previous[index]?.path)
      filesRef.current = merged
      setFiles(merged)
      if (queueChanged) notifyConversionQueueChanged()

      const notices = []
      if (trsCompanions.length > 0) {
        const chunkCount = filtered.filter(path => /\.(?:ftr|trm)$/i.test(path)).length
        notices.push(
          chunkCount > 0
            ? `TRS session companion detected. All ${chunkCount} dropped FTR/TRM ${chunkCount === 1 ? 'chunk is' : 'chunks are'} queued in chronological order, but TRS session metadata and protection validation are not yet supported.`
            : 'TRS session companion detected. TRS session metadata and protection validation are not yet supported, so select or drop the accompanying FTR/TRM chunks directly.',
        )
      }
      if (formatFailures.length > 0) {
        const failedNames = formatFailures.map(result => result.file.name)
        notices.push(
          `Could not inspect ${failedNames.length === 1 ? failedNames[0] : `${failedNames.length} files`}: ${String(formatFailures[0].error)}. The ${failedNames.length === 1 ? 'file was' : 'files were'} queued, but format support could not be verified.`,
        )
      }
      if (options.replace || notices.length > 0) setQueueNotice(notices.join(' '))

      if (!caseNameRef.current && merged.length > 0) {
        const detected = await invoke('infer_case_name_cmd', { filename: merged[0].name }).catch(() => '')
        if (detected && !caseNameRef.current) {
          caseNameRef.current = detected
          setCaseName(detected)
        }
      }
      return { added, locked: false }
    }

    const queued = addQueueRef.current.then(task, task)
    addQueueRef.current = queued.catch(() => {})
    return queued
  }, [])

  const removeFile = useCallback((path, converting) => {
    if (converting || queueLockedRef.current) return
    const previous = filesRef.current
    const next = previous.filter(f => f.path !== path)
    if (next.length === previous.length) return
    filesRef.current = next
    setFiles(next)
    notifyConversionQueueChanged()
    if (next.length === 0) {
      caseNameRef.current = ''
      setCaseName('')
      setQueueNotice('')
    }
  }, [])

  const clearAll = useCallback(converting => {
    if (!converting && !queueLockedRef.current) {
      const hadFiles = filesRef.current.length > 0
      filesRef.current = []
      caseNameRef.current = ''
      setFiles([])
      setCaseName('')
      setQueueNotice('')
      if (hadFiles) notifyConversionQueueChanged()
    }
  }, [])

  // Tauri native drag-drop
  useEffect(() => {
    const unlisten = listen('tauri://drag-drop', event => {
      setDragOver(false)
      if (!event.payload?.paths?.length) return
      // A mounted tab (e.g. Player) may claim drops for itself
      if (dropOverrideRef?.current) dropOverrideRef.current(event.payload.paths)
      else if (!queueLockedRef.current) addFiles(event.payload.paths)
    })
    // Resolve the promise in cleanup so a listener registered after a fast
    // unmount (e.g. StrictMode double-mount) is still removed
    return () => {
      unlisten.then(fn => fn()).catch(() => {})
    }
  }, [addFiles, dropOverrideRef])

  const onDragOver = e => {
    e.preventDefault()
    if (!queueLockedRef.current) setDragOver(true)
  }
  const onDragLeave = () => setDragOver(false)
  const onDrop = e => {
    e.preventDefault()
    setDragOver(false)
  }

  const browseFiles = async () => {
    if (queueLockedRef.current) return { added: 0, locked: true }
    let selected
    try {
      selected = await openDialog({
        multiple: true,
        filters: [
          {
            name: 'Audio',
            extensions: [
              'mp3',
              'wav',
              'flac',
              'm4a',
              'aac',
              'ogg',
              'opus',
              'wma',
              'aif',
              'aiff',
              'caf',
              'amr',
              '3ga',
              'sgmca',
              'trm',
              'ftr',
              'bwf',
              'dm',
            ],
          },
          { name: 'Video (audio extracted)', extensions: ['mp4', 'mov', 'mkv', 'avi', 'webm', 'm4v', '3gp'] },
          { name: 'All Files', extensions: ['*'] },
        ],
      })
    } catch (error) {
      setQueueNotice(`Could not open the file picker: ${String(error)}`)
      return { added: 0, locked: false, error: String(error) }
    }
    if (selected) return addFiles(Array.isArray(selected) ? selected : [selected])
    return { added: 0, locked: false }
  }

  const browseOutDir = async setOutDir => {
    if (queueLockedRef.current) return
    let dir
    try {
      dir = await openDialog({ directory: true })
    } catch (error) {
      setQueueNotice(`Could not open the output-folder picker: ${String(error)}`)
      return
    }
    if (dir) setOutDir(dir)
  }

  return {
    files,
    setFiles,
    dragOver,
    caseName,
    setCaseName,
    queueNotice,
    addFiles,
    removeFile,
    clearAll,
    onDragOver,
    onDragLeave,
    onDrop,
    browseFiles,
    browseOutDir,
  }
}
