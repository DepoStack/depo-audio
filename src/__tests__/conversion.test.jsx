import { act, renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { open } from '@tauri-apps/plugin-dialog'
import useConversion, { cancelQueuedJobs } from '../hooks/useConversion'
import useFileDrop, { AUDIO_PROBE_CONCURRENCY } from '../hooks/useFileDrop'
import { countHiddenProcessingOptions } from '../components/Convert/ConvertTab'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

const listeners = new Map()

function emit(event, payload) {
  for (const callback of listeners.get(event) || []) callback({ payload })
}

function conversionOptions(files, overrides = {}) {
  return {
    files,
    outDir: '',
    mode: 'stereo',
    formatOut: 'wav',
    rate: '48000',
    mp3Bitrate: 192,
    labels: [],
    chanVols: [],
    normalize: false,
    trim: false,
    fade: false,
    fadeDur: 0.5,
    hpf: false,
    denoise: false,
    denoiseQuality: 'fast',
    autoLevel: false,
    declip: false,
    enhance: false,
    dereverb: false,
    hpfCutoff: 80,
    normalizeLufs: -16,
    normalizeTp: -1.5,
    silenceThresh: -50,
    ffmpegTimeout: 300,
    maxFileSizeGb: 2,
    caseName: '',
    setCases: vi.fn(),
    onLibraryError: vi.fn(),
    ...overrides,
  }
}

describe('conversion cancellation', () => {
  beforeEach(() => {
    listeners.clear()
    invoke.mockReset()
    listen.mockReset()
    open.mockReset()
    open.mockResolvedValue(null)
    listen.mockImplementation(async (event, callback) => {
      if (!listeners.has(event)) listeners.set(event, new Set())
      listeners.get(event).add(callback)
      return () => listeners.get(event)?.delete(callback)
    })
    invoke.mockImplementation(command => {
      if (command === 'begin_conversion_batch_cmd') return Promise.resolve(42)
      if (command === 'convert') return new Promise(() => {})
      if (command === 'library_get') return Promise.resolve([])
      return Promise.resolve(43)
    })
  })

  it('marks only queued work cancelled in the pure queue transition', () => {
    expect(
      cancelQueuedJobs({
        active: { status: 'converting' },
        later: { status: 'queued' },
        complete: { status: 'done' },
      }),
    ).toMatchObject({
      active: { status: 'converting' },
      later: { status: 'cancelled' },
      complete: { status: 'done' },
    })
  })

  it('cancels the active backend job and never dispatches the remaining queue', async () => {
    const files = [
      { path: 'C:\\case\\first.trm', name: 'first.trm' },
      { path: 'C:\\case\\second.trm', name: 'second.trm' },
    ]
    const { result } = renderHook(() => useConversion())

    let batch
    act(() => {
      batch = result.current.startConversion(conversionOptions(files))
    })
    await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === 'convert')).toHaveLength(1))

    const activeJob = invoke.mock.calls.find(([command]) => command === 'convert')[1].job
    expect(activeJob.cancelGeneration).toBe(42)

    await act(async () => result.current.cancelConversion())
    expect(invoke.mock.calls.some(([command]) => command === 'cancel_conversion_cmd')).toBe(true)
    expect(result.current.jobs[files[1].path].status).toBe('cancelled')

    act(() => emit('convert:cancelled', { id: activeJob.id, message: 'Conversion cancelled' }))
    await act(async () => batch)

    expect(invoke.mock.calls.filter(([command]) => command === 'convert')).toHaveLength(1)
    expect(result.current.jobs[files[0].path].status).toBe('cancelled')
    expect(result.current.jobs[files[1].path].status).toBe('cancelled')
    expect(result.current.converting).toBe(false)
  })

  it('allows the active cancellation signal to be retried without restarting queued work', async () => {
    let cancelAttempts = 0
    invoke.mockImplementation(command => {
      if (command === 'begin_conversion_batch_cmd') return Promise.resolve(42)
      if (command === 'convert') return new Promise(() => {})
      if (command === 'cancel_conversion_cmd') {
        cancelAttempts += 1
        return cancelAttempts === 1 ? Promise.reject(new Error('IPC unavailable')) : Promise.resolve(43)
      }
      if (command === 'library_get') return Promise.resolve([])
      return Promise.resolve(43)
    })

    const files = [
      { path: 'C:\\case\\first.trm', name: 'first.trm' },
      { path: 'C:\\case\\second.trm', name: 'second.trm' },
    ]
    const { result } = renderHook(() => useConversion())

    let batch
    act(() => {
      batch = result.current.startConversion(conversionOptions(files))
    })
    await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === 'convert')).toHaveLength(1))
    const activeJob = invoke.mock.calls.find(([command]) => command === 'convert')[1].job

    await act(async () => result.current.cancelConversion())
    expect(result.current.cancelError).toContain('IPC unavailable')
    expect(result.current.cancelling).toBe(false)

    await act(async () => result.current.cancelConversion())
    expect(invoke.mock.calls.filter(([command]) => command === 'cancel_conversion_cmd')).toHaveLength(2)
    expect(result.current.cancelError).toBe('')
    expect(result.current.cancelling).toBe(true)

    act(() => emit('convert:cancelled', { id: activeJob.id, message: 'Conversion cancelled' }))
    await act(async () => batch)

    expect(invoke.mock.calls.filter(([command]) => command === 'convert')).toHaveLength(1)
    expect(result.current.jobs[files[1].path].status).toBe('cancelled')
  })

  it('leaves an empty output directory for the backend to resolve from a Windows drive-root source', async () => {
    const file = { path: 'C:\\hearing.wav', name: 'hearing.wav' }
    const { result } = renderHook(() => useConversion())

    let batch
    act(() => {
      batch = result.current.startConversion(conversionOptions([file]))
    })
    await waitFor(() => expect(invoke.mock.calls.some(([command]) => command === 'convert')).toBe(true))
    const activeJob = invoke.mock.calls.find(([command]) => command === 'convert')[1].job
    expect(activeJob.outDir).toBe('')

    act(() => emit('convert:done', { id: activeJob.id, files: [] }))
    await act(async () => batch)
  })

  it('sends no learned-model fields in the IPC payload, even when asked to', async () => {
    const file = { path: 'C:\\case\\source.wav', name: 'source.wav' }
    const { result } = renderHook(() => useConversion())

    let batch
    act(() => {
      batch = result.current.startConversion(
        conversionOptions([file], { denoise: true, denoiseQuality: 'best', enhance: true, dereverb: true }),
      )
    })
    await waitFor(() => expect(invoke.mock.calls.some(([command]) => command === 'convert')).toBe(true))
    const activeJob = invoke.mock.calls.find(([command]) => command === 'convert')[1].job

    for (const field of ['denoise', 'denoiseQuality', 'enhance', 'dereverb'])
      expect(activeJob).not.toHaveProperty(field)
    act(() => emit('convert:done', { id: activeJob.id, files: [] }))
    await act(async () => batch)
  })

  it('resets completed batch jobs whenever the unlocked file queue changes', async () => {
    const first = 'C:\\case\\first.wav'
    const second = 'C:\\case\\second.wav'
    const dropOverrideRef = { current: null }
    const { result } = renderHook(() => ({
      conversion: useConversion(),
      queue: useFileDrop(dropOverrideRef),
    }))

    await act(async () => result.current.queue.addFiles([first]))
    act(() => result.current.conversion.setJobs({ [first]: { status: 'done' } }))
    expect(result.current.conversion.doneCount).toBe(1)

    await act(async () => result.current.queue.addFiles([second]))
    expect(result.current.conversion.jobs).toEqual({})

    act(() =>
      result.current.conversion.setJobs({
        [first]: { status: 'done' },
        [second]: { status: 'done' },
      }),
    )
    act(() => result.current.queue.removeFile(second, false))
    expect(result.current.conversion.jobs).toEqual({})

    act(() => result.current.conversion.setJobs({ [first]: { status: 'done' } }))
    await act(async () => result.current.queue.addFiles([first], { replace: true }))
    expect(result.current.conversion.jobs).toEqual({})

    act(() => result.current.conversion.setJobs({ [first]: { status: 'done' } }))
    act(() => result.current.queue.clearAll(false))
    expect(result.current.conversion.jobs).toEqual({})
    expect(result.current.conversion.doneCount).toBe(0)
  })

  it('probes each newly queued file for its audio tracks without changing the queue', async () => {
    const info = { convertedIndex: 0, streams: [{ channels: 1 }, { channels: 4 }], durationSecs: 60 }
    invoke.mockImplementation(command => {
      if (command === 'detect_format') return Promise.resolve(null)
      if (command === 'infer_case_name_cmd') return Promise.resolve('')
      if (command === 'probe_audio_streams_cmd') return Promise.resolve(info)
      return Promise.resolve(null)
    })
    const { result } = renderHook(() => useFileDrop({ current: null }))

    await act(async () => result.current.addFiles(['/case/hearing.mkv']))
    const queued = result.current.files
    await waitFor(() => expect(result.current.audioStreams['/case/hearing.mkv']).toEqual(info))
    expect(result.current.files).toBe(queued)
    expect(invoke).toHaveBeenCalledWith('probe_audio_streams_cmd', { path: '/case/hearing.mkv' })

    // Adding the same file again does not probe it again.
    const probes = () => invoke.mock.calls.filter(([command]) => command === 'probe_audio_streams_cmd').length
    await act(async () => result.current.addFiles(['/case/hearing.mkv']))
    expect(probes()).toBe(1)

    act(() => result.current.clearAll(false))
    expect(result.current.audioStreams).toEqual({})
  })

  it('probes a large drop a few files at a time and forgets removed files', async () => {
    const pending = []
    let inFlight = 0
    let peak = 0
    invoke.mockImplementation((command, args) => {
      if (command === 'detect_format') return Promise.resolve(null)
      if (command === 'infer_case_name_cmd') return Promise.resolve('')
      if (command === 'probe_audio_streams_cmd') {
        inFlight += 1
        peak = Math.max(peak, inFlight)
        return new Promise(resolve =>
          pending.push(() => {
            inFlight -= 1
            resolve({ convertedIndex: 0, streams: [{ channels: 4 }], durationSecs: 600, path: args.path })
          }),
        )
      }
      return Promise.resolve(null)
    })
    const { result } = renderHook(() => useFileDrop({ current: null }))
    const chunks = Array.from({ length: 40 }, (_, index) => `/session/chunk-${String(index).padStart(3, '0')}.trm`)

    await act(async () => result.current.addFiles(chunks))
    expect(result.current.files).toHaveLength(40)
    const probes = () => invoke.mock.calls.filter(([command]) => command === 'probe_audio_streams_cmd')
    await waitFor(() => expect(probes()).toHaveLength(AUDIO_PROBE_CONCURRENCY))
    expect(peak).toBe(AUDIO_PROBE_CONCURRENCY)

    // A file removed while its probe waits is never probed, and a file removed
    // while its probe runs never gets a result.
    const running = probes()[0][1].path
    const waiting = chunks[chunks.length - 1]
    act(() => result.current.removeFile(running, false))
    act(() => result.current.removeFile(waiting, false))
    while (pending.length > 0) {
      await act(async () => pending.shift()())
    }
    await waitFor(() => expect(Object.keys(result.current.audioStreams)).toHaveLength(38))
    expect(peak).toBe(AUDIO_PROBE_CONCURRENCY)
    expect(result.current.audioStreams[running]).toBeUndefined()
    expect(probes().map(([, args]) => args.path)).not.toContain(waiting)

    // Removing a probed file drops its stale result.
    const probed = chunks[1]
    expect(result.current.audioStreams[probed]).toBeDefined()
    act(() => result.current.removeFile(probed, false))
    expect(result.current.audioStreams[probed]).toBeUndefined()
  })

  it('queues files with a visible warning when format detection fails', async () => {
    invoke.mockImplementation(command => {
      if (command === 'detect_format') return Promise.reject(new Error('probe unavailable'))
      if (command === 'infer_case_name_cmd') return Promise.resolve('')
      return Promise.resolve([])
    })
    const { result } = renderHook(() => useFileDrop({ current: null }))

    await act(async () => result.current.addFiles(['C:\\case\\unknown.trm']))

    expect(result.current.files).toHaveLength(1)
    expect(result.current.files[0].fmt).toBeNull()
    expect(result.current.queueNotice).toContain('Could not inspect unknown.trm')
    expect(result.current.queueNotice).toContain('probe unavailable')
  })

  it('keeps picker cancellation quiet but surfaces picker and output-folder failures', async () => {
    const { result } = renderHook(() => useFileDrop({ current: null }))
    const setOutDir = vi.fn()

    await act(async () => result.current.browseFiles())
    expect(result.current.queueNotice).toBe('')
    await act(async () => result.current.browseOutDir(setOutDir))
    expect(result.current.queueNotice).toBe('')
    expect(setOutDir).not.toHaveBeenCalled()

    open.mockRejectedValueOnce(new Error('file dialog unavailable'))
    await act(async () => result.current.browseFiles())
    expect(result.current.queueNotice).toContain('Could not open the file picker')
    expect(result.current.queueNotice).toContain('file dialog unavailable')

    open.mockRejectedValueOnce(new Error('folder dialog unavailable'))
    await act(async () => result.current.browseOutDir(setOutDir))
    expect(result.current.queueNotice).toContain('Could not open the output-folder picker')
    expect(result.current.queueNotice).toContain('folder dialog unavailable')
  })

  it('offers the legacy deposition video containers in the file picker', async () => {
    const { result } = renderHook(() => useFileDrop({ current: null }))
    open.mockResolvedValueOnce(null)

    await act(async () => result.current.browseFiles())

    const filters = open.mock.calls.at(-1)[0].filters
    const video = filters.find(filter => filter.name === 'Video (audio extracted)')
    for (const extension of ['mpg', 'mpeg', 'vob', 'ts', 'mts', 'm2ts', 'wmv', 'asf']) {
      expect(video.extensions).toContain(extension)
    }
    expect(filters.flatMap(filter => filter.extensions)).not.toContain('ds2')
  })

  it('counts dropped FTR files as chunks in a TRS companion notice', async () => {
    const { result } = renderHook(() => useFileDrop({ current: null }))

    await act(async () => result.current.addFiles(['C:\\case\\session.trs', 'C:\\case\\part-1.ftr']))

    expect(result.current.files.map(file => file.path)).toEqual(['C:\\case\\part-1.ftr'])
    expect(result.current.queueNotice).toContain('All 1 dropped FTR chunk is queued')
  })

  it('keeps valid outputs visible when library filing finishes with a warning', async () => {
    const file = { path: '/case/source.wav', name: 'source.wav' }
    const onLibraryError = vi.fn()
    const { result } = renderHook(() => useConversion())

    let batch
    act(() => {
      batch = result.current.startConversion(conversionOptions([file], { onLibraryError }))
    })
    await waitFor(() => expect(invoke.mock.calls.some(([command]) => command === 'convert')).toBe(true))
    const activeJob = invoke.mock.calls.find(([command]) => command === 'convert')[1].job
    const output = { path: '/case/source-1.wav', name: 'source-1.wav', size: 100 }

    act(() =>
      emit('convert:done', {
        id: activeJob.id,
        files: [output],
        warning: 'The audio was converted, but the library could not be updated.',
        libraryWarning: true,
      }),
    )
    await act(async () => batch)

    expect(result.current.jobs[file.path]).toMatchObject({
      status: 'done',
      outputs: [output],
      warning: 'The audio was converted, but the library could not be updated.',
    })
    expect(onLibraryError).toHaveBeenCalledWith('The audio was converted, but the library could not be updated.')
  })
})

