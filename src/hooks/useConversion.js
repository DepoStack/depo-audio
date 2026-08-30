import { useState, useCallback, useEffect, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

let jobCounter = 0
const CANCELLED_MESSAGE = 'Conversion cancelled'
const QUEUE_CHANGED_EVENT = 'depo-audio:conversion-queue-changed'

export function notifyConversionQueueChanged() {
  if (typeof window !== 'undefined') window.dispatchEvent(new Event(QUEUE_CHANGED_EVENT))
}

export function cancelQueuedJobs(jobs) {
  return Object.fromEntries(
    Object.entries(jobs).map(([path, job]) => [
      path,
      job.status === 'queued'
        ? { ...job, status: 'cancelled', error: 'Cancelled before this file started. No output was created.' }
        : job,
    ]),
  )
}

export default function useConversion() {
  const [jobs, setJobs] = useState({})
  const [converting, setConverting] = useState(false)
  const [cancelling, setCancelling] = useState(false)
  const [cancelError, setCancelError] = useState('')
  const convertingRef = useRef(false)
  const cancelRequestedRef = useRef(false)
  const cancelSignalSentRef = useRef(false)

  // Queue changes invalidate every result from the previous batch, including
  // same-path replacements. useFileDrop publishes this only while unlocked.
  useEffect(() => {
    const resetBatchResults = () => {
      if (convertingRef.current) return
      cancelRequestedRef.current = false
      cancelSignalSentRef.current = false
      setCancelError('')
      setJobs({})
    }

    window.addEventListener(QUEUE_CHANGED_EVENT, resetBatchResults)
    return () => window.removeEventListener(QUEUE_CHANGED_EVENT, resetBatchResults)
  }, [])

  const cancelConversion = useCallback(async () => {
    if (!convertingRef.current || cancelSignalSentRef.current) return
    // Set the local guard before the IPC round-trip so the batch loop can
    // never dispatch another queued job while cancellation is in flight.
    cancelRequestedRef.current = true
    cancelSignalSentRef.current = true
    setCancelling(true)
    setCancelError('')
    setJobs(cancelQueuedJobs)

    try {
      await invoke('cancel_conversion_cmd')
    } catch (error) {
      // The client-side guard still prevents subsequent jobs from starting.
      // The active command will settle normally if the backend could not be
      // reached, and its row remains truthful rather than being faked done.
      // Clear only the signal guard so the user can retry stopping that job;
      // the batch-stop guard remains set and no queued job can start.
      cancelSignalSentRef.current = false
      setCancelling(false)
      setCancelError(
        `The active conversion could not be stopped: ${String(error)}. It may finish, but no additional queued files will start.`,
      )
    }
  }, [])

  const startConversion = useCallback(
    async ({
      files,
      outDir,
      mode,
      formatOut,
      rate,
      mp3Bitrate,
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
      maxFileSizeGb,
      caseName,
      setCases,
      onLibraryError,
    }) => {
      if (convertingRef.current || !files.length) return
      convertingRef.current = true
      cancelRequestedRef.current = false
      cancelSignalSentRef.current = false
      setConverting(true)
      setCancelling(false)
      setCancelError('')
      setJobs(Object.fromEntries(files.map(file => [file.path, { status: 'queued', outputs: [], error: null }])))

      let unlistenProgress = null
      try {
        const cancelGeneration = await invoke('begin_conversion_batch_cmd')

        // Cancel can be clicked while the begin command is crossing IPC. Bump
        // the epoch once more and exit without dispatching the first file.
        if (cancelRequestedRef.current) {
          await invoke('cancel_conversion_cmd').catch(() => {})
          return
        }

        unlistenProgress = await listen('convert:progress', ({ payload }) => {
          setJobs(previous => {
            const match = Object.entries(previous).find(([, job]) => job.id === payload.id)
            if (!match) return previous
            return {
              ...previous,
              [match[0]]: {
                ...match[1],
                seconds: payload.seconds,
                phase: payload.phase || null,
                total: payload.total ?? match[1].total,
              },
            }
          })
        })

        for (const file of files) {
          if (cancelRequestedRef.current) break

          const id = `job_${++jobCounter}`
          setJobs(previous => ({
            ...previous,
            [file.path]: { ...previous[file.path], status: 'converting', id },
          }))

          // Empty is an intentional backend sentinel: Rust resolves it to the
          // source parent without browser-side platform/path ambiguities.
          const resolvedOutDir = outDir || ''

          await new Promise(resolve => {
            let settled = false
            let unlisteners = []

            const settle = update => {
              if (settled) return
              settled = true
              setJobs(previous => ({
                ...previous,
                [file.path]: { ...previous[file.path], ...update },
              }))
              for (const unlisten of unlisteners) unlisten()
              resolve()
            }

            Promise.allSettled([
              listen('convert:done', ({ payload }) => {
                if (payload.id !== id) return
                if (payload.libraryWarning && payload.warning) onLibraryError?.(payload.warning)
                settle({
                  status: 'done',
                  outputs: payload.files,
                  warning: payload.warning || null,
                  error: null,
                })
              }),
              listen('convert:error', ({ payload }) => {
                if (payload.id !== id) return
                settle({ status: 'error', error: payload.message })
              }),
              listen('convert:cancelled', ({ payload }) => {
                if (payload.id !== id) return
                settle({ status: 'cancelled', error: payload.message || CANCELLED_MESSAGE })
              }),
            ]).then(registrations => {
              unlisteners = registrations
                .filter(registration => registration.status === 'fulfilled')
                .map(registration => registration.value)
              const failedRegistration = registrations.find(registration => registration.status === 'rejected')
              if (failedRegistration) {
                settle({
                  status: 'error',
                  error: `Could not monitor conversion: ${String(failedRegistration.reason)}`,
                })
                return
              }
              if (cancelRequestedRef.current) {
                settle({ status: 'cancelled', error: 'Cancelled before this file started. No output was created.' })
                return
              }
              invoke('convert', {
                job: {
                  id,
                  cancelGeneration,
                  srcPath: file.path,
                  outDir: resolvedOutDir,
                  mode,
                  format: formatOut,
                  rate: formatOut === 'opus' ? '48000' : rate,
                  mp3Bitrate: mp3Bitrate ?? 192,
                  labels,
                  chanVols,
                  normalize,
                  trim,
                  fade,
                  fadeDur,
                  hpf,
                  // The release boundary is enforced at the final IPC payload
                  // as well as in preferences and controls.
                  denoise: false,
                  denoiseQuality: 'fast',
                  autoLevel,
                  declip,
                  enhance: false,
                  dereverb: false,
                  hpfCutoff: hpfCutoff ?? 80,
                  normalizeLufs: normalizeLufs ?? -16,
                  normalizeTp: normalizeTp ?? -1.5,
                  silenceThresh: silenceThresh ?? -50,
                  ffmpegTimeout: ffmpegTimeout ?? 300,
                  maxFileSizeGb: maxFileSizeGb ?? 2,
                  caseName: caseName || null,
                },
              }).catch(error => {
                settle(
                  cancelRequestedRef.current
                    ? { status: 'cancelled', error: CANCELLED_MESSAGE }
                    : { status: 'error', error: String(error) },
                )
              })
            })
          })
        }
      } catch (error) {
        const message = cancelRequestedRef.current ? CANCELLED_MESSAGE : `Could not start conversion: ${String(error)}`
        setJobs(previous =>
          Object.fromEntries(
            Object.entries(previous).map(([path, job]) => [
              path,
              job.status === 'queued'
                ? {
                    ...job,
                    status: cancelRequestedRef.current ? 'cancelled' : 'error',
                    error: message,
                  }
                : job,
            ]),
          ),
        )
      } finally {
        if (cancelRequestedRef.current) setJobs(cancelQueuedJobs)
        unlistenProgress?.()
        convertingRef.current = false
        cancelSignalSentRef.current = false
        setConverting(false)
        setCancelling(false)
        invoke('library_get').then(setCases).catch(onLibraryError)
      }
    },
    [],
  )

  const doneCount = Object.values(jobs).filter(job => job.status === 'done').length
  const failCount = Object.values(jobs).filter(job => job.status === 'error').length
  const cancelledCount = Object.values(jobs).filter(job => job.status === 'cancelled').length

  return {
    jobs,
    setJobs,
    converting,
    cancelling,
    cancelError,
    startConversion,
    cancelConversion,
    doneCount,
    failCount,
    cancelledCount,
  }
}
