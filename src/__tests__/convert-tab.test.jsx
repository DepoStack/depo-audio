import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import ConvertTab from '../components/Convert/ConvertTab'

const preferencesContext = vi.hoisted(() => ({ current: null }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../hooks/PreferencesContext', () => ({
  usePreferencesContext: () => preferencesContext.current,
}))

function preferences() {
  return {
    mode: 'stereo',
    setMode: vi.fn(),
    formatOut: 'wav',
    setFormatOut: vi.fn(),
    labels: [],
    setLabels: vi.fn(),
    chanVols: [],
    setChanVols: vi.fn(),
    outDir: '',
    setOutDir: vi.fn(),
    rate: '48000',
    setRate: vi.fn(),
    mp3Bitrate: 192,
    setMp3Bitrate: vi.fn(),
    normalize: false,
    setNormalize: vi.fn(),
    trim: false,
    setTrim: vi.fn(),
    fade: false,
    setFade: vi.fn(),
    fadeDur: 0.5,
    setFadeDur: vi.fn(),
    hpf: false,
    setHpf: vi.fn(),
    denoise: false,
    setDenoise: vi.fn(),
    denoiseQuality: 'fast',
    setDenoiseQuality: vi.fn(),
    autoLevel: false,
    setAutoLevel: vi.fn(),
    declip: false,
    setDeclip: vi.fn(),
    enhance: false,
    setEnhance: vi.fn(),
    dereverb: false,
    setDereverb: vi.fn(),
  }
}

const firstFile = { path: 'C:\\case\\first.trm', name: 'first.trm', fmt: null }

function props(files = [firstFile]) {
  return {
    capabilities: { dereverbAvailable: false },
    files,
    dragOver: false,
    caseName: '',
    setCaseName: vi.fn(),
    onDragOver: vi.fn(),
    onDragLeave: vi.fn(),
    onDrop: vi.fn(),
    browseFiles: vi.fn(),
    browseOutDir: vi.fn(),
    removeFile: vi.fn(),
    clearAll: vi.fn(),
    jobs: {},
    converting: false,
    startConversion: vi.fn(),
    cancelConversion: vi.fn(),
    doneCount: 0,
    failCount: 0,
  }
}

describe('ConvertTab scan cancellation', () => {
  afterEach(cleanup)

  beforeEach(() => {
    preferencesContext.current = preferences()
    invoke.mockReset()
    invoke.mockResolvedValue()
    listen.mockReset()
    listen.mockResolvedValue(vi.fn())
  })

  it('keeps a failed user cancellation visible and retryable', async () => {
    let cancelAttempts = 0
    invoke.mockImplementation(command => {
      if (command === 'analyze_audio_cmd') return new Promise(() => {})
      if (command === 'cancel_scan_cmd') {
        cancelAttempts += 1
        return cancelAttempts === 1 ? Promise.reject(new Error('cancel IPC unavailable')) : Promise.resolve()
      }
      return Promise.resolve()
    })
    render(<ConvertTab {...props()} />)

    fireEvent.click(screen.getByRole('button', { name: 'Scan' }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('analyze_audio_cmd', { path: firstFile.path }))

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('Could not stop the scan'))
    expect(screen.getByRole('alert')).toHaveTextContent('cancel IPC unavailable')
    expect(screen.getByRole('button', { name: 'Retry cancel' })).toBeEnabled()

    fireEvent.click(screen.getByRole('button', { name: 'Retry cancel' }))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Scan' })).toBeEnabled())
    expect(invoke.mock.calls.filter(([command]) => command === 'cancel_scan_cmd')).toHaveLength(2)
  })

  it('keeps Scan blocked when a queue-change generation bump fails', async () => {
    let resolveAnalysis
    let cancellationAllowed = false
    invoke.mockImplementation(command => {
      if (command === 'analyze_audio_cmd') {
        return new Promise(resolve => {
          resolveAnalysis = resolve
        })
      }
      if (command === 'cancel_scan_cmd') {
        return cancellationAllowed ? Promise.resolve() : Promise.reject(new Error('epoch bump failed'))
      }
      return Promise.resolve()
    })
    const view = render(<ConvertTab {...props()} />)

    fireEvent.click(screen.getByRole('button', { name: 'Scan' }))
    await waitFor(() => expect(resolveAnalysis).toBeTypeOf('function'))
    view.rerender(
      <ConvertTab {...props([{ path: 'C:\\case\\replacement.trm', name: 'replacement.trm', fmt: null }])} />,
    )
    resolveAnalysis({ recommendations: [] })

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('active scan could not be stopped'))
    expect(screen.getByRole('alert')).toHaveTextContent('epoch bump failed')
    expect(screen.queryByRole('button', { name: 'Scan' })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Retry cancel' })).toBeEnabled()

    cancellationAllowed = true
    fireEvent.click(screen.getByRole('button', { name: 'Retry cancel' }))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Scan' })).toBeEnabled())
  })

  it('omits unavailable hardware measurements from the performance summary', () => {
    render(
      <ConvertTab
        {...props()}
        capabilities={{
          dereverbAvailable: false,
          tier: 'low',
          cpuCores: null,
          ramMb: null,
          appleSilicon: false,
        }}
      />,
    )

    expect(screen.getByText(/Lightweight mode/)).not.toHaveAttribute('title')
  })

  it('includes only measured hardware details in the performance summary', () => {
    render(
      <ConvertTab
        {...props()}
        capabilities={{
          dereverbAvailable: false,
          tier: 'high',
          cpuCores: 12,
          ramMb: 32768,
          appleSilicon: true,
        }}
      />,
    )

    expect(screen.getByText(/High performance/)).toHaveAttribute('title', '12 cores · 32 GB RAM · Apple Silicon')
  })
})