describe('conversion results', () => {
  beforeEach(() => {
    listeners.clear()
    invoke.mockReset()
    listen.mockReset()
    listen.mockImplementation(async (event, callback) => {
      if (!listeners.has(event)) listeners.set(event, new Set())
      listeners.get(event).add(callback)
      return () => listeners.get(event)?.delete(callback)
    })
    invoke.mockImplementation(command => {
      if (command === 'begin_conversion_batch_cmd') return Promise.resolve(42)
      if (command === 'convert') return new Promise(() => {})
      if (command === 'library_get') return Promise.resolve([])
      return Promise.resolve(43)
    })
  })

  it.each([
    ['stereo', true, true],
    ['keep', true, true],
    ['split', true, false],
    ['stereo', false, false],
  ])('records whether leading-silence trim ran (%s, trim %s)', async (mode, trim, trimmed) => {
    const file = { path: '/case/source.wav', name: 'source.wav' }
    const { result } = renderHook(() => useConversion())
    let batch
    act(() => {
      batch = result.current.startConversion(conversionOptions([file], { mode, trim }))
    })
    await waitFor(() => expect(invoke.mock.calls.some(([command]) => command === 'convert')).toBe(true))
    const activeJob = invoke.mock.calls.find(([command]) => command === 'convert')[1].job
    act(() => emit('convert:done', { id: activeJob.id, files: [{ path: '/case/out.wav', name: 'out.wav', size: 1 }] }))
    await act(async () => batch)
    expect(result.current.jobs[file.path]).toMatchObject({ status: 'done', trimmed })
  })
})

describe('conversion processing UI contracts', () => {
  it('counts only released non-learned processing options', () => {
    const allHidden = {
      hasAnalysis: true,
      showAllProcessing: false,
      showAutoLevel: false,
      showDeclip: false,
      showHpf: false,
      showNormalize: false,
      showTrim: false,
      showFade: false,
    }

    expect(countHiddenProcessingOptions(allHidden)).toBe(6)
  })
})
